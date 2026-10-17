//! Source audio « fichier » : des WAV rejoués comme une capture, au rythme
//! réel ou accéléré, pour tester sans micro ni sortie audio.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use crate::{AudioFrame, AudioInput, Converter, Source};

/// Réglages de la relecture.
#[derive(Debug, Clone)]
pub struct FileOptions {
    /// 1 = temps réel, 4 = quatre fois plus vite, 0 = sans attente.
    pub speed: f64,
    /// Durée d'un bloc, comme un callback de pilote.
    pub block: Duration,
}

impl Default for FileOptions {
    fn default() -> Self {
        Self {
            speed: 1.0,
            block: Duration::from_millis(20),
        }
    }
}

/// WAV décodé : échantillons entrelacés dans [-1, 1].
struct Track {
    source: Source,
    rate: u32,
    channels: u16,
    data: Vec<f32>,
}

/// Relecture de fichiers WAV (un par source), à l'image de
/// [`Capture`](crate::Capture) : blocs 16 kHz mono horodatés depuis le
/// début des fichiers. Le canal de blocs se ferme à la fin du plus long.
pub struct FileInput {
    sources: Vec<Source>,
    frames: Receiver<AudioFrame>,
    /// Audio déjà poussé, en microsecondes.
    position_us: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FileInput {
    pub fn start(files: &[(Source, PathBuf)], options: &FileOptions) -> Result<Self> {
        if files.is_empty() {
            bail!("aucun fichier à rejouer");
        }
        if !(options.speed >= 0.0 && options.speed.is_finite()) || options.block.is_zero() {
            bail!("vitesse ou taille de bloc invalide");
        }
        let tracks = files
            .iter()
            .map(|(source, path)| read_track(*source, path))
            .collect::<Result<Vec<_>>>()?;
        let sources = tracks.iter().map(|t| t.source).collect();
        let (tx, frames) = mpsc::channel();
        let position_us = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (pos, halt, options) = (position_us.clone(), stop.clone(), options.clone());
        let thread = std::thread::Builder::new()
            .name("kobi-audio-fichier".into())
            .spawn(move || {
                let mut converters: Vec<Converter> = tracks
                    .iter()
                    .map(|t| Converter::new(t.source, t.rate, t.channels))
                    .collect();
                // Positions en entiers (µs, échantillons) : pas de dérive ni de
                // bloc parasite en fin de fichier.
                let block_us = options.block.as_micros() as u64;
                let frames_of = |t: &Track| (t.data.len() / t.channels as usize) as u64;
                let longest_us = tracks
                    .iter()
                    .map(|t| frames_of(t) * 1_000_000 / t.rate as u64)
                    .max()
                    .unwrap_or(0);
                let started = Instant::now();
                let mut k = 0u64;
                while k * block_us < longest_us && !halt.load(Ordering::Relaxed) {
                    let (t0, t1) = (k * block_us, (k + 1) * block_us);
                    if options.speed > 0.0 {
                        let deadline =
                            started + Duration::from_secs_f64(t1 as f64 / 1e6 / options.speed);
                        if let Some(wait) = deadline.checked_duration_since(Instant::now()) {
                            std::thread::sleep(wait);
                        }
                    }
                    for (track, conv) in tracks.iter().zip(&mut converters) {
                        let ch = track.channels as usize;
                        let at = |us: u64| {
                            ((us * track.rate as u64 / 1_000_000).min(frames_of(track)) as usize)
                                * ch
                        };
                        let (a, b) = (at(t0), at(t1));
                        if a < b {
                            if let Some(frame) = conv.push(&track.data[a..b], t0 as f64 / 1e6) {
                                if tx.send(frame).is_err() {
                                    return;
                                }
                            }
                        }
                    }
                    pos.store(t1.min(longest_us), Ordering::Relaxed);
                    k += 1;
                }
            })
            .context("thread de relecture")?;
        Ok(Self {
            sources,
            frames,
            position_us,
            stop,
            thread: Some(thread),
        })
    }
}

impl AudioInput for FileInput {
    fn sources(&self) -> Vec<Source> {
        self.sources.clone()
    }

    fn frames(&self) -> &Receiver<AudioFrame> {
        &self.frames
    }

