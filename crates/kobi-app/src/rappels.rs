//! Rappels et tâches à faire (KOB-145) : la demande dite ou tapée est
//! comprise sur place ([`kobi_session::rappels`]), rangée dans
//! `rappels.json`, et le planificateur annonce chaque rappel à l'heure dite
//! (événement [`EVENEMENT_RAPPEL`], bulle de notification).
//!
//! Le planificateur dort jusqu'au prochain rappel, au plus [`RELECTURE`] :
//! un rappel ajouté ou coché ailleurs (Carnet, autre fenêtre) est vu au
//! plus tard au réveil suivant ; [`reveiller`] le fait tout de suite.

use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::time::Duration;

use jiff::Zoned;
use kobi_session::rappels::{self, Demande};
use kobi_session::stockage::{maintenant_ms, Rappel, Stockage};
use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime};

use crate::chef::{publier, Message};

/// Rappel arrivé à échéance, pour l'interface.
pub const EVENEMENT_RAPPEL: &str = "rappel";

/// Relecture de `rappels.json` au moins toutes les 30 s.
const RELECTURE: Duration = Duration::from_secs(30);

/// Rappel annoncé à l'interface.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RappelUi {
    pub id: String,
    pub texte: String,
    pub echeance_ms: Option<u64>,
}

impl From<&Rappel> for RappelUi {
    fn from(r: &Rappel) -> Self {
        Self {
            id: r.id.clone(),
            texte: r.texte.clone(),
            echeance_ms: r.echeance_ms,
        }
    }
}

/// Canal de réveil du planificateur.
struct Reveil(Mutex<Sender<()>>);

/// Réveille le planificateur (rappel ajouté, changé ou supprimé).
pub fn reveiller<R: Runtime>(app: &AppHandle<R>) {
    if let Some(r) = app.try_state::<Reveil>() {
        let _ = r.0.lock().map(|tx| tx.send(()));
    }
}

