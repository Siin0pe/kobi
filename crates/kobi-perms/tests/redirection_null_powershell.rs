//! KOB-480 : sous le vrai PowerShell, `> $null` n'écrit rien, alors que `> null`
//! et `> '$null'` écrivent un fichier. C'est ce qui permet à `$null` non cité
//! de rester une lecture.
#![cfg(windows)]

use std::path::Path;
use std::process::{Command, Output};

const SHELLS: [&str; 2] = ["powershell", "pwsh"];

fn lancer(exe: &str, ligne: &str, dossier: &Path) -> Output {
    Command::new(exe)
        .args(["-NoProfile", "-NonInteractive", "-Command", ligne])
        .current_dir(dossier)
        .output()
        .unwrap_or_else(|e| panic!("{exe} : lancement impossible : {e}"))
}

/// Fichiers du dossier, hors `a.txt`, créé par le test.
fn crees(dossier: &Path) -> Vec<String> {
    let mut noms: Vec<String> = std::fs::read_dir(dossier)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n != "a.txt")
        .collect();
    noms.sort();
    noms
}

fn dossier() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.txt"), "contenu\n").unwrap();
    d
}

/// `exit 42` en fin de ligne : la ligne a été analysée et lancée. Sans lui, une
/// erreur d'analyse passerait pour « rien d'écrit ».
fn jouer(exe: &str, ligne: &str, dossier: &Path) -> Output {
    let sortie = lancer(exe, &format!("{ligne}; exit 42"), dossier);
    assert_eq!(
        sortie.status.code(),
        Some(42),
        "{exe} {ligne:?} : {}",
        String::from_utf8_lossy(&sortie.stderr)
    );
    sortie
}

#[test]
fn dollar_null_n_ecrit_rien() {
    let mut ecarts = Vec::new();
    for exe in SHELLS {
        // (ligne, la sortie standard est-elle jetée ?)
        for (ligne, jetee) in [
            ("Get-Content a.txt > $null", true),
            ("Get-Content a.txt >$null", true),
            ("Get-Content a.txt >> $null", true),
            ("Get-Content a.txt 1> $null", true),
            ("Get-Content a.txt *> $null", true),
            ("Get-Content a.txt > $NULL", true),
            ("Get-Content a.txt > $null; Get-Content a.txt > $null", true),
            ("Get-Content absent.txt 2>$null", true),
            ("Get-Content absent.txt 2> $null", true),
            // Flux d'avertissement : la sortie standard reste affichée.
            ("Get-Content a.txt 3> $null", false),
        ] {
            let d = dossier();
            let sortie = jouer(exe, ligne, d.path());
            let noms = crees(d.path());
            if !noms.is_empty() {
                ecarts.push(format!("{exe} {ligne:?} : créés {noms:?}"));
            }
            if jetee != sortie.stdout.is_empty() {
                ecarts.push(format!(
                    "{exe} {ligne:?} : sortie {:?}",
                    String::from_utf8_lossy(&sortie.stdout)
                ));
            }
            if std::fs::read_to_string(d.path().join("a.txt")).unwrap() != "contenu\n" {
                ecarts.push(format!("{exe} {ligne:?} : a.txt modifié"));
            }
        }
    }
    assert!(ecarts.is_empty(), "{}", ecarts.join("\n"));
}

/// Témoins : ces formes, elles, écrivent : le test saurait voir une écriture.
#[test]
fn les_faux_null_ecrivent_un_fichier() {
    let mut ecarts = Vec::new();
    for exe in SHELLS {
        for (ligne, attendu) in [
            ("Get-Content a.txt > null", "null"),
            ("Get-Content a.txt > '$null'", "$null"),
            ("Get-Content a.txt > \"null\"", "null"),
        ] {
            let d = dossier();
            jouer(exe, ligne, d.path());
            let noms = crees(d.path());
            if noms != [attendu] {
                ecarts.push(format!(
                    "{exe} {ligne:?} : créés {noms:?}, attendu {attendu}"
                ));
            }
        }
    }
    assert!(ecarts.is_empty(), "{}", ecarts.join("\n"));
}

/// `7>` n'est pas une redirection : c'est un mot, et `null` reste un argument.
/// `Write-Output` les affiche, et aucun fichier n'est créé.
#[test]
fn flux_hors_de_1_a_6_est_un_mot() {
    let mut ecarts = Vec::new();
    for exe in SHELLS {
        for numero in ["7", "0", "01", "9"] {
            let d = dossier();
            let ligne = format!("Write-Output a {numero}> null");
            let sortie = jouer(exe, &ligne, d.path());
            let texte = String::from_utf8_lossy(&sortie.stdout).replace("\r\n", "\n");
            let attendu = format!("a\n{numero}>\nnull\n");
            let noms = crees(d.path());
            if texte != attendu || !noms.is_empty() {
                ecarts.push(format!(
                    "{exe} {ligne:?} : sortie {texte:?}, créés {noms:?}"
                ));
            }
        }
    }
    assert!(ecarts.is_empty(), "{}", ecarts.join("\n"));
}
