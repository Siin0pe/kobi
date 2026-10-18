//! Le parleur complet (voix Piper, sortie WAV au rythme réel) sur une réponse
//! qui arrive en flux, avec le temps jusqu'au premier son.
//!
//! `cargo run -p kobi-tts --features piper --example parleur -- <paquet> <sortie.wav> "texte"`

use std::sync::mpsc;
use std::time::{Duration, Instant};

use kobi_tts::piper::{Piper, PiperConfig};
use kobi_tts::sortie::Fichier;
use kobi_tts::texte::Diction;
use kobi_tts::{Evenement, Parleur, Sortie, Synthese};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [paquet, wav, texte] = args.as_slice() else {
        eprintln!("usage : parleur <paquet vits-piper-…> <sortie.wav> \"texte\"");
        std::process::exit(2);
    };
    let (paquet, wav) = (paquet.clone(), wav.clone());
    let (tx, rx) = mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    let (pret_tx, pret_rx) = mpsc::channel();
    let p = Parleur::new(
        Box::new(move || {
            let v = Piper::new(&PiperConfig::new(&paquet))?;
            let _ = pret_tx.send(());
            Ok(Box::new(v) as Box<dyn Synthese>)
        }),
        Box::new(move || Ok(Box::new(Fichier::new(wav.as_ref(), true)) as Box<dyn Sortie>)),
        move |e| {
            let _ = tx.lock().unwrap().send(e);
        },
    );
    pret_rx.recv().expect("voix");
    // La réponse arrive par morceaux de 8 caractères toutes les 40 ms.
    let debut = Instant::now();
    let e = p.nouvel_enonce();
    let mut d = Diction::new();
    let mut accumule = String::new();
    for morceau in texte.chars().collect::<Vec<_>>().chunks(8) {
        accumule.extend(morceau);
        for phrase in d.suite(&accumule) {
            println!(
                "{:>6} ms  phrase prête : {phrase}",
                debut.elapsed().as_millis()
            );
            p.dire(e, &phrase);
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    for phrase in d.fin(&accumule) {
        p.dire(e, &phrase);
    }
    p.finir(e);
    while let Ok(ev) = rx.recv_timeout(Duration::from_secs(60)) {
        println!("{:>6} ms  {ev:?}", debut.elapsed().as_millis());
        if matches!(ev, Evenement::Fin { .. } | Evenement::Erreur(_)) {
            break;
        }
    }
}
