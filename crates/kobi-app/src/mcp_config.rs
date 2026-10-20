//! Config MCP des sessions de l'agent de Kobi : une seule fonction,
//! [`assembler`], partagée entre KOB-161 (le MCP de Kobi, toujours présent)
//! et KOB-155 (les serveurs de l'utilisateur et le serveur de permission).
//!
//! Le JSON va dans un fichier passé par `--mcp-config` (avec
//! `--strict-mcp-config`) : en ligne de commande, les guillemets se perdent
//! en passant par `claude.cmd` sous Windows. Il ne contient aucun secret.

use std::io;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

/// Nom du MCP de Kobi dans les sessions : ses outils s'appellent
/// `mcp__kobi__<outil>`.
pub const SERVEUR_KOBI: &str = "kobi";

/// Ajouté au prompt système quand le MCP de Kobi est branché.
pub const PROMPT_MCP_KOBI: &str = "\
## Tes propres outils

Les outils `mcp__kobi__*` sont les tiens : ton état, tes réunions enregistrées (notes, \
comptes rendus, transcriptions), tes conversations, tes rappels et tes réglages. \
Sers-t'en pour répondre à « qu'est-ce qu'on a décidé hier ? », « quels sont mes rappels ? », \
« que disent mes notes ? ». Les outils marqués [lecture] et [affichage] passent tout de suite, \
comme démarrer, mettre en pause ou arrêter la réunion. Ceux marqués [écriture] (changer un réglage, \
ajouter un rappel…) demandent d'abord un « oui » à l'utilisateur : Kobi lui montre une carte et attend \
jusqu'à une minute. Appelle simplement l'outil, sans demander l'accord en texte ; s'il est refusé, \
dis-le en une phrase et ne réessaie pas. Ne dis jamais qu'un réglage a changé sans avoir appelé \
l'outil et reçu sa réponse : répondre à voix haute ou en texte seul, par exemple, c'est le réglage \
`voice.assistant` (`voice.meeting` pendant une réunion). Quand tu prends des notes ou écris un compte rendu, ne les \
appelle pas : tout ce qu'il te faut est dans le message, et l'état de la réunion y change encore.";

/// Ajouté au prompt système de la conversation (KOB-165) : c'est Claude qui
/// démarre et arrête l'enregistrement, d'après la demande et la transcription.
pub const PROMPT_PILOTAGE_REUNION: &str = "\
## Piloter la réunion

C'est toi qui démarres, mets en pause, reprends et arrêtes l'enregistrement de la réunion, \
avec `mcp__kobi__meeting_start`, `mcp__kobi__meeting_pause`, `mcp__kobi__meeting_resume` et \
`mcp__kobi__meeting_stop`. Pas de confirmation : Kobi agit tout de suite et l'annonce. La fin \
est immédiate (le compte rendu se prépare) et s'annule pendant 10 secondes avec « Annuler » ou \
« reprends » (`meeting_resume`).

- Appelle l'outil dès que la question te le demande, quelle que soit la formulation. \
La question vient de la reconnaissance vocale, qui écorche les mots (« mais faire \
l'enregistrement » pour « mets fin à l'enregistrement », « Kobe », « Koby » ou « Cobi » pour \
« Kobi ») : appuie-toi sur la transcription récente du message pour comprendre.
- Adressés à toi, « on a fini la réunion », « fin de réunion », « termine », « coupe », \
« mets fin à l'enregistrement », « tu peux arrêter » veulent dire `meeting_stop` ; \
« enregistre », « prends des notes de la réunion » veulent dire `meeting_start` ; \
« pause » et « reprends » veulent dire `meeting_pause` et `meeting_resume`.
- Jamais sur une simple mention : « on en reparle à la fin de la réunion », « la réunion \
finit à midi », « l'enregistrement du client », une question sur la réunion ou ce qu'un \
participant dit à un autre ne sont pas des ordres. Dans le doute, n'appelle rien et demande \
en une phrase : « Tu veux que j'arrête l'enregistrement ? ».
- Après l'appel, réponds en une phrase : le message rendu par l'outil, tel quel.
- Ces quatre outils passent sans carte ; les autres outils qui écrivent demandent un « oui » \
à l'utilisateur.";

/// Un serveur MCP d'une session.
#[derive(Debug, Clone, PartialEq)]
pub struct ServeurMcp {
    pub nom: String,
    /// Entrée de `mcpServers` : `{"command", "args"}` (stdio) ou
    /// `{"type": "http", "url", "headers"}`.
    pub config: Value,
    /// Noms complets (`mcp__<nom>__<outil>`) ajoutés à `--allowedTools`.
    pub outils_autorises: Vec<String>,
}

