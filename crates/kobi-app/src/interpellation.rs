//! Interpellation de bout en bout (KOB-67) : raccourci, micro, Nemotron,
//! Claude Code, réponse dans la bulle.
//!
//! Un fil dédié garde le modèle Nemotron chargé et une session Claude Code
//! ouverte (config isolée, prompt système de Kobi), pour que chaque question
//! ne paie ni le chargement du modèle ni le démarrage de `claude`. Le
//! raccourci lui envoie un signal ; il écoute le micro jusqu'à la fin de la
//! phrase, envoie la question et pousse la réponse en flux vers la webview.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use kobi_audio::{Capture, CaptureOptions};
use kobi_brain::{ClaudeCode, ClaudeOptions, Detection, SYSTEM_PROMPT};
use kobi_session::etat::Evenement;
use kobi_stt::nemotron::{NemotronConfig, NemotronEngine};
use kobi_stt::SttEngine;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::session::{appliquer, etat};

/// Question en cours de transcription (texte complet à chaque fois).
pub const EVENEMENT_QUESTION: &str = "question";
/// Réponse en flux (texte accumulé à chaque fois).
pub const EVENEMENT_REPONSE: &str = "answer";
/// Erreur à afficher dans la carte d'erreur.
pub const EVENEMENT_ERREUR: &str = "erreur";

/// Durée maximale d'une question.
const ECOUTE_MAX: Duration = Duration::from_secs(15);
/// Sans aucun mot reconnu après ce délai, l'écoute s'arrête.
const SANS_PAROLE: Duration = Duration::from_secs(6);
/// Temps de lecture minimal d'une réponse avant le retour au calme.
const LECTURE_MIN: Duration = Duration::from_secs(5);
/// Temps d'affichage d'une erreur.
const ERREUR_AFFICHEE: Duration = Duration::from_secs(6);
/// Threads de Nemotron : la moitié d'un processeur de bureau courant.
const THREADS_STT: i32 = 4;
/// Modèle des interpellations (KOB-30, KOB-123 : ~1 s au premier mot).
const MODELE_CLAUDE: &str = "haiku";
/// Outils interdits pendant une réunion (KOB-61).
const OUTILS_INTERDITS: &str = "Bash,Edit,Write,NotebookEdit,MultiEdit";

/// Canal vers le fil d'interpellation.
pub struct Pilote(Mutex<Sender<()>>);

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

/// Lance le fil d'interpellation.
pub fn installer<R: Runtime>(app: &AppHandle<R>) {
    let (tx, rx) = mpsc::channel();
    app.manage(Pilote(Mutex::new(tx)));
    let app = app.clone();
    std::thread::Builder::new()
        .name("interpellation".into())
        .spawn(move || travailleur(app, rx))
        .expect("fil d'interpellation");
}

/// Interpelle Kobi depuis le raccourci : démarre la réunion si besoin (le
/// mode assistant hors réunion arrive au J4), passe en Écoute et réveille le
/// fil d'interpellation.
pub fn declencher<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    if etat(app).etat == "veille" {
        appliquer(app, &[Evenement::DemarrerReunion])?;
    }
    // Le délai de secours couvre toute l'écoute : c'est le fil qui conclut.
    crate::raccourci::interpeller(app, ECOUTE_MAX + Duration::from_secs(10))?;
    app.state::<Pilote>()
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .send(())
        .map_err(|e| e.to_string())
}

fn travailleur<R: Runtime>(app: AppHandle<R>, rx: Receiver<()>) {
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

    let mut stt = charger_stt();
    let mut claude = rt.block_on(demarrer_claude());
    match &claude {
        Ok(_) => tracing::info!("session Claude Code prête"),
        Err(e) => tracing::warn!(erreur = %e, "Claude Code indisponible"),
    }

    while rx.recv().is_ok() {
        // Plusieurs appuis rapprochés ne lancent qu'une écoute.
        while rx.try_recv().is_ok() {}

        if stt.is_err() {
            stt = charger_stt();
        }
        let question = match stt.as_mut() {
            Ok(engine) => ecouter(&app, engine),
            Err(e) => Err(e.clone()),
        };
        match question {
            Ok(None) => {
                let _ = appliquer(&app, &[Evenement::PasDeQuestion]);
            }
            Ok(Some(question)) => {
                tracing::info!(longueur = question.len(), "question transcrite");
                if appliquer(&app, &[Evenement::QuestionEnvoyee]).is_ok() {
                    let _ = app.emit(EVENEMENT_QUESTION, &question);
                    repondre(&app, &rt, &mut claude, &question);
                }
            }
            Err(e) => erreur(&app, "Micro ou transcription", &e),
        }
    }
}

fn charger_stt() -> Result<NemotronEngine, String> {
    let dir = modele_nemotron().ok_or_else(|| {
        "modèle Nemotron introuvable : placer le paquet sherpa-onnx-nemotron-… dans ~/Kobi/models"
            .to_string()
    })?;
    let mut config = NemotronConfig::new(&dir);
    config.num_threads = THREADS_STT;
    let debut = Instant::now();
    let engine = NemotronEngine::new(config).map_err(|e| e.to_string())?;
    tracing::info!(modele = %dir.display(), ms = debut.elapsed().as_millis() as u64, "Nemotron chargé");
    Ok(engine)
}

/// Écoute le micro jusqu'à la fin de la phrase. `None` : rien n'a été dit.
fn ecouter<R: Runtime>(
    app: &AppHandle<R>,
    engine: &mut NemotronEngine,
) -> Result<Option<String>, String> {
    engine.reset();
    let capture = Capture::start(&CaptureOptions {
        sources: vec![kobi_audio::Source::Moi],
        mic: None,
        output: None,
    })
    .map_err(|e| format!("micro indisponible : {e}"))?;

    let debut = Instant::now();
    let mut texte = String::new();
    let mut finale: Option<String> = None;
    while finale.is_none() {
        if debut.elapsed() > ECOUTE_MAX || (texte.is_empty() && debut.elapsed() > SANS_PAROLE) {
            break;
        }
        match capture.frames().recv_timeout(Duration::from_millis(200)) {
            Ok(frame) => {
                let frame = kobi_stt::AudioFrame {
                    source: kobi_stt::Source::Moi,
                    samples: frame.samples,
                    t: frame.t.as_secs_f64(),
                };
                for segment in engine.push(&frame).map_err(|e| e.to_string())? {
                    texte.clone_from(&segment.text);
                    let _ = app.emit(EVENEMENT_QUESTION, &texte);
                    if segment.is_final {
                        finale = Some(segment.text);
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    drop(capture);

    if finale.is_none() && !texte.is_empty() {
        for segment in engine.finish().map_err(|e| e.to_string())? {
            finale = Some(segment.text);
        }
    }
    Ok(finale
        .or((!texte.is_empty()).then_some(texte))
        .filter(|t| !t.trim().is_empty()))
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

fn erreur<R: Runtime>(app: &AppHandle<R>, titre: &str, message: &str) {
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
