//! Audio système sous Linux (KOB-106) : un son joué sur la sortie par défaut
//! doit revenir par son monitor.
//!
//! Il faut un serveur PulseAudio ou PipeWire, et le test joue un bip sur la
//! sortie par défaut : il est donc ignoré par défaut. Pour le lancer sans
//! carte son, avec une sortie nulle :
//!
//! ```text
//! pulseaudio -n --daemonize=yes --exit-idle-time=-1 \
//!     --load=module-native-protocol-unix --load="module-null-sink sink_name=kobi_nul"
//! pactl set-default-sink kobi_nul
//! cargo test -p kobi-audio --test pulse_monitor -- --ignored
//! ```
#![cfg(target_os = "linux")]

use std::time::{Duration, Instant};

use kobi_audio::pulse::{detect, MonitorCapture};
use kobi_audio::{AudioFrame, AudioInput, Capture, CaptureOptions, Source};
use libpulse_binding::sample::{Format, Spec};
use libpulse_binding::stream::Direction;
use libpulse_simple_binding::Simple;

const FREQ: f64 = 440.0;

/// Joue un sinus de `FREQ` Hz pendant `secs` sur la sortie par défaut.
fn play_tone(secs: f64) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let spec = Spec {
            format: Format::F32le,
            channels: 1,
            rate: 48_000,
        };
        let out = Simple::new(
            None,
            "Kobi test",
            Direction::Playback,
            None,
            "bip",
            &spec,
            None,
            None,
        )
        .expect("lecture sur la sortie par défaut");
        let n = (48_000.0 * secs) as usize;
        let bytes: Vec<u8> = (0..n)
            .map(|i| (0.5 * (2.0 * std::f64::consts::PI * FREQ * i as f64 / 48_000.0).sin()) as f32)
            .flat_map(f32::to_le_bytes)
            .collect();
        out.write(&bytes).unwrap();
        out.drain().unwrap();
    })
}

/// Niveau et fréquence (passages par zéro) des échantillons.
fn analyse(samples: &[f32]) -> (f32, f64) {
    let rms = (samples.iter().map(|x| x * x).sum::<f32>() / samples.len() as f32).sqrt();
    let crossings = samples
        .windows(2)
        .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
        .count();
    let freq = crossings as f64 / 2.0 / (samples.len() as f64 / 16_000.0);
    (rms, freq)
}

/// Attend que le son arrive (3 s au plus), puis rend `n` échantillons.
fn collect_tone(frames: &std::sync::mpsc::Receiver<AudioFrame>, n: usize) -> Vec<f32> {
    let end = Instant::now() + Duration::from_secs(3);
    let mut out = Vec::new();
    while out.len() < n {
        let Some(left) = end.checked_duration_since(Instant::now()) else {
            break;
        };
        let Ok(f) = frames.recv_timeout(left) else {
            continue;
        };
        assert_eq!(f.source, Source::Eux);
        let (rms, _) = analyse(&f.samples);
        // Avant le son : silence du monitor, ignoré.
        if out.is_empty() && rms < 0.05 {
            continue;
        }
        out.extend(f.samples);
    }
    out.truncate(n);
    out
}

#[test]
#[ignore = "serveur de son requis, joue un bip sur la sortie par défaut"]
fn le_monitor_renvoie_le_son_joue() {
    let info = detect().expect("serveur PulseAudio ou PipeWire");
    eprintln!(
        "serveur : {:?} « {} » {} ; sortie par défaut : {:?}",
        info.server, info.name, info.version, info.default_sink
    );
    let monitor = MonitorCapture::start(None).expect("monitor de la sortie par défaut");
    assert_eq!(monitor.sources(), vec![Source::Eux]);
    let tone = play_tone(3.0);
    let samples = collect_tone(monitor.frames(), 16_000);
    tone.join().unwrap();
    assert_eq!(samples.len(), 16_000, "1 s de son attendue");
    let (rms, freq) = analyse(&samples);
    eprintln!(
        "monitor : {} échantillons, RMS {rms:.3}, {freq:.0} Hz",
        samples.len()
    );
    assert!(rms > 0.1, "son absent du monitor (RMS {rms})");
    assert!((freq - FREQ).abs() < 20.0, "fréquence {freq} Hz");
    assert!(monitor.elapsed() >= Duration::from_secs(1));
}

#[test]
#[ignore = "serveur de son requis, joue un bip sur la sortie par défaut"]
fn capture_eux_passe_par_le_monitor() {
    let capture = Capture::start(&CaptureOptions {
        sources: vec![Source::Eux],
        ..Default::default()
    })
    .expect("capture de l'audio système");
    let infos = capture.sources();
    assert_eq!(infos.len(), 1);
    assert!(infos[0].device.contains("monitor") || infos[0].device.contains("MONITOR"));
    let tone = play_tone(3.0);
    let samples = collect_tone(capture.frames(), 8_000);
    tone.join().unwrap();
    assert_eq!(samples.len(), 8_000, "0,5 s de son attendue");
    let (rms, freq) = analyse(&samples);
    eprintln!("capture : RMS {rms:.3}, {freq:.0} Hz");
    assert!(
        rms > 0.1 && (freq - FREQ).abs() < 20.0,
        "RMS {rms}, {freq} Hz"
    );
    let (_, elapsed) = capture.stop();
    assert!(elapsed >= Duration::from_millis(800), "{elapsed:?}");
}
