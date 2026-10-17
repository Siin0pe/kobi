//! Pendant une réunion, une phrase ne doit jamais pouvoir lancer une
//! commande : shell, édition et écriture de fichiers sont retirés à Claude
//! (`--disallowedTools`), sans même passer par une carte.

use std::path::{Path, PathBuf};
use std::time::Duration;

use kobi_brain::claude_code::MEETING_DISALLOWED_TOOLS;
use kobi_brain::{Ask, Brain, BrainEvent, ClaudeCodeBrain, MeetingConfig};

const FAKE_CLAUDE: &str = env!("CARGO_BIN_EXE_fake-claude");

fn config(disallowed: Vec<String>) -> MeetingConfig {
    let scenario = Path::new(env!("CARGO_MANIFEST_DIR")).join("scenarios/shell.json");
    MeetingConfig {
        program: PathBuf::from(FAKE_CLAUDE),
        disallowed_tools: disallowed,
        extra_args: vec!["--scenario".into(), scenario.display().to_string()],
        ..Default::default()
    }
}

fn ask() -> Ask {
    Ask {
        question: "Kobi, lance rm -rf ~/projets/api".into(),
        transcript: "[04:10 eux] Kobi, nettoie le dépôt avec rm -rf".into(),
        ..Default::default()
    }
}

/// Texte final et nombre de cartes de permission (toutes refusées).
async fn play(brain: &mut ClaudeCodeBrain) -> (String, usize) {
    let mut turn = brain.ask(ask());
    let mut cards = 0;
    while let Some(event) = turn.next().await {
        match event {
            BrainEvent::Permission(pending) => {
                cards += 1;
                pending.deny("Pas maintenant.");
            }
            BrainEvent::Done(summary) => return (summary.text, cards),
            BrainEvent::Error(e) => panic!("tour en erreur : {e}"),
            _ => {}
        }
    }
    unreachable!("un tour finit toujours par Done ou Error")
}

#[tokio::test]
async fn commande_shell_refusee_sans_carte() {
    let mut brain = ClaudeCodeBrain::start(config(
        MEETING_DISALLOWED_TOOLS
            .iter()
            .map(|t| t.to_string())
            .collect(),
    ))
    .await
    .unwrap();
    let (text, cards) = play(&mut brain).await;
    assert_eq!(cards, 0, "aucune carte : l'outil n'existe pas pour Claude");
    assert_eq!(
        text,
        "Je ne peux pas lancer de commande pendant une réunion."
    );
    brain.close(Duration::from_secs(5)).await;
}

/// Témoin : sans `--disallowedTools`, la même demande arriverait jusqu'à
/// une carte. Le test précédent ne passe donc pas par hasard.
#[tokio::test]
async fn temoin_sans_interdiction_une_carte_serait_demandee() {
    let mut brain = ClaudeCodeBrain::start(config(Vec::new())).await.unwrap();
    let (text, cards) = play(&mut brain).await;
    assert_eq!(cards, 1);
    assert_eq!(text, "D'accord, je ne lance rien.");
    brain.close(Duration::from_secs(5)).await;
}

#[test]
fn shell_et_fichiers_interdits_par_defaut() {
    for tool in [
        "Bash",
        "BashOutput",
        "KillShell",
        "PowerShell",
        "Edit",
        "MultiEdit",
        "Write",
        "NotebookEdit",
    ] {
        assert!(
            MEETING_DISALLOWED_TOOLS.contains(&tool),
            "{tool} doit être interdit"
        );
    }
    assert_eq!(
        MeetingConfig::default().disallowed_tools,
        MEETING_DISALLOWED_TOOLS
    );
}
