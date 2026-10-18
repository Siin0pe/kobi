//! Appels d'outils normalisés, indépendants du harness (KOB-157).
//!
//! L'adaptateur d'un harness (Claude Code : [`crate::claude_code::outils`])
//! traduit ses noms d'outils et leurs arguments en [`Action`] ; les règles de
//! Kobi (classement, validation, fil d'activité de KOB-152) ne voient que ce
//! type. Voir « Kobi — contrat commun des permissions », section 0.

use std::path::{Component, Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

/// Interpréteur d'une [`Action::Commande`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Bash,
    PowerShell,
}

/// Ce qu'un outil s'apprête à faire.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    LireFichier {
        chemin: PathBuf,
    },
    Chercher {
        motif: String,
        dossier: Option<PathBuf>,
    },
    Lister {
        dossier: PathBuf,
    },
    Commande {
        ligne: String,
        dossier: Option<PathBuf>,
        shell: Shell,
    },
    EcrireFichier {
        chemin: PathBuf,
        avant: Option<String>,
        apres: String,
    },
    SupprimerFichier {
        chemin: PathBuf,
    },
    Mcp {
        serveur: String,
        outil: String,
        params: Value,
    },
    Reglage {
        cle: String,
        valeur: Value,
    },
    Autre {
        nom: String,
        entree: Value,
    },
}

/// Appel d'outil d'un tour.
#[derive(Debug, Clone, PartialEq)]
pub struct DemandeOutil {
    pub id: String,
    pub tour: u64,
    pub action: Action,
}

/// Famille d'une étape, pour l'icône et l'animation de Kobi.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Famille {
    Lecture,
    Recherche,
    Commande,
    Mcp,
    Ecriture,
    Reglage,
}

/// Une ligne du fil d'activité : ce que fait Kobi, en une phrase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Etape {
    #[serde(rename = "type")]
    pub famille: Famille,
    pub resume: String,
    /// Chemin montré, relatif au dossier de code qui le contient.
    pub chemin_relatif: Option<String>,
}

/// Longueur maximale d'une commande ou d'un motif dans le fil.
const COURT: usize = 60;

impl Action {
    pub fn famille(&self) -> Famille {
        match self {
            Action::LireFichier { .. } => Famille::Lecture,
            Action::Chercher { .. } | Action::Lister { .. } => Famille::Recherche,
            Action::Commande { .. } => Famille::Commande,
            Action::Mcp { .. } => Famille::Mcp,
            Action::EcrireFichier { .. } | Action::SupprimerFichier { .. } => Famille::Ecriture,
            Action::Reglage { .. } => Famille::Reglage,
            Action::Autre { .. } => Famille::Commande,
        }
    }

    /// Résumé d'une ligne ; les chemins sont raccourcis relativement au
    /// dossier de code (`dossiers`) qui les contient, les secrets masqués.
    pub fn etape(&self, dossiers: &[PathBuf]) -> Etape {
        let rel = |p: &Path| relatif(p, dossiers);
        let (resume, chemin) = match self {
            Action::LireFichier { chemin } => {
                let r = rel(chemin);
                (format!("Je lis {r}"), Some(r))
            }
            Action::Chercher { motif, dossier } => {
                let motif = court(&masquer(motif), COURT);
                match dossier {
                    Some(d) => {
                        let r = rel(d);
                        (format!("Je cherche « {motif} » dans {r}"), Some(r))
                    }
                    None => (format!("Je cherche « {motif} »"), None),
                }
            }
            Action::Lister { dossier } => {
                let r = rel(dossier);
                (format!("Je parcours {r}"), Some(r))
            }
            Action::Commande { ligne, dossier, .. } => {
                let ligne = court(&masquer(ligne.lines().next().unwrap_or_default()), COURT);
                (format!("Je lance {ligne}"), dossier.as_deref().map(&rel))
            }
            Action::EcrireFichier { chemin, avant, .. } => {
                let r = rel(chemin);
                let verbe = if avant.is_some() {
                    "Je modifie"
                } else {
                    "J'écris"
                };
                (format!("{verbe} {r}"), Some(r))
            }
            Action::SupprimerFichier { chemin } => {
                let r = rel(chemin);
                (format!("Je supprime {r}"), Some(r))
            }
            Action::Mcp {
                serveur,
                outil,
                params,
            } => {
                let mut s = format!(
                    "J'interroge {} : {}",
                    majuscule(serveur),
                    outil.replace('_', " ")
                );
                let p = parametres(params);
                if !p.is_empty() {
                    s.push_str(&format!(" ({p})"));
                }
                (s, None)
            }
            Action::Reglage { cle, .. } => (format!("Je change le réglage {cle}"), None),
            Action::Autre { nom, .. } => (format!("J'utilise {nom}"), None),
        };
        Etape {
            famille: self.famille(),
            resume,
            chemin_relatif: chemin,
        }
    }
}

