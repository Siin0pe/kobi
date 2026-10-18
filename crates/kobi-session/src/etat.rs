//! Machine à états de la session : veille, échange hors réunion (assistant),
//! réunion (avec l'activité de la mascotte), pause, fin.
//!
//! Les transitions sont pures : [`Etat::suivant`] ne fait aucune entrée-sortie
//! et se teste sans audio ni Claude. La session applique l'état retourné et
//! en déduit l'état affiché par la mascotte ([`Etat::mascotte`]).

use std::fmt;

/// Activité de Kobi pendant une réunion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Activite {
    /// La réunion tourne : transcription en direct, Kobi ne fait rien d'autre.
    Calme,
    /// Mot-clé ou raccourci : la question est en train d'être capturée.
    Ecoute,
    /// Question envoyée à Claude, pas encore de texte.
    Reflechit,
    /// Mise à jour des notes en cours.
    Note,
    /// Réponse en flux dans la bulle.
    Repond,
    /// Carte de permission d'écriture affichée.
    Propose,
    /// Action confirmée et réussie.
    Fait,
    /// Erreur affichée.
    Oups,
    /// « Je m'en occupe » (KOB-152) : Claude travaille avec ses outils
    /// (tâche longue), le fil d'activité montre chaque étape.
    Occupe,
}

impl Activite {
    pub const TOUTES: [Activite; 9] = [
        Activite::Calme,
        Activite::Ecoute,
        Activite::Reflechit,
        Activite::Note,
        Activite::Repond,
        Activite::Propose,
        Activite::Fait,
        Activite::Oups,
        Activite::Occupe,
    ];
}

/// État de la session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Etat {
    /// Hors réunion : seul le mot-clé écoute, rien n'est transcrit.
    Veille,
    /// Hors réunion, Kobi a été interpellé : même activité qu'en réunion,
    /// mais rien n'est écrit. Le retour au calme ramène en veille.
    Assistant(Activite),
    /// Réunion en cours.
    Reunion(Activite),
    /// Réunion en pause : ni transcription ni mot-clé en réunion.
    Pause,
    /// Réunion terminée, compte rendu en cours d'écriture.
    Fin,
}

/// Ce qui arrive à la session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Evenement {
    /// « Kobi, démarre la réunion », clic ou menu.
    DemarrerReunion,
    /// Mot-clé détecté ou raccourci pressé.
    Interpellation,
    /// La question est capturée et part chez Claude.
    QuestionEnvoyee,
    /// Mot-clé sans question en 4 s : retour au calme sans appeler Claude.
    PasDeQuestion,
    /// Premier texte de la réponse reçu.
    ReponseCommence,
    /// Fin du flux de réponse.
    ReponseTerminee,
    /// Bloc de notes envoyé à Claude.
    NotesCommencent,
    /// Notes complètes reçues et écrites.
    NotesTerminees,
    /// Claude demande la permission d'écrire (carte Créer / Pas maintenant).
    PermissionDemandee,
    /// L'utilisateur clique « Créer » : l'action s'exécute.
    PermissionAccordee,
    /// « Pas maintenant », ou pas de réponse en 60 s.
    PermissionRefusee,
    /// L'action confirmée a réussi.
    ActionReussie,
    /// Erreur (Claude, MCP, audio, disque…).
    Erreur,
    /// Fin de l'affichage de « fait » ou de « oups ».
    Acquitter,
    /// « Kobi, pause » ou clic.
    Pause,
    /// Reprise après une pause.
    Reprendre,
    /// « Kobi, termine la réunion » ou clic.
    TerminerReunion,
    /// Compte rendu écrit (ou abandonné) : retour en veille.
    CompteRenduEcrit,
    /// Le tour devient une tâche longue, ou une tâche longue tourne encore
    /// au retour au calme : Kobi s'en occupe (KOB-152).
    TacheLongue,
}

