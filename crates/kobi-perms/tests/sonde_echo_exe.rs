//! Sonde temporaire (k), PR fermée sans merge : sous PowerShell Windows, `echo.exe
//! ../dehors/*` (MSYS) liste-t-il des noms hors du dossier, et comment Kobi le
//! classe-t-il sous Windows ? Rapport par échec voulu.
#![cfg(windows)]
use std::path::PathBuf;
use std::process::Command;

use kobi_perms::{commande, Contexte, Shell};

#[test]
fn sonde_echo_exe_motif_hors_du_dossier() {
    let chemin = format!(
        r"C:\Program Files\Git\usr\bin;{}",
        std::env::var("PATH").unwrap()
    );
    let mut r = String::new();
    let d = tempfile::tempdir().unwrap();
    let ici = d.path().join("ici");
    let dehors = d.path().join("dehors");
    std::fs::create_dir_all(&ici).unwrap();
    std::fs::create_dir_all(&dehors).unwrap();
    std::fs::write(dehors.join("SECRET_NOM"), "x").unwrap();
    let mut ctx = Contexte::new(vec![PathBuf::from(&ici)]);
    ctx.cwd = Some(PathBuf::from(&ici));
    for ligne in [
        "echo.exe ../dehors/*",
        "echo.exe '../dehors/*'",
        "printf.exe '%s ' ../dehors/*",
        "echo ../dehors/*",
    ] {
        let a = commande::analyser(ligne, Shell::PowerShell, &ctx);
        r.push_str(&format!("CLASSE [{ligne}] {:?} : {}\n", a.classe, a.raison));
        for exe in ["powershell", "pwsh"] {
            let o = Command::new(exe)
                .args(["-NoProfile", "-NonInteractive", "-Command", ligne])
                .current_dir(&ici)
                .env("PATH", &chemin)
                .output()
                .unwrap();
            let sortie = String::from_utf8_lossy(&o.stdout);
            r.push_str(&format!(
                "SONDE {exe} [{ligne}] fuite={} : {:?}\n",
                sortie.contains("SECRET_NOM"),
                sortie
            ));
        }
    }
    panic!("RAPPORT DE SONDE (échec voulu)\n{r}");
}
