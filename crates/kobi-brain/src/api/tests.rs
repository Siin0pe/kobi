//! Tests du backend clé API contre un faux serveur HTTP : chaque connexion
//! reçoit la réponse suivante du scénario, et chaque requête est gardée.

use std::collections::VecDeque;
use std::sync::Mutex as StdMutex;

use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::*;

const CLE: &str = "sk-ant-api03-cle-de-test-0000";

/// Requête reçue par le faux serveur.
#[derive(Debug, Clone)]
struct Recue {
    entetes: Vec<(String, String)>,
    corps: Value,
}

impl Recue {
    fn entete(&self, nom: &str) -> Option<&str> {
        self.entetes
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(nom))
            .map(|(_, v)| v.as_str())
    }
}

/// Réponse scriptée : statut, en-têtes en plus, corps.
struct Script {
    statut: u16,
    entetes: Vec<(&'static str, String)>,
    corps: String,
}

fn flux(evenements: &[Value]) -> Script {
    let mut corps = String::new();
    for e in evenements {
        corps.push_str(&format!(
            "event: {}\ndata: {}\n\n",
            e["type"].as_str().unwrap(),
            e
        ));
    }
    Script {
        statut: 200,
        entetes: vec![("content-type", "text/event-stream".into())],
        corps,
    }
}

fn erreur(statut: u16, genre: &str, message: &str) -> Script {
    Script {
        statut,
        entetes: vec![
            ("content-type", "application/json".into()),
            ("retry-after", "0".into()),
        ],
        corps: json!({"type":"error","error":{"type":genre,"message":message}}).to_string(),
    }
}

/// Réponse en texte seul, coupée en deux morceaux.
fn texte(a: &str, b: &str) -> Script {
    flux(&[
        json!({"type":"message_start","message":{"id":"msg","type":"message","role":"assistant","content":[],"usage":{"input_tokens":12,"cache_creation_input_tokens":800,"cache_read_input_tokens":0,"output_tokens":1}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig-texte"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"ping"}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":a}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":b}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":20}}),
        json!({"type":"message_stop"}),
    ])
}

/// Réponse qui appelle un outil.
fn appel(id: &str, nom: &str, entree: &str) -> Script {
    flux(&[
        json!({"type":"message_start","message":{"id":"msg","usage":{"input_tokens":5,"cache_read_input_tokens":800,"output_tokens":1}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig-outil"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":id,"name":nom,"input":{}}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":entree}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":30}}),
        json!({"type":"message_stop"}),
    ])
}

fn arret(raison: &str) -> Script {
    flux(&[
        json!({"type":"message_start","message":{"id":"msg","usage":{"input_tokens":5,"output_tokens":0}}}),
        json!({"type":"message_delta","delta":{"stop_reason":raison},"usage":{"output_tokens":0}}),
        json!({"type":"message_stop"}),
    ])
}

/// Faux serveur de l'API Messages.
struct Serveur {
    url: String,
    recues: Arc<StdMutex<Vec<Recue>>>,
}

impl Serveur {
    async fn lancer(scripts: Vec<Script>) -> Self {
        let ecoute = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", ecoute.local_addr().unwrap());
        let recues = Arc::new(StdMutex::new(Vec::new()));
        let journal = recues.clone();
        let mut scripts: VecDeque<Script> = scripts.into();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = ecoute.accept().await {
                let Some(recue) = lire_requete(&mut sock).await else {
                    continue;
                };
                journal.lock().unwrap().push(recue);
                let s = scripts
                    .pop_front()
                    .unwrap_or_else(|| erreur(500, "api_error", "scénario épuisé"));
                let mut tete = format!(
                    "HTTP/1.1 {} X\r\ncontent-length: {}\r\nconnection: close\r\n",
                    s.statut,
                    s.corps.len()
                );
                for (k, v) in &s.entetes {
                    tete.push_str(&format!("{k}: {v}\r\n"));
                }
                tete.push_str("\r\n");
                let _ = sock.write_all(tete.as_bytes()).await;
                let _ = sock.write_all(s.corps.as_bytes()).await;
                let _ = sock.shutdown().await;
            }
        });
        Self { url, recues }
    }

    fn recues(&self) -> Vec<Recue> {
        self.recues.lock().unwrap().clone()
    }
}

