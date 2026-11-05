//! KOB-474 : accent grave et fin de ligne, sous le vrai PowerShell. Une forme
//! qui donne une option à la commande doit passer par la carte. La forme est
//! jouée sur `git log --output`, dont l'effet se voit, et jugée sur
//! `rg --pre` : toute commande git passe déjà par la carte (KOB-473), ce qui
//! ne prouverait rien.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::process::Command;

use kobi_perms::{commande, Classe, Contexte, Shell};

fn git(dossier: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(args)
        .current_dir(dossier)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

/// Un dépôt avec un commit, pour que `git log` ait de quoi écrire.
fn depot() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    git(d.path(), &["init", "-q"]);
    git(d.path(), &["config", "user.name", "sonde"]);
    git(d.path(), &["config", "user.email", "sonde@example.invalid"]);
    git(d.path(), &["commit", "-q", "--allow-empty", "-m", "x"]);
    d
}

/// Nom, forme de l'option, et l'option atteint-elle la commande ?
type Forme = (&'static str, fn(&str) -> String, bool);

#[test]
fn accent_grave_et_fin_de_ligne() {
    let formes: [Forme; 6] = [
        ("LF", |o| format!("`\n{o}"), true),
        ("CRLF", |o| format!("`\r\n{o}"), true),
        ("CR", |o| format!("`\r{o}"), true),
        ("guillemets LF", |o| format!("\"`\n{o}\""), false),
        ("guillemets CRLF", |o| format!("\"`\r\n{o}\""), false),
        ("guillemets CR", |o| format!("\"`\r{o}\""), false),
    ];
    let mut ecarts = Vec::new();
    let mut compte_rendu = Vec::new();
    for (nom, forme, attendu) in formes {
        // Le classeur, sur une commande qu'aucune autre règle n'envoie à la carte.
        let d = tempfile::tempdir().unwrap();
        let mut ctx = Contexte::new(vec![PathBuf::from(d.path())]);
        ctx.cwd = Some(PathBuf::from(d.path()));
        let ligne = format!("rg {} a f", forme("--pre=./p"));
        let classe = commande::analyser(&ligne, Shell::PowerShell, &ctx).classe;
        if classe == Classe::Lecture {
            ecarts.push(format!("{nom} : {ligne:?} classé en lecture"));
        }
        // Le vrai PowerShell.
        for exe in ["powershell", "pwsh"] {
            let depot = depot();
            let ligne = format!("git log {}", forme("--output=m"));
            let sortie = Command::new(exe)
                .args(["-NoProfile", "-NonInteractive", "-Command", &ligne])
                .current_dir(depot.path())
                .output()
                .unwrap_or_else(|e| panic!("{exe} : lancement impossible : {e}"));
            let effet = depot.path().join("m").exists();
            compte_rendu.push(format!(
                "{exe} {nom} : m écrit = {effet} ({})",
                sortie.status
            ));
            if effet != attendu {
                ecarts.push(format!(
                    "{exe} {nom} : m écrit = {effet}, attendu {attendu}"
                ));
            }
        }
    }
    assert!(
        ecarts.is_empty(),
        "ÉCARTS\n{}\nCOMPTE RENDU\n{}",
        ecarts.join("\n"),
        compte_rendu.join("\n")
    );
}
