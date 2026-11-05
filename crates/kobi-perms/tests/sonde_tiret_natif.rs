//! SONDE (PR fermée sans merge) : revue croisée (suite de #455). Un tiret
//! Unicode devant une option d'un programme natif : `decouper` le ramène à
//! `-`, le programme reçoit autre chose. Avec le vrai grep de Git pour
//! Windows en tête du PATH. Échoue toujours, pour le compte rendu.
#![cfg(windows)]

use std::path::PathBuf;
use std::process::Command;

use kobi_perms::{commande, Contexte, Shell};

#[test]
fn sonde_tiret_natif() {
    let racine = tempfile::tempdir().unwrap();
    let code = racine.path().join("code");
    let dehors = racine.path().join("dehors");
    std::fs::create_dir_all(&code).unwrap();
    std::fs::create_dir_all(&dehors).unwrap();
    std::fs::write(dehors.join("secret.txt"), "contenu-dehors\n").unwrap();
    let mut ctx = Contexte::new(vec![PathBuf::from(&code)]);
    ctx.cwd = Some(code.clone());
    let git_usr = r"C:\Program Files\Git\usr\bin";
    let chemin = format!("{git_usr};{}", std::env::var("PATH").unwrap());
    let mut cas = vec![format!(
        "grep de Git présent : {}",
        PathBuf::from(git_usr).join("grep.exe").is_file()
    )];
    for l in [
        "grep contenu –e ../dehors/secret.txt",
        "grep contenu —e ../dehors/secret.txt",
        "grep contenu -e ../dehors/secret.txt",
        "grep contenu ../dehors/secret.txt",
    ] {
        let a = commande::analyser(l, Shell::PowerShell, &ctx);
        cas.push(format!("KOBI {l:?} => {} ({})", a.classe.cle(), a.raison));
        for exe in ["powershell", "pwsh"] {
            let o = Command::new(exe)
                .args(["-NoProfile", "-NonInteractive", "-Command", l])
                .current_dir(&code)
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
    panic!("COMPTE RENDU\n{}", cas.join("\n"));
}
