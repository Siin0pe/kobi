//! KOB-491 : un joker non cité donne à `uniq` un nombre inconnu d'opérandes.
//! Vu avec le vrai bash : `uniq src/*` écrase `src/b`, comme `uniq src/a src/b`.
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::Command;

use kobi_perms::{commande, Classe, Contexte, Shell};

fn classe(ligne: &str, dossier: &Path) -> Classe {
    let mut ctx = Contexte::new(vec![PathBuf::from(dossier)]);
    ctx.cwd = Some(PathBuf::from(dossier));
    commande::analyser(ligne, Shell::Bash, &ctx).classe
}

#[cfg(unix)]
#[test]
fn bash_uniq_avec_un_joker_ecrase_le_second_fichier() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir(d.path().join("src")).unwrap();
    std::fs::write(d.path().join("src/a"), "l\n").unwrap();
    std::fs::write(d.path().join("src/b"), "ANCIEN\n").unwrap();
    let ligne = "uniq src/*";
    let sortie = Command::new("bash")
        .args(["-c", ligne])
        .current_dir(d.path())
        .output()
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(d.path().join("src/b")).unwrap(),
        "l\n",
        "{sortie:?}"
    );
    assert_ne!(classe(ligne, d.path()), Classe::Lecture);
}

/// Sans fichier aussi : la règle ne lit pas le disque, la même ligne peut les créer.
#[test]
fn la_regle_ne_depend_pas_du_disque() {
    let d = tempfile::tempdir().unwrap();
    for ligne in ["uniq src/*", "xxd f*"] {
        assert_ne!(classe(ligne, d.path()), Classe::Lecture, "{ligne}");
    }
}

/// zsh étend `<->` en noms numériques ; bash y voit des redirections
/// (relecture de KOB-491). La ligne passe par la carte.
#[cfg(unix)]
#[test]
fn zsh_uniq_avec_le_joker_numerique_ecrase_le_second_fichier() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("1"), "l\n").unwrap();
    std::fs::write(d.path().join("2"), "ANCIEN\n").unwrap();
    let ligne = "uniq <->>/dev/null";
    let Ok(sortie) = Command::new("zsh")
        .args(["-f", "-c", ligne])
        .current_dir(d.path())
        .output()
    else {
        // Pas de zsh sur cette machine : seul le classement est vérifié. macOS
        // a toujours zsh : la CI y fait la vraie preuve.
        if cfg!(target_os = "macos") {
            panic!("zsh absent sous macOS");
        }
        assert_ne!(classe(ligne, d.path()), Classe::Lecture);
        return;
    };
    assert_eq!(
        std::fs::read_to_string(d.path().join("2")).unwrap(),
        "l\n",
        "{sortie:?}"
    );
    assert_ne!(classe(ligne, d.path()), Classe::Lecture);
}

/// KOB-492 : avec un seul fichier `env`, bash fait de `jq -n e*` un `jq -n env`,
/// qui affiche l'environnement (ici une variable factice).
#[cfg(unix)]
#[test]
fn bash_jq_avec_un_joker_affiche_l_environnement() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("env"), "").unwrap();
    let ligne = "jq -n e*";
    let Ok(sortie) = Command::new("bash")
        .args(["-c", ligne])
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin")
        .env("KOBI_FAUX", "valeur-factice")
        .current_dir(d.path())
        .output()
    else {
        panic!("bash absent");
    };
    // `jq` peut manquer sur la machine : le classement reste vérifié.
    if sortie.status.code() != Some(127) {
        let texte = String::from_utf8_lossy(&sortie.stdout);
        assert!(texte.contains("valeur-factice"), "{sortie:?}");
    }
    assert_ne!(classe(ligne, d.path()), Classe::Lecture);
}

/// KOB-492 : avec un fichier `e`, bash fait de `sed e* f` un `sed e f`, et la
/// commande `e` de sed exécute chaque ligne de `f`.
#[cfg(target_os = "linux")]
#[test]
fn bash_sed_avec_un_joker_execute_les_lignes() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("e"), "").unwrap();
    std::fs::write(d.path().join("f"), "touch execute\n").unwrap();
    let ligne = "sed e* f";
    let sortie = Command::new("bash")
        .args(["-c", ligne])
        .current_dir(d.path())
        .output()
        .unwrap();
    assert!(d.path().join("execute").exists(), "{sortie:?}");
    assert_ne!(classe(ligne, d.path()), Classe::Lecture);
}