/// Chemin relatif au dossier de code le plus précis qui le contient
/// (séparateurs `/`). Un dossier de code lui-même est montré par son nom ;
/// un chemin hors des dossiers, par ses deux derniers éléments.
pub fn relatif(chemin: &Path, dossiers: &[PathBuf]) -> String {
    let normal = |p: &Path| -> Vec<String> {
        p.components()
            .filter(|c| !matches!(c, Component::CurDir))
            .map(|c| {
                c.as_os_str()
                    .to_string_lossy()
                    .to_lowercase()
                    .replace('\\', "/")
            })
            .collect()
    };
    let morceaux: Vec<String> = chemin
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let bas = normal(chemin);
    let racine = dossiers
        .iter()
        .map(|d| (d, normal(d)))
        .filter(|(_, d)| !d.is_empty() && bas.starts_with(d))
        .max_by_key(|(_, d)| d.len());
    if let Some((dossier, d)) = racine {
        if d.len() == bas.len() {
            return dossier
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| dossier.display().to_string());
        }
        return morceaux[d.len()..].join("/");
    }
    if chemin.is_relative() || morceaux.len() <= 2 {
        return morceaux.join("/").trim_start_matches('/').to_owned();
    }
    format!("…/{}", morceaux[morceaux.len() - 2..].join("/"))
}

/// Coupe au-delà de `max` caractères.
fn court(texte: &str, max: usize) -> String {
    let texte = texte.trim();
    if texte.chars().count() <= max {
        return texte.to_owned();
    }
    let mut s: String = texte.chars().take(max - 1).collect();
    s.push('…');
    s
}

