//! Où trouver l'app en cours : point d'accès local et jeton.
//!
//! * Windows : canal nommé `\\.\pipe\kobi-<utilisateur>` ;
//! * ailleurs : socket Unix `~/Kobi/controle.sock`.
//!
//! Le jeton est dans `~/Kobi/controle.jeton`, régénéré à chaque lancement de
//! l'app. Jamais de port réseau.

use std::path::{Path, PathBuf};

/// Nom du fichier du jeton dans le dossier de Kobi.
pub const FICHIER_JETON: &str = "controle.jeton";
/// Nom de la socket Unix dans le dossier de Kobi.
#[cfg(unix)]
pub const FICHIER_SOCKET: &str = "controle.sock";

/// Point d'accès de l'app et fichier de son jeton.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Point {
    /// Canal nommé (Windows) ou chemin de la socket (ailleurs).
    pub adresse: String,
    pub jeton: PathBuf,
}

/// Dossier de Kobi : `~/Kobi`.
pub fn dossier_kobi() -> Option<PathBuf> {
    std::env::home_dir().map(|h| h.join("Kobi"))
}

/// Nom d'utilisateur réduit aux caractères sûrs dans un nom de canal.
#[cfg(windows)]
fn utilisateur() -> String {
    let nom = std::env::var("USERNAME").unwrap_or_else(|_| "kobi".into());
    let nom: String = nom
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if nom.is_empty() {
        "kobi".into()
    } else {
        nom
    }
}

impl Point {
    /// Point d'accès de l'app de l'utilisateur.
    pub fn defaut() -> Option<Self> {
        let dossier = dossier_kobi()?;
        #[cfg(windows)]
        let adresse = format!(r"\\.\pipe\kobi-{}", utilisateur());
        #[cfg(unix)]
        let adresse = dossier.join(FICHIER_SOCKET).display().to_string();
        Some(Self {
            adresse,
            jeton: dossier.join(FICHIER_JETON),
        })
    }

    /// Point d'accès isolé, pour les tests : jeton dans `dossier`, canal ou
    /// socket nommé d'après `nom`.
    pub fn dans(dossier: &Path, nom: &str) -> Self {
        #[cfg(windows)]
        let adresse = format!(r"\\.\pipe\kobi-test-{nom}");
        #[cfg(unix)]
        let adresse = dossier.join(format!("{nom}.sock")).display().to_string();
        Self {
            adresse,
            jeton: dossier.join(FICHIER_JETON),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_par_defaut_local() {
        let Some(p) = Point::defaut() else { return };
        assert!(p.jeton.ends_with(Path::new("Kobi").join(FICHIER_JETON)));
        #[cfg(windows)]
        assert!(p.adresse.starts_with(r"\\.\pipe\kobi-"), "{}", p.adresse);
        #[cfg(unix)]
        assert!(p.adresse.ends_with("Kobi/controle.sock"), "{}", p.adresse);
    }
}
