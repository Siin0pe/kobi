//! KOB-525 : jq charge `~/.jq` tout seul. Une fonction de ce fichier au nom anodin
//! (`def ouvrir: env;`) affiche l'environnement : avec un `~/.jq`, toute commande
//! jq passe par la carte. Vu avec le vrai jq, dans un HOME isolé et une variable
//! factice.
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::Command;

use kobi_perms::{commande, Classe, Contexte, Shell};

fn classe(ligne: &str, code: &Path, home: Option<&Path>) -> Classe {
    let mut ctx = Contexte::new(vec![code.to_path_buf()]);
    ctx.cwd = Some(code.to_path_buf());
    ctx.dossiers_personnels = home.map(PathBuf::from).into_iter().collect();
    commande::analyser(ligne, Shell::Bash, &ctx).classe
}

#[test]
fn avec_un_fichier_jq_toute_commande_jq_sur_la_carte() {
    let home = tempfile::tempdir().unwrap();
    let code = home.path().join("code");
    std::fs::create_dir_all(&code).unwrap();
    // Sans ~/.jq : la lecture de jq reste une lecture.
    assert_eq!(
        classe("jq -n ouvrir", &code, Some(home.path())),
        Classe::Lecture
    );
    assert_eq!(
        classe("jq . a.json", &code, Some(home.path())),
        Classe::Lecture
    );
    std::fs::write(home.path().join(".jq"), "def ouvrir: env;\n").unwrap();
    for ligne in ["jq -n ouvrir", "jq . a.json", "jq -r .x a.json"] {
        assert_ne!(
            classe(ligne, &code, Some(home.path())),
            Classe::Lecture,
            "{ligne}"
        );
    }
    // HOME inconnu : rien n'y est cherché.
    assert_eq!(classe("jq -n ouvrir", &code, None), Classe::Lecture);
}

/// jq 1.7 et plus charge `~/jq/main.jq` quand il n'y a pas de `~/.jq` (relecture
/// de KOB-525).
#[test]
fn avec_jq_main_jq_toute_commande_jq_sur_la_carte() {
    let home = tempfile::tempdir().unwrap();
    let code = home.path().join("code");
    std::fs::create_dir_all(home.path().join("jq")).unwrap();
    std::fs::create_dir_all(&code).unwrap();
    std::fs::write(home.path().join("jq").join("main.jq"), "def ouvrir: env;\n").unwrap();
    assert_ne!(
        classe("jq -n ouvrir", &code, Some(home.path())),
        Classe::Lecture
    );
}

/// Le vrai jq charge `~/.jq` : la fonction de l'utilisateur affiche la variable.
#[cfg(unix)]
#[test]
fn vrai_jq_charge_le_fichier_perso() {
    let home = tempfile::tempdir().unwrap();
    let code = home.path().join("code");
    std::fs::create_dir_all(&code).unwrap();
    std::fs::write(home.path().join(".jq"), "def ouvrir: env;\n").unwrap();
    let Ok(sortie) = Command::new("jq")
        .args(["-n", "ouvrir"])
        .current_dir(&code)
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin")
        .env("HOME", home.path())
        .env("KOBI_FAUX", "valeur-factice")
        .output()
    else {
        return;
    };
    if sortie.status.code() != Some(127) {
        assert!(
            String::from_utf8_lossy(&sortie.stdout).contains("valeur-factice"),
            "{sortie:?}"
        );
    }
    assert_ne!(
        classe("jq -n ouvrir", &code, Some(home.path())),
        Classe::Lecture
    );
}
