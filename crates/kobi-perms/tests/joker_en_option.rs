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

/// KOB-512 : un programme MSYS retire les guillemets d'un argument, et Windows
/// PowerShell 5.1 ne protège pas les guillemets doubles d'une valeur (sonde #475).
#[cfg(windows)]
#[test]
fn powershell_guillemets_retires_avant_le_programme() {
    let chemin = format!(
        r"C:\Program Files\Git\usr\bin;{}",
        std::env::var("PATH").unwrap()
    );
    let lancer = |exe: &str, ligne: &str, dossier: &Path| {
        let o = Command::new(exe)
            .args(["-NoProfile", "-NonInteractive", "-Command", ligne])
            .current_dir(dossier)
            .env("PATH", &chemin)
            .output()
            .unwrap();
        String::from_utf8_lossy(&o.stdout).into_owned()
    };
    let dossiers = || {
        let d = tempfile::tempdir().unwrap();
        let ici = d.path().join("ici");
        std::fs::create_dir_all(&ici).unwrap();
        std::fs::create_dir_all(d.path().join("dehors")).unwrap();
        std::fs::write(d.path().join("dehors").join("b"), "SECRET_B\n").unwrap();
        std::fs::write(ici.join("a"), "x\n").unwrap();
        (d, ici)
    };
    // Lit hors du dossier sous les deux PowerShell.
    for exe in ["powershell", "pwsh"] {
        let (_d, ici) = dossiers();
        let ligne = "head.exe \"'../dehors/b'\"";
        assert!(
            lancer(exe, ligne, &ici).contains("SECRET_B"),
            "{exe} {ligne}"
        );
        assert_ne!(
            classe(ligne, Shell::PowerShell, &ici),
            Classe::Lecture,
            "{ligne}"
        );
    }
    // Windows PowerShell 5.1 seulement.
    let (_d, ici) = dossiers();
    let ligne = "head.exe '\"../dehors/b\"'";
    assert!(
        lancer("powershell", ligne, &ici).contains("SECRET_B"),
        "{ligne}"
    );
    assert_ne!(
        classe(ligne, Shell::PowerShell, &ici),
        Classe::Lecture,
        "{ligne}"
    );
    let (_d, ici) = dossiers();
    let ligne = "sort.exe '\"--output=M\"' a";
    lancer("powershell", ligne, &ici);
    assert!(ici.join("M").exists(), "{ligne}");
    assert_ne!(
        classe(ligne, Shell::PowerShell, &ici),
        Classe::Lecture,
        "{ligne}"
    );
}

/// KOB-513 : sous Windows, un programme MSYS étend le motif, même cité ; une cmdlet
/// n'étend rien. Le motif cité d'un programme natif passe par la carte, pas un refus.
#[cfg(windows)]
#[test]
fn msys_etend_le_motif_cite_la_cmdlet_non() {
    let chemin = format!(
        r"C:\Program Files\Git\usr\bin;{}",
        std::env::var("PATH").unwrap()
    );
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a"), "").unwrap();
    std::fs::write(d.path().join("x"), "").unwrap();
    for exe in ["powershell", "pwsh"] {
        let sortie = |ligne: &str| {
            let o = Command::new(exe)
                .args(["-NoProfile", "-NonInteractive", "-Command", ligne])
                .current_dir(d.path())
                .env("PATH", &chemin)
                .output()
                .unwrap();
            String::from_utf8_lossy(&o.stdout).trim().to_owned()
        };
        assert_eq!(
            sortie("echo.exe '[ax]'"),
            "a x",
            "{exe} : MSYS étend le motif cité"
        );
        assert_eq!(
            sortie("Write-Output '[ax]'"),
            "[ax]",
            "{exe} : la cmdlet n'étend rien"
        );
    }
    assert_eq!(
        classe(
            "Select-String -Pattern '[ax]' -Path a",
            Shell::PowerShell,
            d.path()
        ),
        Classe::Lecture
    );
    let c = classe(r"rg '\[test\]' .", Shell::PowerShell, d.path());
    assert!(!matches!(c, Classe::Refus(_)), "{c:?}");
}

/// KOB-513 : `pwsh` hors Windows n'étend pas un mot cité, mais étend un mot non cité
/// et y lit `\` comme `/` (vu avec le vrai pwsh par la relecture).
#[cfg(unix)]
#[test]
fn pwsh_hors_windows_cite_ou_non() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir(d.path().join("sub")).unwrap();
    std::fs::write(d.path().join("sub").join("lien2"), "").unwrap();
    std::fs::write(d.path().join("l1"), "").unwrap();
    let pwsh = |ligne: &str| {
        Command::new("pwsh")
            .args(["-NoProfile", "-NonInteractive", "-Command", ligne])
            .current_dir(d.path())
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
    };
    let Ok(cite) = pwsh("/bin/echo 'l*'") else {
        // Pas de pwsh sur cette machine : le test tourne sur la CI.
        return;
    };
    assert_eq!(cite, "l*");
    assert_eq!(pwsh(r"/bin/echo sub\*").unwrap(), "sub/lien2");
    // Commencé hors guillemets, le mot est étendu même si le joker est cité.
    assert_eq!(pwsh("/bin/echo sub/'*'").unwrap(), "sub/lien2");
    assert_eq!(pwsh("/bin/echo sub\"/*\"").unwrap(), "sub/lien2");
    let c = classe("grep -v /root/.ssh/'*' f", Shell::PowerShell, d.path());
    assert!(matches!(c, Classe::Refus(_)), "{c:?}");
    assert_eq!(
        classe("rg 'l*' .", Shell::PowerShell, d.path()),
        Classe::Lecture
    );
    let c = classe(r"grep -v /root/.ssh\* f", Shell::PowerShell, d.path());
    assert!(matches!(c, Classe::Refus(_)), "{c:?}");
}

/// KOB-513 : `..` n'est ajouté à un motif qui commence par un point que s'il peut le
/// former (`.*`, `.[.]`, classe POSIX) ; un filtre jq comme `.[]` ne le forme jamais.
#[test]
fn point_point_seulement_si_le_motif_peut_le_former() {
    let d = tempfile::tempdir().unwrap();
    let ici = d.path().join("ici");
    std::fs::create_dir_all(&ici).unwrap();
    std::fs::write(ici.join("a"), "").unwrap();
    for ligne in ["cat .*", "cat .[.]", "cat .?"] {
        let c = classe(ligne, Shell::Bash, &ici);
        assert!(matches!(c, Classe::Refus(_)), "{ligne} : {c:?}");
    }
    // Une classe POSIX : jamais une lecture (carte, ou refus sous Windows où `:`
    // désigne un lecteur).
    assert_ne!(
        classe("cat .[[:punct:]]", Shell::Bash, &ici),
        Classe::Lecture
    );
    for ligne in ["cat .[n]", "cat .[]", "cat .items[0].name", "jq -c '.[]' a"] {
        assert_eq!(classe(ligne, Shell::Bash, &ici), Classe::Lecture, "{ligne}");
    }
}
