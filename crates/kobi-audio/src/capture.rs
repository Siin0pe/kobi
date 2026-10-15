//! Capture cpal du micro et de l'audio système.
//!
//! Chaque flux reçoit une origine prise sur sa propre horloge (`StreamTrait::now`)
//! juste après le démarrage de tous les flux. Les blocs sont horodatés avec l'instant
//! de capture fourni par le pilote : sous Windows, la position QPC du premier
//! échantillon, commune à tous les périphériques.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{ErrorKind, FromSample, InputCallbackInfo, SampleFormat, SizedSample, StreamInstant};

use crate::{downmix, AudioFrame, Resampler, Source, SAMPLE_RATE};

/// Sources à capturer.
#[derive(Debug, Clone)]
pub struct CaptureOptions {
    pub sources: Vec<Source>,
    /// Morceau du nom du micro à utiliser ; micro par défaut sinon.
    pub micro: Option<String>,
}

impl Default for CaptureOptions {
    fn default() -> Self {
        Self {
            sources: vec![Source::Moi, Source::Eux],
            micro: None,
        }
    }
}

/// Périphérique et format natif d'une source.
#[derive(Debug, Clone)]
pub struct SourceInfo {
    pub source: Source,
    pub device: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_format: SampleFormat,
}

struct Running {
    stream: cpal::Stream,
    origin: Arc<OnceLock<StreamInstant>>,
}

/// Capture en cours. L'arrêt se fait en la détruisant (ou par [`Capture::stop`]).
pub struct Capture {
    running: Vec<Running>,
    infos: Vec<SourceInfo>,
    frames: Receiver<AudioFrame>,
    errors: Receiver<(Source, String)>,
}

impl Capture {
    pub fn start(options: &CaptureOptions) -> Result<Self> {
        let host = cpal::default_host();
        let (frame_tx, frames) = mpsc::channel();
        let (error_tx, errors) = mpsc::channel();
        let mut running = Vec::new();
        let mut infos = Vec::new();

        for &source in &options.sources {
            let (device, config) = match source {
                Source::Moi => micro(&host, options.micro.as_deref())?,
                Source::Eux => loopback(&host)?,
            };
            let info = SourceInfo {
                source,
                device: device_name(&device),
                sample_rate: config.sample_rate(),
                channels: config.channels(),
                sample_format: config.sample_format(),
            };
            let origin = Arc::new(OnceLock::new());
            let stream = build_stream(
                &device,
                &config,
                source,
                origin.clone(),
                frame_tx.clone(),
                error_tx.clone(),
            )
            .with_context(|| format!("ouverture de « {} » ({})", info.device, source.label()))?;
            running.push(Running { stream, origin });
            infos.push(info);
        }

        for r in &running {
            r.stream.play().context("démarrage d'un flux audio")?;
        }
        // Origine commune : relevée sur chaque horloge au même moment.
        for r in &running {
            let _ = r.origin.set(r.stream.now());
        }

        Ok(Self {
            running,
            infos,
            frames,
            errors,
        })
    }

    pub fn sources(&self) -> &[SourceInfo] {
        &self.infos
    }

    /// Blocs 16 kHz mono horodatés depuis l'origine commune.
    pub fn frames(&self) -> &Receiver<AudioFrame> {
        &self.frames
    }

    /// Erreurs signalées par les flux (périphérique perdu, sous-alimentation…).
    pub fn errors(&self) -> &Receiver<(Source, String)> {
        &self.errors
    }

    /// Temps écoulé depuis l'origine, sur l'horloge du premier flux.
    pub fn elapsed(&self) -> Duration {
        self.running
            .first()
            .and_then(|r| {
                r.origin
                    .get()
                    .map(|o| r.stream.now().saturating_duration_since(*o))
            })
            .unwrap_or_default()
    }

    /// Arrête les flux et renvoie les blocs restants ainsi que la durée finale.
    pub fn stop(self) -> (Vec<AudioFrame>, Duration) {
        let elapsed = self.elapsed();
        drop(self.running);
        (self.frames.try_iter().collect(), elapsed)
    }
}

fn device_name(device: &cpal::Device) -> String {
    device
        .description()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|_| "périphérique inconnu".into())
}

