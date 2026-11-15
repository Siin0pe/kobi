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
