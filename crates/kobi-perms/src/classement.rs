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
    /// Sur le texte, puis sur le disque : chemin et dossiers sont résolus
    /// sans rien suivre ni ouvrir ([`resolu`]), un lien remplacé par sa
    /// cible et `..` appliqué au chemin déjà résolu (KOB-432). Sans cela,
    /// `code/sortie/secret`, où `sortie` vise `~/.ssh`, passait pour un
    /// chemin du dossier.
    pub fn contient(&self, chemin: &Path, depuis: Option<&Path>) -> bool {
        let Some(cible) = absolu(chemin, depuis) else {
            return false;
        };
        if !self.dossiers.iter().any(|d| sous(&cible, d)) {
            return false;
        }
        let brut = match depuis {
            Some(d) if !est_absolu(&chemin.to_string_lossy().replace('\\', "/")) => d.join(chemin),
            _ => PathBuf::from(git_bash_vers_windows(
                &chemin.to_string_lossy().replace('\\', "/"),
            )),
        };
        // Deux lectures de `..` : physique (le système, `cat`) et sur le
        // texte (Claude Code, Win32, PowerShell, Git Bash le réduisent avant
        // d'ouvrir). Les deux doivent rester dans les dossiers : sinon
        // `s1/../y`, où `s1` reste dedans et `y` sort, passait.
        let dossiers: Vec<PathBuf> = self.dossiers.iter().filter_map(|d| resolu(d)).collect();
        [brut, cible]
            .iter()
            .all(|p| resolu(p).is_some_and(|reel| dossiers.iter().any(|d| sous(&reel, d))))
    }
}

/// `cible` est `dossier` ou en dessous, sur le texte (casse ignorée).
fn sous(cible: &Path, dossier: &Path) -> bool {
    let (cible, d) = (cle_de_chemin(cible), cle_de_chemin(dossier));
    !d.is_empty() && (cible == d || cible.starts_with(&format!("{d}/")))
}

/// Chemin résolu sans rien suivre ni ouvrir : chaque composant existant est
/// lu avec `symlink_metadata`, un lien est remplacé par sa cible
/// (`read_link`), et `..` remonte le chemin déjà résolu, comme le système.
/// Rien n'est résolu par `canonicalize` : sous Windows, suivre un lien vers
/// un partage réseau ouvrirait déjà une connexion SMB (KOB-344). Après un
/// composant absent (fichier à créer), la suite est prise telle quelle :
/// rien n'y peut être un lien. `None` : cible réseau ou de périphérique,
/// trop de liens (boucle), ou erreur de lecture autre qu'« absent ».
pub fn resolu(chemin: &Path) -> Option<PathBuf> {
    use std::collections::VecDeque;
    let mut a_lire: VecDeque<PathBuf> = chemin
        .components()
        .map(|c| PathBuf::from(c.as_os_str()))
        .collect();
    let mut actuel = PathBuf::new();
    let mut liens = 0;
    // Composants absents au bout du chemin (fichier ou dossier à créer).
    let mut absents = 0usize;
    while let Some(morceau) = a_lire.pop_front() {
        match morceau.components().next() {
            None | Some(Component::CurDir) => continue,
            // Après un composant absent, `..` le retire, comme un outil qui
            // réduit sur le texte ; revenu sur ce qui existe, la suite est de
            // nouveau lue sur le disque, pas ajoutée sans être vue (KOB-432).
            Some(Component::ParentDir) => {
                actuel.pop();
                absents = absents.saturating_sub(1);
                continue;
            }
            Some(Component::Prefix(_) | Component::RootDir) => {
                actuel.push(&morceau);
                continue;
            }
            Some(Component::Normal(_)) => {}
        }
        let candidat = actuel.join(&morceau);
        if absents > 0 {
            actuel = candidat;
            absents += 1;
            continue;
        }
        match std::fs::symlink_metadata(&candidat) {
            Ok(m) if m.file_type().is_symlink() => {
                liens += 1;
                if liens > LIENS_MAX {
                    return None;
                }
                let lien = std::fs::read_link(&candidat).ok()?;
                let cible = cible_locale(&lien, &actuel)?;
                actuel = PathBuf::new();
                for c in cible.components().rev() {
                    a_lire.push_front(PathBuf::from(c.as_os_str()));
                }
            }
            Ok(_) => actuel = candidat,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                absents = 1;
                actuel = candidat;
            }
            Err(_) => return None,
        }
    }
    Some(actuel)
}

/// Liens suivis au plus pour un chemin, comme Linux (KOB-432).
const LIENS_MAX: usize = 40;

