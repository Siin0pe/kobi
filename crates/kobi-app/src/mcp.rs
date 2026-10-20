//! Serveurs MCP connectés à Kobi (KOB-155).
//!
//! * **Réglages → MCP** : liste, ajout (stdio, HTTP, SSE), activation,
//!   réglage d'écriture par serveur, bouton « Tester » qui liste les outils.
//! * **Import** des serveurs de Claude Code (`~/.claude.json` : portées
//!   utilisateur et locale ; `.mcp.json` des dossiers de code : portée
//!   projet). Kobi ne lit que les sections `mcpServers`, jamais les
//!   identifiants de Claude (conditions Anthropic, point 2). Un serveur
//!   importé garde son nom et son URL : Claude Code réutilise alors la
//!   connexion OAuth qu'il a déjà (vérifié le 18/10/2026 avec Linear).
//! * **Secrets** (en-têtes HTTP, variables d'environnement sensibles) : dans
//!   le gestionnaire d'identifiants du système (service « Kobi MCP »), jamais
//!   dans `config.toml`. Ils arrivent aux sessions par l'environnement du
//!   process `claude`, que la config MCP cite en `${KOBI_MCP_…}` : rien de
//!   secret sur la ligne de commande ni sur le disque.
//! * **Classement** des outils : annotations `readOnlyHint` /
//!   `destructiveHint` lues au test, règles dans [`kobi_perms::mcp`].
//!   Pour lire les annotations d'un serveur protégé par OAuth, Kobi demande
//!   sa propre autorisation (bouton « Autoriser Kobi »), gardée elle aussi
//!   dans le gestionnaire d'identifiants.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use kobi_perms::mcp::{Catalogue, Nature, Politique};
use kobi_session::config;
use kobi_session::mcp::{nom_valide, Ecriture, McpConfig, NatureOutil, Serveur, Transport};
use kobi_session::stockage::maintenant_ms;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

// ----- Secrets -----

/// Où vivent les secrets des serveurs.
pub trait Coffre: Send + Sync {
    fn lire(&self, serveur: &str, nom: &str) -> Result<Option<String>, String>;
    fn ecrire(&self, serveur: &str, nom: &str, valeur: &str) -> Result<(), String>;
    fn effacer(&self, serveur: &str, nom: &str) -> Result<(), String>;
}

/// Service des entrées de Kobi dans le gestionnaire d'identifiants.
const SERVICE: &str = "Kobi MCP";
/// Nom de l'entrée qui garde l'autorisation OAuth de Kobi pour un serveur
/// (`:` ne peut pas apparaître dans un nom d'en-tête ni de variable).
const OAUTH: &str = ":oauth";

/// Gestionnaire d'identifiants du système (Windows : Gestionnaire
/// d'identifiants ; macOS : trousseau ; Linux : Secret Service).
pub struct CoffreSysteme;

impl CoffreSysteme {
    fn entree(serveur: &str, nom: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(SERVICE, &format!("{serveur}/{nom}")).map_err(erreur_coffre)
    }
}

impl Coffre for CoffreSysteme {
    fn lire(&self, serveur: &str, nom: &str) -> Result<Option<String>, String> {
        match Self::entree(serveur, nom)?.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(erreur_coffre(e)),
        }
    }

    fn ecrire(&self, serveur: &str, nom: &str, valeur: &str) -> Result<(), String> {
        Self::entree(serveur, nom)?
            .set_password(valeur)
            .map_err(erreur_coffre)
    }

    fn effacer(&self, serveur: &str, nom: &str) -> Result<(), String> {
        match Self::entree(serveur, nom)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(erreur_coffre(e)),
        }
    }
}

/// Erreur du gestionnaire d'identifiants, dite à l'utilisateur (KOB-185) :
/// un accès refusé dit quoi faire, le détail technique va au journal.
fn erreur_coffre(e: keyring::Error) -> String {
    tracing::warn!(erreur = %e, "gestionnaire d'identifiants");
    match e {
        keyring::Error::PlatformFailure(_) | keyring::Error::NoStorageAccess(_) => format!(
            "Le gestionnaire d'identifiants du système refuse l'accès : Kobi ne peut pas \
             y ranger ce secret, et ne l'écrit jamais en clair sur le disque. {}",
            MARCHE_A_SUIVRE
        ),
        e => format!("gestionnaire d'identifiants : {e}"),
    }
}

/// Que faire quand le gestionnaire d'identifiants refuse l'accès.
#[cfg(target_os = "linux")]
const MARCHE_A_SUIVRE: &str = "Sous Linux, Kobi passe par Secret Service : démarre le \
    trousseau de ta session (gnome-keyring ou KWallet) et déverrouille-le, puis réessaie.";
#[cfg(target_os = "macos")]
const MARCHE_A_SUIVRE: &str =
    "Déverrouille ton trousseau de session (app Trousseaux d'accès), puis réessaie.";
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
const MARCHE_A_SUIVRE: &str = "Vérifie que le Gestionnaire d'identifiants de Windows \
    fonctionne (service « Gestionnaire d'informations d'identification »), puis réessaie.";

