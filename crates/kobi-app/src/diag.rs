//! Diagnostic de l'écoute continue (`KOBI_DIAG=1`).
//!
//! Écrit dans `~/Kobi/logs/diag-ecoute.log`, à part du journal : le
//! périphérique ouvert, le niveau du micro et le nombre de trames reçues
//! chaque seconde, et chaque segment transcrit (partiel et final) avec la
//! décision de détection. Ce fichier contient donc ce qui a été dit : il
//! n'existe que si le propriétaire active le diagnostic.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use crate::mot_cle::{reperer, Action};

/// Chemin du fichier de diagnostic.
pub fn chemin() -> Option<PathBuf> {
    Some(kobi_session::config::kobi_dir()?.join("logs/diag-ecoute.log"))
}

/// Le diagnostic est-il demandé ?
pub fn demande() -> bool {
    std::env::var("KOBI_DIAG").is_ok_and(|v| !v.is_empty() && v != "0")
}

/// Niveau d'une seconde d'audio.
#[derive(Debug, Default)]
struct Seconde {
    debut: f64,
    trames: u32,
    echantillons: u64,
    carres: f64,
    crete: f32,
}

/// Fichier de diagnostic ouvert.
pub struct Diag {
    /// `None` : diagnostic en mémoire, pour les tests.
    sortie: Option<BufWriter<File>>,
    seconde: Seconde,
    /// Segments finals gardés en mémoire (tests seulement).
    finals: Vec<String>,
}

/// Niveau en dB pleine échelle (`-inf` pour un silence numérique).
pub fn dbfs(valeur: f64) -> f64 {
    20.0 * valeur.max(1e-10).log10()
}

impl Diag {
    /// Ouvre le fichier si `KOBI_DIAG` est posé.
    pub fn ouvrir() -> Option<Diag> {
        if !demande() {
            return None;
        }
        let chemin = chemin()?;
        if let Some(dossier) = chemin.parent() {
            let _ = std::fs::create_dir_all(dossier);
        }
        match OpenOptions::new().create(true).append(true).open(&chemin) {
            Ok(f) => {
                tracing::info!(fichier = %chemin.display(), "diagnostic de l'écoute activé");
                let mut d = Diag {
                    sortie: Some(BufWriter::new(f)),
                    seconde: Seconde::default(),
                    finals: Vec::new(),
                };
                d.ligne(format_args!(
                    "--- démarrage de Kobi (pid {})",
                    std::process::id()
                ));
                Some(d)
            }
            Err(e) => {
                tracing::warn!(fichier = %chemin.display(), erreur = %e, "diagnostic impossible");
                None
            }
        }
    }

    /// Diagnostic sans fichier qui garde les segments finals.
    #[cfg(test)]
    pub fn memoire() -> Diag {
        Diag {
            sortie: None,
            seconde: Seconde::default(),
            finals: Vec::new(),
        }
    }

    /// Segments finals reçus depuis le dernier appel (diagnostic en mémoire).
    #[cfg(test)]
    pub fn finals(&mut self) -> Vec<String> {
        std::mem::take(&mut self.finals)
    }

    fn ligne(&mut self, texte: std::fmt::Arguments) {
        if let Some(sortie) = &mut self.sortie {
            let _ = writeln!(sortie, "{} {texte}", horodatage());
            let _ = sortie.flush();
        }
    }

    /// Une entrée audio vient d'être ouverte.
    pub fn entree(&mut self, description: &str) {
        self.ligne(format_args!("entrée ouverte : {description}"));
        self.seconde = Seconde::default();
    }

    /// Une trame est arrivée (avant Nemotron). `t` : début de la trame.
    pub fn trame(&mut self, t: f64, samples: &[f32]) {
        let s = &mut self.seconde;
        if s.trames == 0 {
            s.debut = t;
        }
        s.trames += 1;
        s.echantillons += samples.len() as u64;
        s.carres += samples
            .iter()
            .map(|&x| f64::from(x) * f64::from(x))
            .sum::<f64>();
        s.crete = samples.iter().fold(s.crete, |m, &x| m.max(x.abs()));
        if t + samples.len() as f64 / 16_000.0 - s.debut >= 1.0 {
            let rms = (s.carres / s.echantillons.max(1) as f64).sqrt();
            let (debut, trames, n, crete) = (s.debut, s.trames, s.echantillons, s.crete);
            self.ligne(format_args!(
                "niveau t={debut:.1}s trames={trames} echantillons={n} rms={:.1}dBFS crete={:.1}dBFS",
                dbfs(rms),
                dbfs(f64::from(crete))
            ));
            self.seconde = Seconde::default();
        }
    }

    /// Un segment transcrit et ce que l'écoute en a fait.
    pub fn segment(&mut self, texte: &str, finale: bool, actions: &[Action]) {
        if finale && self.sortie.is_none() {
            self.finals.push(texte.to_string());
        }
        let detection = match reperer(texte) {
            Some(suite) => format!("« Kobi » repéré, suite « {suite} »"),
            None => "pas de « Kobi »".into(),
        };
        self.ligne(format_args!(
            "segment {} « {texte} » : {detection} ; actions {actions:?}",
            if finale { "final" } else { "partiel" },
        ));
    }

    /// Événement libre (raccourci, refus de la session…).
    pub fn note(&mut self, texte: &str) {
        self.ligne(format_args!("{texte}"));
    }
}

/// Heure UTC `HH:MM:SS.mmm`, sans dépendance de date.
fn horodatage() -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let s = ms / 1000;
    format!(
        "{:02}:{:02}:{:02}.{:03}Z",
        (s / 3600) % 24,
        (s / 60) % 60,
        s % 60,
        ms % 1000
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn niveaux() {
        assert!((dbfs(1.0)).abs() < 1e-9);
        assert!((dbfs(0.1) + 20.0).abs() < 1e-9);
        assert!(dbfs(0.0) < -150.0);
    }
}
