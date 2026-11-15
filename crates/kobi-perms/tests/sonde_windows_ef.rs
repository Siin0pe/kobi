//! Sonde temporaire (PR fermée sans merge) : points (e), (f) et `grep -v` de
//! priorites.md. N'affirme rien : affiche les effets observés dans le journal.
#![cfg(windows)]
use std::path::Path;
use std::process::Command;

macro_rules! note {
    ($r:ident, $($t:tt)*) => {{
        $r.push_str(&format!($($t)*));
        $r.push('\n');
    }};
}

const MSYS: &str = r"C:\Program Files\Git\usr\bin";

fn lancer(exe: &str, ligne: &str, dossier: &Path, msys: bool) -> String {
    let mut c = Command::new(exe);
    c.args(["-NoProfile", "-NonInteractive", "-Command", ligne])
        .current_dir(dossier);
    if msys {
        c.env("PATH", format!("{MSYS};{}", std::env::var("PATH").unwrap()));
    }
    match c.output() {
        Ok(o) => format!(
            "rc={:?} out={:?} err={:?}",
            o.status.code(),
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        ),
        Err(e) => format!("lancement impossible : {e}"),
    }
}

#[test]
fn sonde_e_rg_fd_n_etendent_rien() {
    let mut r = String::new();
    for exe in ["powershell", "pwsh"] {
        for outil in ["rg", "fd"] {
            let ou = lancer(
                exe,
                &format!("(Get-Command {outil}).Source"),
                Path::new("."),
                false,
            );
            note!(r, "SONDE e {exe} {outil} chemin : {ou}");
        }
        for ligne in [
            "rg x '*'",
            "rg x *",
            "rg -g '*.rs' x",
            "rg --glob '*.rs' x",
            "fd -g '*.rs'",
            "fd . '*'",
        ] {
            let d = tempfile::tempdir().unwrap();
            // `--files` : si l'outil étendait le joker, il listerait les fichiers.
            std::fs::write(d.path().join("--files"), "").unwrap();
            std::fs::write(d.path().join("--files.rs"), "").unwrap();
            std::fs::write(d.path().join("a.rs"), "x\n").unwrap();
            std::fs::write(d.path().join("-H"), "").unwrap();
            note!(
                r,
                "SONDE e {exe} [{ligne}] : {}",
                lancer(exe, ligne, d.path(), true)
            );
        }
    }
    panic!("RAPPORT DE SONDE (échec voulu)\n{r}");
}

#[test]
fn sonde_f_guillemets_doubles_litteraux() {
    let mut r = String::new();
    for exe in ["powershell", "pwsh"] {
        for ligne in [
            r#"sort.exe '"--output=M"'"#,
            r#"sort.exe '"-o" M'"#,
            r#"sort.exe "`"--output=M`"""#,
        ] {
            let d = tempfile::tempdir().unwrap();
            std::fs::write(d.path().join("a"), "x\n").unwrap();
            let sortie = lancer(exe, &format!("{ligne} a"), d.path(), true);
            let m = d.path().join("M").exists();
            note!(r, "SONDE f {exe} [{ligne} a] M_cree={m} : {sortie}");
        }
    }
    panic!("RAPPORT DE SONDE (échec voulu)\n{r}");
}

#[test]
fn sonde_grep_v_motif_a_joker() {
    let mut r = String::new();
    for exe in ["powershell", "pwsh"] {
        for ligne in [
            "grep.exe -v '../dehors/*' f",
            "grep.exe -v ../dehors/* f",
            "grep -v '../dehors/*' f",
            "diff.exe '../dehors/*' f",
            // Accolades et guillemets : MSYS les traite-t-il lui-même ?
            "head.exe '{f,../dehors/b}'",
            "head.exe {f,../dehors/b}",
            "sed.exe -n p '{f,../dehors/b}'",
            r#"head.exe '"../dehors/b"'"#,
            r#"head.exe '"..\dehors\b"'"#,
            "head.exe \"'../dehors/b'\"",
            "cat.exe '{f,../dehors/b}'",
        ] {
            let d = tempfile::tempdir().unwrap();
            let ici = d.path().join("ici");
            let dehors = d.path().join("dehors");
            std::fs::create_dir_all(&ici).unwrap();
            std::fs::create_dir_all(&dehors).unwrap();
            std::fs::write(dehors.join("a"), "SECRET_A\n").unwrap();
            std::fs::write(dehors.join("b"), "SECRET_B\n").unwrap();
            std::fs::write(ici.join("f"), "local\n").unwrap();
            let sortie = lancer(exe, ligne, &ici, true);
            let fuite = sortie.contains("SECRET_B");
            note!(r, "SONDE grep {exe} [{ligne}] fuite={fuite} : {sortie}");
        }
    }
    panic!("RAPPORT DE SONDE (échec voulu)\n{r}");
}
