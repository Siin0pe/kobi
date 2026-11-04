//! SONDE (PR fermée sans merge) : relecture de KOB-470. `--%` (arrêt de
//! l'analyse) remplace `%VAR%` par la valeur pour un programme natif ; et une
//! continuation au milieu d'un mot coupe-t-elle le jeton (`a`+accent grave+LF
//! +`@env:X`) ? Une variable factice, jamais une clé.
//! Échoue toujours, pour que le compte rendu apparaisse dans la CI.
#![cfg(windows)]

use std::path::PathBuf;
use std::process::Command;

use kobi_perms::{commande, Contexte, Shell};

const VALEUR: &str = "valeurfactice";

#[test]
fn sonde_stop_parsing() {
    let racine = tempfile::tempdir().unwrap();
    let code = racine.path().join("code");
    std::fs::create_dir_all(&code).unwrap();
    let mut ctx = Contexte::new(vec![PathBuf::from(&code)]);
    ctx.cwd = Some(code.clone());
    let lignes = [
        "which --% %KOBI_SONDE%",
        "where.exe --% %KOBI_SONDE%",
        "git --% log %KOBI_SONDE%",
        "git log --% %KOBI_SONDE%",
        "rg --% %KOBI_SONDE% .",
        "findstr --% %KOBI_SONDE% x",
        "echo --% %KOBI_SONDE%",
        "echo a`\n@env:KOBI_SONDE",
        "echo `\n@env:KOBI_SONDE",
    ];
    let mut cas = Vec::new();
    for l in lignes {
        let a = commande::analyser(l, Shell::PowerShell, &ctx);
        cas.push(format!("KOBI {l:?} => {} ({})", a.classe.cle(), a.raison));
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
                    let serre: String = tout.split_whitespace().collect();
                    format!(
                        "valeur vue : {} | en morceaux : {} | {:?}",
                        tout.contains(VALEUR),
                        serre.contains(VALEUR),
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
