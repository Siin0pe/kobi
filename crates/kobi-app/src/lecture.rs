//! Kobi en lecture seule (KOB-138) : pour l'instant, Claude ne fait que lire
//! et chercher dans les dossiers de code déclarés (`codebases` de
//! `config.toml`). Ni écriture, ni commande, ni outil web, ni MCP : la
//! session n'a que Read, Grep et Glob, et refuse le reste sans carte.
//!
//! Voir « Kobi — parcours utilisateur et règles d'UX », principe 6.

use std::path::{Path, PathBuf};

use kobi_session::config;

use kobi_brain::claude_code::Event;

/// Seuls outils de la session.
pub const OUTILS_LECTURE: [&str; 3] = ["Read", "Grep", "Glob"];

/// Outils refusés explicitement, en plus de `--tools` qui ne garde que la
/// lecture : écriture, commandes, web, sous-agents.
pub const OUTILS_REFUSES: &[&str] = &[
    "Bash",
    "BashOutput",
    "KillShell",
    "PowerShell",
    "Edit",
    "MultiEdit",
    "Write",
    "NotebookEdit",
    "WebFetch",
    "WebSearch",
    "Task",
    "Agent",
];

/// Ajouté au prompt système de Kobi.
pub const PROMPT_LECTURE: &str = "\
## Pour l'instant, tu ne fais que lire

Tu as seulement les outils Read, Grep et Glob, sur les dossiers de code de l'utilisateur. \
Tu n'écris aucun fichier, ne lances aucune commande et n'agis sur aucun outil externe \
(tickets, messages, agenda…), même si on te le demande. C'est vrai partout, en réunion comme au quotidien.

- Pour une question sur le code, cherche avec Grep et Glob, lis avec Read, et cite le \
fichier et la ligne (`chemin/fichier.rs:42`).
- Si on te demande une action (créer, modifier, envoyer, lancer, installer), réponds en \
une phrase que tu ne peux pas encore le faire, puis donne le texte prêt à copier \
(le ticket, le message, la commande) pour que l'utilisateur le fasse lui-même. \
La raison est toujours la même : pour l'instant, tu ne fais que lire. Ne parle jamais de \
réunion dans ce refus. Exemple : « Je ne peux pas encore lancer de commande, je ne fais que \
lire pour l'instant. La voici à copier : `cargo test` ».
- Les règles « Actions » plus haut, y compris « pas de shell pendant une réunion », \
sont remplacées par celles-ci tant que tu es en lecture seule.";

/// Arguments de `claude` (après ceux du mode stream-json) pour une session
/// en lecture seule sur `codebases`.
pub fn arguments(modele: &str, systeme: &str, codebases: &[PathBuf]) -> Vec<String> {
    let mut args: Vec<String> = [
        "--model",
        modele,
        // Ni hooks ni réglages de l'utilisateur, ni MCP de sa config.
        "--setting-sources",
        "project",
        "--strict-mcp-config",
        // Seuls outils disponibles, et autorisés sans demande.
        "--tools",
        &OUTILS_LECTURE.join(","),
        "--allowedTools",
        &OUTILS_LECTURE.join(","),
        "--disallowedTools",
        &OUTILS_REFUSES.join(","),
        // Tout ce qui n'est pas autorisé d'avance est refusé, sans carte.
        "--permission-mode",
        "dontAsk",
    ]
    .map(String::from)
    .into();
    for dossier in codebases {
        args.push("--add-dir".into());
        args.push(dossier.display().to_string());
    }
    args.push("--append-system-prompt".into());
    args.push(format!("{systeme}\n\n{PROMPT_LECTURE}"));
    args
}

/// Dossiers déclarés qui existent ; les autres sont signalés et ignorés.
pub fn codebases_valides(codebases: &[PathBuf]) -> Vec<PathBuf> {
    codebases
        .iter()
        .filter(|d| {
            let ok = d.is_dir();
            if !ok {
                tracing::warn!(dossier = %d.display(), "codebase introuvable, ignorée");
            }
            ok
        })
        .cloned()
        .collect()
}

