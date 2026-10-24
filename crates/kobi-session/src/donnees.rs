//! Contrôle des données par l'utilisateur : pause (rien n'est transcrit),
//! suppression d'une réunion en un clic, purge après la durée de conservation.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use kobi_stt::Segment;

use crate::config::Retention;
use crate::etat::{Etat, Evenement, Refus};
use crate::reunion::{Date, Reunion};

impl Date {
    /// Jour courant en UTC (le calendrier local viendra de l'interface).
    pub fn aujourdhui_utc() -> Date {
        let secondes = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        Date::depuis_jours((secondes / 86_400) as i64)
    }

    /// Nombre de jours depuis le 1970-01-01 (algorithme de H. Hinnant).
    pub fn en_jours(self) -> i64 {
        let (a, m, j) = (self.annee as i64, self.mois as i64, self.jour as i64);
        let a = if m <= 2 { a - 1 } else { a };
        let ere = a.div_euclid(400);
        let ae = a - ere * 400;
        let mp = (m + 9) % 12;
        let ja = (153 * mp + 2) / 5 + j - 1;
        let je = ae * 365 + ae / 4 - ae / 100 + ja;
        ere * 146_097 + je - 719_468
    }

    pub fn depuis_jours(jours: i64) -> Date {
        let z = jours + 719_468;
        let ere = z.div_euclid(146_097);
        let je = z - ere * 146_097;
        let ae = (je - je / 1460 + je / 36_524 - je / 146_096) / 365;
        let ja = je - (365 * ae + ae / 4 - ae / 100);
        let mp = (5 * ja + 2) / 153;
        let jour = (ja - (153 * mp + 2) / 5 + 1) as u8;
        let mois = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
        let annee = (ae + ere * 400 + i64::from(mois <= 2)) as u16;
        Date { annee, mois, jour }
    }

    /// Date en tête d'un nom de dossier `AAAA-MM-JJ-…`.
    pub fn depuis_nom_de_dossier(nom: &str) -> Option<Date> {
        let tete = nom.get(..10)?;
        let suite = nom.get(10..)?;
        if !(suite.is_empty() || suite.starts_with('-')) {
            return None;
        }
        let mut parties = tete.split('-');
        let annee = parties.next()?;
        let mois = parties.next()?;
        let jour = parties.next()?;
        if annee.len() != 4 || mois.len() != 2 || jour.len() != 2 {
            return None;
        }
        let date = Date {
            annee: annee.parse().ok()?,
            mois: mois.parse().ok()?,
            jour: jour.parse().ok()?,
        };
        // Refuse le 2026-02-31 & co : l'aller-retour doit redonner la date.
        (Date::depuis_jours(date.en_jours()) == date).then_some(date)
    }
}

/// Session en cours : son état et la réunion ouverte. C'est le seul chemin
/// d'écriture de la transcription, et il n'écrit rien hors de l'état
/// `Reunion` (en pause, en veille, à la fin).
#[derive(Debug)]
pub struct Session {
    etat: Etat,
    reunion: Option<Reunion>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            etat: Etat::Veille,
            reunion: None,
        }
    }
}

impl Session {
    pub fn etat(&self) -> Etat {
        self.etat
    }

    pub fn reunion(&self) -> Option<&Reunion> {
        self.reunion.as_ref()
    }

    /// Démarre une réunion : crée son dossier, puis passe en réunion.
    pub fn demarrer(
        &mut self,
        meetings: &Path,
        date: Date,
        titre: Option<&str>,
    ) -> Result<(), ErreurDonnees> {
        let suivant = self.etat.suivant(Evenement::DemarrerReunion)?;
        self.reunion = Some(Reunion::creer(meetings, date, titre)?);
        self.etat = suivant;
        Ok(())
    }

    /// Applique un événement (pause, reprise, fin, activité…). La réunion
    /// est fermée au retour en veille.
    pub fn evenement(&mut self, evenement: Evenement) -> Result<Etat, Refus> {
        self.etat = self.etat.suivant(evenement)?;
        if self.etat == Etat::Veille {
            self.reunion = None;
        }
        Ok(self.etat)
    }

    /// Écrit un segment si la réunion transcrit ; en pause il est jeté sans
    /// rien écrire. Retourne `true` s'il a été écrit.
    pub fn segment(&mut self, segment: &Segment) -> io::Result<bool> {
        match (&mut self.reunion, self.etat.transcrit()) {
            (Some(reunion), true) => reunion.ecrire_segment(segment),
            _ => Ok(false),
        }
    }

    /// La transcription doit-elle tourner ? À consulter avant d'envoyer de
    /// l'audio au moteur : en pause, l'audio n'est pas transcrit du tout.
    pub fn doit_transcrire(&self) -> bool {
        self.etat.transcrit() && self.reunion.is_some()
    }
}

