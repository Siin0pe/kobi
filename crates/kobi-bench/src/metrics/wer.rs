//! Taux d'erreur par mot (WER) par alignement de Levenshtein.

use std::ops::AddAssign;

use serde::{Deserialize, Serialize};

/// Comptes d'erreurs entre une référence et une hypothèse.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WerStats {
    pub ref_words: usize,
    pub hyp_words: usize,
    pub substitutions: usize,
    pub deletions: usize,
    pub insertions: usize,
}

impl WerStats {
    pub fn errors(&self) -> usize {
        self.substitutions + self.deletions + self.insertions
    }

    /// WER entre 0 et plus de 1 (les insertions peuvent dépasser la
    /// référence). Une référence vide donne 0 sans hypothèse, 1 sinon.
    pub fn wer(&self) -> f64 {
        if self.ref_words == 0 {
            return if self.hyp_words == 0 { 0.0 } else { 1.0 };
        }
        self.errors() as f64 / self.ref_words as f64
    }
}

impl AddAssign for WerStats {
    fn add_assign(&mut self, o: Self) {
        self.ref_words += o.ref_words;
        self.hyp_words += o.hyp_words;
        self.substitutions += o.substitutions;
        self.deletions += o.deletions;
        self.insertions += o.insertions;
    }
}

/// Opération d'alignement, indices dans la référence (`r`) et l'hypothèse
/// (`h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Match { r: usize, h: usize },
    Sub { r: usize, h: usize },
    Del { r: usize },
    Ins { h: usize },
}

/// Alignement de coût minimal entre deux suites de mots.
pub fn align<S: AsRef<str>>(reference: &[S], hypothesis: &[S]) -> Vec<Op> {
    let (n, m) = (reference.len(), hypothesis.len());
    let w = m + 1;
    let mut d = vec![0u32; (n + 1) * w];
    for (j, cell) in d.iter_mut().take(w).enumerate() {
        *cell = j as u32;
    }
    for i in 1..=n {
        d[i * w] = i as u32;
        for j in 1..=m {
            let same = reference[i - 1].as_ref() == hypothesis[j - 1].as_ref();
            let diag = d[(i - 1) * w + j - 1] + u32::from(!same);
            let up = d[(i - 1) * w + j] + 1;
            let left = d[i * w + j - 1] + 1;
            d[i * w + j] = diag.min(up).min(left);
        }
    }

    let mut ops = Vec::with_capacity(n.max(m));
    let (mut i, mut j) = (n, m);
    while i > 0 || j > 0 {
        let cur = d[i * w + j];
        if i > 0 && j > 0 {
            let same = reference[i - 1].as_ref() == hypothesis[j - 1].as_ref();
            if cur == d[(i - 1) * w + j - 1] + u32::from(!same) {
                i -= 1;
                j -= 1;
                ops.push(if same {
                    Op::Match { r: i, h: j }
                } else {
                    Op::Sub { r: i, h: j }
                });
                continue;
            }
        }
        if i > 0 && cur == d[(i - 1) * w + j] + 1 {
            i -= 1;
            ops.push(Op::Del { r: i });
        } else {
            j -= 1;
            ops.push(Op::Ins { h: j });
        }
    }
    ops.reverse();
    ops
}

/// Compte les erreurs entre deux suites de mots déjà normalisées.
pub fn wer<S: AsRef<str>>(reference: &[S], hypothesis: &[S]) -> WerStats {
    let mut stats = WerStats {
        ref_words: reference.len(),
        hyp_words: hypothesis.len(),
        ..Default::default()
    };
    for op in align(reference, hypothesis) {
        match op {
            Op::Match { .. } => {}
            Op::Sub { .. } => stats.substitutions += 1,
            Op::Del { .. } => stats.deletions += 1,
            Op::Ins { .. } => stats.insertions += 1,
        }
    }
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::normalize::{normalize, Lang};

    fn words(s: &str) -> Vec<&str> {
        s.split_whitespace().collect()
    }

    #[test]
    fn identical_is_zero() {
        let s = wer(&words("a b c"), &words("a b c"));
        assert_eq!(s.errors(), 0);
        assert_eq!(s.wer(), 0.0);
    }

    #[test]
    fn one_of_each_error() {
        // réf : le chat dort sur le tapis
        // hyp : le chien dort le tapis rouge
        let s = wer(
            &words("le chat dort sur le tapis"),
            &words("le chien dort le tapis rouge"),
        );
        assert_eq!((s.substitutions, s.deletions, s.insertions), (1, 1, 1));
        assert!((s.wer() - 0.5).abs() < 1e-12);
    }

    #[test]
    fn empty_cases() {
        assert_eq!(wer::<&str>(&[], &[]).wer(), 0.0);
        assert_eq!(wer(&[], &words("bruit")).wer(), 1.0);
        let all_deleted = wer(&words("a b c d"), &[]);
        assert_eq!(all_deleted.deletions, 4);
        assert_eq!(all_deleted.wer(), 1.0);
    }

    #[test]
    fn insertions_can_exceed_one() {
        let s = wer(&words("oui"), &words("euh oui oui alors"));
        assert_eq!(s.insertions, 3);
        assert_eq!(s.wer(), 3.0);
    }

    #[test]
    fn normalization_removes_formatting_errors() {
        let reference = normalize("Kobi, crée un ticket pour les 3 bugs.", Some(Lang::Fr));
        let hypothesis = normalize("kobi crée un ticket pour les trois bugs", Some(Lang::Fr));
        assert_eq!(wer(&reference, &hypothesis).errors(), 0);
    }

    #[test]
    fn alignment_reports_matched_pairs() {
        let ops = align(&words("a b c"), &words("a x c"));
        assert_eq!(
            ops,
            vec![
                Op::Match { r: 0, h: 0 },
                Op::Sub { r: 1, h: 1 },
                Op::Match { r: 2, h: 2 }
            ]
        );
    }

    #[test]
    fn accumulates() {
        let mut total = wer(&words("a b"), &words("a"));
        total += wer(&words("c d"), &words("c d e"));
        assert_eq!(total.ref_words, 4);
        assert_eq!(total.errors(), 2);
        assert_eq!(total.wer(), 0.5);
    }
}
