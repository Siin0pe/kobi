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

use flux::{a_renvoyer, Apport, Decoupeur, Reponse};

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
/// Remarque du journal d'activité quand la conversation repart à neuf
/// (KOB-316).
pub const AVIS_REPRISE: &str = "Conversation trop longue, reprise à neuf.";
/// Remarque du journal d'activité quand la conversation est résumée par
/// l'API (compaction, KOB-332).
pub const AVIS_RESUME: &str = "Conversation trop longue, résumée.";
/// Compaction à la demande (KOB-332).
const BETA_COMPACTION: &str = "compact-2026-09-04";
/// Consignes du résumé : ce que la suite de la réunion doit garder.
const CONSIGNES_RESUME: &str =
    "Résume cette conversation de réunion pour la suite. Garde tout ce qui \
compte : les décisions, les actions (qui fait quoi, faites, refusées ou en attente), les questions \
ouvertes, les questions posées à Kobi et ses réponses, et l'état des notes. Garde les noms, les \
chiffres et les dates exacts. N'appelle aucun outil ; réponds seulement par le résumé.";
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
    /// Au-delà de ce contexte (tokens d'entrée de la dernière requête), la
    /// conversation de la réunion repart de zéro au tour suivant : chaque
    /// message porte déjà les notes courantes et la transcription nouvelle
    /// (KOB-316).
    pub contexte_max: u64,
    /// Au-delà de `contexte_max`, résumer la conversation par l'API
    /// (compaction à la demande, bêta) plutôt que repartir à neuf. Désactivé
    /// par défaut : à décider après un essai réel (KOB-332). Sans résumé
    /// (refus, réponse coupée…), la conversation repart à neuf.
    pub compaction: bool,
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
            contexte_max: 150_000,
            compaction: false,
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
    /// Tokens d'entrée de la dernière requête (contexte de la conversation).
    contexte: std::sync::atomic::AtomicU64,
    /// Remarque pour le journal d'activité, rendue avec le prochain tour
    /// réussi.
    avis: std::sync::Mutex<Option<String>>,
    /// La conversation commence par un bloc de compaction : chaque requête
    /// porte l'en-tête bêta.
    compacte: AtomicBool,
    /// Un résumé est en cours, en tâche de fond (KOB-350).
    en_resume: AtomicBool,
    /// Le dernier résumé n'a pas pu se faire (refus, réponse coupée…) : la
    /// conversation repart à neuf au tour suivant (KOB-349).
    reprise_demandee: AtomicBool,
}

/// Conversation d'une réunion sur l'API Messages, vue comme un [`Brain`].
pub struct ApiBrain {
    partage: Arc<Partage>,
    busy: Arc<AtomicBool>,
    /// Tours, et résumés qui les suivent : arrêtés avec le cerveau.
    taches: Vec<tokio::task::JoinHandle<()>>,
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
                contexte: std::sync::atomic::AtomicU64::new(0),
                avis: std::sync::Mutex::new(None),
                compacte: AtomicBool::new(false),
                en_resume: AtomicBool::new(false),
                reprise_demandee: AtomicBool::new(false),
            }),
            busy: Arc::new(AtomicBool::new(false)),
            taches: Vec::new(),
        })
    }

    /// `resumer` : un résumé peut suivre le tour (pas après le compte rendu
    /// final, la conversation s'arrête là).
    fn submit(&mut self, texte: String, resumer: bool) -> Turn {
        if self.busy.swap(true, Ordering::SeqCst) {
            return Turn::failed(BrainError::Busy);
        }
        let (events, turn) = Turn::channel();
        let (partage, busy) = (self.partage.clone(), self.busy.clone());
        self.taches.retain(|t| !t.is_finished());
        self.taches.push(tokio::spawn(async move {
            let fin = tour(&partage, texte, &events).await;
            busy.store(false, Ordering::SeqCst);
            let fini = matches!(fin, BrainEvent::Done(_));
            let _ = events.send(fin).await;
            // Résumé après la réponse, jamais avant : il ne retarde ni une
            // réponse, ni le compte rendu (KOB-350).
            if fini && resumer {
                resumer_si_besoin(&partage).await;
            }
        }));
        turn
    }
}

/// Cerveau abandonné : le tour en cours s'arrête avec lui.
impl Drop for ApiBrain {
    fn drop(&mut self) {
        for t in &self.taches {
            t.abort();
        }
    }
}

impl Brain for ApiBrain {
    fn name(&self) -> &str {
        "api"
    }

