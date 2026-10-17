//! Tampon circulaire des dernières secondes d'audio, par source.
//!
//! Sert à transcrire une question qui commence juste avant le mot-clé : quand
//! « Kobi » est détecté, on relit l'audio depuis un peu avant. Chaque source a
//! sa piste continue ; les blocs y sont placés selon leur horodatage par un
//! [`TrackAligner`] (trou comblé par du silence, chevauchement coupé), puis
//! l'audio plus vieux que la capacité est écrasé.

use std::collections::HashMap;
use std::time::Duration;

use crate::{AudioFrame, Source, TrackAligner, SAMPLE_RATE};

/// Durée gardée par défaut.
pub const DEFAULT_HISTORY: Duration = Duration::from_secs(30);

/// Tolérance de placement des blocs (gigue d'horodatage).
const TOLERANCE: Duration = Duration::from_millis(10);

/// Intervalle demandé hors de ce que le tampon contient.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum HistoryError {
    #[error("intervalle vide ou inversé")]
    Empty,
    #[error("début trop ancien : le tampon commence à {oldest:?}")]
    TooOld { oldest: Duration },
    #[error("fin pas encore reçue : le tampon s'arrête à {end:?}")]
    NotYet { end: Duration },
    #[error("aucun audio reçu pour cette source")]
    UnknownSource,
}

/// Piste circulaire d'une source, 16 kHz mono.
#[derive(Debug, Clone)]
pub struct RingTrack {
    data: Vec<f32>,
    aligner: TrackAligner,
}

impl RingTrack {
    pub fn new(capacity: Duration) -> Self {
        let n = (capacity.as_secs_f64() * SAMPLE_RATE as f64).round() as usize;
        assert!(n > 0, "capacité nulle");
        Self {
            data: vec![0.0; n],
            aligner: TrackAligner::new(SAMPLE_RATE, TOLERANCE),
        }
    }

    /// Ajoute un bloc à la position de son horodatage.
    pub fn push(&mut self, t: Duration, samples: &[f32]) {
        let start = self.aligner.written();
        let placement = self.aligner.place(t, samples);
        let silence = placement.silence;
        self.write(start, silence, placement.samples);
    }

    /// Écrit `silence` zéros puis `samples` à partir de l'indice absolu `start`,
    /// en ne gardant que ce qui tient dans la capacité.
    fn write(&mut self, start: u64, silence: u64, samples: &[f32]) {
        let cap = self.data.len() as u64;
        let end = start + silence + samples.len() as u64;
        // Seuls les `cap` derniers échantillons survivent.
        let first_kept = end.saturating_sub(cap).max(start);
        for i in first_kept..end {
            let v = if i < start + silence {
                0.0
            } else {
                samples[(i - start - silence) as usize]
            };
            self.data[(i % cap) as usize] = v;
        }
    }

    /// Fin de la piste (instant suivant le dernier échantillon reçu).
    pub fn end(&self) -> Duration {
        to_duration(self.aligner.written())
    }

    /// Plus ancien instant encore disponible.
    pub fn oldest(&self) -> Duration {
        to_duration(
            self.aligner
                .written()
                .saturating_sub(self.data.len() as u64),
        )
    }

    /// Copie de l'audio de `t1` (inclus) à `t2` (exclu).
    pub fn read(&self, t1: Duration, t2: Duration) -> Result<Vec<f32>, HistoryError> {
        let (a, b) = (to_samples(t1), to_samples(t2));
        if b <= a {
            return Err(HistoryError::Empty);
        }
        let written = self.aligner.written();
        let oldest = written.saturating_sub(self.data.len() as u64);
        if a < oldest {
            return Err(HistoryError::TooOld {
                oldest: to_duration(oldest),
            });
        }
        if b > written {
            return Err(HistoryError::NotYet {
                end: to_duration(written),
            });
        }
        let cap = self.data.len() as u64;
        Ok((a..b).map(|i| self.data[(i % cap) as usize]).collect())
    }
}

/// Tampons de toutes les sources.
#[derive(Debug, Clone)]
pub struct AudioHistory {
    capacity: Duration,
    tracks: HashMap<Source, RingTrack>,
}

impl Default for AudioHistory {
    fn default() -> Self {
        Self::new(DEFAULT_HISTORY)
    }
}

impl AudioHistory {
    pub fn new(capacity: Duration) -> Self {
        Self {
            capacity,
            tracks: HashMap::new(),
        }
    }

    pub fn push(&mut self, frame: &AudioFrame) {
        let capacity = self.capacity;
        self.tracks
            .entry(frame.source)
            .or_insert_with(|| RingTrack::new(capacity))
            .push(frame.t, &frame.samples);
    }

    pub fn track(&self, source: Source) -> Option<&RingTrack> {
        self.tracks.get(&source)
    }

