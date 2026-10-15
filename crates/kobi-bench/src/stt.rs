//! Sous-commande `kobi-bench stt` : rejoue un corpus dans un ou plusieurs
//! moteurs et écrit `rapport.json` et `rapport.md`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context};
use clap::{Args, ValueEnum};
use kobi_bench::metrics::evaluate;
use kobi_bench::metrics::reference::{common_lang, load_jsonl, timed_words, RefUtterance};
use kobi_bench::metrics::resources::ResourceMonitor;
use kobi_bench::report::{now_rfc3339, Report, ReportRow};
use kobi_stt::fake::FakeEngine;
use kobi_stt::replay::{Track, WavReplayer};
use kobi_stt::wav::read_wav_16k_mono;
use kobi_stt::{Source, SttEngine, SAMPLE_RATE};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Device {
    Cpu,
    Cuda,
}

impl Device {
    fn label(self) -> &'static str {
        match self {
            Device::Cpu => "cpu",
            Device::Cuda => "cuda",
        }
    }
}

#[derive(Debug, Args)]
pub struct SttArgs {
    /// Moteur à mesurer (répétable) : voir `--help` pour la liste.
    #[arg(long = "engine", required = true, value_name = "MOTEUR")]
    engines: Vec<String>,

    /// Dossier d'une réunion (moi.wav, eux.wav, reference.jsonl) ou dossier
    /// qui contient plusieurs réunions.
    #[arg(long, value_name = "DOSSIER")]
    corpus: PathBuf,

    /// Ne garder que ces réunions (noms de dossiers, répétable).
    #[arg(long = "meeting", value_name = "NOM")]
    meetings: Vec<String>,

    /// Vitesse de relecture : 1 = temps réel, 0 = sans attente (pas de
    /// délais mesurés).
    #[arg(long, default_value_t = 1.0)]
    speed: f64,

    /// Ne rejouer que les N premières secondes de chaque réunion.
    #[arg(long, value_name = "SECONDES")]
    limit: Option<f64>,

    /// Matériel du moteur.
    #[arg(long, value_enum, default_value_t = Device::Cpu)]
    device: Device,

    /// Dossier des modèles (défaut : ~/Kobi-bench/models).
    #[arg(long, value_name = "DOSSIER")]
    models: Option<PathBuf>,

    /// Dossier du rapport (défaut : ~/Kobi-bench/rapports). Un rapport
    /// existant y est complété.
    #[arg(long, value_name = "DOSSIER")]
    out: Option<PathBuf>,

    /// Nom de la machine dans le rapport (défaut : processeur et mémoire).
    #[arg(long)]
    machine: Option<String>,
}

/// Moteurs compilés dans ce binaire.
pub const ENGINES: &[&str] = &["fake"];

struct Meeting {
    name: String,
    tracks: Vec<Track>,
    reference: Vec<RefUtterance>,
}

pub fn run(args: SttArgs) -> anyhow::Result<()> {
    for e in &args.engines {
        if !ENGINES.contains(&e.as_str()) {
            bail!(
                "moteur inconnu « {e} » ; disponibles : {}",
                ENGINES.join(", ")
            );
        }
    }
    let home = home_dir();
    let models = args
        .models
        .clone()
        .unwrap_or_else(|| home.join("Kobi-bench/models"));
    let out = args
        .out
        .clone()
        .unwrap_or_else(|| home.join("Kobi-bench/rapports"));
    let machine = args.machine.clone().unwrap_or_else(machine_label);

    let meetings = load_corpus(&args.corpus, &args.meetings, args.limit)?;
    let report_path = out.join("rapport.json");
    let mut report = Report::load_or_default(&report_path)
        .with_context(|| format!("lecture de {}", report_path.display()))?;

    for meeting in &meetings {
        for name in &args.engines {
            eprintln!(
                "{name} × {} ({:.0} s d'audio)…",
                meeting.name,
                duration(meeting)
            );
            let monitor = ResourceMonitor::start(Duration::from_millis(250));
            let (mut engine, device) =
                build_engine(name, &models, args.device, &meeting.reference)?;
            let outcome = WavReplayer::new(meeting.tracks.clone())
                .speed(args.speed)
                .run(&mut engine)?;
            let resources = monitor.stop();
            let metrics = evaluate(
                &meeting.reference,
                &outcome,
                resources,
                common_lang(&meeting.reference),
            );
            eprintln!(
                "  WER {:.1} %, {} segments, {:.1} s",
                metrics.wer * 100.0,
                metrics.segments,
                metrics.wall_duration
            );
            report.upsert(ReportRow {
                engine: name.clone(),
                meeting: meeting.name.clone(),
                machine: machine.clone(),
                device,
                date: now_rfc3339(),
                metrics,
            });
            report.write(&out)?;
        }
    }
    eprintln!("rapport : {}", out.join("rapport.md").display());
    Ok(())
}

