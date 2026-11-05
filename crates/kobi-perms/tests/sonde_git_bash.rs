//! SONDE (PR fermée sans merge) : KOB-497. Sous Git Bash, `cat "..\x"` sort-il du
//! dossier ? Et `sort.exe` de MSYS étend-il `*` sous PowerShell ? Échoue toujours,
//! pour que le compte rendu apparaisse dans la CI. Tout se passe dans un dossier
//! temporaire, avec un contenu factice.
#![cfg(windows)]

use std::path::PathBuf;
use std::process::Command;

use kobi_perms::{commande, Contexte, Shell};

const BASH: &str = r"C:\Program Files\Git\bin\bash.exe";
const SORT: &str = r"C:\Program Files\Git\usr\bin\sort.exe";
const SECRET: &str = "contenu-factice-de-sonde";

fn classe(ligne: &str, shell: Shell, code: &std::path::Path) -> String {
    let mut ctx = Contexte::new(vec![PathBuf::from(code)]);
    ctx.cwd = Some(PathBuf::from(code));
    let a = commande::analyser(ligne, shell, &ctx);
    format!("{:?}", a.classe)
}

#[test]
fn sonde_git_bash() {
    let mut cas = vec![format!(
        "bash existe = {} ; sort existe = {}",
        PathBuf::from(BASH).exists(),
        PathBuf::from(SORT).exists()
    )];
    // Git Bash : la barre oblique inverse entre guillemets doubles.
    for ligne in [
        r#"cat "..\secret.txt""#,
        r#"cat "..\..\racine\secret.txt""#,
        r"cat '..\secret.txt'",
        r"cat ..\secret.txt",
        r#"ls "..\""#,
        r#"cat ".\..\secret.txt""#,
    ] {
        let racine = tempfile::tempdir().unwrap();
        let code = racine.path().join("code");
        std::fs::create_dir_all(&code).unwrap();
        std::fs::write(racine.path().join("secret.txt"), SECRET).unwrap();
        let k = classe(ligne, Shell::Bash, &code);
        let o = Command::new(BASH)
            .args(["-c", ligne])
            .current_dir(&code)
            .output();
        let r = match o {
            Ok(o) => {
                let t = format!(
                    "{}{}",
                    String::from_utf8_lossy(&o.stdout),
                    String::from_utf8_lossy(&o.stderr)
                );
                format!(
                    "code {:?} secret lu = {} sortie {:?}",
                    o.status.code(),
                    t.contains(SECRET),
                    t.chars().take(90).collect::<String>()
                )
            }
            Err(e) => format!("lancement impossible : {e}"),
        };
        cas.push(format!("GITBASH {ligne:?} => classe {k} | {r}"));
    }
    // PowerShell et les programmes MSYS : qui étend `*` ?
    for exe in ["powershell", "pwsh"] {
        for (nom, ligne) in [
            ("sort-etoile", format!("& '{SORT}' *")),
            ("sort-cite", format!("& '{SORT}' '*'")),
            (
                "echo-msys",
                r"& 'C:\Program Files\Git\usr\bin\echo.exe' *".to_owned(),
            ),
            ("sort-court", "sort.exe *".to_owned()),
        ] {
            let d = tempfile::tempdir().unwrap();
            std::fs::write(d.path().join("--output=M"), "").unwrap();
            std::fs::write(d.path().join("a"), "x\n").unwrap();
            let k = classe(&ligne, Shell::PowerShell, d.path());
            let o = Command::new(exe)
                .args(["-NoProfile", "-NonInteractive", "-Command", &ligne])
                .current_dir(d.path())
                .output();
            let r = match o {
                Ok(o) => format!(
                    "code {:?} M cree = {} sortie {:?} erreur {:?}",
                    o.status.code(),
                    d.path().join("M").exists(),
                    String::from_utf8_lossy(&o.stdout)
                        .chars()
                        .take(70)
                        .collect::<String>(),
                    String::from_utf8_lossy(&o.stderr)
                        .chars()
                        .take(90)
                        .collect::<String>()
                ),
                Err(e) => format!("lancement impossible : {e}"),
            };
            cas.push(format!("{exe} {nom} {ligne:?} => classe {k} | {r}"));
        }
    }
    // `Git\usr\bin` dans le PATH (option « outils Unix » de Git pour Windows).
    let chemin = format!(
        r"C:\Program Files\Git\usr\bin;{}",
        std::env::var("PATH").unwrap()
    );
    for exe in ["powershell", "pwsh"] {
        for (nom, ligne, piege, marque) in [
            ("sort-exe", "sort.exe *", "--output=M", "M"),
            ("sort-exe-cite", "sort.exe '*'", "--output=M", "M"),
            ("sort-exe-chemin", "sort.exe ./*", "--output=M", "M"),
            ("sort-exe-fixe", "sort.exe a*", "a--output=M", "M"),
            ("grep-exe", "grep.exe x *", "--include=a", "a"),
            ("sort-alias", "sort *", "--output=M", "M"),
        ] {
            let d = tempfile::tempdir().unwrap();
            std::fs::write(d.path().join(piege), "").unwrap();
            std::fs::write(d.path().join("a"), "x\n").unwrap();
            let k = classe(ligne, Shell::PowerShell, d.path());
            let o = Command::new(exe)
                .args(["-NoProfile", "-NonInteractive", "-Command", ligne])
                .current_dir(d.path())
                .env("PATH", &chemin)
                .output();
            let r = match o {
                Ok(o) => format!(
                    "code {:?} {marque} cree = {} sortie {:?} erreur {:?}",
                    o.status.code(),
                    d.path().join("M").exists(),
                    String::from_utf8_lossy(&o.stdout)
                        .chars()
                        .take(70)
                        .collect::<String>(),
                    String::from_utf8_lossy(&o.stderr)
                        .chars()
                        .take(70)
                        .collect::<String>()
                ),
                Err(e) => format!("lancement impossible : {e}"),
            };
            cas.push(format!("PATH {exe} {nom} {ligne:?} => classe {k} | {r}"));
        }
    }
    // `uniq` de MSYS : `uniq a b` écrit dans `b`. Étend-il lui-même le joker ?
    let uniq = r"C:\Program Files\Git\usr\bin\uniq.exe";
    for exe in ["powershell", "pwsh"] {
        for (nom, ligne, avec_path) in [
            ("uniq-chemin", format!("& '{uniq}' src/*"), false),
            ("uniq-chemin-cite", format!("& '{uniq}' 'src/*'"), false),
            ("uniq-exe-path", "uniq.exe src/*".to_owned(), true),
            ("uniq-exe-path-cite", "uniq.exe 'src/*'".to_owned(), true),
            ("uniq-nom-path", "uniq src/*".to_owned(), true),
        ] {
            let d = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(d.path().join("src")).unwrap();
            std::fs::write(d.path().join("src/a"), "l\n").unwrap();
            std::fs::write(d.path().join("src/b"), "ANCIEN\n").unwrap();
            let k = classe(&ligne, Shell::PowerShell, d.path());
            let mut c = Command::new(exe);
            c.args(["-NoProfile", "-NonInteractive", "-Command", &ligne])
                .current_dir(d.path());
            if avec_path {
                c.env("PATH", &chemin);
            }
            let r = match c.output() {
                Ok(o) => format!(
                    "code {:?} b = {:?} sortie {:?} erreur {:?}",
                    o.status.code(),
                    std::fs::read_to_string(d.path().join("src/b")).unwrap_or_default(),
                    String::from_utf8_lossy(&o.stdout)
                        .chars()
                        .take(50)
                        .collect::<String>(),
                    String::from_utf8_lossy(&o.stderr)
                        .chars()
                        .take(70)
                        .collect::<String>()
                ),
                Err(e) => format!("lancement impossible : {e}"),
            };
            cas.push(format!("UNIQ {exe} {nom} {ligne:?} => classe {k} | {r}"));
        }
    }
    panic!("COMPTE RENDU\n{}", cas.join("\n"));
}
