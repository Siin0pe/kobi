//! KOB-518 : PowerShell en mode Legacy (Windows PowerShell 5.1 toujours, pwsh
//! sous Unix jusqu'à 7.2) retire un argument vide avant de lancer un
//! exécutable natif. Vu avec le vrai shell ; sur la CI, le shell doit être là.
use std::path::{Path, PathBuf};
use std::process::Command;

use kobi_perms::{commande, Classe, Contexte, Shell};

fn classe(ligne: &str, dossier: &Path) -> Classe {
    let mut ctx = Contexte::new(vec![PathBuf::from(dossier)]);
    ctx.cwd = Some(PathBuf::from(dossier));
    commande::analyser(ligne, Shell::PowerShell, &ctx).classe
}

/// Sur la CI (`CI` posé par GitHub), un shell absent est une erreur : la
/// preuve ne doit pas passer par la branche de repli.
fn exige(present: bool, shell: &str) -> bool {
    if !present && std::env::var("CI").is_ok() {
        panic!("{shell} absent sur la CI");
    }
    present
}

/// pwsh sous Linux, mode Legacy : `jq -n '' e*` devient `jq -n env`.
#[cfg(target_os = "linux")]
#[test]
fn pwsh_legacy_jq_affiche_l_environnement() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("env"), "").unwrap();
    let ligne = "jq -n '' e*";
    let sortie = Command::new("pwsh")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!("$PSNativeCommandArgumentPassing='Legacy'; {ligne}"),
        ])
        .current_dir(d.path())
        .env("KOBI_FAUX", "valeur-factice")
        .output();
    if exige(sortie.is_ok(), "pwsh") {
        let sortie = sortie.unwrap();
        let texte = String::from_utf8_lossy(&sortie.stdout);
        assert!(texte.contains("valeur-factice"), "{sortie:?}");
    }
    assert_eq!(
        classe(ligne, d.path()),
        Classe::Ecriture(kobi_perms::Genre::Indetermine)
    );
}

/// Windows PowerShell 5.1 (toujours en Legacy) : un argument vide ne
/// parvient pas à l'exécutable, qui voit donc des positionnels décalés.
#[cfg(windows)]
#[test]
fn powershell_51_retire_un_argument_vide() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(
        d.path().join("argv.py"),
        "import sys\nprint(repr(sys.argv[1:]))\n",
    )
    .unwrap();
    let sortie = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "python argv.py -n '' x",
        ])
        .current_dir(d.path())
        .output();
    if exige(sortie.is_ok(), "powershell") {
        let sortie = sortie.unwrap();
        let texte = String::from_utf8_lossy(&sortie.stdout);
        assert!(texte.contains("['-n', 'x']"), "{sortie:?}");
    }
    assert_eq!(
        classe("jq -n '' f", d.path()),
        Classe::Ecriture(kobi_perms::Genre::Indetermine)
    );
}

/// Relecture de KOB-518 : un `"` impair dans un mot sans blanc fait déborder les
/// guillemets sur l'argument suivant. `head 'a"' 'b ../secret'` devient
/// `head "a b" ../secret` et lit le fichier extérieur.
#[cfg(target_os = "linux")]
#[test]
fn pwsh_legacy_guillemet_impair_lit_hors_du_dossier() {
    let racine = tempfile::tempdir().unwrap();
    let code = racine.path().join("code");
    std::fs::create_dir_all(&code).unwrap();
    std::fs::write(racine.path().join("secret"), "LIGNE-SECRETE\n").unwrap();
    let ligne = "head 'a\"' 'b ../secret'";
    let sortie = Command::new("pwsh")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!("$PSNativeCommandArgumentPassing='Legacy'; {ligne}"),
        ])
        .current_dir(&code)
        .output();
    if exige(sortie.is_ok(), "pwsh") {
        let sortie = sortie.unwrap();
        let texte = String::from_utf8_lossy(&sortie.stdout);
        assert!(texte.contains("LIGNE-SECRETE"), "{sortie:?}");
    }
    assert_eq!(
        classe(ligne, &code),
        Classe::Ecriture(kobi_perms::Genre::Indetermine)
    );
}
