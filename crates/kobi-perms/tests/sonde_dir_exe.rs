//! Sonde temporaire (i), PR fermée sans merge : `dir.exe -LR sub` (MSYS) suit-il un
//! lien de `sub` vers un dossier extérieur ? Le rapport s'affiche par un échec voulu.
#![cfg(windows)]
use std::process::Command;

#[test]
fn sonde_dir_exe_suit_un_lien() {
    let chemin = format!(
        r"C:\Program Files\Git\usr\bin;{}",
        std::env::var("PATH").unwrap()
    );
    let mut r = String::new();
    let d = tempfile::tempdir().unwrap();
    let ici = d.path().join("ici");
    let dehors = d.path().join("dehors");
    std::fs::create_dir_all(ici.join("sub")).unwrap();
    std::fs::create_dir_all(&dehors).unwrap();
    std::fs::write(dehors.join("SECRET_DEHORS"), "x").unwrap();
    let lien = std::os::windows::fs::symlink_dir(&dehors, ici.join("sub").join("lien"));
    r.push_str(&format!("lien symbolique : {lien:?}\n"));
    let jonction = Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(ici.join("sub").join("jonction"))
        .arg(&dehors)
        .output();
    r.push_str(&format!(
        "jonction : {:?}\n",
        jonction.map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
    ));
    for exe in ["powershell", "pwsh"] {
        for ligne in [
            "dir.exe -LR sub",
            "dir.exe -R sub",
            "dir.exe sub/lien",
            "ls.exe -LR sub",
        ] {
            let o = Command::new(exe)
                .args(["-NoProfile", "-NonInteractive", "-Command", ligne])
                .current_dir(&ici)
                .env("PATH", &chemin)
                .output()
                .unwrap();
            let sortie = String::from_utf8_lossy(&o.stdout);
            let fuite = sortie.contains("SECRET_DEHORS");
            r.push_str(&format!(
                "SONDE {exe} [{ligne}] fuite={fuite} : {:?} err={:?}\n",
                sortie,
                String::from_utf8_lossy(&o.stderr)
            ));
        }
    }
    panic!("RAPPORT DE SONDE (échec voulu)\n{r}");
}