fn majuscule(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|p| p.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// Deux premières valeurs simples des paramètres d'un outil MCP.
fn parametres(params: &Value) -> String {
    let Some(objet) = params.as_object() else {
        return String::new();
    };
    objet
        .iter()
        .filter(|(cle, _)| !secret_de_nom(cle))
        .filter_map(|(cle, v)| {
            let v = match v {
                Value::String(s) => court(&masquer(s), 30),
                Value::Number(n) => n.to_string(),
                Value::Bool(b) => b.to_string(),
                _ => return None,
            };
            Some(format!("{cle} {v}"))
        })
        .take(2)
        .collect::<Vec<_>>()
        .join(", ")
}

fn secret_de_nom(nom: &str) -> bool {
    let nom = nom.to_lowercase();
    [
        "token",
        "secret",
        "password",
        "passwd",
        "api_key",
        "apikey",
        "authorization",
        "cle",
        "key",
    ]
    .iter()
    .any(|m| nom.contains(m))
}

/// Masque ce qui ressemble à un secret : jeton après `Bearer`, valeur d'une
/// option ou d'une variable dont le nom parle de clé, de jeton ou de mot de
/// passe, clés au format connu (`sk-…`, `ghp_…`, `lin_api_…`…).
pub fn masquer(texte: &str) -> String {
    const PREFIXES: [&str; 7] = [
        "sk-",
        "ghp_",
        "gho_",
        "github_pat_",
        "lin_api_",
        "xoxb-",
        "AKIA",
    ];
    let mots: Vec<&str> = texte.split(' ').collect();
    let mut sortie = Vec::with_capacity(mots.len());
    let mut suivant_secret = false;
    for mot in mots {
        let nu = mot.trim_matches(|c| c == '"' || c == '\'');
        if suivant_secret && !mot.is_empty() && !nu.eq_ignore_ascii_case("bearer") {
            // Le guillemet fermant reste, pour que la commande se lise.
            let guillemet = &mot[mot.trim_end_matches(['"', '\'']).len()..];
            sortie.push(format!("•••{guillemet}"));
            suivant_secret = false;
            continue;
        }
        if nu.eq_ignore_ascii_case("bearer") || nu.eq_ignore_ascii_case("authorization:") {
            suivant_secret = true;
            sortie.push(mot.to_owned());
            continue;
        }
        if let Some((nom, _)) = mot.split_once('=') {
            if secret_de_nom(nom.trim_start_matches('-')) {
                sortie.push(format!("{nom}=•••"));
                continue;
            }
        }
        if (mot.starts_with("--") || mot.starts_with('-')) && secret_de_nom(mot) {
            suivant_secret = true;
            sortie.push(mot.to_owned());
            continue;
        }
        if PREFIXES
            .iter()
            .any(|p| nu.starts_with(p) && nu.len() > p.len() + 8)
        {
            sortie.push("•••".to_owned());
            continue;
        }
        sortie.push(mot.to_owned());
    }
    sortie.join(" ")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn code() -> Vec<PathBuf> {
        vec![
            PathBuf::from("C:/Users/moi/Desktop/kobi"),
            PathBuf::from("C:/Users/moi/Desktop"),
        ]
    }

    #[test]
    fn chemins_relatifs_au_dossier_le_plus_precis() {
        let d = code();
        assert_eq!(
            relatif(
                Path::new("C:/Users/moi/Desktop/kobi/crates/kobi-app/src/chef.rs"),
                &d
            ),
            "crates/kobi-app/src/chef.rs"
        );
        assert_eq!(
            relatif(Path::new(r"C:\Users\moi\Desktop\kobi\README.md"), &d),
            "README.md"
        );
        assert_eq!(
            relatif(Path::new("c:/users/moi/desktop/site/index.html"), &d),
            "site/index.html"
        );
        assert_eq!(relatif(Path::new("C:/Users/moi/Desktop/kobi"), &d), "kobi");
        assert_eq!(
            relatif(Path::new("D:/autre/projet/src/main.rs"), &d),
            "…/src/main.rs"
        );
        assert_eq!(relatif(Path::new("src/lib.rs"), &d), "src/lib.rs");
    }

    #[test]
    fn resumes_lisibles() {
        let d = code();
        let lire = Action::LireFichier {
            chemin: "C:/Users/moi/Desktop/kobi/crates/kobi-app/src/chef.rs".into(),
        };
        let e = lire.etape(&d);
        assert_eq!(e.resume, "Je lis crates/kobi-app/src/chef.rs");
        assert_eq!(e.famille, Famille::Lecture);
        let chercher = Action::Chercher {
            motif: "fabrique_claude".into(),
            dossier: Some("C:/Users/moi/Desktop/kobi".into()),
        };
        assert_eq!(
            chercher.etape(&d).resume,
            "Je cherche « fabrique_claude » dans kobi"
        );
        let git = Action::Commande {
            ligne: "git log -5\ngit status".into(),
            dossier: None,
            shell: Shell::Bash,
        };
        assert_eq!(git.etape(&d).resume, "Je lance git log -5");
        let mcp = Action::Mcp {
            serveur: "linear".into(),
            outil: "list_issues".into(),
            params: json!({"state": "In Progress", "limit": 5, "filtre": {"a": 1}}),
        };
        let e = mcp.etape(&d);
        assert_eq!(
            e.resume,
            "J'interroge Linear : list issues (limit 5, state In Progress)"
        );
        assert_eq!(e.famille, Famille::Mcp);
        let long = Action::Commande {
            ligne: "x".repeat(200),
            dossier: None,
            shell: Shell::PowerShell,
        };
        assert!(long.etape(&d).resume.chars().count() <= "Je lance ".len() + COURT);
    }

    #[test]
    fn secrets_masques() {
        assert_eq!(
            masquer("curl -H \"Authorization: Bearer abcdef123456\" https://x"),
            "curl -H \"Authorization: Bearer •••\" https://x"
        );
        assert_eq!(masquer("API_KEY=sk-123 cargo run"), "API_KEY=••• cargo run");
        assert_eq!(
            masquer("gh auth --token ghp_aaaaaaaaaaaa"),
            "gh auth --token •••"
        );
        assert_eq!(masquer("echo sk-ant-api03-zzzzzzzzzzzz"), "echo •••");
        assert_eq!(masquer("git log -5"), "git log -5");
        let mcp = Action::Mcp {
            serveur: "x".into(),
            outil: "appel".into(),
            params: json!({"api_key": "secret", "nom": "ok"}),
        };
        assert_eq!(mcp.etape(&[]).resume, "J'interroge X : appel (nom ok)");
    }
}