/// Outils appelés dans un message `assistant` du flux.
pub fn outils_appeles(event: &Event) -> Vec<String> {
    let Event::Assistant { message } = event else {
        return Vec::new();
    };
    message
        .get("content")
        .and_then(|c| c.as_array())
        .into_iter()
        .flatten()
        .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
        .filter_map(|b| b.get("name").and_then(|n| n.as_str()).map(String::from))
        .collect()
}

/// L'outil est-il permis en lecture seule ?
pub fn permis(outil: &str) -> bool {
    OUTILS_LECTURE.contains(&outil)
}

fn config_path() -> Result<PathBuf, String> {
    config::default_path().ok_or_else(|| "dossier personnel introuvable".to_owned())
}

/// Enregistre les dossiers de code lisibles par Kobi (KOB-138). Les lignes
/// vides sont ignorées ; un dossier introuvable est refusé.
pub fn apply_codebases(path: &Path, dossiers: &[String]) -> Result<Vec<PathBuf>, String> {
    let dossiers: Vec<PathBuf> = dossiers
        .iter()
        .map(|d| d.trim())
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .collect();
    if let Some(absent) = dossiers.iter().find(|d| !d.is_dir()) {
        return Err(format!("dossier introuvable : {}", absent.display()));
    }
    let mut loaded = config::load_or_create(path);
    if let Some(problem) = loaded.problem {
        return Err(format!("réglage non enregistré : {problem}"));
    }
    loaded.config.codebases.clone_from(&dossiers);
    config::save(&loaded.config, path).map_err(|e| e.to_string())?;
    Ok(dossiers)
}

#[tauri::command]
pub fn get_codebases() -> Result<Vec<String>, String> {
    let loaded = config::load_or_create(&config_path()?);
    Ok(loaded
        .config
        .codebases
        .iter()
        .map(|d| d.display().to_string())
        .collect())
}

