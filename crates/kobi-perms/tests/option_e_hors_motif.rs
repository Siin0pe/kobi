//! KOB-541 : `-e`, `--regexp` et `-pattern` ne prennent un motif que pour
//! grep, rg et Select-String. Pour `cat`, `od`, `less`, `xxd` ou `ls`, le mot
//! suivant est un fichier lu : `cat -e ../dehors/secret.txt` l'affiche.

use std::path::{Path, PathBuf};

use kobi_perms::{commande, Classe, Contexte, Shell};

fn dossiers() -> (tempfile::TempDir, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let ici = d.path().join("ici");
    std::fs::create_dir_all(&ici).unwrap();
    std::fs::write(ici.join("notes.txt"), "x-contenu-ici\n").unwrap();
    std::fs::create_dir_all(d.path().join("dehors")).unwrap();
    std::fs::write(
        d.path().join("dehors").join("secret.txt"),
        "x-contenu-secret\n",
    )
    .unwrap();
    (d, ici)
}

fn classe(ligne: &str, shell: Shell, ici: &Path) -> Classe {
    let mut ctx = Contexte::new(vec![ici.to_path_buf()]);
    ctx.cwd = Some(ici.to_path_buf());
    commande::analyser(ligne, shell, &ctx).classe
}

/// Le vrai `cat -e` (GNU comme BSD) affiche le fichier qui suit.
#[cfg(unix)]
#[test]
fn le_vrai_cat_moins_e_lit_le_fichier() {
    let (_d, ici) = dossiers();
    let o = std::process::Command::new("cat")
        .args(["-e", "../dehors/secret.txt"])
        .current_dir(&ici)
        .output()
        .unwrap();
    let texte = String::from_utf8_lossy(&o.stdout);
    assert!(texte.contains("contenu-secret"), "{texte:?}");
}

#[test]
fn moins_e_ne_cache_pas_le_chemin_hors_des_motifs() {
    let (_d, ici) = dossiers();
    // Témoin : sans l'option, le chemin du dehors est refusé.
    assert!(matches!(
        classe("cat ../dehors/secret.txt", Shell::Bash, &ici),
        Classe::Refus(_)
    ));
    for ligne in [
        "cat -e ../dehors/secret.txt",
        "od -e ../dehors/secret.txt",
        "less -e ../dehors/secret.txt",
        "xxd -e ../dehors/secret.txt",
        "head -e ../dehors/secret.txt",
        "ls -e ../dehors",
        "ls -pattern ../dehors",
        "cat --regexp ../dehors/secret.txt",
        "cat -pattern ../dehors/secret.txt",
    ] {
        assert_ne!(classe(ligne, Shell::Bash, &ici), Classe::Lecture, "{ligne}");
    }
    // Hors de Windows, `cat` sous PowerShell est le programme natif.
    if cfg!(unix) {
        let ligne = "cat -e ../dehors/secret.txt";
        assert_ne!(
            classe(ligne, Shell::PowerShell, &ici),
            Classe::Lecture,
            "{ligne}"
        );
    }
}

/// Dans le dossier, rien ne change ; grep, rg et Select-String gardent leur
/// option de motif.
#[test]
fn les_vrais_motifs_restent_des_motifs() {
    let (_d, ici) = dossiers();
    for (ligne, shell) in [
        ("cat -e notes.txt", Shell::Bash),
        ("grep -e x notes.txt", Shell::Bash),
        ("grep --regexp x notes.txt", Shell::Bash),
        ("grep -e ../dehors notes.txt", Shell::Bash),
        ("egrep -e x notes.txt", Shell::Bash),
        ("rg -e x notes.txt", Shell::Bash),
        ("rg -e ../dehors notes.txt", Shell::Bash),
        ("Select-String -Pattern x notes.txt", Shell::PowerShell),
        (
            "Select-String -Pattern ../dehors notes.txt",
            Shell::PowerShell,
        ),
        ("jq -e . notes.txt", Shell::Bash),
    ] {
        assert_eq!(classe(ligne, shell, &ici), Classe::Lecture, "{ligne}");
    }
    for (ligne, shell) in [
        ("grep -e x ../dehors/secret.txt", Shell::Bash),
        ("rg -e x ../dehors/secret.txt", Shell::Bash),
        (
            "Select-String -Pattern x ../dehors/secret.txt",
            Shell::PowerShell,
        ),
    ] {
        assert!(
            matches!(classe(ligne, shell, &ici), Classe::Refus(_)),
            "{ligne}"
        );
    }
}

