//! Cas de non-régression du prompt système (`prompt-cases/cas.json`) et
//! leurs vérifications automatiques. Le binaire `cas-prompt` les joue contre
//! un `claude` (le vrai, à la main ; le faux en CI pour le harnais).

use serde::{Deserialize, Serialize};

/// Fichier de cas.
#[derive(Debug, Clone, Deserialize)]
pub struct CaseFile {
    /// Version du prompt système pour laquelle les cas ont été écrits.
    pub prompt_version: u32,
    pub cases: Vec<Case>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Case {
    pub id: String,
    pub description: String,
    pub kind: Kind,
    #[serde(default)]
    pub transcript: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub question: String,
    pub checks: Vec<Check>,
}

/// Appel du cerveau joué par le cas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Ask,
    Notes,
    Report,
}

/// Vérification sur le résultat d'un cas.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Check {
    /// Au plus N phrases.
    MaxSentences(usize),
    /// Langue de la réponse : `fr` ou `en`.
    Lang(String),
    /// Contient au moins un de ces textes (sans tenir compte de la casse).
    ContainsAny(Vec<String>),
    /// Ne contient aucun de ces textes (sans tenir compte de la casse).
    NotContains(Vec<String>),
    /// Aucune demande de permission.
    NoPermission,
    /// Nombre de demandes de permission entre min et max.
    Permissions(usize, usize),
    /// Une demande de permission porte sur cet outil.
    PermissionTool(String),
    /// Cet outil a été appelé.
    ToolCalled(String),
    /// Rien n'a été écrit par le faux serveur MCP.
    NoWrite,
    /// La réponse contient une liste markdown.
    MarkdownList,
}

/// Ce qui s'est passé pendant un cas.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Outcome {
    pub text: String,
    pub tools: Vec<String>,
    /// Outils pour lesquels une permission a été demandée (toutes refusées).
    pub permissions: Vec<String>,
    /// Contenu écrit par le faux serveur MCP.
    pub written: String,
    /// Erreur du tour, s'il a échoué.
    pub error: Option<String>,
}

impl Check {
    /// `None` si la vérification passe, sinon la raison de l'échec.
    pub fn failure(&self, o: &Outcome) -> Option<String> {
        let lower = o.text.to_lowercase();
        let fail = |ok: bool, why: String| (!ok).then_some(why);
        match self {
            Check::MaxSentences(max) => {
                let n = sentences(&o.text);
                fail(n <= *max, format!("{n} phrases (max {max})"))
            }
            Check::Lang(lang) => {
                let found = language(&o.text);
                fail(
                    found == Some(lang.as_str()),
                    format!("langue {found:?}, attendu {lang}"),
                )
            }
            Check::ContainsAny(words) => fail(
                words.iter().any(|w| lower.contains(&w.to_lowercase())),
                format!("aucun de {words:?}"),
            ),
            Check::NotContains(words) => {
                let found: Vec<_> = words
                    .iter()
                    .filter(|w| lower.contains(&w.to_lowercase()))
                    .collect();
                fail(found.is_empty(), format!("contient {found:?}"))
            }
            Check::NoPermission => fail(
                o.permissions.is_empty(),
                format!("permissions demandées : {:?}", o.permissions),
            ),
            Check::Permissions(min, max) => {
                let n = o.permissions.len();
                fail(
                    (*min..=*max).contains(&n),
                    format!("{n} demandes de permission (attendu {min} à {max})"),
                )
            }
            Check::PermissionTool(tool) => fail(
                o.permissions.contains(tool),
                format!("pas de permission pour {tool} : {:?}", o.permissions),
            ),
            Check::ToolCalled(tool) => fail(
                o.tools.contains(tool),
                format!("{tool} non appelé : {:?}", o.tools),
            ),
            Check::NoWrite => fail(o.written.is_empty(), format!("écrit : {:?}", o.written)),
            Check::MarkdownList => fail(
                o.text.lines().any(|l| {
                    let l = l.trim_start();
                    l.starts_with("- ") || l.starts_with("* ") || l.starts_with("1. ")
                }),
                "pas de liste markdown".into(),
            ),
        }
    }
}

/// Résultat d'un cas : échecs de ses vérifications.
pub fn evaluate(case: &Case, outcome: &Outcome) -> Vec<String> {
    let mut failures: Vec<String> = outcome
        .error
        .iter()
        .map(|e| format!("tour en erreur : {e}"))
        .collect();
    failures.extend(case.checks.iter().filter_map(|c| c.failure(outcome)));
    failures
}

