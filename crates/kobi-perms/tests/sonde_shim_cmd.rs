//! SONDE (PR fermée sans merge) : relecture de KOB-471. Un script `.cmd`
//! appelé par son nom (shim `yarn.cmd`, `npm.cmd` sans `.ps1`) passe par
//! cmd.exe : `%VAR%` y est-il remplacé par sa valeur, sans `--%` ? Une
//! variable factice, jamais une clé. Échoue toujours, pour que le compte
//! rendu apparaisse dans la CI.
#![cfg(windows)]

use std::path::PathBuf;
use std::process::Command;

use kobi_perms::{commande, Contexte, Shell};

const VALEUR: &str = "valeurfactice";

#[test]
fn sonde_shim_cmd() {
    let racine = tempfile::tempdir().unwrap();
    let code = racine.path().join("code");
    let bin = racine.path().join("bin");
    std::fs::create_dir_all(&code).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    // Shims qui affichent leurs arguments, comme un vrai shim les passerait à
    // node : seulement `.cmd`, sans `.ps1` à côté.
    for nom in ["yarn", "npm", "pnpm"] {
        std::fs::write(bin.join(format!("{nom}.cmd")), "@echo ARGS %*\r\n").unwrap();
    }
    let chemin = format!("{};{}", bin.display(), std::env::var("PATH").unwrap());
    let mut ctx = Contexte::new(vec![PathBuf::from(&code)]);
    ctx.cwd = Some(code.clone());
    let lignes = [
        "yarn info %KOBI_SONDE%",
        "yarn info '%KOBI_SONDE%'",
        "npm view %KOBI_SONDE%",
        "npm ls %KOBI_SONDE%",
        "pnpm view %KOBI_SONDE%",
        "yarn why %KOBI_SONDE%",
    ];
    let mut cas = Vec::new();
    for l in lignes {
        for (shell, nom) in [(Shell::PowerShell, "PS"), (Shell::Bash, "BASH")] {
            let a = commande::analyser(l, shell, &ctx);
            cas.push(format!("{nom} {l:?} => {} ({})", a.classe.cle(), a.raison));
        }
        for exe in ["powershell", "pwsh"] {
            let o = Command::new(exe)
                .args(["-NoProfile", "-NonInteractive", "-Command", l])
                .current_dir(&code)
                .env("KOBI_SONDE", VALEUR)
                .env("PATH", &chemin)
                .output();
            let r = match o {
                Ok(o) => {
                    let tout = format!(
                        "{}{}",
                        String::from_utf8_lossy(&o.stdout),
                        String::from_utf8_lossy(&o.stderr)
                    );
                    format!(
                        "valeur vue : {} | {:?}",
                        tout.contains(VALEUR),
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
