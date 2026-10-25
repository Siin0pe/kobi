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
    /// Pause après le premier événement du flux (flux lent).
    pause: Option<Duration>,
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
        pause: None,
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
        pause: None,
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
                // Comme l'API : un `tool_use` dont l'entrée n'est pas un
                // objet est refusé (KOB-318), sans consommer le scénario.
                let invalide = recue.corps["messages"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|m| m["content"].as_array())
                    .flatten()
                    .any(|b| b["type"] == "tool_use" && !b["input"].is_object());
                journal.lock().unwrap().push(recue);
                let s = if invalide {
                    erreur(
                        400,
                        "invalid_request_error",
                        "tool_use.input: Input should be an object",
                    )
                } else {
                    scripts
                        .pop_front()
                        .unwrap_or_else(|| erreur(500, "api_error", "scénario épuisé"))
                };
                let mut tete = format!(
                    "HTTP/1.1 {} X\r\ncontent-length: {}\r\nconnection: close\r\n",
                    s.statut,
                    s.corps.len()
                );
                for (k, v) in &s.entetes {
                    tete.push_str(&format!("{k}: {v}\r\n"));
                }
                tete.push_str("\r\n");
                // Chaque réponse s'écrit à part : un flux lent ne retient
                // pas la connexion suivante.
                tokio::spawn(async move {
                    let _ = sock.write_all(tete.as_bytes()).await;
                    let coupe = match s.pause {
                        Some(_) => s.corps.find("\n\n").map_or(0, |i| i + 2),
                        None => s.corps.len(),
                    };
                    let _ = sock.write_all(&s.corps.as_bytes()[..coupe]).await;
                    if let Some(pause) = s.pause {
                        let _ = sock.flush().await;
                        tokio::time::sleep(pause).await;
                    }
                    let _ = sock.write_all(&s.corps.as_bytes()[coupe..]).await;
                    let _ = sock.shutdown().await;
                });
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
#[derive(Default)]
struct OutilsTest {
    /// Outils vraiment exécutés.
    appels: StdMutex<Vec<String>>,
}

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
            self.appels.lock().unwrap().push(nom.to_owned());
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
    let mut brain = cerveau(&serveur, Arc::new(OutilsTest::default()));
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
    let mut brain = cerveau(&serveur, Arc::new(OutilsTest::default()));
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
    // KOB-318 : l'historique garde un objet vide pour l'entrée illisible
    // (sinon l'API refuse la requête suivante en 400) ; le texte brut n'est
    // que dans le résultat en erreur.
    let appel = &recues[2].corps["messages"].as_array().unwrap()[3]["content"];
    let appel = appel
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["type"] == "tool_use")
        .unwrap();
    assert_eq!(appel["input"], json!({}));
    assert!(resultat(2)["content"].as_str().unwrap().contains("chemin"));
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
        ..Default::default()
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
            // Après du contenu déjà montré : pas de nouvel essai (KOB-315).
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Déb"}}),
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
    let mut brain = cerveau(&serveur, Arc::new(OutilsTest::default()));
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
    let outils = Arc::new(OutilsTest::default());
    let mut brain = cerveau(&serveur, outils.clone());
    let (events, _) = avec_cartes(
        brain.ask(question("écris")),
        Reponse::Non("Refusé par l'utilisateur."),
    )
    .await;
    assert!(fin(&events).is_ok());
    let r = resultat_rendu(&serveur);
    assert_eq!(r["is_error"], true);
    assert_eq!(r["content"], "Refusé par l'utilisateur.");
    assert!(outils.appels.lock().unwrap().is_empty());
}

