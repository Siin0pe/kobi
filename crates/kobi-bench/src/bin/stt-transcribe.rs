//! stt-transcribe : transcrit un WAV avec un moteur et mesure le coût brut.
//!
//! Outil d'essai des adaptateurs, sans corpus ni référence (le rapport complet
//! du benchmark est `kobi-bench stt`).
//!
//! ```text
//! stt-transcribe [options] <moi.wav> [--eux <eux.wav>]
//!   --engine <nom>       fake (défaut), nemotron-160ms, nemotron-560ms
//!   --models <dossier>   dossier des modèles (~/Kobi-bench/models)
//!   --model <dossier>    paquet imposé, à la place de la recherche
//!   --lang <code>        langue forcée (fr, en…), détection sinon
//!   --threads <n>        fils d'inférence (4)
//!   --device cpu|cuda    matériel (cpu)
//!   --speed <x>          1 = temps réel, 0 = sans attente (défaut)
//!   --json <fichier>     rapport complet en JSON
//! ```

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use kobi_bench::engines::{self, EngineOptions};
use kobi_bench::metrics::resources::ResourceMonitor;
use kobi_stt::fake::FakeEngine;
use kobi_stt::replay::{Track, WavReplayer};
use kobi_stt::wav::read_wav_16k_mono;
use kobi_stt::{Source, SttEngine};

#[derive(Debug)]
struct Args {
    engine: String,
    opts: EngineOptions,
    speed: f64,
    json: Option<PathBuf>,
    moi: PathBuf,
    eux: Option<PathBuf>,
}

fn parse_args(mut it: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut engine = "fake".to_string();
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map_or_else(|| PathBuf::from("."), PathBuf::from);
    let mut opts = EngineOptions {
        models: home.join("Kobi-bench/models"),
        ..Default::default()
    };
    let (mut speed, mut json, mut moi, mut eux) = (0.0, None, None, None);
    while let Some(arg) = it.next() {
        let mut value = || it.next().ok_or(format!("valeur manquante après {arg}"));
        match arg.as_str() {
            "--engine" => engine = value()?,
            "--models" => opts.models = PathBuf::from(value()?),
            "--model" => opts.model = Some(PathBuf::from(value()?)),
            "--lang" => opts.language = Some(value()?),
            "--threads" => {
                opts.threads = value()?.parse().map_err(|e| format!("--threads : {e}"))?
            }
            "--device" => opts.device = value()?,
            "--speed" => speed = value()?.parse().map_err(|e| format!("--speed : {e}"))?,
            "--json" => json = Some(PathBuf::from(value()?)),
            "--eux" => eux = Some(PathBuf::from(value()?)),
            a if a.starts_with("--") => return Err(format!("option inconnue : {a}")),
            _ if moi.is_none() => moi = Some(PathBuf::from(arg)),
            _ => return Err(format!("argument en trop : {arg}")),
        }
    }
    Ok(Args {
        engine,
        opts,
        speed,
        json,
        moi: moi.ok_or("WAV manquant")?,
        eux,
    })
}

fn run(args: Args) -> Result<(), String> {
    let read = |p: &PathBuf| read_wav_16k_mono(p).map_err(|e| format!("{} : {e}", p.display()));
    let mut tracks = vec![Track {
        source: Source::Moi,
        samples: read(&args.moi)?,
    }];
    if let Some(eux) = &args.eux {
        tracks.push(Track {
            source: Source::Eux,
            samples: read(eux)?,
        });
    }

    let loading = Instant::now();
    let mut engine: Box<dyn SttEngine> = match args.engine.as_str() {
        "fake" => Box::new(FakeEngine::ticking(0.5, 2.0)),
        name => engines::build(name, &args.opts)?,
    };
    let load_s = loading.elapsed().as_secs_f64();

    let monitor = ResourceMonitor::start(Duration::from_millis(250));
    let out = WavReplayer::new(tracks)
        .speed(args.speed)
        .run(&mut engine)
        .map_err(|e| e.to_string())?;
    let resources = monitor.stop();

    for s in out.finals() {
        println!(
            "[{:>7.2} → {:>7.2}] {:?} {}",
            s.start, s.end, s.source, s.text
        );
    }

    let summary = serde_json::json!({
        "engine": out.engine,
        "device": args.opts.device,
        "threads": args.opts.threads,
        "speed": args.speed,
        "audio_s": out.audio_duration,
        "load_s": load_s,
        "wall_s": out.wall_duration,
        "rtf": out.wall_duration / out.audio_duration,
        "segments": out.emissions.len(),
        "finals": out.finals().count(),
        "resources": resources,
    });
    eprintln!("{}", serde_json::to_string_pretty(&summary).expect("JSON"));

    if let Some(path) = &args.json {
        let report = serde_json::json!({ "summary": summary, "replay": out });
        std::fs::write(path, serde_json::to_vec_pretty(&report).expect("JSON"))
            .map_err(|e| format!("{} : {e}", path.display()))?;
    }
    Ok(())
}

fn main() -> ExitCode {
    match parse_args(std::env::args().skip(1)).and_then(run) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("erreur : {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Result<Args, String> {
        parse_args(list.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_options_and_wav() {
        let a = args(&[
            "--engine",
            "nemotron-160ms",
            "--lang",
            "fr",
            "--threads",
            "2",
            "--device",
            "cuda",
            "a.wav",
        ])
        .unwrap();
        assert_eq!(a.engine, "nemotron-160ms");
        assert_eq!(a.opts.language.as_deref(), Some("fr"));
        assert_eq!(a.opts.threads, 2);
        assert_eq!(a.opts.device, "cuda");
        assert_eq!(a.speed, 0.0);
        assert_eq!(a.moi, PathBuf::from("a.wav"));
    }

    #[test]
    fn rejects_missing_wav_and_unknown_option() {
        assert!(args(&["--engine", "fake"]).is_err());
        assert!(args(&["--nope", "a.wav"]).is_err());
    }
}
