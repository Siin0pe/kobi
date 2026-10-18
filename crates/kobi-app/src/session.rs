//! Pont entre l'interface et la session : commandes Tauri (démarrer, pause,
//! reprendre, terminer) et événements vers la webview (`state`, `segment`).
//!
//! L'interface ne contient aucune logique métier : elle envoie des commandes
//! et affiche l'état reçu. Les transitions sont celles de
//! `kobi_session::etat`.

use std::sync::Mutex;

use kobi_session::etat::{Activite, Etat, Evenement};
use kobi_stt::Segment;
use serde::Serialize;
use tauri::{AppHandle, Builder, Emitter, Manager, Runtime};

use crate::chef::{publier, Bulle, ChefPartage, Enregistrement, Message};
use crate::intentions::Intention;

/// Nom de l'événement qui porte l'état de la session.
pub const EVENEMENT_ETAT: &str = "state";
/// Nom de l'événement qui porte un segment de transcription.
#[cfg_attr(not(test), allow(dead_code))] // branché avec la transcription en direct
pub const EVENEMENT_SEGMENT: &str = "segment";

/// État de la session partagé entre les commandes.
pub struct SessionPartagee(Mutex<Etat>);

impl Default for SessionPartagee {
    fn default() -> Self {
        Self(Mutex::new(Etat::Veille))
    }
}

/// État envoyé à la webview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EtatUi {
    /// `veille`, `assistant`, `reunion`, `pause` ou `fin`.
    pub etat: &'static str,
    /// Activité en réunion ou en échange (`calme`, `ecoute`…), absente sinon.
    pub activite: Option<&'static str>,
    /// État de la mascotte (`idle`, `listen`…).
    pub mascotte: &'static str,
    /// La transcription tourne (indicateur REC).
    pub transcrit: bool,
}

impl From<Etat> for EtatUi {
    fn from(etat: Etat) -> Self {
        let cle = |a| match a {
            Activite::Calme => "calme",
            Activite::Ecoute => "ecoute",
            Activite::Reflechit => "reflechit",
            Activite::Note => "note",
            Activite::Repond => "repond",
            Activite::Propose => "propose",
            Activite::Fait => "fait",
            Activite::Oups => "oups",
        };
        let (nom, activite) = match etat {
            Etat::Veille => ("veille", None),
            Etat::Pause => ("pause", None),
            Etat::Fin => ("fin", None),
            Etat::Reunion(a) => ("reunion", Some(cle(a))),
            Etat::Assistant(a) => ("assistant", Some(cle(a))),
        };
        EtatUi {
            etat: nom,
            activite,
            mascotte: etat.mascotte().cle(),
            transcrit: etat.transcrit(),
        }
    }
}

/// Applique les événements dans l'ordre et émet chaque état traversé. Un
/// refus laisse l'état inchangé et revient à l'interface comme erreur.
pub fn appliquer<R: Runtime>(
    app: &AppHandle<R>,
    evenements: &[Evenement],
) -> Result<EtatUi, String> {
    let session = app.state::<SessionPartagee>();
    let mut etat = session.0.lock().unwrap_or_else(|e| e.into_inner());
    let mut suivant = *etat;
    let mut traverses = Vec::with_capacity(evenements.len());
    for &evenement in evenements {
        suivant = suivant.suivant(evenement).map_err(|e| e.to_string())?;
        traverses.push(suivant);
    }
    *etat = suivant;
    drop(etat);
    for traverse in traverses {
        let ui = EtatUi::from(traverse);
        tracing::info!(
            etat = ui.etat,
            activite = ui.activite,
            mascotte = ui.mascotte,
            "état de la session"
        );
        let _ = app.emit(EVENEMENT_ETAT, ui);
    }
    Ok(suivant.into())
}

/// Envoie un segment de transcription à la webview.
#[cfg_attr(not(test), allow(dead_code))] // branché avec la transcription en direct
pub fn emettre_segment<R: Runtime>(app: &AppHandle<R>, segment: &Segment) {
    let _ = app.emit(EVENEMENT_SEGMENT, segment);
}

/// État courant de la session.
pub fn etat<R: Runtime>(app: &AppHandle<R>) -> EtatUi {
    let session = app.state::<SessionPartagee>();
    let etat = *session.0.lock().unwrap_or_else(|e| e.into_inner());
    etat.into()
}

#[tauri::command]
fn etat_session<R: Runtime>(app: AppHandle<R>) -> EtatUi {
    etat(&app)
}

