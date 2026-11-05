//! SONDE (PR fermée sans merge) : relecture de #419. Sous pwsh 6+, l'échappement
//! `` `u{…} `` entre guillemets doubles produit un caractère que `decouper` ne
//! voit pas. Une variable factice, jamais une clé. Échoue toujours, pour que le
//! compte rendu apparaisse dans la CI.
#![cfg(windows)]

use std::path::PathBuf;
use std::process::Command;

use kobi_perms::{commande, Contexte, Shell};

const VALEUR: &str = "valeurfactice";

#[test]
fn sonde_echappement_u() {
    let racine = tempfile::tempdir().unwrap();
    let code = racine.path().join("code");
    let dehors = racine.path().join("dehors");
    std::fs::create_dir_all(&code).unwrap();
    std::fs::create_dir_all(&dehors).unwrap();
    std::fs::write(dehors.join("secret.txt"), "contenu-dehors").unwrap();
    let mut ctx = Contexte::new(vec![PathBuf::from(&code)]);
    ctx.cwd = Some(code.clone());
    let lignes = [
        "jq -n \"`u{65}nv.KOBI_SONDE\"",
        "jq -n \"`u{24}ENV.KOBI_SONDE\"",
        "Get-Content \"`u{2E}`u{2E}/dehors/secret.txt\"",
        "gc \"`u{2E}`u{2E}`u{5C}dehors`u{5C}secret.txt\"",
        "cat \"`u{2E}`u{2E}/dehors/secret.txt\"",
        // Autres échappements entre guillemets doubles.
        "Get-Content \"`0..`/dehors/secret.txt\"",
        "Get-Content \"..`/dehors/secret.txt\"",
        // Témoin.
        "Get-Content ../dehors/secret.txt",
    ];
    let mut cas = Vec::new();
    for l in lignes {
        let a = commande::analyser(l, Shell::PowerShell, &ctx);
        cas.push(format!("KOBI {l:?} => {} ({})", a.classe.cle(), a.raison));
        for exe in ["pwsh", "powershell"] {
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
