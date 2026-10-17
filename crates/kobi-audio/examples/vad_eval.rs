//! Évalue la VAD (Silero v5 + Endpointer) sur les WAV annotés du corpus
//! synthétique (KOB-12) : chaque fin de phrase suivie d'au moins
//! `min_silence` de silence sur sa piste doit être détectée à ± 150 ms.
//!
//! ORT_DYLIB_PATH=…/libonnxruntime.so cargo run --release -p kobi-audio \
//!     --features silero --example vad_eval -- \
//!     [--corpus ~/Kobi-bench/corpus] [--model ~/Kobi-bench/models/silero/silero_vad.onnx]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use kobi_audio::vad::{SileroVad, Vad, VadEvent, VadOptions};
use kobi_audio::{AudioFrame, Source};
use serde::Deserialize;

/// Tolérance du critère de KOB-43.
const TOLERANCE: f64 = 0.150;

#[derive(Deserialize)]
struct Line {
    start: f64,
    end: f64,
    source: String,
}

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

fn main() -> Result<()> {
    let home = bench_home();
    let mut corpus = home.join("corpus");
    let mut model = home.join("models").join("silero").join("silero_vad.onnx");
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let v = args.next().context("valeur manquante")?;
        match a.as_str() {
            "--corpus" => corpus = v.into(),
            "--model" => model = v.into(),
            _ => anyhow::bail!("option inconnue : {a}"),
        }
    }
    let options = VadOptions::default();
    let min_silence = options.min_silence.as_secs_f64();
    let mut meetings: Vec<PathBuf> = std::fs::read_dir(&corpus)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join("reference.jsonl").is_file())
        .collect();
    meetings.sort();

    let (mut total, mut ok) = (0usize, 0usize);
    let mut errors_ms: Vec<f64> = Vec::new();
    let mut audio_s = 0.0;
    let started = Instant::now();
    for meeting in &meetings {
        let name = meeting.file_name().unwrap().to_string_lossy().into_owned();
        let lines: Vec<Line> = std::fs::read_to_string(meeting.join("reference.jsonl"))?
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(serde_json::from_str)
            .collect::<Result<_, _>>()?;
        for (source, file) in [(Source::Moi, "moi.wav"), (Source::Eux, "eux.wav")] {
            let mut reader = hound::WavReader::open(meeting.join(file))?;
            let samples: Vec<f32> = reader
                .samples::<i16>()
                .map(|s| s.map(|s| s as f32 / 32_768.0))
                .collect::<Result<_, _>>()?;
            audio_s += samples.len() as f64 / 16_000.0;
            let mut vad = Vad::new(source, SileroVad::load(&model)?, options.clone());
            let mut ends = Vec::new();
            for (k, chunk) in samples.chunks(320).enumerate() {
                let frame = AudioFrame {
                    source,
                    samples: chunk.to_vec(),
                    t: Duration::from_secs_f64(k as f64 * 0.02),
                };
                for e in vad.push(&frame)? {
                    if let VadEvent::SpeechEnd { end, .. } = e {
                        ends.push(end);
                    }
                }
            }
            if let Some(VadEvent::SpeechEnd { end, .. }) = vad.finish() {
                ends.push(end);
            }
            // Fins de phrase de cette piste suivies d'un silence assez long.
            let mine: Vec<&Line> = lines
                .iter()
                .filter(|l| l.source == source.label())
                .collect();
            let (mut n, mut good) = (0, 0);
            for (i, l) in mine.iter().enumerate() {
                let gap = mine
                    .get(i + 1)
                    .map_or(f64::INFINITY, |next| next.start - l.end);
                if gap < min_silence {
                    continue;
                }
                n += 1;
                let nearest = ends
                    .iter()
                    .map(|e| e - l.end)
                    .min_by(|a, b| a.abs().total_cmp(&b.abs()));
                if let Some(d) = nearest {
                    errors_ms.push(d * 1000.0);
                    if d.abs() <= TOLERANCE {
                        good += 1;
                    }
                }
            }
            println!(
                "{name:6} {:4} : {good}/{n} fins à ± 150 ms ({} fins détectées)",
                source.label(),
                ends.len()
            );
            total += n;
            ok += good;
        }
    }
    let elapsed = started.elapsed().as_secs_f64();
    errors_ms.sort_by(f64::total_cmp);
    let med = errors_ms
        .get(errors_ms.len() / 2)
        .copied()
        .unwrap_or(f64::NAN);
    let abs_p90 = {
        let mut a: Vec<f64> = errors_ms.iter().map(|e| e.abs()).collect();
        a.sort_by(f64::total_cmp);
        a.get((a.len() * 9 / 10).saturating_sub(1))
            .copied()
            .unwrap_or(f64::NAN)
    };
    println!(
        "total : {ok}/{total} fins de phrase à ± 150 ms ({:.1} %), écart médian {med:+.0} ms, |écart| p90 {abs_p90:.0} ms",
        100.0 * ok as f64 / total.max(1) as f64
    );
    println!(
        "{audio_s:.0} s d'audio en {elapsed:.1} s (1 thread), facteur temps réel {:.3}",
        elapsed / audio_s
    );
    Ok(())
}
