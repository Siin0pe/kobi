//! Moteur factice : produit des segments réalistes sans modèle.
//!
//! Deux modes :
//! - [`FakeEngine::ticking`] invente un mot `mot` à intervalle fixe ;
//! - [`FakeEngine::scripted`] restitue un script de mots horodatés (par
//!   exemple la référence d'un corpus), avec un retard configurable.
//!
//! Dans les deux cas, chaque nouveau mot émet un segment partiel qui reprend
//! l'énoncé en cours ; l'énoncé devient final après une pause ou quand il
//! dépasse sa longueur maximale.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{AudioFrame, Result, Segment, Source, SttEngine};

/// Mot horodaté dans l'audio.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimedWord {
    pub source: Source,
    pub start: f64,
    pub end: f64,
    pub text: String,
}

#[derive(Debug, Clone, Copy)]
enum Mode {
    /// Un mot toutes les `every` secondes.
    Ticking {
        every: f64,
    },
    Scripted,
}

#[derive(Debug, Default)]
struct Track {
    /// Fin de l'audio reçu, en secondes.
    pos: f64,
    samples: usize,
    /// Mots à venir (ordre chronologique), consommés depuis l'avant.
    pending: std::collections::VecDeque<TimedWord>,
    /// Mots générés jusqu'ici (mode `Ticking`).
    generated: usize,
    current: Vec<TimedWord>,
}

/// Moteur factice, voir le module.
#[derive(Debug)]
pub struct FakeEngine {
    mode: Mode,
    /// Retard entre la fin d'un mot dans l'audio et sa sortie, en secondes
    /// d'audio.
    latency: f64,
    /// Pause qui clôt un énoncé.
    final_gap: f64,
    /// Longueur maximale d'un énoncé.
    max_utterance: f64,
    tracks: HashMap<Source, Track>,
}

impl FakeEngine {
    /// Un mot `mot` toutes les `word_every` secondes, énoncé final toutes les
    /// `final_every` secondes.
    pub fn ticking(word_every: f64, final_every: f64) -> Self {
        assert!(word_every > 0.0 && final_every > 0.0);
        Self {
            mode: Mode::Ticking { every: word_every },
            latency: 0.0,
            final_gap: f64::INFINITY,
            max_utterance: final_every,
            tracks: HashMap::new(),
        }
    }

    /// Restitue `words`, chaque mot sortant `latency` secondes après sa fin
    /// dans l'audio. Une pause de 0,5 s clôt un énoncé, comme une VAD.
    pub fn scripted(words: Vec<TimedWord>, latency: f64) -> Self {
        let mut engine = Self {
            mode: Mode::Scripted,
            latency,
            final_gap: 0.5,
            max_utterance: 30.0,
            tracks: HashMap::new(),
        };
        let mut words = words;
        words.sort_by(|a, b| a.start.total_cmp(&b.start));
        for w in words {
            engine
                .tracks
                .entry(w.source)
                .or_default()
                .pending
                .push_back(w);
        }
        engine
    }

    /// Nombre d'échantillons reçus pour `source`.
    pub fn samples_received(&self, source: Source) -> usize {
        self.tracks.get(&source).map_or(0, |t| t.samples)
    }

    fn finalize(track: &mut Track, source: Source, out: &mut Vec<Segment>) {
        if let Some(seg) = utterance(&track.current, source, true) {
            out.push(seg);
        }
        track.current.clear();
    }
}

fn utterance(words: &[TimedWord], source: Source, is_final: bool) -> Option<Segment> {
    let first = words.first()?;
    let last = words.last()?;
    Some(Segment {
        start: first.start,
        end: last.end,
        source,
        text: words
            .iter()
            .map(|w| w.text.as_str())
            .collect::<Vec<_>>()
            .join(" "),
        is_final,
    })
}

impl SttEngine for FakeEngine {
    fn name(&self) -> &str {
        "fake"
    }

    fn push(&mut self, frame: &AudioFrame) -> Result<Vec<Segment>> {
        let (latency, final_gap, max_utterance, mode) =
            (self.latency, self.final_gap, self.max_utterance, self.mode);
        let source = frame.source;
        let track = self.tracks.entry(source).or_default();
        track.pos = track.pos.max(frame.end());
        track.samples += frame.samples.len();

        if let Mode::Ticking { every } = mode {
            while (track.generated + 1) as f64 * every <= track.pos + 1e-9 {
                let n = track.generated as f64;
                track.pending.push_back(TimedWord {
                    source,
                    start: n * every,
                    end: (n + 1.0) * every,
                    text: "mot".into(),
                });
                track.generated += 1;
            }
        }

        let mut out = Vec::new();
        let mut changed = false;
        while let Some(w) = track.pending.front() {
            if w.end + latency > track.pos + 1e-9 {
                break;
            }
            let w = track.pending.pop_front().expect("mot présent");
            if let (Some(first), Some(last)) = (track.current.first(), track.current.last()) {
                if w.start - last.end >= final_gap || w.end - first.start > max_utterance + 1e-9 {
                    Self::finalize(track, source, &mut out);
                }
            }
            track.current.push(w);
            changed = true;
        }

        if let Some(last) = track.current.last() {
            let ends_by_gap = track.pos >= last.end + latency + final_gap;
            let first = &track.current[0];
            let ends_by_length = last.end - first.start >= max_utterance - 1e-9;
            if ends_by_gap || ends_by_length {
                Self::finalize(track, source, &mut out);
                changed = false;
            }
        }
        if changed {
            out.extend(utterance(&track.current, source, false));
        }
        Ok(out)
    }

