//! Notes en direct (KOB-73) : pendant une réunion, la section « En direct »
//! de l'app Kobi (KOB-159) montre les notes prises (décisions, actions,
//! questions) et met en valeur la dernière. Elle s'ouvre d'un clic sur la
//! fiche ou l'étiquette REC de Kobi.
//!
//! Les notes sont lues sur le disque, dans le `notes.md` de la réunion en
//! cours (stockage, KOB-141) : le panneau ne dépend que du fichier écrit par
//! l'écrivain de la réunion, et se rafraîchit sur les événements `notes` et
//! `enregistrement` de la coque.

use std::path::Path;
use std::time::UNIX_EPOCH;

use kobi_session::stockage::Stockage;
use serde::Serialize;
use tauri::{AppHandle, Runtime};

/// Notes de la réunion en cours, découpées par section.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NotesEnCours {
    /// Nom du dossier de la réunion.
    pub reunion: String,
    pub titre: Option<String>,
    pub debut_ms: Option<u64>,
    pub decisions: String,
    pub actions: String,
    pub questions: String,
    /// Sections non reconnues, et notes sans section.
    pub autres: Vec<(String, String)>,
    /// La dernière puce des notes, sans marques markdown.
    pub derniere_note: Option<String>,
    /// Nombre de puces.
    pub lignes: usize,
    /// Dernière écriture de `notes.md`, en ms depuis l'époque Unix.
    pub ecrit_ms: Option<u64>,
}

/// Puces des notes (`-`, `*`, `+`, `1.`), sans la marque.
fn puces(notes: &str) -> impl Iterator<Item = &str> {
    notes.lines().filter_map(|l| {
        let l = l.trim();
        let reste = l
            .strip_prefix("- ")
            .or_else(|| l.strip_prefix("* "))
            .or_else(|| l.strip_prefix("+ "))
            .or_else(|| {
                let (n, r) = l.split_once(". ")?;
                n.chars().all(|c| c.is_ascii_digit()).then_some(r)
            })?;
        let reste = reste
            .trim_start_matches("[ ] ")
            .trim_start_matches("[x] ")
            .trim();
        (!reste.is_empty()).then_some(reste)
    })
}

/// Puce qui dit seulement qu'il n'y a rien (« Aucune question ouverte. ») :
/// la section est vide, ce n'est pas une note.
fn rien_a_noter(puce: &str) -> bool {
    let p = sans_marques(puce).to_lowercase();
    [
        "aucun ",
        "aucune ",
        "rien ",
        "néant",
        "pas de ",
        "pas encore",
    ]
    .iter()
    .any(|debut| p.starts_with(debut))
}

fn sans_marques(texte: &str) -> String {
    texte.replace("**", "").replace('`', "")
}

/// Titre de section : `## Titre`, ou une ligne seule en gras (`**Titre**`,
/// `**Titre :**`), comme Claude écrit souvent ses notes.
fn titre_de_section(ligne: &str) -> Option<&str> {
    let l = ligne.trim();
    if let Some(t) = l.strip_prefix('#') {
        return Some(t.trim_start_matches('#').trim());
    }
    let t = l
        .strip_prefix("**")?
        .trim_end_matches(':')
        .trim_end()
        .strip_suffix("**")?;
    (!t.contains("**")).then(|| t.trim().trim_end_matches(':').trim())
}

/// Où ranger une section, d'après son titre.
fn cle(titre: &str) -> &'static str {
    let t = kobi_session::reunion::nettoyer_titre(titre);
    if t.contains("decision") {
        "decisions"
    } else if t.contains("action")
        || t.contains("a-faire")
        || t.contains("todo")
        || t.contains("tache")
    {
        "actions"
    } else if t.contains("question") {
        "questions"
    } else {
        "autre"
    }
}