/// Variable d'environnement qui porte un secret jusqu'à la session :
/// `KOBI_MCP_<SERVEUR>_<NOM>`, en majuscules, `_` pour le reste.
pub fn variable(serveur: &str, nom: &str) -> String {
    format!("KOBI_MCP_{serveur}_{nom}")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

// ----- Sessions -----

/// Ce que les sessions de Kobi reçoivent des serveurs activés.
#[derive(Debug, Default)]
pub struct PourSession {
    /// Config de chaque serveur pour `--mcp-config`, secrets en `${…}`.
    pub serveurs: Vec<(String, Value)>,
    /// Secrets, pour l'environnement du process `claude`.
    pub env: Vec<(String, String)>,
    /// Classement des outils (lecture sans carte, écriture avec).
    pub catalogue: Catalogue,
}

impl PourSession {
    pub fn noms(&self) -> Vec<&str> {
        self.serveurs.iter().map(|(n, _)| n.as_str()).collect()
    }
}

fn nature(n: NatureOutil) -> Nature {
    match n {
        NatureOutil::Lecture => Nature::Lecture,
        NatureOutil::Ecriture => Nature::Ecriture,
        NatureOutil::Suppression => Nature::Suppression,
    }
}

fn nature_outil(n: Nature) -> NatureOutil {
    match n {
        Nature::Lecture => NatureOutil::Lecture,
        Nature::Ecriture => NatureOutil::Ecriture,
        Nature::Suppression => NatureOutil::Suppression,
    }
}

/// Config d'un serveur au format de Claude Code ; `secret` donne la valeur
/// à écrire à la place de chaque secret (`${VAR}` pour une session, la
/// vraie valeur pour un test).
fn config_claude(s: &Serveur, mut secret: impl FnMut(&str) -> Option<String>) -> Value {
    let mut cles: Map<String, Value> = Map::new();
    let paires = match s.transport {
        Transport::Stdio => &s.env,
        Transport::Http | Transport::Sse => &s.entetes,
    };
    for (k, v) in paires {
        cles.insert(k.clone(), v.clone().into());
    }
    for nom in &s.secrets {
        if let Some(v) = secret(nom) {
            cles.insert(nom.clone(), v.into());
        }
    }
    match s.transport {
        Transport::Stdio => {
            let mut c = json!({ "command": s.commande, "args": s.args });
            if !cles.is_empty() {
                c["env"] = Value::Object(cles);
            }
            c
        }
        Transport::Http | Transport::Sse => {
            let genre = if s.transport == Transport::Http {
                "http"
            } else {
                "sse"
            };
            let mut c = json!({ "type": genre, "url": s.url });
            if !cles.is_empty() {
                c["headers"] = Value::Object(cles);
            }
            c
        }
    }
}

/// Serveurs activés, prêts pour une session.
pub fn pour_session(config: &McpConfig, coffre: &dyn Coffre) -> PourSession {
    let mut session = PourSession::default();
    for (nom, s) in config.serveurs.iter().filter(|(_, s)| s.actif) {
        let config = config_claude(s, |cle| match coffre.lire(nom, cle) {
            Ok(Some(valeur)) => {
                let var = variable(nom, cle);
                session.env.push((var.clone(), valeur));
                Some(format!("${{{var}}}"))
            }
            Ok(None) => {
                tracing::warn!(serveur = %nom, %cle, "secret MCP absent, ignoré");
                None
            }
            Err(e) => {
                tracing::warn!(serveur = %nom, %cle, erreur = %e, "secret MCP illisible");
                None
            }
        });
        session.serveurs.push((nom.clone(), config));
        session.catalogue.serveurs.insert(
            nom.clone(),
            kobi_perms::mcp::Serveur {
                politique: match s.ecriture {
                    Ecriture::Demander => Politique::Demander,
                    Ecriture::SansDemander => Politique::SansDemander,
                },
                outils: s
                    .outils
                    .iter()
                    .map(|(o, n)| (o.clone(), nature(*n)))
                    .collect(),
            },
        );
    }
    session
}

/// Ajouté au prompt système quand des serveurs MCP sont connectés.
pub fn prompt(noms: &[&str]) -> String {
    format!(
        "\
## Outils MCP connectés

Exception à la lecture seule : tu peux utiliser les outils MCP des serveurs connectés \
({}), écriture comprise. Lire (chercher, lister, consulter) se fait directement. Pour ce \
qui modifie ou envoie quelque chose (créer, modifier, commenter, envoyer, supprimer), \
appelle l'outil directement : Kobi montre lui-même une carte de confirmation à \
l'utilisateur et ne fait l'action qu'après son accord. Ne demande pas « tu confirmes ? » \
avant l'appel. Si l'action est refusée, dis-le en une phrase, sans réessayer. \
Fichiers et commandes restent en lecture seule.",
        noms.join(", ")
    )
}

/// Ce que [`crate::chef::fabrique_claude`] donne à une session.
pub struct SessionClaude {
    pub args: kobi_brain::ArgsWithPerms,
    pub regles: kobi_perms::Rules,
    pub env: Vec<(String, String)>,
}

/// Fichiers de config de session plus vieux qu'un jour : retirés (chaque
/// session écrit le sien, `claude` le lit au lancement).
fn nettoyer(dossier: &Path) {
    let Ok(entrees) = std::fs::read_dir(dossier) else {
        return;
    };
    for e in entrees.flatten() {
        let vieux = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|d| d > Duration::from_secs(24 * 3600));
        let nom = e.file_name().to_string_lossy().into_owned();
        if vieux && nom.starts_with("session-") && nom.ends_with(".json") {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Session Claude de Kobi : serveurs MCP activés (relus dans `config.toml`),
/// MCP de Kobi (KOB-161) et serveur de permission, qui classe chaque appel
/// d'outil MCP (lecture sans carte, écriture avec) ; avec `reglages`, les
/// commandes passent par le hook de Kobi (KOB-154).
pub fn session_claude(
    role: crate::cerveau::Role,
    modele: &'static str,
    codebases: Arc<Vec<PathBuf>>,
    memoire: Option<crate::lecture::Memoire>,
    reglages: Option<PathBuf>,
) -> SessionClaude {
    let cfg = config::default_path()
        .map(|p| config::load_or_create(&p).config)
        .unwrap_or_default();
    let session = pour_session(&cfg.mcp, &CoffreSysteme);
    let regles = session.catalogue.regles();
    let noms: Vec<&str> = session
        .noms()
        .into_iter()
        .filter(|n| nom_valide(n).is_ok())
        .collect();
    tracing::info!(?role, serveurs = ?noms, "serveurs MCP de la session");
    let prompt = (!noms.is_empty()).then(|| prompt(&noms));
    let serveurs: Vec<(String, Value)> = session
        .serveurs
        .into_iter()
        .filter(|(n, _)| nom_valide(n).is_ok())
        .collect();
    // La conversation pilote la réunion (KOB-165), pas les notes.
    let pilote = role == crate::cerveau::Role::Conversation;
    let role = format!("{role:?}").to_lowercase();
    let args = kobi_brain::ArgsWithPerms(Arc::new(move |perms: &Value| {
        let dossier = config::kobi_dir()
            .ok_or("dossier personnel introuvable")?
            .join("mcp");
        nettoyer(&dossier);
        let port = perms["url"]
            .as_str()
            .and_then(|u| u.rsplit(':').next())
            .and_then(|p| p.split('/').next())
            .unwrap_or("0");
        let fichier = dossier.join(format!("session-{role}-{port}.json"));
        let mut utilisateur: Vec<crate::mcp_config::ServeurMcp> = serveurs
            .iter()
            .map(|(nom, config)| crate::mcp_config::ServeurMcp {
                nom: nom.clone(),
                config: config.clone(),
                outils_autorises: Vec::new(),
            })
            .collect();
        utilisateur.push(crate::mcp_config::ServeurMcp {
            nom: kobi_perms::SERVER_NAME.into(),
            config: perms.clone(),
            outils_autorises: Vec::new(),
        });
        let mut mcp = crate::mcp_config::assembler(&fichier, utilisateur)
            .map_err(|e| format!("config MCP de la session : {e}"))?;
        mcp.permission_prompt_tool = Some(kobi_perms::PERMISSION_PROMPT_TOOL.into());
        mcp.prompt = prompt.clone();
        // Commandes validées par le hook de Kobi (KOB-154).
        Ok(crate::commandes::brancher(
            crate::lecture::arguments(
                modele,
                kobi_brain::SYSTEM_PROMPT,
                &codebases,
                memoire.as_ref(),
                Some(&mcp),
                pilote,
            ),
            reglages.as_deref(),
        ))
    }));
    SessionClaude {
        args,
        regles,
        env: session.env,
    }
}

// ----- Import depuis Claude Code -----

/// Serveur trouvé dans la config de Claude Code.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Candidat {
    pub nom: String,
    /// « utilisateur », « local · C:/code/site », « projet · C:/code/site ».
    pub portee: String,
    pub transport: Transport,
    /// Adresse ou commande, sans secret (ni requête d'URL ni arguments).
    pub resume: String,
    /// Un serveur du même nom est déjà dans Kobi.
    pub deja: bool,
    /// Noms des valeurs qui iront dans le gestionnaire d'identifiants.
    pub secrets: Vec<String>,
    #[serde(skip)]
    brut: Value,
}

/// Une variable d'environnement de ce nom est-elle un secret ?
fn env_secrete(nom: &str) -> bool {
    let nom = nom.to_ascii_uppercase();
    [
        "KEY",
        "TOKEN",
        "SECRET",
        "PASS",
        "AUTH",
        "CREDENTIAL",
        "COOKIE",
        "PRIVATE",
    ]
    .iter()
    .any(|m| nom.contains(m))
}

/// Serveur Kobi et secrets à ranger, depuis une entrée `mcpServers`.
fn depuis_claude(brut: &Value, portee: &str) -> Option<(Serveur, Vec<(String, String)>)> {
    let texte = |c: &str| brut.get(c).and_then(Value::as_str).unwrap_or_default();
    let paires = |c: &str| -> Vec<(String, String)> {
        brut.get(c)
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
            .collect()
    };
    let mut secrets = Vec::new();
    let transport = match texte("type") {
        "" | "stdio" => Transport::Stdio,
        "http" | "streamable-http" => Transport::Http,
        "sse" => Transport::Sse,
        _ => return None,
    };
    let mut s = match transport {
        Transport::Stdio => {
            let args = brut
                .get("args")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|a| a.as_str().map(String::from))
                .collect();
            let mut s = Serveur::stdio(texte("command"), args);
            for (k, v) in paires("env") {
                // `${VAR}` renvoie à l'environnement, ce n'est pas un secret.
                if env_secrete(&k) && !v.contains("${") {
                    s.secrets.push(k.clone());
                    secrets.push((k, v));
                } else {
                    s.env.insert(k, v);
                }
            }
            s
        }
        Transport::Http | Transport::Sse => {
            let mut s = Serveur::http(texte("url"));
            s.transport = transport;
            // Tout en-tête peut porter un jeton : tous secrets.
            for (k, v) in paires("headers") {
                s.secrets.push(k.clone());
                secrets.push((k, v));
            }
            s
        }
    };
    if s.commande.is_empty() && s.url.is_empty() {
        return None;
    }
    s.origine = Some(format!("Claude Code ({portee})"));
    Some((s, secrets))
}

fn resume(s: &Serveur) -> String {
    match s.transport {
        Transport::Stdio => s.commande.clone(),
        Transport::Http | Transport::Sse => s
            .url
            .split(['?', '#'])
            .next()
            .unwrap_or_default()
            .to_owned(),
    }
}

/// Serveurs de Claude Code : `claude_json` (contenu de `~/.claude.json`),
/// puis `.mcp.json` de chaque dossier de code (`projets`).
pub fn candidats(
    claude_json: &Value,
    projets: &[(PathBuf, Value)],
    kobi: &McpConfig,
) -> Vec<Candidat> {
    let mut sources: Vec<(String, &Value)> = Vec::new();
    if let Some(s) = claude_json.get("mcpServers") {
        sources.push(("utilisateur".into(), s));
    }
    if let Some(p) = claude_json.get("projects").and_then(Value::as_object) {
        for (dossier, v) in p {
            if let Some(s) = v.get("mcpServers") {
                sources.push((format!("local · {dossier}"), s));
            }
        }
    }
    for (dossier, v) in projets {
        if let Some(s) = v.get("mcpServers") {
            sources.push((format!("projet · {}", dossier.display()), s));
        }
    }
    let mut vus = BTreeSet::new();
    let mut liste = Vec::new();
    for (portee, serveurs) in sources {
        for (nom, brut) in serveurs.as_object().into_iter().flatten() {
            let Some((s, secrets)) = depuis_claude(brut, &portee) else {
                continue;
            };
            // Le même serveur sous deux portées : la première l'emporte.
            if nom_valide(nom).is_err() || !vus.insert(nom.clone()) {
                continue;
            }
            liste.push(Candidat {
                nom: nom.clone(),
                portee: portee.clone(),
                transport: s.transport,
                resume: resume(&s),
                deja: kobi.serveurs.contains_key(nom),
                secrets: secrets.into_iter().map(|(k, _)| k).collect(),
                brut: brut.clone(),
            });
        }
    }
    liste
}

fn lire_json(chemin: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(chemin).ok()?).ok()
}

