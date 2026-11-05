//! SONDE (PR fermée sans merge) : revue croisée des merges de kobi-a (05/11).
//! (1) `> /dev/null` sous PowerShell Windows écrit-il `\dev\null` ?
//! (2) Un tiret Unicode (`–g`) est-il passé tel quel à un programme natif ?
//! Échoue toujours, pour que le compte rendu apparaisse dans la CI.
#![cfg(windows)]

use std::path::PathBuf;
use std::process::Command;

use kobi_perms::{commande, Contexte, Shell};

#[test]
fn sonde_revue_croisee() {
    let racine = tempfile::tempdir().unwrap();
    let code = racine.path().join("code");
    let dehors = racine.path().join("dehors");
    std::fs::create_dir_all(&code).unwrap();
    std::fs::create_dir_all(&dehors).unwrap();
    std::fs::write(code.join("a.txt"), "contenu-a\n").unwrap();
    std::fs::write(dehors.join("secret.txt"), "contenu-dehors\n").unwrap();
    std::fs::write(
        code.join("argv.py"),
        "import sys\nprint(repr(sys.argv[1:]))\n",
    )
    .unwrap();
    let mut ctx = Contexte::new(vec![PathBuf::from(&code)]);
    ctx.cwd = Some(code.clone());
    let mut cas = Vec::new();
    // (1) `> /dev/null` : le dossier racine `\dev` existe (fréquent), puis non.
    let lecteur = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    let dev = PathBuf::from(format!("{lecteur}\\dev"));
    for avec_dev in [true, false] {
        if avec_dev {
            std::fs::create_dir_all(&dev).ok();
        } else {
            std::fs::remove_dir_all(&dev).ok();
        }
        let l = "Get-Content a.txt > /dev/null";
        let a = commande::analyser(l, Shell::PowerShell, &ctx);
        cas.push(format!(
            "KOBI {l:?} (\\dev existe : {avec_dev}) => {} ({})",
            a.classe.cle(),
            a.raison
        ));
        for exe in ["powershell", "pwsh"] {
            std::fs::remove_file(dev.join("null")).ok();
            let o = Command::new(exe)
                .args(["-NoProfile", "-NonInteractive", "-Command", l])
                .current_dir(&code)
                .output();
            let erreur = o
                .map(|o| {
                    String::from_utf8_lossy(&o.stderr)
                        .lines()
                        .next()
                        .unwrap_or("")
                        .to_owned()
                })
                .unwrap_or_else(|e| e.to_string());
            cas.push(format!(
                "  {exe} : fichier \\dev\\null écrit : {} | {:?}",
                dev.join("null").is_file(),
                erreur
            ));
        }
    }
    std::fs::remove_dir_all(&dev).ok();
    // (2) Ce qu'un programme natif reçoit d'un tiret Unicode, et le classement.
    for l in [
        "python argv.py –s",
        "python argv.py —g x",
        "rg x –g ../dehors/secret.txt",
        "rg contenu –g ../dehors/secret.txt",
        "hostname –s",
    ] {
        let a = commande::analyser(l, Shell::PowerShell, &ctx);
        cas.push(format!("KOBI {l:?} => {} ({})", a.classe.cle(), a.raison));
        for exe in ["powershell", "pwsh"] {
            let o = Command::new(exe)
                .args(["-NoProfile", "-NonInteractive", "-Command", l])
                .current_dir(&code)
                .output();
            let r = match o {
                Ok(o) => {
                    let tout = format!(
                        "{}{}",
                        String::from_utf8_lossy(&o.stdout),
                        String::from_utf8_lossy(&o.stderr)
                    );
                    format!(
                        "dehors lu : {} | {:?}",
                        tout.contains("contenu-dehors"),
                        tout.lines().take(2).collect::<Vec<_>>().join(" / ")
                    )
                }
                Err(e) => format!("lancement impossible : {e}"),
            };
            cas.push(format!("  {exe} : {r}"));
        }
    }
    // Faux positif : `| Out-Null`.
    for l in [
        "Get-Content a.txt | Out-Null",
        "Get-Content a.txt 2>&1 | Out-Null",
    ] {
        let a = commande::analyser(l, Shell::PowerShell, &ctx);
        cas.push(format!("KOBI {l:?} => {} ({})", a.classe.cle(), a.raison));
    }
    panic!("COMPTE RENDU\n{}", cas.join("\n"));
}