async fn lire_requete(sock: &mut tokio::net::TcpStream) -> Option<Recue> {
    let mut brut = Vec::new();
    let mut buf = [0u8; 4096];
    let fin_tete = loop {
        let n = sock.read(&mut buf).await.ok()?;
        if n == 0 {
            return None;
        }
        brut.extend_from_slice(&buf[..n]);
        if let Some(i) = brut.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
    };
    let tete = String::from_utf8_lossy(&brut[..fin_tete]).to_string();
    let entetes: Vec<(String, String)> = tete
        .lines()
        .skip(1)
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    let longueur: usize = entetes
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut corps = brut[fin_tete + 4..].to_vec();
    while corps.len() < longueur {
        let n = sock.read(&mut buf).await.ok()?;
        if n == 0 {
            break;
        }
        corps.extend_from_slice(&buf[..n]);
    }
    Some(Recue {
        entetes,
        corps: serde_json::from_slice(&corps).unwrap_or(Value::Null),
    })
}

/// Outil de test : `lire` rend le contenu d'un faux fichier, `casse`
/// échoue.
struct OutilsTest;

impl Outillage for OutilsTest {
    fn definitions(&self) -> Vec<Value> {
        vec![json!({
            "name": "lire",
            "description": "Lit un fichier de la codebase.",
            "input_schema": {
                "type": "object",
                "properties": { "chemin": { "type": "string" } },
                "required": ["chemin"],
            },
        })]
    }

    fn action(&self, nom: &str, entree: &Value) -> Action {
        match nom {
            "lire" => Action::LireFichier {
                chemin: entree["chemin"].as_str().unwrap_or_default().into(),
            },
            _ => Action::Autre {
                nom: nom.into(),
                entree: entree.clone(),
            },
        }
    }

    fn autorisation(&self, nom: &str) -> Autorisation {
        match nom {
            "ecrire" => Autorisation::Carte,
            "interdit" => Autorisation::Refus,
            _ => Autorisation::Libre,
        }
    }

    fn appeler<'a>(
        &'a self,
        nom: &'a str,
        entree: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(async move {
            match nom {
                "lire" => Ok(format!("contenu de {}", entree["chemin"].as_str().unwrap())),
                "ecrire" => Ok("écrit".into()),
                _ => Err("échec de l'outil".into()),
            }
        })
    }
}

fn cerveau(serveur: &Serveur, outils: Arc<dyn Outillage>) -> ApiBrain {
    ApiBrain::new(ApiConfig {
        cle: CLE.into(),
        url: serveur.url.clone(),
        outils,
        ..Default::default()
    })
    .unwrap()
}

fn question(q: &str) -> Ask {
    Ask {
        question: q.into(),
        ..Default::default()
    }
}

/// Tous les événements d'un tour.
async fn tous(mut turn: Turn) -> Vec<BrainEvent> {
    let mut v = Vec::new();
    while let Some(e) = turn.next().await {
        v.push(e);
    }
    v
}

fn fin(events: &[BrainEvent]) -> Result<TurnSummary, BrainError> {
    match events.last() {
        Some(BrainEvent::Done(s)) => Ok(s.clone()),
        Some(BrainEvent::Error(e)) => Err(e.clone()),
        autre => panic!("pas de fin : {autre:?}"),
    }
}

