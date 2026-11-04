//! SONDE (PR fermée sans merge) : relecture de KOB-468. `@env:X` (splatting
//! d'une variable de lecteur) livre-t-il la valeur à `echo` ou `gc` ? Et
//! `cd ' -'` parcourt-il l'historique ? Une variable factice, jamais une clé.
//! Échoue toujours, pour que le compte rendu apparaisse dans la CI.
#![cfg(windows)]

use std::path::PathBuf;
use std::process::Command;

use kobi_perms::{commande, Contexte, Shell};

const VALEUR: &str = "valeur-factice-de-sonde";

#[test]
fn sonde_env_splatting() {
    let racine = tempfile::tempdir().unwrap();
    let code = racine.path().join("code");
    let dehors = racine.path().join("dehors");
    std::fs::create_dir_all(&code).unwrap();
    std::fs::create_dir_all(&dehors).unwrap();
    std::fs::write(dehors.join("secret.txt"), "contenu-dehors").unwrap();
    let mut ctx = Contexte::new(vec![PathBuf::from(&code)]);
    ctx.cwd = Some(code.clone());
    let d = dehors.display().to_string();
    let c = code.display().to_string();
    let lignes = [
        "echo @env:KOBI_SONDE".to_owned(),
        "Write-Output @env:KOBI_SONDE".to_owned(),
        "gc @env:KOBI_SONDE".to_owned(),
        "ls @env:KOBI_SONDE".to_owned(),
        "echo @variable:HOME".to_owned(),
        // Historique des dossiers : seul `-` exact, ou aussi avec espaces ?
        format!("Set-Location '{d}'; Set-Location '{c}'; cd ' -'; gc secret.txt"),
        format!("Set-Location '{d}'; Set-Location '{c}'; cd '- '; gc secret.txt"),
        format!("Set-Location '{d}'; Set-Location '{c}'; cd '-'; gc secret.txt"),
    ];
    let mut cas = Vec::new();
    for l in &lignes {
        let a = commande::analyser(l, Shell::PowerShell, &ctx);
        cas.push(format!("KOBI {l} => {} ({})", a.classe.cle(), a.raison));
        for exe in ["powershell", "pwsh"] {
            let o = Command::new(exe)
                .args(["-NoProfile", "-NonInteractive", "-Command", l])
                .current_dir(&code)
                .env("KOBI_SONDE", VALEUR)
                .output();
            let r = match o {
                Ok(o) => {
                    let tout = format!(
                        "{}{}",
                        String::from_utf8_lossy(&o.stdout),
                        String::from_utf8_lossy(&o.stderr)
                    );
                    format!(
                        "valeur vue : {} | dehors lu : {} | {:?}",
                        tout.contains(VALEUR),
                        tout.contains("contenu-dehors"),
                        tout.lines().next().unwrap_or("").trim()
                    )
                }
                Err(e) => format!("lancement impossible : {e}"),
            };
            cas.push(format!("  {exe} : {r}"));
        }
    }
    panic!("COMPTE RENDU\n{}", cas.join("\n"));
}
