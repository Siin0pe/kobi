//! Audio système sous Linux : la source « monitor » de la sortie par défaut,
//! par l'API PulseAudio. PipeWire la fournit aussi, par `pipewire-pulse`,
//! installé par défaut sur les distributions qui l'utilisent ; le serveur
//! réel est indiqué par [`detect`].

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use libpulse_binding as pulse;
use libpulse_simple_binding::Simple;
use pulse::context::{Context as PaContext, FlagSet, State};
use pulse::mainloop::standard::{IterateResult, Mainloop};
use pulse::sample::{Format, Spec};
use pulse::stream::Direction;

use crate::{AudioFrame, AudioInput, Converter, Source, SAMPLE_RATE};

/// Bloc lu sur le monitor : 20 ms, comme un callback de pilote.
const BLOCK: usize = SAMPLE_RATE as usize / 50;
/// Source PulseAudio qui suit toujours le monitor de la sortie par défaut.
pub const DEFAULT_MONITOR: &str = "@DEFAULT_MONITOR@";

/// Serveur de son joint par l'API PulseAudio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundServer {
    PulseAudio,
    /// PipeWire par sa couche de compatibilité `pipewire-pulse`.
    PipeWire,
}

/// Ce que le serveur annonce.
#[derive(Debug, Clone)]
pub struct ServerInfo {
    pub server: SoundServer,
    /// Nom complet (« pulseaudio », « PulseAudio (on PipeWire 1.0.5) »).
    pub name: String,
    pub version: String,
    /// Sortie par défaut, dont le monitor est capturé.
    pub default_sink: Option<String>,
}

impl SoundServer {
    /// Serveur d'après le nom annoncé.
    pub fn from_name(name: &str) -> Self {
        if name.to_lowercase().contains("pipewire") {
            SoundServer::PipeWire
        } else {
            SoundServer::PulseAudio
        }
    }
}

