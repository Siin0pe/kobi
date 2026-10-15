//! Relecteur de WAV : pousse des pistes vers un moteur au rythme réel.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::{AudioFrame, Result, Segment, Source, SttEngine, SAMPLE_RATE};

/// Une piste à rejouer.
#[derive(Debug, Clone)]
pub struct Track {
    pub source: Source,
    /// Échantillons 16 kHz mono.
    pub samples: Vec<f32>,
}

impl Track {
    pub fn duration(&self) -> f64 {
        self.samples.len() as f64 / SAMPLE_RATE as f64
    }
}

/// Segment produit par le moteur, avec l'instant où il est sorti.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Emission {
    pub segment: Segment,
    /// Temps écoulé depuis le début de la relecture, en secondes.
    pub wall: f64,
    /// Position de l'audio déjà poussé au moment de l'émission, en secondes.
    pub audio_pos: f64,
}

/// Résultat d'une relecture complète.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayOutcome {
    pub engine: String,
    /// Vitesse de relecture (1 = temps réel, 0 = aussi vite que possible).
    pub speed: f64,
    /// Durée de la piste la plus longue, en secondes.
    pub audio_duration: f64,
    /// Durée réelle de la relecture, `finish` compris, en secondes.
    pub wall_duration: f64,
    pub emissions: Vec<Emission>,
}

impl ReplayOutcome {
    /// Segments finaux, dans l'ordre d'émission.
    pub fn finals(&self) -> impl Iterator<Item = &Segment> {
        self.emissions
            .iter()
            .map(|e| &e.segment)
            .filter(|s| s.is_final)
    }
}

/// Pousse des pistes vers un moteur, trame par trame.
///
/// Une trame qui couvre l'audio `[t, t + d]` est poussée à l'instant
/// `(t + d) / speed` : elle n'est disponible qu'une fois entièrement
/// « entendue », comme avec un vrai micro. Les échéances sont calculées depuis
/// le début, sans dérive cumulée ; un moteur trop lent prend du retard puis
/// reçoit les trames suivantes sans attente.
#[derive(Debug, Clone)]
pub struct WavReplayer {
    tracks: Vec<Track>,
    frame_ms: u32,
    speed: f64,
}

impl WavReplayer {
    pub fn new(tracks: Vec<Track>) -> Self {
        Self {
            tracks,
            frame_ms: 20,
            speed: 1.0,
        }
    }

    /// Durée d'une trame en millisecondes (20 par défaut).
    pub fn frame_ms(mut self, frame_ms: u32) -> Self {
        assert!(frame_ms > 0, "frame_ms doit être positif");
        self.frame_ms = frame_ms;
        self
    }

    /// Vitesse de relecture : 1 = temps réel, 2 = deux fois plus vite,
    /// 0 = sans attente.
    pub fn speed(mut self, speed: f64) -> Self {
        assert!(speed >= 0.0, "speed doit être positive ou nulle");
        self.speed = speed;
        self
    }

    pub fn audio_duration(&self) -> f64 {
        self.tracks.iter().map(Track::duration).fold(0.0, f64::max)
    }

    /// Rejoue toutes les pistes dans `engine` et collecte les segments.
    pub fn run<E: SttEngine + ?Sized>(&self, engine: &mut E) -> Result<ReplayOutcome> {
        let frame_len = (SAMPLE_RATE * self.frame_ms / 1000) as usize;
        let frame_secs = frame_len as f64 / SAMPLE_RATE as f64;
        let longest = self
            .tracks
            .iter()
            .map(|t| t.samples.len())
            .max()
            .unwrap_or(0);
        let n_frames = longest.div_ceil(frame_len);

        let mut emissions = Vec::new();
        let started = Instant::now();
        let mut audio_pos = 0.0;

        for k in 0..n_frames {
            let t = k as f64 * frame_secs;
            let lo = k * frame_len;

            for track in &self.tracks {
                if lo >= track.samples.len() {
                    continue;
                }
                let hi = (lo + frame_len).min(track.samples.len());
                let frame = AudioFrame {
                    source: track.source,
                    samples: track.samples[lo..hi].to_vec(),
                    t,
                };
                let end = frame.end();
                if self.speed > 0.0 {
                    sleep_until(started, end / self.speed);
                }
                audio_pos = f64::max(audio_pos, end);
                let wall = started.elapsed().as_secs_f64();
                emissions.extend(engine.push(&frame)?.into_iter().map(|segment| Emission {
                    segment,
                    wall,
                    audio_pos,
                }));
            }
        }

        let finals = engine.finish()?;
        let wall = started.elapsed().as_secs_f64();
        emissions.extend(finals.into_iter().map(|segment| Emission {
            segment,
            wall,
            audio_pos,
        }));

        Ok(ReplayOutcome {
            engine: engine.name().to_string(),
            speed: self.speed,
            audio_duration: self.audio_duration(),
            wall_duration: started.elapsed().as_secs_f64(),
            emissions,
        })
    }
}

fn sleep_until(started: Instant, deadline_secs: f64) {
    let deadline = started + Duration::from_secs_f64(deadline_secs);
    let now = Instant::now();
    if deadline > now {
        std::thread::sleep(deadline - now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeEngine;

    fn silence(samples: usize) -> Vec<f32> {
        vec![0.0; samples]
    }

    #[test]
    fn fast_replay_pushes_every_sample() {
        let tracks = vec![
            Track {
                source: Source::Moi,
                samples: silence(48_000),
            },
            Track {
                source: Source::Eux,
                samples: silence(32_160),
            },
        ];
        let mut engine = FakeEngine::ticking(0.5, 2.0);
        let out = WavReplayer::new(tracks)
            .speed(0.0)
            .run(&mut engine)
            .unwrap();
        assert_eq!(out.audio_duration, 3.0);
        assert_eq!(engine.samples_received(Source::Moi), 48_000);
        assert_eq!(engine.samples_received(Source::Eux), 32_160);
        assert!(out.finals().any(|s| s.source == Source::Eux));
    }

    #[test]
    fn accelerated_replay_takes_a_fraction_of_the_time() {
        let tracks = vec![Track {
            source: Source::Moi,
            samples: silence(32_000),
        }];
        let mut engine = FakeEngine::ticking(0.5, 1.0);
        let out = WavReplayer::new(tracks)
            .speed(4.0)
            .run(&mut engine)
            .unwrap();
        assert!(
            (out.wall_duration - 0.5).abs() < 0.05,
            "durée {}",
            out.wall_duration
        );
    }
}
