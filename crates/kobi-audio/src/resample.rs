//! Rééchantillonnage par sinc fenêtré (Blackman), table de phases précalculée.
//!
//! La position de chaque échantillon de sortie est calculée en entiers, sans dérive :
//! la sortie `k` correspond exactement à l'entrée `k × in / out`. Le délai de groupe
//! est donc nul vu de l'appelant, ce qui permet d'horodater la sortie à partir de
//! l'horodatage de l'entrée.

use std::f64::consts::PI;

/// Nombre de phases de la table d'interpolation.
const PHASES: usize = 512;
/// Fréquence de coupure relative à la plus petite des deux fréquences de Nyquist.
const CUTOFF: f64 = 0.9;
/// Nombre de passages par zéro du sinc de chaque côté.
const ZERO_CROSSINGS: f64 = 12.0;

/// Moyenne des canaux d'un signal entrelacé.
pub fn downmix(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    let scale = 1.0 / channels as f32;
    interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() * scale)
        .collect()
}

/// Rééchantillonneur en flux, mono.
pub struct Resampler {
    in_rate: u64,
    out_rate: u64,
    /// Demi-largeur du noyau, en échantillons d'entrée.
    half: usize,
    /// `PHASES + 1` lignes de `2 × half` coefficients.
    table: Vec<f32>,
    /// Échantillons d'entrée en attente ; `buf[0]` a l'indice absolu `buf_start`.
    buf: Vec<f32>,
    buf_start: i64,
    /// Nombre total d'échantillons d'entrée reçus.
    received: i64,
    /// Indice du prochain échantillon de sortie.
    next_out: u64,
}

impl Resampler {
    pub fn new(in_rate: u32, out_rate: u32) -> Self {
        assert!(in_rate > 0 && out_rate > 0, "fréquences nulles");
        let cut = CUTOFF * (out_rate as f64 / in_rate as f64).min(1.0);
        let half = (ZERO_CROSSINGS / cut).ceil() as usize;
        let width = 2 * half;
        let mut table = Vec::with_capacity((PHASES + 1) * width);
        for phase in 0..=PHASES {
            let frac = phase as f64 / PHASES as f64;
            let row: Vec<f64> = (0..width)
                .map(|k| {
                    let d = k as f64 - (half as f64 - 1.0) - frac;
                    let x = d / half as f64;
                    let window = if x.abs() >= 1.0 {
                        0.0
                    } else {
                        0.42 + 0.5 * (PI * x).cos() + 0.08 * (2.0 * PI * x).cos()
                    };
                    cut * sinc(cut * d) * window
                })
                .collect();
            let sum: f64 = row.iter().sum();
            table.extend(row.iter().map(|c| (c / sum) as f32));
        }
        Self {
            in_rate: in_rate as u64,
            out_rate: out_rate as u64,
            half,
            table,
            // Amorçage par des zéros pour que la sortie 0 tombe sur l'entrée 0.
            buf: vec![0.0; half - 1],
            buf_start: -(half as i64 - 1),
            received: 0,
            next_out: 0,
        }
    }

    /// Ajoute un bloc d'entrée et renvoie la sortie disponible, avec la position du
    /// premier échantillon de sortie exprimée en échantillons d'entrée depuis le début
    /// de `input` (négative si cet échantillon appartient à un bloc précédent).
    pub fn process(&mut self, input: &[f32]) -> (Vec<f32>, f64) {
        let chunk_start = self.received;
        self.buf.extend_from_slice(input);
        self.received += input.len() as i64;

        let width = 2 * self.half;
        let first_pos = self.out_position(self.next_out);
        let mut out = Vec::new();
        loop {
            let num = self.next_out * self.in_rate;
            let i0 = (num / self.out_rate) as i64;
            if i0 + self.half as i64 >= self.received {
                break;
            }
            let phase = ((num % self.out_rate) * PHASES as u64 + self.out_rate / 2) / self.out_rate;
            let coeffs = &self.table[phase as usize * width..][..width];
            let first = (i0 - (self.half as i64 - 1) - self.buf_start) as usize;
            let taps = &self.buf[first..first + width];
            out.push(taps.iter().zip(coeffs).map(|(x, c)| x * c).sum());
            self.next_out += 1;
        }

        // On ne garde que ce dont la prochaine sortie a besoin.
        let next_i0 = (self.next_out * self.in_rate / self.out_rate) as i64;
        let keep_from = next_i0 - (self.half as i64 - 1);
        if keep_from > self.buf_start {
            let drop = (keep_from - self.buf_start) as usize;
            self.buf.drain(..drop.min(self.buf.len()));
            self.buf_start = keep_from;
        }

        (out, first_pos - chunk_start as f64)
    }