/// Interroge le serveur de son (PulseAudio ou PipeWire). Échoue s'il n'y en a pas.
pub fn detect() -> Result<ServerInfo> {
    let mut mainloop = Mainloop::new().ok_or_else(|| anyhow!("boucle PulseAudio"))?;
    let mut context =
        PaContext::new(&mainloop, "Kobi").ok_or_else(|| anyhow!("contexte PulseAudio"))?;
    context
        .connect(None, FlagSet::NOAUTOSPAWN, None)
        .map_err(|e| anyhow!("connexion au serveur de son : {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        iterate(&mut mainloop)?;
        match context.get_state() {
            State::Ready => break,
            State::Failed | State::Terminated => {
                bail!("aucun serveur PulseAudio ou PipeWire joignable")
            }
            _ if Instant::now() > deadline => bail!("serveur de son : délai dépassé"),
            _ => {}
        }
    }
    let found = Arc::new(std::sync::Mutex::new(None));
    let slot = found.clone();
    let op = context.introspect().get_server_info(move |i| {
        let name = i.server_name.as_deref().unwrap_or("?").to_string();
        *slot.lock().unwrap() = Some(ServerInfo {
            server: SoundServer::from_name(&name),
            name,
            version: i.server_version.as_deref().unwrap_or("?").to_string(),
            default_sink: i.default_sink_name.as_deref().map(str::to_string),
        });
    });
    while op.get_state() == pulse::operation::State::Running {
        iterate(&mut mainloop)?;
    }
    context.disconnect();
    let info = found.lock().unwrap().take();
    info.ok_or_else(|| anyhow!("informations du serveur de son indisponibles"))
}

fn iterate(mainloop: &mut Mainloop) -> Result<()> {
    match mainloop.iterate(true) {
        IterateResult::Success(_) => Ok(()),
        IterateResult::Quit(_) | IterateResult::Err(_) => bail!("boucle PulseAudio arrêtée"),
    }
}

/// Capture du monitor d'une sortie : blocs 16 kHz mono horodatés depuis
/// `origin`. Le serveur rééchantillonne lui-même vers 16 kHz mono.
pub struct MonitorCapture {
    device: String,
    frames: Option<Receiver<AudioFrame>>,
    /// Audio déjà lu, en microsecondes.
    position_us: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl MonitorCapture {
    /// Capture autonome (son propre canal) du monitor de `sink`, ou de la
    /// sortie par défaut.
    pub fn start(sink: Option<&str>) -> Result<Self> {
        let (tx, rx) = mpsc::channel();
        let mut capture = Self::spawn(sink, tx, Instant::now())?;
        capture.frames = Some(rx);
        Ok(capture)
    }

    /// Capture qui envoie ses blocs dans `frames`, horodatés depuis `origin`
    /// (utilisé par [`Capture`](crate::Capture) avec le micro).
    pub(crate) fn spawn(
        sink: Option<&str>,
        frames: Sender<AudioFrame>,
        origin: Instant,
    ) -> Result<Self> {
        let device = match sink {
            Some(s) => format!("{s}.monitor"),
            None => DEFAULT_MONITOR.to_string(),
        };
        let spec = Spec {
            format: Format::F32le,
            channels: 1,
            rate: SAMPLE_RATE,
        };
        let attr = pulse::def::BufferAttr {
            maxlength: u32::MAX,
            tlength: u32::MAX,
            prebuf: u32::MAX,
            minreq: u32::MAX,
            fragsize: (BLOCK * 4) as u32,
        };
        // Le flux est ouvert dans le thread qui le lit ; l'ouverture est
        // attendue ici pour signaler un monitor absent.
        let (opened_tx, opened) = mpsc::sync_channel::<Result<(), String>>(1);
        let position_us = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (pos, halt, dev) = (position_us.clone(), stop.clone(), device.clone());
        let thread = std::thread::Builder::new()
            .name("kobi-audio-monitor".into())
            .spawn(move || {
                let simple = match Simple::new(
                    None,
                    "Kobi",
                    Direction::Record,
                    Some(&dev),
                    "audio système",
                    &spec,
                    None,
                    Some(&attr),
                ) {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = opened_tx.send(Err(format!("ouverture de « {dev} » : {e}")));
                        return;
                    }
                };
                // Instant du premier échantillon, depuis l'origine commune.
                let t0 = origin.elapsed().as_secs_f64();
                let _ = opened_tx.send(Ok(()));
                let mut converter = Converter::new(Source::Eux, SAMPLE_RATE, 1);
                let mut bytes = vec![0u8; BLOCK * 4];
                let mut read = 0u64;
                while !halt.load(Ordering::Relaxed) {
                    if let Err(e) = simple.read(&mut bytes) {
                        eprintln!("monitor « {dev} » : lecture interrompue ({e})");
                        break;
                    }
                    let samples: Vec<f32> = bytes
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|b| f32::from_le_bytes(*b))
                        .collect();
                    let t = t0 + read as f64 / SAMPLE_RATE as f64;
                    read += BLOCK as u64;
                    pos.store(read * 1_000_000 / SAMPLE_RATE as u64, Ordering::Relaxed);
                    if let Some(frame) = converter.push(&samples, t) {
                        if frames.send(frame).is_err() {
                            break;
                        }
                    }
                }
            })
            .context("thread du monitor")?;
        match opened.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                let _ = thread.join();
                bail!(e)
            }
            Err(_) => bail!("thread du monitor arrêté"),
        }
        Ok(Self {
            device,
            frames: None,
            position_us,
            stop,
            thread: Some(thread),
        })
    }

    /// Source PulseAudio capturée (`…​.monitor` ou `@DEFAULT_MONITOR@`).
    pub fn device(&self) -> &str {
        &self.device
    }
}

impl AudioInput for MonitorCapture {
    fn sources(&self) -> Vec<Source> {
        vec![Source::Eux]
    }

    /// Panique si la capture a été créée pour [`Capture`](crate::Capture),
    /// qui reçoit les blocs à sa place.
    fn frames(&self) -> &Receiver<AudioFrame> {
        self.frames
            .as_ref()
            .expect("capture autonome (MonitorCapture::start)")
    }

    fn elapsed(&self) -> Duration {
        Duration::from_micros(self.position_us.load(Ordering::Relaxed))
    }
}

impl Drop for MonitorCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // `read` rend la main toutes les 20 ms : l'arrêt est rapide.
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconnait_pipewire_par_son_nom() {
        assert_eq!(
            SoundServer::from_name("PulseAudio (on PipeWire 1.0.5)"),
            SoundServer::PipeWire
        );
        assert_eq!(
            SoundServer::from_name("pulseaudio"),
            SoundServer::PulseAudio
        );
    }
}
