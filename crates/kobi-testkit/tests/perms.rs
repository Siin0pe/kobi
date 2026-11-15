//! Serveur de permission « kobi » de bout en bout, avec le faux `claude` :
//! une écriture attend la réponse de l'utilisateur à la carte.

use std::path::{Path, PathBuf};
use std::time::Duration;

use kobi_brain::{
    Ask, Brain, BrainEvent, ClaudeCodeBrain, Decision, MeetingConfig, NotesUpdate, Turn,
};
use kobi_perms::Cards;
use serde_json::{json, Map};

const FAKE_CLAUDE: &str = env!("CARGO_BIN_EXE_fake-claude");
const FAKE_MCP: &str = env!("CARGO_BIN_EXE_fake-mcp");

fn work_dir(test: &str) -> kobi_testkit::DossierTemp {
    kobi_testkit::DossierTemp::nouveau("perms", test)
}

async fn start(dir: &Path, timeout: Duration) -> ClaudeCodeBrain {
    start_avec(dir, timeout, "permission.json").await
}

async fn start_avec(dir: &Path, timeout: Duration, scenario: &str) -> ClaudeCodeBrain {
    let mut servers = Map::new();
    servers.insert(
        "notes".into(),
        json!({ "command": FAKE_MCP, "args": ["--journal", dir.join("journal.txt")] }),
    );
    let scenario = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("scenarios")
        .join(scenario);
    ClaudeCodeBrain::start(MeetingConfig {
        program: PathBuf::from(FAKE_CLAUDE),
        mcp_servers: servers,
        permission_timeout: timeout,
        extra_args: vec![
            "--scenario".into(),
            scenario.display().to_string(),
            "--fake-attente".into(),
            temoin(dir).display().to_string(),
        ],
        ..Default::default()
    })
    .await
    .unwrap()
}

/// Lit le tour jusqu'à la demande de permission et ouvre la carte.
async fn until_card(turn: &mut Turn, cards: &Cards) -> u64 {
    loop {
        match turn.next().await.expect("le tour continue") {
            BrainEvent::Permission(pending) => return cards.open(pending).id,
            BrainEvent::Done(_) | BrainEvent::Error(_) => panic!("fin de tour sans carte"),
            _ => {}
        }
    }
}

/// Fichier témoin où le faux `claude` écrit l'appel qui attend sa
/// permission, juste avant de s'y bloquer (`--fake-attente`, KOB-533).
fn temoin(dir: &Path) -> PathBuf {
    dir.join("attente.txt")
}

/// Le scénario `permission.json`, lu tel quel.
fn scenario_permission() -> serde_json::Value {
    let chemin = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("scenarios")
        .join("permission.json");
    serde_json::from_str(&std::fs::read_to_string(chemin).unwrap()).unwrap()
}

/// Ce que le faux `claude` écrit avant de demander la permission, lu dans le
/// scénario jusqu'à son repère `{"fake": "permission"}` : les identifiants des
/// appels d'outils (`tool_use`) et ceux des résultats (`tool_result`). Rien
/// n'est recopié à la main : si le scénario change, le test suit (KOB-530).
fn avant_la_carte(scenario: &serde_json::Value) -> (Vec<String>, Vec<String>) {
    let (mut appels, mut rendus) = (Vec::new(), Vec::new());
    for ligne in scenario["turns"][0]["lines"].as_array().unwrap() {
        if ligne["fake"] == "permission" {
            return (appels, rendus);
        }
        for bloc in ligne["message"]["content"].as_array().into_iter().flatten() {
            match bloc["type"].as_str() {
                Some("tool_use") => appels.push(bloc["id"].as_str().unwrap().to_owned()),
                Some("tool_result") => {
                    rendus.push(bloc["tool_use_id"].as_str().unwrap().to_owned())
                }
                _ => {}
            }
        }
    }
    panic!("pas de repère de permission dans le scénario");
}

fn ask() -> Ask {
    Ask {
        question: "note qu'il faut envoyer le devis".into(),
        ..Default::default()
    }
}

fn journal(dir: &Path) -> Option<String> {
    std::fs::read_to_string(dir.join("journal.txt")).ok()
}

