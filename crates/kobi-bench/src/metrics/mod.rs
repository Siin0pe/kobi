//! Métriques du benchmark de transcription : WER, délais, CPU et mémoire.

pub mod latency;
pub mod normalize;
pub mod reference;
pub mod resources;
pub mod wer;

use kobi_stt::replay::ReplayOutcome;
use kobi_stt::Source;
use serde::{Deserialize, Serialize};

use latency::Delays;
use normalize::{normalize, Lang};
use reference::{ref_words, RefUtterance};
use resources::ResourceStats;
use wer::WerStats;

/// WER d'une source (piste `moi` ou `eux`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SourceWer {
    pub source: Source,
    pub wer: f64,
    #[serde(flatten)]
    pub stats: WerStats,
}

/// Toutes les métriques d'une relecture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SttMetrics {
    pub engine: String,
    pub audio_duration: f64,
    pub wall_duration: f64,
    /// Facteur temps réel : durée de traitement / durée de l'audio, mesuré
    /// sur une relecture sans attente ; à 1× il vaut environ 1 par
    /// construction.
    pub rtf: f64,
    pub speed: f64,
    pub segments: usize,
    pub final_segments: usize,
    pub wer: f64,
    pub wer_stats: WerStats,
    pub wer_by_source: Vec<SourceWer>,
    pub delays: Option<Delays>,
    pub resources: ResourceStats,
}

/// Calcule les métriques d'une relecture par rapport à la référence.
///
/// Le WER porte sur les segments finaux de chaque source, mis bout à bout
/// dans l'ordre de leur début. `lang` force la langue de normalisation ; à
/// défaut, elle est détectée énoncé par énoncé.
pub fn evaluate(
    reference: &[RefUtterance],
    outcome: &ReplayOutcome,
    resources: ResourceStats,
    lang: Option<Lang>,
) -> SttMetrics {
    let words = ref_words(reference, lang);

    let mut wer_stats = WerStats::default();
    let mut wer_by_source = Vec::new();
    for source in [Source::Moi, Source::Eux] {
        let r: Vec<&str> = words
            .iter()
            .filter(|w| w.source == source)
            .map(|w| w.text.as_str())
            .collect();
        let mut finals: Vec<_> = outcome.finals().filter(|s| s.source == source).collect();
        if r.is_empty() && finals.is_empty() {
            continue;
        }
        finals.sort_by(|a, b| a.start.total_cmp(&b.start));
        let h: Vec<String> = finals
            .iter()
            .flat_map(|s| normalize(&s.text, lang))
            .collect();
        let h: Vec<&str> = h.iter().map(String::as_str).collect();
        let stats = wer::wer(&r, &h);
        wer_stats += stats;
        wer_by_source.push(SourceWer {
            source,
            wer: stats.wer(),
            stats,
        });
    }

    SttMetrics {
        engine: outcome.engine.clone(),
        audio_duration: outcome.audio_duration,
        wall_duration: outcome.wall_duration,
        rtf: if outcome.audio_duration > 0.0 {
            outcome.wall_duration / outcome.audio_duration
        } else {
            0.0
        },
        speed: outcome.speed,
        segments: outcome.emissions.len(),
        final_segments: outcome.finals().count(),
        wer: wer_stats.wer(),
        wer_stats,
        wer_by_source,
        delays: latency::delays(&words, &outcome.emissions, outcome.speed, lang),
        resources,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobi_stt::fake::FakeEngine;
    use kobi_stt::replay::{Track, WavReplayer};
    use kobi_stt::SAMPLE_RATE;
    use reference::timed_words;
    use resources::ResourceMonitor;
    use std::time::Duration;

    fn utt(source: Source, start: f64, end: f64, text: &str) -> RefUtterance {
        RefUtterance {
            source,
            start,
            end,
            text: text.into(),
            lang: None,
            words: None,
        }
    }

    /// Critère de validation de KOB-16 : le moteur factice produit un rapport
    /// complet.
    #[test]
    fn fake_engine_produces_a_complete_report() {
        let reference = vec![
            utt(Source::Moi, 0.2, 1.6, "Kobi, combien de tickets ouverts ?"),
            utt(Source::Eux, 2.0, 3.2, "On en a 12 cette semaine."),
            utt(Source::Moi, 3.6, 4.8, "Merci, crée un ticket."),
        ];
        // Le moteur restitue la référence avec une erreur et 0,3 s de retard.
        let mut script = timed_words(&reference);
        script
            .iter_mut()
            .find(|w| w.text == "semaine.")
            .expect("mot présent")
            .text = "semaines".into();
        let mut engine = FakeEngine::scripted(script, 0.3);

        let silence = vec![0.0; 5 * SAMPLE_RATE as usize];
        let tracks = vec![
            Track {
                source: Source::Moi,
                samples: silence.clone(),
            },
            Track {
                source: Source::Eux,
                samples: silence,
            },
        ];
        let monitor = ResourceMonitor::start(Duration::from_millis(200));
        let outcome = WavReplayer::new(tracks)
            .speed(5.0)
            .run(&mut engine)
            .unwrap();
        let metrics = evaluate(&reference, &outcome, monitor.stop(), None);

        assert_eq!(metrics.engine, "fake");
        assert_eq!(metrics.audio_duration, 5.0);
        assert!(metrics.final_segments >= 3);
        // Référence : 5 + 6 (« douze » compris) + 4 = 15 mots, une substitution.
        assert_eq!(metrics.wer_stats.ref_words, 15);
        assert_eq!(metrics.wer_stats.errors(), 1);
        assert!((metrics.wer - 1.0 / 15.0).abs() < 1e-9);
        assert_eq!(metrics.wer_by_source.len(), 2);

        let delays = metrics.delays.as_ref().expect("délais à 5×");
        let first = delays.first_word.expect("premier mot");
        assert_eq!(first.count, 14);
        // 0,3 s de retard du moteur + au plus une trame de 20 ms × 5.
        assert!(first.median >= 0.29 && first.median < 0.5, "{first:?}");
        assert!(delays.final_word.is_some());
        // La fin de phrase « semaine » est mal transcrite : deux phrases sur trois.
        assert_eq!(delays.sentence_final.expect("fin de phrase").count, 2);
        assert!(metrics.resources.samples >= 1);
        assert!(metrics.resources.rss_peak_mb > 0.0);
    }
}
