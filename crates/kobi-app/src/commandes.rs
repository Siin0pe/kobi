//! Commandes en lecture seule (KOB-154) : Claude Code peut lancer Bash et
//! PowerShell, mais chaque commande passe d'abord par le validateur de Kobi
//! ([`kobi_perms::commande`]), qui seul décide.
//!
//! Le branchement est un détail de l'adaptateur Claude Code : un hook
//! `PreToolUse` (fichier passé par `--settings`) lance `kobi valider-commande`,
//! qui lit l'appel sur l'entrée standard et répond « allow » ou « deny » avec
//! la raison. Bash et PowerShell ne sont pas dans `--allowedTools` : si le
//! hook ne répond pas, `--permission-mode dontAsk` refuse la commande.
//! Le même fichier borne la durée et la taille de sortie des commandes.

use std::io::{self, Read};
use std::path::{Path, PathBuf};

use kobi_brain::Action;
use kobi_perms::commande;
use kobi_perms::{Classe, Contexte};
use serde_json::{json, Value};

/// Sous-commande de `kobi` appelée par le hook.
pub const SOUS_COMMANDE: &str = "valider-commande";
/// Outils de commande de Claude Code soumis au hook.
pub const OUTILS_COMMANDE: [&str; 2] = ["Bash", "PowerShell"];
/// Durée d'une commande : par défaut et au plus (ms).
const DUREE_DEFAUT_MS: u32 = 20_000;
const DUREE_MAX_MS: u32 = 30_000;
/// Sortie gardée d'une commande (caractères).
const SORTIE_MAX: u32 = 20_000;

/// Ajouté au prompt système quand les commandes sont branchées.
pub const PROMPT_COMMANDES: &str = "\
## Commandes en lecture seule

Tu peux lancer des commandes qui ne modifient rien, avec l'outil Bash (ou PowerShell) : \
`git log`, `git status`, `git show`, `git diff`, `git blame`, `ls`, `cat`, `head`, `tail`, `wc`, \
`rg`, `grep`, `find`, `tree`, `cargo metadata`, `cargo tree`, `npm ls`, `gh pr view`, \
`gh issue list`, `Get-ChildItem`, `Get-Content`, `Select-String`… Kobi vérifie chaque commande \
avant de la lancer, dans les dossiers de code seulement, avec une durée et une sortie limitées.

- Lance la commande depuis un dossier de code (`cd <dossier> && git log -5` si besoin), \
sans `$( )`, variable, redirection vers un fichier ni commande qui modifie, crée ou supprime.
- Si Kobi refuse une commande, ne la contourne pas : dis en une phrase que tu ne fais que lire, \
et donne la commande prête à copier pour que l'utilisateur la lance lui-même.";

/// Branche les commandes sur les arguments d'une session en lecture seule :
/// Bash et PowerShell rejoignent `--tools` (pas `--allowedTools` : sans le
/// hook, `dontAsk` les refuse), le fichier de réglages passe par
/// `--settings`, et le prompt dit ce qui est permis. Sans réglages, rien ne
/// change : Kobi ne lance aucune commande.
pub fn brancher(mut args: Vec<String>, reglages: Option<&Path>) -> Vec<String> {
    let Some(reglages) = reglages else {
        return args;
    };
    for i in 0..args.len().saturating_sub(1) {
        match args[i].as_str() {
            "--tools" => {
                let mut outils: Vec<&str> = args[i + 1].split(',').collect();
                outils.extend(OUTILS_COMMANDE);
                args[i + 1] = outils.join(",");
            }
            "--disallowedTools" => {
                let garde: Vec<&str> = args[i + 1]
                    .split(',')
                    .filter(|o| !OUTILS_COMMANDE.contains(o))
                    .collect();
                args[i + 1] = garde.join(",");
            }
            "--append-system-prompt" => {
                args[i + 1] = format!("{}\n\n{PROMPT_COMMANDES}", args[i + 1]);
            }
            _ => {}
        }
    }
    args.push("--settings".into());
    args.push(reglages.display().to_string());
    args
}

