//! Le Carnet (KOB-140) : deuxième fenêtre, qui relit le stockage local
//! (réunions, conversations, rappels) et permet de chercher, copier,
//! exporter et supprimer.
//!
//! Toute la lecture passe par `kobi_session::stockage` ; la fenêtre n'a pas
//! d'autre accès au disque que ces commandes.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use kobi_session::config::Publication;
use kobi_session::stockage::{
    ConversationLue, DocumentAPublier, Rappel, ResumeConversation, ResumeReunion, ReunionLue,
    Stockage, Trouve,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Runtime};
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
    crate::stockage::actuel(app)
        .ok_or_else(|| kobi_i18n::tr("coque.commun.dossierKobiIntrouvable", &[]))
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

// Asynchrone : une commande synchrone tourne sur le fil principal, et y
// créer la fenêtre bloque WebView2 sous Windows (fenêtre blanche, KOB-167).
#[tauri::command(async)]
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

/// Consigne donnée à Claude pour publier un compte rendu (KOB-75). En
/// français, comme toutes les consignes : Claude écrit dans la langue du
/// document.
fn consigne_publication(doc: &DocumentAPublier, p: &Publication) -> String {
    let ou = match p.destination.trim() {
        "" => String::new(),
        d => format!(", à cet endroit : {d}"),
    };
    format!(
        "Publie le compte rendu ci-dessous avec le serveur MCP « {mcp} »{ou}. \
         Crée un seul document (page, document ou ticket, selon ce que propose cet outil), \
         avec ce titre et ce contenu Markdown, sans les modifier. \
         Réponds ensuite en une phrase, avec le lien du document créé.\n\n\
         Titre : {titre}\n\n{markdown}",
        mcp = p.mcp.trim(),
        titre = doc.titre,
        markdown = doc.markdown.trim(),
    )
}

/// Bouton « Publier » du Carnet (KOB-75) : le compte rendu de la réunion
/// part vers la destination réglée (Réglages › Stockage), par Claude et le
/// MCP, après la carte de confirmation. Rend la phrase montrée dans la bulle.
#[tauri::command]
pub fn carnet_publier<R: Runtime>(app: AppHandle<R>, nom: String) -> Result<String, String> {
    let config = kobi_session::config::default_path()
        .map(|p| kobi_session::config::load_or_create(&p).config)
        .unwrap_or_default();
    let p = &config.stockage.publication.reunions;
    if !p.est_reglee() {
        return Err(kobi_i18n::tr("coque.carnet.publicationNonReglee", &[]));
    }
    let doc = stockage(&app)?
        .compte_rendu_a_publier(&nom)
        .map_err(|e| e.to_string())?;
    let affiche = kobi_i18n::tr(
        "coque.carnet.publierQuestion",
        &[("titre", &doc.titre), ("mcp", p.mcp.trim())],
    );
    crate::chef::publier(
        &app,
        crate::chef::Message::Publier {
            affiche: affiche.clone(),
            consigne: consigne_publication(&doc, p),
        },
    );
    Ok(affiche)
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
        .set_title(kobi_i18n::tr("coque.carnet.exporterTitre", &[]))
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
    retenir_export(&chemin);
    Ok(Some(chemin))
}

/// Dernier fichier exporté, celui qu'ouvre « Ouvrir » après l'export
/// (KOB-224). Gardé côté coque : la fenêtre ne donne jamais de chemin à ouvrir.
static DERNIER_EXPORT: Mutex<Option<PathBuf>> = Mutex::new(None);

fn retenir_export(chemin: &Path) {
    *DERNIER_EXPORT.lock().unwrap_or_else(|e| e.into_inner()) = Some(chemin.to_owned());
}

/// Le dernier export, s'il existe encore sur le disque.
fn dernier_export() -> Result<PathBuf, String> {
    let chemin = DERNIER_EXPORT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .ok_or_else(|| kobi_i18n::tr("coque.carnet.aucunExport", &[]))?;
    if !chemin.is_file() {
        return Err(kobi_i18n::tr(
            "coque.carnet.nExistePlus",
            &[("chemin", &chemin.display().to_string())],
        ));
    }
    Ok(chemin)
}

/// Ouvre le dernier fichier exporté avec l'application du système.
#[tauri::command]
pub fn carnet_ouvrir_export() -> Result<(), String> {
    montrer_dans_l_explorateur(&dernier_export()?)
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

/// Ouvre un dossier dans l'explorateur, ou un fichier avec l'application
/// que le système lui associe.
fn montrer_dans_l_explorateur(chemin: &Path) -> Result<(), String> {
    #[cfg(windows)]
    let programme = "explorer";
    #[cfg(target_os = "macos")]
    let programme = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    let programme = "xdg-open";
    let mut enfant = std::process::Command::new(programme)
        .arg(chemin)
        .spawn()
        .map_err(|e| format!("{} : {e}", chemin.display()))?;
    // Attendu à part, comme le navigateur des MCP : sinon chaque « Ouvrir »
    // laisse un zombie jusqu'à la sortie de Kobi (KOB-266).
    std::thread::spawn(move || {
        let _ = enfant.wait();
    });
    Ok(())
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
                return Err(kobi_i18n::tr("coque.carnet.reunionEnCours", &[]));
            }
            st.supprimer_reunion(&id, None).map_err(|e| e.to_string())?;
        }
        Genre::Conversation => st.supprimer_conversation(&id).map_err(|e| e.to_string())?,
    }
    tracing::info!(?genre, id, "carnet : supprimé");
    Ok(())
}

