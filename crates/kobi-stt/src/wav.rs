//! Lecture de WAV en 16 kHz mono `f32`.

use std::path::Path;

use crate::{Result, SttError, SAMPLE_RATE};

/// Lit un WAV PCM (entier ou flottant), mélange les canaux en mono et
/// rééchantillonne en 16 kHz si besoin (interpolation linéaire).
pub fn read_wav_16k_mono(path: impl AsRef<Path>) -> Result<Vec<f32>> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let channels = spec.channels as usize;
    if channels == 0 {
        return Err(SttError::Format("aucun canal".into()));
    }

    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<std::result::Result<_, _>>()?,
        hound::SampleFormat::Int => {
            let scale = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 / scale))
                .collect::<std::result::Result<_, _>>()?
        }
    };

    let mono: Vec<f32> = interleaved
        .chunks_exact(channels)
        .map(|c| c.iter().sum::<f32>() / channels as f32)
        .collect();

    Ok(resample_linear(&mono, spec.sample_rate, SAMPLE_RATE))
}

/// Rééchantillonnage par interpolation linéaire, suffisant pour la parole
/// vers 16 kHz.
pub fn resample_linear(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() {
        return input.to_vec();
    }
    let ratio = from as f64 / to as f64;
    let out_len = (input.len() as f64 / ratio).round() as usize;
    (0..out_len)
        .map(|i| {
            let pos = i as f64 * ratio;
            let idx = pos.floor() as usize;
            let frac = (pos - idx as f64) as f32;
            let a = input[idx.min(input.len() - 1)];
            let b = input[(idx + 1).min(input.len() - 1)];
            a + (b - a) * frac
        })
        .collect()
}

/// Écrit un WAV 16 kHz mono `i16` (utile aux tests et au corpus).
pub fn write_wav_16k_mono(path: impl AsRef<Path>, samples: &[f32]) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    for &s in samples {
        writer.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
    }
    writer.finalize()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_halves_length() {
        let input: Vec<f32> = (0..32_000).map(|i| i as f32).collect();
        let out = resample_linear(&input, 32_000, 16_000);
        assert_eq!(out.len(), 16_000);
        assert_eq!(out[1], 2.0);
    }

    #[test]
    fn stereo_44k_is_downmixed_and_resampled() {
        let dir = std::env::temp_dir().join(format!("kobi-stt-wav-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("stereo.wav");
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for _ in 0..44_100 {
            w.write_sample(i16::MAX / 2).unwrap();
            w.write_sample(0i16).unwrap();
        }
        w.finalize().unwrap();

        let samples = read_wav_16k_mono(&path).unwrap();
        assert_eq!(samples.len(), 16_000);
        assert!((samples[100] - 0.25).abs() < 0.01);
        std::fs::remove_dir_all(&dir).ok();
    }
}