/// Écrit le fichier `--settings` des sessions : le hook vers l'exécutable en
/// cours, et les bornes de durée et de sortie.
pub fn reglages(fichier: &Path) -> io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    reglages_avec(fichier, &exe)
}

fn reglages_avec(fichier: &Path, exe: &Path) -> io::Result<PathBuf> {
    // Barres obliques et guillemets : la ligne passe dans Git Bash comme dans cmd.
    let exe = exe.display().to_string().replace('\\', "/");
    let reglages = json!({
        "hooks": {
            "PreToolUse": [{
                "matcher": OUTILS_COMMANDE.join("|"),
                "hooks": [{ "type": "command", "command": format!("\"{exe}\" {SOUS_COMMANDE}"), "timeout": 10 }],
            }],
        },
        "env": {
            "BASH_DEFAULT_TIMEOUT_MS": DUREE_DEFAUT_MS.to_string(),
            "BASH_MAX_TIMEOUT_MS": DUREE_MAX_MS.to_string(),
            "BASH_MAX_OUTPUT_LENGTH": SORTIE_MAX.to_string(),
        },
    });
    if let Some(dossier) = fichier.parent() {
        std::fs::create_dir_all(dossier)?;
    }
    let texte = serde_json::to_string_pretty(&reglages).map_err(io::Error::other)?;
    std::fs::write(fichier, texte)?;
    Ok(fichier.to_owned())
}

/// `kobi valider-commande` : lit l'appel du hook sur l'entrée standard,
/// écrit la décision sur la sortie standard. Code de sortie 0 dans tous les
/// cas : une réponse illisible vaudrait refus côté Claude Code (`dontAsk`).
pub fn hook() -> i32 {
    let mut entree = String::new();
    let _ = io::stdin().read_to_string(&mut entree);
    let dossiers = kobi_session::config::default_path()
        .map(|p| kobi_session::config::load_or_create(&p).config.codebases)
        .unwrap_or_default();
    let sortie = decider(&entree, &crate::lecture::codebases_valides(&dossiers));
    println!("{sortie}");
    0
}

/// Décision du hook pour un appel (JSON de Claude Code) et des dossiers.
pub fn decider(entree: &str, dossiers: &[PathBuf]) -> Value {
    let appel: Value = serde_json::from_str(entree).unwrap_or_default();
    let outil = appel["tool_name"].as_str().unwrap_or_default();
    let params = &appel["tool_input"];
    let mut ctx = Contexte::new(dossiers.to_vec());
    ctx.cwd = appel["cwd"].as_str().map(PathBuf::from);
    if params["run_in_background"].as_bool() == Some(true) {
        return reponse(false, "Kobi ne lance pas de commande en arrière-plan.");
    }
    let action = kobi_brain::claude_code::outils::action(outil, params);
    let Action::Commande { ligne, shell, .. } = &action else {
        return reponse(false, "Kobi ne reconnaît pas cette commande.");
    };
    let avis = commande::analyser(ligne, *shell, &ctx);
    match avis.classe {
        Classe::Lecture => reponse(true, "commande en lecture seule, validée par Kobi"),
        Classe::Ecriture(_) => reponse(
            false,
            &format!(
                "Kobi ne lance que des commandes qui ne modifient rien : {}. \
                 Donne la commande à l'utilisateur, prête à copier, pour qu'il la lance lui-même.",
                avis.raison
            ),
        ),
        Classe::Refus(raison) => reponse(
            false,
            &format!(
                "Kobi ne lance de commandes que dans les dossiers de code déclarés : {raison}. \
                 Donne la commande à l'utilisateur, prête à copier."
            ),
        ),
    }
}

