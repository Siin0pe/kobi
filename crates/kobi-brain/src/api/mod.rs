//! Backend « clé API » (KOB-101) : repli pour qui n'a pas Claude Code. Pas
//! d'Agent SDK en Rust : la boucle agent est écrite ici, sur l'API Messages
//! (`POST /v1/messages`, en flux SSE).
//!
//! Une réunion = une conversation gardée en mémoire et seulement allongée :
//! chaque appel ajoute un message utilisateur, puis les réponses de Claude
//! et les résultats d'outils jusqu'à la réponse finale. Les blocs de Claude
//! (réflexion et signature comprises) sont renvoyés tels quels, sans jamais
//! réécrire le début : c'est ce qui garde le cache de prompt et la
//! réflexion valides d'un appel à l'autre. Un tour qui échoue est retiré
//! en entier.
//!
//! Cache : un point d'arrêt explicite sur le prompt système, plus le cache
//! automatique (`cache_control` au niveau de la requête) qui suit la fin de
//! la conversation.

mod flux;

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::{mpsc, Mutex};

use crate::brain::{
    Ask, Brain, BrainError, BrainEvent, NotesUpdate, ReportRequest, Turn, TurnSummary,
};
use crate::claude_code::{texte_ask, texte_compte_rendu, texte_notes, Usage, SYSTEM_PROMPT};
use crate::outils::Action;
use kobi_perms::{ApprovalRequest, Decision, PermissionRequest};

use flux::{a_renvoyer, entree_illisible, Apport, Decoupeur, Reponse};

/// Modèle par défaut.
pub const MODELE_PAR_DEFAUT: &str = "claude-sonnet-5-5";
/// Modèles proposés : Sonnet par défaut, Opus au choix.
pub const MODELES: &[&str] = &["claude-sonnet-5-5", "claude-opus-5-5"];
/// Adresse de l'API Anthropic.
pub const URL_API: &str = "https://api.anthropic.com";
/// Version de l'API (en-tête `anthropic-version`).
const VERSION_API: &str = "2026-10-21";
/// Repli côté serveur quand un classifieur de sécurité refuse
/// (`fallbacks: "default"`).
const BETA_REPLI: &str = "server-side-fallback-2026-07-01";
/// Modèles qui acceptent `fallbacks: "default"` sur l'API Anthropic.
const MODELES_AVEC_REPLI: &[&str] = &["claude-sonnet-5-5", "claude-opus-5-5"];
/// Statuts HTTP réessayés (limite, surcharge, panne passagère).
const STATUTS_REESSAYES: &[u16] = &[429, 500, 502, 503, 504, 529];

/// Outils proposés à Claude par le backend clé API (lecture MCP, KOB-102).
pub trait Outillage: Send + Sync {
    /// Définitions envoyées à l'API (`name`, `description`,
    /// `input_schema`). Lues une fois à l'ouverture : la liste ne change
    /// plus pendant la réunion (cache et réflexion en dépendent).
    fn definitions(&self) -> Vec<Value>;

    /// Traduction normalisée d'un appel, pour les règles de Kobi.
    fn action(&self, nom: &str, entree: &Value) -> Action {
        Action::Autre {
            nom: nom.to_owned(),
            entree: entree.clone(),
        }
    }

    /// Ce que demande l'appel avant de s'exécuter (mêmes règles que le
    /// backend Claude Code, KOB-320). Par prudence : une carte.
    fn autorisation(&self, _nom: &str) -> Autorisation {
        Autorisation::Carte
    }

    /// Exécute l'appel ; `Err` est rendu à Claude comme un résultat en
    /// erreur.
    fn appeler<'a>(
        &'a self,
        nom: &'a str,
        entree: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>>;
}

/// Ce qu'un appel d'outil demande avant de s'exécuter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Autorisation {
    /// Lecture, ou écriture d'un serveur réglé « faire sans demander ».
    Libre,
    /// Carte de confirmation : l'utilisateur dit oui ou non.
    Carte,
    /// Interdit dans cette session (question différée en lecture seule),
    /// refusé sans carte.
    Refus,
}

/// Aucun outil.
pub struct SansOutils;

impl Outillage for SansOutils {
    fn definitions(&self) -> Vec<Value> {
        Vec::new()
    }

