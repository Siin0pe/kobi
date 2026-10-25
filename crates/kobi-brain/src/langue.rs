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
    "le",
    "la",
    "les",
    "un",
    "une",
    "des",
    "du",
    "de",
    "est",
    "et",
    "que",
    "qui",
    "quoi",
    "quel",
    "quelle",
    "quand",
    "où",
    "comment",
    "pourquoi",
    "tu",
    "vous",
    "je",
    "nous",
    "il",
    "elle",
    "on",
    "ce",
    "ça",
    "dans",
    "pour",
    "avec",
    "sur",
    "pas",
    "ne",
    "mon",
    "ma",
    "mes",
    "ton",
    "ta",
    "peux",
    "peut",
    "fais",
    "dis",
    "aujourd",
    "hui",
    "demain",
    "merci",
    // Une demande en français sur une phrase anglaise (KOB-290).
    "en",
    "traduis",
    "traduire",
    "traduction",
    "explique",
    "signifie",
    "veut",
    "dire",
];

/// La question est-elle nettement en anglais ? Une phrase en franglais
/// (« on fait un call sur le pricing ? ») ne l'est pas.
pub fn nettement_anglaise(question: &str) -> bool {
    // Une phrase citée (« Traduis "where is the station" en espagnol ») ne
    // dit pas la langue de la demande (KOB-290) ; une question entière entre
    // guillemets, si.
    let hors_citations = sans_citations(question);
    let question = if hors_citations.chars().any(char::is_alphabetic) {
        hors_citations.as_str()
    } else {
        question
    };
    let minuscules = question.to_lowercase();
    let mots: Vec<&str> = minuscules
        .split(|c: char| !c.is_alphabetic())
        .filter(|m| !m.is_empty())
        .collect();
    let en = mots.iter().filter(|m| ANGLAIS.contains(m)).count();
    let fr = mots.iter().filter(|m| FRANCAIS.contains(m)).count();
    en >= 2 && en > 2 * fr
}

/// Le texte sans ce qui est entre guillemets doubles (`"…"`, `“…”`, `«…»`).
/// Un guillemet sans fermant est gardé, comme l'apostrophe (« l'heure »).
fn sans_citations(texte: &str) -> String {
    let mut reste = String::new();
    let mut chars = texte.chars().peekable();
    while let Some(c) = chars.next() {
        let fermant = match c {
            '"' => '"',
            '“' => '”',
            '«' => '»',
            _ => {
                reste.push(c);
                continue;
            }
        };
        let suite: String = chars.clone().collect();
        match suite.find(fermant) {
            Some(i) => {
                // Saute la citation et son guillemet fermant.
                let n = suite[..i].chars().count() + 1;
                for _ in 0..n {
                    chars.next();
                }
                reste.push(' ');
            }
            None => reste.push(c),
        }
    }
    reste
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

    /// KOB-290 : une demande en français qui cite de l'anglais est en
    /// français ; une question anglaise entre guillemets reste anglaise.
    #[test]
    fn citation_anglaise_dans_une_demande_francaise() {
        for q in [
            "Traduis \"where is the station\" en espagnol",
            "Explique \"what does it do\"",
            "Comment dit-on « what time is it » ?",
            "Que veut dire “how do you do” ?",
        ] {
            assert!(!nettement_anglaise(q), "{q}");
        }
        assert!(nettement_anglaise("\"what day is it today\""));
        assert!(nettement_anglaise("what does \"pricing\" mean here?"));
    }
}