    /// Audio de `source` entre `t1` et `t2`.
    pub fn read(
        &self,
        source: Source,
        t1: Duration,
        t2: Duration,
    ) -> Result<Vec<f32>, HistoryError> {
        self.tracks
            .get(&source)
            .ok_or(HistoryError::UnknownSource)?
            .read(t1, t2)
    }
}

fn to_samples(t: Duration) -> u64 {
    (t.as_secs_f64() * SAMPLE_RATE as f64).round() as u64
}

fn to_duration(samples: u64) -> Duration {
    Duration::from_secs_f64(samples as f64 / SAMPLE_RATE as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(v: u64) -> Duration {
        Duration::from_millis(v)
    }

    /// Bloc de 20 ms dont chaque échantillon vaut son indice absolu (lisible).
    fn bloc(k: u64) -> AudioFrame {
        AudioFrame {
            source: Source::Moi,
            samples: (k * 320..(k + 1) * 320).map(|i| i as f32).collect(),
            t: ms(k * 20),
        }
    }

    #[test]
    fn lecture_simple() {
        let mut h = AudioHistory::new(Duration::from_secs(1));
        for k in 0..10 {
            h.push(&bloc(k));
        }
        let x = h.read(Source::Moi, ms(10), ms(50)).unwrap();
        assert_eq!(x.len(), 640);
        assert_eq!(x[0], 160.0);
        assert_eq!(*x.last().unwrap(), 799.0);
    }

    #[test]
    fn ecrasement_au_dela_de_la_capacite() {
        let mut h = AudioHistory::new(Duration::from_secs(1));
        for k in 0..100 {
            h.push(&bloc(k)); // 2 s reçues, 1 s gardée
        }
        let t = h.track(Source::Moi).unwrap();
        assert_eq!(t.end(), Duration::from_secs(2));
        assert_eq!(t.oldest(), Duration::from_secs(1));
        assert_eq!(
            h.read(Source::Moi, ms(500), ms(1_500)),
            Err(HistoryError::TooOld {
                oldest: Duration::from_secs(1)
            })
        );
        let x = h.read(Source::Moi, ms(1_000), ms(2_000)).unwrap();
        assert_eq!(x.len(), 16_000);
        assert!(x.iter().enumerate().all(|(i, &v)| v == (16_000 + i) as f32));
    }

    #[test]
    fn lecture_a_cheval_sur_la_frontiere_du_tampon() {
        // Capacité de 1 s = 16 000 échantillons ; après 1,3 s reçues,
        // l'intervalle [0,9 s ; 1,2 s] traverse l'indice 16 000 du tableau.
        let mut h = AudioHistory::new(Duration::from_secs(1));
        for k in 0..65 {
            h.push(&bloc(k));
        }
        let x = h.read(Source::Moi, ms(900), ms(1_200)).unwrap();
        assert_eq!(x.len(), 4_800);
        assert!(x.iter().enumerate().all(|(i, &v)| v == (14_400 + i) as f32));
    }

    #[test]
    fn trou_comble_par_du_silence() {
        let mut h = AudioHistory::new(Duration::from_secs(5));
        h.push(&bloc(0));
        h.push(&bloc(50)); // 1 s plus tard
        let x = h.read(Source::Moi, ms(0), ms(1_020)).unwrap();
        assert_eq!(x[319], 319.0);
        assert!(x[320..16_000].iter().all(|&v| v == 0.0));
        assert_eq!(x[16_000], 16_000.0);
    }

    #[test]
    fn grand_trou_plus_long_que_la_capacite() {
        let mut h = AudioHistory::new(Duration::from_secs(1));
        h.push(&bloc(0));
        h.push(&bloc(500)); // 10 s plus tard
        let x = h.read(Source::Moi, ms(9_020), ms(10_020)).unwrap();
        assert!(x[..15_680].iter().all(|&v| v == 0.0));
        assert_eq!(x[15_680], 160_000.0);
    }

    #[test]
    fn erreurs_et_sources_separees() {
        let mut h = AudioHistory::default();
        h.push(&bloc(0));
        assert_eq!(
            h.read(Source::Eux, ms(0), ms(10)),
            Err(HistoryError::UnknownSource)
        );
        assert_eq!(
            h.read(Source::Moi, ms(10), ms(10)),
            Err(HistoryError::Empty)
        );
        assert!(matches!(
            h.read(Source::Moi, ms(0), ms(40)),
            Err(HistoryError::NotYet { .. })
        ));
        let mut eux = bloc(0);
        eux.source = Source::Eux;
        eux.samples = vec![-1.0; 320];
        h.push(&eux);
        assert_eq!(h.read(Source::Eux, ms(0), ms(20)).unwrap(), vec![-1.0; 320]);
        assert_eq!(h.read(Source::Moi, ms(0), ms(20)).unwrap()[5], 5.0);
    }
}
