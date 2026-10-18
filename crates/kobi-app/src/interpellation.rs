//! Interpellation de bout en bout (KOB-67, KOB-132, KOB-134) : branchement
//! des composants qui tournent en parallèle.
//!
//! * le fil d'écoute ([`crate::ecoute`]) garde le micro ouvert et Nemotron
//!   chargé, et publie segments et questions ;
//! * le chef d'orchestre ([`crate::chef`]) enregistre la réunion quand on le
//!   lui demande, tient l'historique et l'état affiché ;
//! * l'acteur Claude ([`crate::cerveau`]) répond, prend les notes et mène
//!   les tâches longues sur ses propres sessions.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use kobi_session::etat::Evenement;
use kobi_stt::nemotron::{NemotronConfig, NemotronEngine};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::ecoute::{self, Commande};
use crate::session::appliquer;

/// Question en cours de transcription (texte complet à chaque fois).
pub const EVENEMENT_QUESTION: &str = "question";
/// Réponse en flux (texte accumulé à chaque fois).
pub const EVENEMENT_REPONSE: &str = "answer";
/// Erreur à afficher dans la carte d'erreur.
pub const EVENEMENT_ERREUR: &str = "erreur";

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

/// Lance le chef, l'acteur Claude et le fil d'écoute. `codebases` : dossiers
/// de code que Claude peut lire (KOB-138).
pub fn installer<R: Runtime>(
    app: &AppHandle<R>,
    intervalle_notes_min: u8,
    codebases: Vec<PathBuf>,
) {
    let codebases = crate::lecture::codebases_valides(&codebases);
    tracing::info!(codebases = ?codebases, "Claude en lecture seule");
    // Partagés : `set_codebases` les change à chaud (KOB-153).
    let dossiers: crate::lecture::Dossiers = std::sync::Arc::new(std::sync::RwLock::new(codebases));
    app.manage(crate::lecture::Codebases(dossiers.clone()));
    crate::chef::installer(
        app,
        crate::chef::fabrique_claude(MODELE_CLAUDE, dossiers, source_memoire(app)),
        intervalle_notes_min,
    );
    ecoute::installer(app);
}

/// Mémoire des réunions passées (KOB-76) : les dossiers du stockage courant.
fn source_memoire<R: Runtime>(app: &AppHandle<R>) -> crate::chef::SourceMemoire {
    let app = app.clone();
    std::sync::Arc::new(move || {
        crate::stockage::actuel(&app).map(|st| crate::lecture::Memoire::de(&st))
    })
}

/// Interpelle Kobi depuis le raccourci, secours du mot-clé : la phrase
/// suivante est la question.
pub fn declencher<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    ecoute::commander(app, Commande::Raccourci)
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