/// Instancie un moteur ; renvoie aussi le matériel réellement utilisé.
fn build_engine(
    name: &str,
    _models: &Path,
    device: Device,
    reference: &[RefUtterance],
) -> anyhow::Result<(Box<dyn SttEngine>, String)> {
    match name {
        // Restitue la référence avec 0,3 s de retard : valide le harnais.
        "fake" => {
            let _ = device;
            Ok((
                Box::new(FakeEngine::scripted(timed_words(reference), 0.3)),
                Device::Cpu.label().to_string(),
            ))
        }
        _ => bail!("moteur inconnu « {name} »"),
    }
}

fn duration(m: &Meeting) -> f64 {
    m.tracks.iter().map(Track::duration).fold(0.0, f64::max)
}

fn load_corpus(corpus: &Path, only: &[String], limit: Option<f64>) -> anyhow::Result<Vec<Meeting>> {
    let dirs: Vec<PathBuf> = if corpus.join("reference.jsonl").is_file() {
        vec![corpus.to_path_buf()]
    } else {
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(corpus)
            .with_context(|| format!("lecture du corpus {}", corpus.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.join("reference.jsonl").is_file())
            .collect();
        dirs.sort();
        dirs
    };

    let mut meetings = Vec::new();
    for dir in dirs {
        let name = dir
            .file_name()
            .map_or_else(|| "réunion".into(), |n| n.to_string_lossy().into_owned());
        if !only.is_empty() && !only.contains(&name) {
            continue;
        }
        let mut reference = load_jsonl(dir.join("reference.jsonl"))
            .with_context(|| format!("référence de {name}"))?;
        let mut tracks = Vec::new();
        for (source, file) in [(Source::Moi, "moi.wav"), (Source::Eux, "eux.wav")] {
            let path = dir.join(file);
            if path.is_file() {
                let mut samples = read_wav_16k_mono(&path)
                    .with_context(|| format!("lecture de {}", path.display()))?;
                if let Some(limit) = limit {
                    samples.truncate((limit * SAMPLE_RATE as f64) as usize);
                }
                tracks.push(Track { source, samples });
            }
        }
        if tracks.is_empty() {
            bail!("aucune piste moi.wav ou eux.wav dans {}", dir.display());
        }
        if let Some(limit) = limit {
            reference.retain(|u| u.end <= limit);
        }
        meetings.push(Meeting {
            name,
            tracks,
            reference,
        });
    }
    if meetings.is_empty() {
        bail!("aucune réunion trouvée dans {}", corpus.display());
    }
    Ok(meetings)
}

fn home_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
}

/// « Ryzen 5 5500X3D, 32 Go » à partir du processeur et de la mémoire.
fn machine_label() -> String {
    use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};
    let sys = System::new_with_specifics(
        RefreshKind::nothing()
            .with_cpu(CpuRefreshKind::nothing())
            .with_memory(MemoryRefreshKind::nothing().with_ram()),
    );
    let brand = sys.cpus().first().map_or("CPU inconnu", |c| c.brand());
    let mut cpu: Vec<&str> = brand
        .split_whitespace()
        .filter(|w| !matches!(*w, "AMD" | "Intel(R)" | "Core(TM)" | "Processor" | "CPU"))
        .filter(|w| !w.ends_with("-Core") && *w != "@")
        .collect();
    if let Some(i) = cpu.iter().position(|w| w.ends_with("GHz")) {
        cpu.truncate(i);
    }
    let ram_gb = (sys.total_memory() as f64 / 1024f64.powi(3)).round();
    format!("{}, {ram_gb:.0} Go", cpu.join(" "))
}