/// Passe la commande au chef d'orchestre (qui crée et ferme les fichiers de
/// la réunion) ; sans lui (tests de l'IPC), applique seulement la transition.
fn par_le_chef<R: Runtime>(
    app: &AppHandle<R>,
    intention: Intention,
    evenements: &[Evenement],
) -> Result<EtatUi, String> {
    match app.try_state::<ChefPartage<R>>() {
        Some(chef) => {
            chef.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .commande(intention);
            Ok(etat(app))
        }
        None => appliquer(app, evenements),
    }
}

#[tauri::command]
fn demarrer<R: Runtime>(app: AppHandle<R>) -> Result<EtatUi, String> {
    par_le_chef(
        &app,
        Intention::DemarrerReunion,
        &[Evenement::DemarrerReunion],
    )
}

#[tauri::command]
fn pause<R: Runtime>(app: AppHandle<R>) -> Result<EtatUi, String> {
    par_le_chef(&app, Intention::Pause, &[Evenement::Pause])
}

#[tauri::command]
fn reprendre<R: Runtime>(app: AppHandle<R>) -> Result<EtatUi, String> {
    par_le_chef(&app, Intention::Reprendre, &[Evenement::Reprendre])
}

/// Termine la réunion : l'enregistrement s'arrête tout de suite, le compte
/// rendu s'écrit ensuite (sans le chef, retour direct en veille).
#[tauri::command]
fn terminer<R: Runtime>(app: AppHandle<R>) -> Result<EtatUi, String> {
    par_le_chef(
        &app,
        Intention::TerminerReunion,
        &[Evenement::TerminerReunion, Evenement::CompteRenduEcrit],
    )
}

/// Message tapé dans le champ sous la mascotte : même conversation que la voix.
#[tauri::command]
fn envoyer_texte<R: Runtime>(app: AppHandle<R>, texte: String) {
    publier(&app, Message::Question { texte, voix: false });
}

/// Échap : la réponse en cours s'arrête net.
#[tauri::command]
fn interrompre<R: Runtime>(app: AppHandle<R>) {
    publier(&app, Message::Interrompre);
}

/// Clic sur une carte de confirmation (`id` de la carte, `accepte`).
#[tauri::command]
fn decider_carte<R: Runtime>(app: AppHandle<R>, id: String, accepte: bool) -> Result<(), String> {
    let carte = id.parse().map_err(|_| format!("carte inconnue : {id}"))?;
    publier(&app, Message::Decision { carte, accepte });
    Ok(())
}

/// État de l'enregistrement, au démarrage de la webview.
#[tauri::command]
fn etat_enregistrement<R: Runtime>(app: AppHandle<R>) -> Enregistrement {
    app.try_state::<ChefPartage<R>>()
        .map(|c| {
            c.0.lock()
                .unwrap_or_else(|e| e.into_inner())
                .enregistrement()
        })
        .unwrap_or_default()
}

/// Dernières bulles de la conversation.
#[tauri::command]
fn historique<R: Runtime>(app: AppHandle<R>) -> Vec<Bulle> {
    app.try_state::<ChefPartage<R>>()
        .map(|c| c.0.lock().unwrap_or_else(|e| e.into_inner()).historique())
        .unwrap_or_default()
}