/// KOB-101 : réponse en flux et requête conforme à l'API (en-têtes,
/// modèle par défaut, cache sur le prompt système et sur la conversation,
/// effort, repli côté serveur).
#[tokio::test]
async fn reponse_en_flux_et_requete_conforme() {
    let serveur = Serveur::lancer(vec![texte("Il est ", "midi.")]).await;
    let mut brain = cerveau(&serveur, Arc::new(SansOutils));
    assert_eq!(brain.name(), "api");
    let events = tous(brain.ask(question("quelle heure ?"))).await;
    let morceaux: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            BrainEvent::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(morceaux, ["Il est ", "midi."]);
    let resume = fin(&events).unwrap();
    assert_eq!(resume.text, "Il est midi.");
    let usage = resume.usage.unwrap();
    assert_eq!(usage.cache_creation_input_tokens, 800);
    assert_eq!(usage.output_tokens, 20);

    let r = &serveur.recues()[0];
    assert_eq!(r.entete("x-api-key"), Some(CLE));
    assert_eq!(r.entete("anthropic-version"), Some("2026-10-21"));
    assert_eq!(
        r.entete("anthropic-beta"),
        Some("server-side-fallback-2026-07-01")
    );
    let c = &r.corps;
    assert_eq!(c["model"], "claude-sonnet-5-5");
    assert_eq!(c["stream"], true);
    assert_eq!(c["fallbacks"], "default");
    assert_eq!(c["output_config"]["effort"], "low");
    assert_eq!(c["cache_control"]["type"], "ephemeral");
    assert_eq!(c["system"][0]["text"], SYSTEM_PROMPT);
    assert_eq!(c["system"][0]["cache_control"]["type"], "ephemeral");
    // Ni outils vides, ni réglage de réflexion (adaptative par défaut),
    // ni paramètre d'échantillonnage (refusés par l'API 2026-10-21).
    for absent in ["tools", "thinking", "temperature", "top_p", "top_k"] {
        assert!(c.get(absent).is_none(), "{absent}");
    }
    let texte = c["messages"][0]["content"][0]["text"].as_str().unwrap();
    assert!(texte.starts_with("[interpellation]"), "{texte}");
    assert!(texte.contains("quelle heure ?"));
}

