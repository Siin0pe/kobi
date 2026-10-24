//! Langue de la question (KOB-268). Kobi entoure chaque question de
//! contexte en français (le moment, l'état de la réunion, la longueur
//! voulue) : pour une question courte en anglais, ce français l'emporte et
//! Claude répond en français. Un rappel en anglais, juste avant la question,
//! remet la règle « réponds dans la langue de la question » à sa place.

/// Mots outils anglais. Les mots aussi français (« a », « me », « on »,
/// « an ») n'y sont pas.
const ANGLAIS: &[&str] = &[
    "the",
    "is",
    "are",
    "was",
    "were",
    "what",
    "when",
    "where",
    "who",
    "why",
    "how",
    "which",
    "can",
    "could",
    "would",
    "should",
    "will",
    "do",
    "does",
    "did",
    "you",
    "your",
    "it",
    "its",
    "this",
    "that",
    "these",
    "those",
    "and",
    "or",
    "of",
    "to",
    "for",
    "with",
    "about",
    "from",
    "at",
    "my",
    "we",
    "our",
    "they",
    "there",
    "have",
    "has",
    "be",
    "i",
    "today",
    "tomorrow",
    "yesterday",
    "tell",
    "give",
    "please",
    "many",
    "much",
    "time",
    "day",
    "thanks",
    "all",
];

/// Mots outils français.
const FRANCAIS: &[&str] = &[
    "le", "la", "les", "un", "une", "des", "du", "de", "est", "et", "que", "qui", "quoi", "quel",
    "quelle", "quand", "où", "comment", "pourquoi", "tu", "vous", "je", "nous", "il", "elle", "on",
    "ce", "ça", "dans", "pour", "avec", "sur", "pas", "ne", "mon", "ma", "mes", "ton", "ta",
    "peux", "peut", "fais", "dis", "aujourd", "hui", "demain", "merci",
];

/// La question est-elle nettement en anglais ? Une phrase en franglais
/// (« on fait un call sur le pricing ? ») ne l'est pas.
pub fn nettement_anglaise(question: &str) -> bool {
    let minuscules = question.to_lowercase();
    let mots: Vec<&str> = minuscules
        .split(|c: char| !c.is_alphabetic())
        .filter(|m| !m.is_empty())
        .collect();
    let en = mots.iter().filter(|m| ANGLAIS.contains(m)).count();
    let fr = mots.iter().filter(|m| FRANCAIS.contains(m)).count();
    en >= 2 && en > 2 * fr
}

/// Rappel à placer juste avant une question nettement anglaise.
pub fn rappel_de_langue(question: &str) -> Option<&'static str> {
    nettement_anglaise(question)
        .then_some("(The question below is in English: answer in English, dates included.)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn questions_courtes_en_anglais() {
        for q in [
            "what day is it today",
            "', what day is it today",
            "Tell me a long story about a little robot who learns to cook pancakes",
            "Kobi, when is the launch?",
            "what time is it",
            "is it the weekend",
            "thanks, that's all",
        ] {
            assert!(nettement_anglaise(q), "{q}");
            assert!(rappel_de_langue(q).is_some(), "{q}");
        }
    }

    #[test]
    fn francais_et_franglais_sans_rappel() {
        for q in [
            "quel jour on est ?",
            "Kobi, on livre quand ?",
            "on fait un call sur le pricing du Q3 ?",
            "c'est quoi le scope du sprint ?",
            "dis-moi la date d'aujourd'hui",
            "OK",
            "",
        ] {
            assert!(!nettement_anglaise(q), "{q}");
        }
        assert_eq!(rappel_de_langue("quelle heure est-il ?"), None);
    }
}