/// Messages dans la langue de l'interface (KOB-266).
#[derive(Debug, thiserror::Error)]
pub enum ErreurDonnees {
    #[error(transparent)]
    Transition(#[from] Refus),
    #[error("{}", kobi_i18n::tr("session.donnees.introuvable", &[("nom", .0)]))]
    Introuvable(String),
    #[error("{}", kobi_i18n::tr("session.donnees.enCours", &[("nom", .0)]))]
    EnCours(String),
    #[error("{}", kobi_i18n::tr("session.donnees.nomRefuse", &[("nom", .0)]))]
    Nom(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}

fn nom_simple(nom: &str) -> bool {
    !nom.is_empty() && nom != "." && nom != ".." && !nom.contains(['/', '\\', ':'])
}

fn est_en_cours(session: Option<&Session>, chemin: &Path) -> bool {
    session
        .and_then(Session::reunion)
        .is_some_and(|r| r.dossier() == chemin)
}

/// Supprime le dossier `meetings/<nom>` et tout son contenu. Refuse la
/// réunion en cours et tout nom qui sortirait du dossier des réunions.
pub fn supprimer_reunion(
    meetings: &Path,
    nom: &str,
    session: Option<&Session>,
) -> Result<(), ErreurDonnees> {
    if !nom_simple(nom) {
        return Err(ErreurDonnees::Nom(nom.to_owned()));
    }
    let chemin = meetings.join(nom);
    if !chemin.is_dir() {
        return Err(ErreurDonnees::Introuvable(nom.to_owned()));
    }
    if est_en_cours(session, &chemin) {
        return Err(ErreurDonnees::EnCours(nom.to_owned()));
    }
    fs::remove_dir_all(&chemin)?;
    Ok(())
}

/// Supprime les réunions plus vieilles que la durée de conservation, d'après
/// la date de leur nom de dossier. Une réunion du jour J est gardée tant que
/// `aujourdhui - J < durée`. Les dossiers sans date, la réunion en cours et
/// tout ce qui n'est pas un dossier sont laissés. Retourne les dossiers
/// supprimés.
pub fn purger(
    meetings: &Path,
    retention: Retention,
    aujourdhui: Date,
    session: Option<&Session>,
) -> Result<Vec<PathBuf>, ErreurDonnees> {
    let Retention::Days(jours) = retention else {
        return Ok(Vec::new());
    };
    let entrees = match fs::read_dir(meetings) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let limite = aujourdhui.en_jours() - i64::from(jours);
    let mut supprimes = Vec::new();
    for entree in entrees {
        let entree = entree?;
        if !entree.file_type()?.is_dir() {
            continue;
        }
        let chemin = entree.path();
        let Some(date) = entree
            .file_name()
            .to_str()
            .and_then(Date::depuis_nom_de_dossier)
        else {
            continue;
        };
        if date.en_jours() <= limite && !est_en_cours(session, &chemin) {
            fs::remove_dir_all(&chemin)?;
            supprimes.push(chemin);
        }
    }
    supprimes.sort();
    Ok(supprimes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::etat::Activite;
    use crate::reunion::{lire_transcript, TRANSCRIPT};
    use kobi_stt::Source;

    const AUJOURDHUI: Date = Date {
        annee: 2026,
        mois: 10,
        jour: 17,
    };

    fn segment(i: usize) -> Segment {
        Segment {
            start: i as f64,
            end: i as f64 + 1.0,
            source: Source::Eux,
            text: format!("segment {i}"),
            is_final: true,
        }
    }

    #[test]
    fn rien_n_est_ecrit_pendant_la_pause() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::default();
        assert!(!session.segment(&segment(0)).unwrap(), "veille");

        session
            .demarrer(dir.path(), AUJOURDHUI, Some("pause"))
            .unwrap();
        let chemin = session.reunion().unwrap().dossier().join(TRANSCRIPT);
        assert!(session.doit_transcrire());
        assert!(session.segment(&segment(1)).unwrap());

        session.evenement(Evenement::Pause).unwrap();
        assert!(!session.doit_transcrire());
        let taille = fs::metadata(&chemin).unwrap().len();
        for i in 2..5 {
            assert!(!session.segment(&segment(i)).unwrap());
        }
        assert_eq!(fs::metadata(&chemin).unwrap().len(), taille);

        session.evenement(Evenement::Reprendre).unwrap();
        assert!(session.segment(&segment(5)).unwrap());
        // Une activité (Kobi répond…) ne coupe pas la transcription.
        session.evenement(Evenement::Interpellation).unwrap();
        assert_eq!(session.etat(), Etat::Reunion(Activite::Ecoute));
        assert!(session.segment(&segment(6)).unwrap());

        session.evenement(Evenement::TerminerReunion).unwrap();
        assert!(!session.segment(&segment(7)).unwrap(), "fin");
        session.evenement(Evenement::CompteRenduEcrit).unwrap();
        assert!(session.reunion().is_none());

        let lus: Vec<_> = lire_transcript(&chemin)
            .unwrap()
            .segments
            .iter()
            .map(|s| s.text.clone())
            .collect();
        assert_eq!(lus, ["segment 1", "segment 5", "segment 6"]);
    }

    #[test]
    fn demarrer_refuse_hors_veille_sans_creer_de_dossier() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::default();
        session.demarrer(dir.path(), AUJOURDHUI, None).unwrap();
        session.evenement(Evenement::Pause).unwrap();
        assert!(matches!(
            session.demarrer(dir.path(), AUJOURDHUI, Some("autre")),
            Err(ErreurDonnees::Transition(_))
        ));
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    fn reunion_du(meetings: &Path, date: Date, titre: &str) -> PathBuf {
        Reunion::creer(meetings, date, Some(titre))
            .unwrap()
            .dossier()
            .to_owned()
    }

    #[test]
    fn suppression_en_un_clic() {
        let dir = tempfile::tempdir().unwrap();
        let vieille = reunion_du(dir.path(), AUJOURDHUI, "a-supprimer");
        fs::write(vieille.join("notes.md"), "notes").unwrap();
        let mut session = Session::default();
        session
            .demarrer(dir.path(), AUJOURDHUI, Some("en cours"))
            .unwrap();

        supprimer_reunion(dir.path(), "2026-10-17-a-supprimer", Some(&session)).unwrap();
        assert!(!vieille.exists());

        assert!(matches!(
            supprimer_reunion(dir.path(), "2026-10-17-en-cours", Some(&session)),
            Err(ErreurDonnees::EnCours(_))
        ));
        assert!(matches!(
            supprimer_reunion(dir.path(), "absente", None),
            Err(ErreurDonnees::Introuvable(_))
        ));
        for nom in ["..", "../x", "a/b", "", "C:x"] {
            assert!(
                matches!(
                    supprimer_reunion(dir.path(), nom, None),
                    Err(ErreurDonnees::Nom(_))
                ),
                "{nom}"
            );
        }
        assert!(dir.path().exists());
    }

    #[test]
    fn purge_apres_la_duree() {
        let dir = tempfile::tempdir().unwrap();
        let jour = |n: i64| Date::depuis_jours(AUJOURDHUI.en_jours() - n);
        let a_29 = reunion_du(dir.path(), jour(29), "j29");
        let a_30 = reunion_du(dir.path(), jour(30), "j30");
        let a_400 = reunion_du(dir.path(), jour(400), "j400");
        let sans_date = dir.path().join("archives");
        fs::create_dir(&sans_date).unwrap();
        fs::write(dir.path().join("2020-01-01-fichier"), "").unwrap();

        // Une réunion en cours n'est jamais purgée, même datée de loin.
        let mut session = Session::default();
        session
            .demarrer(dir.path(), jour(500), Some("en cours"))
            .unwrap();
        let en_cours = session.reunion().unwrap().dossier().to_owned();

        assert!(
            purger(dir.path(), Retention::Unlimited, AUJOURDHUI, Some(&session))
                .unwrap()
                .is_empty()
        );

        let supprimes =
            purger(dir.path(), Retention::Days(30), AUJOURDHUI, Some(&session)).unwrap();
        assert_eq!(supprimes, {
            let mut v = vec![a_30.clone(), a_400.clone()];
            v.sort();
            v
        });
        assert!(a_29.exists() && !a_30.exists() && !a_400.exists());
        assert!(sans_date.exists() && en_cours.exists());
        assert!(dir.path().join("2020-01-01-fichier").exists());

        // 90 et 365 jours.
        let a_100 = reunion_du(dir.path(), jour(100), "j100");
        assert!(
            purger(dir.path(), Retention::Days(365), AUJOURDHUI, Some(&session))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            purger(dir.path(), Retention::Days(90), AUJOURDHUI, Some(&session)).unwrap(),
            vec![a_100]
        );
        assert!(purger(
            &dir.path().join("absent"),
            Retention::Days(30),
            AUJOURDHUI,
            None
        )
        .unwrap()
        .is_empty());
    }

    #[test]
    fn dates() {
        for (jours, date) in [
            (0, (1970, 1, 1)),
            (59, (1970, 3, 1)),
            (11_016, (2000, 2, 29)),
            (20_743, (2026, 10, 17)),
            (20_513, (2026, 3, 1)),
        ] {
            let d = Date {
                annee: date.0,
                mois: date.1,
                jour: date.2,
            };
            assert_eq!(Date::depuis_jours(jours), d);
            assert_eq!(d.en_jours(), jours);
        }
        assert_eq!(
            Date::depuis_nom_de_dossier("2026-10-17-point-d-equipe"),
            Some(AUJOURDHUI)
        );
        assert_eq!(Date::depuis_nom_de_dossier("2026-10-17"), Some(AUJOURDHUI));
        for nom in [
            "2026-02-30-x",
            "2026-13-01-x",
            "26-10-17-x",
            "2026-10-17x",
            "archives",
            "é",
        ] {
            assert_eq!(Date::depuis_nom_de_dossier(nom), None, "{nom}");
        }
        let a = Date::aujourdhui_utc();
        assert!(a.annee >= 2026);
    }
}
