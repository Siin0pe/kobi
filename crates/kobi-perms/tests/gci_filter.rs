//! KOB-537 : sous pwsh 7, la partie dossier de `-Filter` est suivie :
//! `Get-ChildItem -Filter ../dehors/* | Get-Content` lit hors du dossier. Preuve :
//! sonde #506 (run 43755944282) sur les 3 OS, tests de kobi-b repris de
//! `revue_1511.rs` (`sonde_2_*`).

use std::path::{Path, PathBuf};
use std::process::Command;

use kobi_perms::{commande, Classe, Contexte, Shell};

fn dossiers() -> (tempfile::TempDir, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let ici = d.path().join("ici");
    std::fs::create_dir_all(ici.join("sub")).unwrap();
    std::fs::write(ici.join("sub").join("a.txt"), "a").unwrap();
    std::fs::create_dir_all(d.path().join("dehors")).unwrap();
    std::fs::write(
        d.path().join("dehors").join("secret.txt"),
        "x-contenu-secret\n",
    )
    .unwrap();
    (d, ici)
}

fn classe(ligne: &str, ici: &Path) -> Classe {
    classe_depuis(ligne, ici, ici)
}

/// Classement avec `cwd` pour dossier courant, `ici` restant le dossier déclaré.
fn classe_depuis(ligne: &str, ici: &Path, cwd: &Path) -> Classe {
    let mut ctx = Contexte::new(vec![ici.to_path_buf()]);
    ctx.cwd = Some(cwd.to_path_buf());
    commande::analyser(ligne, Shell::PowerShell, &ctx).classe
}

/// Sortie du vrai pwsh. Sans pwsh : `None` sous Linux (il n'y est pas toujours
/// installé), panique sous Windows et macOS, où les runners l'ont et doivent prouver
/// ce cas. Un pwsh qui plante à son démarrage est relancé une fois ; s'il plante
/// encore, le test échoue avec son message (KOB-526).
fn pwsh(ligne: &str, dossier: &Path) -> Option<String> {
    let lancer = || {
        Command::new("pwsh")
            .args(["-NoProfile", "-NonInteractive", "-Command", ligne])
            .current_dir(dossier)
            .output()
    };
    let sortie = match lancer() {
        Ok(o) => o,
        Err(e) if cfg!(any(windows, target_os = "macos")) => panic!("pwsh absent : {e}"),
        Err(_) => return None,
    };
    let sortie = if sortie.status.success() {
        sortie
    } else {
        let encore = lancer().unwrap();
        assert!(
            encore.status.success(),
            "pwsh a échoué deux fois sur {ligne:?} : {}",
            String::from_utf8_lossy(&encore.stderr)
        );
        encore
    };
    Some(String::from_utf8_lossy(&sortie.stdout).into_owned())
}

/// Le vrai pwsh 7 suit la partie dossier du filtre.
#[test]
fn pwsh_suit_le_dossier_du_filtre() {
    let (_d, ici) = dossiers();
    let mut lignes = vec![
        "Get-ChildItem -Filter ../dehors/* | Get-Content",
        "gci -Filter ../dehors/secret.txt | Get-Content",
        "gci -Filter:../dehors/* | gc",
        "gci -filt ../dehors/* | gc",
        "gci sub -Filter ../../dehors/* | gc",
    ];
    if cfg!(windows) {
        lignes.push(r"gci -Filter ..\dehors\* | gc");
    }
    for ligne in lignes {
        let Some(t) = pwsh(ligne, &ici) else {
            return;
        };
        assert!(t.contains("contenu-secret"), "{ligne} : {t:?}");
        assert_ne!(classe(ligne, &ici), Classe::Lecture, "{ligne}");
    }
}

#[test]
fn filtre_avec_dossier_sur_la_carte() {
    let (_d, ici) = dossiers();
    for ligne in [
        "Get-ChildItem -Filter ../dehors/* | Get-Content",
        "gci -Filter ../dehors/secret.txt | Get-Content",
        "gci -Filter ../dehors/*",
        "gci -Filter:../dehors/* | gc",
        "gci -filt ../dehors/* | gc",
        "gci -Filter '../dehors/*' | gc",
        "gci sub -Filter ../../dehors/* | gc",
        "dir -Filter ../dehors/*",
        "Get-ChildItem -Recurse -Filter ../dehors/*",
    ] {
        assert_ne!(classe(ligne, &ici), Classe::Lecture, "{ligne}");
    }
    // `\` n'est un séparateur que sous Windows.
    #[cfg(windows)]
    assert_ne!(
        classe(r"gci -Filter ..\dehors\* | gc", &ici),
        Classe::Lecture
    );
}

/// Relecture : le deuxième positionnel de Get-ChildItem est `-Filter` (le premier
/// si `-Path` est nommé), et `-Filter: x` lie le mot suivant. Depuis un
/// sous-dossier du dossier déclaré, pwsh résout le filtre depuis `..` : il sort.
#[test]
fn filtre_positionnel_depuis_un_sous_dossier() {
    let (_d, ici) = dossiers();
    let sub = ici.join("sub");
    let lignes = [
        "gci .. ../dehors/* | gc",
        "gci .. ../dehors/secret.txt | gc",
        "gci -Path .. ../dehors/secret.txt | gc",
        "gci .. ../dehors/* -Recurse | gc",
        "gci -File .. ../dehors/* | gc",
        "gci -d .. ../dehors/*",
        "gci .. -Filter: ../dehors/* | gc",
        "Get-ChildItem -LiteralPath .. ../dehors/* | gc",
        // Deuxième relecture : option à valeur inconnue, tableau, pipe.
        "gci -ea 0 .. ../dehors/* | gc",
        "gci -ErrorAction SilentlyContinue .. ../dehors/* | gc",
        "gci .,.. ../dehors/* | gc",
        "gci -Path .,.. ../dehors/* | gc",
        "gi .. | gci -Filter ../dehors/* | gc",
    ];
    for ligne in lignes {
        assert_ne!(classe_depuis(ligne, &ici, &sub), Classe::Lecture, "{ligne}");
    }
    // Le vrai pwsh lit bien le fichier du dehors (la forme `-Recurse` le lit aussi,
    // mais sort en erreur sur les dossiers qu'elle ne trouve pas).
    // Les tableaux (`.,..`) lisent aussi le secret, mais pwsh sort en erreur sur `.`.
    for ligne in [0, 1, 2, 8, 9, 12].map(|k| lignes[k]) {
        let Some(t) = pwsh(ligne, &sub) else {
            return;
        };
        assert!(t.contains("contenu-secret"), "{ligne} : {t:?}");
    }
}

/// Un chemin hors des dossiers reste un refus, filtre ou non.
#[test]
fn un_refus_reste_un_refus() {
    let (_d, ici) = dossiers();
    assert!(matches!(
        classe("gci ../dehors -Filter a/b", &ici),
        Classe::Refus(_)
    ));
}

/// Un filtre de noms reste une lecture.
#[test]
fn filtre_de_noms_reste_une_lecture() {
    let (_d, ici) = dossiers();
    for ligne in [
        "Get-ChildItem -Filter *.txt sub",
        "gci sub -Filter *.txt | Get-Content",
        "gci -Filter:*.txt sub",
        "Get-ChildItem -Recurse -Filter *.rs",
        "gci sub *.txt | gc",
        "gci -Path sub *.txt",
        "Get-ChildItem -Path sub/a.txt",
        "gci -Depth 1 sub *.txt",
    ] {
        assert_eq!(classe(ligne, &ici), Classe::Lecture, "{ligne}");
    }
}
