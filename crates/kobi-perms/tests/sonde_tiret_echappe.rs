//! SONDE (PR fermée sans merge) : relecture de KOB-477. Hors guillemets, un
//! tiret échappé (`` `-Pattern ``) est une valeur pour PowerShell, pas un
//! paramètre. Échoue toujours, pour que le compte rendu apparaisse dans la CI.
#![cfg(windows)]

use std::path::PathBuf;
use std::process::Command;

use kobi_perms::{commande, Contexte, Shell};

#[test]
fn sonde_tiret_echappe() {
    let racine = tempfile::tempdir().unwrap();
    let code = racine.path().join("code");
    let dehors = racine.path().join("dehors");
    std::fs::create_dir_all(&code).unwrap();
    std::fs::create_dir_all(&dehors).unwrap();
    std::fs::write(dehors.join("secret.txt"), "contenu-dehors\n").unwrap();
    let mut ctx = Contexte::new(vec![PathBuf::from(&code)]);
    ctx.cwd = Some(code.clone());
    let lignes = [
        "Select-String -NotMatch `-Pattern ../dehors/secret.txt",
        "Select-String -NotMatch `–Pattern ../dehors/secret.txt",
        "sls -NotMatch `-Pattern ..\\dehors\\secret.txt",
        "Select-String `-Context ../dehors/secret.txt",
        "Get-Content `-Encoding ../dehors/secret.txt",
        "Get-ChildItem `-Include ../dehors",
        "Get-Content a`tb",
        // KOB-475, points non prouvés : guillemet doublé entre guillemets
        // doubles (PowerShell lit `a"b`), et redirection vers `nul`.
        "Write-Output \"a\"\"b\"",
        "Get-Content \"src\"\"/../../dehors/secret.txt\"",
        "Get-Content \"..\"\"/dehors/secret.txt\"",
        "Write-Output kobi-nul >nul; Get-ChildItem -Name",
        "Write-Output kobi-nul > NUL; Get-ChildItem -Name",
        // Témoins.
        "Select-String -NotMatch -Pattern x ../dehors/secret.txt",
        "Select-String -NotMatch '-Pattern' ../dehors/secret.txt",
    ];
    let mut cas = Vec::new();
    for l in lignes {
        let a = commande::analyser(l, Shell::PowerShell, &ctx);
        cas.push(format!("KOBI {l:?} => {} ({})", a.classe.cle(), a.raison));
        for exe in ["pwsh", "powershell"] {
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
                        tout.lines().take(3).collect::<Vec<_>>().join(" / ")
                    )
                }
                Err(e) => format!("lancement impossible : {e}"),
            };
            cas.push(format!("  {exe} : {r}"));
        }
    }
    panic!("COMPTE RENDU\n{}", cas.join("\n"));
}
