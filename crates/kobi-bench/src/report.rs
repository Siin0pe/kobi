//! Rapport du benchmark de transcription : `rapport.json` et `rapport.md`.
//!
//! Une ligne par moteur × réunion × machine. Un nouveau passage remplace la
//! ligne de même clé et garde les autres, pour remplir le tableau moteur par
//! moteur.

use std::fmt::Write as _;
use std::path::Path;

use kobi_stt::Source;
use serde::{Deserialize, Serialize};

use crate::metrics::latency::DelayStats;
use crate::metrics::SttMetrics;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReportRow {
    pub engine: String,
    pub meeting: String,
    pub machine: String,
    /// Matériel d'exécution du moteur (`cpu`, `cuda`…).
    pub device: String,
    /// Date de la mesure (UTC, RFC 3339).
    pub date: String,
    pub metrics: SttMetrics,
}

impl ReportRow {
    fn key(&self) -> (&str, &str, &str, &str) {
        (&self.engine, &self.meeting, &self.machine, &self.device)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub rows: Vec<ReportRow>,
}

impl Report {
    /// Charge `rapport.json` s'il existe, sinon un rapport vide.
    pub fn load_or_default(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
    }

    /// Ajoute une ligne ou remplace celle de même moteur, réunion, machine
    /// et matériel.
    pub fn upsert(&mut self, row: ReportRow) {
        match self.rows.iter_mut().find(|r| r.key() == row.key()) {
            Some(existing) => *existing = row,
            None => self.rows.push(row),
        }
        self.rows
            .sort_by(|a, b| a.meeting.cmp(&b.meeting).then(a.key().cmp(&b.key())));
    }

    /// Écrit `rapport.json` et `rapport.md` dans `dir`.
    pub fn write(&self, dir: &Path) -> anyhow::Result<()> {
        std::fs::create_dir_all(dir)?;
        std::fs::write(
            dir.join("rapport.json"),
            serde_json::to_string_pretty(self)? + "\n",
        )?;
        std::fs::write(dir.join("rapport.md"), self.to_markdown())?;
        Ok(())
    }

    /// Tableau markdown, à coller tel quel dans le document Benchmark.
    pub fn to_markdown(&self) -> String {
        let mut md = String::new();
        md.push_str(
            "| Moteur | Réunion | Machine | Matériel | WER | WER moi | WER eux \
             | 1er mot méd. / p90 | Final méd. / p90 | Fin de phrase méd. / p90 \
             | Vitesse | RTF | CPU moy. / pic | RAM pic |\n",
        );
        md.push_str(
            "| -- | -- | -- | -- | --: | --: | --: | --: | --: | --: | --: | --: | --: | --: |\n",
        );
        for r in &self.rows {
            let m = &r.metrics;
            let by = |source: Source| {
                m.wer_by_source
                    .iter()
                    .find(|s| s.source == source)
                    .map_or("—".to_string(), |s| pct(s.wer))
            };
            let delays = m.delays.as_ref();
            let _ = writeln!(
                md,
                "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} / {} | {} Mo |",
                r.engine,
                r.meeting,
                r.machine,
                r.device,
                pct(m.wer),
                by(Source::Moi),
                by(Source::Eux),
                delay(delays.and_then(|d| d.first_word)),
                delay(delays.and_then(|d| d.final_word)),
                delay(delays.and_then(|d| d.sentence_final)),
                if m.speed > 0.0 {
                    format!("{}×", num(m.speed, 1))
                } else {
                    "max".to_string()
                },
                num(m.rtf, 2),
                pct_raw(m.resources.cpu_mean_pct),
                pct_raw(m.resources.cpu_peak_pct),
                num(m.resources.rss_peak_mb, 0),
            );
        }
        md.push_str(
            "\nWER après normalisation (casse, ponctuation, nombres en lettres). \
             Délais en secondes depuis la fin du mot dans l'audio. \
             CPU en % d'un cœur logique. RTF = durée de traitement / durée de l'audio.\n",
        );
        md
    }
}

/// Nombre à la française (virgule décimale).
fn num(x: f64, decimals: usize) -> String {
    format!("{x:.decimals$}").replace('.', ",")
}

fn pct(ratio: f64) -> String {
    format!("{} %", num(ratio * 100.0, 1))
}

fn pct_raw(p: f64) -> String {
    format!("{} %", num(p, 0))
}

fn delay(stats: Option<DelayStats>) -> String {
    stats.map_or("—".to_string(), |s| {
        format!("{} / {} s", num(s.median, 2), num(s.p90, 2))
    })
}

/// Date courante en UTC au format RFC 3339, à la seconde.
pub fn now_rfc3339() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs()) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Algorithme « civil_from_days » de Howard Hinnant.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::resources::ResourceStats;
    use crate::metrics::wer::WerStats;

    fn row(engine: &str, meeting: &str, wer: f64) -> ReportRow {
        ReportRow {
            engine: engine.into(),
            meeting: meeting.into(),
            machine: "poste".into(),
            device: "cpu".into(),
            date: "2026-10-15T21:00:00Z".into(),
            metrics: SttMetrics {
                engine: engine.into(),
                audio_duration: 10.0,
                wall_duration: 10.1,
                rtf: 1.01,
                speed: 1.0,
                segments: 4,
                final_segments: 2,
                wer,
                wer_stats: WerStats::default(),
                wer_by_source: vec![],
                delays: None,
                resources: ResourceStats::default(),
            },
        }
    }

    #[test]
    fn upsert_replaces_same_key_and_sorts() {
        let mut r = Report::default();
        r.upsert(row("whisper", "fr", 0.2));
        r.upsert(row("fake", "en", 0.0));
        r.upsert(row("whisper", "fr", 0.1));
        assert_eq!(r.rows.len(), 2);
        assert_eq!(r.rows[0].meeting, "en");
        assert_eq!(r.rows[1].metrics.wer, 0.1);
    }

    #[test]
    fn markdown_has_one_line_per_row() {
        let mut r = Report::default();
        r.upsert(row("fake", "fr", 0.123));
        let md = r.to_markdown();
        let table: Vec<_> = md.lines().filter(|l| l.starts_with('|')).collect();
        assert_eq!(table.len(), 3);
        assert!(table[2].starts_with("| fake | fr | poste | cpu | 12,3 % | — | — | — |"));
    }

    #[test]
    fn rfc3339_shape() {
        let d = now_rfc3339();
        assert_eq!(d.len(), 20);
        assert!(d.starts_with("20") && d.ends_with('Z'));
    }
}
