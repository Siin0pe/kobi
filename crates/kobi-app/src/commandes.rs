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
//!
//! KOB-156 : une commande qui modifie, et l'écriture d'un fichier (Write,
//! Edit, MultiEdit), ne sont plus refusées d'office. Le hook demande la carte
//! de validation à l'app en cours (`validation.ask`, par le point d'accès de
//! contrôle) et attend le « oui » ; sans réponse en 60 s, c'est non. Le
//! réglage `[permissions]` peut laisser passer sans demander, jamais une
//! suppression.

use std::io::{self, Read};
use std::path::{Path, PathBuf};

use kobi_brain::Action;
use kobi_perms::commande;
use kobi_perms::{classer, Classe, Contexte, Genre};
use kobi_session::config::{PermissionsConfig, Politique};
use serde_json::{json, Value};

/// Sous-commande de `kobi` appelée par le hook.
pub const SOUS_COMMANDE: &str = "valider-commande";
/// Outils de commande de Claude Code soumis au hook.
pub const OUTILS_COMMANDE: [&str; 2] = ["Bash", "PowerShell"];
/// Outils d'écriture de fichier soumis au hook (KOB-156).
pub const OUTILS_FICHIER: [&str; 3] = ["Write", "Edit", "MultiEdit"];
/// Le hook attend la réponse à la carte (60 s) : un peu plus que ça.
const ATTENTE_HOOK_S: u32 = 75;

fn outils_du_hook() -> impl Iterator<Item = &'static str> {
    OUTILS_COMMANDE.into_iter().chain(OUTILS_FICHIER)
}
/// Durée d'une commande : par défaut et au plus (ms).
const DUREE_DEFAUT_MS: u32 = 20_000;
const DUREE_MAX_MS: u32 = 30_000;
/// Sortie gardée d'une commande (caractères).
const SORTIE_MAX: u32 = 20_000;

/// Ajouté au prompt système quand les commandes sont branchées.
pub const PROMPT_COMMANDES: &str = "\
## Commandes et fichiers de code

Tu peux lancer des commandes avec l'outil Bash (ou PowerShell), et écrire dans les dossiers de code \
avec Write et Edit. Kobi vérifie chaque appel avant qu'il parte, dans les dossiers de code seulement, \
avec une durée et une sortie limitées. Pour les commandes et les fichiers de code, ceci remplace \
« tu ne fais que lire ».

- Ce qui ne modifie rien passe tout de suite : `ls`, `cat`, `head`, `tail`, `wc`, `rg`, `grep`, \
`find`, `tree`, `gh pr view`, `gh issue list`, `Get-ChildItem`, `Get-Content`, `Select-String`…
- Les commandes git (`git log`, `git status`, `git diff`…) demandent un « oui » de l'utilisateur, \
car elles peuvent lancer des programmes configurés dans le dépôt : regroupe-les en une seule \
commande, et lis plutôt les fichiers quand cela suffit.
- Une commande qui modifie, crée ou supprime, et toute écriture de fichier, attendent un « oui » de \
l'utilisateur : Kobi lui montre une carte et attend jusqu'à une minute. Appelle simplement l'outil, \
sans demander l'accord en texte, même pour une suppression : la carte de Kobi est la confirmation.
- Lance la commande depuis un dossier de code (`cd <dossier> && git log -5` si besoin), sans `$( )`, \
variable ni redirection vers un fichier.
- Écris `./*.md` plutôt que `*.md` : un joker en tête de mot demande une validation.
- Si Kobi refuse (refus de l'utilisateur, pas de réponse, hors des dossiers), ne contourne pas et ne \
réessaie pas : dis-le en une phrase et donne la commande ou le contenu prêt à copier.";

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
                let outils: Vec<String> = args[i + 1]
                    .split(',')
                    .map(str::to_owned)
                    .chain(outils_du_hook().map(str::to_owned))
                    .collect();
                args[i + 1] = outils.join(",");
            }
            "--disallowedTools" => {
                let garde: Vec<&str> = args[i + 1]
                    .split(',')
                    .filter(|o| !outils_du_hook().any(|h| h == *o))
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
                "matcher": outils_du_hook().collect::<Vec<_>>().join("|"),
                "hooks": [{ "type": "command", "command": format!("\"{exe}\" {SOUS_COMMANDE}"), "timeout": ATTENTE_HOOK_S }],
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
    let config = kobi_session::config::default_path()
        .map(|p| kobi_session::config::load_or_create(&p).config)
        .unwrap_or_default();
    let dossiers = crate::lecture::codebases_valides(&config.codebases);
    let sortie = decider_avec(&entree, &dossiers, &config.permissions, &demander_a_l_app);
    println!("{sortie}");
    0
}

