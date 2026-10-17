//! Conversion d'un flux natif (entrelacé, fréquence quelconque) en blocs
//! 16 kHz mono horodatés. Commune à la capture cpal et aux sources de test.

use std::time::Duration;

use crate::{downmix, AudioFrame, Resampler, Source, SAMPLE_RATE};

/// Convertit les blocs natifs d'une source en [`AudioFrame`].
pub struct Converter {
    source: Source,
    channels: usize,
    in_rate: u32,
    resampler: Resampler,
}

impl Converter {
    pub fn new(source: Source, in_rate: u32, channels: u16) -> Self {
        assert!(channels > 0, "aucun canal");
        Self {
            source,
            channels: channels as usize,
            in_rate,
            resampler: Resampler::new(in_rate, SAMPLE_RATE),
        }
    }

    /// Convertit un bloc entrelacé dont le premier échantillon est à `t`
    /// secondes de l'origine (négatif si capturé avant elle). Les échantillons
    /// antérieurs à l'origine sont écartés ; `None` s'il ne reste rien.
    pub fn push(&mut self, interleaved: &[f32], t: f64) -> Option<AudioFrame> {
        let (mut samples, pos) = self.resampler.process(&downmix(interleaved, self.channels));
        if samples.is_empty() {
            return None;
        }
        // `pos` : position de la première sortie, en échantillons d'entrée
        // depuis le début du bloc (le rééchantillonneur garde une traîne).
        let mut t = t + pos / self.in_rate as f64;
        if t < 0.0 {
            let skip = ((-t * SAMPLE_RATE as f64).ceil() as usize).min(samples.len());
            samples.drain(..skip);
            t += skip as f64 / SAMPLE_RATE as f64;
            if samples.is_empty() {
                return None;
            }
        }
        Some(AudioFrame {
            source: self.source,
            samples,
            t: Duration::from_secs_f64(t.max(0.0)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stereo_sine(freq: f64, rate: u32, secs: f64) -> Vec<f32> {
        let n = (rate as f64 * secs) as usize;
        (0..n)
            .flat_map(|i| {
                let s = (2.0 * std::f64::consts::PI * freq * i as f64 / rate as f64).sin() as f32;
                [0.5 * s, 0.5 * s]
            })
            .collect()
    }

    /// Rejoue un signal par blocs de `block` trames, horodatés depuis 0.
    fn convert(
        conv: &mut Converter,
        data: &[f32],
        channels: usize,
        rate: u32,
        block: usize,
    ) -> Vec<AudioFrame> {
        data.chunks(block * channels)
            .enumerate()
            .filter_map(|(i, chunk)| conv.push(chunk, (i * block) as f64 / rate as f64))
            .collect()
    }

    #[test]
    fn stereo_48k_devient_mono_16k_continu() {
        let data = stereo_sine(440.0, 48_000, 2.0);
        let mut conv = Converter::new(Source::Moi, 48_000, 2);
        let frames = convert(&mut conv, &data, 2, 48_000, 480);
        let total: usize = frames.iter().map(|f| f.samples.len()).sum();
        // 2 s à 16 kHz, moins la traîne du rééchantillonneur (< 1 ms).
        assert!((31_950..=32_000).contains(&total), "{total}");
        // Blocs contigus : chaque bloc commence où finit le précédent.
        for w in frames.windows(2) {
            let end = w[0].t.as_secs_f64() + w[0].samples.len() as f64 / 16_000.0;
            assert!(
                (w[1].t.as_secs_f64() - end).abs() < 1e-6,
                "{end} → {:?}",
                w[1].t
            );
        }
        assert!(frames.iter().all(|f| f.source == Source::Moi));
        // Amplitude conservée (sinus de crête 0,5 → RMS ≈ 0,354).
        let all: Vec<f32> = frames
            .iter()
            .flat_map(|f| f.samples.iter().copied())
            .collect();
        let rms =
            (all[1000..].iter().map(|x| x * x).sum::<f32>() / (all.len() - 1000) as f32).sqrt();
        assert!((rms - 0.354).abs() < 0.01, "{rms}");
    }

    #[test]
    fn echantillons_avant_l_origine_ecartes() {
        let mut conv = Converter::new(Source::Eux, 16_000, 1);
        // Bloc de 0,1 s commencé 0,05 s avant l'origine.
        let f = conv
            .push(&vec![0.1; 1_600], -0.05)
            .expect("une moitié reste");
        assert!(f.t.as_secs_f64() < 1e-3, "{:?}", f.t);
        assert!(
            f.samples.len() <= 800 && f.samples.len() > 700,
            "{}",
            f.samples.len()
        );
        // Bloc entièrement avant l'origine : rien.
        let mut conv = Converter::new(Source::Eux, 16_000, 1);
        assert!(conv.push(&[0.1; 160], -1.0).is_none());
    }

    #[test]
    fn fichier_wav_44k_stereo() {
        // Source fichier : un WAV 44,1 kHz stéréo 16 bits écrit puis relu.
        let path = std::env::temp_dir().join(format!("kobi-audio-{}.wav", std::process::id()));
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for s in stereo_sine(1_000.0, 44_100, 1.5) {
            w.write_sample((s * i16::MAX as f32) as i16).unwrap();
        }
        w.finalize().unwrap();

        let mut reader = hound::WavReader::open(&path).unwrap();
        let spec = reader.spec();
        let data: Vec<f32> = reader
            .samples::<i16>()
            .map(|s| s.unwrap() as f32 / 32_768.0)
            .collect();
        std::fs::remove_file(&path).ok();

        let mut conv = Converter::new(Source::Moi, spec.sample_rate, spec.channels);
        let frames = convert(&mut conv, &data, 2, 44_100, 441);
        let total: usize = frames.iter().map(|f| f.samples.len()).sum();
        assert!((23_950..=24_000).contains(&total), "{total}");
        assert_eq!(frames[0].t, Duration::ZERO);
    }
}