/// Renomme une réunion ou une conversation à la main (KOB-223), depuis l'app
/// (titre édité sur place) ou `kobi meeting rename`. Le titre devient
/// manuel : Kobi ne le remplace plus. Renvoie l'identifiant à jour (le
/// dossier d'une réunion terminée change de nom tout de suite ; celui de la
/// réunion en cours suit par l'événement `reunion-nommee`).
pub fn renommer<R: Runtime>(
    app: &AppHandle<R>,
    genre: Genre,
    id: Option<&str>,
    titre: &str,
) -> Result<String, String> {
    let titre = titre.trim();
    if titre.is_empty() {
        return Err(kobi_i18n::tr("coque.carnet.titreVide", &[]));
    }
    let st = stockage(app)?;
    if genre == Genre::Conversation {
        let id = id.ok_or_else(|| kobi_i18n::tr("coque.carnet.quelleConversation", &[]))?;
        st.renommer_conversation(id, titre)
            .map_err(|e| e.to_string())?;
        tracing::info!(id, "conversation renommée");
        return Ok(id.to_owned());
    }
    let chef = app.try_state::<crate::chef::ChefPartage<R>>();
    let en_cours = chef.as_ref().and_then(|c| {
        c.0.lock()
            .unwrap_or_else(|e| e.into_inner())
            .dossier_en_cours()
    });
    let visee = match id {
        Some(id) => Some(st.lire_reunion(id).map_err(|e| e.to_string())?.dossier),
        None => None,
    };
    let courante = match (&visee, &en_cours) {
        (None, Some(_)) => true,
        (Some(v), Some(c)) => v == c,
        (None, None) => return Err(kobi_i18n::tr("coque.commun.aucuneReunion", &[])),
        (Some(_), None) => false,
    };
    if courante {
        let chef = chef.ok_or_else(|| kobi_i18n::tr("coque.commun.aucuneReunion", &[]))?;
        chef.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .renommer_reunion(titre)?;
        let dossier = en_cours.unwrap_or_default();
        return Ok(dossier
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default());
    }
    let id = id.expect("réunion visée");
    let nouveau = st.renommer_reunion(id, titre).map_err(|e| e.to_string())?;
    tracing::info!(id, nouveau, "réunion renommée");
    Ok(nouveau)
}

/// Titre édité sur place dans l'app (KOB-223).
#[tauri::command(async)]
pub fn carnet_renommer<R: Runtime>(
    app: AppHandle<R>,
    genre: Genre,
    id: String,
    titre: String,
) -> Result<String, String> {
    renommer(&app, genre, Some(&id), &titre)
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
        .set_title(titre.unwrap_or_else(|| kobi_i18n::tr("reglages.stockage.dossierTitre", &[])));
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

    /// KOB-75 : la consigne dit à Claude quel MCP, où, et donne le document
    /// tel quel ; sans destination, elle n'en invente pas.
    #[test]
    fn consigne_de_publication() {
        let doc = DocumentAPublier {
            titre: "Point budget — 2026-10-20".into(),
            markdown: "# Point budget — 2026-10-20\n\n## Décisions\n- Budget validé\n".into(),
        };
        let mut p = Publication {
            mcp: " linear ".into(),
            destination: "Projet Kobi".into(),
            ..Default::default()
        };
        let c = consigne_publication(&doc, &p);
        assert!(c.starts_with("Publie le compte rendu ci-dessous avec le serveur MCP « linear », à cet endroit : Projet Kobi."), "{c}");
        assert!(
            c.contains("Titre : Point budget — 2026-10-20\n\n# Point budget"),
            "{c}"
        );
        assert!(c.ends_with("- Budget validé"), "{c}");
        p.destination = "  ".into();
        assert!(consigne_publication(&doc, &p)
            .starts_with("Publie le compte rendu ci-dessous avec le serveur MCP « linear ». "));
    }
    use kobi_session::stockage::Qui;

    #[test]
    fn ouvrir_export_seulement_le_dernier_fichier_encore_la() {
        let dir = tempfile::tempdir().unwrap();
        let fichier = dir.path().join("point-budget.md");
        std::fs::write(&fichier, "# Point budget\n").unwrap();
        retenir_export(&fichier);
        assert_eq!(dernier_export().unwrap(), fichier);
        // Déplacé ou supprimé depuis : on le dit au lieu de lancer l'ouverture.
        std::fs::remove_file(&fichier).unwrap();
        assert!(dernier_export().unwrap_err().contains("n'existe plus"));
    }

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
