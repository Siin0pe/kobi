//! Placement des blocs horodatés sur une piste continue.
//!
//! Le loopback WASAPI ne livre rien tant qu'aucun son ne joue, et les horloges des
//! périphériques dérivent légèrement. On écrit donc chaque bloc à la position que lui
//! donne son horodatage : un trou est comblé par du silence, un chevauchement est
//! coupé. En dessous de la tolérance, on écrit à la suite pour ne pas créer de clics.

use std::time::Duration;

/// Suit la longueur écrite d'une piste et y place les blocs horodatés.
#[derive(Debug, Clone)]
pub struct TrackAligner {
    rate: u32,
    tolerance: u64,
    written: u64,
    /// Silence inséré au total, en échantillons.
    pub padded: u64,
    /// Échantillons écartés au total.
    pub dropped: u64,
    /// Plus grand écart observé entre l'horodatage d'un bloc et la fin de la piste,
    /// en échantillons (avant recalage).
    pub max_gap: u64,
}

/// Ce qu'il faut écrire pour un bloc : `silence` zéros, puis `samples`.
#[derive(Debug, PartialEq)]
pub struct Placement<'a> {
    pub silence: u64,
    pub samples: &'a [f32],
}

impl TrackAligner {
    pub fn new(rate: u32, tolerance: Duration) -> Self {
        Self {
            rate,
            tolerance: to_samples(tolerance, rate),
            written: 0,
            padded: 0,
            dropped: 0,
            max_gap: 0,
        }
    }

    /// Nombre d'échantillons déjà écrits sur la piste.
    pub fn written(&self) -> u64 {
        self.written
    }

    /// Durée de la piste.
    pub fn duration(&self) -> Duration {
        Duration::from_secs_f64(self.written as f64 / self.rate as f64)
    }

    /// Place un bloc qui commence à l'instant `t` de la piste.
    pub fn place<'a>(&mut self, t: Duration, samples: &'a [f32]) -> Placement<'a> {
        let target = to_samples(t, self.rate);
        self.max_gap = self.max_gap.max(target.abs_diff(self.written));
        let mut placement = Placement {
            silence: 0,
            samples,
        };
        if target > self.written + self.tolerance {
            placement.silence = target - self.written;
            self.padded += placement.silence;
        } else if target + self.tolerance < self.written {
            let skip = ((self.written - target) as usize).min(samples.len());
            placement.samples = &samples[skip..];
            self.dropped += skip as u64;
        }
        self.written += placement.silence + placement.samples.len() as u64;
        placement
    }

    /// Silence à ajouter pour que la piste dure au moins `t`.
    pub fn pad_to(&mut self, t: Duration) -> u64 {
        let target = to_samples(t, self.rate);
        let silence = target.saturating_sub(self.written);
        self.written += silence;
        self.padded += silence;
        silence
    }
}

fn to_samples(t: Duration, rate: u32) -> u64 {
    (t.as_secs_f64() * rate as f64).round() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(v: u64) -> Duration {
        Duration::from_millis(v)
    }

    #[test]
    fn blocs_contigus_ecrits_a_la_suite() {
        let mut a = TrackAligner::new(16_000, ms(10));
        let bloc = [0.5; 160];
        assert_eq!(a.place(ms(0), &bloc).silence, 0);
        // Léger retard d'horodatage (gigue) : pas de silence inséré.
        assert_eq!(a.place(ms(12), &bloc).silence, 0);
        assert_eq!(a.written(), 320);
        assert_eq!(a.padded, 0);
        assert_eq!(a.max_gap, 32);
    }

    #[test]
    fn trou_comble_par_du_silence() {
        let mut a = TrackAligner::new(16_000, ms(10));
        a.place(ms(0), &[0.5; 160]);
        let p = a.place(ms(1_000), &[0.5; 160]);
        assert_eq!(p.silence, 16_000 - 160);
        assert_eq!(p.samples.len(), 160);
        assert_eq!(a.written(), 16_160);
    }

    #[test]
    fn premier_bloc_tardif() {
        let mut a = TrackAligner::new(16_000, ms(10));
        let p = a.place(ms(500), &[0.5; 10]);
        assert_eq!(p.silence, 8_000);
    }

    #[test]
    fn chevauchement_coupe() {
        let mut a = TrackAligner::new(16_000, ms(10));
        a.place(ms(0), &[0.5; 1_600]);
        // Bloc annoncé 50 ms trop tôt : les 800 premiers échantillons sont déjà écrits.
        let bloc = [0.25; 1_000];
        let p = a.place(ms(50), &bloc);
        assert_eq!(p.silence, 0);
        assert_eq!(p.samples.len(), 200);
        assert_eq!(a.dropped, 800);
        assert_eq!(a.written(), 1_800);
    }

    #[test]
    fn fin_alignee() {
        let mut a = TrackAligner::new(16_000, ms(10));
        a.place(ms(0), &[0.5; 160]);
        assert_eq!(a.pad_to(ms(2_000)), 32_000 - 160);
        assert_eq!(a.pad_to(ms(1_000)), 0);
        assert_eq!(a.duration(), ms(2_000));
    }
}