    fn appeler<'a>(
        &'a self,
        nom: &'a str,
        _: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(async move { Err(format!("outil inconnu : {nom}")) })
    }
}

/// Réglages du backend clé API.
#[derive(Clone)]
pub struct ApiConfig {
    /// Clé de l'API, lue dans le gestionnaire d'identifiants (KOB-104).
    pub cle: String,
    pub modele: String,
    /// Adresse de l'API (un faux serveur dans les tests).
    pub url: String,
    /// `output_config.effort` : `low` par défaut, des réponses de bulle
    /// courtes et rapides.
    pub effort: Option<String>,
    pub max_tokens: u32,
    pub system_prompt: String,
    pub outils: Arc<dyn Outillage>,
    /// Allers-retours d'outils au plus dans un tour.
    pub max_etapes: usize,
    /// Nouvelles tentatives sur une erreur passagère, avant le flux.
    pub tentatives: u32,
    /// Délai de réponse à une carte de confirmation avant refus.
    pub delai_carte: Duration,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            cle: String::new(),
            modele: MODELE_PAR_DEFAUT.to_owned(),
            url: URL_API.to_owned(),
            effort: Some("low".to_owned()),
            max_tokens: 32_000,
            system_prompt: SYSTEM_PROMPT.to_owned(),
            outils: Arc::new(SansOutils),
            max_etapes: 16,
            tentatives: 2,
            delai_carte: kobi_perms::DEFAULT_TIMEOUT,
        }
    }
}

/// La clé ne s'affiche jamais, même dans un journal de débogage.
impl std::fmt::Debug for ApiConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiConfig")
            .field("cle", &"<masquée>")
            .field("modele", &self.modele)
            .field("url", &self.url)
            .field("effort", &self.effort)
            .field("max_tokens", &self.max_tokens)
            .field("max_etapes", &self.max_etapes)
            .finish_non_exhaustive()
    }
}

/// Ce que partagent le cerveau et la tâche du tour en cours.
struct Partage {
    client: reqwest::Client,
    config: ApiConfig,
    /// Définitions d'outils, figées à l'ouverture.
    outils: Vec<Value>,
    historique: Mutex<Vec<Value>>,
}

/// Conversation d'une réunion sur l'API Messages, vue comme un [`Brain`].
pub struct ApiBrain {
    partage: Arc<Partage>,
    busy: Arc<AtomicBool>,
    tache: Option<tokio::task::JoinHandle<()>>,
}

impl ApiBrain {
    pub fn new(config: ApiConfig) -> Result<Self, BrainError> {
        if config.cle.trim().is_empty() {
            return Err(BrainError::Unavailable("aucune clé API enregistrée".into()));
        }
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            // Silence maximal dans le flux (l'API envoie des `ping`).
            .read_timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| BrainError::Unavailable(format!("client HTTP : {e}")))?;
        let outils = config.outils.definitions();
        Ok(Self {
            partage: Arc::new(Partage {
                client,
                config,
                outils,
                historique: Mutex::new(Vec::new()),
            }),
            busy: Arc::new(AtomicBool::new(false)),
            tache: None,
        })
    }

    fn submit(&mut self, texte: String) -> Turn {
        if self.busy.swap(true, Ordering::SeqCst) {
            return Turn::failed(BrainError::Busy);
        }
        let (events, turn) = Turn::channel();
        let (partage, busy) = (self.partage.clone(), self.busy.clone());
        self.tache = Some(tokio::spawn(async move {
            let fin = tour(&partage, texte, &events).await;
            busy.store(false, Ordering::SeqCst);
            let _ = events.send(fin).await;
        }));
        turn
    }
}

/// Cerveau abandonné : le tour en cours s'arrête avec lui.
impl Drop for ApiBrain {
    fn drop(&mut self) {
        if let Some(t) = &self.tache {
            t.abort();
        }
    }
}

impl Brain for ApiBrain {
    fn name(&self) -> &str {
        "api"
    }

    fn ask(&mut self, ask: Ask) -> Turn {
        self.submit(texte_ask(&ask))
    }

    fn update_notes(&mut self, update: NotesUpdate) -> Turn {
        self.submit(texte_notes(&update))
    }

