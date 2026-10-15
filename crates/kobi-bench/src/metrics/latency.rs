//! Délais de transcription, mesurés depuis la fin de chaque mot dans
//! l'audio.
//!
//! - **Premier mot** : fin du mot dans l'audio → première émission (partielle
//!   ou finale) qui le contient.
//! - **Final** : fin du mot → premier segment final qui le contient.
//! - **Fin de phrase** : délai final du dernier mot de chaque énoncé de la
//!   référence.
//!
//! Les délais sont exprimés en secondes d'audio (temps réel écoulé × vitesse
//! de relecture) ; ils valent le temps réel quand la relecture est à 1×.

use kobi_stt::replay::Emission;
use kobi_stt::Source;
use serde::{Deserialize, Serialize};

use super::normalize::{normalize, Lang};
use super::reference::RefWord;
use super::wer::{align, Op};

/// Marge autour des horodatages d'un segment pour chercher les mots de la
/// référence qu'il contient.
pub const WINDOW_MARGIN: f64 = 2.0;

/// Statistiques d'une série de délais, en secondes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DelayStats {
    pub count: usize,
    pub mean: f64,
    pub median: f64,
    pub p90: f64,
    pub max: f64,
}

impl DelayStats {
    pub fn from_samples(mut samples: Vec<f64>) -> Option<Self> {
        if samples.is_empty() {
            return None;
        }
        samples.sort_by(f64::total_cmp);
        let count = samples.len();
        Some(Self {
            count,
            mean: samples.iter().sum::<f64>() / count as f64,
            median: percentile(&samples, 50.0),
            p90: percentile(&samples, 90.0),
            max: samples[count - 1],
        })
    }
}

/// Centile par interpolation linéaire sur des valeurs triées.
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    let rank = p / 100.0 * (sorted.len() - 1) as f64;
    let (lo, hi) = (rank.floor() as usize, rank.ceil() as usize);
    sorted[lo] + (sorted[hi] - sorted[lo]) * (rank - lo as f64)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Delays {
    /// Mots de la référence.
    pub ref_words: usize,
    pub first_word: Option<DelayStats>,
    pub final_word: Option<DelayStats>,
    pub sentence_final: Option<DelayStats>,
}

/// Calcule les délais d'une relecture. Renvoie `None` si la relecture était
/// sans attente (`speed == 0`), les délais n'ayant alors pas de sens.
pub fn delays(
    reference: &[RefWord],
    emissions: &[Emission],
    speed: f64,
    lang: Option<Lang>,
) -> Option<Delays> {
    if speed <= 0.0 {
        return None;
    }
    let mut first = Vec::new();
    let mut fin = Vec::new();
    let mut sentence = Vec::new();

    for source in [Source::Moi, Source::Eux] {
        let mut refs: Vec<&RefWord> = reference.iter().filter(|w| w.source == source).collect();
        refs.sort_by(|a, b| a.end.total_cmp(&b.end));
        let mut first_seen = vec![None::<f64>; refs.len()];
        let mut final_seen = vec![None::<f64>; refs.len()];

        for e in emissions.iter().filter(|e| e.segment.source == source) {
            let seg = &e.segment;
            let lo = seg.start - WINDOW_MARGIN;
            let hi = (seg.end + WINDOW_MARGIN).min(e.audio_pos + 1e-6);
            let from = refs.partition_point(|w| w.end < lo);
            let to = refs.partition_point(|w| w.end <= hi);
            if from >= to {
                continue;
            }
            let window: Vec<&str> = refs[from..to].iter().map(|w| w.text.as_str()).collect();
            let hyp = normalize(&seg.text, lang);
            let hyp: Vec<&str> = hyp.iter().map(String::as_str).collect();
            let t = e.wall * speed;
            for op in align(&window, &hyp) {
                if let Op::Match { r, .. } = op {
                    let k = from + r;
                    first_seen[k].get_or_insert(t);
                    if seg.is_final {
                        final_seen[k].get_or_insert(t);
                    }
                }
            }
        }

        for (k, w) in refs.iter().enumerate() {
            if let Some(t) = first_seen[k] {
                first.push(t - w.end);
            }
            if let Some(t) = final_seen[k] {
                fin.push(t - w.end);
                if w.ends_utterance {
                    sentence.push(t - w.end);
                }
            }
        }
    }

    Some(Delays {
        ref_words: reference.len(),
        first_word: DelayStats::from_samples(first),
        final_word: DelayStats::from_samples(fin),
        sentence_final: DelayStats::from_samples(sentence),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobi_stt::Segment;

    fn rw(text: &str, end: f64, ends_utterance: bool) -> RefWord {
        RefWord {
            source: Source::Moi,
            text: text.into(),
            end,
            ends_utterance,
        }
    }

    fn em(text: &str, start: f64, end: f64, is_final: bool, wall: f64) -> Emission {
        Emission {
            segment: Segment {
                start,
                end,
                source: Source::Moi,
                text: text.into(),
                is_final,
            },
            wall,
            audio_pos: wall,
        }
    }

    #[test]
    fn percentiles() {
        let v = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
        assert_eq!(percentile(&v, 50.0), 5.5);
        assert!((percentile(&v, 90.0) - 9.1).abs() < 1e-12);
        let s = DelayStats::from_samples(vec![0.3, 0.1, 0.2]).unwrap();
        assert_eq!((s.count, s.median, s.max), (3, 0.2, 0.3));
        assert!(DelayStats::from_samples(vec![]).is_none());
    }

    #[test]
    fn first_word_final_and_sentence_delays() {
        let reference = [rw("bonjour", 0.5, false), rw("kobi", 1.0, true)];
        let emissions = [
            em("Bonjour", 0.0, 0.5, false, 0.8),
            em("Bonjour Kobi", 0.0, 1.0, false, 1.4),
            em("Bonjour, Kobi.", 0.0, 1.0, true, 2.0),
        ];
        let d = delays(&reference, &emissions, 1.0, Some(Lang::Fr)).unwrap();
        let first = d.first_word.unwrap();
        assert_eq!(first.count, 2);
        assert!((first.mean - 0.35).abs() < 1e-9); // (0.3 + 0.4) / 2
        let fin = d.final_word.unwrap();
        assert!((fin.mean - 1.25).abs() < 1e-9); // (1.5 + 1.0) / 2
        let sentence = d.sentence_final.unwrap();
        assert_eq!(sentence.count, 1);
        assert!((sentence.mean - 1.0).abs() < 1e-9);
    }

    #[test]
    fn wrong_words_are_not_counted_and_speed_scales() {
        let reference = [rw("ticket", 1.0, true)];
        let mut emissions = [
            em("tiquet", 0.0, 1.0, false, 0.3),
            em("ticket", 0.0, 1.0, true, 0.4),
        ];
        for e in &mut emissions {
            e.audio_pos = e.wall * 4.0;
        }
        // Relecture à 4× : 0,4 s réelles = 1,6 s d'audio.
        let d = delays(&reference, &emissions, 4.0, None).unwrap();
        let first = d.first_word.unwrap();
        assert!((first.mean - 0.6).abs() < 1e-9);
        assert!(delays(&reference, &emissions, 0.0, None).is_none());
    }
}
