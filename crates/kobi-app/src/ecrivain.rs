//! Écrivain (KOB-134) : toutes les écritures sur le disque passent par un
//! fil à part, dans l'ordre où le chef les demande. Chaque segment et chaque
//! tour de conversation est poussé sur le disque (fsync) : sur un disque
//! lent, cela ne retarde ni les questions ni l'interface.

use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};

use kobi_session::stockage::{Conversation, Origine, Qui, ReunionOuverte, Stockage};
use kobi_stt::Segment;

/// Ce que l'écrivain garde ouvert.
#[derive(Default)]
pub struct Disque {
    reunion: Option<ReunionOuverte>,
    conversation: Option<Conversation>,
    compte: Option<Box<dyn Fn(Nouvelle) + Send>>,
}

impl Disque {
    fn compter(&self, compte: Nouvelle) {
        if let Some(f) = &self.compte {
            f(compte);
        }
    }
}

type Ecriture = Box<dyn FnOnce(&mut Disque) + Send>;

/// Poignée de l'écrivain.
pub struct Ecrivain(Sender<Ecriture>);

/// Ce que l'écrivain signale au chef.
#[derive(Debug, Clone, PartialEq)]
pub enum Nouvelle {
    /// Fichiers de la réunion créés, dans ce dossier.
    ReunionOuverte(PathBuf),
    /// `notes.md` écrit, dans ce dossier de réunion.
    NotesEcrites(PathBuf),
    /// Compte rendu écrit, à ce chemin.
    CompteRenduEcrit(PathBuf),
    /// Titre de la réunion donné (KOB-223) : son dossier (renommé si
    /// `ancien` est là) et le titre retenu (un titre manuel reste).
    ReunionNommee {
        dossier: PathBuf,
        ancien: Option<String>,
        titre: Option<String>,
    },
    Erreur(String),
}

impl Ecrivain {
    /// Lance le fil ; `compte` reçoit ce que le chef doit savoir.
    pub fn lancer(compte: impl Fn(Nouvelle) + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel::<Ecriture>();
        std::thread::Builder::new()
            .name("ecrivain".into())
            .spawn(move || {
                let mut disque = Disque {
                    compte: Some(Box::new(compte)),
                    ..Disque::default()
                };
                while let Ok(ecriture) = rx.recv() {
                    ecriture(&mut disque);
                }
            })
            .expect("fil de l'écrivain");
        Self(tx)
    }

    fn envoyer(&self, ecriture: impl FnOnce(&mut Disque) + Send + 'static) {
        let _ = self.0.send(Box::new(ecriture));
    }

    /// Crée les fichiers de la réunion.
    pub fn ouvrir_reunion(&self, stockage: Stockage, debut_ms: u64) {
        self.envoyer(move |d| match stockage.nouvelle_reunion(debut_ms, None) {
            Ok(r) => {
                let dossier = r.dossier().to_path_buf();
                d.reunion = Some(r);
                d.compter(Nouvelle::ReunionOuverte(dossier));
            }
            Err(e) => d.compter(Nouvelle::Erreur(format!(
                "Je n'arrive pas à créer le dossier de la réunion : {e}"
            ))),
        });
    }

    /// Donne un titre à la réunion en cours (KOB-223) : son dossier est
    /// renommé ici, entre deux segments, sans perdre l'écriture.
    pub fn nommer(&self, titre: String, origine: Origine) {
        self.envoyer(move |d| {
            let Some(r) = &mut d.reunion else { return };
            let nouvelle = match r.nommer(&titre, origine) {
                Ok(ancien) => Nouvelle::ReunionNommee {
                    dossier: r.dossier().to_path_buf(),
                    ancien,
                    titre: r.meta().titre.clone(),
                },
                Err(e) => {
                    tracing::warn!(erreur = %e, "titre de la réunion");
                    Nouvelle::Erreur(format!("Je n'arrive pas à renommer la réunion : {e}"))
                }
            };
            d.compter(nouvelle);
        });
    }

    pub fn segment(&self, segment: Segment) {
        self.envoyer(move |d| {
            if let Some(r) = &mut d.reunion {
                if let Err(e) = r.ecrire_segment(&segment) {
                    tracing::warn!(erreur = %e, "transcript.jsonl");
                }
            }
        });
    }

    pub fn notes(&self, notes: String) {
        self.envoyer(move |d| {
            let Some(r) = &d.reunion else { return };
            match r.ecrire_notes(&notes) {
                Ok(()) => {
                    let dossier = r.dossier().to_path_buf();
                    d.compter(Nouvelle::NotesEcrites(dossier));
                }
                Err(e) => tracing::warn!(erreur = %e, "notes.md"),
            }
        });
    }

    /// Compte rendu, notes finales et fin de la réunion ; ferme ses fichiers.
    pub fn terminer_reunion(&self, compte_rendu: String, notes: String, fin_ms: u64) {
        self.envoyer(move |d| {
            let Some(mut r) = d.reunion.take() else {
                d.compter(Nouvelle::Erreur(
                    "Compte rendu non rangé : les fichiers de la réunion manquent.".into(),
                ));
                return;
            };
            let chemin = r.dossier().join("compte-rendu.md");
            let ecrit = r.ecrire_compte_rendu(&compte_rendu);
            if let Err(e) = &ecrit {
                tracing::warn!(erreur = %e, "compte-rendu.md");
            }
            // Notes et fin de réunion tentées même sans compte rendu.
            if let Err(e) = r.ecrire_notes(&notes) {
                tracing::warn!(erreur = %e, "notes.md");
            }
            if let Err(e) = r.terminer(fin_ms) {
                tracing::warn!(erreur = %e, "reunion.json");
            }
            d.compter(match ecrit {
                Ok(()) => Nouvelle::CompteRenduEcrit(chemin),
                Err(e) => Nouvelle::Erreur(format!(
                    "Compte rendu non écrit dans {} : {e}",
                    chemin.display()
                )),
            });
        });
    }

