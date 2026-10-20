//! Capture cpal du micro et de l'audio système.
//!
//! Chaque flux reçoit une origine prise sur sa propre horloge (`StreamTrait::now`)
//! juste après le démarrage de tous les flux. Les blocs sont horodatés avec l'instant
//! de capture fourni par le pilote : sous Windows, la position QPC du premier
//! échantillon, commune à tous les périphériques.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, InputCallbackInfo, SampleFormat, SizedSample, StreamInstant};

use crate::{AudioFrame, Converter, Source};

/// Sources à capturer.
#[derive(Debug, Clone)]
pub struct CaptureOptions {
    pub sources: Vec<Source>,
    /// Nom (ou morceau de nom) du micro ; micro par défaut sinon.
    pub mic: Option<String>,
    /// Nom (ou morceau de nom) de la sortie capturée en loopback ; sortie par défaut sinon.
    pub output: Option<String>,
}

impl Default for CaptureOptions {
    fn default() -> Self {
        Self {
            sources: vec![Source::Moi, Source::Eux],
            mic: None,
            output: None,
        }
    }
}

/// Erreur signalée par un flux en cours.
#[derive(Debug, Clone)]
pub struct StreamError {
    pub source: Source,
    /// Discontinuité du flux (paquets perdus) : sans gravité pour les pistes, que
    /// l'horodatage recale ; le pilote WASAPI en signale souvent au démarrage.
    pub xrun: bool,
    pub message: String,
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
    /// Audio système sous Linux : monitor PulseAudio ou PipeWire.
    #[cfg(target_os = "linux")]
    monitor: Option<crate::pulse::MonitorCapture>,
    infos: Vec<SourceInfo>,
    frames: Receiver<AudioFrame>,
    errors: Receiver<StreamError>,
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
                Source::Moi => select(&host, Direction::Input, options.mic.as_deref())?,
                // Sous Linux, l'audio système passe par le monitor (plus bas).
                #[cfg(target_os = "linux")]
                Source::Eux => continue,
                #[cfg(not(target_os = "linux"))]
                Source::Eux => loopback(&host, options.output.as_deref())?,
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
        // Le monitor prend comme origine le même instant, sur l'horloge du système.
        #[cfg(target_os = "linux")]
        let monitor = if options.sources.contains(&Source::Eux) {
            let monitor = crate::pulse::MonitorCapture::spawn(
                options.output.as_deref(),
                frame_tx.clone(),
                std::time::Instant::now(),
            )
            .context("audio système (monitor PulseAudio ou PipeWire)")?;
            infos.push(SourceInfo {
                source: Source::Eux,
                device: monitor.device().to_string(),
                sample_rate: crate::SAMPLE_RATE,
                channels: 1,
                sample_format: SampleFormat::F32,
            });
            Some(monitor)
        } else {
            None
        };