/// Config MCP prête pour une session.
#[derive(Debug, Clone, PartialEq)]
pub struct McpSession {
    pub fichier: PathBuf,
    pub outils_autorises: Vec<String>,
    /// Outil de `--permission-prompt-tool` (serveur de permission de Kobi,
    /// KOB-155) ; absent : ce qui n'est pas autorisé d'avance est refusé.
    pub permission_prompt_tool: Option<String>,
    /// Prompt à ajouter pour les serveurs de l'utilisateur (KOB-155).
    pub prompt: Option<String>,
}

/// Le MCP de Kobi, servi par l'exécutable `exe` en mode agent (lecture
/// seule en attendant la validation de KOB-156).
pub fn serveur_kobi(exe: &Path) -> ServeurMcp {
    ServeurMcp {
        nom: SERVEUR_KOBI.into(),
        config: json!({
            "type": "stdio",
            "command": exe.display().to_string(),
            "args": ["mcp", "--agent"],
        }),
        outils_autorises: kobi_ctl::mcp::outils_de_l_agent(SERVEUR_KOBI),
    }
}

/// Assemble `{"mcpServers": {…}}` : le MCP de Kobi (toujours présent, servi
/// par l'exécutable en cours) puis `utilisateur` (KOB-155), et l'écrit dans
/// `fichier`. Un serveur de l'utilisateur nommé « kobi » est ignoré.
pub fn assembler(fichier: &Path, utilisateur: Vec<ServeurMcp>) -> io::Result<McpSession> {
    let exe = std::env::current_exe()?;
    assembler_avec(fichier, serveur_kobi(&exe), utilisateur)
}

pub(crate) fn assembler_avec(
    fichier: &Path,
    kobi: ServeurMcp,
    utilisateur: Vec<ServeurMcp>,
) -> io::Result<McpSession> {
    let mut serveurs = Map::new();
    let mut outils = Vec::new();
    for s in std::iter::once(kobi).chain(utilisateur.into_iter().filter(|s| {
        let reserve = s.nom == SERVEUR_KOBI;
        if reserve {
            tracing::warn!(nom = %s.nom, "serveur MCP de l'utilisateur ignoré : nom réservé à Kobi");
        }
        !reserve
    })) {
        outils.extend(s.outils_autorises);
        serveurs.insert(s.nom, s.config);
    }
    if let Some(dossier) = fichier.parent() {
        std::fs::create_dir_all(dossier)?;
    }
    let texte = serde_json::to_string_pretty(&json!({ "mcpServers": serveurs }))
        .map_err(io::Error::other)?;
    std::fs::write(fichier, texte)?;
    Ok(McpSession {
        fichier: fichier.to_owned(),
        outils_autorises: outils,
        permission_prompt_tool: None,
        prompt: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kobi_toujours_present() {
        let dir = tempfile::tempdir().unwrap();
        let fichier = dir.path().join("mcp").join("session-conversation.json");
        let linear = ServeurMcp {
            nom: "linear".into(),
            config: json!({"type": "http", "url": "https://mcp.linear.app/mcp"}),
            outils_autorises: vec!["mcp__linear__list_issues".into()],
        };
        let imposteur = ServeurMcp {
            nom: "kobi".into(),
            config: json!({"command": "ailleurs"}),
            outils_autorises: vec!["mcp__kobi__config_set".into()],
        };
        let session = assembler_avec(
            &fichier,
            serveur_kobi(Path::new("C:/Program Files/Kobi/kobi.exe")),
            vec![linear, imposteur],
        )
        .unwrap();
        let ecrit: Value =
            serde_json::from_str(&std::fs::read_to_string(&fichier).unwrap()).unwrap();
        let serveurs = ecrit["mcpServers"].as_object().unwrap();
        assert_eq!(serveurs.len(), 2);
        assert_eq!(
            serveurs["kobi"]["command"],
            "C:/Program Files/Kobi/kobi.exe"
        );
        assert_eq!(serveurs["kobi"]["args"], json!(["mcp", "--agent"]));
        assert_eq!(serveurs["linear"]["url"], "https://mcp.linear.app/mcp");
        assert_eq!(session.fichier, fichier);
        assert!(session
            .outils_autorises
            .contains(&"mcp__kobi__notes_last".into()));
        assert!(session
            .outils_autorises
            .contains(&"mcp__linear__list_issues".into()));
        // Les outils d'écriture de Kobi passent le harness : c'est Kobi qui
        // demande la validation de l'utilisateur (KOB-156).
        for o in [
            "mcp__kobi__config_set",
            "mcp__kobi__meeting_start",
            "mcp__kobi__app_quit",
        ] {
            assert!(session.outils_autorises.contains(&o.to_string()), "{o}");
        }
    }
}
