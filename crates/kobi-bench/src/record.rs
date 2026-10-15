//! `kobi-bench record` : enregistre le micro dans `moi.wav` et l'audio système dans
//! `eux.wav`, en 16 kHz mono, sur la même origine de temps.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use kobi_audio::{AudioFrame, Capture, CaptureOptions, Source, TrackAligner, SAMPLE_RATE};

/// Écart toléré entre l'horodatage d'un bloc et la fin de la piste avant recalage.
const TOLERANCE: Duration = Duration::from_millis(10);

#[derive(clap::Args, Debug)]
pub struct RecordArgs {
    /// Dossier de sortie [défaut : ~/Kobi-bench/enregistrements/AAAA-MM-JJ_HHhMMmSS]
    #[arg(long)]
    dossier: Option<PathBuf>,
    /// Liste les micros et les sorties disponibles, puis quitte
    #[arg(long)]
    list_devices: bool,
    /// Micro à enregistrer dans moi.wav, nom exact ou morceau unique [défaut : micro par défaut]
    #[arg(long, value_name = "NOM")]
    mic: Option<String>,
    /// Sortie à capturer en loopback dans eux.wav, nom exact ou morceau unique
    /// (le casque de la visio, par exemple) [défaut : sortie par défaut]
    #[arg(long, value_name = "NOM")]
    output: Option<String>,
    /// Arrêt automatique après ce nombre de secondes [défaut : Ctrl+C]
    #[arg(long)]
    duree: Option<f64>,
}

struct Track {
    path: PathBuf,
    aligner: TrackAligner,
    writer: hound::WavWriter<BufWriter<File>>,
    /// Crête depuis le dernier affichage.
    peak: f32,
}

impl Track {
    fn create(dir: &Path, source: Source) -> Result<Self> {
        let path = dir.join(format!("{}.wav", source.label()));
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: SAMPLE_RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let writer = hound::WavWriter::create(&path, spec)
            .with_context(|| format!("création de {}", path.display()))?;
        Ok(Self {
            path,
            aligner: TrackAligner::new(SAMPLE_RATE, TOLERANCE),
            writer,
            peak: 0.0,
        })
    }

    fn write(&mut self, frame: &AudioFrame) -> Result<()> {
        let placement = self.aligner.place(frame.t, &frame.samples);
        self.silence(placement.silence)?;
        for &s in placement.samples {
            self.peak = self.peak.max(s.abs());
            self.writer
                .write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16)?;
        }
        Ok(())
    }

    fn silence(&mut self, n: u64) -> Result<()> {
        for _ in 0..n {
            self.writer.write_sample(0i16)?;
        }
        Ok(())
    }

    fn finish(mut self, end: Duration) -> Result<TrackSummary> {
        let pad = self.aligner.pad_to(end);
        self.silence(pad)?;
        self.writer.finalize()?;
        Ok(TrackSummary {
            path: self.path,
            aligner: self.aligner,
        })
    }
}

struct TrackSummary {
    path: PathBuf,
    aligner: TrackAligner,
}