    fn finish(&mut self) -> Result<Vec<Segment>> {
        let mut out = Vec::new();
        let mut sources: Vec<Source> = self.tracks.keys().copied().collect();
        sources.sort_by_key(|s| *s as u8);
        for source in sources {
            let track = self.tracks.get_mut(&source).expect("source connue");
            let rest: Vec<TimedWord> = track.pending.drain(..).collect();
            track.current.extend(rest);
            Self::finalize(track, source, &mut out);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replay::{Track as ReplayTrack, WavReplayer};
    use crate::wav::{read_wav_16k_mono, write_wav_16k_mono};
    use crate::SAMPLE_RATE;

    fn word(start: f64, end: f64, text: &str) -> TimedWord {
        TimedWord {
            source: Source::Moi,
            start,
            end,
            text: text.into(),
        }
    }

    fn frame(t: f64, secs: f64) -> AudioFrame {
        AudioFrame {
            source: Source::Moi,
            samples: vec![0.0; (secs * SAMPLE_RATE as f64) as usize],
            t,
        }
    }

    #[test]
    fn scripted_emits_partials_then_final_after_a_pause() {
        let mut e = FakeEngine::scripted(
            vec![
                word(0.0, 0.4, "bonjour"),
                word(0.5, 0.9, "kobi"),
                word(2.0, 2.4, "merci"),
            ],
            0.2,
        );
        let a = e.push(&frame(0.0, 0.6)).unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].text, "bonjour");
        assert!(!a[0].is_final);

        let b = e.push(&frame(0.6, 0.6)).unwrap();
        assert_eq!(b.last().unwrap().text, "bonjour kobi");

        // 0.9 + 0.2 + 0.5 = 1.6 : l'énoncé devient final.
        let c = e.push(&frame(1.2, 0.6)).unwrap();
        assert_eq!(c.len(), 1);
        assert!(c[0].is_final);
        assert_eq!((c[0].start, c[0].end), (0.0, 0.9));

        let d = e.finish().unwrap();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].text, "merci");
        assert!(d[0].is_final);
    }

    /// Critère de validation de KOB-15 : un WAV de 10 s est reçu en
    /// 10 s ± 2 % et produit des segments horodatés.
    #[test]
    fn ten_second_wav_is_replayed_in_real_time() {
        let dir = std::env::temp_dir().join(format!("kobi-stt-replay-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("dix-secondes.wav");
        let tone: Vec<f32> = (0..10 * SAMPLE_RATE as usize)
            .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / SAMPLE_RATE as f32).sin() * 0.3)
            .collect();
        write_wav_16k_mono(&path, &tone).unwrap();
        let samples = read_wav_16k_mono(&path).unwrap();
        std::fs::remove_dir_all(&dir).ok();

        let mut engine = FakeEngine::ticking(0.5, 2.0);
        let out = WavReplayer::new(vec![ReplayTrack {
            source: Source::Moi,
            samples,
        }])
        .run(&mut engine)
        .unwrap();

        assert_eq!(engine.samples_received(Source::Moi), 160_000);
        let err = (out.wall_duration - 10.0).abs() / 10.0;
        assert!(err <= 0.02, "relecture en {:.3} s", out.wall_duration);

        let finals: Vec<_> = out.finals().collect();
        assert_eq!(finals.len(), 5);
        for (i, s) in finals.iter().enumerate() {
            assert_eq!(s.start, i as f64 * 2.0);
            assert_eq!(s.end, (i + 1) as f64 * 2.0);
            assert_eq!(s.text, "mot mot mot mot");
        }
        assert!(out.emissions.iter().any(|e| !e.segment.is_final));
        for e in &out.emissions {
            // Un segment ne sort jamais avant que son audio ait été entendu.
            assert!(e.wall + 0.05 >= e.segment.end, "{e:?}");
        }
        println!(
            "relecture de {:.1} s d'audio en {:.3} s ({:+.2} %), {} segments dont {} finaux",
            out.audio_duration,
            out.wall_duration,
            err * 100.0 * (out.wall_duration - 10.0).signum(),
            out.emissions.len(),
            finals.len()
        );
    }
}