fn micro(
    host: &cpal::Host,
    name: Option<&str>,
) -> Result<(cpal::Device, cpal::SupportedStreamConfig)> {
    let device = match name {
        None => host
            .default_input_device()
            .ok_or_else(|| anyhow!("aucun micro par défaut"))?,
        Some(name) => {
            let wanted = name.to_lowercase();
            host.input_devices()
                .context("liste des micros")?
                .find(|d| device_name(d).to_lowercase().contains(&wanted))
                .ok_or_else(|| anyhow!("aucun micro ne contient « {name} »"))?
        }
    };
    let config = device
        .default_input_config()
        .context("format par défaut du micro")?;
    Ok((device, config))
}

#[cfg(windows)]
fn loopback(host: &cpal::Host) -> Result<(cpal::Device, cpal::SupportedStreamConfig)> {
    // Un flux d'entrée ouvert sur une sortie WASAPI active le mode loopback.
    let device = host
        .default_output_device()
        .ok_or_else(|| anyhow!("aucune sortie audio par défaut"))?;
    let config = device
        .default_output_config()
        .context("format par défaut de la sortie audio")?;
    Ok((device, config))
}

#[cfg(not(windows))]
fn loopback(_host: &cpal::Host) -> Result<(cpal::Device, cpal::SupportedStreamConfig)> {
    bail!("la capture de l'audio système n'est disponible que sous Windows pour l'instant")
}

fn build_stream(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    source: Source,
    origin: Arc<OnceLock<StreamInstant>>,
    frames: Sender<AudioFrame>,
    errors: Sender<(Source, String)>,
) -> Result<cpal::Stream> {
    let args = (device, config, source, origin, frames, errors);
    match config.sample_format() {
        SampleFormat::F32 => typed::<f32>(args),
        SampleFormat::F64 => typed::<f64>(args),
        SampleFormat::I16 => typed::<i16>(args),
        SampleFormat::I32 => typed::<i32>(args),
        SampleFormat::U16 => typed::<u16>(args),
        SampleFormat::U8 => typed::<u8>(args),
        SampleFormat::I8 => typed::<i8>(args),
        other => bail!("format d'échantillon non géré : {other:?}"),
    }
}

type StreamArgs<'a> = (
    &'a cpal::Device,
    &'a cpal::SupportedStreamConfig,
    Source,
    Arc<OnceLock<StreamInstant>>,
    Sender<AudioFrame>,
    Sender<(Source, String)>,
);

fn typed<T>((device, config, source, origin, frames, errors): StreamArgs) -> Result<cpal::Stream>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let channels = config.channels() as usize;
    let in_rate = config.sample_rate();
    let mut resampler = Resampler::new(in_rate, SAMPLE_RATE);
    let mut converted = Vec::new();
    let delivering = Arc::new(AtomicBool::new(false));
    let delivering_err = delivering.clone();

    let on_data = move |data: &[T], info: &InputCallbackInfo| {
        let Some(origin) = origin.get() else {
            // Blocs arrivés avant l'origine commune : écartés.
            return;
        };
        converted.clear();
        converted.extend(data.iter().map(|&s| s.to_sample::<f32>()));
        let (mut samples, pos) = resampler.process(&downmix(&converted, channels));
        if samples.is_empty() {
            return;
        }
        let capture = info.timestamp().capture;
        let since_origin = match capture.checked_duration_since(*origin) {
            Some(d) => d.as_secs_f64(),
            None => -origin.duration_since(capture).as_secs_f64(),
        };
        let mut t = since_origin + pos / in_rate as f64;
        if t < 0.0 {
            let skip = ((-t * SAMPLE_RATE as f64).ceil() as usize).min(samples.len());
            samples.drain(..skip);
            t += skip as f64 / SAMPLE_RATE as f64;
            if samples.is_empty() {
                return;
            }
        }
        delivering.store(true, Ordering::Relaxed);
        let _ = frames.send(AudioFrame {
            source,
            samples,
            t: Duration::from_secs_f64(t.max(0.0)),
        });
    };
    let on_error = move |e: cpal::Error| {
        // WASAPI signale une discontinuité sur les premiers paquets, avant tout son utile.
        if e.kind() == ErrorKind::Xrun && !delivering_err.load(Ordering::Relaxed) {
            return;
        }
        let _ = errors.send((source, e.to_string()));
    };

    Ok(device.build_input_stream::<T, _, _>(config.config(), on_data, on_error, None)?)
}