impl Evenement {
    pub const TOUS: [Evenement; 19] = [
        Evenement::DemarrerReunion,
        Evenement::Interpellation,
        Evenement::QuestionEnvoyee,
        Evenement::PasDeQuestion,
        Evenement::ReponseCommence,
        Evenement::ReponseTerminee,
        Evenement::NotesCommencent,
        Evenement::NotesTerminees,
        Evenement::PermissionDemandee,
        Evenement::PermissionAccordee,
        Evenement::PermissionRefusee,
        Evenement::ActionReussie,
        Evenement::Erreur,
        Evenement::Acquitter,
        Evenement::Pause,
        Evenement::Reprendre,
        Evenement::TerminerReunion,
        Evenement::CompteRenduEcrit,
        Evenement::TacheLongue,
    ];
}

/// Transition refusée : l'événement n'a pas de sens dans cet état.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refus {
    pub etat: Etat,
    pub evenement: Evenement,
}

impl fmt::Display for Refus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "transition refusée : {:?} dans l'état {:?}",
            self.evenement, self.etat
        )
    }
}

impl std::error::Error for Refus {}

/// État affiché par la mascotte (`idle`, `listen`…, voir `ui/src/states.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mascotte {
    Idle,
    Listen,
    Think,
    Write,
    Answer,
    Suggest,
    Done,
    Error,
    Work,
}

impl Mascotte {
    /// Clé transmise à l'interface (`idle`, `listen`…).
    pub fn cle(self) -> &'static str {
        match self {
            Mascotte::Idle => "idle",
            Mascotte::Listen => "listen",
            Mascotte::Think => "think",
            Mascotte::Write => "write",
            Mascotte::Answer => "answer",
            Mascotte::Suggest => "suggest",
            Mascotte::Done => "done",
            Mascotte::Error => "error",
            Mascotte::Work => "work",
        }
    }
}

impl Etat {
    /// État suivant, ou [`Refus`] si l'événement est hors de propos.
    pub fn suivant(self, evenement: Evenement) -> Result<Etat, Refus> {
        use Activite as A;
        use Etat::*;
        use Evenement as E;

        let suivant = match (self, evenement) {
            (Veille, E::DemarrerReunion) => Some(Reunion(A::Calme)),

            // Une erreur ne fait jamais quitter la réunion : Oups, la
            // transcription continue.
            (Reunion(_), E::Erreur) => Some(Reunion(A::Oups)),
            (Reunion(_), E::Pause) => Some(Pause),
            (Reunion(_), E::TerminerReunion) => Some(Fin),

            (Reunion(a), e) => activite_suivante(a, e).map(Reunion),

            // Hors réunion : une interpellation ouvre un échange, qui se
            // referme en veille. Ni notes, ni pause, ni fin de réunion.
            (Veille, E::Interpellation) => Some(Assistant(A::Ecoute)),
            (Veille, E::TacheLongue) => Some(Assistant(A::Occupe)),
            (Assistant(_), E::Erreur) => Some(Assistant(A::Oups)),
            (Assistant(_), E::NotesCommencent | E::NotesTerminees) => None,
            (Assistant(a), e) => activite_suivante(a, e).map(|a| match a {
                A::Calme => Veille,
                a => Assistant(a),
            }),

            (Pause, E::Reprendre) => Some(Reunion(A::Calme)),
            (Pause, E::TerminerReunion) => Some(Fin),

            (Fin, E::CompteRenduEcrit) => Some(Veille),

            _ => None,
        };
        suivant.ok_or(Refus {
            etat: self,
            evenement,
        })
    }

    /// État de la mascotte correspondant.
    pub fn mascotte(self) -> Mascotte {
        match self {
            Etat::Veille | Etat::Pause => Mascotte::Idle,
            // Le compte rendu s'écrit : Kobi note.
            Etat::Fin => Mascotte::Write,
            Etat::Reunion(a) | Etat::Assistant(a) => match a {
                Activite::Calme => Mascotte::Idle,
                Activite::Ecoute => Mascotte::Listen,
                Activite::Reflechit => Mascotte::Think,
                Activite::Note => Mascotte::Write,
                Activite::Repond => Mascotte::Answer,
                Activite::Propose => Mascotte::Suggest,
                Activite::Fait => Mascotte::Done,
                Activite::Oups => Mascotte::Error,
                Activite::Occupe => Mascotte::Work,
            },
        }
    }

    /// Une réunion est ouverte (en cours ou en pause).
    pub fn en_reunion(self) -> bool {
        matches!(self, Etat::Reunion(_) | Etat::Pause)
    }