/// Joue un tour du scénario `scenario` jusqu'à la carte de l'écriture,
/// attend que le faux `claude` soit bloqué dans l'attente de la réponse
/// (fichier témoin, sans durée fixe), vérifie que rien n'est écrit, accepte,
/// puis lit le tour jusqu'au bout. Rend l'événement qui a fait avancer le
/// tour sans réponse, s'il y en a un, sinon le texte de la réponse.
///
/// La carte vient du serveur de permission, le reste du flux de la sortie du
/// programme : sous charge, la carte double des événements écrits avant elle
/// (KOB-511, sonde #481 : `Vie` sous Windows). L'ordre de la sortie, lui, est
/// sûr : ce que le faux `claude` écrit pendant l'attente arrive avant ce qu'il
/// écrit après la réponse. Jusqu'au résultat de l'écriture, seuls sont donc
/// admis la carte, `Vie`, et les appels et résultats d'outils du scénario
/// d'avant la carte : l'appel de l'écriture elle-même, jamais son résultat, un
/// autre outil, un texte ou une fin de tour (KOB-530, KOB-533), à n'importe
/// quel moment avant la réponse.
async fn tour_avec_carte(dir: &Path, scenario: &serde_json::Value) -> Result<String, String> {
    let (appels, rendus) = avant_la_carte(scenario);
    let ecriture = appels.last().unwrap().clone();
    assert!(!rendus.contains(&ecriture), "{appels:?} {rendus:?}");
    let fichier = dir.join("scenario.json");
    std::fs::write(&fichier, scenario.to_string()).unwrap();
    let cards = Cards::new(Duration::from_secs(60));
    let mut brain = start_avec(dir, Duration::from_secs(60), fichier.to_str().unwrap()).await;
    let mut turn = brain.ask(ask());
    let mut carte = false;
    let mut ecrit = false;
    let mut erreur: Option<String> = None;
    let resultat = loop {
        let evenement = turn.next().await;
        // Déjà en faute : le tour est lu jusqu'au bout, la carte refusée si
        // elle arrive, pour ne pas laisser le faux `claude` bloqué.
        if let Some(e) = &erreur {
            match evenement {
                Some(BrainEvent::Permission(pending)) => {
                    let id = cards.open(pending).id;
                    cards.answer(id, Decision::Deny("Fin de l'essai.".into()));
                }
                Some(BrainEvent::Done(_) | BrainEvent::Error(_)) | None => break Err(e.clone()),
                Some(_) => {}
            }
            continue;
        }
        match evenement {
            Some(BrainEvent::Permission(pending)) if !carte => {
                carte = true;
                let id = cards.open(pending).id;
                let card = &cards.pending()[0];
                assert_eq!(
                    (card.server.as_deref(), card.tool.as_str()),
                    (Some("notes"), "write_note")
                );
                assert_eq!(card.input["text"], "Il faut envoyer le devis au client.");
                attendre_le_temoin(dir, &ecriture).await;
                // Bloqué dans l'attente : rien n'est écrit.
                assert_eq!(journal(dir), None);
                assert!(cards.answer(id, Decision::Allow));
            }
            Some(BrainEvent::OutilRendu { ref id }) if carte && *id == ecriture => ecrit = true,
            Some(BrainEvent::Done(summary)) if ecrit => break Ok(summary.text),
            _ if ecrit => {}
            Some(BrainEvent::Vie) => {}
            Some(BrainEvent::ToolCall { ref id, .. }) if appels.contains(id) => {}
            Some(BrainEvent::OutilRendu { ref id }) if rendus.contains(id) => {}
            Some(BrainEvent::Done(_) | BrainEvent::Error(_)) | None if !carte => {
                break Err("fin de tour sans carte".into())
            }
            autre => erreur = Some(format!("le tour avance sans réponse : {autre:?}")),
        }
    };
    brain.close(Duration::from_secs(5)).await;
    resultat
}

