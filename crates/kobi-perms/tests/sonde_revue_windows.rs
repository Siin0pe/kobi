//! SONDE (PR fermée sans merge) : revue qualité de #391 et #394, cas propres à
//! Windows. Échoue toujours, pour que le compte rendu apparaisse dans la CI.
#![cfg(windows)]

use std::path::PathBuf;
use std::process::Command;

use kobi_perms::{commande, Contexte, Shell};

#[test]
fn sonde_revue_windows() {
    let racine = tempfile::tempdir().unwrap();
    let code = racine.path().join("code");
    let dehors = racine.path().join("dehors");
    std::fs::create_dir_all(code.join("src")).unwrap();
    std::fs::create_dir_all(&dehors).unwrap();
    std::fs::write(dehors.join("secret.txt"), "x").unwrap();
    std::fs::write(code.join("src").join("a.rs"), "").unwrap();
    let j = Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(code.join("jonction"))
        .arg(&dehors)
        .output()
        .unwrap();
    let mut ctx = Contexte::new(vec![PathBuf::from(&code)]);
    ctx.cwd = Some(code.clone());
    let v = |l: &str| {
        let a = commande::analyser(l, Shell::PowerShell, &ctx);
        format!("{l} => {} ({})", a.classe.cle(), a.raison)
    };
    let cas = [
        format!("jonction creee : {}", j.status.success()),
        // B1 : racine du parcours prise dans une valeur d'option inconnue.
        v("Get-ChildItem -Recurse -ErrorAction SilentlyContinue"),
        v("Get-ChildItem -Recurse -ea 0"),
        v("Get-ChildItem -Recurse -Exclude target,node_modules"),
        // témoin : sans option, la jonction est vue.
        v("Get-ChildItem -Recurse"),
        // B6 : joker sur un chemin réel du dossier.
        v(r"Get-Content src\*.rs"),
        v("Select-String -Path src/*.rs motif"),
        // témoin
        v(r"Get-Content src\a.rs"),
    ];
    panic!("COMPTE RENDU\n{}", cas.join("\n"));
}