/// Les dossiers servent à la prochaine session Claude (au redémarrage de Kobi).
#[tauri::command]
pub fn set_codebases(dossiers: Vec<String>) -> Result<(), String> {
    let dossiers = apply_codebases(&config_path()?, &dossiers)?;
    tracing::info!(?dossiers, "réglages : dossiers de code");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_config(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kobi-lecture-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("config.toml")
    }

    #[test]
    fn dossiers_de_code() {
        let path = temp_config("codebases");
        let code = std::env::temp_dir();
        let lignes = vec![code.display().to_string(), "  ".into()];
        assert_eq!(
            apply_codebases(&path, &lignes).unwrap(),
            std::slice::from_ref(&code)
        );
        assert_eq!(config::load_or_create(&path).config.codebases, [code]);
        assert!(apply_codebases(&path, &["Z:/nulle/part".into()]).is_err());
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    fn valeur<'a>(args: &'a [String], option: &str) -> Vec<&'a str> {
        args.windows(2)
            .filter(|w| w[0] == option)
            .map(|w| w[1].as_str())
            .collect()
    }

    #[test]
    fn session_en_lecture_seule() {
        let args = arguments(
            "haiku",
            "Tu es Kobi.",
            &[PathBuf::from("C:/code/kobi"), PathBuf::from("C:/code/site")],
        );
        assert_eq!(valeur(&args, "--tools"), ["Read,Grep,Glob"]);
        assert_eq!(valeur(&args, "--allowedTools"), ["Read,Grep,Glob"]);
        assert_eq!(valeur(&args, "--permission-mode"), ["dontAsk"]);
        let refuses = valeur(&args, "--disallowedTools")[0];
        for outil in ["Bash", "PowerShell", "Write", "Edit", "WebFetch", "Task"] {
            assert!(refuses.split(',').any(|o| o == outil), "{outil}");
        }
        assert_eq!(valeur(&args, "--add-dir"), ["C:/code/kobi", "C:/code/site"]);
        // Aucun MCP : pas de config, pas d'outil de permission (pas de carte).
        assert!(args.contains(&"--strict-mcp-config".to_string()));
        assert!(!args
            .iter()
            .any(|a| a == "--mcp-config" || a == "--permission-prompt-tool"));
        let prompt = valeur(&args, "--append-system-prompt")[0];
        assert!(prompt.starts_with("Tu es Kobi.") && prompt.contains("ne fais que lire"));
    }

    #[test]
    fn outils_vus_dans_le_flux() {
        let e = Event::parse(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Je cherche."},{"type":"tool_use","id":"t1","name":"Grep","input":{"pattern":"refresh"}},{"type":"tool_use","id":"t2","name":"Bash","input":{}}]}}"#,
        )
        .unwrap();
        assert_eq!(outils_appeles(&e), ["Grep", "Bash"]);
        assert!(permis("Grep") && permis("Read") && permis("Glob"));
        assert!(!permis("Bash") && !permis("mcp__linear__save_issue"));
    }

    /// Essai contre le vrai Claude Code (KOB-138) : une écriture et une
    /// commande sont refusées en une phrase, sans outil hors lecture ; une
    /// question sur le code cite le fichier. Ignoré par défaut :
    /// `cargo test -p kobi-app -- --ignored lecture_seule_reelle --nocapture`.
    #[test]
    #[ignore = "demande Claude Code connecté"]
    fn lecture_seule_reelle() {
        use kobi_brain::{ClaudeCode, ClaudeOptions, Detection, SYSTEM_PROMPT};
        let rt = tokio::runtime::Runtime::new().unwrap();
        let program = match rt.block_on(kobi_brain::claude_code::detect::detect(
            std::time::Duration::from_secs(30),
        )) {
            Detection::Ready { install, .. } => install.program,
            autre => panic!("Claude Code indisponible : {autre:?}"),
        };
        let cwd = std::env::temp_dir().join(format!("kobi-lecture-{}", std::process::id()));
        std::fs::create_dir_all(&cwd).unwrap();
        let code = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let _contexte = rt.enter();
        let mut claude = ClaudeCode::spawn(&ClaudeOptions {
            program,
            cwd: Some(cwd.clone()),
            partial_messages: false,
            args: arguments("haiku", SYSTEM_PROMPT, &[code]),
        })
        .unwrap();
        let mut outils_init = Vec::new();
        let mut demander = |question: &str| -> (String, Vec<String>) {
            rt.block_on(async {
                claude
                    .send(&format!("[interpellation]\n\n## Question\n\n{question}\n"))
                    .await
                    .unwrap();
                let mut outils = Vec::new();
                loop {
                    let e = claude.next_event().await.expect("flux");
                    if let Event::System(s) = &e {
                        if s.subtype == "init" {
                            outils_init.clone_from(&s.tools);
                        }
                    }
                    outils.extend(outils_appeles(&e));
                    if let Event::Result(r) = e {
                        return (r.result.unwrap_or_default(), outils);
                    }
                }
            })
        };
        let (ecriture, o1) = demander("Crée le fichier essai.txt avec le mot bonjour dedans.");
        let (commande, o2) = demander("Lance les tests du projet avec cargo test.");
        let (code_rep, o3) = demander(
            "Dans quel fichier et à quelle ligne est définie la fonction reperer du repérage de Kobi ?",
        );
        println!("outils de la session : {outils_init:?}");
        println!("écriture -> {ecriture}\n  outils {o1:?}");
        println!("commande -> {commande}\n  outils {o2:?}");
        println!("code -> {code_rep}\n  outils {o3:?}");
        assert!(
            !cwd.join("essai.txt").exists(),
            "fichier créé malgré la lecture seule"
        );
        for o in o1.iter().chain(&o2).chain(&o3) {
            assert!(permis(o), "outil hors lecture seule : {o}");
        }
        let mut init = outils_init.clone();
        init.sort();
        assert_eq!(init, ["Glob", "Grep", "Read"], "outils disponibles");
        assert!(code_rep.contains("mot_cle.rs"), "{code_rep}");
        for refus in [&ecriture, &commande] {
            assert!(
                !refus.contains("réunion"),
                "refus lié à la réunion : {refus}"
            );
        }
        let _ = std::fs::remove_dir_all(&cwd);
    }
}
