//! Transcription de référence d'une réunion.

use kobi_stt::fake::TimedWord;
use kobi_stt::Source;
use serde::{Deserialize, Serialize};

use super::normalize::{normalize, Lang};

/// Énoncé de la référence, avec ses mots horodatés si on les connaît.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RefUtterance {
    pub source: Source,
    pub start: f64,
    pub end: f64,
    pub text: String,
    /// Mots horodatés ; à défaut, leurs fins sont réparties sur l'énoncé au
    /// prorata du nombre de caractères.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub words: Option<Vec<TimedWord>>,
}

/// Mot normalisé de la référence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RefWord {
    pub source: Source,
    pub text: String,
    /// Fin du mot dans l'audio, en secondes.
    pub end: f64,
    /// Dernier mot de son énoncé.
    pub ends_utterance: bool,
}

/// Normalise la référence en mots horodatés, dans l'ordre des énoncés.
pub fn ref_words(utterances: &[RefUtterance], lang: Option<Lang>) -> Vec<RefWord> {
    let mut sorted: Vec<&RefUtterance> = utterances.iter().collect();
    sorted.sort_by(|a, b| a.start.total_cmp(&b.start));
    let mut out = Vec::new();
    for u in sorted {
        let lang = lang.unwrap_or_else(|| Lang::detect(&u.text));
        let first = out.len();
        match &u.words {
            Some(words) => {
                for w in words {
                    out.extend(
                        normalize(&w.text, Some(lang))
                            .into_iter()
                            .map(|text| RefWord {
                                source: u.source,
                                text,
                                end: w.end,
                                ends_utterance: false,
                            }),
                    );
                }
            }
            None => {
                let tokens = normalize(&u.text, Some(lang));
                let total: usize = tokens.iter().map(|t| t.chars().count()).sum();
                let mut acc = 0;
                for text in tokens {
                    acc += text.chars().count();
                    let end = u.start + (u.end - u.start) * acc as f64 / total.max(1) as f64;
                    out.push(RefWord {
                        source: u.source,
                        text,
                        end,
                        ends_utterance: false,
                    });
                }
            }
        }
        if out.len() > first {
            out.last_mut().expect("mot ajouté").ends_utterance = true;
        }
    }
    out
}

/// Mots de la référence pour un moteur factice scripté (texte brut).
pub fn timed_words(utterances: &[RefUtterance]) -> Vec<TimedWord> {
    let mut out = Vec::new();
    for u in utterances {
        match &u.words {
            Some(words) => out.extend(words.iter().cloned()),
            None => {
                let raw: Vec<&str> = u.text.split_whitespace().collect();
                let total: usize = raw.iter().map(|t| t.chars().count()).sum();
                let mut acc = 0;
                for text in raw {
                    let start = u.start + (u.end - u.start) * acc as f64 / total.max(1) as f64;
                    acc += text.chars().count();
                    let end = u.start + (u.end - u.start) * acc as f64 / total.max(1) as f64;
                    out.push(TimedWord {
                        source: u.source,
                        start,
                        end,
                        text: text.to_string(),
                    });
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolates_word_ends() {
        let u = RefUtterance {
            source: Source::Eux,
            start: 1.0,
            end: 2.0,
            text: "Oui, 2 bugs.".into(),
            words: None,
        };
        let w = ref_words(std::slice::from_ref(&u), Some(Lang::Fr));
        let texts: Vec<_> = w.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(texts, ["oui", "deux", "bugs"]);
        assert!((w[0].end - (1.0 + 3.0 / 11.0)).abs() < 1e-9);
        assert_eq!(w[2].end, 2.0);
        assert!(w[2].ends_utterance && !w[1].ends_utterance);

        let t = timed_words(&[u]);
        assert_eq!(t.len(), 3);
        assert_eq!(t[2].end, 2.0);
    }
}
