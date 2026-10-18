//! Interpellation de bout en bout (KOB-67, KOB-132) : de la question
//! repérée dans l'écoute continue à la réponse dans la bulle.
//!
//! Un fil dédié garde une session Claude Code ouverte (config isolée, prompt
//! système de Kobi), pour que chaque question ne paie pas le démarrage de
//! `claude`. Il reçoit les questions du fil d'écoute ([`crate::ecoute`]), qui
//! garde le modèle Nemotron chargé et le micro ouvert, et pousse la réponse
//! en flux vers la webview.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use kobi_brain::{ClaudeCode, ClaudeOptions, Detection, SYSTEM_PROMPT};
use kobi_session::etat::Evenement;
use kobi_stt::nemotron::{NemotronConfig, NemotronEngine};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime};

use crate::ecoute::{self, Commande};
use crate::session::appliquer;

/// Question en cours de transcription (texte complet à chaque fois).
pub const EVENEMENT_QUESTION: &str = "question";
/// Réponse en flux (texte accumulé à chaque fois).
pub const EVENEMENT_REPONSE: &str = "answer";
/// Erreur à afficher dans la carte d'erreur.
pub const EVENEMENT_ERREUR: &str = "erreur";

/// Temps de lecture minimal d'une réponse avant le retour au calme.
const LECTURE_MIN: Duration = Duration::from_secs(5);
/// Temps d'affichage d'une erreur.
const ERREUR_AFFICHEE: Duration = Duration::from_secs(6);
/// Threads de Nemotron. L'écoute est continue : mesuré sur ce PC (6 cœurs)
/// en parole continue, 4 fils prennent 1,6 cœur, 2 fils 0,8 et 1 fil 0,45
/// (KOB-132, cible : moins d'un cœur).
const THREADS_STT: i32 = 2;
/// Pénalité du symbole vide de Nemotron, voir [`NemotronConfig`]. Mesuré
/// sur un corpus synthétique (KOB-132) : sans elle, « Kobi » n'est
/// presque jamais écrit ; à 2, 16 phrases sur 18 sont repérées, aucune
/// fausse alerte sur 4 min de réunion, et les mots sautés baissent (erreurs
/// de mots 10 % → 4 %).
const BLANK_PENALTY: f32 = 2.0;
/// Modèle des interpellations (KOB-30, KOB-123 : ~1 s au premier mot).
const MODELE_CLAUDE: &str = "haiku";
/// Outils interdits pendant une réunion (KOB-61).
const OUTILS_INTERDITS: &str = "Bash,Edit,Write,NotebookEdit,MultiEdit";

#[derive(Debug, Clone, Serialize)]
struct ErreurUi {
    title: String,
    message: String,
}

/// Dossier de travail de Kobi (`~/Kobi`).
fn kobi_dir() -> Option<PathBuf> {
    kobi_session::config::kobi_dir()
}

/// Paquet Nemotron : `KOBI_NEMOTRON`, sinon le premier
/// `sherpa-onnx-nemotron-*` de `~/Kobi/models`.
pub fn modele_nemotron() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("KOBI_NEMOTRON") {
        return Some(PathBuf::from(dir));
    }
    let models = kobi_dir()?.join("models");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(models)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_dir()
                && p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("sherpa-onnx-nemotron"))
        })
        .collect();
    dirs.sort();
    dirs.into_iter().next()
}

/// Lance le fil d'interpellation et le fil d'écoute qui l'alimente.
pub fn installer<R: Runtime>(app: &AppHandle<R>) {
    let (tx, rx) = mpsc::channel();
    let fil = app.clone();
    std::thread::Builder::new()
        .name("interpellation".into())
        .spawn(move || travailleur(fil, rx))
        .expect("fil d'interpellation");
    ecoute::installer(app, tx);
}

/// Interpelle Kobi depuis le raccourci, secours du mot-clé : la phrase
/// suivante est la question.
pub fn declencher<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    ecoute::commander(app, Commande::Raccourci)
}

fn travailleur<R: Runtime>(app: AppHandle<R>, rx: Receiver<String>) {
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!(erreur = %e, "runtime tokio indisponible");
            return;
        }
    };

    let mut claude = rt.block_on(demarrer_claude());
    match &claude {
        Ok(_) => tracing::info!("session Claude Code prête"),
        Err(e) => tracing::warn!(erreur = %e, "Claude Code indisponible"),
    }

    while let Ok(question) = rx.recv() {
        repondre(&app, &rt, &mut claude, &question);
    }
}

