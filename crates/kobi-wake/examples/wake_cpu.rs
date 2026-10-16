//! Charge CPU du détecteur openWakeWord « au repos » : l'audio arrive en
//! temps réel par blocs de 80 ms, comme depuis le micro, et on mesure le CPU
//! consommé par le process (en % d'un cœur), le temps de calcul par bloc et
//! la mémoire résidente. Critère du J0 : la machine cible n'a pas de GPU,
//! le détecteur tourne en permanence et doit rester négligeable sur 1 thread.
//!
//! cargo run --release -p kobi-wake --example wake_cpu -- \
//!     [--seconds 60] [--threads 1] [--wav fichier.wav] \
//!     [--features ~/Kobi-bench/models/openwakeword] [--model ~/Kobi-bench/models/kobi-oww/kobi.onnx]
//!
//! Sans `--wav`, le flux est un souffle à peine audible (pièce calme) ; avec
//! `--wav`, le fichier (16 kHz mono) est rejoué en boucle.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use kobi_wake::oww::{OpenWakeWord, OwwConfig};
use kobi_wake::WakeDetector;
use sysinfo::{ProcessesToUpdate, System};

const CHUNK: usize = 1280; // 80 ms à 16 kHz

fn bench_home() -> PathBuf {
    std::env::var_os("KOBI_BENCH_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .or_else(|| std::env::var_os("HOME"))
                .map(|h| PathBuf::from(h).join("Kobi-bench"))
        })
        .expect("ni KOBI_BENCH_HOME ni dossier personnel")
}

struct ProcessCpu {
    sys: System,
    pid: sysinfo::Pid,
}

impl ProcessCpu {
    fn new() -> Self {
        Self {
            sys: System::new(),
            pid: sysinfo::get_current_pid().expect("pid courant"),
        }
    }

    /// (temps CPU cumulé en ms, mémoire résidente en octets)
    fn sample(&mut self) -> (u64, u64) {
        self.sys
            .refresh_processes(ProcessesToUpdate::Some(&[self.pid]), false);
        let p = self.sys.process(self.pid).expect("process courant");
        (p.accumulated_cpu_time(), p.memory())
    }
}

fn main() -> Result<()> {
    let home = bench_home();
    let mut features = home.join("models").join("openwakeword");
    let mut model = home.join("models").join("kobi-oww").join("kobi.onnx");
    let mut seconds = 60.0;
    let mut threads = 1;
    let mut wav: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let v = args.next().context("valeur manquante")?;
        match a.as_str() {
            "--features" => features = v.into(),
            "--model" => model = v.into(),
            "--seconds" => seconds = v.parse()?,
            "--threads" => threads = v.parse()?,
            "--wav" => wav = Some(v.into()),
            _ => anyhow::bail!("option inconnue : {a}"),
        }
    }

    let audio: Vec<i16> = match &wav {
        Some(path) => hound::WavReader::open(path)?
            .samples::<i16>()
            .collect::<Result<_, _>>()?,
        None => {
            let mut seed = 7u32;
            (0..16_000 * 10)
                .map(|_| {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    ((seed >> 16) % 7) as i16 - 3
                })
                .collect()
        }
    };
    anyhow::ensure!(audio.len() >= CHUNK, "audio trop court");

    let mut cpu = ProcessCpu::new();
    let (_, mem_before) = cpu.sample();
    let load = Instant::now();
    let mut config = OwwConfig::new(&features, &model);
    config.threads = threads;
    let mut det = OpenWakeWord::new(&config)?;
    let load_s = load.elapsed().as_secs_f64();

    // Chauffe : 2 s, hors mesure.
    for chunk in audio.chunks(CHUNK).cycle().take(25) {
        det.push(chunk)?;
    }

    let n_chunks = (seconds * 16_000.0 / CHUNK as f64).round() as usize;
    let mut busy = Vec::with_capacity(n_chunks);
    let mut detections = 0;
    let (cpu0, _) = cpu.sample();
    let start = Instant::now();
    for (i, chunk) in audio.chunks_exact(CHUNK).cycle().take(n_chunks).enumerate() {
        let t = Instant::now();
        detections += det.push(chunk)?.len();
        busy.push(t.elapsed());
        // Rythme du micro : un bloc toutes les 80 ms.
        let next = start + Duration::from_millis(80 * (i as u64 + 1));
        if let Some(wait) = next.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
    }
    let wall = start.elapsed().as_secs_f64();
    let (cpu1, mem_after) = cpu.sample();

    busy.sort();
    let mean_ms = busy.iter().map(Duration::as_secs_f64).sum::<f64>() / busy.len() as f64 * 1e3;
    let p99_ms = busy[busy.len() * 99 / 100].as_secs_f64() * 1e3;
    let max_ms = busy[busy.len() - 1].as_secs_f64() * 1e3;
    println!(
        "détecteur : {} sur CPU, {threads} thread(s) ONNX par modèle",
        det.name()
    );
    println!(
        "audio : {}",
        wav.as_ref()
            .map_or("souffle (pièce calme)".into(), |p| p.display().to_string())
    );
    println!(
        "durée : {wall:.1} s en temps réel, {n_chunks} blocs de 80 ms, {detections} détection(s)"
    );
    println!(
        "CPU du process : {:.2} % d'un cœur",
        100.0 * (cpu1 - cpu0) as f64 / 1e3 / wall
    );
    println!(
        "calcul par bloc : moyenne {mean_ms:.2} ms, 99e centile {p99_ms:.2} ms, max {max_ms:.2} ms (budget 80 ms)"
    );
    println!(
        "mémoire résidente : {:.0} Mo (dont {:.0} Mo pour les modèles), chargement {load_s:.2} s",
        mem_after as f64 / 1e6,
        mem_after.saturating_sub(mem_before) as f64 / 1e6
    );
    Ok(())
}