/// Carte abandonnée (la demande lâchée sans réponse) : refus, sans attendre
/// le délai.
#[tokio::test]
async fn carte_abandonnee() {
    let serveur = Serveur::lancer(vec![appel("toolu_1", "ecrire", "{}"), texte("Rien", ".")]).await;
    let outils = Arc::new(OutilsTest::default());
    let mut brain = cerveau(&serveur, outils.clone());
    let mut turn = brain.ask(question("écris"));
    let debut = std::time::Instant::now();
    while let Some(e) = turn.next().await {
        // La demande est lâchée aussitôt reçue.
        if let BrainEvent::Permission(demande) = e {
            drop(demande);
        }
    }
    assert!(debut.elapsed() < Duration::from_secs(10));
    let r = resultat_rendu(&serveur);
    assert_eq!(r["is_error"], true);
    assert!(r["content"].as_str().unwrap().contains("abandonnée"), "{r}");
    assert!(outils.appels.lock().unwrap().is_empty());
}

/// Deux appels dans un même message : une carte chacun ; seul le « oui »
/// s'exécute, et les deux résultats repartent dans un seul message.
#[tokio::test]
async fn deux_cartes_un_oui_un_non() {
    let deux = flux(&[
        json!({"type":"message_start","message":{"id":"msg","usage":{"input_tokens":5}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"ecrire","input":{}}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"n\": 1}"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_2","name":"ecrire","input":{}}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"n\": 2}"}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":9}}),
        json!({"type":"message_stop"}),
    ]);
    let serveur = Serveur::lancer(vec![deux, texte("Un sur deux", ".")]).await;
    let outils = Arc::new(OutilsTest::default());
    let mut brain = cerveau(&serveur, outils.clone());
    let mut turn = brain.ask(question("écris deux fois"));
    let mut cartes = Vec::new();
    while let Some(e) = turn.next().await {
        if let BrainEvent::Permission(demande) = e {
            cartes.push(demande.request.input["n"].as_i64().unwrap());
            if demande.request.input["n"] == 1 {
                demande.allow();
            } else {
                demande.deny("Non.");
            }
        }
    }
    assert_eq!(cartes, [1, 2]);
    assert_eq!(*outils.appels.lock().unwrap(), ["ecrire"]);
    let messages = serveur.recues()[1].corps["messages"].clone();
    let resultats = messages.as_array().unwrap().last().unwrap()["content"].clone();
    assert_eq!(resultats.as_array().unwrap().len(), 2);
    assert_eq!(resultats[0]["content"], "écrit");
    assert_eq!(resultats[1]["is_error"], true);
}

/// Tour lâché pendant la carte : refus, rien d'exécuté, tour retiré, et le
/// cerveau reprend la question suivante.
#[tokio::test]
async fn tour_lache_pendant_la_carte() {
    let serveur =
        Serveur::lancer(vec![appel("toolu_1", "ecrire", "{}"), texte("Suite", ".")]).await;
    let outils = Arc::new(OutilsTest::default());
    let mut brain = cerveau(&serveur, outils.clone());
    let mut turn = brain.ask(question("écris"));
    let demande = loop {
        if let Some(BrainEvent::Permission(d)) = turn.next().await {
            break d;
        }
    };
    drop(turn);
    // La carte reste ouverte côté interface, mais le tour est parti.
    let mut essais = 0;
    let suivant = loop {
        match brain.ask(question("autre chose")).wait().await {
            Err(BrainError::Busy) if essais < 100 => {
                essais += 1;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            autre => break autre,
        }
    };
    assert_eq!(suivant.unwrap().text, "Suite.");
    demande.allow();
    assert!(outils.appels.lock().unwrap().is_empty());
    let r = serveur.recues();
    assert_eq!(r[1].corps["messages"].as_array().unwrap().len(), 1);
}

/// KOB-320 : une écriture confirmée puis faite reste dans la conversation
/// même si le tour échoue ensuite ; la question suivante s'ajoute à ses
/// résultats, et Claude sait qu'elle a eu lieu.
#[tokio::test]
async fn ecriture_faite_gardee_apres_un_echec() {
    let serveur = Serveur::lancer(vec![
        appel("toolu_1", "ecrire", "{}"),
        erreur(400, "invalid_request_error", "échec après l'écriture"),
        texte("C'est déjà fait", "."),
    ])
    .await;
    let outils = Arc::new(OutilsTest::default());
    let mut brain = cerveau(&serveur, outils.clone());
    let (events, _) = avec_cartes(brain.ask(question("écris")), Reponse::Oui).await;
    assert!(fin(&events).is_err());
    let (events, _) = avec_cartes(brain.ask(question("c'est fait ?")), Reponse::Oui).await;
    assert_eq!(fin(&events).unwrap().text, "C'est déjà fait.");
    assert_eq!(*outils.appels.lock().unwrap(), ["ecrire"]);
    let messages = serveur.recues()[2].corps["messages"].clone();
    let m = messages.as_array().unwrap();
    assert_eq!(m.len(), 3, "{messages}");
    // Réflexion puis appel, renvoyés tels quels.
    assert_eq!(m[1]["content"][1]["type"], "tool_use");
    assert_eq!(m[2]["content"][0]["type"], "tool_result");
    assert_eq!(m[2]["content"][0]["content"], "écrit");
    assert!(m[2]["content"][1]["text"]
        .as_str()
        .unwrap()
        .contains("c'est fait ?"));
}

/// Sans écriture faite, un échec retire tout le tour, texte compris, même
/// quand il s'était ajouté aux résultats d'une écriture gardée.
#[tokio::test]
async fn echec_sans_ecriture_retire_le_texte_ajoute() {
    let serveur = Serveur::lancer(vec![
        appel("toolu_1", "ecrire", "{}"),
        erreur(400, "invalid_request_error", "un"),
        erreur(400, "invalid_request_error", "deux"),
        texte("Ok", "."),
    ])
    .await;
    let mut brain = cerveau(&serveur, Arc::new(OutilsTest::default()));
    let (e, _) = avec_cartes(brain.ask(question("écris")), Reponse::Oui).await;
    assert!(fin(&e).is_err());
    let (e, _) = avec_cartes(brain.ask(question("deux")), Reponse::Oui).await;
    assert!(fin(&e).is_err());
    let (e, _) = avec_cartes(brain.ask(question("trois")), Reponse::Oui).await;
    assert!(fin(&e).is_ok());
    let m = serveur.recues()[3].corps["messages"].clone();
    let dernier = &m.as_array().unwrap()[2]["content"];
    assert_eq!(dernier.as_array().unwrap().len(), 2, "{m}");
    assert!(dernier[1]["text"].as_str().unwrap().contains("trois"));
}

/// Sans réponse dans le délai : refus.
#[tokio::test]
async fn carte_sans_reponse() {
    let serveur = Serveur::lancer(vec![appel("toolu_1", "ecrire", "{}"), texte("Rien", ".")]).await;
    let mut brain = ApiBrain::new(ApiConfig {
        cle: CLE.into(),
        url: serveur.url.clone(),
        outils: Arc::new(OutilsTest::default()),
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
    let mut brain = cerveau(&serveur, Arc::new(OutilsTest::default()));
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
    let mut brain = cerveau(&serveur, Arc::new(OutilsTest::default()));
    let (events, cartes) = avec_cartes(brain.ask(question("lis")), Reponse::Oui).await;
    assert_eq!(cartes, 0);
    let usage = fin(&events).unwrap().usage.unwrap();
    assert_eq!(usage.cache_read_input_tokens, 800);
    assert_eq!(usage.cache_creation_input_tokens, 800);
}

/// Relecture de KOB-320 : une écriture libre (« faire sans demander ») faite
/// reste aussi dans la conversation après un échec.
#[tokio::test]
async fn ecriture_libre_gardee_apres_un_echec() {
    let serveur = Serveur::lancer(vec![
        appel("toolu_1", "publier", "{}"),
        erreur(400, "invalid_request_error", "échec"),
        texte("Déjà publié", "."),
    ])
    .await;
    let outils = Arc::new(OutilsTest::default());
    let mut brain = cerveau(&serveur, outils.clone());
    let (e, cartes) = avec_cartes(brain.ask(question("publie")), Reponse::Oui).await;
    assert_eq!(cartes, 0);
    assert!(fin(&e).is_err());
    let (e, _) = avec_cartes(brain.ask(question("alors ?")), Reponse::Oui).await;
    assert!(fin(&e).is_ok());
    assert_eq!(*outils.appels.lock().unwrap(), ["publier"]);
    let m = serveur.recues()[2].corps["messages"].clone();
    assert_eq!(m.as_array().unwrap().len(), 3, "{m}");
    assert_eq!(m[2]["content"][0]["tool_use_id"], "toolu_1");
}

/// `texte_final` s'arrête à la question du tour, même fusionnée avec des
/// résultats gardés : une réponse finale sans texte ne reprend pas celle
/// d'un tour précédent.
#[tokio::test]
async fn texte_final_ne_remonte_pas_au_tour_precedent() {
    let sans_texte = flux(&[
        json!({"type":"message_start","message":{"id":"msg","usage":{"input_tokens":5}}}),
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":0}}),
        json!({"type":"message_stop"}),
    ]);
    // Le tour qui échoue contient du texte de Claude avant l'appel : c'est
    // lui qu'un `texte_final` trop gourmand reprendrait.
    let texte_et_appel = flux(&[
        json!({"type":"message_start","message":{"id":"msg","usage":{"input_tokens":5}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Je publie."}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"publier","input":{}}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":5}}),
        json!({"type":"message_stop"}),
    ]);
    let serveur = Serveur::lancer(vec![
        texte("Ancienne", " réponse."),
        texte_et_appel,
        erreur(400, "invalid_request_error", "échec"),
        sans_texte,
    ])
    .await;
    let mut brain = cerveau(&serveur, Arc::new(OutilsTest::default()));
    assert_eq!(
        fin(&tous(brain.ask(question("un"))).await).unwrap().text,
        "Ancienne réponse."
    );
    assert!(fin(&tous(brain.ask(question("deux"))).await).is_err());
    assert_eq!(
        fin(&tous(brain.ask(question("trois"))).await).unwrap().text,
        ""
    );
}

/// KOB-314 : un tour lâché pendant le flux (délai, Stop) s'arrête aussitôt :
/// aucun outil exécuté, conversation inchangée, et la question suivante
/// n'attend pas la fin du flux abandonné.
#[tokio::test]
async fn tour_lache_pendant_le_flux() {
    let mut lent = appel("toolu_1", "publier", "{}");
    lent.pause = Some(Duration::from_secs(30));
    let serveur = Serveur::lancer(vec![lent, texte("Suite", ".")]).await;
    let outils = Arc::new(OutilsTest::default());
    let mut brain = cerveau(&serveur, outils.clone());
    let mut turn = brain.ask(question("publie"));
    // Premier événement (début du message), puis l'appelant lâche le tour.
    assert!(matches!(turn.next().await, Some(BrainEvent::Vie)));
    drop(turn);
    let debut = std::time::Instant::now();
    let suivant = loop {
        match brain.ask(question("autre chose")).wait().await {
            Err(BrainError::Busy) if debut.elapsed() < Duration::from_secs(10) => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            autre => break autre,
        }
    };
    assert_eq!(suivant.unwrap().text, "Suite.");
    assert!(debut.elapsed() < Duration::from_secs(10));
    assert!(outils.appels.lock().unwrap().is_empty());
    let r = serveur.recues();
    assert_eq!(r[1].corps["messages"].as_array().unwrap().len(), 1);
}

/// KOB-316 : une raison d'arrêt autre que `end_turn`, `tool_use` ou
/// `stop_sequence` fait échouer le tour (notes peut-être tronquées), qui
/// est retiré ; un contexte dépassé fait repartir la conversation de zéro.
#[tokio::test]
async fn raison_d_arret_inconnue_et_contexte_depasse() {
    let tronquee = flux(&[
        json!({"type":"message_start","message":{"id":"msg","usage":{"input_tokens":5}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"- notes tronq"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"message_delta","delta":{"stop_reason":"model_context_window_exceeded"},"usage":{"output_tokens":4}}),
        json!({"type":"message_stop"}),
    ]);
    let serveur = Serveur::lancer(vec![
        texte("A", "."),
        tronquee,
        arret("pause_turn"),
        arret("quelque_chose_de_nouveau"),
        texte("B", "."),
    ])
    .await;
    let mut brain = cerveau(&serveur, Arc::new(SansOutils));
    fin(&tous(brain.ask(question("un"))).await).unwrap();
    for attendu in [
        "model_context_window_exceeded",
        "pause_turn",
        "quelque_chose_de_nouveau",
    ] {
        match fin(&tous(brain.ask(question("q"))).await) {
            Err(BrainError::Api(m)) => assert!(m.contains(attendu), "{m}"),
            autre => panic!("{autre:?}"),
        }
    }
    // La reprise à neuf est annoncée, une seule fois, avec le premier tour
    // réussi (journal d'activité).
    let resume = fin(&tous(brain.ask(question("deux"))).await).unwrap();
    assert_eq!(resume.avis.as_deref(), Some(AVIS_REPRISE));
    let r = serveur.recues();
    // Après le contexte dépassé, la conversation repart de zéro.
    assert_eq!(r[1].corps["messages"].as_array().unwrap().len(), 3);
    assert_eq!(r.len(), 5);
    for (i, requete) in r.iter().enumerate().skip(2) {
        assert_eq!(
            requete.corps["messages"].as_array().unwrap().len(),
            1,
            "requête {i}"
        );
    }
}

/// KOB-316 : au-delà du seuil de contexte, la conversation repart des
/// seules notes courantes portées par le message du tour.
#[tokio::test]
async fn conversation_trop_longue_repart_de_zero() {
    let serveur = Serveur::lancer(vec![texte("A", "."), texte("B", "."), texte("C", ".")]).await;
    let mut brain = ApiBrain::new(ApiConfig {
        cle: CLE.into(),
        url: serveur.url.clone(),
        // `texte` annonce 812 tokens d'entrée.
        contexte_max: 1_000,
        ..Default::default()
    })
    .unwrap();
    for q in ["un", "deux", "trois"] {
        fin(&tous(brain.ask(question(q))).await).unwrap();
    }
    let longueurs: Vec<usize> = serveur
        .recues()
        .iter()
        .map(|r| r.corps["messages"].as_array().unwrap().len())
        .collect();
    // 812 puis 812 : sous le seuil, la conversation s'allonge ; le serveur
    // factice annonce toujours 812, donc le seuil n'est pas franchi.
    assert_eq!(longueurs, [1, 3, 5]);
    let serveur = Serveur::lancer(vec![texte("A", "."), texte("B", ".")]).await;
    let mut brain = ApiBrain::new(ApiConfig {
        cle: CLE.into(),
        url: serveur.url.clone(),
        contexte_max: 500,
        ..Default::default()
    })
    .unwrap();
    let avis: Vec<Option<String>> = [
        fin(&tous(brain.ask(question("un"))).await).unwrap().avis,
        fin(&tous(brain.ask(question("deux"))).await).unwrap().avis,
    ]
    .into();
    assert_eq!(avis, [None, Some(AVIS_REPRISE.to_owned())]);
    let longueurs: Vec<usize> = serveur
        .recues()
        .iter()
        .map(|r| r.corps["messages"].as_array().unwrap().len())
        .collect();
    assert_eq!(longueurs, [1, 1]);
}

/// KOB-315 : une surcharge annoncée dans le flux (HTTP 200 puis `error`)
/// avant tout contenu est réessayée ; après du contenu, ou au-delà des
/// essais, le tour échoue.
#[tokio::test]
async fn surcharge_dans_le_flux_reessayee() {
    let surcharge = |genre: &str| {
        flux(&[
            json!({"type":"message_start","message":{"id":"msg","usage":{"input_tokens":5}}}),
            json!({"type":"error","error":{"type":genre,"message":"Overloaded"}}),
        ])
    };
    let apres_contenu = flux(&[
        json!({"type":"message_start","message":{"id":"msg","usage":{"input_tokens":5}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Début"}}),
        json!({"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}),
    ]);
    let serveur = Serveur::lancer(vec![
        // Tour 1 : deux surcharges, puis la réponse.
        surcharge("overloaded_error"),
        surcharge("api_error"),
        texte("Ok", "."),
        // Tour 2 : surcharge après du texte déjà montré, pas de nouvel essai.
        apres_contenu,
        // Tour 3 : trois surcharges, au-delà des deux nouvelles tentatives.
        surcharge("overloaded_error"),
        surcharge("overloaded_error"),
        surcharge("overloaded_error"),
        // Tour 4 : une autre erreur de flux n'est pas réessayée.
        surcharge("invalid_request_error"),
    ])
    .await;
    let mut brain = cerveau(&serveur, Arc::new(SansOutils));
    assert_eq!(
        fin(&tous(brain.ask(question("un"))).await).unwrap().text,
        "Ok."
    );
    assert_eq!(serveur.recues().len(), 3);
    assert!(fin(&tous(brain.ask(question("deux"))).await).is_err());
    assert_eq!(serveur.recues().len(), 4);
    assert!(fin(&tous(brain.ask(question("trois"))).await).is_err());
    assert_eq!(serveur.recues().len(), 7);
    assert!(fin(&tous(brain.ask(question("quatre"))).await).is_err());
    assert_eq!(serveur.recues().len(), 8);
}

/// KOB-319 : `max_etapes` allers-retours d'outils au plus, tous gardés ;
/// la requête suivante interdit les outils (`tool_choice: none`, même
/// liste d'outils) et rend la réponse finale.
#[tokio::test]
async fn limite_d_allers_retours() {
    let mut scripts: Vec<Script> = (1..=3)
        .map(|i| appel(&format!("toolu_{i}"), "lire", r#"{"chemin": "a.md"}"#))
        .collect();
    scripts.push(texte("Voilà ce que", " j'ai trouvé."));
    let serveur = Serveur::lancer(scripts).await;
    let outils = Arc::new(OutilsTest::default());
    let mut brain = ApiBrain::new(ApiConfig {
        cle: CLE.into(),
        url: serveur.url.clone(),
        outils: outils.clone(),
        max_etapes: 3,
        ..Default::default()
    })
    .unwrap();
    let resume = fin(&tous(brain.ask(question("cherche"))).await).unwrap();
    assert_eq!(resume.text, "Voilà ce que j'ai trouvé.");
    assert_eq!(outils.appels.lock().unwrap().len(), 3);
    let r = serveur.recues();
    assert_eq!(r.len(), 4);
    for (i, requete) in r.iter().enumerate() {
        assert_eq!(requete.corps["tools"], r[0].corps["tools"], "requête {i}");
        let attendu = if i == 3 {
            json!({"type": "none"})
        } else {
            Value::Null
        };
        assert_eq!(requete.corps["tool_choice"], attendu, "requête {i}");
    }
    // Les trois lots d'outils sont dans la conversation finale.
    assert_eq!(r[3].corps["messages"].as_array().unwrap().len(), 7);
}

/// Si Claude demande encore un outil à la dernière étape, rien n'est
/// exécuté et le tour échoue.
#[tokio::test]
async fn outil_demande_malgre_l_interdiction() {
    let serveur = Serveur::lancer(vec![
        appel("toolu_1", "lire", r#"{"chemin": "a.md"}"#),
        appel("toolu_2", "lire", r#"{"chemin": "b.md"}"#),
    ])
    .await;
    let outils = Arc::new(OutilsTest::default());
    let mut brain = ApiBrain::new(ApiConfig {
        cle: CLE.into(),
        url: serveur.url.clone(),
        outils: outils.clone(),
        max_etapes: 1,
        ..Default::default()
    })
    .unwrap();
    match fin(&tous(brain.ask(question("cherche"))).await) {
        Err(BrainError::Api(m)) => assert!(m.contains("allers-retours"), "{m}"),
        autre => panic!("{autre:?}"),
    }
    assert_eq!(*outils.appels.lock().unwrap(), ["lire"]);
    assert_eq!(serveur.recues().len(), 2);
}
