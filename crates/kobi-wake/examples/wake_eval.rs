//! Rejoue la partition `test` du jeu « Kobi » dans le détecteur openWakeWord
//! en Rust, en flux, et compte les positifs détectés et les négatifs qui
//! déclenchent, comme `bench/wake/train_oww.py` le fait en Python.
//!
//! cargo run --release -p kobi-wake --example wake_eval -- \
//!     [--data ~/Kobi-bench/wake] [--features ~/Kobi-bench/models/openwakeword] \
//!     [--model ~/Kobi-bench/models/kobi-oww/kobi.onnx] [--threads 1]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use kobi_wake::oww::{OpenWakeWord, OwwConfig};
use kobi_wake::WakeDetector;
use serde::Deserialize;

const THRESHOLDS: [f32; 4] = [0.3, 0.5, 0.7, 0.9];

#[derive(Deserialize)]
struct Clip {
    file: String,
    label: String,
    split: String,
    lang: String,
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

/// Souffle à peine audible, déterministe (le silence numérique pur fausse le mel).
fn hiss(n: usize, seed: &mut u32) -> Vec<i16> {
    (0..n)
        .map(|_| {
            *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((*seed >> 16) % 7) as i16 - 3
        })
        .collect()
}

fn main() -> Result<()> {
    let home = bench_home();
    let mut data = home.join("wake");
    let mut features = home.join("models").join("openwakeword");
    let mut model = home.join("models").join("kobi-oww").join("kobi.onnx");
    let mut threads = 1;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let v = args.next().context("valeur manquante")?;
        match a.as_str() {
            "--data" => data = v.into(),
            "--features" => features = v.into(),
            "--model" => model = v.into(),
            "--threads" => threads = v.parse()?,
            _ => anyhow::bail!("option inconnue : {a}"),
        }
    }

    let mut config = OwwConfig::new(&features, &model);
    config.threads = threads;
    let mut det = OpenWakeWord::new(&config)?;
    let meta = std::fs::read_to_string(data.join("metadata.jsonl"))?;
    let clips: Vec<Clip> = meta
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;

    // (label, langue) -> scores maximaux
    let mut maxima: BTreeMap<(String, String), Vec<f32>> = BTreeMap::new();
    let mut seed = 1u32;
    let mut audio_s = 0.0;
    let start = Instant::now();
    for c in clips.iter().filter(|c| c.split == "test") {
        let mut reader = hound::WavReader::open(data.join(&c.file))?;
        let clip: Vec<i16> = reader.samples::<i16>().collect::<Result<_, _>>()?;
        let mut audio = hiss(40_000, &mut seed); // 2,5 s avant
        audio.extend(clip.iter().zip(hiss(clip.len(), &mut seed)).map(|(a, b)| a.saturating_add(b)));
        audio.extend(hiss(8_000, &mut seed)); // 0,5 s après
        audio_s += audio.len() as f64 / 16_000.0;
        det.reset();
        let mut best = 0.0f32;
        for chunk in audio.chunks(512) {
            for (_, s) in det.push_scores(chunk)? {
                best = best.max(s);
            }
        }
        maxima.entry((c.label.clone(), c.lang.clone())).or_default().push(best);
    }
    let elapsed = start.elapsed().as_secs_f64();

    println!("détecteur : {} (Rust, {} thread(s) par modèle)", det.name(), threads);
    for ((label, lang), scores) in &maxima {
        let rates: Vec<String> = THRESHOLDS
            .iter()
            .map(|t| {
                let n = scores.iter().filter(|&&s| s >= *t).count();
                format!("{t} : {:.1} %", 100.0 * n as f64 / scores.len() as f64)
            })
            .collect();
        println!("{label:8} {lang} ({} clips) déclenchés à {}", scores.len(), rates.join(", "));
    }
    println!(
        "{audio_s:.0} s d'audio traités en {elapsed:.1} s, facteur temps réel {:.3}",
        elapsed / audio_s
    );
    Ok(())
}