    fn final_report(&mut self, report: ReportRequest) -> Turn {
        self.submit(texte_compte_rendu(&report))
    }
}

/// Joue un tour et rend son événement de fin. En cas d'échec, la
/// conversation revient à son état d'avant le tour, sauf ce qui suit une
/// écriture confirmée et faite : Claude doit savoir qu'elle a eu lieu, pour
/// ne pas la refaire au tour suivant (KOB-320).
async fn tour(partage: &Partage, texte: String, events: &mpsc::Sender<BrainEvent>) -> BrainEvent {
    let mut historique = partage.historique.lock().await;
    let debut = historique.len();
    // Après une écriture gardée, le dernier message est celui des
    // résultats d'outils : le texte du tour s'y ajoute (l'API veut des
    // rôles alternés).
    let ajoute_a = historique
        .last()
        .filter(|m| m["role"] == "user")
        .and_then(|m| m["content"].as_array())
        .map(Vec::len);
    let mut garde = None;
    let fin = boucle(partage, &mut historique, texte, events, &mut garde).await;
    if matches!(fin, BrainEvent::Error(_)) {
        match garde {
            Some(n) => historique.truncate(n),
            None => {
                historique.truncate(debut);
                if let (Some(n), Some(dernier)) = (ajoute_a, historique.last_mut()) {
                    if let Some(c) = dernier["content"].as_array_mut() {
                        c.truncate(n);
                    }
                }
            }
        }
    }
    fin
}

async fn boucle(
    partage: &Partage,
    historique: &mut Vec<Value>,
    texte: String,
    events: &mpsc::Sender<BrainEvent>,
    garde: &mut Option<usize>,
) -> BrainEvent {
    let config = &partage.config;
    let bloc = json!({ "type": "text", "text": texte });
    match historique.last_mut().filter(|m| m["role"] == "user") {
        Some(dernier) => match dernier["content"].as_array_mut() {
            Some(c) => c.push(bloc),
            None => historique.push(json!({ "role": "user", "content": [bloc] })),
        },
        None => historique.push(json!({ "role": "user", "content": [bloc] })),
    }
    let mut usage = Usage::default();
    for _ in 0..=config.max_etapes {
        if events.is_closed() {
            return BrainEvent::Error(BrainError::Crashed("tour abandonné".into()));
        }
        let reponse = match requete(partage, historique, events).await {
            Ok(r) => r,
            Err(e) => return BrainEvent::Error(e),
        };
        ajouter(&mut usage, &reponse.usage);
        let raison = reponse.stop_reason.clone().unwrap_or_default();
        let blocs = a_renvoyer(reponse.blocs());
        match raison.as_str() {
            "refusal" => {
                return BrainEvent::Error(BrainError::Api(
                    "Claude a refusé de répondre à cette demande".into(),
                ))
            }
            "max_tokens" => {
                return BrainEvent::Error(BrainError::Api(format!(
                    "réponse coupée à {} tokens",
                    config.max_tokens
                )))
            }
            _ => {}
        }
        let appels: Vec<Value> = blocs
            .iter()
            .filter(|b| b["type"] == "tool_use")
            .cloned()
            .collect();
        historique.push(json!({ "role": "assistant", "content": blocs }));
        if raison != "tool_use" || appels.is_empty() {
            return BrainEvent::Done(TurnSummary {
                text: texte_final(historique),
                usage: Some(usage),
            });
        }
        let mut resultats = Vec::new();
        let mut execute = false;
        for appel in &appels {
            let (resultat, lance) = executer(partage, appel, events).await;
            execute |= lance;
            resultats.push(resultat);
        }
        historique.push(json!({ "role": "user", "content": resultats }));
        // Un outil lancé (écriture confirmée ou « faire sans demander »,
        // lecture) reste dans la conversation, même si le tour échoue
        // ensuite.
        if execute {
            *garde = Some(historique.len());
        }
    }
    BrainEvent::Error(BrainError::Api(format!(
        "plus de {} appels d'outils dans un tour",
        config.max_etapes
    )))
}

