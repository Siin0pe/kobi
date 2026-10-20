//! Latence de Kobi en réunion (KOB-85) : pour chaque interpellation, le
//! temps jusqu'au premier mot de la réponse et le temps total, depuis la
//! question envoyée à Claude. Le compte rendu se termine par leur résumé
//! (médiane, 90e centile), pour suivre Kobi d'une réunion à l'autre.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Une interpellation mesurée.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mesure {
    pub premier_mot: Duration,
    pub total: Duration,
}

/// Interpellations d'une réunion : en cours (par tour), puis mesurées.
#[derive(Debug, Default)]
pub struct Latences {
    en_cours: HashMap<u64, (Instant, Option<Duration>)>,
    mesures: Vec<Mesure>,
}

impl Latences {
    /// Question du `tour` envoyée à Claude.
    pub fn question(&mut self, tour: u64, quand: Instant) {
        self.en_cours.insert(tour, (quand, None));
    }

    /// Premier texte de la réponse montré (le premier seulement compte).
    pub fn premier_mot(&mut self, tour: u64, quand: Instant) {
        if let Some((debut, premier @ None)) = self.en_cours.get_mut(&tour) {
            *premier = Some(quand.saturating_duration_since(*debut));
        }
    }

    /// Réponse complète : l'interpellation est mesurée. Sans texte montré
    /// avant, le premier mot arrive avec la fin.
    pub fn fin(&mut self, tour: u64, quand: Instant) {
        if let Some((debut, premier)) = self.en_cours.remove(&tour) {
            let total = quand.saturating_duration_since(debut);
            self.mesures.push(Mesure {
                premier_mot: premier.unwrap_or(total),
                total,
            });
        }
    }

    /// Tour sans réponse à mesurer (erreur, phrase qui n'était pas pour
    /// Kobi, interruption) : oublié.
    pub fn abandon(&mut self, tour: u64) {
        self.en_cours.remove(&tour);
    }

    #[cfg(test)]
    pub fn mesures(&self) -> &[Mesure] {
        &self.mesures
    }

    /// Section Markdown du compte rendu, `None` sans interpellation mesurée.
    pub fn resume(&self) -> Option<String> {
        if self.mesures.is_empty() {
            return None;
        }
        let premiers: Vec<Duration> = self.mesures.iter().map(|m| m.premier_mot).collect();
        let totaux: Vec<Duration> = self.mesures.iter().map(|m| m.total).collect();
        let n = self.mesures.len().to_string();
        let ligne = |cle: &str, v: &[Duration]| {
            kobi_i18n::tr(
                cle,
                &[
                    ("mediane", &secondes(centile(v, 50))),
                    ("p90", &secondes(centile(v, 90))),
                ],
            )
        };
        Some(format!(
            "## {}\n\n{}\n\n- {}\n- {}\n",
            kobi_i18n::tr("coque.latence.titre", &[]),
            kobi_i18n::tr("coque.latence.interpellations", &[("n", &n)]),
            ligne("coque.latence.premierMot", &premiers),
            ligne("coque.latence.total", &totaux),
        ))
    }
}

/// Centile `p` (rang le plus proche) d'une liste non vide.
fn centile(valeurs: &[Duration], p: usize) -> Duration {
    let mut v = valeurs.to_vec();
    v.sort();
    let rang = (p * v.len()).div_ceil(100).max(1);
    v[rang.min(v.len()) - 1]
}

/// Durée en secondes, à une décimale (« 2,4 s » / « 2.4 s »).
fn secondes(d: Duration) -> String {
    let mut nombre = format!("{:.1}", d.as_secs_f64());
    if kobi_i18n::courante() == kobi_i18n::Langue::Fr {
        nombre = nombre.replace('.', ",");
    }
    kobi_i18n::tr("coque.latence.secondes", &[("s", &nombre)])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn premier_mot_puis_fin() {
        let t0 = Instant::now();
        let mut l = Latences::default();
        l.question(1, t0);
        l.premier_mot(1, t0 + ms(1_200));
        // Les morceaux suivants ne changent pas le premier mot.
        l.premier_mot(1, t0 + ms(1_900));
        l.fin(1, t0 + ms(4_000));
        assert_eq!(
            l.mesures(),
            [Mesure {
                premier_mot: ms(1_200),
                total: ms(4_000)
            }]
        );
    }

    #[test]
    fn sans_texte_avant_la_fin_et_abandons() {
        let t0 = Instant::now();
        let mut l = Latences::default();
        l.question(1, t0);
        l.fin(1, t0 + ms(3_000));
        // Erreur ou phrase qui n'était pas pour Kobi : rien n'est mesuré.
        l.question(2, t0);
        l.premier_mot(2, t0 + ms(500));
        l.abandon(2);
        l.fin(2, t0 + ms(9_000));
        // Une fin sans question connue (réponse d'avant la réunion) non plus.
        l.fin(7, t0 + ms(1_000));
        assert_eq!(
            l.mesures(),
            [Mesure {
                premier_mot: ms(3_000),
                total: ms(3_000)
            }]
        );
    }

    #[test]
    fn centiles_au_rang_le_plus_proche() {
        let v: Vec<Duration> = (1..=10).map(Duration::from_secs).collect();
        assert_eq!(centile(&v, 50), Duration::from_secs(5));
        assert_eq!(centile(&v, 90), Duration::from_secs(9));
        assert_eq!(centile(&v[..1], 90), Duration::from_secs(1));
        assert_eq!(centile(&v[..3], 50), Duration::from_secs(2));
    }

    #[test]
    fn resume_du_compte_rendu() {
        assert_eq!(Latences::default().resume(), None, "pas d'interpellation");
        let t0 = Instant::now();
        let mut l = Latences::default();
        for (tour, premier, total) in [(1, 1_000, 3_000), (2, 2_000, 5_000), (3, 4_000, 9_000)] {
            l.question(tour, t0);
            l.premier_mot(tour, t0 + ms(premier));
            l.fin(tour, t0 + ms(total));
        }
        assert_eq!(
            l.resume().unwrap(),
            "## Latence de Kobi\n\n3 interpellations mesurées, depuis la question envoyée.\n\n\
             - Premier mot : médiane 2,0 s, 90e centile 4,0 s\n\
             - Réponse complète : médiane 5,0 s, 90e centile 9,0 s\n"
        );
    }
}
