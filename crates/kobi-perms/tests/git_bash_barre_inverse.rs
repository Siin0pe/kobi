//! KOB-497 : sous le vrai Git Bash, `\` sépare les dossiers. Entre guillemets
//! doubles, `"..\secret.txt"` sort donc du dossier de code : la ligne doit être
//! refusée, comme `'..\secret.txt'`. Contenu factice, dossier temporaire.
#![cfg(windows)]

use std::path::PathBuf;
use std::process::Command;

use kobi_perms::{commande, Classe, Contexte, Shell};

const BASH: &str = r"C:\Program Files\Git\bin\bash.exe";
const SECRET: &str = "contenu-factice";

#[test]
fn barre_inverse_entre_guillemets_doubles_sort_du_dossier_et_est_refusee() {
    for ligne in [r#"cat "..\secret.txt""#, r#"cat ".\..\secret.txt""#] {
        let racine = tempfile::tempdir().unwrap();
        let code = racine.path().join("code");
        std::fs::create_dir_all(&code).unwrap();
        std::fs::write(racine.path().join("secret.txt"), SECRET).unwrap();
        let sortie = Command::new(BASH)
            .args(["-c", ligne])
            .current_dir(&code)
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&sortie.stdout).contains(SECRET),
            "{ligne} : {sortie:?}"
        );
        let mut ctx = Contexte::new(vec![code.clone()]);
        ctx.cwd = Some(PathBuf::from(&code));
        let classe = commande::analyser(ligne, Shell::Bash, &ctx).classe;
        assert!(matches!(classe, Classe::Refus(_)), "{ligne} : {classe:?}");
    }
}
