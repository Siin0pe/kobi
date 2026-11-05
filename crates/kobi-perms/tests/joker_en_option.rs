//! KOB-489 : un joker non cité en tête de mot peut devenir une option. Vu avec
//! le vrai shell : le fichier piégé est un nom d'option, et la règle ne dépend
//! pas du disque (`touch -- --pre=x; rg a *` crée le piège dans la même ligne).
use std::path::{Path, PathBuf};
use std::process::Command;

use kobi_perms::{commande, Classe, Contexte, Shell};

fn classe(ligne: &str, shell: Shell, dossier: &Path) -> Classe {
    let mut ctx = Contexte::new(vec![PathBuf::from(dossier)]);
    ctx.cwd = Some(PathBuf::from(dossier));
    commande::analyser(ligne, shell, &ctx).classe
}

/// Bash étend le joker en une option : `sort` écrit le fichier qu'elle nomme.
#[cfg(unix)]
#[test]
fn bash_le_joker_devient_une_option() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("--output=M"), "").unwrap();
    std::fs::write(d.path().join("a"), "x\n").unwrap();
    let ligne = "sort *";
    let ok = Command::new("bash")
        .args(["-c", ligne])
        .current_dir(d.path())
        .output()
        .unwrap();
    assert!(d.path().join("M").exists(), "{ok:?}");
    assert_ne!(classe(ligne, Shell::Bash, d.path()), Classe::Lecture);
}

/// Dans un dossier sans piège aussi : la règle ne lit pas le disque.
#[test]
fn la_regle_ne_depend_pas_du_disque() {
    let d = tempfile::tempdir().unwrap();
    for ligne in ["sort *", "rg a *", "touch -- --pre=x; rg a *"] {
        assert_ne!(
            classe(ligne, Shell::Bash, d.path()),
            Classe::Lecture,
            "{ligne}"
        );
    }
}

/// `pwsh` hors Windows étend le joker pour un exécutable natif.
#[cfg(unix)]
#[test]
fn pwsh_hors_windows_etend_pour_un_natif() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("--piege"), "").unwrap();
    let Ok(sortie) = Command::new("pwsh")
        .args(["-NoProfile", "-NonInteractive", "-Command", "/bin/echo *"])
        .current_dir(d.path())
        .output()
    else {
        // Pas de pwsh sur cette machine : le test tourne sur la CI.
        return;
    };
    let texte = String::from_utf8_lossy(&sortie.stdout);
    assert!(texte.contains("--piege"), "{texte:?}");
    assert_ne!(
        classe("rg a *", Shell::PowerShell, d.path()),
        Classe::Lecture
    );
    assert_ne!(
        classe("sort *", Shell::PowerShell, d.path()),
        Classe::Lecture
    );
    // `pwsh` normalise le chemin avant de l'étendre : le `./` disparaît (KOB-502).
    std::fs::create_dir(d.path().join("src")).unwrap();
    for motif in [
        "./*",
        "src/../*",
        "././*",
        "--[p]iege",
        "[a-c-e]*",
        "s*/../--piege",
    ] {
        let sortie = Command::new("pwsh")
            .args(["-NoProfile", "-NonInteractive", "-Command"])
            .arg(format!("/bin/echo {motif}"))
            .current_dir(d.path())
            .output()
            .unwrap();
        let texte = String::from_utf8_lossy(&sortie.stdout);
        assert!(
            texte.split_whitespace().any(|m| m == "--piege"),
            "{motif} {texte:?}"
        );
        let ligne = format!("sort {motif}");
        assert_ne!(
            classe(&ligne, Shell::PowerShell, d.path()),
            Classe::Lecture,
            "{ligne}"
        );
    }
}

/// Sous Windows, PowerShell lui-même n'étend rien pour un exécutable natif : `*`
/// reste `*`. Un programme MSYS l'étend ensuite (test suivant, KOB-502).
#[cfg(windows)]
#[test]
fn powershell_sous_windows_n_etend_pas_pour_un_natif() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("--piege"), "").unwrap();
    for exe in ["powershell", "pwsh"] {
        let sortie = Command::new(exe)
            .args(["-NoProfile", "-NonInteractive", "-Command", "cmd /c echo *"])
            .current_dir(d.path())
            .output()
            .unwrap();
        let texte = String::from_utf8_lossy(&sortie.stdout);
        assert!(
            !texte.contains("--piege") && texte.contains('*'),
            "{exe} {texte:?}"
        );
    }
}

/// Une cmdlet reçoit le motif sans expansion par le shell : reste une lecture.
#[test]
fn cmdlet_avec_joker_en_lecture() {
    let d = tempfile::tempdir().unwrap();
    for ligne in ["Get-ChildItem *.md", "Get-ChildItem -Filter *.rs"] {
        assert_eq!(
            classe(ligne, Shell::PowerShell, d.path()),
            Classe::Lecture,
            "{ligne}"
        );
    }
}

/// KOB-502 : avec `Git\usr\bin` dans le PATH, un programme MSYS étend lui-même le
/// joker, même cité. Les alias de cmdlets (`sort`) n'y passent pas.
#[cfg(windows)]
#[test]
fn powershell_avec_les_outils_msys_dans_le_path() {
    let chemin = format!(
        r"C:\Program Files\Git\usr\bin;{}",
        std::env::var("PATH").unwrap()
    );
    let lancer = |exe: &str, ligne: &str, dossier: &Path| {
        Command::new(exe)
            .args(["-NoProfile", "-NonInteractive", "-Command", ligne])
            .current_dir(dossier)
            .env("PATH", &chemin)
            .output()
            .unwrap();
    };
    for exe in ["powershell", "pwsh"] {
        for ligne in ["sort.exe *", "sort.exe '*'"] {
            let d = tempfile::tempdir().unwrap();
            std::fs::write(d.path().join("--output=M"), "").unwrap();
            std::fs::write(d.path().join("a"), "x\n").unwrap();
            lancer(exe, ligne, d.path());
            assert!(d.path().join("M").exists(), "{exe} {ligne}");
            assert_ne!(
                classe(ligne, Shell::PowerShell, d.path()),
                Classe::Lecture,
                "{ligne}"
            );
        }
        for ligne in ["uniq.exe src/*", "uniq.exe 'src/*'", "uniq src/*"] {
            let d = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(d.path().join("src")).unwrap();
            std::fs::write(d.path().join("src/a"), "l\n").unwrap();
            std::fs::write(d.path().join("src/b"), "ANCIEN\n").unwrap();
            lancer(exe, ligne, d.path());
            let b = std::fs::read_to_string(d.path().join("src/b")).unwrap();
            assert!(!b.contains("ANCIEN"), "{exe} {ligne}");
            assert_ne!(
                classe(ligne, Shell::PowerShell, d.path()),
                Classe::Lecture,
                "{ligne}"
            );
        }
        // Témoin : `sort` est l'alias `Sort-Object`, qui n'étend rien.
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("--output=M"), "").unwrap();
        lancer(exe, "sort *", d.path());
        assert!(!d.path().join("M").exists(), "{exe}");
        assert_eq!(
            classe("sort *", Shell::PowerShell, d.path()),
            Classe::Lecture
        );
        assert_eq!(
            classe("ls *.md", Shell::PowerShell, d.path()),
            Classe::Lecture
        );
        assert_eq!(
            classe("cat *.md", Shell::PowerShell, d.path()),
            Classe::Lecture
        );
    }
}
