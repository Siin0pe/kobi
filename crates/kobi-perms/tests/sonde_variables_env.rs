//! SONDE (PR fermée sans merge) : demandée avec KOB-471. Autres façons de lire
//! une variable d'environnement sous PowerShell et Git Bash, et `%VAR%` vu par
//! un script .cmd (npm.cmd) appelé par son nom. Une variable factice, jamais
//! une clé. Échoue toujours, pour que le compte rendu apparaisse dans la CI.
#![cfg(windows)]

use std::path::PathBuf;
use std::process::Command;

use kobi_perms::{commande, Contexte, Shell};

const VALEUR: &str = "valeurfactice";

fn lancer(exe: &str, args: &[&str], code: &std::path::Path) -> String {
    match Command::new(exe)
        .args(args)
        .current_dir(code)
        .env("KOBI_SONDE", VALEUR)
        .output()
    {
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
    }
}

#[test]
fn sonde_variables_env() {
    let racine = tempfile::tempdir().unwrap();
    let code = racine.path().join("code");
    std::fs::create_dir_all(&code).unwrap();
    let mut ctx = Contexte::new(vec![PathBuf::from(&code)]);
    ctx.cwd = Some(code.clone());
    let ps = [
        "echo ${env:KOBI_SONDE}",
        "echo \"${env:KOBI_SONDE}\"",
        "[Environment]::GetEnvironmentVariable('KOBI_SONDE')",
        "[System.Environment]::GetEnvironmentVariables()",
        "gci env:",
        "ls env:KOBI_SONDE",
        "Get-Item env:KOBI_SONDE",
        "dir env:",
        "npm ls %KOBI_SONDE%",
        "npm view %KOBI_SONDE%",
        "npm help %KOBI_SONDE%",
        "npx --version %KOBI_SONDE%",
        "where %KOBI_SONDE%",
        "git log %KOBI_SONDE%",
        "echo %KOBI_SONDE%",
    ];
    let bash = [
        "printenv KOBI_SONDE",
        "printenv",
        "env",
        "set",
        "export -p",
        "declare -p KOBI_SONDE",
        "echo %KOBI_SONDE%",
        "npm ls %KOBI_SONDE%",
        "cat /proc/self/environ",
    ];
    let mut cas = Vec::new();
    for l in ps {
        let a = commande::analyser(l, Shell::PowerShell, &ctx);
        cas.push(format!("PS {l:?} => {} ({})", a.classe.cle(), a.raison));
        for exe in ["powershell", "pwsh"] {
            let r = lancer(
                exe,
                &["-NoProfile", "-NonInteractive", "-Command", l],
                &code,
            );
            cas.push(format!("  {exe} : {r}"));
        }
    }
    let git_bash = r"C:\Program Files\Git\bin\bash.exe";
    for l in bash {
        let a = commande::analyser(l, Shell::Bash, &ctx);
        cas.push(format!("BASH {l:?} => {} ({})", a.classe.cle(), a.raison));
        let r = lancer(git_bash, &["-c", l], &code);
        cas.push(format!("  git-bash : {r}"));
    }
    panic!("COMPTE RENDU\n{}", cas.join("\n"));
}
