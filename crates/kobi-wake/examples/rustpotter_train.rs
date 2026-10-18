//! Entraîne le modèle rustpotter « Kobi » sur le jeu synthétique (KOB-24),
//! sans le modifier : les extraits sont découpés en mémoire.
//!
//! cargo run --release -p kobi-wake --example rustpotter_train -- \
//!     [--data ~/Kobi-bench/wake] [--out ~/Kobi-bench/models/kobi-rustpotter/kobi.rpw] \
//!     [--model small] [--epochs 1500] [--window 1.2]
//!
//! Chaque clip est encadré de souffle, comme dans `rustpotter_eval`.
//! Positifs : la fenêtre de `--window` s qui finit 0,15 s après la fin du
//! mot-clé (`wake_end`). Négatifs : jusqu'à 8 fenêtres glissantes (pas de
//! 0,4 s), du souffle seul jusqu'à la fin du clip. rustpotter entraîne un
//! réseau sur les MFCC de cette fenêtre.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use kobi_wake::rustpotter::{save, train, Sample, TrainOptions};
use serde::Deserialize;

/// Marge gardée après la fin du mot-clé.
const MARGE: f64 = 0.15;
/// Fenêtres prises au plus dans un négatif.
const NEG_FENETRES: usize = 8;
/// Pas entre deux fenêtres négatives : 0,4 s.
const PAS: usize = 6_400;
/// Souffle ajouté avant (1,5 s) et après (0,5 s) chaque clip, comme en flux.
const AVANT: usize = 24_000;
const APRES: usize = 8_000;

#[derive(Deserialize)]
struct Clip {
    file: String,
    label: String,
    split: String,
    #[serde(default)]
    wake_end: Option<f64>,
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

/// Souffle à peine audible, déterministe (même bruit que `rustpotter_eval`).
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
    let mut out = home.join("models").join("kobi-rustpotter").join("kobi.rpw");
    let mut opts = TrainOptions::default();
    let mut window = 1.2f64;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let v = args.next().context("valeur manquante")?;
        match a.as_str() {
            "--data" => data = v.into(),
            "--out" => out = v.into(),
            "--model" => opts.model = v,
            "--epochs" => opts.epochs = v.parse()?,
            "--test-epochs" => opts.test_epochs = v.parse()?,
            "--lr" => opts.learning_rate = v.parse()?,
            "--window" => window = v.parse()?,
            _ => anyhow::bail!("option inconnue : {a}"),
        }
    }
    let n = (window * 16_000.0) as usize;
    anyhow::ensure!(
        n <= AVANT,
        "--window : 1,5 s au plus (souffle placé avant le clip)"
    );
    let meta = std::fs::read_to_string(data.join("metadata.jsonl"))?;
    let clips: Vec<Clip> = meta
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;

    let (mut train_set, mut test_set) = (Vec::new(), Vec::new());
    // Graine différente de celle de l'évaluation.
    let mut seed = 7u32;
    for c in &clips {
        let mut reader =
            hound::WavReader::open(data.join(&c.file)).with_context(|| c.file.clone())?;
        let audio: Vec<i16> = reader.samples::<i16>().collect::<Result<_, _>>()?;
        let name = c.file.replace('/', "_");
        let set = if c.split == "train" {
            &mut train_set
        } else {
            &mut test_set
        };
        // Comme en flux : souffle avant et après le clip.
        let mut flux = hiss(AVANT, &mut seed);
        flux.extend(
            audio
                .iter()
                .zip(hiss(audio.len(), &mut seed))
                .map(|(a, b)| a.saturating_add(b)),
        );
        flux.extend(hiss(APRES, &mut seed));
        if c.label == "positive" {
            let end = c.wake_end.context("positif sans wake_end")? + MARGE;
            let end = (AVANT + (end * 16_000.0) as usize).min(flux.len());
            set.push(Sample {
                name,
                audio: flux[end - n..end].to_vec(),
                wake: true,
            });
        } else {
            // Fenêtres glissantes, du souffle seul jusqu'à la fin du clip.
            for (i, start) in (AVANT.saturating_sub(n)..flux.len().saturating_sub(n))
                .step_by(PAS)
                .take(NEG_FENETRES)
                .enumerate()
            {
                set.push(Sample {
                    name: format!("{i}-{name}"),
                    audio: flux[start..start + n].to_vec(),
                    wake: false,
                });
            }
        }
    }
    let count = |s: &[Sample]| (s.iter().filter(|x| x.wake).count(), s.len());
    let (tp, tn) = count(&train_set);
    let (sp, sn) = count(&test_set);
    println!(
        "fenêtre {window} s ; train : {tp} positifs / {tn} extraits ; test : {sp} positifs / {sn} extraits"
    );
    println!(
        "modèle {} , {} époques, lr {}, mfcc {}",
        opts.model, opts.epochs, opts.learning_rate, opts.mfcc_size
    );

    let t0 = Instant::now();
    let model = train(&opts, &train_set, &test_set)?;
    println!("entraînement : {:.1} s", t0.elapsed().as_secs_f64());
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir)?;
    }
    save(&model, &out)?;
    println!(
        "modèle enregistré ({} Ko)",
        std::fs::metadata(&out)?.len() / 1024
    );
    Ok(())
}