/// Branche la session sur l'application : état partagé et commandes.
pub fn installer<R: Runtime>(builder: Builder<R>) -> Builder<R> {
    builder
        .manage(SessionPartagee::default())
        .invoke_handler(tauri::generate_handler![
            etat_session,
            demarrer,
            pause,
            reprendre,
            terminer,
            envoyer_texte,
            interrompre,
            decider_carte,
            etat_enregistrement,
            historique,
            // Réponse à voix haute ou en texte (KOB-142).
            crate::voix::etat_voix_ui,
            crate::voix::regler_voix,
            crate::voix::basculer_voix,
            // Dossiers de code lisibles (KOB-138), pour la fenêtre des réglages.
            crate::lecture::get_codebases,
            crate::lecture::set_codebases,
            // Dépôts trouvés sous un dossier (KOB-153, premier lancement).
            crate::depots::lister_depots,
            crate::stockage::reglages_stockage,
            crate::stockage::regler_stockage,
            crate::demarrage::demarrage_systeme,
            crate::demarrage::regler_demarrage_systeme,
            crate::demarrage::demarrage_par_defaut,
            crate::carnet::ouvrir_carnet,
            crate::carnet::carnet_index,
            crate::carnet::carnet_reunion,
            crate::carnet::carnet_conversation,
            crate::carnet::carnet_chercher,
            crate::carnet::carnet_markdown,
            crate::carnet::carnet_exporter,
            crate::carnet::carnet_ouvrir_dossier,
            crate::carnet::carnet_supprimer,
            crate::carnet::carnet_rappel_fait,
            crate::carnet::choisir_dossier,
            crate::reglages::ouvrir_reglages,
            crate::app::ouvrir_app,
            crate::app::a_propos,
            crate::bienvenue::ouvrir_bienvenue,
            crate::bienvenue::terminer_bienvenue,
            crate::bienvenue::etat_claude,
            crate::bienvenue::micros,
            crate::bienvenue::choisir_micro,
            crate::bienvenue::raccourcis,
            crate::bienvenue::etat_modele,
            crate::bienvenue::telecharger_modele,
            crate::bienvenue::annuler_telechargement,
            crate::notes::notes_en_cours,
            crate::notes::basculer_notes,
            crate::notes::replier_notes,
            crate::notes::fermer_notes
        ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tauri::ipc::{CallbackFn, InvokeBody};
    use tauri::test::{get_ipc_response, mock_builder, mock_context, noop_assets, INVOKE_KEY};
    use tauri::webview::InvokeRequest;
    use tauri::{Listener, WebviewWindowBuilder};

    fn requete(cmd: &str) -> InvokeRequest {
        InvokeRequest {
            cmd: cmd.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: if cfg!(any(windows, target_os = "android")) {
                "http://tauri.localhost"
            } else {
                "tauri://localhost"
            }
            .parse()
            .unwrap(),
            body: InvokeBody::default(),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_string(),
        }
    }

    /// Démarre puis termine une réunion comme le ferait la webview : les
    /// commandes passent par l'IPC de Tauri et les états reviennent par
    /// l'événement `state`.
    #[test]
    fn demarrer_puis_terminer_depuis_la_webview() {
        let app = installer(mock_builder())
            .build(mock_context(noop_assets()))
            .unwrap();
        let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let recus = Arc::new(Mutex::new(Vec::new()));
        let r = Arc::clone(&recus);
        app.listen(EVENEMENT_ETAT, move |e| {
            let v: serde_json::Value = serde_json::from_str(e.payload()).unwrap();
            r.lock().unwrap().push(v);
        });
        let appel = |cmd: &str| {
            get_ipc_response(&webview, requete(cmd))
                .map(|b| b.deserialize::<serde_json::Value>().unwrap())
        };

        assert_eq!(appel("etat_session").unwrap()["etat"], "veille");

        let etat = appel("demarrer").unwrap();
        assert_eq!(etat["etat"], "reunion");
        assert_eq!(etat["activite"], "calme");
        assert_eq!(etat["mascotte"], "idle");
        assert_eq!(etat["transcrit"], true);

        assert_eq!(appel("pause").unwrap()["etat"], "pause");
        assert_eq!(appel("reprendre").unwrap()["etat"], "reunion");

        let etat = appel("terminer").unwrap();
        assert_eq!(etat["etat"], "veille");
        assert_eq!(etat["transcrit"], false);

        // Commande refusée : erreur lisible, état inchangé, aucun événement.
        let refus = appel("pause").unwrap_err();
        assert!(
            refus.as_str().unwrap().contains("transition refusée"),
            "{refus}"
        );
        assert_eq!(appel("etat_session").unwrap()["etat"], "veille");

        let etats: Vec<_> = recus
            .lock()
            .unwrap()
            .iter()
            .map(|v| {
                (
                    v["etat"].as_str().unwrap().to_owned(),
                    v["mascotte"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        let attendu = [
            ("reunion", "idle"),
            ("pause", "idle"),
            ("reunion", "idle"),
            ("fin", "write"),
            ("veille", "idle"),
        ];
        assert_eq!(
            etats,
            attendu.map(|(a, b)| (a.to_owned(), b.to_owned())).to_vec()
        );
    }

    #[test]
    fn segment_emis_vers_la_webview() {
        let app = installer(mock_builder())
            .build(mock_context(noop_assets()))
            .unwrap();
        let recus = Arc::new(Mutex::new(Vec::new()));
        let r = Arc::clone(&recus);
        app.listen(EVENEMENT_SEGMENT, move |e| {
            r.lock().unwrap().push(e.payload().to_owned());
        });
        let segment = Segment {
            start: 1.0,
            end: 2.5,
            source: kobi_stt::Source::Eux,
            text: "bonjour".into(),
            is_final: true,
        };
        emettre_segment(app.handle(), &segment);
        let recus = recus.lock().unwrap();
        let v: serde_json::Value = serde_json::from_str(&recus[0]).unwrap();
        assert_eq!(v["text"], "bonjour");
        assert_eq!(v["source"], "eux");
        assert_eq!(v["is_final"], true);
    }

    #[test]
    fn etat_ui_de_chaque_etat() {
        for etat in Etat::tous() {
            let ui = EtatUi::from(etat);
            assert_eq!(ui.mascotte, etat.mascotte().cle());
            assert_eq!(
                ui.activite.is_some(),
                matches!(etat, Etat::Reunion(_) | Etat::Assistant(_))
            );
        }
    }
}