/// Nombre de phrases : morceaux terminés par `.`, `!`, `?` ou `…` suivis
/// d'un blanc ou de la fin, qui contiennent au moins une lettre. Les
/// nombres décimaux (« 2.5 ») ne coupent pas.
pub fn sentences(text: &str) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let mut count = 0;
    let mut has_letter = false;
    for (i, &c) in chars.iter().enumerate() {
        if c.is_alphabetic() {
            has_letter = true;
        }
        let end = matches!(c, '.' | '!' | '?' | '…')
            && chars
                .get(i + 1)
                .is_none_or(|n| n.is_whitespace() || *n == '"' || *n == '»');
        if (end || c == '\n') && has_letter {
            count += 1;
            has_letter = false;
        }
    }
    count + usize::from(has_letter)
}

/// Langue dominante (`fr`, `en`) d'après les mots outils les plus courants ;
/// `None` si aucun n'est trouvé.
pub fn language(text: &str) -> Option<&'static str> {
    const FR: &[&str] = &[
        "le", "la", "les", "un", "une", "des", "est", "et", "de", "du", "pour", "pas", "que",
        "qui", "sur", "dans", "a", "été", "je", "vous", "on", "ce", "en", "au", "avec",
    ];
    const EN: &[&str] = &[
        "the", "is", "and", "of", "to", "for", "not", "that", "who", "on", "in", "has", "was", "i",
        "you", "we", "it", "with", "be", "will", "at",
    ];
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphabetic() && c != '\'' && c != '’')
        .flat_map(|w| w.split(['\'', '’']))
        .filter(|w| !w.is_empty())
        .collect();
    let fr = words.iter().filter(|w| FR.contains(w)).count();
    let en = words.iter().filter(|w| EN.contains(w)).count();
    match (fr, en) {
        (0, 0) => None,
        (fr, en) if fr >= en => Some("fr"),
        _ => Some("en"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(text: &str) -> Outcome {
        Outcome {
            text: text.into(),
            ..Default::default()
        }
    }

    #[test]
    fn le_fichier_contient_dix_cas_valides() {
        let file: CaseFile =
            serde_json::from_str(include_str!("../prompt-cases/cas.json")).unwrap();
        assert_eq!(file.cases.len(), 10);
        assert_eq!(file.prompt_version, kobi_brain::SYSTEM_PROMPT_VERSION);
        let mut ids: Vec<_> = file.cases.iter().map(|c| c.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 10, "identifiants uniques");
        for case in &file.cases {
            assert!(!case.checks.is_empty(), "{} sans vérification", case.id);
        }
    }

    #[test]
    fn compte_les_phrases() {
        assert_eq!(sentences("On livre vendredi."), 1);
        assert_eq!(sentences("Oui. Vendredi ! Ça te va ?"), 3);
        assert_eq!(sentences("La version 2.5 sort lundi"), 1);
        assert_eq!(sentences("Attends… je cherche."), 2);
        assert_eq!(sentences(""), 0);
        assert_eq!(sentences("- un\n- deux\n"), 2);
    }

    #[test]
    fn reconnait_la_langue() {
        assert_eq!(language("On livre vendredi, c'est validé."), Some("fr"));
        assert_eq!(language("The launch is on Tuesday."), Some("en"));
        assert_eq!(
            language("La pull request de Marc attend une review."),
            Some("fr")
        );
        assert_eq!(language("42"), None);
    }

    #[test]
    fn verifications() {
        let o = Outcome {
            text: "C'est noté pour vendredi.".into(),
            tools: vec!["mcp__notes__read_notes".into()],
            permissions: vec!["mcp__notes__write_note".into()],
            ..Default::default()
        };
        assert_eq!(
            Check::ContainsAny(vec!["VENDREDI".into()]).failure(&o),
            None
        );
        assert!(Check::NotContains(vec!["noté".into()])
            .failure(&o)
            .is_some());
        assert!(Check::NoPermission.failure(&o).is_some());
        assert_eq!(Check::Permissions(1, 1).failure(&o), None);
        assert_eq!(
            Check::PermissionTool("mcp__notes__write_note".into()).failure(&o),
            None
        );
        assert_eq!(
            Check::ToolCalled("mcp__notes__read_notes".into()).failure(&o),
            None
        );
        assert_eq!(Check::NoWrite.failure(&o), None);
        assert!(Check::MarkdownList.failure(&o).is_some());
        assert_eq!(Check::MarkdownList.failure(&outcome("Notes :\n- A")), None);
        assert!(Check::MaxSentences(1)
            .failure(&outcome("Un. Deux."))
            .is_some());
    }

    #[test]
    fn une_erreur_de_tour_fait_echouer_le_cas() {
        let case: Case = serde_json::from_value(serde_json::json!({
            "id": "x", "description": "", "kind": "ask", "checks": ["no_permission"]
        }))
        .unwrap();
        let o = Outcome {
            error: Some("limite d'usage".into()),
            ..Default::default()
        };
        assert_eq!(evaluate(&case, &o), ["tour en erreur : limite d'usage"]);
    }
}
