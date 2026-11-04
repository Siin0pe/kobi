//! SONDE (PR fermée sans merge) : limite de KOB-455. Une commande peut retirer
//! l'alias `where` (`Remove-Item alias:where`) : `where` lance alors where.exe.
//! Échoue toujours, pour que le compte rendu apparaisse dans la CI.
#![cfg(windows)]

use std::path::PathBuf;
use std::process::Command;

use kobi_perms::{commande, Contexte, Shell};

#[test]
fn sonde_alias_where() {
    let racine = tempfile::tempdir().unwrap();
    let code = racine.path().join("code");
    let dehors = racine.path().join("dehors");
    std::fs::create_dir_all(code.join("src")).unwrap();
    std::fs::create_dir_all(&dehors).unwrap();
    std::fs::write(dehors.join("secret.txt"), "x").unwrap();
    let mut ctx = Contexte::new(vec![PathBuf::from(&code)]);
    ctx.cwd = Some(code.clone());
    let d = dehors.display().to_string();
    let lignes = [
        format!("Remove-Item alias:where; where {d}:*"),
        format!("Remove-Item alias:where -Force; where {d}:*"),
        format!("del alias:where; where {d}:*"),
        format!("del alias:where -Force; where {d}:*"),
        format!("ri alias:\\where -Force; where {d}:*"),
        format!("Remove-Alias where -Force; where {d}:*"),
        format!("Set-Alias where where.exe -Option AllScope -Force; where {d}:*"),
        format!("Set-Item alias:where where.exe -Force; where {d}:*"),
        format!("New-Alias w where.exe; w {d}:*"),
        // témoin : l'alias en place.
        format!("where {d}:*"),
    ];
    let mut cas = Vec::new();
    // Deux commandes séparées, la première acceptée sur une carte.
    for l in [
        "Remove-Item alias:where -Force",
        "Set-Alias where where.exe -Force",
    ] {
        let a = commande::analyser(l, Shell::PowerShell, &ctx);
        cas.push(format!("SEULE {l} => {} ({})", a.classe.cle(), a.raison));
    }
    let a = commande::analyser(&format!("where {d}:*"), Shell::PowerShell, &ctx);
    cas.push(format!(
        "SEULE where <dehors>:* => {} ({})",
        a.classe.cle(),
        a.raison
    ));
    for l in &lignes {
        let a = commande::analyser(l, Shell::PowerShell, &ctx);
        cas.push(format!("KOBI {l} => {} ({})", a.classe.cle(), a.raison));
        for exe in ["powershell", "pwsh"] {
            let o = Command::new(exe)
                .args(["-NoProfile", "-NonInteractive", "-Command", l])
                .current_dir(&code)
                .output();
            let r = match o {
                Ok(o) => {
                    let sortie = String::from_utf8_lossy(&o.stdout);
                    format!(
                        "code {:?} | secret listé : {} | sortie {:?} | erreur {:?}",
                        o.status.code(),
                        sortie.contains("secret.txt"),
                        sortie.trim(),
                        String::from_utf8_lossy(&o.stderr)
                            .lines()
                            .next()
                            .unwrap_or("")
                    )
                }
                Err(e) => format!("lancement impossible : {e}"),
            };
            cas.push(format!("  {exe} : {r}"));
        }
    }
    panic!("COMPTE RENDU\n{}", cas.join("\n"));
}