/// Cible d'un lien, rendue absolue depuis le dossier du lien. `None` pour
/// une cible réseau ou de périphérique (`\\serveur\…`, `\\?\UNC\…`,
/// `\\.\…`, `\??\…`, racine sans disque sous Windows) : refusée sans être
/// touchée. Un `\\?\C:\…` local perd son préfixe, pour se comparer aux
/// dossiers déclarés.
fn cible_locale(lien: &Path, dossier_du_lien: &Path) -> Option<PathBuf> {
    // Sous Windows, NT ne coupe une cible que sur `\\` : un `/` la ferait
    // juger autrement qu'il ne l'ouvre (KOB-425).
    if cfg!(windows) && lien.to_string_lossy().contains('/') {
        return None;
    }
    let texte = lien.to_string_lossy().replace('/', "\\");
    if let Some(reste) = texte.strip_prefix("\\\\?\\") {
        let b = reste.as_bytes();
        // Sans son préfixe, un nom à point ou espace final (`b.`) serait lu
        // par Win32 comme `b` : NT, lui, ouvre `b.` (KOB-398).
        let fin_ambigue = reste
            .split('\\')
            .any(|n| n.ends_with('.') && n != "." && n != ".." || n.ends_with(' '));
        return (b.len() >= 3
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && b[2] == b'\\'
            && !fin_ambigue)
            .then(|| PathBuf::from(reste));
    }
    if texte.starts_with("\\\\") || texte.starts_with("\\??\\") {
        return None;
    }
    if cfg!(windows)
        && lien.has_root()
        && !matches!(lien.components().next(), Some(Component::Prefix(_)))
    {
        return None;
    }
    if lien.is_absolute() || (cfg!(windows) && lien.has_root()) {
        Some(lien.to_path_buf())
    } else if cfg!(windows) && matches!(lien.components().next(), Some(Component::Prefix(_))) {
        // `C:x` : relatif au dossier courant de ce lecteur, inconnu ici.
        None
    } else {
        Some(dossier_du_lien.join(lien))
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

/// Forme comparable : `/` partout, sans `/` final, en minuscules sous
/// Windows et macOS, qui ignorent la casse. Sous Linux, `Code` et `code`
/// sont deux dossiers (KOB-432).
fn cle_de_chemin(chemin: &Path) -> String {
    let s = chemin.to_string_lossy().replace('\\', "/");
    let mut s = if cfg!(any(windows, target_os = "macos")) {
        s.to_lowercase()
    } else {
        s
    };
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

    /// KOB-432 : un lien sous un dossier déclaré est suivi sans être ouvert :
    /// ce qui sort par un lien est hors des dossiers, pour la lecture,
    /// l'écriture et les commandes ; ce qui y reste est permis.
    #[cfg(unix)]
    #[test]
    fn liens_qui_sortent_hors_des_dossiers() {
        use std::os::unix::fs::symlink;
        let code = tempfile::tempdir().unwrap();
        let dehors = tempfile::tempdir().unwrap();
        let (c, d) = (code.path(), dehors.path());
        std::fs::write(d.join("secret"), "x").unwrap();
        std::fs::create_dir_all(c.join("sous")).unwrap();
        std::fs::write(c.join("sous/permis"), "x").unwrap();
        std::fs::create_dir_all(d.join("ssl")).unwrap();
        // Lien qui sort, lien qui reste, chaîne qui sort, lien relatif qui
        // sort, liens cassés dedans et dehors, boucle.
        symlink(d, c.join("sortie")).unwrap();
        symlink(c.join("sous"), c.join("dedans")).unwrap();
        symlink(c.join("maillon2"), c.join("maillon1")).unwrap();
        symlink(d, c.join("maillon2")).unwrap();
        let profondeur = c.components().count();
        symlink(
            format!(
                "{}{}",
                "../".repeat(profondeur),
                d.strip_prefix("/").unwrap().display()
            ),
            c.join("relatif"),
        )
        .unwrap();
        symlink(c.join("absent"), c.join("casse_dedans")).unwrap();
        symlink(d.join("absent"), c.join("casse_dehors")).unwrap();
        symlink(c.join("boucle"), c.join("boucle")).unwrap();
        symlink(d.join("ssl"), c.join("ssl")).unwrap();
        // `s1` reste dedans (`sous/profond`), `sous/y` est ordinaire, `y` sort :
        // `s1/../y` mène à `sous/y` pour le système, à `y` pour un outil qui
        // réduit `..` sur le texte (relecture sécurité de KOB-432).
        std::fs::create_dir_all(c.join("sous/profond")).unwrap();
        std::fs::write(c.join("sous/y"), "x").unwrap();
        symlink(c.join("sous/profond"), c.join("s1")).unwrap();
        symlink(d.join("secret"), c.join("y")).unwrap();
        let mut x = Contexte::new(vec![c.to_path_buf()]);
        x.cwd = Some(c.to_path_buf());
        let lire = |p: &str| classer(&Action::LireFichier { chemin: c.join(p) }, &x).cle();
        let ecrire = |p: &str| {
            classer(
                &Action::EcrireFichier {
                    chemin: c.join(p),
                    avant: None,
                    apres: String::new(),
                },
                &x,
            )
            .cle()
        };
        let cat = |p: &str| {
            commande::analyser(&format!("cat {p}"), crate::Shell::Bash, &x)
                .classe
                .cle()
        };
        for dehors in [
            "sortie/secret",
            "maillon1/secret",
            "relatif/secret",
            "casse_dehors",
            "boucle",
            // `..` après un lien remonte depuis sa cible, pas depuis le lien.
            "ssl/../secret",
            // Réduits sur le texte par l'outil qui ouvre : `y`, qui sort.
            "s1/../y",
            "absent/../y",
        ] {
            assert_eq!(lire(dehors), "refus", "lecture {dehors}");
            assert_eq!(ecrire(dehors), "refus", "écriture {dehors}");
            assert_eq!(cat(dehors), "refus", "commande {dehors}");
        }
        for dedans in [
            "dedans/permis",
            "sous/permis",
            "casse_dedans",
            "nouveau.txt",
        ] {
            assert_eq!(lire(dedans), "lecture", "lecture {dedans}");
            assert_eq!(ecrire(dedans), "ecriture", "écriture {dedans}");
            assert_eq!(cat(dedans), "lecture", "commande {dedans}");
        }
        // Motifs que le shell développe vers le lien qui sort, et options qui
        // suivent les liens en parcourant : jamais une lecture sans carte.
        for ligne in [
            "cat sorti?/secret",
            "cat */secret",
            "cat {sortie,x}/secret",
            "cat [s]ortie/secret",
            // Ce que notre lecture du motif ne prend pas, mais un shell si.
            "cat [[:alpha:]]ortie/secret",
            "cat [!]a]ortie/secret",
            "cat .*/secret",
            "cat {x}/{sortie,q}/secret",
            "cat **/secret",
            "cat +(sortie)/secret",
            "grep -R motif .",
            "rg -L motif",
            "find -L .",
            "ls -L sous",
            "du -L .",
            "tree -l",
            "diff -r sous dedans",
        ] {
            assert_ne!(
                commande::analyser(ligne, crate::Shell::Bash, &x)
                    .classe
                    .cle(),
                "lecture",
                "{ligne}"
            );
        }
        // PowerShell compare les jokers sans la casse : `SORTI?` désigne
        // `sortie`, que notre lecture du motif écarte ; jamais lu sans carte.
        assert_ne!(
            commande::analyser("Get-Content SORTI?/secret", crate::Shell::PowerShell, &x)
                .classe
                .cle(),
            "lecture"
        );
        // Un motif qui ne désigne que des fichiers du dossier reste une lecture.
        assert_eq!(cat("sous/*"), "lecture");
        // Sous Linux, la casse compte : `projet` n'est pas `Projet`.
        #[cfg(target_os = "linux")]
        {
            std::fs::create_dir_all(c.join("Projet")).unwrap();
            std::fs::create_dir_all(c.join("projet")).unwrap();
            let p = Contexte::new(vec![c.join("Projet")]);
            assert!(p.contient(&c.join("Projet/f"), None));
            assert!(!p.contient(&c.join("projet/f"), None));
        }
        // Le dossier de la commande lui-même, par un lien qui sort.
        let mut par_lien = x.clone();
        par_lien.cwd = Some(c.join("sortie"));
        assert_eq!(
            commande::analyser("cat secret", crate::Shell::Bash, &par_lien)
                .classe
                .cle(),
            "refus"
        );
    }

    /// KOB-432, sous Windows : un lien vers un partage réseau est refusé sans
    /// être suivi (aucune connexion SMB) ; une cible à `/` aussi (KOB-425).
    #[cfg(windows)]
    #[test]
    fn liens_reseau_refuses_sans_les_suivre() {
        use std::os::windows::fs::{symlink_dir, symlink_file};
        let code = tempfile::tempdir().unwrap();
        let c = code.path();
        if symlink_dir(r"\\attaquant\p", c.join("reseau")).is_err() {
            assert!(std::env::var_os("CI").is_none(), "liens impossibles en CI");
            return;
        }
        std::fs::create_dir_all(c.join("x")).unwrap();
        std::fs::write(c.join("y"), "x").unwrap();
        symlink_file(r"x\../y", c.join("barre")).unwrap();
        let x = Contexte::new(vec![c.to_path_buf()]);
        let debut = std::time::Instant::now();
        assert!(!x.contient(&c.join(r"reseau\fichier"), None));
        assert!(!x.contient(&c.join("barre"), None));
        // Cible verbatim à point final : NT ouvre `b.`, Win32 lirait `b`.
        let point = format!(r"\\?\{}\b.", c.display());
        std::fs::create_dir_all(&point).unwrap();
        symlink_dir(&point, c.join("point")).unwrap();
        assert!(!x.contient(&c.join(r"point\f"), None), "{point}");
        // Sans tentative de connexion : la réponse est immédiate.
        assert!(
            debut.elapsed() < std::time::Duration::from_secs(2),
            "{:?}",
            debut.elapsed()
        );
        assert!(x.contient(&c.join("y"), None));
    }

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
        // Casse ignorée là où le système l'ignore (Windows, macOS).
        #[cfg(any(windows, target_os = "macos"))]
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