/// Exécute un appel d'outil et rend son bloc `tool_result`, et si l'outil
/// a vraiment été lancé.
async fn executer(
    partage: &Partage,
    appel: &Value,
    events: &mpsc::Sender<BrainEvent>,
) -> (Value, bool) {
    let id = appel["id"].as_str().unwrap_or_default().to_owned();
    let nom = appel["name"].as_str().unwrap_or_default().to_owned();
    let entree = appel["input"].clone();
    let outils = &partage.config.outils;
    let _ = events
        .send(BrainEvent::ToolCall {
            id: id.clone(),
            name: nom.clone(),
            input: entree.clone(),
            action: outils.action(&nom, &entree),
        })
        .await;
    let mut lance = false;
    let resultat = if entree_illisible(appel) {
        Err(json!({ "INVALID_JSON": entree }).to_string())
    } else if let Err(refus) = autoriser(partage, &id, &nom, &entree, events).await {
        Err(refus)
    } else if events.is_closed() {
        // Tour lâché : plus rien ne s'exécute, même sans carte.
        Err("Question abandonnée.".into())
    } else {
        lance = true;
        outils.appeler(&nom, &entree).await
    };
    let _ = events.send(BrainEvent::OutilRendu { id: id.clone() }).await;
    let (contenu, erreur) = match resultat {
        Ok(c) => (c, false),
        Err(e) => (e, true),
    };
    let mut bloc = json!({ "type": "tool_result", "tool_use_id": id, "content": contenu });
    if erreur {
        bloc["is_error"] = Value::Bool(true);
    }
    (bloc, lance)
}

/// Carte de confirmation avant un outil qui écrit (mêmes règles et même
/// carte que le backend Claude Code, KOB-320). `Err` : le refus, rendu à
/// Claude comme résultat de l'outil. Un tour lâché pendant la carte vaut
/// refus.
async fn autoriser(
    partage: &Partage,
    id: &str,
    nom: &str,
    entree: &Value,
    events: &mpsc::Sender<BrainEvent>,
) -> Result<(), String> {
    match partage.config.outils.autorisation(nom) {
        Autorisation::Libre => return Ok(()),
        Autorisation::Refus => return Err(format!("{nom} n'est pas permis dans cette session.")),
        Autorisation::Carte => {}
    }
    let (demande, reponse) = ApprovalRequest::new(PermissionRequest {
        tool_name: nom.to_owned(),
        input: entree.clone(),
        tool_use_id: Some(id.to_owned()),
    });
    if events.send(BrainEvent::Permission(demande)).await.is_err() {
        return Err("Question abandonnée.".into());
    }
    let delai = partage.config.delai_carte;
    tokio::select! {
        decision = tokio::time::timeout(delai, reponse) => match decision {
            Ok(Ok(Decision::Allow)) => Ok(()),
            Ok(Ok(Decision::Deny(raison))) => Err(raison),
            Ok(Err(_)) => Err("Demande abandonnée par Kobi.".into()),
            Err(_) => Err(format!(
                "Pas de confirmation en {} s : refusé.",
                delai.as_secs()
            )),
        },
        _ = events.closed() => Err("Question abandonnée.".into()),
    }
}

