//! KOB-488 : `git version --help` lance le `man.<outil>.cmd` de la config du
//! dépôt, avec le vrai git. Il ne doit donc jamais être une lecture.
#![cfg(unix)]

use std::path::Path;
use std::process::Command;

use kobi_perms::{commande, Classe, Contexte, Shell};

/// Un dépôt dont la config lance un marqueur à l'ouverture de l'aide.
fn depot_hostile(dossier: &Path) {
    let git = |args: &[&str]| {
        let ok = Command::new("git")
            .args(args)
            .current_dir(dossier)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap())
            .env("HOME", dossier.join("home"))
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    };
    std::fs::create_dir_all(dossier.join("home")).unwrap();
    git(&["init", "-q"]);
    let cmd = format!("touch {}; true", dossier.join("MARQUEUR").display());
    git(&["config", "help.format", "man"]);
    git(&["config", "man.viewer", "x"]);
    git(&["config", "man.x.cmd", &cmd]);
}

fn lance_le_marqueur(args: &[&str]) -> bool {
    let d = tempfile::tempdir().unwrap();
    depot_hostile(d.path());
    Command::new("git")
        .args(args)
        .current_dir(d.path())
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap())
        .env("HOME", d.path().join("home"))
        .output()
        .unwrap();
    d.path().join("MARQUEUR").exists()
}

fn classe(ligne: &str) -> Classe {
    let d = tempfile::tempdir().unwrap();
    let mut ctx = Contexte::new(vec![d.path().to_path_buf()]);
    ctx.cwd = Some(d.path().to_path_buf());
    commande::analyser(ligne, Shell::Bash, &ctx).classe
}

/// Le danger est réel, et la ligne passe par la carte.
#[test]
fn version_help_lance_le_programme_et_passe_par_la_carte() {
    assert!(lance_le_marqueur(&["version", "--help"]));
    assert_ne!(classe("git version --help"), Classe::Lecture);
}

/// Témoin : `git version` seul ne lance rien et reste une lecture.
#[test]
fn version_seul_ne_lance_rien_et_reste_une_lecture() {
    assert!(!lance_le_marqueur(&["version"]));
    assert_eq!(classe("git version"), Classe::Lecture);
}
