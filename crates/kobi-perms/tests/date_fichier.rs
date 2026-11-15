//! KOB-507 : `date -f fichier` affiche chaque ligne du fichier dans son
//! erreur. Le vrai `date` est appelé sans shell et sans opérande : `-f` ne
//! fait qu'afficher, il ne change jamais l'heure (seuls `-s` et un
//! positionnel le font).
use std::path::Path;
#[cfg(target_os = "linux")]
use std::process::Command;

use kobi_perms::{commande, Classe, Contexte, Shell};

fn classe(ligne: &str, code: &Path) -> Classe {
    let mut ctx = Contexte::new(vec![code.to_path_buf()]);
    ctx.cwd = Some(code.to_path_buf());
    commande::analyser(ligne, Shell::Bash, &ctx).classe
}

/// `-f` est l'option du `date` de GNU (macOS a celui de BSD).
#[cfg(target_os = "linux")]
#[test]
fn date_f_affiche_les_lignes_d_un_fichier_hors_des_dossiers() {
    let racine = tempfile::tempdir().unwrap();
    let code = racine.path().join("code");
    std::fs::create_dir_all(&code).unwrap();
    let secret = racine.path().join("secret");
    std::fs::write(&secret, "LIGNE-SECRETE\n").unwrap();
    let sortie = Command::new("date")
        .arg("-f")
        .arg(&secret)
        .current_dir(&code)
        .output()
        .unwrap();
    let texte = String::from_utf8_lossy(&sortie.stderr);
    assert!(texte.contains("LIGNE-SECRETE"), "{sortie:?}");
    assert_ne!(classe("date -f ../secret", &code), Classe::Lecture);
}

/// Sans fichier aussi : la règle ne lit pas le disque.
#[test]
fn la_regle_ne_depend_pas_du_disque() {
    let racine = tempfile::tempdir().unwrap();
    let code = racine.path().join("code");
    std::fs::create_dir_all(&code).unwrap();
    for ligne in ["date -f ../secret", "date -r ../secret", "date --file=x"] {
        assert_ne!(classe(ligne, &code), Classe::Lecture, "{ligne}");
    }
}
