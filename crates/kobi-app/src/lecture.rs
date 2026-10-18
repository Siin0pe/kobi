//! Kobi en lecture seule (KOB-138) : pour l'instant, Claude ne fait que lire
//! et chercher dans les dossiers de code déclarés (`codebases` de
//! `config.toml`). Ni écriture, ni commande, ni outil web, ni MCP : la
//! session n'a que Read, Grep et Glob, et refuse le reste sans carte.
//!
//! Voir « Kobi — parcours utilisateur et règles d'UX », principe 6.

use std::path::{Path, PathBuf};

use kobi_session::config;

#[cfg(test)]
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

/// Mémoire des réunions passées (KOB-76) : les dossiers que le stockage
/// remplit, donnés à Claude en lecture comme les codebases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Memoire {
    pub reunions: PathBuf,
    pub conversations: PathBuf,
}

impl Memoire {
    pub fn de(stockage: &kobi_session::stockage::Stockage) -> Self {
        Self {
            reunions: stockage.dossier_reunions(),
            conversations: stockage.dossier_conversations(),
        }
    }

    /// Dossiers qui existent, pour `--add-dir`.
    fn dossiers(&self) -> impl Iterator<Item = &PathBuf> {
        [&self.reunions, &self.conversations]
            .into_iter()
            .filter(|d| d.is_dir())
    }