pub fn charger_stt() -> Result<NemotronEngine, String> {
    let dir = modele_nemotron().ok_or_else(|| {
        "modèle Nemotron introuvable : placer le paquet sherpa-onnx-nemotron-… dans ~/Kobi/models"
            .to_string()
    })?;
    let mut config = NemotronConfig::new(&dir);
    config.num_threads = std::env::var("KOBI_STT_THREADS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(THREADS_STT);
    // Sans pénalité, Nemotron saute « Kobi », nom inconnu de son vocabulaire
    // (KOB-132).
    config.blank_penalty = std::env::var("KOBI_BLANK_PENALTY")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(BLANK_PENALTY);
    // Langue imposée (`fr`, `en`…) ; détection automatique sinon.
    config.language = std::env::var("KOBI_LANGUE").ok().filter(|l| !l.is_empty());
    let debut = Instant::now();
    let engine = NemotronEngine::new(config).map_err(|e| e.to_string())?;
    tracing::info!(modele = %dir.display(), ms = debut.elapsed().as_millis() as u64, "Nemotron chargé");
    Ok(engine)
}

async fn demarrer_claude() -> Result<ClaudeCode, String> {
    let program = match kobi_brain::claude_code::detect::detect(Duration::from_secs(30)).await {
        Detection::Ready { install, .. } => install.program,
        Detection::NotLoggedIn(_) => {
            return Err("Claude Code n'est pas connecté : lancer « claude auth login »".into())
        }
        autre => return Err(format!("Claude Code indisponible : {autre:?}")),
    };
    let cwd = kobi_dir().ok_or("dossier personnel introuvable")?;
    std::fs::create_dir_all(&cwd).map_err(|e| e.to_string())?;
    let options = ClaudeOptions {
        program,
        cwd: Some(cwd),
        partial_messages: true,
        args: [
            "--model",
            MODELE_CLAUDE,
            "--setting-sources",
            "project",
            "--strict-mcp-config",
            "--disallowedTools",
            OUTILS_INTERDITS,
            "--append-system-prompt",
            SYSTEM_PROMPT,
        ]
        .map(String::from)
        .into(),
    };
    ClaudeCode::spawn(&options).map_err(|e| e.to_string())
}

fn repondre<R: Runtime>(
    app: &AppHandle<R>,
    rt: &tokio::runtime::Runtime,
    claude: &mut Result<ClaudeCode, String>,
    question: &str,
) {
    if claude.is_err() {
        *claude = rt.block_on(demarrer_claude());
    }
    let session = match claude {
        Ok(session) => session,
        Err(e) => {
            let e = e.clone();
            erreur(app, "Claude Code indisponible", &e);
            return;
        }
    };

    let message = format!("[interpellation]\n\n## Question\n\n{question}\n");
    let mut texte = String::new();
    let mut commence = false;
    let resultat = rt.block_on(session.ask(&message, |delta| {
        if !commence {
            commence = true;
            let _ = appliquer(app, &[Evenement::ReponseCommence]);
        }
        texte.push_str(delta);
        let _ = app.emit(EVENEMENT_REPONSE, &texte);
    }));
    match resultat {
        Ok(_) => {
            // Le temps de lire la réponse, puis retour au calme.
            let lecture = LECTURE_MIN + Duration::from_millis(60 * texte.chars().count() as u64);
            std::thread::sleep(lecture.min(Duration::from_secs(25)));
            let _ = appliquer(app, &[Evenement::ReponseTerminee]);
        }
        Err(e) => {
            let message = e.to_string();
            // La session sera relancée à la prochaine question.
            *claude = Err(message.clone());
            erreur(app, "Claude n'a pas répondu", &message);
        }
    }
}

pub fn erreur<R: Runtime>(app: &AppHandle<R>, titre: &str, message: &str) {
    tracing::warn!(%titre, %message, "interpellation en erreur");
    let _ = appliquer(app, &[Evenement::Erreur]);
    let _ = app.emit(
        EVENEMENT_ERREUR,
        ErreurUi {
            title: titre.into(),
            message: message.into(),
        },
    );
    std::thread::sleep(ERREUR_AFFICHEE);
    let _ = appliquer(app, &[Evenement::Acquitter]);
}
