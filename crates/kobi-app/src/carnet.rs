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
use tauri::{AppHandle, Runtime};
use tauri_plugin_dialog::DialogExt;

/// Une réunion marquée « en cours » mais sans écriture depuis ce délai est
/// une réunion interrompue (arrêt brutal) : on peut la supprimer.
const EN_COURS_ACTIVE: Duration = Duration::from_secs(10 * 60);

/// Ouvre le Carnet, la section « Notes et réunions » de l'app (KOB-159).
/// Utilisable depuis le menu, la barre système ou une intention vocale.
pub fn ouvrir<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    ouvrir_sur(app, None)
}

/// Ouvre le Carnet sur une réunion, onglet Compte rendu (carte « Compte
/// rendu prêt », KOB-74) ; un nom douteux ouvre simplement le Carnet.
pub fn ouvrir_sur<R: Runtime>(app: &AppHandle<R>, reunion: Option<&str>) -> tauri::Result<()> {
    let section = match reunion {
        Some(r) => crate::app::Section::lire(&format!("notes/{r}"))
            .unwrap_or(crate::app::Section::Notes { reunion: None }),
        None => crate::app::Section::Notes { reunion: None },
    };
    crate::app::ouvrir(app, Some(section))
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
pub fn ouvrir_carnet<R: Runtime>(app: AppHandle<R>, reunion: Option<String>) -> Result<(), String> {
    ouvrir_sur(&app, reunion.as_deref()).map_err(|e| e.to_string())
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
    let fait = stockage(&app)?
        .marquer_fait(&id)
        .map_err(|e| e.to_string())?;
    crate::rappels::reveiller(&app);
    Ok(fait)
}

/// Sélecteur de dossier natif (dossier de Kobi, dossiers de code). `None`
/// si annulé.
#[tauri::command]
pub async fn choisir_dossier<R: Runtime>(
    app: AppHandle<R>,
    titre: Option<String>,
) -> Result<Option<PathBuf>, String> {
    let mut dialogue = app
        .dialog()
        .file()
        .set_title(titre.as_deref().unwrap_or("Dossier de Kobi"));
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

    /// Les commandes appelées par la fenêtre du Carnet, comme depuis la
    /// webview : index, lecture, recherche, copie, rappel fait, suppression.
    #[test]
    fn commandes_du_carnet_par_ipc() {
        use crate::test_ipc::appeler;
        use kobi_session::config::Config;
        use kobi_stt::{Segment, Source};
        use serde_json::json;
        use tauri::test::{mock_builder, mock_context, noop_assets};

        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.stockage.dossier = Some(dir.path().to_owned());
        let st = Stockage::new(dir.path());
        st.preparer().unwrap();

        // Une réunion terminée, avec transcription et compte rendu.
        let mut r = st
            .nouvelle_reunion(1_792_315_800_000, Some("Point budget"))
            .unwrap();
        for (start, source, text) in [
            (0.0, Source::Moi, "On parle du budget de la fusée."),
            (4.5, Source::Eux, "Le prototype vole jeudi."),
        ] {
            r.ecrire_segment(&Segment {
                start,
                end: start + 4.0,
                source,
                text: text.into(),
                is_final: true,
            })
            .unwrap();
        }
        r.ecrire_compte_rendu("## Résumé\nBudget validé.\n\n## Actions\n- [ ] Envoyer le devis\n")
            .unwrap();
        r.terminer(1_792_316_400_000).unwrap();
        let nom = r.nom().to_owned();
        // Deux conversations.
        let mut c1 = st.nouvelle_conversation(1_792_320_000_000);
        c1.ajouter(Qui::Moi, "Quelle heure à Tokyo ?", true)
            .unwrap();
        c1.ajouter(Qui::Kobi, "Il est 23 h.", false).unwrap();
        let mut c2 = st.nouvelle_conversation(1_792_330_000_000);
        c2.ajouter(Qui::Moi, "Où est la fusée dans le code ?", false)
            .unwrap();
        let rappel = st
            .ajouter_rappel("Appeler Léa", Some(1_792_400_000_000), None)
            .unwrap();

        let app = crate::session::installer(mock_builder())
            .build(mock_context(noop_assets()))
            .unwrap();
        crate::stockage::installer(app.handle(), &config, None);
        let webview =
            tauri::WebviewWindowBuilder::new(&app, crate::app::FENETRE, Default::default())
                .build()
                .unwrap();
        let appel = |cmd: &str, corps| appeler(&webview, cmd, corps);

        let index = appel("carnet_index", json!({})).unwrap();
        assert_eq!(index["racine"], dir.path().to_str().unwrap());
        assert_eq!(index["reunions"].as_array().unwrap().len(), 1);
        assert_eq!(index["reunions"][0]["titre"], "Point budget");
        assert_eq!(index["reunions"][0]["a_compte_rendu"], true);
        assert_eq!(index["conversations"].as_array().unwrap().len(), 2);
        assert_eq!(index["rappels"][0]["texte"], "Appeler Léa");

        let lue = appel("carnet_reunion", json!({ "nom": nom })).unwrap();
        assert_eq!(lue["segments"].as_array().unwrap().len(), 2);
        assert_eq!(lue["segments"][1]["source"], "eux");
        assert_eq!(lue["compte_rendu"]["resume"], "Budget validé.");
        assert!(lue["compte_rendu"]["actions"]
            .as_str()
            .unwrap()
            .contains("devis"));

        let conv = appel("carnet_conversation", json!({ "id": c1.id() })).unwrap();
        assert_eq!(conv["tours"][0]["qui"], "moi");
        assert_eq!(conv["tours"][1]["texte"], "Il est 23 h.");

        // Sans casse ni accents : « FUSEE » retrouve la réunion et une conversation.
        let trouves = appel("carnet_chercher", json!({ "requete": "FUSEE" })).unwrap();
        let types: Vec<_> = trouves
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["type"].as_str().unwrap())
            .collect();
        assert!(
            types.contains(&"reunion") && types.contains(&"conversation"),
            "{trouves}"
        );
        let dans_transcription = trouves
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["partie"] == "transcription")
            .unwrap();
        assert_eq!(dans_transcription["start"], 0.0);

        let md = appel("carnet_markdown", json!({ "genre": "reunion", "id": nom })).unwrap();
        assert!(md.as_str().unwrap().contains("Budget validé"));
        assert!(appel("carnet_markdown", json!({ "genre": "inconnu", "id": nom })).is_err());

        assert_eq!(
            appel("carnet_rappel_fait", json!({ "id": rappel.id })).unwrap(),
            json!(true)
        );
        assert!(st.rappels().unwrap()[0].fait);

        appel(
            "carnet_supprimer",
            json!({ "genre": "conversation", "id": c2.id() }),
        )
        .unwrap();
        appel("carnet_supprimer", json!({ "genre": "reunion", "id": nom })).unwrap();
        let index = appel("carnet_index", json!({})).unwrap();
        assert!(index["reunions"].as_array().unwrap().is_empty());
        assert_eq!(index["conversations"].as_array().unwrap().len(), 1);

        // Une réunion en cours, écrite à l'instant, ne se supprime pas.
        let en_cours = st
            .nouvelle_reunion(kobi_session::stockage::maintenant_ms(), Some("En direct"))
            .unwrap();
        let refus = appel(
            "carnet_supprimer",
            json!({ "genre": "reunion", "id": en_cours.nom() }),
        )
        .unwrap_err();
        assert!(refus.as_str().unwrap().contains("en cours"), "{refus}");
        assert!(en_cours.dossier().is_dir());
    }
}