pub fn run(args: RecordArgs) -> Result<()> {
    if args.list_devices {
        return print_devices();
    }

    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        ctrlc::set_handler(move || stop.store(true, Ordering::SeqCst))
            .context("installation du gestionnaire Ctrl+C")?;
    }

    let options = CaptureOptions {
        mic: args.mic,
        output: args.output,
        ..CaptureOptions::default()
    };
    // Périphériques ouverts avant de créer les fichiers : un nom erroné ne laisse rien sur le disque.
    let capture = Capture::start(&options)?;

    let dir = match args.dossier {
        Some(dir) => dir,
        None => default_dir()?,
    };
    std::fs::create_dir_all(&dir).with_context(|| format!("création de {}", dir.display()))?;
    let mut tracks = HashMap::new();
    for &source in &options.sources {
        tracks.insert(source, Track::create(&dir, source)?);
    }
    for info in capture.sources() {
        println!(
            "{} : {} ({} Hz, {} canaux, {:?})",
            info.source.label(),
            info.device,
            info.sample_rate,
            info.channels,
            info.sample_format
        );
    }
    println!(
        "Enregistrement dans {} — Ctrl+C pour arrêter.",
        dir.display()
    );

    let limit = args.duree.map(Duration::from_secs_f64);
    let mut last_status = Instant::now();
    while !stop.load(Ordering::SeqCst) && limit.is_none_or(|l| capture.elapsed() < l) {
        match capture.frames().recv_timeout(Duration::from_millis(50)) {
            Ok(frame) => write_frame(&mut tracks, &frame)?,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        for (source, error) in capture.errors().try_iter() {
            eprintln!(
                "\n{} à {} : {error}",
                source.label(),
                format_duration(capture.elapsed())
            );
        }
        if last_status.elapsed() >= Duration::from_secs(1) {
            last_status = Instant::now();
            print_status(capture.elapsed(), &mut tracks);
        }
    }

    let (rest, elapsed) = capture.stop();
    for frame in &rest {
        write_frame(&mut tracks, frame)?;
    }
    // Les deux pistes finissent au même instant.
    let end = tracks
        .values()
        .map(|t| t.aligner.duration())
        .fold(elapsed, Duration::max);

    println!("\nDurée : {}", format_duration(end));
    let mut sources: Vec<_> = tracks.into_iter().collect();
    sources.sort_by_key(|(s, _)| s.label());
    for (_, track) in sources {
        let s = track.finish(end)?;
        println!(
            "{} : {} (silence comblé {:.1} ms, écarté {:.1} ms, écart d'horodatage max {:.1} ms)",
            s.path.display(),
            format_duration(s.aligner.duration()),
            millis(s.aligner.padded),
            millis(s.aligner.dropped),
            millis(s.aligner.max_gap),
        );
    }
    Ok(())
}

fn print_devices() -> Result<()> {
    let list = kobi_audio::list_devices()?;
    for (title, devices) in [
        ("Micros (--mic)", &list.inputs),
        ("Sorties capturables en loopback (--output)", &list.outputs),
    ] {
        println!("{title} :");
        if devices.is_empty() {
            println!("  (aucun)");
        }
        for d in devices {
            let format = d
                .format
                .map(|(rate, channels)| format!("{rate} Hz, {channels} canaux"))
                .unwrap_or_else(|| "format inconnu".into());
            let default = if d.is_default { "  [défaut]" } else { "" };
            println!("  {}  ({format}){default}", d.name);
        }
    }
    Ok(())
}

fn write_frame(tracks: &mut HashMap<Source, Track>, frame: &AudioFrame) -> Result<()> {
    match tracks.get_mut(&frame.source) {
        Some(track) => track.write(frame),
        None => Ok(()),
    }
}

fn print_status(elapsed: Duration, tracks: &mut HashMap<Source, Track>) {
    let mut line = format!("\r● {}", format_duration(elapsed));
    for source in [Source::Moi, Source::Eux] {
        if let Some(track) = tracks.get_mut(&source) {
            let level = if track.peak > 0.0 {
                format!("{:>4.0} dB", 20.0 * track.peak.log10())
            } else {
                "   — dB".into()
            };
            line.push_str(&format!("   {} {level}", source.label()));
            track.peak = 0.0;
        }
    }
    print!("{line}   ");
    let _ = std::io::stdout().flush();
}

fn millis(samples: u64) -> f64 {
    samples as f64 * 1000.0 / SAMPLE_RATE as f64
}

fn format_duration(d: Duration) -> String {
    let secs = d.as_secs_f64();
    let minutes = (secs / 60.0).floor();
    format!("{:02}:{:04.1}", minutes as u64, secs - minutes * 60.0)
}

fn default_dir() -> Result<PathBuf> {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .context("dossier personnel introuvable : préciser --dossier")?;
    let name = chrono::Local::now().format("%Y-%m-%d_%Hh%Mm%S").to_string();
    Ok(PathBuf::from(home)
        .join("Kobi-bench")
        .join("enregistrements")
        .join(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duree_lisible() {
        assert_eq!(format_duration(Duration::from_millis(83_400)), "01:23.4");
        assert_eq!(format_duration(Duration::from_millis(5_000)), "00:05.0");
    }
}