    fn out_position(&self, k: u64) -> f64 {
        (k * self.in_rate) as f64 / self.out_rate as f64
    }
}

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-12 {
        1.0
    } else {
        (PI * x).sin() / (PI * x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f64, rate: u32, len: usize) -> Vec<f32> {
        (0..len)
            .map(|i| (2.0 * PI * freq * i as f64 / rate as f64).sin() as f32)
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    /// Rééchantillonne par blocs de tailles irrégulières, comme un vrai périphérique.
    fn run(r: &mut Resampler, input: &[f32]) -> Vec<f32> {
        let mut out = Vec::new();
        let mut i = 0;
        let mut n = 7;
        while i < input.len() {
            let end = (i + n).min(input.len());
            out.extend(r.process(&input[i..end]).0);
            i = end;
            n = n * 3 % 1013 + 1;
        }
        out
    }

    #[test]
    fn downmix_moyenne_les_canaux() {
        assert_eq!(downmix(&[1.0, 0.0, 0.5, 0.5], 2), vec![0.5, 0.5]);
        assert_eq!(downmix(&[0.25, 0.75], 1), vec![0.25, 0.75]);
    }

    #[test]
    fn longueur_de_sortie() {
        for in_rate in [48_000, 44_100, 16_000, 8_000] {
            let mut r = Resampler::new(in_rate, 16_000);
            let out = run(&mut r, &vec![0.0; in_rate as usize]);
            let attendu = 16_000.0;
            assert!(
                (out.len() as f64 - attendu).abs() <= 40.0 * 16_000.0 / in_rate as f64,
                "{in_rate} Hz : {} échantillons",
                out.len()
            );
        }
    }

    #[test]
    fn conserve_la_bande_utile() {
        for in_rate in [48_000, 44_100] {
            let mut r = Resampler::new(in_rate, 16_000);
            let out = run(&mut r, &sine(1_000.0, in_rate, in_rate as usize));
            let attendu = sine(1_000.0, 16_000, out.len());
            let utile = 200..out.len() - 200;
            let err: Vec<f32> = out[utile.clone()]
                .iter()
                .zip(&attendu[utile])
                .map(|(a, b)| a - b)
                .collect();
            assert!(rms(&err) < 1e-3, "{in_rate} Hz : erreur {}", rms(&err));
        }
    }

    #[test]
    fn coupe_au_dessus_de_nyquist() {
        let mut r = Resampler::new(48_000, 16_000);
        let out = run(&mut r, &sine(12_000.0, 48_000, 48_000));
        let attenuation_db = 20.0 * (rms(&out[200..]) / (0.5f32).sqrt()).log10();
        assert!(attenuation_db < -60.0, "atténuation {attenuation_db} dB");
    }

    #[test]
    fn position_du_premier_echantillon() {
        let mut r = Resampler::new(48_000, 16_000);
        let (out, pos) = r.process(&[0.0; 1000]);
        assert_eq!(pos, 0.0);
        let n = out.len() as f64;
        let (_, pos) = r.process(&[0.0; 1000]);
        // La sortie suivante est à l'entrée 3n, soit 3n - 1000 depuis le début du bloc.
        assert_eq!(pos, 3.0 * n - 1000.0);
    }

    #[test]
    fn impulsion_au_bon_endroit() {
        let mut input = vec![0.0f32; 48_000];
        input[30_000] = 1.0;
        let mut r = Resampler::new(48_000, 16_000);
        let out = run(&mut r, &input);
        let pic = out
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        assert_eq!(pic, 10_000);
    }
}