/// Découpe le markdown des notes par section (titres `##` ou en gras). Le
/// texte hors section va dans « Notes » ; un titre sans contenu propre (le
/// titre général des notes) disparaît ; les puces « rien à noter » aussi.
pub fn decouper(
    reunion: &str,
    titre: Option<String>,
    debut_ms: Option<u64>,
    notes: &str,
) -> NotesEnCours {
    let mut blocs: Vec<(Option<String>, String)> = vec![(None, String::new())];
    for ligne in notes.lines() {
        match titre_de_section(ligne) {
            Some(t) => blocs.push((Some(t.to_owned()), String::new())),
            None => {
                let puce = puces(ligne).next();
                if puce.is_some_and(rien_a_noter) {
                    continue;
                }
                let corps = &mut blocs.last_mut().expect("au moins un bloc").1;
                corps.push_str(ligne);
                corps.push('\n');
            }
        }
    }
    let mut n = NotesEnCours {
        reunion: reunion.to_owned(),
        titre,
        debut_ms,
        decisions: String::new(),
        actions: String::new(),
        questions: String::new(),
        autres: Vec::new(),
        derniere_note: None,
        lignes: 0,
        ecrit_ms: None,
    };
    for (t, corps) in blocs {
        let corps = corps.trim().to_owned();
        let k = t.as_deref().map_or("autre", cle);
        let cible = match k {
            "decisions" => &mut n.decisions,
            "actions" => &mut n.actions,
            "questions" => &mut n.questions,
            _ => {
                if !corps.is_empty() {
                    n.autres.push((t.unwrap_or_else(|| "Notes".into()), corps));
                }
                continue;
            }
        };
        if !cible.is_empty() && !corps.is_empty() {
            cible.push('\n');
        }
        cible.push_str(&corps);
    }
    let toutes: Vec<&str> = puces(notes).filter(|p| !rien_a_noter(p)).collect();
    n.derniere_note = toutes.last().map(|p| sans_marques(p));
    n.lignes = toutes.len();
    n
}

fn ecrit_ms(chemin: &Path) -> Option<u64> {
    let t = std::fs::metadata(chemin).ok()?.modified().ok()?;
    Some(t.duration_since(UNIX_EPOCH).ok()?.as_millis() as u64)
}

/// Notes de la réunion en cours la plus récente, `None` hors réunion.
pub fn lire(st: &Stockage) -> std::io::Result<Option<NotesEnCours>> {
    let Some(r) = st.reunions()?.into_iter().find(|r| r.en_cours) else {
        return Ok(None);
    };
    let lue = match st.lire_reunion(&r.nom) {
        Ok(lue) => lue,
        Err(_) => return Ok(None),
    };
    let mut n = decouper(&r.nom, r.titre.clone(), r.debut_ms, &lue.notes);
    n.ecrit_ms = ecrit_ms(&lue.dossier.join(kobi_session::reunion::NOTES));
    Ok(Some(n))
}

#[tauri::command]
pub fn notes_en_cours<R: Runtime>(app: AppHandle<R>) -> Result<Option<NotesEnCours>, String> {
    let Some(st) = crate::stockage::actuel(&app) else {
        return Ok(None);
    };
    lire(&st).map_err(|e| e.to_string())
}

/// Ouvre les notes en direct dans l'app. Rend `true` (la section est
/// montrée), comme le faisait l'ancien panneau flottant.
pub fn basculer<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<bool> {
    crate::app::ouvrir(app, Some(crate::app::Section::Direct))?;
    Ok(true)
}

