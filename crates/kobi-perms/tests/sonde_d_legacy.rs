//! SONDE (d) (ne pas merger) : sous pwsh en mode Legacy
//! (`$PSNativeCommandArgumentPassing='Legacy'`, défaut sous Unix jusqu'à 7.2),
//! `jq '"-n"' e*` ou `jq -n '' e*` arrivent-ils à jq comme `-n env` ? Comparé au
//! mode Standard. Échoue toujours, pour que le compte rendu apparaisse dans la CI.
#![cfg(target_os = "linux")]

use std::path::PathBuf;
use std::process::Command;

use kobi_perms::{commande, Contexte, Shell};

#[test]
fn sonde_d_pwsh_legacy() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("env"), "").unwrap();
    let mut ctx = Contexte::new(vec![PathBuf::from(d.path())]);
    ctx.cwd = Some(PathBuf::from(d.path()));
    let mut compte = Vec::new();
    for ligne in [r#"jq '"-n"' e*"#, "jq -n '' e*"] {
        let kobi = commande::analyser(ligne, Shell::PowerShell, &ctx);
        compte.push(format!(
            "KOBI {ligne:?} => {:?} ({})",
            kobi.classe, kobi.raison
        ));
        for mode in ["Legacy", "Standard"] {
            let script = format!("$PSNativeCommandArgumentPassing='{mode}'; {ligne}");
            let r = match Command::new("pwsh")
                .args(["-NoProfile", "-NonInteractive", "-Command", &script])
                .current_dir(d.path())
                .env("KOBI_FAUX", "valeur-factice")
                .output()
            {
                Ok(o) => {
                    let t = format!(
                        "{}{}",
                        String::from_utf8_lossy(&o.stdout),
                        String::from_utf8_lossy(&o.stderr)
                    );
                    format!(
                        "environnement affiché : {} | {:?}",
                        t.contains("valeur-factice"),
                        t.lines().next().unwrap_or("")
                    )
                }
                Err(e) => format!("pwsh absent : {e}"),
            };
            compte.push(format!("  pwsh {mode} : {r}"));
        }
    }
    let version = Command::new("pwsh")
        .args([
            "-NoProfile",
            "-Command",
            "$PSVersionTable.PSVersion.ToString()",
        ])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default();
    panic!("COMPTE RENDU (d) pwsh {version}\n{}", compte.join("\n"));
}
