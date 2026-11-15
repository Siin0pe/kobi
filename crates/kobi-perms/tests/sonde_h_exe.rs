//! SONDE (h) (ne pas merger) : sous PowerShell Windows, `Get-Content.exe a.txt`
//! lance-t-il un exécutable du PATH au lieu du cmdlet ? Kobi le classe en lecture
//! (le `.exe` est retiré, et le nom tombe dans LECTURE_PS). Échoue toujours,
//! pour que le compte rendu apparaisse dans la CI.
#![cfg(windows)]

use std::process::Command;

#[test]
fn sonde_h_exe_plante_dans_le_path() {
    let d = tempfile::tempdir().unwrap();
    let bin = d.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let systeme = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    let whoami = std::path::Path::new(&systeme).join("System32").join("whoami.exe");
    let mut compte = Vec::new();
    for nom in ["Get-Content.exe", "gci.exe", "Write-Output.exe"] {
        std::fs::copy(&whoami, bin.join(nom)).unwrap();
    }
    std::fs::write(d.path().join("a.txt"), "CONTENU-DU-FICHIER\n").unwrap();
    let chemin = format!("{};{}", bin.display(), std::env::var("PATH").unwrap_or_default());
    for exe in ["powershell", "pwsh"] {
        for ligne in ["Get-Content.exe a.txt", "gci.exe", "Write-Output.exe x", "Get-Content a.txt"] {
            let o = Command::new(exe)
                .args(["-NoProfile", "-NonInteractive", "-Command", ligne])
                .current_dir(d.path())
                .env("PATH", &chemin)
                .output();
            let r = match o {
                Ok(o) => {
                    let t = format!(
                        "{}{}",
                        String::from_utf8_lossy(&o.stdout),
                        String::from_utf8_lossy(&o.stderr)
                    );
                    format!(
                        "fichier lu : {} | {:?}",
                        t.contains("CONTENU-DU-FICHIER"),
                        t.lines().next().unwrap_or("")
                    )
                }
                Err(e) => format!("lancement impossible : {e}"),
            };
            compte.push(format!("{exe} {ligne:?} => {r}"));
        }
    }
    panic!("COMPTE RENDU (h)\n{}", compte.join("\n"));
}
