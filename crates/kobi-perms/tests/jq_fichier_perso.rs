//! KOB-525 : jq charge `~/.jq` tout seul. Une fonction de ce fichier au nom anodin
//! (`def ouvrir: env;`) affiche l'environnement : avec un `~/.jq`, toute commande
//! jq passe par la carte. Vu avec le vrai jq, dans un HOME isolé et une variable
//! factice.
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::Command;

use kobi_perms::{commande, Classe, Contexte, Shell};

fn classe(ligne: &str, code: &Path, home: Option<&Path>) -> Classe {
    let mut ctx = Contexte::new(vec![code.to_path_buf()]);
    ctx.cwd = Some(code.to_path_buf());
    ctx.dossiers_personnels = home.map(PathBuf::from).into_iter().collect();
    commande::analyser(ligne, Shell::Bash, &ctx).classe
}

#[test]
fn avec_un_fichier_jq_toute_commande_jq_sur_la_carte() {
    let home = tempfile::tempdir().unwrap();
    let code = home.path().join("code");
    std::fs::create_dir_all(&code).unwrap();
    // Sans ~/.jq : la lecture de jq reste une lecture.
    assert_eq!(
        classe("jq -n ouvrir", &code, Some(home.path())),
        Classe::Lecture
    );
    assert_eq!(
        classe("jq . a.json", &code, Some(home.path())),
        Classe::Lecture
    );
    std::fs::write(home.path().join(".jq"), "def ouvrir: env;\n").unwrap();
    for ligne in ["jq -n ouvrir", "jq . a.json", "jq -r .x a.json"] {
        assert_ne!(
            classe(ligne, &code, Some(home.path())),
            Classe::Lecture,
            "{ligne}"
        );
    }
    // HOME inconnu : rien n'y est cherché.
    assert_eq!(classe("jq -n ouvrir", &code, None), Classe::Lecture);
}

/// jq 1.7 et plus charge `~/jq/main.jq` quand il n'y a pas de `~/.jq` (relecture
/// de KOB-525).
#[test]
fn avec_jq_main_jq_toute_commande_jq_sur_la_carte() {
    let home = tempfile::tempdir().unwrap();
    let code = home.path().join("code");
    std::fs::create_dir_all(home.path().join("jq")).unwrap();
    std::fs::create_dir_all(&code).unwrap();
    std::fs::write(home.path().join("jq").join("main.jq"), "def ouvrir: env;\n").unwrap();
    assert_ne!(
        classe("jq -n ouvrir", &code, Some(home.path())),
        Classe::Lecture
    );
}

/// Le vrai jq charge `~/.jq` : la fonction de l'utilisateur affiche la variable.
#[cfg(unix)]
#[test]
fn vrai_jq_charge_le_fichier_perso() {
    let home = tempfile::tempdir().unwrap();
    let code = home.path().join("code");
    std::fs::create_dir_all(&code).unwrap();
    std::fs::write(home.path().join(".jq"), "def ouvrir: env;\n").unwrap();
    let Ok(sortie) = Command::new("jq")
        .args(["-n", "ouvrir"])
        .current_dir(&code)
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin")
        .env("HOME", home.path())
        .env("KOBI_FAUX", "valeur-factice")
        .output()
    else {
        return;
    };
    if sortie.status.code() != Some(127) {
        assert!(
            String::from_utf8_lossy(&sortie.stdout).contains("valeur-factice"),
            "{sortie:?}"
        );
    }
    assert_ne!(
        classe("jq -n ouvrir", &code, Some(home.path())),
        Classe::Lecture
    );
}