/// Attend que le faux `claude` signale qu'il est bloqué dans l'attente de la
/// permission de `appel` (le délai n'est qu'un filet si le signal manque).
async fn attendre_le_temoin(dir: &Path, appel: &str) {
    let filet = tokio::time::Instant::now() + Duration::from_secs(30);
    while std::fs::read_to_string(temoin(dir)).ok().as_deref() != Some(appel) {
        assert!(
            tokio::time::Instant::now() < filet,
            "le faux claude n'a pas signalé son attente"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Le scénario `permission.json`, avec `lignes` écrites par le faux `claude`
/// pendant qu'il attend la réponse à la carte (`pendant`, KOB-533).
fn permission_avec_pendant(lignes: Vec<serde_json::Value>) -> serde_json::Value {
    let mut scenario = scenario_permission();
    let ligne = scenario["turns"][0]["lines"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|l| l["fake"] == "permission")
        .unwrap();
    ligne["pendant"] = lignes.into();
    scenario
}

#[tokio::test]
async fn une_ecriture_attend_la_reponse() {
    let dir = work_dir("attend");
    let texte = tour_avec_carte(&dir, &scenario_permission()).await.unwrap();
    assert!(texte.contains("C'est noté"));
    assert_eq!(
        journal(&dir).as_deref(),
        Some("- Il faut envoyer le devis au client.\n")
    );
}

/// KOB-533 : un autre outil appelé pendant la carte (appel complet : blocs du
/// flux et message assistant, ceux de l'écriture sous un autre identifiant)
/// fait échouer la vérification. Pas `ToolSearch` : outil interne, le cerveau
/// ne le montre pas (`INTERNAL_TOOLS`), c'est pourquoi l'essai de KOB-530
/// n'arrivait pas jusqu'au test.
#[tokio::test]
async fn un_autre_outil_pendant_la_carte_est_vu() {
    let dir = work_dir("autre-outil");
    let scenario = scenario_permission();
    let lignes = scenario["turns"][0]["lines"].as_array().unwrap();
    let outil = lignes.iter().find(|l| l["fake"] == "permission").unwrap()["tool"].clone();
    let debut = lignes
        .iter()
        .position(|l| l["event"]["content_block"]["name"] == outil)
        .unwrap();
    let fin = debut
        + lignes[debut..]
            .iter()
            .position(|l| l["event"]["type"] == "content_block_stop")
            .unwrap();
    let ancien = lignes[debut]["event"]["content_block"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let appel: Vec<serde_json::Value> = lignes[debut..=fin]
        .iter()
        .map(|l| {
            serde_json::from_str(&l.to_string().replace(&ancien, "toolu_pendant_la_carte")).unwrap()
        })
        .collect();
    let erreur = tour_avec_carte(&dir, &permission_avec_pendant(appel))
        .await
        .unwrap_err();
    assert!(erreur.contains("toolu_pendant_la_carte"), "{erreur}");
}

/// KOB-533 : une fin de tour écrite pendant l'attente, plus d'une seconde
/// après la carte, fait échouer la vérification (plus de fenêtre fixe).
#[tokio::test]
async fn une_fin_tardive_pendant_la_carte_est_vue() {
    let dir = work_dir("fin-tardive");
    let scenario = scenario_permission();
    let suite = scenario["turns"][0]["lines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["fake"] == "permission")
        .unwrap()["allow"]
        .as_array()
        .unwrap()
        .clone();
    // La suite « accepté », sans le résultat de l'écriture, 1,5 s après.
    let mut lignes = vec![json!({"fake": "sleep", "ms": 1500})];
    lignes.extend(suite.into_iter().skip(1));
    let erreur = tour_avec_carte(&dir, &permission_avec_pendant(lignes))
        .await
        .unwrap_err();
    assert!(erreur.contains("avance sans réponse"), "{erreur}");
}

/// KOB-72 : en notes continues, le message de notes demande les
/// propositions (le faux `claude` attend la consigne) ; l'action proposée
/// ouvre une carte, et rien n'est écrit avant le oui.
#[tokio::test]
async fn notes_en_continu_une_proposition_ouvre_une_carte() {
    let dir = work_dir("notes-continu");
    let cards = Cards::new(Duration::from_secs(60));
    let mut brain = start_avec(&dir, Duration::from_secs(60), "notes-continu.json").await;
    let mut turn = brain.update_notes(NotesUpdate {
        transcript: "[00:10 moi] Il faut envoyer le devis au client.".into(),
        notes: String::new(),
        propositions: true,
    });
    let id = until_card(&mut turn, &cards).await;
    assert_eq!(cards.pending()[0].tool, "write_note");
    assert_eq!(journal(&dir), None);
    assert!(cards.answer(id, Decision::Allow));
    turn.wait().await.unwrap();
    assert_eq!(
        journal(&dir).as_deref(),
        Some("- Il faut envoyer le devis au client.\n")
    );
    brain.close(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn pas_maintenant() {
    let dir = work_dir("refus");
    let cards = Cards::new(Duration::from_secs(60));
    let mut brain = start(&dir, Duration::from_secs(60)).await;
    let mut turn = brain.ask(ask());
    let id = until_card(&mut turn, &cards).await;
    assert!(cards.answer(id, Decision::Deny("Pas maintenant.".into())));
    assert!(turn.wait().await.unwrap().text.contains("je n'ajoute rien"));
    assert_eq!(journal(&dir), None);
    brain.close(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn carte_expiree_refus_automatique() {
    let dir = work_dir("expiree");
    let cards = Cards::new(Duration::from_millis(300));
    let mut brain = start(&dir, Duration::from_millis(300)).await;
    let mut turn = brain.ask(ask());
    let id = until_card(&mut turn, &cards).await;
    // Sans réponse dans le délai, le serveur refuse et le tour se termine.
    assert!(turn.wait().await.unwrap().text.contains("je n'ajoute rien"));
    assert!(cards.pending().is_empty());
    assert!(!cards.answer(id, Decision::Allow), "clic trop tard");
    assert_eq!(journal(&dir), None);
    brain.close(Duration::from_secs(5)).await;
}
