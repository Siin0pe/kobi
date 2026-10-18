//! Dit un texte avec la voix Piper et l'écrit en WAV, avec les temps de
//! synthèse (premier son, facteur temps réel).
//!
//! `cargo run -p kobi-tts --features piper --example dire -- <paquet> <sortie.wav> "texte"`

use std::time::Instant;

use kobi_tts::piper::{Piper, PiperConfig};
use kobi_tts::sortie::{Fichier, Sortie};
use kobi_tts::texte::Diction;
use kobi_tts::{Arret, Synthese};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [paquet, wav, texte] = args.as_slice() else {
        eprintln!("usage : dire <paquet vits-piper-…> <sortie.wav> \"texte\"");
        std::process::exit(2);
    };
    let t = Instant::now();
    let mut voix = Piper::new(&PiperConfig::new(paquet)).expect("voix");
    println!("chargement : {} ms", t.elapsed().as_millis());
    let mut sortie = Fichier::new(wav.as_ref(), false);
    let phrases = Diction::new().fin(texte);
    let arret = Arret::default();
    let debut = Instant::now();
    let mut audio = 0.0;
    for (i, phrase) in phrases.iter().enumerate() {
        let t = Instant::now();
        let son = voix.synthetiser(phrase).expect("synthèse");
        let duree = son.len() as f64 / voix.frequence() as f64;
        audio += duree;
        if i == 0 {
            println!("premier son : {} ms", debut.elapsed().as_millis());
        }
        println!(
            "phrase {} : {} ms pour {:.2} s de voix — {phrase}",
            i + 1,
            t.elapsed().as_millis(),
            duree
        );
        sortie.jouer(&son, voix.frequence(), &arret).expect("wav");
    }
    println!(
        "total : {:.2} s de voix en {} ms (facteur temps réel {:.2})",
        audio,
        debut.elapsed().as_millis(),
        debut.elapsed().as_secs_f64() / audio.max(0.01)
    );
}
