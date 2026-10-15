use clap::{Parser, Subcommand};

mod record;

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
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().commande {
        Commande::Record(args) => record::run(args),
    }
}