fn candidats_du_poste(kobi: &config::Config) -> Vec<Candidat> {
    let claude = std::env::home_dir()
        .and_then(|h| lire_json(&h.join(".claude.json")))
        .unwrap_or(Value::Null);
    let projets: Vec<(PathBuf, Value)> = kobi
        .codebases
        .iter()
        .filter_map(|d| Some((d.clone(), lire_json(&d.join(".mcp.json"))?)))
        .collect();
    candidats(&claude, &projets, &kobi.mcp)
}

/// Revue des serveurs importés de Claude Code (KOB-239), une fois : l'import
/// de KOB-155 les cochait tous d'office, et chaque session les chargeait
/// tous sans que l'utilisateur les ait choisis. Ceux qui sont actifs sont
/// désactivés ; les serveurs ajoutés dans Kobi ne bougent pas. Renvoie le
/// nombre de serveurs désactivés.
pub fn revoir_import(config: &mut McpConfig) -> usize {
    if config.import_revu {
        return 0;
    }
    config.import_revu = true;
    let mut desactives = 0;
    for s in config.serveurs.values_mut() {
        let importe = s
            .origine
            .as_deref()
            .is_some_and(|o| o.starts_with("Claude Code"));
        if importe && s.actif {
            s.actif = false;
            desactives += 1;
        }
    }
    if desactives > 0 {
        config.choix_a_proposer = true;
    }
    desactives
}

/// Au démarrage, avant toute session : la revue de [`revoir_import`].
pub fn revoir_import_au_demarrage(path: &Path) {
    let lu = config::load_or_create(path);
    if lu.problem.is_some() || lu.config.mcp.import_revu {
        return;
    }
    let mut c = lu.config;
    let n = revoir_import(&mut c.mcp);
    match config::save(&c, path) {
        Ok(()) if n > 0 => tracing::info!(
            desactives = n,
            "serveurs MCP importés de Claude Code désactivés : à choisir dans Réglages → Outils MCP"
        ),
        Ok(()) => {}
        Err(e) => tracing::warn!(erreur = %e, "revue des serveurs MCP importés non enregistrée"),
    }
}

/// Ajoute `candidat` à `config`, secrets dans `coffre`.
fn importer(
    config: &mut McpConfig,
    candidat: &Candidat,
    coffre: &dyn Coffre,
) -> Result<(), String> {
    let (s, secrets) = depuis_claude(&candidat.brut, &candidat.portee)
        .ok_or_else(|| format!("{} : config illisible", candidat.nom))?;
    for (cle, valeur) in &secrets {
        coffre.ecrire(&candidat.nom, cle, valeur).map_err(|e| {
            format!(
                "Rien n'est importé : le secret {cle} de « {} » n'a pas pu être rangé. {e}",
                candidat.nom
            )
        })?;
    }
    config.serveurs.insert(candidat.nom.clone(), s);
    Ok(())
}

// ----- Test d'un serveur -----

/// Outil vu au test.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OutilVu {
    pub nom: String,
    pub description: String,
    pub nature: NatureOutil,
}

/// Résultat du bouton « Tester ».
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "etat", rename_all = "snake_case")]
pub enum Test {
    Ok {
        outils: Vec<OutilVu>,
    },
    /// Le serveur demande une connexion OAuth que Kobi n'a pas encore.
    Autorisation {
        message: String,
    },
    Erreur {
        message: String,
    },
}

const DELAI_TEST: Duration = Duration::from_secs(30);

enum Echec {
    Autorisation,
    Autre(String),
}

impl<E: std::fmt::Display> From<E> for Echec {
    fn from(e: E) -> Self {
        let texte = e.to_string();
        if texte.contains("Auth required") || texte.contains("401") {
            Echec::Autorisation
        } else {
            Echec::Autre(texte)
        }
    }
}

