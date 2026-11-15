//! KOB-517 : sous PowerShell, `gci.exe` ou `Get-Content.exe` lancent une application
//! du PATH, jamais le cmdlet. Vu avec le vrai PowerShell (sonde #481).
#[cfg(windows)]
#[test]
fn powershell_lance_le_gci_exe_du_path() {
    use std::path::PathBuf;
    use std::process::Command;

    use kobi_perms::{commande, Classe, Contexte, Shell};

    let d = tempfile::tempdir().unwrap();
    let bin = d.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let whoami = PathBuf::from(std::env::var("SystemRoot").unwrap())
        .join("System32")
        .join("whoami.exe");
    std::fs::copy(&whoami, bin.join("gci.exe")).unwrap();
    let chemin = format!("{};{}", bin.display(), std::env::var("PATH").unwrap());
    for exe in ["powershell", "pwsh"] {
        let o = Command::new(exe)
            .args(["-NoProfile", "-NonInteractive", "-Command", "gci.exe"])
            .current_dir(d.path())
            .env("PATH", &chemin)
            .output()
            .unwrap();
        let texte = String::from_utf8_lossy(&o.stdout);
        // whoami affiche `domaine\utilisateur` : l'exécutable planté a été lancé.
        assert!(texte.contains('\\'), "{exe} : {texte:?}");
    }
    let mut ctx = Contexte::new(vec![PathBuf::from(d.path())]);
    ctx.cwd = Some(PathBuf::from(d.path()));
    assert_ne!(
        commande::analyser("gci.exe", Shell::PowerShell, &ctx).classe,
        Classe::Lecture
    );
}
