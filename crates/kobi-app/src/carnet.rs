//! Le Carnet (KOB-140) : deuxième fenêtre, qui relit le stockage local
//! (réunions, conversations, rappels) et permet de chercher, copier,
//! exporter et supprimer.
//!
//! Toute la lecture passe par `kobi_session::stockage` ; la fenêtre n'a pas
//! d'autre accès au disque que ces commandes.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use kobi_session::stockage::{
    ConversationLue, DocumentAPublier, Rappel, ResumeConversation, ResumeReunion, ReunionLue,
    Stockage, Trouve,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_dialog::DialogExt;

/// Libellé de la fenêtre du Carnet.
pub const FENETRE: &str = "carnet";

/// Une réunion marquée « en cours » mais sans écriture depuis ce délai est
/// une réunion interrompue (arrêt brutal) : on peut la supprimer.
const EN_COURS_ACTIVE: Duration = Duration::from_secs(10 * 60);

/// Ouvre le Carnet, ou le ramène devant s'il est déjà ouvert. Utilisable
/// depuis le menu, la barre système ou une intention vocale.
pub fn ouvrir<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    if let Some(w) = app.get_webview_window(FENETRE) {
        let _ = w.unminimize();
        w.show()?;
        return w.set_focus();
    }
    WebviewWindowBuilder::new(app, FENETRE, WebviewUrl::App("carnet.html".into()))
        .title("Carnet de Kobi")
        .inner_size(1120.0, 740.0)
        .min_inner_size(720.0, 480.0)
        .center()
        .visible(true)
        .focused(true)
        .build()?;
    tracing::info!("carnet ouvert");
    Ok(())
}

fn stockage<R: Runtime>(app: &AppHandle<R>) -> Result<Stockage, String> {
    crate::stockage::actuel(app).ok_or_else(|| "dossier de Kobi introuvable".to_owned())
}

/// Type d'élément du Carnet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Genre {
    Reunion,
    Conversation,
}

/// Tout ce que montre l'index du Carnet.
#[derive(Debug, Clone, Serialize)]
pub struct Index {
    pub racine: PathBuf,
    pub reunions: Vec<ResumeReunion>,
    pub conversations: Vec<ResumeConversation>,
    pub rappels: Vec<Rappel>,
}

#[tauri::command]
pub fn ouvrir_carnet<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    ouvrir(&app).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn carnet_index<R: Runtime>(app: AppHandle<R>) -> Result<Index, String> {
    let st = stockage(&app)?;
    let err = |e: std::io::Error| e.to_string();
    Ok(Index {
        racine: st.racine().to_owned(),
        reunions: st.reunions().map_err(err)?,
        conversations: st.conversations().map_err(err)?,
        rappels: st.rappels().map_err(err)?,
    })
}

