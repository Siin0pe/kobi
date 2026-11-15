//! KOB-538 : un `ls` natif (pwsh hors Windows) ou un `X.exe` imprime des noms nus,
//! pas des objets à `PSPath`. Une cmdlet en aval (`Get-ChildItem`) les résout
//! ailleurs : `~` est le dossier personnel, un nom peut être un lien du dossier
//! courant. Ce n'est pas un amont sûr. Preuve : sonde #506 (run 43755944282),
//! tests de kobi-b repris de `revue_1511.rs` (`sonde_3_*`).

use std::path::{Path, PathBuf};

use kobi_perms::{commande, Classe, Contexte, Shell};

fn dossiers() -> (tempfile::TempDir, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let ici = d.path().join("ici");
    std::fs::create_dir_all(ici.join("sub")).unwrap();
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

/// Lance `pwsh -Command ligne` dans `dossier`, avec `home` pour HOME, et rend sa
/// sortie. Sans pwsh : `None` sous Linux (il n'est pas toujours installé), panique
/// sous macOS, où macos-latest l'a et doit prouver ce cas. Un pwsh qui plante à son
/// démarrage est relancé une fois ; s'il plante encore, le test échoue avec son
/// message (KOB-526).
#[cfg(unix)]
fn pwsh(ligne: &str, dossier: &Path, home: &Path) -> Option<String> {
    use std::process::Command;
    let lancer = || {
        Command::new("pwsh")
            .args(["-NoProfile", "-NonInteractive", "-Command", ligne])
            .current_dir(dossier)
            .env("HOME", home)
            .output()
    };
    let sortie = match lancer() {
        Ok(o) => o,
        Err(e) if cfg!(target_os = "macos") => panic!("pwsh absent sous macOS : {e}"),
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

/// Le vrai pwsh 7 (Linux, macOS) : `ls sub` imprime `~`, que `Get-ChildItem`
/// résout vers le dossier personnel, lu par `Get-Content`.
#[cfg(unix)]
#[test]
fn pwsh_hors_windows_ls_natif_mene_au_dossier_personnel() {
    let (d, ici) = dossiers();
    let maison = d.path().join("maison");
    std::fs::create_dir_all(&maison).unwrap();
    std::fs::write(maison.join("SECRET_MAISON"), "contenu-maison").unwrap();
    std::fs::write(ici.join("sub").join("~"), "").unwrap();
    let ligne = "ls sub | Get-ChildItem | Get-Content";
    let Some(t) = pwsh(ligne, &ici, &maison) else {
        return;
    };
    assert!(t.contains("contenu-maison"), "{t:?}");
    assert_ne!(classe(ligne, Shell::PowerShell, &ici), Classe::Lecture);
}

/// Relecture : `Get-ChildItem -Name` imprime `~`, résolu vers le dossier personnel.
#[cfg(unix)]
#[test]
fn pwsh_hors_windows_gci_name_mene_au_dossier_personnel() {
    let (d, ici) = dossiers();
    let maison = d.path().join("maison");
    std::fs::create_dir_all(&maison).unwrap();
    std::fs::write(maison.join("SECRET_MAISON"), "contenu-maison").unwrap();
    std::fs::write(ici.join("sub").join("~"), "").unwrap();
    let ligne = "Get-ChildItem sub -Name | Get-ChildItem | Get-Content";
    let Some(t) = pwsh(ligne, &ici, &maison) else {
        return;
    };
    assert!(t.contains("contenu-maison"), "{t:?}");
    assert_ne!(classe(ligne, Shell::PowerShell, &ici), Classe::Lecture);
}

/// Sous Windows, `ls.exe` et `dir.exe` (ceux de Git) impriment aussi des noms nus :
/// `lien`, fichier ordinaire dans `sub`, est résolu dans le dossier courant, où
/// `lien` mène dehors. Le classement de ces lignes était déjà rattrapé sur main
/// (l'arbre du dossier courant contient le lien) : c'est le test
/// `amont_natif_n_est_pas_un_amont_sur` qui prouve le défaut sous Windows.
#[cfg(windows)]
#[test]
fn powershell_ls_exe_donne_des_noms_resolus_ailleurs() {
    use std::process::Command;
    let chemin = format!(
        r"C:\Program Files\Git\usr\bin;{}",
        std::env::var("PATH").unwrap()
    );
    let (d, ici) = dossiers();
    std::fs::write(ici.join("sub").join("lien"), "").unwrap();
    std::os::windows::fs::symlink_dir(d.path().join("dehors"), ici.join("lien")).unwrap();
    for ligne in [
        "ls.exe sub | Get-ChildItem | Get-Content",
        "dir.exe sub | Get-ChildItem | Get-Content",
    ] {
        for exe in ["powershell", "pwsh"] {
            let o = Command::new(exe)
                .args(["-NoProfile", "-NonInteractive", "-Command", ligne])
                .current_dir(&ici)
                .env("PATH", &chemin)
                .output()
                .unwrap();
            let t = String::from_utf8_lossy(&o.stdout);
            assert!(t.contains("contenu-secret"), "{exe} {ligne} : {t:?}");
        }
        assert_ne!(
            classe(ligne, Shell::PowerShell, &ici),
            Classe::Lecture,
            "{ligne}"
        );
    }
}

#[test]
fn amont_natif_n_est_pas_un_amont_sur() {
    let (_d, ici) = dossiers();
    std::fs::write(ici.join("sub").join("~"), "").unwrap();
    let mut lignes = vec![
        "ls.exe sub | Get-ChildItem | Get-Content",
        "dir.exe sub | gci | gc",
        "ls.exe sub | Get-Item | Get-Content",
        "ls.exe sub | Resolve-Path",
        // Une cmdlet avec `-Name` imprime aussi des noms nus (relecture).
        "Get-ChildItem sub -Name | Get-ChildItem | Get-Content",
        "gci sub -n | gci | gc",
        "gci -Name:$true sub | gi | gc",
        "dir sub -Name | Get-ChildItem",
    ];
    // Hors de Windows, `ls` est le programme natif.
    if cfg!(unix) {
        lignes.extend([
            "ls sub | Get-ChildItem | Get-Content",
            "ls sub | gi | gc",
            "ls sub | Get-ChildItem",
        ]);
    }
    for ligne in lignes {
        assert_ne!(
            classe(ligne, Shell::PowerShell, &ici),
            Classe::Lecture,
            "{ligne}"
        );
    }
}

/// La cmdlet reste un amont sûr : ses objets portent leur chemin, déjà jugé.
#[test]
fn la_cmdlet_reste_un_amont_sur() {
    let (_d, ici) = dossiers();
    std::fs::write(ici.join("sub").join("a.txt"), "a").unwrap();
    for ligne in [
        "Get-ChildItem sub | Get-Content",
        "gci sub | gci | gc",
        "dir sub | Get-Content",
        "Get-Item sub | Get-ChildItem",
        // `gi` est un alias de Get-Item partout, pas un programme.
        "gi sub | gci | gc",
    ] {
        assert_eq!(
            classe(ligne, Shell::PowerShell, &ici),
            Classe::Lecture,
            "{ligne}"
        );
    }
    #[cfg(windows)]
    assert_eq!(
        classe("ls sub | Get-Content", Shell::PowerShell, &ici),
        Classe::Lecture
    );
}
