//! KOB-504 : un joker non cité dans le script de sed. Vu avec le vrai bash et
//! le sed de GNU : dans un dossier qui contient l'arbre `1,/x\/,/e CMD #/p`,
//! `sed -n 1,/x*/,/e*/p f` devient `sed -n '1,/x\/,/e CMD #/p' f`, qui exécute
//! CMD. L'adresse est relative : le chemin reste dans le dossier.
use std::path::Path;
#[cfg(target_os = "linux")]
use std::process::Command;

use kobi_perms::{commande, Classe, Contexte, Shell};

fn classe(ligne: &str, code: &Path) -> Classe {
    let mut ctx = Contexte::new(vec![code.to_path_buf()]);
    ctx.cwd = Some(code.to_path_buf());
    commande::analyser(ligne, Shell::Bash, &ctx).classe
}

/// La commande `e` est propre au sed de GNU (macOS a celui de BSD).
#[cfg(target_os = "linux")]
#[test]
fn bash_sed_avec_un_joker_dans_le_script_execute_une_commande() {
    let d = tempfile::tempdir().unwrap();
    let arbre = d
        .path()
        .join("1,")
        .join("x\\")
        .join(",")
        .join("e touch execute #");
    std::fs::create_dir_all(&arbre).unwrap();
    std::fs::write(arbre.join("p"), "").unwrap();
    std::fs::write(d.path().join("f"), "ligne\n").unwrap();
    let ligne = "sed -n 1,/x*/,/e*/p f";
    let sortie = Command::new("bash")
        .args(["-c", ligne])
        .current_dir(d.path())
        .output()
        .unwrap();
    assert!(d.path().join("execute").exists(), "{sortie:?}");
    assert_ne!(classe(ligne, d.path()), Classe::Lecture);
}

/// Sans l'arbre aussi : la règle ne lit pas le disque. Cité, le joker reste un motif.
#[test]
fn la_regle_ne_depend_pas_du_disque() {
    let d = tempfile::tempdir().unwrap();
    assert_ne!(classe("sed -n 1,/x*/,/e*/p f", d.path()), Classe::Lecture);
    assert_eq!(classe("sed -n '1,/x*/p' f", d.path()), Classe::Lecture);
}
