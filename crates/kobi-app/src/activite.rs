//! Journal d'activité (KOB-159) : ce que Kobi a fait, pour la section
//! Activité de l'app. Questions posées, réunions, actions proposées et tes
//! validations, erreurs.
//!
//! Le journal se remplit de lui-même en écoutant les événements de la
//! session (`conversation`, `enregistrement`, `carte`, `erreur`) ; un autre
//! module peut aussi y écrire avec [`noter`] (les permissions de KOB-156 ; les
//! étapes de travail de KOB-152 ont leur propre fil). Chaque entrée est gardée en mémoire (les
//! [`CAPACITE`] dernières), ajoutée à `activite.jsonl` dans le dossier de
//! Kobi et envoyée à l'app par l'événement [`EVENEMENT`].

use std::collections::{HashSet, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use kobi_i18n::tr;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Listener, Manager, Runtime};

/// Événement envoyé à chaque nouvelle entrée ; la charge est l'[`Entree`].
/// (`activite`, lui, porte les étapes d'un échange : voir `chef`, KOB-152.)
pub const EVENEMENT: &str = "journal";

/// Entrées gardées en mémoire et rendues à l'app.
pub const CAPACITE: usize = 500;

/// Fichier du journal, dans le dossier de Kobi.
const FICHIER: &str = "activite.jsonl";

/// Longueur maximale d'un texte du journal, en caractères.
const LONGUEUR: usize = 240;

/// Genre d'entrée : l'app en tire son glyphe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Genre {
    /// Une question posée à Kobi (à la voix ou au clavier).
    Question,
    /// Une réunion commencée, mise en pause, reprise ou terminée.
    Reunion,
    /// Une recherche faite par Kobi (KOB-152).
    Recherche,
    /// Une action proposée, qui attend ta validation.
    Action,
    /// Ta réponse à une action proposée.
    Validation,
    Erreur,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entree {
    /// Horodatage Unix, en ms.
    pub quand_ms: u64,
    pub genre: Genre,
    pub texte: String,
    /// Précision facultative (refusé, accepté, détail de l'erreur…).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Le journal partagé, chargé du disque à la première lecture.
#[derive(Default)]
pub struct Journal(Mutex<Etat>);

#[derive(Default)]
struct Etat {
    entrees: VecDeque<Entree>,
    charge: bool,
    /// Tours de conversation déjà notés.
    tours: HashSet<u64>,
    /// Réunion en cours, d'après le dernier `enregistrement` (actif, pause).
    reunion: Option<(bool, bool)>,
}

fn maintenant_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn court(texte: &str) -> String {
    let texte = texte.split_whitespace().collect::<Vec<_>>().join(" ");
    if texte.chars().count() <= LONGUEUR {
        return texte;
    }
    let mut s: String = texte.chars().take(LONGUEUR - 1).collect();
    s.push('…');
    s
}

fn fichier<R: Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    crate::stockage::actuel(app).map(|s| s.racine().join(FICHIER))
}

/// Les `n` dernières entrées lisibles d'un journal sur le disque.
fn lire(chemin: &Path, n: usize) -> VecDeque<Entree> {
    let Ok(texte) = std::fs::read_to_string(chemin) else {
        return VecDeque::new();
    };
    let mut entrees: VecDeque<Entree> = texte
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    while entrees.len() > n {
        entrees.pop_front();
    }
    entrees
}

fn charger<R: Runtime>(app: &AppHandle<R>, etat: &mut Etat) {
    if etat.charge {
        return;
    }
    etat.charge = true;
    if let Some(chemin) = fichier(app) {
        etat.entrees = lire(&chemin, CAPACITE);
    }
}

/// Note une entrée dans le journal et l'envoie à l'app.
pub fn noter<R: Runtime>(app: &AppHandle<R>, genre: Genre, texte: &str, detail: Option<&str>) {
    let Some(journal) = app.try_state::<Journal>() else {
        return;
    };
    let entree = Entree {
        quand_ms: maintenant_ms(),
        genre,
        texte: court(texte),
        detail: detail.map(court).filter(|d| !d.is_empty()),
    };
    {
        let mut etat = journal.0.lock().unwrap_or_else(|e| e.into_inner());
        // Le fichier d'abord : la nouvelle entrée y sera écrite juste après.
        charger(app, &mut etat);
        etat.entrees.push_back(entree.clone());
        while etat.entrees.len() > CAPACITE {
            etat.entrees.pop_front();
        }
    }
    if let Some(chemin) = fichier(app) {
        let ecrit = serde_json::to_string(&entree).map(|ligne| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&chemin)
                .and_then(|mut f| writeln!(f, "{ligne}"))
        });
        if let Ok(Err(e)) = ecrit {
            tracing::warn!(chemin = %chemin.display(), "journal d'activité : {e}");
        }
    }
    let _ = app.emit(EVENEMENT, &entree);
}

