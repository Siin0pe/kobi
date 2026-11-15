//! KOB-539 : les paramètres de `Get-ChildItem` (`-Filter`, `-Attributes`…) ne
//! valent que pour la cmdlet. Le ls BSD de macOS lit `-attributes` comme une
//! suite d'options d'une lettre, toutes valides, et liste le dossier suivant.
//! Preuve : sonde #506 (run 43755944282), tests de kobi-b repris de
//! `revue_1511.rs` (`sonde_4_*`).

use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
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

/// Le vrai ls BSD accepte `-attributes` et liste le dossier suivant.
#[cfg(target_os = "macos")]
#[test]
fn ls_bsd_liste_le_mot_apres_attributes() {
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
fn ls_attributes_hors_du_dossier_n_est_pas_une_lecture() {
    let (_d, ici) = dossiers();
    for ligne in [
        "ls -attributes ../dehors",
        "ls -filter ../dehors",
        "ls -include ../dehors",
        "ls -exclude ../dehors",
        "ls -depth ../dehors",
        "dir -attributes ../dehors",
        "ls -Attributes ../dehors/secret.txt",
    ] {
        assert_ne!(classe(ligne, Shell::Bash, &ici), Classe::Lecture, "{ligne}");
    }
    // `ls.exe` est toujours le programme, même sous PowerShell.
    assert_ne!(
        classe("ls.exe -attributes ../dehors", Shell::PowerShell, &ici),
        Classe::Lecture
    );
}

/// Hors de Windows, `ls` sous PowerShell est le programme natif : même règle.
#[cfg(unix)]
#[test]
fn ls_natif_sous_pwsh_hors_windows() {
    let (_d, ici) = dossiers();
    assert_ne!(
        classe("ls -attributes ../dehors", Shell::PowerShell, &ici),
        Classe::Lecture
    );
}

/// La cmdlet garde ses paramètres : leur valeur n'est pas un chemin.
#[test]
fn la_cmdlet_garde_ses_parametres() {
    let (_d, ici) = dossiers();
    for ligne in [
        "Get-ChildItem -Filter *.rs sub",
        "gci -Attributes Hidden sub",
        "Get-ChildItem -Exclude target sub",
    ] {
        assert_eq!(
            classe(ligne, Shell::PowerShell, &ici),
            Classe::Lecture,
            "{ligne}"
        );
    }
    // Sous Windows, `ls` est l'alias de la cmdlet.
    #[cfg(windows)]
    assert_eq!(
        classe("ls -Filter *.rs sub", Shell::PowerShell, &ici),
        Classe::Lecture
    );
    // Sous Bash, le ls du dossier reste une lecture.
    assert_eq!(classe("ls -la sub", Shell::Bash, &ici), Classe::Lecture);
}