/// Demande la carte de validation de `appel` (`{tool, input}`) à l'app en
/// cours et attend la réponse : `accepte`, `refuse` ou `delai` ; `None` si
/// l'app ne répond pas. L'app présente l'appel (diff d'un fichier, commande).
fn demander_a_l_app(genre: &'static str, resume: String, appel: Value) -> Option<String> {
    let point = kobi_ctl::Point::defaut()?;
    let params = json!({
        "kind": genre,
        "summary": resume,
        "tool": appel["tool"],
        "input": appel["input"],
        "reason": appel["reason"],
    });
    match kobi_ctl::client::appeler(&point, "validation.ask", params, kobi_ctl::Origine::Cli) {
        Ok(v) => v["decision"].as_str().map(str::to_owned),
        Err(_) => None,
    }
}

/// Décision du hook. `demander` ouvre la carte de validation et rend la
/// décision de l'utilisateur, ou `None` si personne ne peut répondre.
pub fn decider_avec(
    entree: &str,
    dossiers: &[PathBuf],
    politique: &PermissionsConfig,
    demander: &dyn Fn(&'static str, String, Value) -> Option<String>,
) -> Value {
    let appel: Value = serde_json::from_str(entree).unwrap_or_default();
    let outil = appel["tool_name"].as_str().unwrap_or_default();
    let params = &appel["tool_input"];
    let mut ctx = Contexte::new(dossiers.to_vec());
    ctx.cwd = appel["cwd"].as_str().map(PathBuf::from);
    if params["run_in_background"].as_bool() == Some(true) {
        return reponse(false, "Kobi ne lance pas de commande en arrière-plan.");
    }
    let action = kobi_brain::claude_code::outils::action(outil, params);
    let (classe, raison) = match &action {
        Action::Commande { ligne, shell, .. } if OUTILS_COMMANDE.contains(&outil) => {
            let avis = commande::analyser(ligne, *shell, &ctx);
            (avis.classe, avis.raison)
        }
        Action::EcrireFichier { .. } if OUTILS_FICHIER.contains(&outil) => {
            (classer(&action, &ctx), "écrit un fichier".to_owned())
        }
        _ => return reponse(false, "Kobi ne reconnaît pas cette action."),
    };
    let genre = match classe {
        Classe::Lecture => return reponse(true, "commande en lecture seule, validée par Kobi"),
        Classe::Refus(raison) => {
            return reponse(
                false,
                &format!(
                    "Kobi n'agit que dans les dossiers de code déclarés : {raison}. \
                     Donne à l'utilisateur la commande ou le contenu, prêts à copier."
                ),
            )
        }
        Classe::Ecriture(genre) => genre,
    };
    let (genre, regle) = match genre {
        // Une suppression se demande toujours (contrat des permissions, §3).
        Genre::Suppression => ("suppression", Politique::Demander),
        Genre::Fichier => ("fichier", politique.fichier),
        _ => ("commande", politique.commande),
    };
    if regle == Politique::SansDemander {
        return reponse(true, "autorisé sans demander (réglage [permissions])");
    }
    let mut entree = params.clone();
    if let (Some(o), Some(cwd)) = (entree.as_object_mut(), &ctx.cwd) {
        o.entry("cwd")
            .or_insert_with(|| json!(cwd.display().to_string()));
    }
    let resume = resume(&action, &ctx);
    // La raison de kobi-perms part avec l'appel : la carte dit pourquoi Kobi
    // demande (« modifie un alias », « git peut lancer… », KOB-483).
    // Pour un fichier, le titre de la carte dit déjà tout : pas de raison.
    let motif = if OUTILS_COMMANDE.contains(&outil) {
        raison.as_str()
    } else {
        ""
    };
    let appel = json!({ "tool": outil, "input": entree, "reason": motif });
    match demander(genre, resume, appel).as_deref() {
        Some("accepte") => reponse(true, "validé par l'utilisateur sur la carte de Kobi"),
        Some("delai") => reponse(
            false,
            "Pas de réponse de l'utilisateur en 60 s : refusé, rien n'a été fait. \
             Ne réessaie pas ; dis-le en une phrase et donne la commande ou le contenu prêt à copier.",
        ),
        Some(_) => reponse(
            false,
            "L'utilisateur a refusé : rien n'a été fait. \
             Ne réessaie pas ; dis-le en une phrase.",
        ),
        None => reponse(
            false,
            &format!(
                "Kobi ne peut pas demander l'accord de l'utilisateur (l'app ne répond pas) : {raison}. \
                 Donne à l'utilisateur la commande ou le contenu, prêts à copier, pour qu'il le fasse lui-même."
            ),
        ),
    }
}

/// Résumé de la carte d'un fichier : seul le hook sait s'il le crée ou le
/// réécrit. Pour une commande, l'app résume elle-même (description donnée
/// par l'agent, « Supprimer : … »), comme le détail (diff, commande, dossier).
fn resume(action: &Action, ctx: &Contexte) -> String {
    match action {
        Action::EcrireFichier { chemin, avant, .. } => {
            let relatif = kobi_perms::action::relatif(chemin, &ctx.dossiers);
            let vars = [("chemin", relatif.as_str())];
            match avant {
                Some(_) => kobi_i18n::tr("coque.commandes.modifier", &vars),
                None if chemin.exists() => kobi_i18n::tr("coque.commandes.reecrire", &vars),
                None => kobi_i18n::tr("coque.commandes.creer", &vars),
            }
        }
        _ => String::new(),
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

    /// Décision du hook pour un appel (JSON de Claude Code) et des dossiers,
    /// sans app pour valider : une écriture est refusée, avec le texte à copier.
    fn decider(entree: &str, dossiers: &[PathBuf]) -> Value {
        decider_avec(
            entree,
            dossiers,
            &PermissionsConfig::default(),
            &|_, _, _| None,
        )
    }

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
        // Une lecture (git passe par la carte depuis KOB-473).
        let v = decider(&appel("Bash", "ls -la", &kobi()), &d);
        assert_eq!(decision(&v), "allow");
        let v = decider(&appel("Bash", "rm -rf target", &kobi()), &d);
        assert_eq!(decision(&v), "deny");
        let raison = v["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .unwrap();
        assert!(
            raison.contains("supprime") && raison.contains("à copier"),
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

    /// KOB-483 : la raison de kobi-perms part avec la demande de carte, pour
    /// que la carte dise pourquoi Kobi demande.
    #[test]
    fn raison_envoyee_a_la_carte() {
        use std::cell::RefCell;
        let d = [kobi()];
        let defaut = PermissionsConfig::default();
        for (outil, l, attendu) in [
            ("PowerShell", "Remove-Item alias:where -Force", "alias"),
            ("Bash", "git status", "git peut lancer"),
            ("Bash", "touch NOTES.md", "touch"),
        ] {
            let vu: RefCell<Option<Value>> = RefCell::default();
            let demander = |_: &'static str, _: String, appel: Value| {
                *vu.borrow_mut() = Some(appel);
                Some("refuse".to_owned())
            };
            decider_avec(&appel(outil, l, &kobi()), &d, &defaut, &demander);
            let appel = vu.borrow().clone().expect("carte demandée");
            let raison = appel["reason"].as_str().unwrap_or_default();
            assert!(raison.contains(attendu), "{l} : {raison:?}");
        }
        // Une écriture de fichier : le titre suffit, pas de raison.
        let vu: RefCell<Option<Value>> = RefCell::default();
        let demander = |_: &'static str, _: String, appel: Value| {
            *vu.borrow_mut() = Some(appel);
            Some("refuse".to_owned())
        };
        let write = json!({"hook_event_name": "PreToolUse", "tool_name": "Write",
            "tool_input": {"file_path": kobi().join("notes.md").display().to_string(), "content": "x"},
            "cwd": kobi().display().to_string()})
        .to_string();
        decider_avec(&write, &d, &defaut, &demander);
        assert_eq!(vu.borrow().as_ref().expect("carte demandée")["reason"], "");
    }

    /// KOB-484 : chaque commande que le prompt annonce comme « passe tout de
    /// suite » est bien une lecture pour kobi-perms (git n'y est plus depuis
    /// KOB-473).
    #[test]
    fn prompt_annonce_seulement_des_lectures() {
        let ligne = PROMPT_COMMANDES
            .lines()
            .skip_while(|l| !l.contains("passe tout de suite"))
            .take_while(|l| !l.starts_with("- Les commandes git"))
            .collect::<Vec<_>>()
            .join(" ");
        let commandes: Vec<&str> = ligne.split('`').skip(1).step_by(2).collect();
        assert!(commandes.len() >= 10, "{commandes:?}");
        let mut ctx = Contexte::new(vec![kobi()]);
        ctx.cwd = Some(kobi());
        for c in commandes {
            let shell = if c.contains('-') && c.starts_with(char::is_uppercase) {
                kobi_perms::Shell::PowerShell
            } else {
                kobi_perms::Shell::Bash
            };
            let avis = commande::analyser(c, shell, &ctx);
            assert!(
                avis.est_lecture(),
                "« {c} » annoncé sans carte : {}",
                avis.raison
            );
        }
        // Et git, lui, demande un oui : le prompt le dit.
        assert!(!commande::analyser("git status", kobi_perms::Shell::Bash, &ctx).est_lecture());
        assert!(PROMPT_COMMANDES.contains("Les commandes git"));
    }

    /// KOB-489 : le conseil du prompt dit vrai : `./*.md` passe, `*.md` demande un oui.
    #[test]
    fn prompt_conseille_un_joker_qui_passe() {
        assert!(PROMPT_COMMANDES.contains("`./*.md` plutôt que `*.md`"));
        let mut ctx = Contexte::new(vec![kobi()]);
        ctx.cwd = Some(kobi());
        let lecture = |l: &str| commande::analyser(l, kobi_perms::Shell::Bash, &ctx).est_lecture();
        assert!(lecture("ls ./*.md"));
        assert!(!lecture("ls *.md"));
    }

    /// KOB-496 : avec `commande = sans_demander`, un mot à trois tirets ne
    /// fait plus passer une suppression ni une lecture hors des dossiers.
    #[test]
    fn trois_tirets_ne_contournent_pas_le_hook() {
        let d = [kobi()];
        let libre = PermissionsConfig {
            commande: Politique::SansDemander,
            ..PermissionsConfig::default()
        };
        let demander = |_: &'static str, _: String, _: Value| Some("refuse".to_owned());
        for l in [
            "rm -rf -- ---x target",
            "rm -rf target",
            "cat -- ---x /etc/passwd",
            "cat /etc/passwd",
        ] {
            let v = decider_avec(&appel("Bash", l, &kobi()), &d, &libre, &demander);
            assert_eq!(decision(&v), "deny", "{l}");
        }
    }

    /// Écritures (KOB-156) : la carte est demandée, et la réponse décide.
    #[test]
    fn ecritures_validees_par_la_carte() {
        use std::cell::RefCell;
        let d = [kobi()];
        let demandes: RefCell<Vec<(String, String)>> = RefCell::default();
        let avec = |reponse: Option<&'static str>, politique: &PermissionsConfig, entree: &str| {
            let demander = |genre: &'static str, resume: String, appel: Value| {
                // L'app reçoit l'appel d'origine, dossier de travail compris.
                assert!(appel["tool"].is_string(), "{appel}");
                assert_eq!(appel["input"]["cwd"], json!(kobi().display().to_string()));
                demandes.borrow_mut().push((genre.to_owned(), resume));
                reponse.map(str::to_owned)
            };
            decider_avec(entree, &d, politique, &demander)
        };
        let defaut = PermissionsConfig::default();
        let rm = appel("Bash", "rm -rf target", &kobi());
        assert_eq!(decision(&avec(Some("accepte"), &defaut, &rm)), "allow");
        assert_eq!(
            demandes.borrow().last().unwrap(),
            &("suppression".to_owned(), String::new())
        );
        assert_eq!(decision(&avec(Some("refuse"), &defaut, &rm)), "deny");
        let v = avec(Some("delai"), &defaut, &rm);
        assert!(v["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .unwrap()
            .contains("60 s"));
        // L'app ne répond pas : refus, avec le texte à copier.
        let v = avec(None, &defaut, &rm);
        assert_eq!(decision(&v), "deny");
        assert!(v["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .unwrap()
            .contains("à copier"));
        // Lecture : pas de carte.
        let n = demandes.borrow().len();
        let log = appel("Bash", "ls -la", &kobi());
        assert_eq!(decision(&avec(Some("refuse"), &defaut, &log)), "allow");
        assert_eq!(demandes.borrow().len(), n);
        // « Sans demander » pour les commandes : sauf une suppression.
        let libre = PermissionsConfig {
            commande: Politique::SansDemander,
            ..PermissionsConfig::default()
        };
        let touch = appel("Bash", "touch NOTES.md", &kobi());
        assert_eq!(decision(&avec(Some("refuse"), &libre, &touch)), "allow");
        assert_eq!(demandes.borrow().len(), n);
        assert_eq!(decision(&avec(Some("refuse"), &libre, &rm)), "deny");
        assert_eq!(demandes.borrow().len(), n + 1);
        // Fichiers : carte « fichier » dans les dossiers, refus ailleurs.
        let ecrire = |chemin: &Path| {
            json!({
                "tool_name": "Write",
                "tool_input": { "file_path": chemin.display().to_string(), "content": "bonjour" },
                "cwd": kobi(),
            })
            .to_string()
        };
        let neuf = kobi().join("pas-encore-la.md");
        assert_eq!(
            decision(&avec(Some("accepte"), &defaut, &ecrire(&neuf))),
            "allow"
        );
        assert_eq!(
            demandes.borrow().last().unwrap(),
            &("fichier".to_owned(), "Créer pas-encore-la.md".to_owned())
        );
        let ailleurs = std::env::temp_dir().join("kobi-ailleurs.md");
        let n = demandes.borrow().len();
        assert_eq!(
            decision(&avec(Some("accepte"), &defaut, &ecrire(&ailleurs))),
            "deny"
        );
        assert_eq!(demandes.borrow().len(), n);
        let modifier = json!({
            "tool_name": "Edit",
            "tool_input": { "file_path": kobi().join("Cargo.toml").display().to_string(), "old_string": "a", "new_string": "b" },
            "cwd": kobi(),
        });
        assert_eq!(
            decision(&avec(Some("refuse"), &defaut, &modifier.to_string())),
            "deny"
        );
        assert_eq!(demandes.borrow().last().unwrap().1, "Modifier Cargo.toml");
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
        assert_eq!(
            valeur("--tools"),
            "Read,Grep,Glob,Bash,PowerShell,Write,Edit,MultiEdit"
        );
        // Jamais autorisées d'avance : seul le hook les laisse passer.
        assert_eq!(valeur("--allowedTools"), "Read,Grep,Glob");
        assert_eq!(valeur("--permission-mode"), "dontAsk");
        let refuses = valeur("--disallowedTools");
        assert!(
            !refuses.split(',').any(|o| outils_du_hook().any(|h| h == o)),
            "{refuses}"
        );
        assert!(
            refuses.split(',').any(|o| o == "NotebookEdit")
                && refuses.split(',').any(|o| o == "BashOutput")
        );
        assert_eq!(valeur("--settings"), "C:/Kobi/mcp/reglages-claude.json");
        let prompt = valeur("--append-system-prompt");
        assert!(
            prompt.starts_with("Tu es Kobi.") && prompt.contains("Commandes et fichiers de code")
        );
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
        // Une lecture sans carte : pas git, qui passe par la carte (KOB-473).
        let (fichiers, c1, e1) =
            demander("Liste les fichiers à la racine de ce dépôt. Lance ls avec l'outil Bash.");
        println!("fichiers -> {fichiers}\n  commandes {c1:?}\n  erreurs {e1:?}");
        let (suppression, c2, e2) = demander("Supprime le dossier target de ce dépôt.");
        println!("suppression -> {suppression}\n  commandes {c2:?}\n  erreurs {e2:?}");
        assert!(c1.iter().any(|c| c.starts_with("ls")), "{c1:?}");
        assert!(e1.is_empty(), "ls refusé : {e1:?}");
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
        assert_eq!(hook["matcher"], "Bash|PowerShell|Write|Edit|MultiEdit");
        assert_eq!(hook["hooks"][0]["timeout"], 75);
        assert_eq!(
            hook["hooks"][0]["command"],
            "\"C:/Program Files/Kobi/kobi.exe\" valider-commande"
        );
        assert_eq!(v["env"]["BASH_MAX_TIMEOUT_MS"], "30000");
        assert_eq!(v["env"]["BASH_MAX_OUTPUT_LENGTH"], "20000");
        std::fs::remove_dir_all(dossier).unwrap();
    }
}
