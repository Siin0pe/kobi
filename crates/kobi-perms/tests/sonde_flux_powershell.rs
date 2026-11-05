//! SONDE (PR fermée sans merge) : relecture de KOB-480. Que fait `7>` sous le
//! vrai PowerShell ? Échoue toujours, pour que le compte rendu apparaisse dans
//! la CI. Tout se passe dans un dossier temporaire.
#![cfg(windows)]

use std::process::Command;

#[test]
fn sonde_flux_powershell() {
    let mut cas = Vec::new();
    for exe in ["powershell", "pwsh"] {
        for ligne in [
            "Write-Output 1> null",
            "Write-Output a 1> null",
            "Write-Output 7 > null",
            "Write-Output 7> null",
            "Write-Output a 7> null",
            "Write-Output 0> null",
            "Write-Output 01> null",
            "Write-Output 9> null",
            "Write-Output a 7>$null",
            "Write-Output a 7> $null",
        ] {
            let d = tempfile::tempdir().unwrap();
            let commande = format!("{ligne}; exit 42");
            let o = Command::new(exe)
                .args(["-NoProfile", "-NonInteractive", "-Command", &commande])
                .current_dir(d.path())
                .output()
                .unwrap();
            let mut fichiers = Vec::new();
            for e in std::fs::read_dir(d.path()).unwrap() {
                let e = e.unwrap();
                let contenu = std::fs::read(e.path()).unwrap_or_default();
                fichiers.push(format!(
                    "{:?}={:?}",
                    e.file_name(),
                    String::from_utf8_lossy(&contenu)
                ));
            }
            cas.push(format!(
                "{exe} {ligne:?} : code {:?} | fichiers {fichiers:?} | sortie {:?} | erreur {:?}",
                o.status.code(),
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
                    .chars()
                    .take(160)
                    .collect::<String>()
            ));
        }
    }
    panic!("COMPTE RENDU\n{}", cas.join("\n"));
}
