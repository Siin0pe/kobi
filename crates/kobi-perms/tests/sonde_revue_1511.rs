//! SONDE (ne pas merger) : revue croisée du 15/11 des merges #482 à #500.
//! Chaque sonde vérifie d'abord le vrai shell, puis le classement sur main.

use std::path::{Path, PathBuf};
use std::process::Command;

use kobi_perms::{commande, Classe, Contexte, Shell};

fn dossiers() -> (tempfile::TempDir, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let ici = d.path().join("ici");
    std::fs::create_dir_all(ici.join("sub")).unwrap();
    std::fs::create_dir_all(d.path().join("dehors")).unwrap();
    std::fs::write(
        d.path().join("dehors").join("secret.txt"),
        "x-contenu-secret\n",
    )
    .unwrap();
    (d, ici)
}

fn classe(ligne: &str, shell: Shell, ici: &Path) -> Classe {
    let mut ctx = Contexte::new(vec![ici.to_path_buf()]);
    ctx.cwd = Some(ici.to_path_buf());
    commande::analyser(ligne, shell, &ctx).classe
}

/// Sortie du vrai pwsh ; None s'il manque (hors CI).
fn pwsh(ligne: &str, ici: &Path, home: Option<&Path>) -> Option<String> {
    let mut c = Command::new("pwsh");
    c.args(["-NoProfile", "-NonInteractive", "-Command", ligne])
        .current_dir(ici);
    if let Some(h) = home {
        c.env("HOME", h);
    }
    let o = c.output().ok()?;
    Some(format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    ))
}

/// (1) `-SimpleMatch` est un interrupteur : le mot suivant est le chemin.
#[test]
fn sonde_1_simplematch_vrai_pwsh() {
    let (_d, ici) = dossiers();
    let Some(t) = pwsh(
        "Select-String x -SimpleMatch ../dehors/secret.txt",
        &ici,
        None,
    ) else {
        if cfg!(windows) {
            panic!("pwsh absent sous Windows");
        }
        return;
    };
    assert!(t.contains("contenu-secret"), "{t:?}");
}

#[test]
fn sonde_1_simplematch_classement() {
    let (_d, ici) = dossiers();
    assert!(matches!(
        classe(
            "Select-String x ../dehors/secret.txt",
            Shell::PowerShell,
            &ici
        ),
        Classe::Refus(_)
    ));
    for ligne in [
        "Select-String x -SimpleMatch ../dehors/secret.txt",
        "Select-String -Pattern x -SimpleMatch ../dehors/secret.txt",
        "sls zzz -NotMatch -SimpleMatch ../dehors/secret.txt",
        "Select-String x -SimpleMatch ../dehors/*",
    ] {
        assert_ne!(
            classe(ligne, Shell::PowerShell, &ici),
            Classe::Lecture,
            "{ligne}"
        );
    }
}

/// (2) pwsh 7 : `-Filter` avec un dossier remonte-t-il ?
#[test]
fn sonde_2_gci_filter_vrai_pwsh() {
    let (_d, ici) = dossiers();
    for ligne in [
        "Get-ChildItem -Filter ../dehors/* | Get-Content",
        "Get-ChildItem -Filter ../dehors/secret.txt | Get-Content",
    ] {
        let Some(t) = pwsh(ligne, &ici, None) else {
            return;
        };
        assert!(t.contains("contenu-secret"), "{ligne} : {t:?}");
    }
}

#[test]
fn sonde_2_gci_filter_classement() {
    let (_d, ici) = dossiers();
    for ligne in [
        "Get-ChildItem -Filter ../dehors/* | Get-Content",
        "gci -Filter ../dehors/secret.txt | Get-Content",
    ] {
        assert_ne!(
            classe(ligne, Shell::PowerShell, &ici),
            Classe::Lecture,
            "{ligne}"
        );
    }
}

/// (3) Un `ls` natif dans un pipe vers Get-ChildItem : le texte `~` est le
/// dossier personnel.
#[cfg(unix)]
#[test]
fn sonde_3_ls_natif_vrai_pwsh() {
    let (d, ici) = dossiers();
    let maison = d.path().join("maison");
    std::fs::create_dir_all(&maison).unwrap();
    std::fs::write(maison.join("SECRET_MAISON"), "contenu-maison").unwrap();
    std::fs::write(ici.join("sub").join("~"), "").unwrap();
    let Some(t) = pwsh("ls sub | Get-ChildItem | Get-Content", &ici, Some(&maison)) else {
        return;
    };
    assert!(t.contains("contenu-maison"), "{t:?}");
}

#[cfg(unix)]
#[test]
fn sonde_3_ls_natif_classement() {
    let (_d, ici) = dossiers();
    std::fs::write(ici.join("sub").join("~"), "").unwrap();
    let ligne = "ls sub | Get-ChildItem | Get-Content";
    assert_ne!(classe(ligne, Shell::PowerShell, &ici), Classe::Lecture);
}

/// (4) BSD ls (macOS) accepte `-attributes` : il liste le dossier suivant.
#[cfg(target_os = "macos")]
#[test]
fn sonde_4_ls_bsd_vrai() {
    let (_d, ici) = dossiers();
    let o = Command::new("ls")
        .args(["-attributes", "../dehors"])
        .current_dir(&ici)
        .output()
        .unwrap();
    let t = String::from_utf8_lossy(&o.stdout);
    assert!(t.contains("secret.txt"), "{t:?}");
}

#[test]
fn sonde_4_ls_attributes_classement() {
    let (_d, ici) = dossiers();
    for ligne in ["ls -attributes ../dehors", "ls -filter ../dehors"] {
        assert_ne!(classe(ligne, Shell::Bash, &ici), Classe::Lecture, "{ligne}");
    }
}