/// Texte de la dernière réponse de Claude qui en contient.
fn texte_final(historique: &[Value]) -> String {
    historique
        .iter()
        .rev()
        // Jusqu'au message qui porte le texte du tour (résultats d'outils
        // seuls avant lui).
        .take_while(|m| {
            m["role"] == "assistant"
                || !m["content"]
                    .as_array()
                    .is_some_and(|c| c.iter().any(|b| b["type"] == "text"))
        })
        .filter(|m| m["role"] == "assistant")
        .map(|m| {
            m["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|b| b["type"] == "text")
                .filter_map(|b| b["text"].as_str())
                .collect::<String>()
        })
        .find(|t| !t.trim().is_empty())
        .unwrap_or_default()
}

fn ajouter(total: &mut Usage, usage: &serde_json::Map<String, Value>) {
    let n = |cle: &str| usage.get(cle).and_then(Value::as_u64).unwrap_or(0);
    total.input_tokens += n("input_tokens");
    total.cache_creation_input_tokens += n("cache_creation_input_tokens");
    total.cache_read_input_tokens += n("cache_read_input_tokens");
    total.output_tokens += n("output_tokens");
}

/// Corps d'une requête.
fn corps(partage: &Partage, historique: &[Value]) -> Value {
    let config = &partage.config;
    let mut corps = json!({
        "model": config.modele,
        "max_tokens": config.max_tokens,
        "stream": true,
        "system": [{
            "type": "text",
            "text": config.system_prompt,
            "cache_control": { "type": "ephemeral" },
        }],
        // Cache automatique : le point d'arrêt suit la fin de la
        // conversation.
        "cache_control": { "type": "ephemeral" },
        "messages": historique,
    });
    if !partage.outils.is_empty() {
        corps["tools"] = Value::Array(partage.outils.clone());
    }
    if let Some(effort) = &config.effort {
        corps["output_config"] = json!({ "effort": effort });
    }
    if MODELES_AVEC_REPLI.contains(&config.modele.as_str()) {
        corps["fallbacks"] = json!("default");
    }
    corps
}

/// Envoie la conversation et lit la réponse en flux, en relayant le texte.
async fn requete(
    partage: &Partage,
    historique: &[Value],
    events: &mpsc::Sender<BrainEvent>,
) -> Result<Reponse, BrainError> {
    let config = &partage.config;
    let corps = corps(partage, historique);
    let mut essai = 0;
    let mut reponse = loop {
        let mut req = partage
            .client
            .post(format!("{}/v1/messages", config.url.trim_end_matches('/')))
            .header("x-api-key", &config.cle)
            .header("anthropic-version", VERSION_API)
            .header("content-type", "application/json");
        if corps.get("fallbacks").is_some() {
            req = req.header("anthropic-beta", BETA_REPLI);
        }
        let resultat = req.body(corps.to_string()).send().await;
        let attente = match resultat {
            Ok(r) if r.status().is_success() => break r,
            Ok(r)
                if STATUTS_REESSAYES.contains(&r.status().as_u16())
                    && essai < config.tentatives =>
            {
                r.headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .map(|s| Duration::from_secs(s.min(20)))
                    .unwrap_or(Duration::from_secs(1 << essai))
            }
            Ok(r) => return Err(erreur_http(r).await),
            Err(e) if essai < config.tentatives && (e.is_connect() || e.is_timeout()) => {
                Duration::from_secs(1 << essai)
            }
            Err(e) => return Err(BrainError::Unavailable(format!("API injoignable : {e}"))),
        };
        tracing::warn!(essai, ?attente, "API Anthropic : nouvel essai");
        tokio::time::sleep(attente).await;
        essai += 1;
    };
    let mut decoupeur = Decoupeur::default();
    let mut message = Reponse::default();
    while !message.fini {
        let morceau = match reponse.chunk().await {
            Ok(Some(m)) => m,
            Ok(None) => break,
            Err(e) => return Err(BrainError::Crashed(format!("flux interrompu : {e}"))),
        };
        for donnee in decoupeur.pousser(&morceau) {
            let apport = message
                .appliquer(&donnee)
                .map_err(|e| BrainError::Api(format!("{} : {}", e.genre, e.message)))?;
            let event = match apport {
                Apport::Texte(t) if !t.is_empty() => BrainEvent::Text(t),
                Apport::Vie => BrainEvent::Vie,
                _ => continue,
            };
            let _ = events.send(event).await;
        }
    }
    if !message.fini {
        return Err(BrainError::Crashed(
            "flux terminé sans fin de message".into(),
        ));
    }
    Ok(message)
}

/// Erreur lisible d'une réponse HTTP en échec. La clé n'y figure jamais :
/// seul le message d'erreur de l'API est repris.
async fn erreur_http(r: reqwest::Response) -> BrainError {
    let statut = r.status().as_u16();
    let corps: Value = match r.bytes().await {
        Ok(b) => serde_json::from_slice(&b).unwrap_or(Value::Null),
        Err(_) => Value::Null,
    };
    let message = corps["error"]["message"]
        .as_str()
        .unwrap_or("sans détail")
        .to_owned();
    match statut {
        401 => BrainError::Unavailable(format!("clé API refusée : {message}")),
        403 => BrainError::Unavailable(format!("accès refusé : {message}")),
        _ => BrainError::Api(format!("HTTP {statut} : {message}")),
    }
}

#[cfg(test)]
mod tests;