    /// Audio déjà rejoué (et non le temps réel écoulé, qui diffère en accéléré).
    fn elapsed(&self) -> Duration {
        Duration::from_micros(self.position_us.load(Ordering::Relaxed))
    }
}

impl Drop for FileInput {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn read_track(source: Source, path: &Path) -> Result<Track> {
    let mut reader =
        hound::WavReader::open(path).with_context(|| format!("lecture de {}", path.display()))?;
    let spec = reader.spec();
    let data: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>()?,
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|s| s as f32 * scale))
                .collect::<Result<_, _>>()?
        }
    };
    if spec.channels == 0 || spec.sample_rate == 0 {
        bail!("{} : format WAV invalide", path.display());
    }
    Ok(Track {
        source,
        rate: spec.sample_rate,
        channels: spec.channels,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_wav(name: &str, rate: u32, channels: u16, secs: f64) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("kobi-fichier-{}-{name}.wav", std::process::id()));
        let spec = hound::WavSpec {
            channels,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        let n = (rate as f64 * secs) as usize;
        for i in 0..n {
            let s = (2.0 * std::f64::consts::PI * 440.0 * i as f64 / rate as f64).sin();
            for _ in 0..channels {
                w.write_sample((s * 10_000.0) as i16).unwrap();
            }
        }
        w.finalize().unwrap();
        path
    }

    fn drain(input: &dyn AudioInput) -> Vec<AudioFrame> {
        input.frames().iter().collect()
    }

    #[test]
    fn deux_pistes_sans_attente() {
        let moi = write_wav("moi", 48_000, 2, 1.0);
        let eux = write_wav("eux", 16_000, 1, 0.5);
        let input = FileInput::start(
            &[(Source::Moi, moi.clone()), (Source::Eux, eux.clone())],
            &FileOptions {
                speed: 0.0,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(input.sources(), vec![Source::Moi, Source::Eux]);
        let frames = drain(&input);
        let len = |s| -> usize {
            frames
                .iter()
                .filter(|f| f.source == s)
                .map(|f| f.samples.len())
                .sum()
        };
        assert!(
            (15_950..=16_000).contains(&len(Source::Moi)),
            "{}",
            len(Source::Moi)
        );
        assert!(
            (7_950..=8_000).contains(&len(Source::Eux)),
            "{}",
            len(Source::Eux)
        );
        // Horodatage croissant par source.
        for s in [Source::Moi, Source::Eux] {
            let ts: Vec<Duration> = frames
                .iter()
                .filter(|f| f.source == s)
                .map(|f| f.t)
                .collect();
            assert!(ts.windows(2).all(|w| w[0] < w[1]));
        }
        assert_eq!(input.elapsed(), Duration::from_secs(1));
        std::fs::remove_file(moi).ok();
        std::fs::remove_file(eux).ok();
    }

    #[test]
    fn rythme_accelere() {
        let path = write_wav("rythme", 16_000, 1, 1.0);
        let started = Instant::now();
        let input = FileInput::start(
            &[(Source::Moi, path.clone())],
            &FileOptions {
                speed: 4.0,
                ..Default::default()
            },
        )
        .unwrap();
        let frames = drain(&input);
        let wall = started.elapsed();
        // 1 s d'audio à ×4 : environ 0,25 s.
        assert!(wall >= Duration::from_millis(240), "{wall:?}");
        assert!(wall < Duration::from_millis(900), "{wall:?}");
        assert_eq!(frames.len(), 50, "blocs de 20 ms");
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn arret_avant_la_fin() {
        let path = write_wav("arret", 16_000, 1, 10.0);
        let started = Instant::now();
        let input =
            FileInput::start(&[(Source::Moi, path.clone())], &FileOptions::default()).unwrap();
        let first = input.frames().recv().unwrap();
        assert_eq!(first.t, Duration::ZERO);
        drop(input);
        assert!(started.elapsed() < Duration::from_secs(2), "arrêt rapide");
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn erreurs() {
        assert!(FileInput::start(&[], &FileOptions::default()).is_err());
        let absent = (Source::Moi, PathBuf::from("/nulle/part.wav"));
        assert!(FileInput::start(&[absent], &FileOptions::default()).is_err());
        let path = write_wav("vitesse", 16_000, 1, 0.1);
        let bad = FileOptions {
            speed: -1.0,
            ..Default::default()
        };
        assert!(FileInput::start(&[(Source::Moi, path.clone())], &bad).is_err());
        std::fs::remove_file(path).ok();
    }
}