        Ok(Self {
            running,
            #[cfg(target_os = "linux")]
            monitor,
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
    pub fn errors(&self) -> &Receiver<StreamError> {
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
            .or_else(|| self.monitor_elapsed())
            .unwrap_or_default()
    }

    #[cfg(target_os = "linux")]
    fn monitor_elapsed(&self) -> Option<Duration> {
        use crate::AudioInput;
        self.monitor.as_ref().map(|m| m.elapsed())
    }

    #[cfg(not(target_os = "linux"))]
    fn monitor_elapsed(&self) -> Option<Duration> {
        None
    }

    /// Arrête les flux et renvoie les blocs restants ainsi que la durée finale.
    pub fn stop(self) -> (Vec<AudioFrame>, Duration) {
        let elapsed = self.elapsed();
        #[cfg(target_os = "linux")]
        drop(self.monitor);
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

/// Choisit le périphérique demandé par nom, ou celui par défaut.
fn select(
    host: &cpal::Host,
    direction: Direction,
    name: Option<&str>,
) -> Result<(cpal::Device, cpal::SupportedStreamConfig)> {
    let label = direction.label();
    let device = match name {
        None => direction
            .default_device(host)
            .ok_or_else(|| anyhow!("aucun {label} par défaut"))?,
        Some(name) => {
            let mut devices: Vec<cpal::Device> = direction
                .devices(host)
                .with_context(|| format!("liste des {label}s"))?;
            let names: Vec<String> = devices.iter().map(device_name).collect();
            let index = pick(&names, name).map_err(|e| anyhow!("{label} : {e}"))?;
            devices.swap_remove(index)
        }
    };
    let config = direction
        .default_config(&device)
        .with_context(|| format!("format par défaut du {label} « {} »", device_name(&device)))?;
    Ok((device, config))
}

/// Indice du nom qui correspond à `wanted` : égalité exacte (sans casse) d'abord,
/// sinon un unique nom qui le contient.
fn pick(names: &[String], wanted: &str) -> Result<usize, String> {
    let wanted_lc = wanted.to_lowercase();
    if let Some(i) = names.iter().position(|n| n.to_lowercase() == wanted_lc) {
        return Ok(i);
    }
    let matches: Vec<usize> = (0..names.len())
        .filter(|&i| names[i].to_lowercase().contains(&wanted_lc))
        .collect();
    match matches.as_slice() {
        [i] => Ok(*i),
        [] => Err(format!(
            "aucun périphérique ne contient « {wanted} » (voir --list-devices)"
        )),
        _ => Err(format!(
            "« {wanted} » est ambigu : {}",
            matches
                .iter()
                .map(|&i| format!("« {} »", names[i]))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

#[derive(Debug, Clone, Copy)]
enum Direction {
    Input,
    Output,
}

impl Direction {
    fn label(self) -> &'static str {
        match self {
            Direction::Input => "micro",
            Direction::Output => "sortie",
        }
    }

    fn default_device(self, host: &cpal::Host) -> Option<cpal::Device> {
        match self {
            Direction::Input => host.default_input_device(),
            Direction::Output => host.default_output_device(),
        }
    }

    fn devices(self, host: &cpal::Host) -> Result<Vec<cpal::Device>> {
        Ok(match self {
            Direction::Input => host.input_devices()?.collect(),
            Direction::Output => host.output_devices()?.collect(),
        })
    }

    fn default_config(self, device: &cpal::Device) -> Result<cpal::SupportedStreamConfig> {
        Ok(match self {
            Direction::Input => device.default_input_config()?,
            Direction::Output => device.default_output_config()?,
        })
    }
}

/// Un périphérique tel que présenté par `--list-devices`.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub name: String,
    pub is_default: bool,
    /// Format natif (`fréquence`, `canaux`), si le pilote le donne.
    pub format: Option<(u32, u16)>,
}

/// Micros et sorties disponibles (les sorties sont capturables en loopback).
#[derive(Debug, Clone)]
pub struct DeviceList {
    pub inputs: Vec<DeviceInfo>,
    pub outputs: Vec<DeviceInfo>,
}

/// Liste les micros et les sorties de l'hôte audio par défaut.
pub fn list_devices() -> Result<DeviceList> {
    let host = cpal::default_host();
    let describe = |direction: Direction| -> Result<Vec<DeviceInfo>> {
        let default = direction.default_device(&host).map(|d| device_name(&d));
        Ok(direction
            .devices(&host)
            .with_context(|| format!("liste des {}s", direction.label()))?
            .iter()
            .map(|d| {
                let name = device_name(d);
                DeviceInfo {
                    is_default: default.as_deref() == Some(name.as_str()),
                    format: direction
                        .default_config(d)
                        .ok()
                        .map(|c| (c.sample_rate(), c.channels())),
                    name,
                }
            })
            .collect())
    };
    Ok(DeviceList {
        inputs: describe(Direction::Input)?,
        outputs: describe(Direction::Output)?,
    })
}

/// Sortie audio par défaut du système, celle que l'audio système suit quand
/// aucune n'est choisie (KOB-40) : sous Linux, le sink PulseAudio ou
/// PipeWire par défaut ; ailleurs, la sortie par défaut de cpal. Légère :
/// lue à chaque vérification pendant une réunion.
pub fn default_output_name() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        crate::pulse::detect().ok().and_then(|i| i.default_sink)
    }
    #[cfg(not(target_os = "linux"))]
    {
        cpal::default_host()
            .default_output_device()
            .map(|d| device_name(&d))
    }
}

#[cfg(windows)]
fn loopback(
    host: &cpal::Host,
    name: Option<&str>,
) -> Result<(cpal::Device, cpal::SupportedStreamConfig)> {
    // Un flux d'entrée ouvert sur une sortie WASAPI active le mode loopback.
    select(host, Direction::Output, name)
}

#[cfg(not(any(windows, target_os = "linux")))]
fn loopback(
    _host: &cpal::Host,
    _name: Option<&str>,
) -> Result<(cpal::Device, cpal::SupportedStreamConfig)> {
    bail!("la capture de l'audio système n'est disponible que sous Windows et Linux pour l'instant")
}

fn build_stream(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    source: Source,
    origin: Arc<OnceLock<StreamInstant>>,
    frames: Sender<AudioFrame>,
    errors: Sender<StreamError>,
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
    Sender<StreamError>,
);

fn typed<T>((device, config, source, origin, frames, errors): StreamArgs) -> Result<cpal::Stream>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let mut converter = Converter::new(source, config.sample_rate(), config.channels());
    let mut converted = Vec::new();

    let on_data = move |data: &[T], info: &InputCallbackInfo| {
        let Some(origin) = origin.get() else {
            // Blocs arrivés avant l'origine commune : écartés.
            return;
        };
        converted.clear();
        converted.extend(data.iter().map(|&s| s.to_sample::<f32>()));
        let capture = info.timestamp().capture;
        let since_origin = match capture.checked_duration_since(*origin) {
            Some(d) => d.as_secs_f64(),
            None => -origin.duration_since(capture).as_secs_f64(),
        };
        if let Some(frame) = converter.push(&converted, since_origin) {
            let _ = frames.send(frame);
        }
    };
    let on_error = move |e: cpal::Error| {
        let _ = errors.send(StreamError {
            source,
            xrun: e.kind() == cpal::ErrorKind::Xrun,
            message: e.to_string(),
        });
    };

    Ok(device.build_input_stream::<T, _, _>(config.config(), on_data, on_error, None)?)
}

#[cfg(test)]
mod tests {
    use super::pick;

    fn names() -> Vec<String> {
        [
            "Haut-parleurs (High Definition Audio Device)",
            "Casque (Jabra Evolve2 65)",
            "Casque (Jabra Link 380)",
            "Speakers",
            "Speakers (USB Audio)",
        ]
        .map(String::from)
        .to_vec()
    }

    #[test]
    fn egalite_exacte_prioritaire() {
        // « speakers » est contenu dans deux noms, mais l'un lui est égal (casse ignorée).
        assert_eq!(pick(&names(), "SPEAKERS"), Ok(3));
    }

    #[test]
    fn morceau_de_nom_unique() {
        assert_eq!(pick(&names(), "evolve2"), Ok(1));
        assert_eq!(pick(&names(), "haut-parleurs"), Ok(0));
    }

    #[test]
    fn morceau_ambigu_refuse() {
        let err = pick(&names(), "jabra").unwrap_err();
        assert!(err.contains("ambigu"), "{err}");
        assert!(err.contains("Evolve2") && err.contains("Link 380"), "{err}");
    }

    #[test]
    fn nom_inconnu_refuse() {
        let err = pick(&names(), "bluetooth").unwrap_err();
        assert!(err.contains("--list-devices"), "{err}");
    }
}
