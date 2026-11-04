//! SONDE (PR fermée sans merge) : découpage des arguments par PowerShell,
//! face au découpeur de kobi-perms. Échoue toujours, pour que le compte
//! rendu apparaisse dans le journal de la CI.
#![cfg(windows)]

use std::path::Path;
use std::process::Command;

use kobi_perms::{commande, Contexte, Shell};

fn ps(cwd: &Path, script: &str) -> String {
    let out = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .current_dir(cwd)
        .output()
        .expect("powershell");
    format!(
        "sortie={:?} erreurs={:?}",
        String::from_utf8_lossy(&out.stdout).trim(),
        String::from_utf8_lossy(&out.stderr)
            .lines()
            .next()
            .unwrap_or("")
    )
}

#[test]
fn sonde_decoupage_powershell() {
    let racine = tempfile::tempdir().unwrap();
    let code = racine.path().join("code");
    let dehors = racine.path().join("dehors");
    std::fs::create_dir_all(&code).unwrap();
    std::fs::create_dir_all(&dehors).unwrap();
    std::fs::write(dehors.join("secret.txt"), "SECRET-KOB").unwrap();
    let mut ctx = Contexte::new(vec![code.clone()]);
    ctx.cwd = Some(code.clone());
    let verdict = |l: &str| commande::analyser(l, Shell::PowerShell, &ctx).classe.cle();
    let mut r = Vec::new();

    // 1. Splatting d'une variable scalaire vers un exécutable et une cmdlet.
    for l in [
        "cmd.exe /c echo @HOME",
        "where.exe /R @HOME secret.txt",
        "Get-Content @PROFILE",
    ] {
        r.push(format!("[1] {l} : kobi={} ; {}", verdict(l), ps(&code, l)));
    }
    r.push(format!("[1] $HOME vaut : {}", ps(&code, "$HOME")));

    // 2. Mot qui commence par un guillemet : coupé après le guillemet fermant ?
    for l in [
        "Write-Output 'x'/R",
        "Write-Output 'x'y",
        "Write-Output x'y'z",
    ] {
        r.push(format!("[2] {l} : kobi={} ; {}", verdict(l), ps(&code, l)));
    }

    // 3. Lecture hors du dossier par ce collage.
    for l in [
        r"Get-Content 'x'..\dehors\secret.txt",
        r"where.exe 'x'/R ..\dehors secret.txt",
    ] {
        r.push(format!("[3] {l} : kobi={} ; {}", verdict(l), ps(&code, l)));
    }

    panic!("COMPTE RENDU DE LA SONDE\n{}", r.join("\n"));
}
