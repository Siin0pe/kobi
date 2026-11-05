//! KOB-489 : un joker non cité en tête de mot peut devenir une option. Vu avec
//! le vrai shell : le fichier piégé est un nom d'option, et la règle ne dépend
//! pas du disque (`touch -- --pre=x; rg a *` crée le piège dans la même ligne).
use std::path::{Path, PathBuf};
use std::process::Command;

use kobi_perms::{commande, Classe, Contexte, Shell};

fn classe(ligne: &str, shell: Shell, dossier: &Path) -> Classe {
    let mut ctx = Contexte::new(vec![PathBuf::from(dossier)]);
    ctx.cwd = Some(PathBuf::from(dossier));
    commande::analyser(ligne, shell, &ctx).classe
}

/// Bash étend le joker en une option : `sort` écrit le fichier qu'elle nomme.
#[cfg(unix)]
#[test]
fn bash_le_joker_devient_une_option() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("--output=M"), "").unwrap();
    std::fs::write(d.path().join("a"), "x\n").unwrap();
    let ligne = "sort *";
    let ok = Command::new("bash")
        .args(["-c", ligne])
        .current_dir(d.path())
        .output()
        .unwrap();
    assert!(d.path().join("M").exists(), "{ok:?}");
    assert_ne!(classe(ligne, Shell::Bash, d.path()), Classe::Lecture);
}

/// Dans un dossier sans piège aussi : la règle ne lit pas le disque.
#[test]
fn la_regle_ne_depend_pas_du_disque() {
    let d = tempfile::tempdir().unwrap();
    for ligne in ["sort *", "rg a *", "touch -- --pre=x; rg a *"] {
        assert_ne!(
            classe(ligne, Shell::Bash, d.path()),
            Classe::Lecture,
            "{ligne}"
        );
    }
}

/// `pwsh` hors Windows étend le joker pour un exécutable natif.
#[cfg(unix)]
#[test]
fn pwsh_hors_windows_etend_pour_un_natif() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("--piege"), "").unwrap();
    let Ok(sortie) = Command::new("pwsh")
        .args(["-NoProfile", "-NonInteractive", "-Command", "/bin/echo *"])
        .current_dir(d.path())
        .output()
    else {
        // Pas de pwsh sur cette machine : le test tourne sur la CI.
        return;
    };
    let texte = String::from_utf8_lossy(&sortie.stdout);
    assert!(texte.contains("--piege"), "{texte:?}");
    assert_ne!(
        classe("rg a *", Shell::PowerShell, d.path()),
        Classe::Lecture
    );
    assert_ne!(
        classe("sort *", Shell::PowerShell, d.path()),
        Classe::Lecture
    );
}

/// Sous Windows, PowerShell n'étend rien pour un exécutable natif : `*` reste `*`.
#[cfg(windows)]
#[test]
fn powershell_sous_windows_n_etend_pas_pour_un_natif() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("--piege"), "").unwrap();
    for exe in ["powershell", "pwsh"] {
        let sortie = Command::new(exe)
            .args(["-NoProfile", "-NonInteractive", "-Command", "cmd /c echo *"])
            .current_dir(d.path())
            .output()
            .unwrap();
        let texte = String::from_utf8_lossy(&sortie.stdout);
        assert!(
            !texte.contains("--piege") && texte.contains('*'),
            "{exe} {texte:?}"
        );
    }
}

/// Une cmdlet reçoit le motif sans expansion par le shell : reste une lecture.
#[test]
fn cmdlet_avec_joker_en_lecture() {
    let d = tempfile::tempdir().unwrap();
    for ligne in ["Get-ChildItem *.md", "Get-ChildItem -Filter *.rs"] {
        assert_eq!(
            classe(ligne, Shell::PowerShell, d.path()),
            Classe::Lecture,
            "{ligne}"
        );
    }
}