/// Le journal, de la plus ancienne à la plus récente entrée.
#[tauri::command]
pub fn journal_activite<R: Runtime>(app: AppHandle<R>) -> Vec<Entree> {
    let Some(journal) = app.try_state::<Journal>() else {
        return Vec::new();
    };
    let mut etat = journal.0.lock().unwrap_or_else(|e| e.into_inner());
    charger(&app, &mut etat);
    etat.entrees.iter().cloned().collect()
}

/// Ce qu'un événement de la session ajoute au journal.
fn traduire(
    etat: &mut Etat,
    evenement: &str,
    charge: &Value,
) -> Vec<(Genre, String, Option<String>)> {
    let texte = |v: &Value, cle: &str| {
        v.get(cle)
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_owned()
    };
    let mut sortie = Vec::new();
    match evenement {
        // L'historique entier, à chaque tour : on note les nouvelles questions.
        "conversation" => {
            for bulle in charge.as_array().into_iter().flatten() {
                let Some(tour) = bulle.get("tour").and_then(Value::as_u64) else {
                    continue;
                };
                let question = texte(bulle, "question");
                if question.is_empty() || !etat.tours.insert(tour) {
                    continue;
                }
                let clavier = bulle.get("clavier").and_then(Value::as_bool) == Some(true);
                sortie.push((
                    Genre::Question,
                    question,
                    clavier.then(|| tr("coque.activite.auClavier", &[])),
                ));
            }
        }
        "enregistrement" => {
            let actif = charge.get("actif").and_then(Value::as_bool) == Some(true);
            let pause = charge.get("pause").and_then(Value::as_bool) == Some(true);
            let titre = Some(texte(charge, "titre")).filter(|t| !t.is_empty());
            let avant = etat.reunion;
            etat.reunion = actif.then_some((actif, pause));
            match (avant, actif, pause) {
                (None, true, _) => sortie.push((
                    Genre::Reunion,
                    tr("coque.activite.reunionCommencee", &[]),
                    titre,
                )),
                (Some(_), false, _) => {
                    let notes = charge.get("notes").and_then(Value::as_u64).unwrap_or(0);
                    let detail = match notes {
                        0 => titre,
                        n => Some(format!(
                            "{}{}",
                            titre.map_or(String::new(), |t| format!("{t} · ")),
                            tr("coque.activite.lignesDeNotes", &[("n", &n.to_string())])
                        )),
                    };
                    sortie.push((
                        Genre::Reunion,
                        tr("coque.activite.reunionTerminee", &[]),
                        detail,
                    ));
                }
                (Some((_, false)), true, true) => sortie.push((
                    Genre::Reunion,
                    tr("coque.activite.reunionEnPause", &[]),
                    titre,
                )),
                (Some((_, true)), true, false) => sortie.push((
                    Genre::Reunion,
                    tr("coque.activite.reunionReprise", &[]),
                    titre,
                )),
                _ => {}
            }
        }
        "carte" => {
            let titre = texte(charge, "title");
            let genre = texte(charge, "kicker");
            let detail = Some(genre).filter(|g| !g.is_empty());
            match charge.get("status").and_then(Value::as_str) {
                Some("pending") => sortie.push((
                    Genre::Action,
                    tr("coque.activite.propose", &[("titre", &titre)]),
                    detail,
                )),
                Some("confirmed") => sortie.push((
                    Genre::Validation,
                    tr("coque.activite.accepte", &[("titre", &titre)]),
                    detail,
                )),
                Some("dismissed") => sortie.push((
                    Genre::Validation,
                    tr("coque.activite.refuse", &[("titre", &titre)]),
                    detail,
                )),
                _ => {}
            }
        }
        "erreur" => {
            let titre = texte(charge, "title");
            let message = Some(texte(charge, "message")).filter(|m| !m.is_empty());
            sortie.push((
                Genre::Erreur,
                if titre.is_empty() {
                    tr("app.sections.genre.erreur", &[])
                } else {
                    titre
                },
                message,
            ));
        }
        _ => {}
    }
    sortie
}