    /// Range un tour de la conversation du quotidien ; la conversation est
    /// créée au premier tour.
    pub fn tour(&self, stockage: Stockage, debut_ms: u64, qui: Qui, texte: String, voix: bool) {
        self.envoyer(move |d| {
            let c = d
                .conversation
                .get_or_insert_with(|| stockage.nouvelle_conversation(debut_ms));
            if let Err(e) = c.ajouter(qui, &texte, voix) {
                tracing::warn!(erreur = %e, "conversation non rangée");
            }
        });
    }

    /// La conversation est finie : le prochain tour en ouvre une autre.
    pub fn fermer_conversation(&self) {
        self.envoyer(|d| d.conversation = None);
    }

    /// Attend que tout ce qui a été demandé soit écrit (tests).
    #[cfg(test)]
    pub fn attendre(&self) {
        let (tx, rx) = mpsc::channel();
        self.envoyer(move |_| {
            let _ = tx.send(());
        });
        let _ = rx.recv();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn segment(t: f64, texte: &str) -> Segment {
        Segment {
            text: texte.into(),
            start: t,
            end: t + 1.0,
            is_final: true,
            source: kobi_stt::Source::Moi,
        }
    }

    #[test]
    fn reunion_ecrite_dans_l_ordre() {
        let dir = tempfile::tempdir().unwrap();
        let stockage = Stockage::new(dir.path());
        let comptes = Arc::new(Mutex::new(Vec::new()));
        let c = comptes.clone();
        let e = Ecrivain::lancer(move |compte| c.lock().unwrap().push(compte));
        e.ouvrir_reunion(stockage.clone(), 1_000);
        e.segment(segment(0.0, "on commence"));
        e.segment(segment(2.0, "le budget"));
        e.notes("- budget".into());
        e.terminer_reunion("## Résumé\n- budget".into(), "- budget".into(), 2_000);
        e.attendre();
        let comptes = comptes.lock().unwrap();
        let Nouvelle::ReunionOuverte(dossier) = &comptes[0] else {
            panic!("{comptes:?}");
        };
        let transcript = std::fs::read_to_string(dossier.join("transcript.jsonl")).unwrap();
        let textes: Vec<_> = transcript
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str::<Segment>(l).unwrap().text)
            .collect();
        assert_eq!(textes, ["on commence", "le budget"]);
        assert_eq!(comptes[1], Nouvelle::NotesEcrites(dossier.clone()));
        assert_eq!(
            comptes[2],
            Nouvelle::CompteRenduEcrit(dossier.join("compte-rendu.md"))
        );
        let lue = stockage
            .lire_reunion(&stockage.reunions().unwrap()[0].nom)
            .unwrap();
        assert_eq!(lue.resume.fin_ms, Some(2_000));
        assert_eq!(lue.notes.trim(), "- budget");
    }

    #[test]
    fn compte_rendu_non_ecrit_signale() {
        let dir = tempfile::tempdir().unwrap();
        let stockage = Stockage::new(dir.path());
        let comptes = Arc::new(Mutex::new(Vec::new()));
        let c = comptes.clone();
        let e = Ecrivain::lancer(move |compte| c.lock().unwrap().push(compte));
        e.ouvrir_reunion(stockage.clone(), 1_000);
        e.attendre();
        let Some(Nouvelle::ReunionOuverte(dossier)) = comptes.lock().unwrap().first().cloned()
        else {
            panic!("{:?}", comptes.lock().unwrap());
        };
        // Un dossier à la place du fichier : le renommage échoue.
        std::fs::create_dir(dossier.join("compte-rendu.md")).unwrap();
        e.terminer_reunion("## Résumé".into(), "- budget".into(), 2_000);
        e.attendre();
        let comptes = comptes.lock().unwrap();
        match &comptes[1] {
            Nouvelle::Erreur(message) => {
                assert!(
                    message.starts_with("Compte rendu non écrit dans "),
                    "{message}"
                );
                assert!(message.contains("compte-rendu.md"), "{message}");
            }
            autre => panic!("{autre:?}"),
        }
        assert_eq!(comptes.len(), 2, "{comptes:?}");
        // Notes et fin de réunion écrites quand même.
        let lue = stockage
            .lire_reunion(&stockage.reunions().unwrap()[0].nom)
            .unwrap();
        assert_eq!(lue.resume.fin_ms, Some(2_000));
        assert_eq!(lue.notes.trim(), "- budget");
    }

    #[test]
    fn conversation_creee_au_premier_tour() {
        let dir = tempfile::tempdir().unwrap();
        let stockage = Stockage::new(dir.path());
        let e = Ecrivain::lancer(|_| {});
        e.tour(
            stockage.clone(),
            1_000,
            Qui::Moi,
            "quelle heure ?".into(),
            true,
        );
        e.tour(stockage.clone(), 1_000, Qui::Kobi, "midi".into(), false);
        e.fermer_conversation();
        e.tour(
            stockage.clone(),
            90_000_000,
            Qui::Moi,
            "et demain ?".into(),
            false,
        );
        e.attendre();
        let conversations = stockage.conversations().unwrap();
        assert_eq!(conversations.len(), 2, "{conversations:?}");
    }
}
