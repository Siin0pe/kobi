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

/// Relecture de KOB-504 : sous zsh avec `setopt extendedglob`, `^y` (tout sauf
/// `y`) est un joker. `sed -n 1,/^y/,/^y/p f` exécute la commande piégée, et
/// `grep -v ../dehors/^a f` lit le dossier extérieur. macOS a toujours zsh :
/// la CI y fait la preuve ; ailleurs, le test vérifie au moins le classement.
#[cfg(unix)]
#[test]
fn zsh_extendedglob_joker_chapeau() {
    let racine = tempfile::tempdir().unwrap();
    let code = racine.path().join("code");
    let arbre = code
        .join("1,")
        .join("x\\")
        .join(",")
        .join("e touch execute #");
    std::fs::create_dir_all(&arbre).unwrap();
    std::fs::write(arbre.join("p"), "").unwrap();
    std::fs::write(code.join("f"), "ligne\n").unwrap();
    let dehors = racine.path().join("dehors");
    std::fs::create_dir_all(&dehors).unwrap();
    std::fs::write(dehors.join("a"), "").unwrap();
    std::fs::write(dehors.join("b"), "").unwrap();
    std::fs::write(dehors.join("secret"), "LIGNE-SECRETE\n").unwrap();
    let sed = "sed -n 1,/^y/,/^y/p f";
    let grep = "grep -v ../dehors/^a f";
    let zsh = |ligne: &str| {
        std::process::Command::new("zsh")
            .args(["-f", "-c", &format!("setopt extendedglob; {ligne}")])
            .current_dir(&code)
            .output()
    };
    match zsh(grep) {
        Ok(sortie) => {
            let texte = String::from_utf8_lossy(&sortie.stdout);
            assert!(texte.contains("LIGNE-SECRETE"), "{sortie:?}");
            // La commande `e` est propre au sed de GNU (macOS a celui de BSD).
            if cfg!(target_os = "linux") {
                let sortie = zsh(sed).unwrap();
                assert!(code.join("execute").exists(), "{sortie:?}");
            }
        }
        Err(_) => {
            if cfg!(target_os = "macos") {
                panic!("zsh absent sous macOS");
            }
        }
    }
    assert_ne!(classe(sed, &code), Classe::Lecture);
    assert!(
        matches!(classe(grep, &code), Classe::Refus(_)),
        "{:?}",
        classe(grep, &code)
    );
}

/// Relectures de KOB-504 : sous zsh `extendedglob`, `s#` (zéro ou plusieurs
/// `s`) et `x^y` (x suivi de tout sauf y) désignent `s` et `x`, des liens qui
/// sortent ; `cd l#` entre dans `l`, et `< s#` lit `s`. `cat s` est refusé ;
/// aucune de ces lignes ne doit être une lecture.
#[cfg(unix)]
#[test]
fn zsh_extendedglob_suit_un_lien_qui_sort() {
    let racine = tempfile::tempdir().unwrap();
    let code = racine.path().join("code");
    let dehors = racine.path().join("dehors");
    std::fs::create_dir_all(&code).unwrap();
    std::fs::create_dir_all(&dehors).unwrap();
    std::fs::write(dehors.join("secret"), "LIGNE-SECRETE\n").unwrap();
    std::os::unix::fs::symlink(dehors.join("secret"), code.join("s")).unwrap();
    std::os::unix::fs::symlink(dehors.join("secret"), code.join("x")).unwrap();
    std::os::unix::fs::symlink(&dehors, code.join("l")).unwrap();
    let lignes = ["cat s#", "cat x^y", "cd l# && cat secret", "cat < s#"];
    for ligne in lignes {
        match std::process::Command::new("zsh")
            .args(["-f", "-c", &format!("setopt extendedglob; {ligne}")])
            .current_dir(&code)
            .output()
        {
            Ok(sortie) => {
                let texte = String::from_utf8_lossy(&sortie.stdout);
                assert!(texte.contains("LIGNE-SECRETE"), "{ligne} : {sortie:?}");
            }
            Err(_) => {
                if cfg!(target_os = "macos") {
                    panic!("zsh absent sous macOS");
                }
            }
        }
        assert_ne!(classe(ligne, &code), Classe::Lecture, "{ligne}");
    }
    assert!(matches!(classe("cat s", &code), Classe::Refus(_)));
}

/// KOB-524 : avec `setopt braceccl`, zsh fait de `uniq x{ab}` un `uniq xa xb`, qui
/// écrase `xb`. Jamais une lecture.
#[cfg(unix)]
#[test]
fn zsh_braceccl_uniq_ecrase_le_second() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("xa"), "l\n").unwrap();
    std::fs::write(d.path().join("xb"), "ANCIEN\n").unwrap();
    let ligne = "uniq x{ab}";
    match std::process::Command::new("zsh")
        .args(["-f", "-c", &format!("setopt braceccl; {ligne}")])
        .current_dir(d.path())
        .output()
    {
        Ok(sortie) => assert_eq!(
            std::fs::read_to_string(d.path().join("xb")).unwrap(),
            "l\n",
            "{sortie:?}"
        ),
        Err(_) => {
            if cfg!(target_os = "macos") {
                panic!("zsh absent sous macOS");
            }
        }
    }
    assert_ne!(classe(ligne, d.path()), Classe::Lecture);
}