/// `tools/list` du serveur, par le client MCP de Kobi.
async fn lister(nom: &str, s: &Serveur, coffre: Arc<dyn Coffre>) -> Result<Vec<Value>, Echec> {
    use rmcp::ServiceExt;
    let config = config_claude(s, |cle| coffre.lire(nom, cle).ok().flatten());
    let outils = match s.transport {
        Transport::Stdio => {
            let mut cmd = tokio::process::Command::new(&s.commande);
            cmd.args(&s.args);
            if let Some(env) = config.get("env").and_then(Value::as_object) {
                for (k, v) in env {
                    cmd.env(k, v.as_str().unwrap_or_default());
                }
            }
            #[cfg(windows)]
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            let (transport, _) = rmcp::transport::TokioChildProcess::builder(cmd)
                .stderr(std::process::Stdio::null())
                .spawn()
                .map_err(|e| Echec::Autre(format!("lancement de « {} » : {e}", s.commande)))?;
            let client = ().serve(transport).await.map_err(|e| Echec::Autre(e.to_string()))?;
            let outils = client.list_all_tools().await;
            let _ = client.cancel().await;
            outils?
        }
        Transport::Http => {
            use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
            use rmcp::transport::StreamableHttpClientTransport;
            let mut entetes = std::collections::HashMap::new();
            for (k, v) in config
                .get("headers")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
            {
                let (Ok(k), Ok(v)) = (
                    reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                    reqwest::header::HeaderValue::from_str(v.as_str().unwrap_or_default()),
                ) else {
                    return Err(Echec::Autre(format!("en-tête invalide : {k}")));
                };
                entetes.insert(k, v);
            }
            let conf = StreamableHttpClientTransportConfig::with_uri(s.url.clone())
                .custom_headers(entetes);
            let client = match autorisation_existante(nom, &s.url, coffre.clone()).await {
                Some(manager) => {
                    let http = rmcp::transport::auth::AuthClient::new(reqwest::Client::new(), manager);
                    ().serve(StreamableHttpClientTransport::with_client(http, conf))
                        .await
                        .map_err(Echec::from)?
                }
                None => ().serve(StreamableHttpClientTransport::with_client(reqwest::Client::new(), conf))
                    .await
                    .map_err(Echec::from)?,
            };
            let outils = client.list_all_tools().await;
            let _ = client.cancel().await;
            outils?
        }
        Transport::Sse => {
            return Err(Echec::Autre(
                "Kobi ne sait pas encore tester un serveur SSE : ses outils restent classés en écriture."
                    .into(),
            ))
        }
    };
    Ok(outils
        .iter()
        .filter_map(|o| serde_json::to_value(o).ok())
        .collect())
}

/// Teste le serveur ; en cas de succès, ses outils et leur nature.
pub async fn tester(nom: &str, s: &Serveur, coffre: Arc<dyn Coffre>) -> Test {
    match tokio::time::timeout(DELAI_TEST, lister(nom, s, coffre)).await {
        Err(_) => Test::Erreur {
            message: format!("pas de réponse en {} s", DELAI_TEST.as_secs()),
        },
        Ok(Err(Echec::Autorisation)) => Test::Autorisation {
            message: "Ce serveur demande une connexion. Claude Code peut l'avoir déjà ; pour lire \
                      la liste des outils et savoir lesquels ne font que lire, Kobi a besoin de sa \
                      propre autorisation."
                .into(),
        },
        Ok(Err(Echec::Autre(message))) => Test::Erreur { message },
        Ok(Ok(outils)) => {
            let natures = kobi_perms::mcp::outils(&json!({ "tools": outils }));
            Test::Ok {
                outils: outils
                    .iter()
                    .filter_map(|o| {
                        let nom = o.get("name")?.as_str()?.to_owned();
                        Some(OutilVu {
                            nature: nature_outil(natures[&nom]),
                            description: o
                                .get("description")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .lines()
                                .next()
                                .unwrap_or_default()
                                .to_owned(),
                            nom,
                        })
                    })
                    .collect(),
            }
        }
    }
}

// ----- Autorisation OAuth de Kobi -----

/// L'autorisation OAuth de Kobi pour un serveur, dans le coffre.
struct CoffreOAuth {
    serveur: String,
    coffre: Arc<dyn Coffre>,
}

#[async_trait::async_trait]
impl rmcp::transport::auth::CredentialStore for CoffreOAuth {
    async fn load(
        &self,
    ) -> Result<Option<rmcp::transport::auth::StoredCredentials>, rmcp::transport::AuthError> {
        let erreur = rmcp::transport::AuthError::CredentialStoreError;
        let Some(texte) = self.coffre.lire(&self.serveur, OAUTH).map_err(erreur)? else {
            return Ok(None);
        };
        Ok(serde_json::from_str(&texte).ok())
    }

    async fn save(
        &self,
        credentials: rmcp::transport::auth::StoredCredentials,
    ) -> Result<(), rmcp::transport::AuthError> {
        let erreur = rmcp::transport::AuthError::CredentialStoreError;
        let texte = serde_json::to_string(&credentials).map_err(|e| erreur(e.to_string()))?;
        self.coffre
            .ecrire(&self.serveur, OAUTH, &texte)
            .map_err(erreur)
    }

    async fn clear(&self) -> Result<(), rmcp::transport::AuthError> {
        self.coffre
            .effacer(&self.serveur, OAUTH)
            .map_err(rmcp::transport::AuthError::CredentialStoreError)
    }
}

async fn manager(
    nom: &str,
    url: &str,
    coffre: Arc<dyn Coffre>,
) -> Result<rmcp::transport::auth::AuthorizationManager, String> {
    let mut m = rmcp::transport::auth::AuthorizationManager::new(url)
        .await
        .map_err(|e| e.to_string())?;
    m.set_credential_store(CoffreOAuth {
        serveur: nom.to_owned(),
        coffre,
    });
    Ok(m)
}

/// Autorisation déjà donnée à Kobi pour ce serveur.
async fn autorisation_existante(
    nom: &str,
    url: &str,
    coffre: Arc<dyn Coffre>,
) -> Option<rmcp::transport::auth::AuthorizationManager> {
    // Rien dans le coffre : inutile d'interroger le serveur d'autorisation.
    coffre.lire(nom, OAUTH).ok().flatten()?;
    let mut m = manager(nom, url, coffre).await.ok()?;
    m.initialize_from_store().await.ok()?.then_some(m)
}

/// Délai pour se connecter dans le navigateur.
const DELAI_AUTORISATION: Duration = Duration::from_secs(300);

/// Page montrée dans le navigateur à la fin de l'autorisation.
const PAGE_FIN: &str = "<!doctype html><meta charset=utf-8><title>Kobi</title>\
<body style=\"font-family:system-ui;background:#f6f1e7;color:#2b2622;display:grid;place-items:center;height:90vh\">\
<p>C'est bon, Kobi peut lire les outils de ce serveur. Tu peux fermer cet onglet.</p>";

/// Autorisation OAuth de Kobi : ouvre la page du serveur dans le
/// navigateur, attend le retour sur la boucle locale, garde le jeton dans
/// le coffre.
pub async fn autoriser(nom: &str, url: &str, coffre: Arc<dyn Coffre>) -> Result<(), String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let ecoute = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| e.to_string())?;
    let port = ecoute.local_addr().map_err(|e| e.to_string())?.port();
    let retour = format!("http://127.0.0.1:{port}/callback");
    let mut etat =
        rmcp::transport::auth::OAuthState::Unauthorized(manager(nom, url, coffre).await?);
    etat.start_authorization(
        rmcp::transport::auth::AuthorizationRequest::new(&retour).with_client_name("Kobi"),
    )
    .await
    .map_err(|e| format!("autorisation impossible : {e}"))?;
    let page = etat
        .get_authorization_url()
        .await
        .map_err(|e| e.to_string())?;
    ouvrir_navigateur(&page)?;
    let attente = async {
        loop {
            let (mut flux, _) = ecoute.accept().await.map_err(|e| e.to_string())?;
            let mut tampon = vec![0u8; 8192];
            let n = flux.read(&mut tampon).await.map_err(|e| e.to_string())?;
            let requete = String::from_utf8_lossy(&tampon[..n]);
            let chemin = requete
                .lines()
                .next()
                .and_then(|l| l.split_whitespace().nth(1))
                .unwrap_or_default()
                .to_owned();
            if !chemin.starts_with("/callback") {
                let _ = flux
                    .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")
                    .await;
                continue;
            }
            let reponse = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{PAGE_FIN}",
                PAGE_FIN.len()
            );
            let _ = flux.write_all(reponse.as_bytes()).await;
            return Ok::<String, String>(format!("http://127.0.0.1:{port}{chemin}"));
        }
    };
    let retour = tokio::time::timeout(DELAI_AUTORISATION, attente)
        .await
        .map_err(|_| "pas de connexion dans le navigateur en 5 min".to_owned())??;
    etat.handle_callback_url(&retour)
        .await
        .map_err(|e| format!("autorisation refusée : {e}"))?;
    Ok(())
}