/// Écoute les événements de la session pour remplir le journal.
pub fn installer<R: Runtime>(app: &AppHandle<R>) {
    for evenement in ["conversation", "enregistrement", "carte", "erreur"] {
        let handle = app.clone();
        app.listen_any(evenement, move |e| {
            let Ok(charge) = serde_json::from_str::<Value>(e.payload()) else {
                return;
            };
            let Some(journal) = handle.try_state::<Journal>() else {
                return;
            };
            let a_noter = {
                let mut etat = journal.0.lock().unwrap_or_else(|e| e.into_inner());
                traduire(&mut etat, evenement, &charge)
            };
            for (genre, texte, detail) in a_noter {
                noter(&handle, genre, &texte, detail.as_deref());
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn les_evenements_de_la_session_deviennent_des_entrees() {
        let mut etat = Etat::default();

        // Les questions, une fois chacune, même si l'historique revient.
        let historique = json!([{ "tour": 1, "question": "Quelle heure est-il ?", "reponse": "", "statut": "attente", "clavier": false }]);
        let n = traduire(&mut etat, "conversation", &historique);
        assert_eq!(
            n,
            [(Genre::Question, "Quelle heure est-il ?".to_owned(), None)]
        );
        assert!(traduire(&mut etat, "conversation", &historique).is_empty());
        let historique = json!([
            { "tour": 1, "question": "Quelle heure est-il ?", "statut": "fini" },
            { "tour": 2, "question": "Et demain ?", "statut": "attente", "clavier": true },
        ]);
        let n = traduire(&mut etat, "conversation", &historique);
        assert_eq!(
            n,
            [(
                Genre::Question,
                "Et demain ?".to_owned(),
                Some("au clavier".to_owned())
            )]
        );

        // Une réunion : début, pause, reprise, fin ; les notes en route ne comptent pas.
        let r = |actif, pause, notes| json!({ "actif": actif, "pause": pause, "debut_ms": 1, "notes": notes, "titre": "Point hebdo" });
        let textes = |v: Vec<(Genre, String, Option<String>)>| {
            v.into_iter().map(|(_, t, d)| (t, d)).collect::<Vec<_>>()
        };
        assert_eq!(
            textes(traduire(&mut etat, "enregistrement", &r(true, false, 0))),
            [(
                "Réunion commencée".to_owned(),
                Some("Point hebdo".to_owned())
            )]
        );
        assert!(traduire(&mut etat, "enregistrement", &r(true, false, 3)).is_empty());
        assert_eq!(
            textes(traduire(&mut etat, "enregistrement", &r(true, true, 3)))[0].0,
            "Réunion en pause"
        );
        assert_eq!(
            textes(traduire(&mut etat, "enregistrement", &r(true, false, 3)))[0].0,
            "Réunion reprise"
        );
        assert_eq!(
            textes(traduire(&mut etat, "enregistrement", &r(false, false, 4))),
            [(
                "Réunion terminée".to_owned(),
                Some("Point hebdo · 4 lignes de notes".to_owned())
            )]
        );
        assert!(traduire(&mut etat, "enregistrement", &r(false, false, 0)).is_empty());

        // Une action proposée puis refusée, une erreur.
        let carte = |status| json!({ "id": "c1", "kicker": "Commande", "title": "cargo test", "status": status });
        assert_eq!(
            traduire(&mut etat, "carte", &carte("pending"))[0].0,
            Genre::Action
        );
        let v = traduire(&mut etat, "carte", &carte("dismissed"));
        assert_eq!(
            (v[0].0, v[0].1.as_str()),
            (Genre::Validation, "Refusé : cargo test")
        );
        let v = traduire(
            &mut etat,
            "erreur",
            &json!({ "title": "Claude ne répond pas", "message": "Réessaie." }),
        );
        assert_eq!(
            v,
            [(
                Genre::Erreur,
                "Claude ne répond pas".to_owned(),
                Some("Réessaie.".to_owned())
            )]
        );
    }

    #[test]
    fn textes_ramasses_et_coupes() {
        assert_eq!(court("  une\n question   posée "), "une question posée");
        let long = "a".repeat(LONGUEUR + 10);
        assert_eq!(court(&long).chars().count(), LONGUEUR);
        assert!(court(&long).ends_with('…'));
    }

    /// Le journal se remplit des événements, s'écrit dans le dossier de Kobi
    /// et se relit après un redémarrage.
    #[test]
    fn journal_ecrit_et_relu() {
        use crate::test_ipc::appeler;
        use kobi_session::stockage::Stockage;
        use tauri::test::{mock_builder, mock_context, noop_assets};

        let dossier = tempfile::tempdir().unwrap();
        let ouvrir = || {
            let app = crate::session::installer(mock_builder())
                .build(mock_context(noop_assets()))
                .unwrap();
            app.manage(crate::stockage::StockagePartage::new(
                Some(Stockage::new(dossier.path())),
                None,
            ));
            installer(app.handle());
            app
        };
        let app = ouvrir();
        let main = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        app.emit(
            "erreur",
            json!({ "title": "Micro introuvable", "message": "Branche un micro." }),
        )
        .unwrap();
        noter(
            app.handle(),
            Genre::Recherche,
            "Recherche : « budget » dans le Carnet",
            None,
        );
        let journal = appeler(&main, "journal_activite", json!({})).unwrap();
        let textes: Vec<_> = journal
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["texte"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            textes,
            ["Micro introuvable", "Recherche : « budget » dans le Carnet"]
        );
        assert_eq!(journal[0]["genre"], "erreur");
        assert_eq!(journal[0]["detail"], "Branche un micro.");
        drop(main);
        drop(app);

        // Après un redémarrage, l'historique revient du fichier.
        let app = ouvrir();
        let main = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let journal = appeler(&main, "journal_activite", json!({})).unwrap();
        assert_eq!(journal.as_array().unwrap().len(), 2);
        assert!(dossier.path().join(FICHIER).exists());
    }
}