fn reponse(autorise: bool, raison: &str) -> Value {
    json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": if autorise { "allow" } else { "deny" },
            "permissionDecisionReason": raison,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kobi() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    fn appel(outil: &str, commande: &str, cwd: &Path) -> String {
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": outil,
            "tool_input": { "command": commande },
            "cwd": cwd.display().to_string(),
        })
        .to_string()
    }

    fn decision(v: &Value) -> &str {
        v["hookSpecificOutput"]["permissionDecision"]
            .as_str()
            .unwrap()
    }

    #[test]
    fn decisions_du_hook() {
        let d = [kobi()];
        let v = decider(&appel("Bash", "git log --oneline -5", &kobi()), &d);
        assert_eq!(decision(&v), "allow");
        let v = decider(&appel("Bash", "rm -rf target", &kobi()), &d);
        assert_eq!(decision(&v), "deny");
        let raison = v["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .unwrap();
        assert!(
            raison.contains("supprime") && raison.contains("prête à copier"),
            "{raison}"
        );
        let v = decider(&appel("PowerShell", "Get-ChildItem src", &kobi()), &d);
        assert_eq!(decision(&v), "allow");
        // Lancé ailleurs que dans un dossier de code : refusé.
        let v = decider(&appel("Bash", "ls", &std::env::temp_dir()), &d);
        assert_eq!(decision(&v), "deny");
        // Sans dossier déclaré, rien ne passe.
        let v = decider(&appel("Bash", "ls", &kobi()), &[]);
        assert_eq!(decision(&v), "deny");
        // Entrée illisible ou en arrière-plan : refus.
        assert_eq!(decision(&decider("pas du json", &d)), "deny");
        let fond = json!({"tool_name": "Bash", "tool_input": {"command": "ls", "run_in_background": true}, "cwd": kobi()});
        assert_eq!(decision(&decider(&fond.to_string(), &d)), "deny");
    }

    #[test]
    fn commandes_branchees_sur_la_session() {
        let base = crate::lecture::arguments("haiku", "Tu es Kobi.", &[kobi()], None, None, false);
        assert_eq!(brancher(base.clone(), None), base);
        let args = brancher(base, Some(Path::new("C:/Kobi/mcp/reglages-claude.json")));
        let valeur = |o: &str| {
            args.windows(2)
                .find(|w| w[0] == o)
                .map(|w| w[1].clone())
                .unwrap()
        };
        assert_eq!(valeur("--tools"), "Read,Grep,Glob,Bash,PowerShell");
        // Jamais autorisées d'avance : seul le hook les laisse passer.
        assert_eq!(valeur("--allowedTools"), "Read,Grep,Glob");
        assert_eq!(valeur("--permission-mode"), "dontAsk");
        let refuses = valeur("--disallowedTools");
        assert!(
            !refuses.split(',').any(|o| o == "Bash" || o == "PowerShell"),
            "{refuses}"
        );
        assert!(
            refuses.split(',').any(|o| o == "Write")
                && refuses.split(',').any(|o| o == "BashOutput")
        );
        assert_eq!(valeur("--settings"), "C:/Kobi/mcp/reglages-claude.json");
        let prompt = valeur("--append-system-prompt");
        assert!(prompt.starts_with("Tu es Kobi.") && prompt.contains("Commandes en lecture seule"));
    }

    /// Essai contre le vrai Claude Code (KOB-154) : le hook est l'exécutable
    /// `kobi` de `KOBI_EXE`, qui lit les dossiers de code de `config.toml`,
    /// et `KOBI_DEPOT` est un dépôt git de ces dossiers. Ignoré par défaut :
    /// `KOBI_EXE=… KOBI_DEPOT=… cargo test -p kobi-app -- --ignored commandes_reelles --nocapture`.
    #[test]
    #[ignore = "demande Claude Code connecté et un kobi.exe construit"]
    fn commandes_reelles() {
        use kobi_brain::claude_code::Event;
        use kobi_brain::{ClaudeCode, ClaudeOptions, Detection, SYSTEM_PROMPT};
        let exe = PathBuf::from(std::env::var("KOBI_EXE").expect("KOBI_EXE"));
        let depot = PathBuf::from(std::env::var("KOBI_DEPOT").expect("KOBI_DEPOT"));
        let rt = tokio::runtime::Runtime::new().unwrap();
        let program = match rt.block_on(kobi_brain::claude_code::detect::detect(
            std::time::Duration::from_secs(30),
        )) {
            Detection::Ready { install, .. } => install.program,
            autre => panic!("Claude Code indisponible : {autre:?}"),
        };
        let dossier = std::env::temp_dir().join(format!("kobi-commandes-{}", std::process::id()));
        let reglages = reglages_avec(&dossier.join("reglages-claude.json"), &exe).unwrap();
        let args = brancher(
            crate::lecture::arguments(
                "haiku",
                SYSTEM_PROMPT,
                std::slice::from_ref(&depot),
                None,
                None,
                false,
            ),
            Some(&reglages),
        );
        let _contexte = rt.enter();
        let mut claude = ClaudeCode::spawn(&ClaudeOptions {
            program,
            cwd: Some(depot.clone()),
            partial_messages: false,
            args,
            env: Vec::new(),
        })
        .unwrap();
        // Question -> (réponse, commandes lancées, résultats d'outils en erreur).
        let mut demander = |question: &str| -> (String, Vec<String>, Vec<String>) {
            rt.block_on(async {
                claude
                    .send(&format!("[interpellation]\n\n## Question\n\n{question}\n"))
                    .await
                    .unwrap();
                let (mut commandes, mut erreurs) = (Vec::new(), Vec::new());
                loop {
                    let e = claude.next_event().await.expect("flux");
                    let blocs = |m: &Value| m["content"].as_array().cloned().unwrap_or_default();
                    match &e {
                        Event::Assistant { message } => {
                            for b in blocs(message) {
                                if b["type"] == "tool_use" {
                                    commandes.push(format!(
                                        "{} : {}",
                                        b["name"].as_str().unwrap_or(""),
                                        b["input"]["command"].as_str().unwrap_or("")
                                    ));
                                }
                            }
                        }
                        Event::User { message } => {
                            for b in blocs(message) {
                                if b["type"] == "tool_result" && b["is_error"] == true {
                                    erreurs.push(b["content"].to_string());
                                }
                            }
                        }
                        Event::Result(r) => {
                            return (r.result.clone().unwrap_or_default(), commandes, erreurs)
                        }
                        _ => {}
                    }
                }
            })
        };
        let (commits, c1, e1) =
            demander("Quels sont les 5 derniers commits de ce dépôt ? Lance git log.");
        println!("commits -> {commits}\n  commandes {c1:?}\n  erreurs {e1:?}");
        let (suppression, c2, e2) = demander("Supprime le dossier target de ce dépôt.");
        println!("suppression -> {suppression}\n  commandes {c2:?}\n  erreurs {e2:?}");
        assert!(c1.iter().any(|c| c.contains("git log")), "{c1:?}");
        assert!(e1.is_empty(), "git log refusé : {e1:?}");
        // La suppression n'est jamais lancée : refusée par Kobi, ou pas même tentée.
        for c in &c2 {
            if c.contains("rm ") || c.contains("Remove-Item") {
                assert!(
                    e2.iter().any(|e| e.contains("Kobi")),
                    "suppression non refusée : {c2:?} {e2:?}"
                );
            }
        }
        assert!(
            suppression.contains("rm") || suppression.contains("Remove-Item"),
            "commande à copier absente : {suppression}"
        );
        let _ = std::fs::remove_dir_all(dossier);
    }

    #[test]
    fn fichier_de_reglages() {
        let dossier = std::env::temp_dir().join(format!("kobi-reglages-{}", std::process::id()));
        let fichier = dossier.join("reglages.json");
        reglages_avec(&fichier, Path::new(r"C:\Program Files\Kobi\kobi.exe")).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&fichier).unwrap()).unwrap();
        let hook = &v["hooks"]["PreToolUse"][0];
        assert_eq!(hook["matcher"], "Bash|PowerShell");
        assert_eq!(
            hook["hooks"][0]["command"],
            "\"C:/Program Files/Kobi/kobi.exe\" valider-commande"
        );
        assert_eq!(v["env"]["BASH_MAX_TIMEOUT_MS"], "30000");
        assert_eq!(v["env"]["BASH_MAX_OUTPUT_LENGTH"], "20000");
        std::fs::remove_dir_all(dossier).unwrap();
    }
}
