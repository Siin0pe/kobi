//! `kobi-bench` : outils de mesure de Kobi (jalon J0).

use clap::{Parser, Subcommand};

mod record;
mod stt;

/// Outils de mesure de Kobi.
#[derive(Parser)]
#[command(name = "kobi-bench")]
struct Cli {
    #[command(subcommand)]
    commande: Commande,
}

#[derive(Subcommand)]
enum Commande {
    /// Enregistre le micro (moi.wav) et l'audio système (eux.wav) en 16 kHz mono.
    Record(record::RecordArgs),
    /// Benchmark de transcription : rejoue un corpus dans des moteurs et écrit
    /// rapport.json et rapport.md.
    Stt(stt::SttArgs),
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().commande {
        Commande::Record(args) => record::run(args),
        Commande::Stt(args) => stt::run(args),
    }
}