    /// La transcription tourne.
    pub fn transcrit(self) -> bool {
        matches!(self, Etat::Reunion(_))
    }

    /// Tous les états, pour les tests exhaustifs.
    pub fn tous() -> impl Iterator<Item = Etat> {
        [Etat::Veille, Etat::Pause, Etat::Fin]
            .into_iter()
            .chain(Activite::TOUTES.into_iter().map(Etat::Reunion))
            .chain(Activite::TOUTES.into_iter().map(Etat::Assistant))
    }
}

fn activite_suivante(activite: Activite, evenement: Evenement) -> Option<Activite> {
    use Activite::*;
    use Evenement as E;

    match (activite, evenement) {
        // On peut interpeller Kobi à tout moment (KOB-134) : la réponse en
        // cours continue dans l'historique, la carte reste cliquable, la
        // nouvelle question passe après.
        (Calme | Note | Fait | Oups | Reflechit | Repond | Propose | Occupe, E::Interpellation) => {
            Some(Ecoute)
        }
        (Ecoute, E::Interpellation) => Some(Ecoute),
        (Ecoute, E::QuestionEnvoyee) => Some(Reflechit),
        (Ecoute, E::PasDeQuestion) => Some(Calme),
        (Reflechit, E::ReponseCommence) => Some(Repond),
        // Réponse vide (ou seulement une action) : retour au calme.
        (Reflechit | Repond, E::ReponseTerminee) => Some(Calme),

        // Tâche longue (KOB-152) : Kobi reste « je m'en occupe » jusqu'au
        // résultat, ou jusqu'à ce qu'on l'interpelle ; il y revient au calme
        // tant qu'une tâche tourne.
        (Reflechit | Repond | Calme, E::TacheLongue) => Some(Occupe),
        (Occupe, E::TacheLongue) => Some(Occupe),
        (Occupe, E::ReponseCommence) => Some(Repond),
        (Occupe, E::ReponseTerminee) => Some(Calme),
        (Occupe, E::PermissionDemandee) => Some(Propose),

        // Les notes ne démarrent qu'au calme ; leur fin ramène au calme.
        (Calme, E::NotesCommencent) => Some(Note),
        (Note, E::NotesTerminees) => Some(Calme),
        // Notes terminées pendant qu'autre chose s'affiche : rien ne change.
        (Ecoute | Reflechit | Repond | Propose | Fait | Oups | Occupe, E::NotesTerminees) => {
            Some(activite)
        }

        // Une écriture peut être demandée pendant la réflexion, la réponse
        // ou les notes (mode continu).
        (Reflechit | Repond | Note, E::PermissionDemandee) => Some(Propose),
        (Propose, E::PermissionAccordee) => Some(Reflechit),
        (Propose, E::PermissionRefusee) => Some(Calme),
        (Reflechit | Repond, E::ActionReussie) => Some(Fait),

        (Fait | Oups, E::Acquitter) => Some(Calme),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Activite as A;
    use Etat::*;
    use Evenement as E;

    /// Table complète des transitions autorisées. Tout couple absent doit
    /// être refusé.
    fn autorisees() -> Vec<(Etat, Evenement, Etat)> {
        let mut t = vec![
            (Veille, E::DemarrerReunion, Reunion(A::Calme)),
            (Pause, E::Reprendre, Reunion(A::Calme)),
            (Pause, E::TerminerReunion, Fin),
            (Fin, E::CompteRenduEcrit, Veille),
            (Veille, E::Interpellation, Assistant(A::Ecoute)),
            (Veille, E::TacheLongue, Assistant(A::Occupe)),
            (Reunion(A::Calme), E::Interpellation, Reunion(A::Ecoute)),
            (Reunion(A::Note), E::Interpellation, Reunion(A::Ecoute)),
            (Reunion(A::Fait), E::Interpellation, Reunion(A::Ecoute)),
            (Reunion(A::Oups), E::Interpellation, Reunion(A::Ecoute)),
            (Reunion(A::Ecoute), E::Interpellation, Reunion(A::Ecoute)),
            (Reunion(A::Reflechit), E::Interpellation, Reunion(A::Ecoute)),
            (Reunion(A::Repond), E::Interpellation, Reunion(A::Ecoute)),
            (Reunion(A::Propose), E::Interpellation, Reunion(A::Ecoute)),
            (
                Reunion(A::Ecoute),
                E::QuestionEnvoyee,
                Reunion(A::Reflechit),
            ),
            (Reunion(A::Ecoute), E::PasDeQuestion, Reunion(A::Calme)),
            (
                Reunion(A::Reflechit),
                E::ReponseCommence,
                Reunion(A::Repond),
            ),
            (Reunion(A::Reflechit), E::ReponseTerminee, Reunion(A::Calme)),
            (Reunion(A::Repond), E::ReponseTerminee, Reunion(A::Calme)),
            (Reunion(A::Calme), E::NotesCommencent, Reunion(A::Note)),
            (Reunion(A::Note), E::NotesTerminees, Reunion(A::Calme)),
            (
                Reunion(A::Reflechit),
                E::PermissionDemandee,
                Reunion(A::Propose),
            ),
            (
                Reunion(A::Repond),
                E::PermissionDemandee,
                Reunion(A::Propose),
            ),
            (Reunion(A::Note), E::PermissionDemandee, Reunion(A::Propose)),
            (
                Reunion(A::Propose),
                E::PermissionAccordee,
                Reunion(A::Reflechit),
            ),
            (Reunion(A::Propose), E::PermissionRefusee, Reunion(A::Calme)),
            (Reunion(A::Reflechit), E::ActionReussie, Reunion(A::Fait)),
            (Reunion(A::Repond), E::ActionReussie, Reunion(A::Fait)),
            (Reunion(A::Fait), E::Acquitter, Reunion(A::Calme)),
            (Reunion(A::Oups), E::Acquitter, Reunion(A::Calme)),
            (Reunion(A::Occupe), E::Interpellation, Reunion(A::Ecoute)),
            (Reunion(A::Reflechit), E::TacheLongue, Reunion(A::Occupe)),
            (Reunion(A::Repond), E::TacheLongue, Reunion(A::Occupe)),
            (Reunion(A::Calme), E::TacheLongue, Reunion(A::Occupe)),
            (Reunion(A::Occupe), E::TacheLongue, Reunion(A::Occupe)),
            (Reunion(A::Occupe), E::ReponseCommence, Reunion(A::Repond)),
            (Reunion(A::Occupe), E::ReponseTerminee, Reunion(A::Calme)),
            (
                Reunion(A::Occupe),
                E::PermissionDemandee,
                Reunion(A::Propose),
            ),
        ];
        // Hors réunion, les mêmes transitions d'activité, le calme valant veille.
        let reunion: Vec<_> = t
            .iter()
            .filter_map(|&(e, ev, s)| match (e, s) {
                (Reunion(a), Reunion(b)) if ev != E::NotesCommencent && ev != E::NotesTerminees => {
                    Some((
                        Assistant(a),
                        ev,
                        if b == A::Calme { Veille } else { Assistant(b) },
                    ))
                }
                _ => None,
            })
            .collect();
        t.extend(reunion);
        for a in Activite::TOUTES {
            t.push((Assistant(a), E::Erreur, Assistant(A::Oups)));
            t.push((Reunion(a), E::Erreur, Reunion(A::Oups)));
            t.push((Reunion(a), E::Pause, Pause));
            t.push((Reunion(a), E::TerminerReunion, Fin));
            if !matches!(a, A::Calme | A::Note) {
                t.push((Reunion(a), E::NotesTerminees, Reunion(a)));
            }
        }
        t
    }

    #[test]
    fn chaque_transition_autorisee() {
        for (etat, evenement, attendu) in autorisees() {
            assert_eq!(
                etat.suivant(evenement),
                Ok(attendu),
                "{etat:?} + {evenement:?}"
            );
        }
    }

    #[test]
    fn chaque_transition_absente_de_la_table_est_refusee() {
        let table = autorisees();
        let mut refusees = 0;
        for etat in Etat::tous() {
            for evenement in Evenement::TOUS {
                if table
                    .iter()
                    .any(|(e, ev, _)| *e == etat && *ev == evenement)
                {
                    continue;
                }
                assert_eq!(
                    etat.suivant(evenement),
                    Err(Refus { etat, evenement }),
                    "{etat:?} + {evenement:?} aurait dû être refusé"
                );
                refusees += 1;
            }
        }
        // 21 états × 19 événements, moins les autorisées.
        assert_eq!(refusees + table.len(), 21 * 19);
    }

    #[test]
    fn table_sans_doublon() {
        let table = autorisees();
        for (i, (e, ev, _)) in table.iter().enumerate() {
            assert!(
                !table[i + 1..]
                    .iter()
                    .any(|(e2, ev2, _)| e2 == e && ev2 == ev),
                "doublon {e:?} + {ev:?}"
            );
        }
    }

    #[test]
    fn hors_reunion_rien_ne_reveille_la_transcription() {
        for evenement in Evenement::TOUS {
            if let Ok(suivant) = Veille.suivant(evenement) {
                assert_eq!(suivant.transcrit(), evenement == E::DemarrerReunion);
            }
        }
        for a in Activite::TOUTES {
            for evenement in Evenement::TOUS {
                if let Ok(suivant) = Assistant(a).suivant(evenement) {
                    assert!(!suivant.transcrit() && !suivant.en_reunion());
                }
            }
        }
        assert!(!Veille.transcrit());
        assert!(!Pause.transcrit());
        assert!(!Fin.transcrit());
    }

    #[test]
    fn une_erreur_ne_fait_jamais_quitter_la_reunion() {
        for a in Activite::TOUTES {
            let suivant = Reunion(a).suivant(E::Erreur).unwrap();
            assert!(suivant.en_reunion() && suivant.transcrit());
        }
    }

    #[test]
    fn scenario_question_puis_ticket_confirme() {
        let parcours = [
            (E::DemarrerReunion, Mascotte::Idle),
            (E::NotesCommencent, Mascotte::Write),
            (E::Interpellation, Mascotte::Listen),
            (E::QuestionEnvoyee, Mascotte::Think),
            (E::ReponseCommence, Mascotte::Answer),
            (E::PermissionDemandee, Mascotte::Suggest),
            (E::PermissionAccordee, Mascotte::Think),
            (E::ActionReussie, Mascotte::Done),
            (E::Acquitter, Mascotte::Idle),
            (E::Pause, Mascotte::Idle),
            (E::Reprendre, Mascotte::Idle),
            (E::Erreur, Mascotte::Error),
            (E::Acquitter, Mascotte::Idle),
            (E::TerminerReunion, Mascotte::Write),
            (E::CompteRenduEcrit, Mascotte::Idle),
        ];
        let mut etat = Veille;
        for (evenement, mascotte) in parcours {
            etat = etat.suivant(evenement).unwrap();
            assert_eq!(etat.mascotte(), mascotte, "après {evenement:?}");
        }
        assert_eq!(etat, Veille);
    }

    #[test]
    fn question_hors_reunion_puis_veille() {
        let parcours = [
            (E::Interpellation, Mascotte::Listen),
            (E::QuestionEnvoyee, Mascotte::Think),
            (E::ReponseCommence, Mascotte::Answer),
            (E::ReponseTerminee, Mascotte::Idle),
        ];
        let mut etat = Veille;
        for (evenement, mascotte) in parcours {
            etat = etat.suivant(evenement).unwrap();
            assert_eq!(etat.mascotte(), mascotte, "après {evenement:?}");
        }
        assert_eq!(etat, Veille);
        assert_eq!(Assistant(A::Ecoute).suivant(E::PasDeQuestion), Ok(Veille));
    }

    #[test]
    fn cles_de_la_mascotte() {
        let cles: Vec<_> = Etat::tous().map(|e| e.mascotte().cle()).collect();
        for cle in [
            "idle", "listen", "think", "write", "answer", "suggest", "done", "error",
        ] {
            assert!(cles.contains(&cle), "{cle} jamais affiché");
        }
    }

    #[test]
    fn refus_lisible() {
        let refus = Veille.suivant(E::Pause).unwrap_err();
        assert_eq!(
            refus.to_string(),
            "transition refusée : Pause dans l'état Veille"
        );
    }
}