/// Le vrai grep et le vrai sed : une valeur collée dans un groupe d'options
/// (`-vexyzzye` = `-v -e xyzzye`) ou l'interrupteur `-E` laissent le mot
/// suivant être le fichier lu (relecture de KOB-541).
#[cfg(unix)]
#[test]
fn le_vrai_grep_lit_le_fichier_apres_un_groupe_ou_moins_e_majuscule() {
    let (_d, ici) = dossiers();
    std::fs::write(ici.join("liste"), "zzz\n").unwrap();
    for (prog, args) in [
        ("grep", vec!["-vexyzzye", "../dehors/secret.txt"]),
        ("grep", vec!["-ee", "../dehors/secret.txt"]),
        ("grep", vec!["-vfliste", "../dehors/secret.txt"]),
        ("grep", vec!["x", "-E", "../dehors/secret.txt"]),
        ("sed", vec!["-n", "1p", "../dehors/secret.txt", "-E"]),
    ] {
        let o = std::process::Command::new(prog)
            .args(&args)
            .current_dir(&ici)
            .output()
            .unwrap();
        let texte = String::from_utf8_lossy(&o.stdout);
        assert!(
            texte.contains("contenu-secret"),
            "{prog} {args:?} : {texte:?}"
        );
    }
}

#[test]
fn groupe_avec_valeur_collee_et_moins_e_majuscule() {
    let (_d, ici) = dossiers();
    std::fs::write(ici.join("liste"), "zzz\n").unwrap();
    for ligne in [
        "grep -vexyzzye ../dehors/secret.txt",
        "grep -ee ../dehors/secret.txt",
        "rg -vexyzzye ../dehors/secret.txt",
        "egrep -vexyzzye ../dehors/secret.txt",
        "grep -vfliste ../dehors/secret.txt",
        "rg -vfliste ../dehors/secret.txt",
    ] {
        assert_ne!(classe(ligne, Shell::Bash, &ici), Classe::Lecture, "{ligne}");
    }
    for shell in [Shell::Bash, Shell::PowerShell] {
        for ligne in [
            "grep x -E ../dehors/secret.txt",
            "egrep x -E ../dehors/secret.txt",
            "sed -n 1p -E ../dehors/secret.txt",
        ] {
            assert_ne!(
                classe(ligne, shell, &ici),
                Classe::Lecture,
                "{ligne} {shell:?}"
            );
        }
    }
    // Les formes ordinaires restent des lectures dans le dossier.
    for ligne in [
        "grep -ve x notes.txt",
        "grep -E 'a|b' notes.txt",
        "grep -vexyz notes.txt",
        "rg -ie x notes.txt",
        "sed -E -n 1p notes.txt",
    ] {
        assert_eq!(classe(ligne, Shell::Bash, &ici), Classe::Lecture, "{ligne}");
    }
}

/// fd : `--search-path` et `--base-directory` donnent le dossier parcouru.
#[test]
fn fd_search_path_est_un_chemin() {
    let (_d, ici) = dossiers();
    for ligne in [
        "fd -e txt --search-path ../dehors",
        "fd --search-path ../dehors",
        "fd --base-directory ../dehors",
        "fd x --search-path ../dehors",
    ] {
        assert_ne!(classe(ligne, Shell::Bash, &ici), Classe::Lecture, "{ligne}");
    }
    assert_eq!(
        classe("fd -e txt --search-path .", Shell::Bash, &ici),
        Classe::Lecture
    );
}