/// KOB-534 : un dossier `~/.jq` (modules) n'est pas chargé par jq : seule une
/// `import` explicite, déjà sur la carte, y prend une fonction. La lecture reste une
/// lecture. Vu avec le vrai jq quand il est là.
#[test]
fn un_dossier_jq_n_est_pas_charge() {
    let home = tempfile::tempdir().unwrap();
    let code = home.path().join("code");
    std::fs::create_dir_all(&code).unwrap();
    std::fs::create_dir_all(home.path().join(".jq").join("jq")).unwrap();
    std::fs::write(home.path().join(".jq").join("m.jq"), "def ouvrir: env;\n").unwrap();
    std::fs::write(
        home.path().join(".jq").join("jq").join("main.jq"),
        "def ouvrir: env;\n",
    )
    .unwrap();
    for ligne in ["jq . a.json", "jq -r .x a.json", "jq -n ouvrir"] {
        assert_eq!(
            classe(ligne, &code, Some(home.path())),
            Classe::Lecture,
            "{ligne}"
        );
    }
    // `import` reste sur la carte (KOB-463).
    assert_ne!(
        classe(
            "jq -n 'import \"m\" as m; m::ouvrir'",
            &code,
            Some(home.path())
        ),
        Classe::Lecture
    );
    #[cfg(unix)]
    if let Ok(sortie) = Command::new("jq")
        .args(["-n", "ouvrir"])
        .current_dir(&code)
        .env_clear()
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin")
        .output()
    {
        // `ouvrir` n'est pas défini : rien n'a été chargé.
        assert!(!sortie.status.success(), "{sortie:?}");
    }
}

/// KOB-534 : la raison nomme le fichier trouvé.
#[test]
fn la_raison_nomme_le_fichier_trouve() {
    let raison = |home: &Path| {
        let code = home.join("code");
        let mut ctx = Contexte::new(vec![code.clone()]);
        ctx.cwd = Some(code);
        ctx.dossiers_personnels = vec![home.to_path_buf()];
        commande::analyser("jq . a.json", Shell::Bash, &ctx).raison
    };
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join("code")).unwrap();
    std::fs::create_dir_all(home.path().join("jq")).unwrap();
    std::fs::write(home.path().join("jq").join("main.jq"), "def ouvrir: env;\n").unwrap();
    assert!(
        raison(home.path()).contains("~/jq/main.jq"),
        "{}",
        raison(home.path())
    );
    std::fs::write(home.path().join(".jq"), "def ouvrir: env;\n").unwrap();
    assert!(
        raison(home.path()).contains("~/.jq"),
        "{}",
        raison(home.path())
    );
}

/// Relecture de KOB-534 : un dossier `~/.jq` avec un fichier `~/jq/main.jq` reste
/// sur la carte (par prudence pour jq 1.7+), un lien vers un fichier aussi, et une
/// FIFO aussi : jq 1.6 la charge.
#[cfg(unix)]
#[test]
fn ce_que_jq_charge_reste_sur_la_carte() {
    let cas = |prep: &dyn Fn(&Path)| {
        let home = tempfile::tempdir().unwrap();
        let code = home.path().join("code");
        std::fs::create_dir_all(&code).unwrap();
        prep(home.path());
        classe("jq . a.json", &code, Some(home.path()))
    };
    let dossier_et_main = |h: &Path| {
        std::fs::create_dir_all(h.join(".jq")).unwrap();
        std::fs::create_dir_all(h.join("jq")).unwrap();
        std::fs::write(h.join("jq").join("main.jq"), "def ouvrir: env;\n").unwrap();
    };
    assert_ne!(cas(&dossier_et_main), Classe::Lecture);
    let lien = |h: &Path| {
        std::fs::write(h.join("vrai.jq"), "def ouvrir: env;\n").unwrap();
        std::os::unix::fs::symlink(h.join("vrai.jq"), h.join(".jq")).unwrap();
    };
    assert_ne!(cas(&lien), Classe::Lecture);
    let fifo = |h: &Path| {
        let ok = Command::new("mkfifo").arg(h.join(".jq")).status().unwrap();
        assert!(ok.success());
    };
    assert_ne!(cas(&fifo), Classe::Lecture);
}