    fn ask(&mut self, ask: Ask) -> Turn {
        self.submit(texte_ask(&ask), true)
    }

    fn update_notes(&mut self, update: NotesUpdate) -> Turn {
        // Pas d'outil sur carte ici (KOB-320) : une proposition d'action
        // (notes en continu, KOB-72) ne serait qu'une consigne sur des outils
        // absents (KOB-331).
        self.submit(
            texte_notes(&NotesUpdate {
                propositions: false,
                ..update
            }),
            true,
        )
    }

    fn final_report(&mut self, report: ReportRequest) -> Turn {
        self.submit(texte_compte_rendu(&report), false)
    }
}

/// Joue un tour et rend son événement de fin. En cas d'échec, la
/// conversation revient à son état d'avant le tour, sauf ce qui suit une
/// écriture confirmée et faite : Claude doit savoir qu'elle a eu lieu, pour
/// ne pas la refaire au tour suivant (KOB-320).
async fn tour(partage: &Partage, texte: String, events: &mpsc::Sender<BrainEvent>) -> BrainEvent {
    let mut historique = partage.historique.lock().await;
    let contexte = partage.contexte.load(Ordering::SeqCst);
    let max = partage.config.contexte_max;
    // Reprise à neuf : sans compaction dès le seuil ; avec compaction, si le
    // résumé est impossible, ou près d'une limite dure (deux fois le seuil),
    // le résumé de fond n'étant pas arrivé à temps (KOB-349).
    let reprise = contexte > max
        && (!partage.config.compaction
            || partage.reprise_demandee.swap(false, Ordering::SeqCst)
            || contexte > max.saturating_mul(2));
    if reprise && !historique.is_empty() {
        tracing::info!(
            contexte,
            messages = historique.len(),
            "conversation de la réunion trop longue : elle repart des notes courantes"
        );
        historique.clear();
        partage.compacte.store(false, Ordering::SeqCst);
        partage.contexte.store(0, Ordering::SeqCst);
        // Une demande de l'ancienne conversation ne vaut pas pour la
        // nouvelle (KOB-356).
        partage.reprise_demandee.store(false, Ordering::SeqCst);
        *partage.avis.lock().unwrap_or_else(|e| e.into_inner()) = Some(AVIS_REPRISE.to_owned());
    }
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
    // Un tour que l'appelant a lâché n'a jamais été montré : il est retiré
    // comme un échec, même s'il a fini (KOB-314).
    if matches!(fin, BrainEvent::Error(_)) || events.is_closed() {
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
    // `max_etapes` allers-retours d'outils au plus, puis une dernière
    // requête sans outil possible pour la réponse finale (KOB-319) : aucun
    // lot d'outils n'est exécuté pour rien.
    for etape in 0..=config.max_etapes {
        let derniere = etape == config.max_etapes;
        if events.is_closed() {
            return BrainEvent::Error(BrainError::Crashed("tour abandonné".into()));
        }
        let reponse = match requete(partage, historique, derniere, events).await {
            Ok(r) => r,
            Err(e) => return BrainEvent::Error(e),
        };
        ajouter(&mut usage, &reponse.usage);
        let n = |cle: &str| reponse.usage.get(cle).and_then(Value::as_u64).unwrap_or(0);
        // Coût suivi requête par requête (KOB-103) : le cache doit servir dès
        // la deuxième question.
        let (entree, cache_lu, cache_ecrit, sortie) = (
            n("input_tokens"),
            n("cache_read_input_tokens"),
            n("cache_creation_input_tokens"),
            n("output_tokens"),
        );
        tracing::info!(
            modele = %config.modele,
            entree,
            cache_lu,
            cache_ecrit,
            sortie,
            "API Messages : tokens de la requête"
        );
        partage.contexte.store(
            n("input_tokens") + n("cache_read_input_tokens") + n("cache_creation_input_tokens"),
            Ordering::SeqCst,
        );
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
            "end_turn" | "tool_use" | "stop_sequence" => {}
            // Contexte dépassé (`model_context_window_exceeded`), raison
            // inconnue ou absente : la réponse est peut-être tronquée, le
            // tour échoue plutôt que de remplacer les notes (KOB-316).
            autre => {
                if autre == "model_context_window_exceeded" {
                    partage.contexte.store(u64::MAX, Ordering::SeqCst);
                }
                return BrainEvent::Error(BrainError::Api(format!(
                    "réponse interrompue ({})",
                    if autre.is_empty() {
                        "sans raison"
                    } else {
                        autre
                    }
                )));
            }
        }
        let appels: Vec<Value> = blocs
            .iter()
            .filter(|b| b["type"] == "tool_use")
            .cloned()
            .collect();
        if derniere && !appels.is_empty() {
            // Claude demande encore un outil malgré `tool_choice: none` :
            // rien n'est exécuté.
            return BrainEvent::Error(BrainError::Api(format!(
                "plus de {} allers-retours d'outils dans un tour",
                config.max_etapes
            )));
        }
        historique.push(json!({ "role": "assistant", "content": blocs }));
        if raison != "tool_use" || appels.is_empty() {
            return BrainEvent::Done(TurnSummary {
                text: texte_final(historique),
                usage: Some(usage),
                // La reprise à neuf se montre avec le premier tour réussi.
                avis: partage
                    .avis
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .take(),
            });
        }
        let mut resultats = Vec::new();
        let mut execute = false;
        for appel in &appels {
            let brut = appel["id"]
                .as_str()
                .and_then(|id| reponse.illisibles.get(id))
                .map(String::as_str);
            let (resultat, lance) = executer(partage, appel, brut, events).await;
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
    // La dernière étape rend toujours une fin de tour ; par prudence.
    BrainEvent::Error(BrainError::Api(format!(
        "plus de {} allers-retours d'outils dans un tour",
        config.max_etapes
    )))
}

/// Exécute un appel d'outil et rend son bloc `tool_result`, et si l'outil
/// a vraiment été lancé. `illisible` : texte brut d'une entrée que le flux
/// n'a pas pu lire, rendu à Claude en erreur sans lancer l'outil.
async fn executer(
    partage: &Partage,
    appel: &Value,
    illisible: Option<&str>,
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
    let resultat = if let Some(brut) = illisible {
        Err(json!({ "INVALID_JSON": brut }).to_string())
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
fn corps(partage: &Partage, historique: &[Value], sans_outil: bool) -> Value {
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
        // Dernier aller-retour : la liste d'outils reste la même (la
        // réflexion préservée et le cache du système et des outils en
        // dépendent), seul `tool_choice` interdit de s'en servir. Changer
        // `tool_choice` invalide le cache des messages : cette requête-là
        // réécrit la conversation au tarif d'écriture. C'est accepté, elle ne
        // vient qu'après `max_etapes` allers-retours (KOB-348).
        if sans_outil {
            corps["tool_choice"] = json!({ "type": "none" });
        }
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
    sans_outil: bool,
    events: &mpsc::Sender<BrainEvent>,
) -> Result<Reponse, BrainError> {
    requete_corps(partage, &corps(partage, historique, sans_outil), events).await
}

/// Issue d'une demande de résumé.
enum Resume {
    /// Le bloc de compaction, à renvoyer tel quel.
    Bloc(Value),
    /// Échec passager (abandon, surcharge, réseau) : on réessaiera après le
    /// tour suivant, la conversation reste entière (KOB-349).
    Passager,
    /// Pas de résumé possible (refus, réponse coupée…) : reprise à neuf.
    Impossible,
}

/// Résumé de fond après un tour réussi (KOB-332, KOB-350) : sur une copie
/// de la conversation, sans la bloquer. Le bloc remplace ensuite le début
/// résumé si celui-ci n'a pas changé ; les tours arrivés entre-temps le
/// suivent, tels quels (compaction à la demande, « en tâche de fond »).
async fn resumer_si_besoin(partage: &Partage) {
    let config = &partage.config;
    if !config.compaction
        || partage.contexte.load(Ordering::SeqCst) <= config.contexte_max
        // Résumé impossible : la reprise à neuf attend le tour suivant.
        || partage.reprise_demandee.load(Ordering::SeqCst)
        || partage.en_resume.swap(true, Ordering::SeqCst)
    {
        return;
    }
    // Remis à faux en sortant, même sur une panique.
    struct Fin<'a>(&'a AtomicBool);
    impl Drop for Fin<'_> {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }
    let _fin = Fin(&partage.en_resume);
    let copie = partage.historique.lock().await.clone();
    let issue = if copie.is_empty() {
        Resume::Passager
    } else {
        compacter(partage, &copie).await
    };
    if matches!(issue, Resume::Passager) {
        tracing::info!("résumé reporté après le tour suivant");
        return;
    }
    let mut historique = partage.historique.lock().await;
    // La conversation commence toujours par la copie résumée : rien n'a été
    // retiré ni repris à neuf pendant le résumé.
    let n = copie.len();
    let inchangee = historique.len() >= n && historique[..n] == copie[..];
    match issue {
        Resume::Bloc(bloc) => {
            if inchangee {
                tracing::info!(
                    messages = n,
                    suite = historique.len() - n,
                    "conversation de la réunion résumée par l'API"
                );
                historique.splice(..n, [json!({ "role": "assistant", "content": [bloc] })]);
                partage.compacte.store(true, Ordering::SeqCst);
                partage.contexte.store(0, Ordering::SeqCst);
                partage.reprise_demandee.store(false, Ordering::SeqCst);
                *partage.avis.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some(AVIS_RESUME.to_owned());
            } else {
                tracing::info!("conversation changée pendant le résumé : résumé écarté");
            }
        }
        Resume::Passager => {}
        // Sur une conversation déjà reprise à neuf, l'échec ne dit rien de
        // la nouvelle : elle sera résumée à son tour (KOB-356).
        Resume::Impossible if inchangee => {
            partage.reprise_demandee.store(true, Ordering::SeqCst);
        }
        Resume::Impossible => {
            tracing::info!("conversation changée pendant le résumé : échec écarté");
        }
    }
}

/// Demande à l'API le résumé de `historique` (compaction à la demande).
async fn compacter(partage: &Partage, historique: &[Value]) -> Resume {
    let mut corps = corps(partage, historique, false);
    if let Some(o) = corps.as_object_mut() {
        o.remove("fallbacks");
    }
    corps["compaction"] = json!({ "type": "summarize", "instructions": CONSIGNES_RESUME });
    // Aucun tour n'écoute ce résumé : ses événements partent dans le vide,
    // sans que le canal se ferme (ce qui vaudrait abandon).
    let (events, mut vide) = mpsc::channel(64);
    let vidage = tokio::spawn(async move { while vide.recv().await.is_some() {} });
    let reponse = requete_corps(partage, &corps, &events).await;
    drop(events);
    let _ = vidage.await;
    let reponse = match reponse {
        Ok(r) => r,
        Err(e) => {
            // Abandon, réseau, surcharge (429, 5xx, 529) : passager. Une
            // erreur de la requête elle-même (4xx, clé refusée) se
            // reproduirait à chaque essai : reprise à neuf.
            let definitive = match &e {
                BrainError::Api(m) => m.starts_with("HTTP 4") && !m.starts_with("HTTP 429"),
                BrainError::Unavailable(_) => true,
                _ => false,
            };
            tracing::warn!(erreur = %e, definitive, "pas de résumé");
            return if definitive {
                Resume::Impossible
            } else {
                Resume::Passager
            };
        }
    };
    // Coût du résumé : l'API le rend dans `usage.iterations`, le premier
    // niveau valant 0 (KOB-351).
    let iterations = reponse.usage.get("iterations").and_then(Value::as_array);
    let somme = |cle: &str| {
        iterations
            .into_iter()
            .flatten()
            .filter_map(|i| i[cle].as_u64())
            .sum::<u64>()
    };
    let (entree, cache_lu, cache_ecrit, sortie) = (
        somme("input_tokens"),
        somme("cache_read_input_tokens"),
        somme("cache_creation_input_tokens"),
        somme("output_tokens"),
    );
    tracing::info!(
        modele = %partage.config.modele,
        detail = iterations.is_some(),
        entree,
        cache_lu,
        cache_ecrit,
        sortie,
        "API Messages : tokens du résumé"
    );
    let bloc = reponse
        .blocs()
        .into_iter()
        .find(|b| b["type"] == "compaction" && b["content"].is_string());
    match (reponse.stop_reason.as_deref(), bloc) {
        (Some("compaction"), Some(bloc)) => Resume::Bloc(bloc),
        (raison, _) => {
            tracing::warn!(?raison, "pas de résumé : la conversation repartira à neuf");
            Resume::Impossible
        }
    }
}

/// Envoie un corps de requête et lit la réponse en flux.
async fn requete_corps(
    partage: &Partage,
    corps: &Value,
    events: &mpsc::Sender<BrainEvent>,
) -> Result<Reponse, BrainError> {
    let config = &partage.config;
    let mut essai = 0;
    let abandon = || BrainError::Crashed("tour abandonné".into());
    // Un essai : la requête, puis la lecture du flux. Une surcharge, qu'elle
    // arrive en statut HTTP ou dans le flux avant tout contenu (KOB-315),
    // est réessayée.
    'essais: loop {
        let mut req = partage
            .client
            .post(format!("{}/v1/messages", config.url.trim_end_matches('/')))
            .header("x-api-key", &config.cle)
            .header("anthropic-version", VERSION_API)
            .header("content-type", "application/json");
        let mut betas = Vec::new();
        if corps.get("fallbacks").is_some() {
            betas.push(BETA_REPLI);
        }
        if corps.get("compaction").is_some() || partage.compacte.load(Ordering::SeqCst) {
            betas.push(BETA_COMPACTION);
        }
        if !betas.is_empty() {
            req = req.header("anthropic-beta", betas.join(","));
        }
        // Tour lâché pendant l'attente : la requête est abandonnée.
        let resultat = tokio::select! {
            r = req.body(corps.to_string()).send() => r,
            _ = events.closed() => return Err(abandon()),
        };
        let attente = match resultat {
            Ok(r) if r.status().is_success() => {
                match lire(r, events, essai < config.tentatives).await? {
                    Some(message) => return Ok(message),
                    None => Duration::from_secs(1 << essai),
                }
            }
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
            // Réseau : erreur d'API, pas de session cassée ; la conversation
            // reste pour la question mise en attente (KOB-361).
            Err(e) => return Err(BrainError::Api(format!("API injoignable : {e}"))),
        };
        tracing::warn!(essai, ?attente, "API Anthropic : nouvel essai");
        tokio::select! {
            _ = tokio::time::sleep(attente) => {}
            _ = events.closed() => return Err(abandon()),
        }
        essai += 1;
        continue 'essais;
    }
}

/// Erreurs du flux réessayées si elles arrivent avant tout texte montré.
const ERREURS_FLUX_REESSAYEES: &[&str] = &["overloaded_error", "api_error"];

/// Lit le flux d'une réponse en relayant le texte. `Ok(None)` : à réessayer
/// (si `reessai`) : surcharge annoncée dans le flux ou flux coupé, avant
/// tout texte montré (KOB-360, KOB-370) : un bloc de réflexion ou un outil
/// pas encore fini ne fait rien, et rien n'a été montré.
///
/// Un flux coupé est une erreur de réseau, pas un plantage : la
/// conversation est intacte (le tour la remet en état), la session reste
/// ouverte. Après un texte montré, il n'est pas rejoué (il le serait en
/// double) : erreur d'API.
async fn lire(
    mut reponse: reqwest::Response,
    events: &mpsc::Sender<BrainEvent>,
    reessai: bool,
) -> Result<Option<Reponse>, BrainError> {
    let mut decoupeur = Decoupeur::default();
    let mut message = Reponse::default();
    let mut montre = false;
    let coupure = |raison: String, montre: bool| {
        if reessai && !montre {
            tracing::warn!(%raison, "API Anthropic : flux coupé avant tout texte");
            Ok(None)
        } else {
            Err(BrainError::Api(raison))
        }
    };
    while !message.fini {
        // Tour lâché pendant le flux (délai, Stop) : la lecture s'arrête et
        // la connexion se ferme (KOB-314).
        let morceau = tokio::select! {
            m = reponse.chunk() => m,
            _ = events.closed() => {
                return Err(BrainError::Crashed("tour abandonné".into()))
            }
        };
        let morceau = match morceau {
            Ok(Some(m)) => m,
            Ok(None) => break,
            Err(e) => return coupure(format!("flux interrompu : {e}"), montre),
        };
        for donnee in decoupeur.pousser(&morceau) {
            let apport = match message.appliquer(&donnee) {
                Ok(a) => a,
                Err(e)
                    if reessai
                        && ERREURS_FLUX_REESSAYEES.contains(&e.genre.as_str())
                        && !montre =>
                {
                    tracing::warn!(genre = %e.genre, "API Anthropic : surcharge dans le flux");
                    return Ok(None);
                }
                Err(e) => return Err(BrainError::Api(format!("{} : {}", e.genre, e.message))),
            };
            let event = match apport {
                Apport::Texte(t) if !t.is_empty() => {
                    montre = true;
                    BrainEvent::Text(t)
                }
                Apport::Vie => BrainEvent::Vie,
                _ => continue,
            };
            let _ = events.send(event).await;
        }
    }
    if !message.fini {
        return coupure("flux terminé sans fin de message".into(), montre);
    }
    Ok(Some(message))
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