fn ouvrir_navigateur(url: &str) -> Result<(), String> {
    #[cfg(windows)]
    let mut cmd = {
        // Pas de `cmd /c start` : les `&` de l'URL y seraient interprétés.
        let mut c = std::process::Command::new("rundll32");
        c.args(["url.dll,FileProtocolHandler", url]);
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    cmd.spawn()
        .map(|_| ())
        .map_err(|e| format!("navigateur introuvable : {e}"))
}

// ----- Commandes de l'interface -----

/// Serveur tel que le montre Réglages → MCP.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ServeurUi {
    pub nom: String,
    pub transport: Transport,
    pub resume: String,
    pub actif: bool,
    pub ecriture: Ecriture,
    pub origine: Option<String>,
    pub commande: String,
    pub args: Vec<String>,
    pub url: String,
    pub env: BTreeMap<String, String>,
    pub entetes: BTreeMap<String, String>,
    /// Secrets et présence de leur valeur dans le coffre (jamais la valeur).
    pub secrets: Vec<SecretUi>,
    pub outils: Vec<OutilUi>,
    pub teste_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SecretUi {
    pub nom: String,
    pub present: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OutilUi {
    pub nom: String,
    pub nature: NatureOutil,
}

fn ui(nom: &str, s: &Serveur, coffre: &dyn Coffre) -> ServeurUi {
    ServeurUi {
        nom: nom.to_owned(),
        transport: s.transport,
        resume: resume(s),
        actif: s.actif,
        ecriture: s.ecriture,
        origine: s.origine.clone(),
        commande: s.commande.clone(),
        args: s.args.clone(),
        url: s.url.clone(),
        env: s.env.clone(),
        entetes: s.entetes.clone(),
        secrets: s
            .secrets
            .iter()
            .map(|n| SecretUi {
                nom: n.clone(),
                present: coffre.lire(nom, n).ok().flatten().is_some(),
            })
            .collect(),
        outils: s
            .outils
            .iter()
            .map(|(n, nature)| OutilUi {
                nom: n.clone(),
                nature: *nature,
            })
            .collect(),
        teste_ms: s.teste_ms,
    }
}

/// Serveur saisi dans « Ajouter un serveur ».
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct NouveauServeur {
    pub nom: String,
    pub transport: Transport,
    pub commande: String,
    pub args: Vec<String>,
    pub url: String,
    pub env: BTreeMap<String, String>,
    pub entetes: BTreeMap<String, String>,
    /// Valeurs secrètes, rangées dans le coffre.
    pub secrets: BTreeMap<String, String>,
}

/// Changement d'un serveur ; un champ absent reste tel quel.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Changement {
    pub actif: Option<bool>,
    pub ecriture: Option<Ecriture>,
}

fn config_path() -> Result<PathBuf, String> {
    config::default_path().ok_or_else(|| "dossier personnel introuvable".to_owned())
}

/// Lit la config, applique `f`, l'enregistre.
fn modifier_config<T>(
    path: &Path,
    f: impl FnOnce(&mut config::Config) -> Result<T, String>,
) -> Result<T, String> {
    let mut loaded = config::load_or_create(path);
    if let Some(problem) = loaded.problem {
        return Err(format!("réglage non enregistré : {problem}"));
    }
    let r = f(&mut loaded.config)?;
    config::save(&loaded.config, path).map_err(|e| e.to_string())?;
    Ok(r)
}

fn lister_ui(path: &Path, coffre: &dyn Coffre) -> Vec<ServeurUi> {
    let loaded = config::load_or_create(path);
    loaded
        .config
        .mcp
        .serveurs
        .iter()
        .map(|(n, s)| ui(n, s, coffre))
        .collect()
}

fn ajouter(path: &Path, n: NouveauServeur, coffre: &dyn Coffre) -> Result<(), String> {
    let nom = n.nom.trim().to_owned();
    nom_valide(&nom)?;
    let mut s = match n.transport {
        Transport::Stdio => {
            if n.commande.trim().is_empty() {
                return Err("indique la commande qui lance le serveur".into());
            }
            Serveur::stdio(n.commande.trim(), n.args)
        }
        Transport::Http | Transport::Sse => {
            let url = n.url.trim();
            if !(url.starts_with("https://") || url.starts_with("http://")) {
                return Err("l'adresse commence par https://".into());
            }
            let mut s = Serveur::http(url);
            s.transport = n.transport;
            s
        }
    };
    s.env = n.env;
    s.entetes = n.entetes;
    s.origine = Some("ajouté dans Kobi".into());
    modifier_config(path, |c| {
        if c.mcp.serveurs.contains_key(&nom) {
            return Err(format!("« {nom} » existe déjà"));
        }
        for (cle, valeur) in n.secrets.iter().filter(|(_, v)| !v.is_empty()) {
            coffre.ecrire(&nom, cle, valeur)?;
            s.secrets.push(cle.clone());
        }
        c.mcp.serveurs.insert(nom.clone(), s);
        Ok(())
    })
}

fn supprimer(path: &Path, nom: &str, coffre: &dyn Coffre) -> Result<(), String> {
    modifier_config(path, |c| {
        let s = c
            .mcp
            .serveurs
            .remove(nom)
            .ok_or_else(|| format!("serveur inconnu : {nom}"))?;
        for cle in s.secrets.iter().map(String::as_str).chain([OAUTH]) {
            coffre.effacer(nom, cle)?;
        }
        Ok(())
    })
}

fn changer(path: &Path, nom: &str, ch: Changement) -> Result<(), String> {
    modifier_config(path, |c| {
        let s = c
            .mcp
            .serveurs
            .get_mut(nom)
            .ok_or_else(|| format!("serveur inconnu : {nom}"))?;
        if let Some(actif) = ch.actif {
            s.actif = actif;
        }
        if let Some(e) = ch.ecriture {
            s.ecriture = e;
        }
        Ok(())
    })
}

/// Garde les outils vus par un test réussi.
fn retenir_test(path: &Path, nom: &str, test: &Test) -> Result<(), String> {
    let Test::Ok { outils } = test else {
        return Ok(());
    };
    modifier_config(path, |c| {
        if let Some(s) = c.mcp.serveurs.get_mut(nom) {
            s.outils = outils.iter().map(|o| (o.nom.clone(), o.nature)).collect();
            s.teste_ms = Some(maintenant_ms());
        }
        Ok(())
    })
}

fn serveur(path: &Path, nom: &str) -> Result<Serveur, String> {
    config::load_or_create(path)
        .config
        .mcp
        .serveurs
        .get(nom)
        .cloned()
        .ok_or_else(|| format!("serveur inconnu : {nom}"))
}

#[tauri::command]
pub fn mcp_serveurs() -> Result<Vec<ServeurUi>, String> {
    Ok(lister_ui(&config_path()?, &CoffreSysteme))
}

/// Les changements valent pour la prochaine conversation de Kobi.
#[tauri::command]
pub fn mcp_ajouter(serveur: NouveauServeur) -> Result<Vec<ServeurUi>, String> {
    let path = config_path()?;
    let nom = serveur.nom.clone();
    ajouter(&path, serveur, &CoffreSysteme)?;
    tracing::info!(serveur = %nom, "MCP ajouté");
    Ok(lister_ui(&path, &CoffreSysteme))
}

#[tauri::command]
pub fn mcp_supprimer(nom: String) -> Result<Vec<ServeurUi>, String> {
    let path = config_path()?;
    supprimer(&path, &nom, &CoffreSysteme)?;
    tracing::info!(serveur = %nom, "MCP retiré");
    Ok(lister_ui(&path, &CoffreSysteme))
}

#[tauri::command]
pub fn mcp_regler(nom: String, changement: Changement) -> Result<Vec<ServeurUi>, String> {
    let path = config_path()?;
    changer(&path, &nom, changement)?;
    Ok(lister_ui(&path, &CoffreSysteme))
}

#[tauri::command]
pub async fn mcp_tester(nom: String) -> Result<Test, String> {
    let path = config_path()?;
    let s = serveur(&path, &nom)?;
    let test = tester(&nom, &s, Arc::new(CoffreSysteme)).await;
    match &test {
        Test::Ok { outils } => tracing::info!(serveur = %nom, outils = outils.len(), "MCP testé"),
        autre => tracing::warn!(serveur = %nom, ?autre, "test MCP"),
    }
    retenir_test(&path, &nom, &test)?;
    Ok(test)
}

/// Autorisation OAuth de Kobi, puis nouveau test.
#[tauri::command]
pub async fn mcp_autoriser(nom: String) -> Result<Test, String> {
    let path = config_path()?;
    let s = serveur(&path, &nom)?;
    autoriser(&nom, &s.url, Arc::new(CoffreSysteme)).await?;
    tracing::info!(serveur = %nom, "Kobi autorisé sur le serveur MCP");
    mcp_tester(nom).await
}

/// Serveurs de Claude Code qu'on peut importer.
#[tauri::command]
pub fn mcp_candidats() -> Result<Vec<Candidat>, String> {
    Ok(candidats_du_poste(
        &config::load_or_create(&config_path()?).config,
    ))
}

/// L'app doit-elle proposer, une fois, de choisir les serveurs MCP
/// désactivés par la revue de KOB-239 ? La question n'est posée qu'une fois :
/// le drapeau tombe à la lecture, que la notification ait pu s'afficher ou non.
#[tauri::command]
pub fn mcp_choix_a_proposer() -> Result<bool, String> {
    let path = config_path()?;
    modifier_config(&path, |c| {
        let proposer = c.mcp.choix_a_proposer;
        c.mcp.choix_a_proposer = false;
        Ok(proposer)
    })
}

/// Importe les serveurs choisis, puis les teste. Renvoie la liste à jour.
#[tauri::command]
pub async fn mcp_importer(noms: Vec<String>) -> Result<Vec<ServeurUi>, String> {
    let path = config_path()?;
    let coffre = CoffreSysteme;
    modifier_config(&path, |c| {
        let liste = candidats_du_poste(c);
        for nom in &noms {
            let candidat = liste
                .iter()
                .find(|k| &k.nom == nom)
                .ok_or_else(|| format!("{nom} : introuvable dans Claude Code"))?;
            importer(&mut c.mcp, candidat, &coffre)?;
        }
        // Serveurs cochés un à un : c'est le choix de l'utilisateur.
        c.mcp.import_revu = true;
        Ok(())
    })?;
    tracing::info!(?noms, "MCP importés depuis Claude Code");
    // Test de chaque serveur importé : les annotations classent ses outils
    // tout de suite. Peu de serveurs, chacun avec son délai : à la suite.
    for nom in &noms {
        let s = serveur(&path, nom)?;
        let test = tester(nom, &s, Arc::new(CoffreSysteme)).await;
        retenir_test(&path, nom, &test)?;
    }
    Ok(lister_ui(&path, &coffre))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct CoffreMemoire(Mutex<BTreeMap<String, String>>);

    impl Coffre for CoffreMemoire {
        fn lire(&self, s: &str, n: &str) -> Result<Option<String>, String> {
            Ok(self.0.lock().unwrap().get(&format!("{s}/{n}")).cloned())
        }
        fn ecrire(&self, s: &str, n: &str, v: &str) -> Result<(), String> {
            self.0.lock().unwrap().insert(format!("{s}/{n}"), v.into());
            Ok(())
        }
        fn effacer(&self, s: &str, n: &str) -> Result<(), String> {
            self.0.lock().unwrap().remove(&format!("{s}/{n}"));
            Ok(())
        }
    }

    fn claude_json() -> Value {
        json!({
            "oauthAccount": { "emailAddress": "jamais lu" },
            "mcpServers": {
                "linear": { "type": "http", "url": "https://mcp.linear.app/mcp" },
                "treg": { "type": "http", "url": "https://treg.to/mcp/?k=1", "headers": { "Authorization": "Bearer s3cr3t" } },
                "magic": { "command": "npx", "args": ["-y", "@21st/magic"], "env": { "API_KEY": "cle", "UV_HTTP_TIMEOUT": "60", "REF": "${HOME}" } },
                "bizarre": { "type": "ws", "url": "wss://x" },
                "kobi": { "command": "kobi", "args": ["mcp"] }
            },
            "projects": {
                "C:/Users/armag": { "mcpServers": {
                    "linear-server": { "type": "http", "url": "https://mcp.linear.app/mcp" },
                    "linear": { "type": "http", "url": "https://autre" }
                } }
            }
        })
    }

    #[test]
    fn candidats_de_claude_code() {
        let projet = (
            PathBuf::from("C:/code/site"),
            json!({ "mcpServers": { "notes": { "command": "node", "args": ["notes.js"] } } }),
        );
        let mut kobi = McpConfig::default();
        kobi.serveurs
            .insert("magic".into(), Serveur::stdio("npx", vec![]));
        let c = candidats(&claude_json(), &[projet], &kobi);
        let noms: Vec<_> = c
            .iter()
            .map(|k| (k.nom.as_str(), k.portee.as_str()))
            .collect();
        assert_eq!(
            noms,
            [
                ("linear", "utilisateur"),
                ("magic", "utilisateur"),
                ("treg", "utilisateur"),
                ("linear-server", "local · C:/Users/armag"),
                ("notes", "projet · C:/code/site"),
            ]
        );
        let treg = &c[2];
        // Ni la requête de l'URL ni la valeur de l'en-tête.
        assert_eq!(treg.resume, "https://treg.to/mcp/");
        assert_eq!(treg.secrets, ["Authorization"]);
        assert!(c[1].deja && !c[0].deja);
        assert_eq!(c[1].secrets, ["API_KEY"]);
        let affiche = serde_json::to_string(&c).unwrap();
        assert!(
            !affiche.contains("s3cr3t") && !affiche.contains("\"cle\""),
            "{affiche}"
        );
    }

    /// KOB-239 : les serveurs importés d'office avant sont désactivés une
    /// fois ; ceux ajoutés dans Kobi ne bougent pas.
    #[test]
    fn revue_des_serveurs_importes() {
        let mut c = McpConfig::default();
        let mut importe = Serveur::stdio("npx", vec![]);
        importe.origine = Some("Claude Code (utilisateur)".into());
        c.serveurs.insert("linear".into(), importe.clone());
        let mut deja_coupe = importe.clone();
        deja_coupe.actif = false;
        c.serveurs.insert("treg".into(), deja_coupe);
        let mut ajoute = Serveur::stdio("node", vec![]);
        ajoute.origine = Some("ajouté dans Kobi".into());
        c.serveurs.insert("notes".into(), ajoute);
        assert_eq!(revoir_import(&mut c), 1);
        assert!(!c.serveurs["linear"].actif);
        assert!(!c.serveurs["treg"].actif);
        assert!(c.serveurs["notes"].actif);
        assert!(c.import_revu && c.choix_a_proposer);
        // Une seule fois : un serveur réactivé ensuite reste actif.
        c.serveurs.get_mut("linear").unwrap().actif = true;
        assert_eq!(revoir_import(&mut c), 0);
        assert!(c.serveurs["linear"].actif);
        // Rien à revoir : rien à proposer.
        let mut vide = McpConfig::default();
        assert_eq!(revoir_import(&mut vide), 0);
        assert!(vide.import_revu && !vide.choix_a_proposer);
    }

    /// Au démarrage : écrite une fois ; une config illisible n'est pas touchée.
    #[test]
    fn revue_au_demarrage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut cfg = config::Config::default();
        let mut importe = Serveur::stdio("npx", vec![]);
        importe.origine = Some("Claude Code (utilisateur)".into());
        cfg.mcp.serveurs.insert("linear".into(), importe);
        config::save(&cfg, &path).unwrap();
        revoir_import_au_demarrage(&path);
        let lu = config::load_or_create(&path).config;
        assert!(!lu.mcp.serveurs["linear"].actif);
        assert!(lu.mcp.import_revu && lu.mcp.choix_a_proposer);
        let illisible = dir.path().join("illisible.toml");
        std::fs::write(&illisible, "mcp = [ pas du toml").unwrap();
        revoir_import_au_demarrage(&illisible);
        assert_eq!(
            std::fs::read_to_string(&illisible).unwrap(),
            "mcp = [ pas du toml"
        );
    }

    #[test]
    fn import_secrets_dans_le_coffre() {
        let coffre = CoffreMemoire::default();
        let mut kobi = McpConfig::default();
        for k in candidats(&claude_json(), &[], &kobi.clone()) {
            importer(&mut kobi, &k, &coffre).unwrap();
        }
        let treg = &kobi.serveurs["treg"];
        assert_eq!(treg.transport, Transport::Http);
        assert_eq!(treg.secrets, ["Authorization"]);
        assert!(treg.entetes.is_empty());
        assert_eq!(treg.origine.as_deref(), Some("Claude Code (utilisateur)"));
        let magic = &kobi.serveurs["magic"];
        assert_eq!(magic.env["UV_HTTP_TIMEOUT"], "60");
        assert_eq!(magic.env["REF"], "${HOME}");
        assert_eq!(magic.secrets, ["API_KEY"]);
        assert_eq!(
            coffre.lire("treg", "Authorization").unwrap().as_deref(),
            Some("Bearer s3cr3t")
        );
        assert_eq!(
            coffre.lire("magic", "API_KEY").unwrap().as_deref(),
            Some("cle")
        );
        // Le fichier de réglages ne voit aucun secret.
        let toml = toml::to_string(&kobi).unwrap();
        assert!(
            !toml.contains("s3cr3t") && !toml.contains("\"cle\""),
            "{toml}"
        );
    }

    /// Coffre qui refuse tout, comme un trousseau absent ou verrouillé.
    struct CoffreRefuse;

    impl Coffre for CoffreRefuse {
        fn lire(&self, _: &str, _: &str) -> Result<Option<String>, String> {
            Ok(None)
        }
        fn ecrire(&self, _: &str, _: &str, _: &str) -> Result<(), String> {
            Err(erreur_coffre(keyring::Error::PlatformFailure(
                std::io::Error::from(std::io::ErrorKind::PermissionDenied).into(),
            )))
        }
        fn effacer(&self, _: &str, _: &str) -> Result<(), String> {
            Ok(())
        }
    }

    /// Gestionnaire d'identifiants inaccessible (KOB-185) : un message en
    /// français qui dit quoi faire, et le serveur n'est pas ajouté.
    #[test]
    fn import_sans_gestionnaire_d_identifiants() {
        let mut kobi = McpConfig::default();
        let treg = candidats(&claude_json(), &[], &kobi.clone())
            .into_iter()
            .find(|k| k.nom == "treg")
            .unwrap();
        let e = importer(&mut kobi, &treg, &CoffreRefuse).unwrap_err();
        assert!(e.starts_with("Rien n'est importé"), "{e}");
        assert!(e.contains("« treg »") && e.contains("Authorization"), "{e}");
        assert!(
            e.contains("refuse l'accès") && e.contains("puis réessaie"),
            "{e}"
        );
        assert!(!e.contains("Platform secure storage"), "{e}");
        assert!(kobi.serveurs.is_empty());
        // Sans secret, le coffre n'est pas sollicité : l'import passe.
        let linear = candidats(&claude_json(), &[], &kobi.clone())
            .into_iter()
            .find(|k| k.nom == "linear")
            .unwrap();
        importer(&mut kobi, &linear, &CoffreRefuse).unwrap();
        assert!(kobi.serveurs.contains_key("linear"));
        // Les autres erreurs gardent leur détail.
        assert!(erreur_coffre(keyring::Error::TooLong("x".into(), 1)).contains("gestionnaire"));
    }

    #[test]
    fn session_avec_secrets_par_l_environnement() {
        let coffre = CoffreMemoire::default();
        let mut kobi = McpConfig::default();
        for k in candidats(&claude_json(), &[], &kobi.clone()) {
            importer(&mut kobi, &k, &coffre).unwrap();
        }
        kobi.serveurs.get_mut("magic").unwrap().actif = false;
        let linear = kobi.serveurs.get_mut("linear").unwrap();
        linear
            .outils
            .insert("list_issues".into(), NatureOutil::Lecture);
        linear
            .outils
            .insert("save_issue".into(), NatureOutil::Ecriture);
        let s = pour_session(&kobi, &coffre);
        assert_eq!(s.noms(), ["linear", "linear-server", "treg"]);
        assert_eq!(
            s.serveurs[0].1,
            json!({ "type": "http", "url": "https://mcp.linear.app/mcp" })
        );
        assert_eq!(
            s.serveurs[2].1["headers"]["Authorization"],
            "${KOBI_MCP_TREG_AUTHORIZATION}"
        );
        assert_eq!(
            s.env,
            [(
                "KOBI_MCP_TREG_AUTHORIZATION".to_owned(),
                "Bearer s3cr3t".to_owned()
            )]
        );
        let regles = s.catalogue.regles();
        assert!(matches!(
            regles.verdict("mcp__linear__list_issues"),
            kobi_perms::Verdict::Allow(_)
        ));
        assert_eq!(
            regles.verdict("mcp__linear__save_issue"),
            kobi_perms::Verdict::Ask
        );
    }

    #[test]
    fn reglages_ajout_changement_suppression() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let coffre = CoffreMemoire::default();
        let n = NouveauServeur {
            nom: "notes".into(),
            transport: Transport::Http,
            url: "https://notes.example/mcp".into(),
            secrets: BTreeMap::from([("Authorization".into(), "Bearer x".into())]),
            ..Default::default()
        };
        ajouter(&path, n.clone(), &coffre).unwrap();
        assert!(ajouter(&path, n, &coffre).is_err(), "doublon");
        assert!(ajouter(
            &path,
            NouveauServeur {
                nom: "a b".into(),
                ..Default::default()
            },
            &coffre
        )
        .is_err());
        let texte = std::fs::read_to_string(&path).unwrap();
        assert!(
            texte.contains("[mcp.serveurs.notes]") && !texte.contains("Bearer"),
            "{texte}"
        );
        changer(
            &path,
            "notes",
            Changement {
                actif: Some(false),
                ecriture: Some(Ecriture::SansDemander),
            },
        )
        .unwrap();
        let ui = lister_ui(&path, &coffre);
        assert_eq!(
            (ui[0].actif, ui[0].ecriture),
            (false, Ecriture::SansDemander)
        );
        assert_eq!(
            ui[0].secrets,
            [SecretUi {
                nom: "Authorization".into(),
                present: true
            }]
        );
        supprimer(&path, "notes", &coffre).unwrap();
        assert!(lister_ui(&path, &coffre).is_empty());
        assert_eq!(coffre.lire("notes", "Authorization").unwrap(), None);
    }

    #[test]
    fn variables_des_secrets() {
        assert_eq!(
            variable("linear-server", "Authorization"),
            "KOBI_MCP_LINEAR_SERVER_AUTHORIZATION"
        );
        assert!(env_secrete("GOOGLE_API_KEY") && env_secrete("github_token"));
        assert!(!env_secrete("UV_HTTP_TIMEOUT") && !env_secrete("GOOGLE_PROJECT_ID"));
    }

    /// Serveur stdio écrit en JavaScript (node), avec des annotations.
    const SERVEUR_ESSAI: &str = r#"