/// KOB-101 : boucle d'outils jusqu'à la réponse finale. L'appel est
/// annoncé avec son action normalisée, le résultat rendu dans un seul
/// message, la réflexion renvoyée telle quelle, la liste d'outils identique
/// d'une requête à l'autre.
#[tokio::test]
async fn boucle_d_outils_jusqu_a_la_reponse() {
    let serveur = Serveur::lancer(vec![
        appel("toolu_1", "lire", r#"{"chemin": "README.md"}"#),
        texte("Le README ", "parle de Kobi."),
    ])
    .await;
    let mut brain = cerveau(&serveur, Arc::new(OutilsTest));
    let events = tous(brain.ask(question("que dit le README ?"))).await;
    assert!(events.iter().any(|e| matches!(e,
        BrainEvent::ToolCall { id, name, action: Action::LireFichier { chemin }, .. }
            if id == "toolu_1" && name == "lire" && chemin.to_str() == Some("README.md"))));
    assert!(events
        .iter()
        .any(|e| matches!(e, BrainEvent::OutilRendu { id } if id == "toolu_1")));
    let resume = fin(&events).unwrap();
    assert_eq!(resume.text, "Le README parle de Kobi.");
    assert_eq!(resume.usage.unwrap().output_tokens, 50);

    let recues = serveur.recues();
    assert_eq!(recues.len(), 2);
    assert_eq!(recues[0].corps["tools"], recues[1].corps["tools"]);
    assert_eq!(recues[0].corps["tools"][0]["name"], "lire");
    let messages = recues[1].corps["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0], recues[0].corps["messages"][0]);
    assert_eq!(
        messages[1],
        json!({"role":"assistant","content":[
            {"type":"thinking","thinking":"","signature":"sig-outil"},
            {"type":"tool_use","id":"toolu_1","name":"lire","input":{"chemin":"README.md"}},
        ]})
    );
    assert_eq!(
        messages[2],
        json!({"role":"user","content":[
            {"type":"tool_result","tool_use_id":"toolu_1","content":"contenu de README.md"},
        ]})
    );
}

/// Un outil en échec, ou une entrée illisible, revient à Claude comme un
/// résultat en erreur ; le tour continue.
#[tokio::test]
async fn outil_en_echec_rendu_en_erreur() {
    let serveur = Serveur::lancer(vec![
        appel("toolu_1", "casse", "{}"),
        appel("toolu_2", "lire", r#"{"chemin": "#),
        texte("Je n'ai pas pu ", "lire."),
    ])
    .await;
    let mut brain = cerveau(&serveur, Arc::new(OutilsTest));
    assert!(fin(&tous(brain.ask(question("lis"))).await).is_ok());
    let recues = serveur.recues();
    let resultat = |i: usize| {
        recues[i].corps["messages"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["content"][0]
            .clone()
    };
    assert_eq!(resultat(1)["is_error"], true);
    assert_eq!(resultat(1)["content"], "échec de l'outil");
    assert_eq!(resultat(2)["is_error"], true);
    assert!(resultat(2)["content"]
        .as_str()
        .unwrap()
        .contains("INVALID_JSON"));
}

/// La conversation de la réunion ne fait que s'allonger : la deuxième
/// requête reprend la première mot pour mot (cache et réflexion valides).
#[tokio::test]
async fn conversation_seulement_allongee() {
    let serveur = Serveur::lancer(vec![texte("A", "."), texte("B", ".")]).await;
    let mut brain = cerveau(&serveur, Arc::new(SansOutils));
    fin(&tous(brain.ask(question("un"))).await).unwrap();
    let notes = NotesUpdate {
        transcript: "[00:10 moi] bonjour".into(),
        notes: "- rien".into(),
    };
    assert_eq!(
        fin(&tous(brain.update_notes(notes)).await).unwrap().text,
        "B."
    );
    let recues = serveur.recues();
    let (m1, m2) = (
        recues[0].corps["messages"].as_array().unwrap(),
        recues[1].corps["messages"].as_array().unwrap(),
    );
    assert_eq!(m2.len(), 3);
    assert_eq!(m2[0], m1[0]);
    assert_eq!(
        m2[1],
        json!({"role":"assistant","content":[
            {"type":"thinking","thinking":"","signature":"sig-texte"},
            {"type":"text","text":"A."},
        ]})
    );
    assert!(m2[2]["content"][0]["text"]
        .as_str()
        .unwrap()
        .starts_with("[notes]"));
    assert_eq!(recues[0].corps["system"], recues[1].corps["system"]);
}

/// Un refus, une réponse coupée ou une erreur dans le flux font échouer
/// le tour, qui est retiré de la conversation.
#[tokio::test]
async fn tour_en_echec_retire_de_la_conversation() {
    let serveur = Serveur::lancer(vec![
        arret("refusal"),
        arret("max_tokens"),
        flux(&[
            json!({"type":"message_start","message":{"id":"msg","usage":{"input_tokens":5}}}),
            json!({"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}),
        ]),
        texte("Enfin", "."),
    ])
    .await;
    let mut brain = cerveau(&serveur, Arc::new(SansOutils));
    for attendu in ["refusé", "coupée", "overloaded_error"] {
        match fin(&tous(brain.ask(question("q"))).await) {
            Err(BrainError::Api(m)) => assert!(m.contains(attendu), "{m}"),
            autre => panic!("{autre:?}"),
        }
    }
    fin(&tous(brain.ask(question("q"))).await).unwrap();
    for r in serveur.recues() {
        assert_eq!(r.corps["messages"].as_array().unwrap().len(), 1);
    }
}

/// Erreurs HTTP : une surcharge passagère est réessayée ; une clé refusée
/// rend le cerveau indisponible, sans jamais citer la clé.
#[tokio::test]
async fn erreurs_http() {
    let serveur = Serveur::lancer(vec![
        erreur(529, "overloaded_error", "Overloaded"),
        texte("Ok", "."),
        erreur(401, "authentication_error", "invalid x-api-key"),
        erreur(400, "invalid_request_error", "model: inconnu"),
    ])
    .await;
    let mut brain = cerveau(&serveur, Arc::new(SansOutils));
    assert_eq!(
        fin(&tous(brain.ask(question("q"))).await).unwrap().text,
        "Ok."
    );
    match fin(&tous(brain.ask(question("q"))).await) {
        Err(BrainError::Unavailable(m)) => {
            assert!(m.contains("invalid x-api-key"), "{m}");
            assert!(!m.contains(CLE));
        }
        autre => panic!("{autre:?}"),
    }
    match fin(&tous(brain.ask(question("q"))).await) {
        Err(BrainError::Api(m)) => assert!(m.contains("400") && m.contains("inconnu"), "{m}"),
        autre => panic!("{autre:?}"),
    }
    assert_eq!(serveur.recues().len(), 4);
}

/// Un seul tour à la fois.
#[tokio::test]
async fn un_seul_tour_a_la_fois() {
    let serveur = Serveur::lancer(vec![texte("A", ".")]).await;
    let mut brain = cerveau(&serveur, Arc::new(SansOutils));
    let premier = brain.ask(question("un"));
    assert_eq!(
        brain.ask(question("deux")).wait().await,
        Err(BrainError::Busy)
    );
    premier.wait().await.unwrap();
}

/// Opus au choix ; un modèle sans repli côté serveur n'envoie ni
/// `fallbacks` ni l'en-tête bêta.
#[tokio::test]
async fn modele_au_choix() {
    let serveur = Serveur::lancer(vec![texte("A", "."), texte("B", ".")]).await;
    for modele in ["claude-opus-5-5", "claude-haiku-5-5"] {
        let mut brain = ApiBrain::new(ApiConfig {
            cle: CLE.into(),
            url: serveur.url.clone(),
            modele: modele.into(),
            ..Default::default()
        })
        .unwrap();
        fin(&tous(brain.ask(question("q"))).await).unwrap();
    }
    let recues = serveur.recues();
    assert_eq!(recues[0].corps["model"], "claude-opus-5-5");
    assert_eq!(recues[0].corps["fallbacks"], "default");
    assert_eq!(recues[1].corps["model"], "claude-haiku-5-5");
    assert!(recues[1].corps.get("fallbacks").is_none());
    assert!(recues[1].entete("anthropic-beta").is_none());
    assert!(MODELES.contains(&MODELE_PAR_DEFAUT));
}

/// Sans clé, pas de cerveau ; la clé ne s'affiche pas en débogage.
#[test]
fn cle_absente_ou_masquee() {
    assert!(matches!(
        ApiBrain::new(ApiConfig::default()),
        Err(BrainError::Unavailable(_))
    ));
    let config = ApiConfig {
        cle: CLE.into(),
        ..Default::default()
    };
    let debug = format!("{config:?}");
    assert!(
        !debug.contains(CLE) && debug.contains("<masquée>"),
        "{debug}"
    );
}

/// Réponse donnée à chaque carte d'un tour.
enum Reponse {
    Oui,
    Non(&'static str),
    Rien,
}

/// Joue le tour en répondant aux cartes ; rend les événements et le nombre
/// de cartes vues.
async fn avec_cartes(mut turn: Turn, reponse: Reponse) -> (Vec<BrainEvent>, usize) {
    let (mut events, mut cartes, mut gardees) = (Vec::new(), 0, Vec::new());
    while let Some(e) = turn.next().await {
        match e {
            BrainEvent::Permission(demande) => {
                cartes += 1;
                assert_eq!(demande.request.tool_name, "ecrire");
                assert_eq!(demande.request.tool_use_id.as_deref(), Some("toolu_1"));
                match reponse {
                    Reponse::Oui => demande.allow(),
                    Reponse::Non(raison) => demande.deny(raison),
                    Reponse::Rien => gardees.push(demande),
                }
            }
            autre => events.push(autre),
        }
    }
    (events, cartes)
}

fn resultat_rendu(serveur: &Serveur) -> Value {
    let r = serveur.recues();
    r[1].corps["messages"].as_array().unwrap().last().unwrap()["content"][0].clone()
}

/// KOB-103 : un outil qui écrit attend la carte ; accepté, il s'exécute.
#[tokio::test]
async fn carte_acceptee() {
    let serveur = Serveur::lancer(vec![appel("toolu_1", "ecrire", "{}"), texte("Fait", ".")]).await;
    let mut brain = cerveau(&serveur, Arc::new(OutilsTest));
    let (events, cartes) = avec_cartes(brain.ask(question("écris")), Reponse::Oui).await;
    assert_eq!(cartes, 1);
    assert_eq!(fin(&events).unwrap().text, "Fait.");
    assert_eq!(
        resultat_rendu(&serveur),
        json!({"type":"tool_result","tool_use_id":"toolu_1","content":"écrit"})
    );
}

/// Refusée : l'outil ne s'exécute pas et Claude reçoit le refus.
#[tokio::test]
async fn carte_refusee() {
    let serveur = Serveur::lancer(vec![
        appel("toolu_1", "ecrire", "{}"),
        texte("D'accord", "."),
    ])
    .await;
    let mut brain = cerveau(&serveur, Arc::new(OutilsTest));
    let (events, _) = avec_cartes(
        brain.ask(question("écris")),
        Reponse::Non("Refusé par l'utilisateur."),
    )
    .await;
    assert!(fin(&events).is_ok());
    let r = resultat_rendu(&serveur);
    assert_eq!(r["is_error"], true);
    assert_eq!(r["content"], "Refusé par l'utilisateur.");
}

/// Sans réponse dans le délai : refus.
#[tokio::test]
async fn carte_sans_reponse() {
    let serveur = Serveur::lancer(vec![appel("toolu_1", "ecrire", "{}"), texte("Rien", ".")]).await;
    let mut brain = ApiBrain::new(ApiConfig {
        cle: CLE.into(),
        url: serveur.url.clone(),
        outils: Arc::new(OutilsTest),
        delai_carte: Duration::from_millis(50),
        ..Default::default()
    })
    .unwrap();
    let (events, cartes) = avec_cartes(brain.ask(question("écris")), Reponse::Rien).await;
    assert_eq!(cartes, 1);
    assert!(fin(&events).is_ok());
    let r = resultat_rendu(&serveur);
    assert_eq!(r["is_error"], true);
    assert!(
        r["content"]
            .as_str()
            .unwrap()
            .contains("Pas de confirmation"),
        "{r}"
    );
}

/// Un outil interdit dans la session est refusé sans carte ni appel.
#[tokio::test]
async fn outil_interdit_sans_carte() {
    let serveur =
        Serveur::lancer(vec![appel("toolu_1", "interdit", "{}"), texte("Non", ".")]).await;
    let mut brain = cerveau(&serveur, Arc::new(OutilsTest));
    let (events, cartes) = avec_cartes(brain.ask(question("fais")), Reponse::Oui).await;
    assert_eq!(cartes, 0);
    assert!(fin(&events).is_ok());
    let r = resultat_rendu(&serveur);
    assert_eq!(r["is_error"], true);
    assert!(r["content"].as_str().unwrap().contains("pas permis"), "{r}");
}

/// Une lecture passe sans carte ; les tokens en cache sont rendus.
#[tokio::test]
async fn lecture_sans_carte_et_cache_visible() {
    let serveur = Serveur::lancer(vec![
        appel("toolu_1", "lire", r#"{"chemin": "a.md"}"#),
        texte("Lu", "."),
    ])
    .await;
    let mut brain = cerveau(&serveur, Arc::new(OutilsTest));
    let (events, cartes) = avec_cartes(brain.ask(question("lis")), Reponse::Oui).await;
    assert_eq!(cartes, 0);
    let usage = fin(&events).unwrap().usage.unwrap();
    assert_eq!(usage.cache_read_input_tokens, 800);
    assert_eq!(usage.cache_creation_input_tokens, 800);
}