    /// Ajouté au prompt système : où chercher, dans quel ordre, et comment
    /// citer la réunion.
    pub fn prompt(&self) -> String {
        format!(
            "\
## Réunions et conversations passées

Les réunions passées sont dans `{reunions}` : un dossier par réunion, nommé \
`AAAA-MM-JJ-titre` (la date, puis le titre), avec `compte-rendu.md` (résumé, décisions, \
actions, questions ouvertes), `notes.md`, `transcript.jsonl` (ce qui a été dit, ligne par \
ligne) et `reunion.json` (titre exact, heure de début). Les conversations avec toi sont \
dans `{conversations}`, un fichier par conversation (`AAAA-MM-JJ/HHMMSS.jsonl`).

- Pour une question sur le passé (« qu'avait-on décidé sur… », « la dernière fois… », \
« qui devait… »), cherche avec Grep dans ces dossiers : d'abord dans les `compte-rendu.md`, \
puis dans les `notes.md`, et seulement ensuite dans les `transcript.jsonl` et les conversations.
- Cite toujours la réunion : sa date et son titre, d'après le nom du dossier ou \
`reunion.json` (« Le 14 octobre, au point budget, vous avez décidé … »). Plusieurs \
réunions en parlent : donne la plus récente et dis qu'il y en a d'autres.
- Si rien n'en parle, dis-le en une phrase, sans inventer.",
            reunions = self.reunions.display(),
            conversations = self.conversations.display(),
        )
    }
}

/// Arguments de `claude` (après ceux du mode stream-json) pour une session
/// en lecture seule sur `codebases`, et sur la mémoire des réunions si elle
/// est donnée (KOB-76).
pub fn arguments(
    modele: &str,
    systeme: &str,
    codebases: &[PathBuf],
    memoire: Option<&Memoire>,
) -> Vec<String> {
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
    let dossiers_memoire = memoire.into_iter().flat_map(Memoire::dossiers);
    for dossier in codebases.iter().chain(dossiers_memoire) {
        args.push("--add-dir".into());
        args.push(dossier.display().to_string());
    }
    let mut prompt = format!("{systeme}\n\n{PROMPT_LECTURE}");
    if let Some(m) = memoire {
        prompt.push_str("\n\n");
        prompt.push_str(&m.prompt());
    }
    args.push("--append-system-prompt".into());
    args.push(prompt);
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

/// Outils appelés dans un message `assistant` du flux (l'app les voit par
/// `BrainEvent::ToolCall`, voir [`crate::cerveau`]).
#[cfg(test)]
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
            None,
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

    /// Mémoire des réunions : les dossiers du stockage s'ajoutent aux
    /// codebases, et le prompt dit où chercher et comment citer.
    #[test]
    fn memoire_des_reunions_donnee_en_lecture() {
        let dir = tempfile::tempdir().unwrap();
        let st = kobi_session::stockage::Stockage::new(dir.path());
        st.preparer().unwrap();
        let memoire = Memoire::de(&st);
        let args = arguments(
            "haiku",
            "Tu es Kobi.",
            &[PathBuf::from("/code/kobi")],
            Some(&memoire),
        );
        assert_eq!(
            valeur(&args, "--add-dir"),
            [
                "/code/kobi".to_owned(),
                dir.path().join("reunions").display().to_string(),
                dir.path().join("conversations").display().to_string(),
            ]
        );
        // Toujours en lecture seule.
        assert_eq!(valeur(&args, "--tools"), ["Read,Grep,Glob"]);
        let prompt = valeur(&args, "--append-system-prompt")[0];
        assert!(prompt.contains("ne fais que lire"));
        assert!(prompt.contains(&dir.path().join("reunions").display().to_string()));
        let (cr, notes, transcript) = (
            prompt.find("d'abord dans les `compte-rendu.md`").unwrap(),
            prompt.find("puis dans les `notes.md`").unwrap(),
            prompt
                .find("`transcript.jsonl` et les conversations")
                .unwrap(),
        );
        assert!(cr < notes && notes < transcript, "ordre de recherche");
        assert!(prompt.contains("Cite toujours la réunion"));
        // Dossier absent : pas d'--add-dir vers un chemin qui n'existe pas.
        let absent = Memoire {
            reunions: dir.path().join("rien"),
            conversations: dir.path().join("rien-non-plus"),
        };
        let args = arguments("haiku", "x", &[], Some(&absent));
        assert!(valeur(&args, "--add-dir").is_empty());
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
            args: arguments("haiku", SYSTEM_PROMPT, &[code], None),
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

    /// Trois réunions d'essai, rangées par le stockage comme en vrai.
    fn reunions_d_essai(st: &kobi_session::stockage::Stockage) {
        use kobi_stt::{Segment, Source};
        // 6, 13 et 15 octobre 2026, 10 h (heure de Paris).
        let reunions = [
            (
                1_791_273_600_000_u64,
                "Point budget",
                "## Résumé\nBudget de l'automne passé en revue.\n\n## Décisions\n- Le budget du séminaire d'équipe est validé à 8 000 euros.\n- Les frais de déplacement passent par le nouvel outil.\n\n## Actions\n- [ ] Sophie : réserver la salle à Annecy\n\n## Questions ouvertes\n- Faut-il inviter les stagiaires ?\n",
                "Pour le séminaire, on part sur huit mille euros, c'est validé.",
            ),
            (
                1_791_878_400_000,
                "Revue technique",
                "## Résumé\nRevue de l'authentification.\n\n## Décisions\n- Le refresh token passe à 30 jours, au lieu de 7.\n- L'authentification migre vers OAuth avant décembre.\n\n## Actions\n- [ ] Thomas : mettre à jour auth/token.rs\n",
                "On garde le refresh token trente jours, c'est décidé.",
            ),
            (
                1_792_051_200_000,
                "Comité produit",
                "## Résumé\nCalendrier de la bêta.\n\n## Décisions\n- Le lancement de la bêta est repoussé au 3 novembre.\n\n## Questions ouvertes\n- Qui fera la démonstration au client ?\n",
                "La bêta sort le trois novembre finalement.",
            ),
        ];
        for (debut, titre, cr, phrase) in reunions {
            let mut r = st.nouvelle_reunion(debut, Some(titre)).unwrap();
            r.ecrire_segment(&Segment {
                start: 12.0,
                end: 16.0,
                source: Source::Eux,
                text: phrase.into(),
                is_final: true,
            })
            .unwrap();
            r.ecrire_notes(&cr.replace("## Résumé", "## Notes"))
                .unwrap();
            r.ecrire_compte_rendu(cr).unwrap();
            r.terminer(debut + 45 * 60_000).unwrap();
        }
    }

    /// Essai réel contre Claude Code (connecté) : « qu'avait-on décidé sur
    /// X ? » trouve la bonne réunion et la cite, en lecture seule.
    /// `cargo test -p kobi-app -- --ignored memoire_reelle --nocapture`.
    #[test]
    #[ignore = "demande Claude Code connecté"]
    fn memoire_reelle() {
        use kobi_brain::{ClaudeCode, ClaudeOptions, Detection, SYSTEM_PROMPT};
        let rt = tokio::runtime::Runtime::new().unwrap();
        let program = match rt.block_on(kobi_brain::claude_code::detect::detect(
            std::time::Duration::from_secs(30),
        )) {
            Detection::Ready { install, .. } => install.program,
            autre => panic!("Claude Code indisponible : {autre:?}"),
        };
        let dir = tempfile::tempdir().unwrap();
        let st = kobi_session::stockage::Stockage::new(dir.path().join("Kobi"));
        st.preparer().unwrap();
        reunions_d_essai(&st);
        // Le dossier de travail n'est pas le stockage : seul --add-dir y donne accès.
        let cwd = dir.path().join("travail");
        std::fs::create_dir_all(&cwd).unwrap();
        let _contexte = rt.enter();
        let mut claude = ClaudeCode::spawn(&ClaudeOptions {
            program,
            cwd: Some(cwd),
            partial_messages: false,
            args: arguments("haiku", SYSTEM_PROMPT, &[], Some(&Memoire::de(&st))),
        })
        .unwrap();
        let mut demander = |question: &str| -> (String, Vec<String>) {
            rt.block_on(async {
                claude
                    .send(&format!("[interpellation]\n\n## Question\n\n{question}\n"))
                    .await
                    .unwrap();
                let mut outils = Vec::new();
                loop {
                    let e = claude.next_event().await.expect("flux");
                    outils.extend(outils_appeles(&e));
                    if let Event::Result(r) = e {
                        return (r.result.unwrap_or_default(), outils);
                    }
                }
            })
        };
        let cas: [(&str, &[&str], &[&str]); 3] = [
            (
                "Qu'avait-on décidé sur le refresh token ?",
                &["30"],
                &["13", "revue technique"],
            ),
            (
                "Qu'avait-on décidé pour le budget du séminaire ?",
                &["8 000", "8000", "huit mille"],
                &["6 ", "point budget"],
            ),
            (
                "C'était quand déjà, le lancement de la bêta ?",
                &["3 novembre"],
                &["15", "comité produit"],
            ),
        ];
        for (question, faits, reunion) in cas {
            let (reponse, outils) = demander(question);
            println!("Q : {question}\nR : {reponse}\n  outils {outils:?}\n");
            let r = reponse.to_lowercase();
            assert!(
                faits.iter().any(|f| r.contains(f)),
                "fait absent : {reponse}"
            );
            assert!(
                reunion.iter().any(|m| r.contains(m)),
                "réunion non citée : {reponse}"
            );
            for o in &outils {
                assert!(permis(o), "outil hors lecture seule : {o}");
            }
        }
    }
}
