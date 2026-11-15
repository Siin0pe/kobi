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

/// KOB-521 : `dir.exe -LR sub` (le `dir` de Git) suit un lien vers l'extérieur sous
/// les deux PowerShell ; Kobi ne le classe pas en lecture.
#[cfg(windows)]
#[test]
fn powershell_dir_exe_suit_un_lien() {
    use std::path::PathBuf;
    use std::process::Command;

    use kobi_perms::{commande, Classe, Contexte, Shell};

    let chemin = format!(
        r"C:\Program Files\Git\usr\bin;{}",
        std::env::var("PATH").unwrap()
    );
    let d = tempfile::tempdir().unwrap();
    let ici = d.path().join("ici");
    let dehors = d.path().join("dehors");
    std::fs::create_dir_all(ici.join("sub")).unwrap();
    std::fs::create_dir_all(&dehors).unwrap();
    std::fs::write(dehors.join("SECRET_DEHORS"), "x").unwrap();
    std::os::windows::fs::symlink_dir(&dehors, ici.join("sub").join("lien")).unwrap();
    for exe in ["powershell", "pwsh"] {
        let o = Command::new(exe)
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "dir.exe -LR sub",
            ])
            .current_dir(&ici)
            .env("PATH", &chemin)
            .output()
            .unwrap();
        let texte = String::from_utf8_lossy(&o.stdout);
        assert!(texte.contains("SECRET_DEHORS"), "{exe} : {texte:?}");
    }
    let mut ctx = Contexte::new(vec![PathBuf::from(&ici)]);
    ctx.cwd = Some(PathBuf::from(&ici));
    let c = commande::analyser("dir.exe -LR sub", Shell::PowerShell, &ctx).classe;
    assert_ne!(c, Classe::Lecture);
}
