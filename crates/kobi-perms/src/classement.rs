//! Classement d'une action normalisée : lecture libre, écriture à valider,
//! ou refus (contrat commun des permissions, sections 0 et 2). Le harness
//! n'apparaît nulle part : on classe une [`Action`], pas un outil Claude.

use std::path::{Component, Path, PathBuf};

use crate::action::Action;
use crate::commande;
use crate::rules::{Rules, Verdict};

/// Ce qu'une écriture modifie (pour la carte de validation, KOB-156).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Genre {
    Fichier,
    Commande,
    Mcp,
    Reglage,
    Suppression,
}

/// Classe d'une action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Classe {
    /// Ne modifie rien : passe sans carte.
    Lecture,
    /// Modifie quelque chose : demande une validation (KOB-156) ; refusée
    /// tant que Kobi est en lecture seule.
    Ecriture(Genre),
    /// Jamais permise (hors des dossiers déclarés…), avec la raison.
    Refus(String),
}

impl Classe {
    /// `lecture`, `ecriture` ou `refus` (fil d'activité).
    pub fn cle(&self) -> &'static str {
        match self {
            Classe::Lecture => "lecture",
            Classe::Ecriture(_) => "ecriture",
            Classe::Refus(_) => "refus",
        }
    }
}

/// Ce qui sert à classer : où Kobi a le droit d'aller.
#[derive(Debug, Clone, Default)]
pub struct Contexte {
    /// Dossiers déclarés (codes, mémoire des réunions) : rien hors d'eux.
    pub dossiers: Vec<PathBuf>,
    /// Dossier où la commande s'exécute ; le premier dossier déclaré sinon.
    pub cwd: Option<PathBuf>,
    /// Outils MCP en lecture seule (annotations des serveurs, KOB-155).
    pub mcp: Rules,
}

impl Contexte {
    pub fn new(dossiers: Vec<PathBuf>) -> Self {
        Self {
            dossiers,
            ..Default::default()
        }
    }

    /// Dossier de travail effectif.
    pub fn dossier_courant(&self) -> Option<PathBuf> {
        self.cwd.clone().or_else(|| self.dossiers.first().cloned())
    }

    /// `chemin`, relatif à `depuis`, est-il dans un dossier déclaré ?
    pub fn contient(&self, chemin: &Path, depuis: Option<&Path>) -> bool {
        let Some(cible) = absolu(chemin, depuis) else {
            return false;
        };
        let cible = cle_de_chemin(&cible);
        self.dossiers.iter().any(|d| {
            let d = cle_de_chemin(d);
            !d.is_empty() && (cible == d || cible.starts_with(&format!("{d}/")))
        })
    }
}

/// Classe une action. Un outil inconnu est une écriture (demandée), jamais
/// une lecture par défaut.
pub fn classer(action: &Action, ctx: &Contexte) -> Classe {
    let cwd = ctx.dossier_courant();
    let dedans = |p: &Path| ctx.contient(p, cwd.as_deref());
    let hors = |p: &Path| {
        Classe::Refus(format!(
            "{} est hors des dossiers de code déclarés",
            p.display()
        ))
    };
    match action {
        Action::LireFichier { chemin } => {
            if dedans(chemin) {
                Classe::Lecture
            } else {
                hors(chemin)
            }
        }
        Action::Chercher { dossier, .. } => match dossier {
            Some(d) if !dedans(d) => hors(d),
            _ => Classe::Lecture,
        },
        Action::Lister { dossier } => {
            // Un motif (`src/**/*.rs`) : on vérifie la partie fixe.
            let fixe: PathBuf = dossier
                .components()
                .take_while(|c| {
                    !c.as_os_str()
                        .to_string_lossy()
                        .contains(['*', '?', '[', '{'])
                })
                .collect();
            if fixe.as_os_str().is_empty() || dedans(&fixe) {
                Classe::Lecture
            } else {
                hors(&fixe)
            }
        }
        Action::Commande {
            ligne,
            shell,
            dossier,
        } => {
            let mut ctx = ctx.clone();
            if let Some(d) = dossier {
                ctx.cwd = Some(d.clone());
            }
            commande::analyser(ligne, *shell, &ctx).classe
        }
        Action::EcrireFichier { chemin, .. } => {
            if dedans(chemin) {
                Classe::Ecriture(Genre::Fichier)
            } else {
                hors(chemin)
            }
        }
        Action::SupprimerFichier { chemin } => {
            if dedans(chemin) {
                Classe::Ecriture(Genre::Suppression)
            } else {
                hors(chemin)
            }
        }
        Action::Mcp { serveur, outil, .. } => {
            match ctx.mcp.verdict(&format!("mcp__{serveur}__{outil}")) {
                Verdict::Allow(_) => Classe::Lecture,
                _ => Classe::Ecriture(Genre::Mcp),
            }
        }
        Action::Reglage { .. } => Classe::Ecriture(Genre::Reglage),
        Action::Autre { .. } => Classe::Ecriture(Genre::Commande),
    }
}

