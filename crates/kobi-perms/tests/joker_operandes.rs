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
