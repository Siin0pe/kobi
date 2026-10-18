//! Rejoue la partition `test` du jeu « Kobi » dans le détecteur rustpotter,
//! en flux, et compte les positifs détectés et les négatifs qui déclenchent,
//! comme `wake_eval` le fait pour openWakeWord (KOB-26).
//!
//! cargo run --release -p kobi-wake --example rustpotter_eval -- \
//!     [--data ~/Kobi-bench/wake] [--model ~/Kobi-bench/models/kobi-rustpotter/kobi.rpw] \
//!     [--min-scores 5]
//!
//! Le détecteur tourne au seuil le plus bas de la liste ; le score de la
//! meilleure détection de chaque clip est ensuite comparé à chaque seuil.
//! Pour un modèle, rustpotter donne sigmoïde((Δ − 2,2) / 2,2), Δ étant l'écart
//! des logits « kobi » − « none » : 0,27 pour Δ = 0, 0,5 pour Δ = 2,2.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use kobi_wake::rustpotter::{RustpotterDetector, RustpotterOptions};
use kobi_wake::WakeDetector;
use serde::Deserialize;

const THRESHOLDS: [f32; 7] = [0.3, 0.35, 0.4, 0.45, 0.5, 0.6, 0.7];

#[derive(Deserialize)]
struct Clip {
    file: String,
    label: String,
    split: String,
    lang: String,
    kind: String,
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

/// Souffle à peine audible, déterministe (le silence numérique pur fausse les MFCC).
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
    let mut model = home.join("models").join("kobi-rustpotter").join("kobi.rpw");
    let mut options = RustpotterOptions {
        threshold: THRESHOLDS[0],
        ..Default::default()
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let v = args.next().context("valeur manquante")?;
        match a.as_str() {
            "--data" => data = v.into(),
            "--model" => model = v.into(),
            "--min-scores" => options.min_scores = v.parse()?,
            _ => anyhow::bail!("option inconnue : {a}"),
        }
    }

    let mut det = RustpotterDetector::load(&model, &options)?;
    let meta = std::fs::read_to_string(data.join("metadata.jsonl"))?;
    let clips: Vec<Clip> = meta
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;

    // (label, langue, type) -> meilleur score de chaque clip (0 sans détection)
    let mut maxima: BTreeMap<(String, String, String), Vec<f32>> = BTreeMap::new();
    let mut seed = 1u32;
    let mut audio_s = 0.0;
    let start = Instant::now();
    for c in clips.iter().filter(|c| c.split == "test") {
        let mut reader = hound::WavReader::open(data.join(&c.file))?;
        let clip: Vec<i16> = reader.samples::<i16>().collect::<Result<_, _>>()?;
        let mut audio = hiss(40_000, &mut seed); // 2,5 s avant
        audio.extend(
            clip.iter()
                .zip(hiss(clip.len(), &mut seed))
                .map(|(a, b)| a.saturating_add(b)),
        );
        audio.extend(hiss(24_000, &mut seed)); // 1,5 s après : rustpotter confirme avec retard
        audio_s += audio.len() as f64 / 16_000.0;
        det.reset();
        let mut best = 0.0f32;
        for chunk in audio.chunks(512) {
            for d in det.push(chunk)? {
                best = best.max(d.score);
            }
        }
        maxima
            .entry((c.label.clone(), c.lang.clone(), c.kind.clone()))
            .or_default()
            .push(best);
    }
    let elapsed = start.elapsed().as_secs_f64();

    println!(
        "détecteur : {} (min_scores {}, 1 thread)",
        det.name(),
        options.min_scores
    );
    for ((label, lang, kind), scores) in &maxima {
        let rates: Vec<String> = THRESHOLDS
            .iter()
            .map(|t| {
                let n = scores.iter().filter(|&&s| s > 0.0 && s >= *t).count();
                format!("{t} : {:.1} %", 100.0 * n as f64 / scores.len() as f64)
            })
            .collect();
        println!(
            "{label:8} {lang} {kind:7} ({} clips) déclenchés à {}",
            scores.len(),
            rates.join(", ")
        );
    }
    println!(
        "{audio_s:.0} s d'audio traités en {elapsed:.1} s, facteur temps réel {:.3}",
        elapsed / audio_s
    );
    Ok(())
}