#[tauri::command]
pub fn carnet_reunion<R: Runtime>(app: AppHandle<R>, nom: String) -> Result<ReunionLue, String> {
    stockage(&app)?
        .lire_reunion(&nom)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn carnet_conversation<R: Runtime>(
    app: AppHandle<R>,
    id: String,
) -> Result<ConversationLue, String> {
    stockage(&app)?
        .lire_conversation(&id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn carnet_chercher<R: Runtime>(
    app: AppHandle<R>,
    requete: String,
) -> Result<Vec<Trouve>, String> {
    stockage(&app)?
        .chercher(&requete, 200)
        .map_err(|e| e.to_string())
}

fn document(st: &Stockage, genre: Genre, id: &str) -> Result<DocumentAPublier, String> {
    match genre {
        Genre::Reunion => st.document_reunion(id),
        Genre::Conversation => st.document_conversation(id),
    }
    .map_err(|e| e.to_string())
}

/// Markdown d'un élément, pour « Copier ».
#[tauri::command]
pub fn carnet_markdown<R: Runtime>(
    app: AppHandle<R>,
    genre: Genre,
    id: String,
) -> Result<String, String> {
    Ok(document(&stockage(&app)?, genre, &id)?.markdown)
}

/// Nom de fichier proposé à l'export : le titre nettoyé.
fn nom_d_export(doc: &DocumentAPublier) -> String {
    format!("{}.md", kobi_session::reunion::nettoyer_titre(&doc.titre))
}

/// Exporte un élément en markdown, là où l'utilisateur le choisit. Retourne
/// le chemin écrit, ou `None` s'il a annulé.
#[tauri::command]
pub async fn carnet_exporter<R: Runtime>(
    app: AppHandle<R>,
    genre: Genre,
    id: String,
) -> Result<Option<PathBuf>, String> {
    let doc = document(&stockage(&app)?, genre, &id)?;
    let mut dialogue = app
        .dialog()
        .file()
        .set_title("Exporter en markdown")
        .set_file_name(nom_d_export(&doc))
        .add_filter("Markdown", &["md"]);
    if let Some(dir) = std::env::home_dir().map(|h| h.join("Documents")) {
        dialogue = dialogue.set_directory(dir);
    }
    let Some(chemin) = dialogue.blocking_save_file() else {
        return Ok(None);
    };
    let chemin = chemin.into_path().map_err(|e| e.to_string())?;
    std::fs::write(&chemin, doc.markdown).map_err(|e| format!("{} : {e}", chemin.display()))?;
    tracing::info!(fichier = %chemin.display(), "carnet : export");
    Ok(Some(chemin))
}

/// Ouvre dans l'explorateur le dossier d'un élément, ou la racine.
#[tauri::command]
pub fn carnet_ouvrir_dossier<R: Runtime>(
    app: AppHandle<R>,
    genre: Option<Genre>,
    id: Option<String>,
) -> Result<(), String> {
    let st = stockage(&app)?;
    let chemin = match (genre, id) {
        (Some(Genre::Reunion), Some(nom)) => {
            st.lire_reunion(&nom).map_err(|e| e.to_string())?.dossier
        }
        (Some(Genre::Conversation), Some(id)) => {
            let lue = st.lire_conversation(&id).map_err(|e| e.to_string())?;
            lue.chemin
                .parent()
                .map(Path::to_owned)
                .unwrap_or_else(|| st.dossier_conversations())
        }
        _ => st.racine().to_owned(),
    };
    montrer_dans_l_explorateur(&chemin)
}

fn montrer_dans_l_explorateur(chemin: &Path) -> Result<(), String> {
    #[cfg(windows)]
    let programme = "explorer";
    #[cfg(target_os = "macos")]
    let programme = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    let programme = "xdg-open";
    std::process::Command::new(programme)
        .arg(chemin)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("{} : {e}", chemin.display()))
}

/// Une réunion en cours d'enregistrement : marquée en cours et écrite il y
/// a peu.
fn reunion_active(dossier: &Path, en_cours: bool, maintenant: SystemTime) -> bool {
    if !en_cours {
        return false;
    }
    let derniere = std::fs::read_dir(dossier)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.metadata().ok()?.modified().ok())
        .max();
    derniere.is_some_and(|t| maintenant.duration_since(t).unwrap_or_default() < EN_COURS_ACTIVE)
}

/// Supprime un élément (l'interface a déjà demandé confirmation). Refuse la
/// réunion en cours d'enregistrement.
#[tauri::command]
pub fn carnet_supprimer<R: Runtime>(
    app: AppHandle<R>,
    genre: Genre,
    id: String,
) -> Result<(), String> {
    let st = stockage(&app)?;
    match genre {
        Genre::Reunion => {
            let lue = st.lire_reunion(&id).map_err(|e| e.to_string())?;
            if reunion_active(&lue.dossier, lue.resume.en_cours, SystemTime::now()) {
                return Err(
                    "Cette réunion est en cours : terminez-la avant de la supprimer.".into(),
                );
            }
            st.supprimer_reunion(&id, None).map_err(|e| e.to_string())?;
        }
        Genre::Conversation => st.supprimer_conversation(&id).map_err(|e| e.to_string())?,
    }
    tracing::info!(?genre, id, "carnet : supprimé");
    Ok(())
}

#[tauri::command]
pub fn carnet_rappel_fait<R: Runtime>(app: AppHandle<R>, id: String) -> Result<bool, String> {
    stockage(&app)?.marquer_fait(&id).map_err(|e| e.to_string())
}

/// Sélecteur de dossier natif, pour le dossier racine. `None` si annulé.
#[tauri::command]
pub async fn choisir_dossier<R: Runtime>(app: AppHandle<R>) -> Result<Option<PathBuf>, String> {
    let mut dialogue = app.dialog().file().set_title("Dossier de Kobi");
    if let Ok(st) = stockage(&app) {
        dialogue = dialogue.set_directory(st.racine());
    }
    match dialogue.blocking_pick_folder() {
        Some(d) => d.into_path().map(Some).map_err(|e| e.to_string()),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobi_session::stockage::Qui;

    #[test]
    fn reunion_active_seulement_si_recente_et_en_cours() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("transcript.jsonl"), "").unwrap();
        let maintenant = SystemTime::now();
        assert!(reunion_active(dir.path(), true, maintenant));
        assert!(!reunion_active(dir.path(), false, maintenant));
        // Interrompue depuis longtemps : supprimable.
        let plus_tard = maintenant + EN_COURS_ACTIVE + Duration::from_secs(1);
        assert!(!reunion_active(dir.path(), true, plus_tard));
    }

    #[test]
    fn nom_d_export_lisible() {
        let doc = DocumentAPublier {
            titre: "Point budget — 2026-10-18".into(),
            markdown: String::new(),
        };
        assert_eq!(nom_d_export(&doc), "point-budget-2026-10-18.md");
    }

    #[test]
    fn index_et_documents_depuis_le_stockage() {
        let dir = tempfile::tempdir().unwrap();
        let st = Stockage::new(dir.path());
        st.preparer().unwrap();
        let r = st
            .nouvelle_reunion(1_792_315_800_000, Some("Point"))
            .unwrap();
        r.ecrire_compte_rendu("## Résumé\nok\n").unwrap();
        let mut c = st.nouvelle_conversation(1_792_315_800_000);
        c.ajouter(Qui::Moi, "salut", true).unwrap();
        assert!(document(&st, Genre::Reunion, r.nom())
            .unwrap()
            .markdown
            .contains("ok"));
        assert!(document(&st, Genre::Conversation, c.id())
            .unwrap()
            .markdown
            .contains("salut"));
        assert!(document(&st, Genre::Reunion, "absente").is_err());
    }
}