/// Lance le planificateur.
pub fn installer<R: Runtime>(app: &AppHandle<R>) {
    let (tx, rx) = mpsc::channel();
    app.manage(Reveil(Mutex::new(tx)));
    let app = app.clone();
    std::thread::Builder::new()
        .name("rappels".into())
        .spawn(move || loop {
            let attente = match crate::stockage::actuel(&app) {
                Some(st) => annoncer_dus(&st, maintenant_ms(), |r| {
                    publier(&app, Message::Rappel(RappelUi::from(r)))
                }),
                None => RELECTURE,
            };
            match rx.recv_timeout(attente) {
                Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        })
        .expect("fil des rappels");
}

/// Annonce les rappels dus à `maintenant` (marqués faits une fois annoncés)
/// et renvoie le temps à attendre avant le prochain.
pub fn annoncer_dus(st: &Stockage, maintenant: u64, mut annoncer: impl FnMut(&Rappel)) -> Duration {
    match st.rappels_dus(maintenant) {
        Ok(dus) => {
            for r in &dus {
                tracing::info!(id = %r.id, "rappel annoncé");
                annoncer(r);
                if let Err(e) = st.marquer_fait(&r.id) {
                    tracing::warn!(erreur = %e, id = %r.id, "rappel annoncé mais pas coché");
                }
            }
        }
        Err(e) => tracing::warn!(erreur = %e, "rappels.json illisible"),
    }
    let prochain = st
        .rappels()
        .unwrap_or_default()
        .iter()
        .filter(|r| !r.fait)
        .filter_map(|r| r.echeance_ms)
        .min();
    match prochain {
        Some(e) => Duration::from_millis(e.saturating_sub(maintenant)).min(RELECTURE),
        None => RELECTURE,
    }
}

/// Traite la phrase si c'est une demande de rappel, de tâche ou de liste :
/// la range et renvoie la phrase de réponse. `None` : ce n'est pas pour nous.
///
/// `source` : d'où vient la demande (conversation, réunion), gardée avec le
/// rappel pour le Carnet.
pub fn traiter(
    st: &Stockage,
    phrase: &str,
    maintenant: &Zoned,
    source: Option<&str>,
) -> Option<String> {
    let demande = rappels::comprendre(phrase, maintenant)?;
    let echeance_ms = demande.echeance_ms();
    Some(match demande {
        Demande::Liste => match st.rappels() {
            Ok(liste) => rappels::resume(&liste, maintenant),
            Err(e) => format!("Je n'arrive pas à lire ta liste : {e}"),
        },
        Demande::Rappel { texte, echeance } => match st.ajouter_rappel(&texte, echeance_ms, source)
        {
            Ok(_) => format!(
                "C'est noté : je te rappelle {} de {texte}.",
                rappels::quand(&echeance, maintenant)
            ),
            Err(e) => format!("Je n'arrive pas à ranger ce rappel : {e}"),
        },
        Demande::Tache { texte } => match st.ajouter_rappel(&texte, None, source) {
            Ok(_) => format!("C'est ajouté à ta liste : {texte}."),
            Err(e) => format!("Je n'arrive pas à ranger cette tâche : {e}"),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stockage() -> (tempfile::TempDir, Stockage) {
        let dir = tempfile::tempdir().unwrap();
        let st = Stockage::new(dir.path().to_path_buf());
        (dir, st)
    }

    #[test]
    fn rappel_dit_puis_annonce_a_l_heure() {
        let (_d, st) = stockage();
        let maintenant = Zoned::now();
        let reponse = traiter(
            &st,
            "Kobi, rappelle-moi dans une minute de boire de l'eau",
            &maintenant,
            None,
        )
        .unwrap();
        assert_eq!(
            reponse,
            "C'est noté : je te rappelle dans 1 minute de boire de l'eau."
        );
        let t0 = maintenant.timestamp().as_millisecond() as u64;

        // Avant l'heure : rien, et le planificateur dort jusqu'à l'échéance.
        let mut annonces = Vec::new();
        let attente = annoncer_dus(&st, t0 + 10_000, |r| annonces.push(r.texte.clone()));
        assert!(annonces.is_empty());
        assert_eq!(attente, RELECTURE);
        let attente = annoncer_dus(&st, t0 + 45_000, |r| annonces.push(r.texte.clone()));
        assert_eq!(attente, Duration::from_secs(15));

        // À l'heure : annoncé une seule fois, puis coché.
        annoncer_dus(&st, t0 + 60_000, |r| annonces.push(r.texte.clone()));
        annoncer_dus(&st, t0 + 61_000, |r| annonces.push(r.texte.clone()));
        assert_eq!(annonces, ["boire de l'eau"]);
        assert!(st.rappels().unwrap()[0].fait);
    }

    #[test]
    fn taches_et_liste() {
        let (_d, st) = stockage();
        let m = Zoned::now();
        assert_eq!(
            traiter(&st, "qu'est-ce que j'ai à faire ?", &m, None).unwrap(),
            "Rien à faire pour l'instant : ta liste est vide."
        );
        assert_eq!(
            traiter(
                &st,
                "ajoute acheter du pain à ma liste",
                &m,
                Some("2026-10-18/100000")
            )
            .unwrap(),
            "C'est ajouté à ta liste : acheter du pain."
        );
        let liste = st.rappels().unwrap();
        assert_eq!(liste[0].echeance_ms, None);
        assert_eq!(liste[0].source.as_deref(), Some("2026-10-18/100000"));
        assert_eq!(
            traiter(&st, "Kobi, qu'est-ce que j'ai à faire ?", &m, None).unwrap(),
            "Une seule chose : acheter du pain."
        );
        // Une tâche sans heure n'est jamais annoncée.
        let mut n = 0;
        annoncer_dus(&st, u64::MAX, |_| n += 1);
        assert_eq!(n, 0);
        assert!(traiter(&st, "où est géré le refresh token ?", &m, None).is_none());
    }
}