// Asynchrone : une commande synchrone tourne sur le fil principal, et y
// créer la fenêtre bloque WebView2 sous Windows (fenêtre blanche, KOB-167).
#[tauri::command(async)]
pub fn basculer_notes<R: Runtime>(app: AppHandle<R>) -> Result<bool, String> {
    basculer(&app).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobi_session::stockage::Stockage;

    const NOTES: &str = "## Décisions\n- Budget validé\n- **Séminaire** à Annecy\n\n## Actions\n- [ ] Sophie : devis\n\n## Questions ouvertes\n- Date du séminaire ?\n";

    #[test]
    fn notes_decoupees_par_section() {
        let n = decouper("r", Some("Point".into()), Some(1), NOTES);
        assert!(n.decisions.contains("Budget validé"));
        assert!(n.actions.contains("Sophie : devis"));
        assert_eq!(n.questions, "- Date du séminaire ?");
        assert_eq!(n.derniere_note.as_deref(), Some("Date du séminaire ?"));
        assert_eq!(n.lignes, 4);
        assert!(n.autres.is_empty());
    }

    /// Notes réelles de Claude (essai sous Xvfb) : titre général en `##`,
    /// sections en gras, puce « aucune » à ignorer.
    #[test]
    fn notes_en_gras_comme_claude_les_ecrit() {
        let notes = "## Notes de réunion : point hebdo\n\n**Décisions**\n- Tester une nouvelle buse lundi.\n\n**Actions**\n- Sophie : commander la buse.\n- Thomas : banc d'essai.\n\n**Questions ouvertes**\n- Aucune question ouverte relevée.\n";
        let n = decouper("r", None, None, notes);
        assert_eq!(n.decisions, "- Tester une nouvelle buse lundi.");
        assert_eq!(
            n.actions,
            "- Sophie : commander la buse.\n- Thomas : banc d'essai."
        );
        assert_eq!(n.questions, "");
        assert!(n.autres.is_empty(), "{:?}", n.autres);
        assert_eq!(n.derniere_note.as_deref(), Some("Thomas : banc d'essai."));
        assert_eq!(n.lignes, 3);
        // Titres avec deux-points, dans ou hors du gras.
        let n = decouper(
            "r",
            None,
            None,
            "**Décisions :**\n- A\n**Actions** :\n- B\n",
        );
        assert_eq!((n.decisions.as_str(), n.actions.as_str()), ("- A", "- B"));
    }

    #[test]
    fn notes_sans_section_gardees() {
        let n = decouper("r", None, None, "- premier point\n- second `point`\n");
        assert_eq!(
            n.autres,
            vec![(
                "Notes".to_owned(),
                "- premier point\n- second `point`".to_owned()
            )]
        );
        assert_eq!(n.derniere_note.as_deref(), Some("second point"));
        assert_eq!(decouper("r", None, None, "").lignes, 0);
    }

    #[test]
    fn lire_la_reunion_en_cours_seulement() {
        let dir = tempfile::tempdir().unwrap();
        let st = Stockage::new(dir.path());
        st.preparer().unwrap();
        assert_eq!(lire(&st).unwrap(), None);
        let mut finie = st
            .nouvelle_reunion(1_792_315_800_000, Some("Hier"))
            .unwrap();
        finie.ecrire_notes("- vieux").unwrap();
        finie.terminer(1_792_315_900_000).unwrap();
        assert_eq!(lire(&st).unwrap(), None, "pas de réunion en cours");
        let r = st
            .nouvelle_reunion(1_792_320_000_000, Some("Point"))
            .unwrap();
        r.ecrire_notes(NOTES).unwrap();
        let n = lire(&st).unwrap().unwrap();
        assert_eq!(n.reunion, r.nom());
        assert_eq!(n.titre.as_deref(), Some("Point"));
        assert!(n.ecrit_ms.is_some());
        assert_eq!(n.lignes, 4);
    }

    /// Les commandes depuis la webview : lecture des notes en cours, puis
    /// ouverture des notes en direct dans l'app.
    #[test]
    fn commandes_par_ipc() {
        use crate::test_ipc::appeler;
        use kobi_session::config::Config;
        use serde_json::json;
        use tauri::test::{mock_builder, mock_context, noop_assets};
        use tauri::{Manager, WebviewWindowBuilder};

        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.stockage.dossier = Some(dir.path().to_owned());
        let st = Stockage::new(dir.path());
        st.preparer().unwrap();
        let app = crate::session::installer(mock_builder())
            .build(mock_context(noop_assets()))
            .unwrap();
        crate::stockage::installer(app.handle(), &config, None);
        let main = WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();

        assert_eq!(
            appeler(&main, "notes_en_cours", json!({})).unwrap(),
            json!(null)
        );
        let r = st
            .nouvelle_reunion(1_792_320_000_000, Some("Point"))
            .unwrap();
        r.ecrire_notes(NOTES).unwrap();
        let n = appeler(&main, "notes_en_cours", json!({})).unwrap();
        assert_eq!(n["titre"], "Point");
        assert_eq!(n["derniere_note"], "Date du séminaire ?");
        assert!(n["decisions"].as_str().unwrap().contains("Budget validé"));

        assert_eq!(
            appeler(&main, "basculer_notes", json!({})).unwrap(),
            json!(true)
        );
        // Plus de panneau flottant : les notes en direct sont dans l'app.
        assert!(app.get_webview_window(crate::app::FENETRE).is_some());
        assert!(app.get_webview_window("notes").is_none());
    }
}
