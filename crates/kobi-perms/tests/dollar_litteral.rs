//! KOB-515 : entre guillemets doubles, un `$` suivi de ces caractères reste littéral
//! pour bash, zsh et pwsh : Kobi n'y voit ni variable ni substitution. zsh développe
//! `$=X`, `$~X` et `$+X` : ces trois-là restent sur la carte.
#![cfg(unix)]
use std::process::Command;

const LITTERAUX: &[char] = &[
    '|', '/', ')', ' ', '.', ',', ';', ']', '%', '>', '<', '&', '\'',
];

fn sortie(programme: &str, args: &[&str]) -> Option<String> {
    let o = Command::new(programme)
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin")
        .env("X", "VARIABLE")
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&o.stdout).into_owned())
}

#[test]
fn bash_zsh_et_pwsh_laissent_le_dollar_litteral() {
    for &c in LITTERAUX {
        // `X` est défini : un développement se verrait.
        let mot = format!("a${c}X");
        let script = format!("printf '%s' \"{mot}\"");
        assert_eq!(
            sortie("bash", &["--norc", "-c", &script]).unwrap(),
            mot,
            "bash {c:?}"
        );
        // zsh : toujours présent sous macOS, parfois absent ailleurs.
        match sortie("zsh", &["-f", "-c", &script]) {
            Some(z) => assert_eq!(z, mot, "zsh {c:?}"),
            None if cfg!(target_os = "macos") => panic!("zsh absent sous macOS"),
            None => {}
        }
        let script = format!("[Console]::Out.Write(\"{mot}\")");
        // pwsh n'est pas installé partout : la CI le fournit.
        if let Some(p) = sortie(
            "pwsh",
            &["-NoProfile", "-NonInteractive", "-Command", &script],
        ) {
            assert_eq!(p, mot, "pwsh {c:?}");
        }
    }
    // Témoins : une vraie variable est remplacée, et zsh développe `$=X`.
    assert_eq!(
        sortie("bash", &["--norc", "-c", "printf '%s' \"a$X\""]).unwrap(),
        "aVARIABLE"
    );
    if let Some(z) = sortie("zsh", &["-f", "-c", "printf '%s' \"a$=X\""]) {
        assert_eq!(z, "aVARIABLE");
    }
}