/// Chemin absolu, normalisé sans toucher au disque (`.` et `..` résolus) ;
/// `None` si on ne peut pas savoir où il mène (`~`, relatif sans dossier).
/// Les chemins de Git Bash (`/c/Users/…`) deviennent `C:/Users/…`.
pub fn absolu(chemin: &Path, depuis: Option<&Path>) -> Option<PathBuf> {
    let texte = chemin.to_string_lossy().replace('\\', "/");
    if texte.starts_with('~') || texte.contains('$') || texte.contains('%') {
        return None;
    }
    let texte = git_bash_vers_windows(&texte);
    let chemin = PathBuf::from(&texte);
    let complet = if est_absolu(&texte) {
        chemin
    } else {
        depuis?.join(chemin)
    };
    let mut pile: Vec<String> = Vec::new();
    let mut racine = String::new();
    for c in PathBuf::from(complet.to_string_lossy().replace('\\', "/")).components() {
        match c {
            Component::Prefix(p) => racine = p.as_os_str().to_string_lossy().into_owned(),
            Component::RootDir => racine.push('/'),
            Component::CurDir => {}
            Component::ParentDir => {
                pile.pop()?;
            }
            Component::Normal(n) => pile.push(n.to_string_lossy().into_owned()),
        }
    }
    let mut s = racine.replace('\\', "/");
    if !s.ends_with('/') && !s.is_empty() {
        s.push('/');
    }
    s.push_str(&pile.join("/"));
    Some(PathBuf::from(s))
}

/// `C:/…`, `C:\…`, `/…` ou `\\serveur\…`.
fn est_absolu(texte: &str) -> bool {
    let b = texte.as_bytes();
    texte.starts_with('/') || (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':')
}

/// `/c/Users/x` → `C:/Users/x` (Git Bash sous Windows), `/cygdrive/c/…` aussi.
fn git_bash_vers_windows(texte: &str) -> String {
    let reste = texte.strip_prefix("/cygdrive").unwrap_or(texte);
    let b = reste.as_bytes();
    if b.len() >= 2 && b[0] == b'/' && b[1].is_ascii_alphabetic() && (b.len() == 2 || b[2] == b'/')
    {
        let lecteur = (b[1] as char).to_ascii_uppercase();
        let suite = if reste.len() > 2 { &reste[2..] } else { "/" };
        return format!("{lecteur}:{suite}");
    }
    texte.to_owned()
}

/// Forme comparable : `/` partout, sans `/` final, minuscules (Windows et
/// macOS ignorent la casse ; sous Linux, deux dossiers qui ne diffèrent que
/// par la casse restent rares dans des dossiers de code).
fn cle_de_chemin(chemin: &Path) -> String {
    let mut s = chemin.to_string_lossy().replace('\\', "/").to_lowercase();
    while s.len() > 1 && s.ends_with('/') && !s.ends_with(":/") {
        s.pop();
    }
    if s.ends_with(":/") {
        s.pop();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx() -> Contexte {
        Contexte::new(vec![
            PathBuf::from("C:/Users/moi/Desktop/kobi"),
            PathBuf::from("C:/Users/moi/Kobi/reunions"),
        ])
    }

    #[test]
    fn chemins_dans_et_hors_des_dossiers() {
        let c = ctx();
        let k = Path::new("C:/Users/moi/Desktop/kobi");
        assert!(c.contient(Path::new("C:/Users/moi/Desktop/kobi/src/main.rs"), None));
        assert!(c.contient(Path::new(r"c:\users\moi\desktop\KOBI\Cargo.toml"), None));
        assert!(c.contient(Path::new("/c/Users/moi/Desktop/kobi/README.md"), None));
        assert!(c.contient(Path::new("src/lib.rs"), Some(k)));
        assert!(c.contient(Path::new("."), Some(k)));
        assert!(!c.contient(Path::new(".."), Some(k)));
        assert!(!c.contient(Path::new("../../../Windows/System32"), Some(k)));
        assert!(!c.contient(Path::new("C:/Users/moi/Desktop/kobi-autre/x"), None));
        assert!(!c.contient(Path::new("C:/Windows/win.ini"), None));
        assert!(!c.contient(Path::new("/etc/passwd"), None));
        assert!(!c.contient(Path::new("~/.ssh/id_rsa"), Some(k)));
        assert!(!c.contient(Path::new("$HOME/x"), Some(k)));
        assert!(!c.contient(Path::new("src/x"), None));
    }

    #[test]
    fn classement_des_actions() {
        let c = ctx();
        let lire = |p: &str| classer(&Action::LireFichier { chemin: p.into() }, &c);
        assert_eq!(
            lire("C:/Users/moi/Desktop/kobi/src/main.rs"),
            Classe::Lecture
        );
        assert!(matches!(
            lire("C:/Users/moi/.claude/.credentials.json"),
            Classe::Refus(_)
        ));
        let lister = |p: &str| classer(&Action::Lister { dossier: p.into() }, &c);
        assert_eq!(lister("C:/Users/moi/Desktop/kobi/**/*.rs"), Classe::Lecture);
        assert_eq!(lister("**/*.rs"), Classe::Lecture);
        assert!(matches!(lister("C:/Windows/**/*"), Classe::Refus(_)));
        let ecrire = Action::EcrireFichier {
            chemin: "C:/Users/moi/Desktop/kobi/README.md".into(),
            avant: None,
            apres: "x".into(),
        };
        assert_eq!(classer(&ecrire, &c), Classe::Ecriture(Genre::Fichier));
        let mcp = Action::Mcp {
            serveur: "linear".into(),
            outil: "save_issue".into(),
            params: json!({}),
        };
        assert_eq!(classer(&mcp, &c), Classe::Ecriture(Genre::Mcp));
        let mut c2 = c.clone();
        c2.mcp.add_read_only("mcp__linear__save_issue");
        assert_eq!(classer(&mcp, &c2), Classe::Lecture);
        let autre = Action::Autre {
            nom: "WebFetch".into(),
            entree: json!({}),
        };
        assert_eq!(classer(&autre, &c), Classe::Ecriture(Genre::Commande));
        assert_eq!(Classe::Refus("x".into()).cle(), "refus");
    }
}
