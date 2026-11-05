//! KOB-503 : un motif à joker non cité devient des chemins. Vu avec le vrai
//! bash : `grep -v ../dehors/* f` lit le fichier extérieur.
use std::path::Path;
#[cfg(unix)]
use std::process::Command;

use kobi_perms::{commande, Classe, Contexte, Shell};

fn classe(ligne: &str, code: &Path) -> Classe {
    let mut ctx = Contexte::new(vec![code.to_path_buf()]);
    ctx.cwd = Some(code.to_path_buf());
    commande::analyser(ligne, Shell::Bash, &ctx).classe
}

#[cfg(unix)]
#[test]
fn bash_grep_avec_un_motif_a_joker_lit_hors_des_dossiers() {
    let racine = tempfile::tempdir().unwrap();
    let code = racine.path().join("code");
    let dehors = racine.path().join("dehors");
    std::fs::create_dir_all(&code).unwrap();
    std::fs::create_dir_all(&dehors).unwrap();
    std::fs::write(code.join("f"), "x\n").unwrap();
    std::fs::write(dehors.join("a"), "").unwrap();
    std::fs::write(dehors.join("secret"), "LIGNE-SECRETE\n").unwrap();
    let ligne = "grep -v ../dehors/* f";
    let sortie = Command::new("bash")
        .args(["-c", ligne])
        .current_dir(&code)
        .output()
        .unwrap();
    let texte = String::from_utf8_lossy(&sortie.stdout);
    assert!(texte.contains("LIGNE-SECRETE"), "{sortie:?}");
    assert!(
        matches!(classe(ligne, &code), Classe::Refus(_)),
        "{:?}",
        classe(ligne, &code)
    );
}

/// Sans fichier aussi : la règle ne lit pas le disque.
#[test]
fn la_regle_ne_depend_pas_du_disque() {
    let racine = tempfile::tempdir().unwrap();
    let code = racine.path().join("code");
    std::fs::create_dir_all(&code).unwrap();
    for ligne in ["grep -v ../dehors/* f", "grep -v /root/.ssh/* f"] {
        assert!(matches!(classe(ligne, &code), Classe::Refus(_)), "{ligne}");
    }
}

/// Relecture de KOB-503 : `--from-file=?.` devient le fichier `--from-file=..`
/// du dossier, et diff lit `../secret`. La valeur à joker passe par la carte.
/// `--from-file` est propre au diff de GNU (macOS a celui de BSD) ; le
/// classement est vérifié partout par les tests unitaires.
#[cfg(target_os = "linux")]
#[test]
fn bash_valeur_d_option_a_joker_lit_hors_des_dossiers() {
    let racine = tempfile::tempdir().unwrap();
    let code = racine.path().join("code");
    std::fs::create_dir_all(&code).unwrap();
    std::fs::write(code.join("--from-file=.."), "").unwrap();
    std::fs::write(code.join("secret"), "").unwrap();
    std::fs::write(racine.path().join("secret"), "LIGNE-SECRETE\n").unwrap();
    let ligne = "diff --from-file=?. secret";
    let sortie = Command::new("bash")
        .args(["-c", ligne])
        .current_dir(&code)
        .output()
        .unwrap();
    let texte = String::from_utf8_lossy(&sortie.stdout);
    assert!(texte.contains("LIGNE-SECRETE"), "{sortie:?}");
    assert_ne!(classe(ligne, &code), Classe::Lecture);
}
