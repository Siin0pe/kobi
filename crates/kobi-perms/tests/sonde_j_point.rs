//! SONDE (j) (ne pas merger) : Windows PowerShell 5.1 coupe-t-il `-nom.suite` au
//! premier point pour un exécutable natif (`-q../secret` -> `-q`, `../secret`) ?
//! Comparé à pwsh 7. Échoue toujours, pour que le compte rendu apparaisse.
#![cfg(windows)]

use std::process::Command;

#[test]
fn sonde_j_point() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(
        d.path().join("argv.py"),
        "import sys\nprint(repr(sys.argv[1:]))\n",
    )
    .unwrap();
    let mut compte = Vec::new();
    for exe in ["powershell", "pwsh"] {
        for args in [
            "-q../secret",
            "-Dmaven.test.skip=true",
            "-x.y",
            "--a.b",
            "-q..\\secret",
        ] {
            let ligne = format!("python argv.py {args}");
            let r = match Command::new(exe)
                .args(["-NoProfile", "-NonInteractive", "-Command", &ligne])
                .current_dir(d.path())
                .output()
            {
                Ok(o) => String::from_utf8_lossy(&o.stdout).trim().to_owned(),
                Err(e) => format!("lancement impossible : {e}"),
            };
            compte.push(format!("{exe} {ligne:?} => {r}"));
        }
    }
    panic!("COMPTE RENDU (j)\n{}", compte.join("\n"));
}