const rl = require("readline").createInterface({ input: process.stdin });
rl.on("line", (l) => {
  const m = JSON.parse(l);
  if (m.id === undefined) return;
  let r = {};
  if (m.method === "initialize") r = { protocolVersion: m.params.protocolVersion, capabilities: { tools: {} }, serverInfo: { name: "essai", version: "1" } };
  else if (m.method === "tools/list") r = { tools: [
    { name: "lister", description: "Liste.\nDétail", inputSchema: { type: "object" }, annotations: { readOnlyHint: true } },
    { name: "creer", inputSchema: { type: "object" }, annotations: { readOnlyHint: false, destructiveHint: false } },
    { name: "effacer", inputSchema: { type: "object" }, annotations: { destructiveHint: true } },
    { name: "jeton_" + (process.env.JETON === "abc" ? "ok" : "absent"), inputSchema: { type: "object" } } ] };
  process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: m.id, result: r }) + "\n");
});
"#;

    #[tokio::test]
    async fn test_d_un_serveur_stdio() {
        if std::process::Command::new("node")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("node absent : test ignoré");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("serveur.js");
        std::fs::write(&script, SERVEUR_ESSAI).unwrap();
        let mut s = Serveur::stdio("node", vec![script.display().to_string()]);
        s.secrets.push("JETON".into());
        let coffre = Arc::new(CoffreMemoire::default());
        coffre.ecrire("essai", "JETON", "abc").unwrap();
        let Test::Ok { outils } = tester("essai", &s, coffre).await else {
            panic!("test en échec");
        };
        let vus: Vec<_> = outils.iter().map(|o| (o.nom.as_str(), o.nature)).collect();
        assert_eq!(
            vus,
            [
                ("lister", NatureOutil::Lecture),
                ("creer", NatureOutil::Ecriture),
                ("effacer", NatureOutil::Suppression),
                ("jeton_ok", NatureOutil::Ecriture),
            ]
        );
        assert_eq!(outils[0].description, "Liste.");
    }

    /// Essai contre le vrai Claude Code et le vrai Linear (KOB-155), tel que
    /// configuré dans Claude Code (connexion OAuth de Claude Code, même nom
    /// et même URL) : une lecture passe sans demande, une création de ticket
    /// arrive au serveur de permission de Kobi, qui la refuse ici. Ignoré par
    /// défaut : `cargo test -p kobi-app -- --ignored linear_reel --nocapture`.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "demande Claude Code connecté, avec Linear"]
    async fn linear_reel() {
        use kobi_brain::{Ask, Brain, BrainEvent, ClaudeCodeBrain, Detection, MeetingConfig};
        let program = match kobi_brain::claude_code::detect::detect(Duration::from_secs(30)).await {
            Detection::Ready { install, .. } => install.program,
            autre => panic!("Claude Code indisponible : {autre:?}"),
        };
        let mut kobi = McpConfig::default();
        let mut linear = Serveur::http("https://mcp.linear.app/mcp");
        // Sans l'autorisation propre de Kobi, les annotations ne sont pas
        // lues : l'essai marque à la main les outils de lecture courants.
        for o in [
            "list_issues",
            "get_issue",
            "list_teams",
            "get_team",
            "list_issue_statuses",
            "get_user",
            "list_users",
            "list_projects",
            "list_cycles",
        ] {
            linear.outils.insert(o.into(), NatureOutil::Lecture);
        }
        linear
            .outils
            .insert("save_issue".into(), NatureOutil::Ecriture);
        kobi.serveurs.insert("linear".into(), linear);
        let session = pour_session(&kobi, &CoffreMemoire::default());
        let dir = tempfile::tempdir().unwrap();
        let fichier = dir.path().join("session.json");
        let serveurs = session.serveurs.clone();
        let prompt = prompt(&session.noms());
        let f = fichier.clone();
        let args = kobi_brain::ArgsWithPerms(Arc::new(move |perms: &Value| {
            let mut tous: Map<String, Value> = serveurs.iter().cloned().collect();
            tous.insert(kobi_perms::SERVER_NAME.into(), perms.clone());
            std::fs::write(&f, json!({ "mcpServers": tous }).to_string()).unwrap();
            let mcp = crate::mcp_config::McpSession {
                fichier: f.clone(),
                outils_autorises: Vec::new(),
                permission_prompt_tool: Some(kobi_perms::PERMISSION_PROMPT_TOOL.into()),
                prompt: Some(prompt.clone()),
            };
            Ok(crate::lecture::arguments(
                "haiku",
                kobi_brain::SYSTEM_PROMPT,
                &[],
                None,
                Some(&mcp),
                false,
            ))
        }));
        let mut brain = ClaudeCodeBrain::start(MeetingConfig {
            program,
            cwd: Some(dir.path().to_owned()),
            args_with_perms: Some(args),
            permission_rules: session.catalogue.regles(),
            ..Default::default()
        })
        .await
        .unwrap();
        async fn tour(
            brain: &mut ClaudeCodeBrain,
            question: &str,
        ) -> (String, Vec<String>, Vec<String>) {
            let mut turn = brain.ask(Ask {
                question: question.into(),
                transcript: String::new(),
                notes: String::new(),
            });
            let (mut texte, mut outils, mut demandes) = (String::new(), Vec::new(), Vec::new());
            while let Some(e) = turn.next().await {
                match e {
                    BrainEvent::Text(t) => texte.push_str(&t),
                    BrainEvent::OutilRendu { .. } | BrainEvent::Vie => {}
                    BrainEvent::ToolCall { name, .. } => outils.push(name),
                    BrainEvent::Permission(p) => {
                        demandes.push(p.request.tool_name.clone());
                        p.deny("Refusé par l'essai KOB-155.");
                    }
                    BrainEvent::Done(_) => break,
                    BrainEvent::Error(e) => panic!("{e}"),
                }
            }
            (texte, outils, demandes)
        }
        let (lecture, o1, d1) = tour(
            &mut brain,
            "Quels sont mes tickets Linear en cours (statut In Progress) dans l'équipe Kobi ? Donne juste les identifiants.",
        )
        .await;
        println!("lecture -> {lecture}\n  outils {o1:?}\n  demandes {d1:?}");
        let (ecriture, o2, d2) = tour(
            &mut brain,
            "Crée un ticket Linear dans l'équipe Kobi intitulé « Essai KOB-155, à supprimer ».",
        )
        .await;
        println!("écriture -> {ecriture}\n  outils {o2:?}\n  demandes {d2:?}");
        brain.close(Duration::from_secs(5)).await;
        assert!(o1.iter().any(|o| o.starts_with("mcp__linear__")), "{o1:?}");
        assert!(d1.is_empty(), "lecture sans carte : {d1:?}");
        assert!(
            d2.iter().any(|o| o == "mcp__linear__save_issue"),
            "écriture par la carte : {d2:?}"
        );
    }

    #[tokio::test]
    async fn test_d_un_serveur_absent() {
        let s = Serveur::stdio("kobi-serveur-introuvable", vec![]);
        assert!(matches!(
            tester("x", &s, Arc::new(CoffreMemoire::default())).await,
            Test::Erreur { .. }
        ));
    }
}
