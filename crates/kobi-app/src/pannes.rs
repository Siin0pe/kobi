//! Pannes du cerveau (KOB-68) : ce qui peut arriver à Claude Code pendant
//! un tour, et ce qu'on en dit à l'utilisateur. Table « Erreurs et cas
//! limites » du document « parcours utilisateur et règles d'UX » :
//!
//! | Panne | Réaction |
//! | -- | -- |
//! | Claude Code plante | session relancée, question renvoyée une fois |
//! | rien pendant longtemps | « je cherche encore » à 8 s, abandon à 5 min sans signe de vie (outil en cours et carte en attente exclus), arrêt à 15 min (réglables, KOB-243) |
//! | limite d'usage, Claude absent ou déconnecté | Oups avec la cause et la marche à suivre |
//! | pas d'Internet | question mise en attente, reposée au retour du réseau |
//!
//! Dans tous les cas, l'écoute, la transcription et l'enregistrement
//! continuent : ils ne passent pas par le cerveau.

use kobi_brain::BrainError;

/// Ce qui est arrivé, du point de vue de l'utilisateur.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Panne {
    /// Le process de Claude Code s'est arrêté pendant le tour.
    Plantage,
    /// Aucun signe de vie trop longtemps.
    Lenteur,
    /// Tâche arrêtée à sa durée maximale, même active (KOB-243).
    DureeMax,
    /// Limite d'usage du compte atteinte.
    LimiteUsage,
    /// Claude Code installé mais pas connecté.
    Deconnecte,
    /// Claude Code introuvable ou qui ne répond pas.
    Absent,
    /// Backend clé API : la clé enregistrée est refusée (401, 403).
    CleRefusee,
    /// Serveurs d'Anthropic surchargés ou en panne passagère (529, 5xx).
    Surcharge,
    /// Pas d'accès à Internet.
    HorsLigne,
    /// Autre erreur renvoyée par Claude.
    Autre,
}

/// Signes d'une coupure réseau dans les messages de Claude Code et de Node.
const RESEAU: &[&str] = &[
    "connection error",
    "network",
    "enotfound",
    "econnrefused",
    "econnreset",
    "etimedout",
    "eai_again",
    "getaddrinfo",
    "fetch failed",
    "unable to connect",
    "socket hang up",
    "internet",
    // Backend clé API (KOB-361).
    "api injoignable",
];

/// Signes d'une surcharge passagère des serveurs d'Anthropic (KOB-362).
const SURCHARGE: &[&str] = &[
    "overloaded",
    "http 529",
    "http 500",
    "http 502",
    "http 503",
    "http 504",
];

/// Signes d'une limite d'usage.
const LIMITE: &[&str] = &[
    "usage limit",
    "rate limit",
    "limit reached",
    "rate_limit",
    "too many requests",
    "quota",
];

/// Limite par minute du backend clé API (HTTP 429, `rate_limit_error`) :
/// elle passe d'elle-même, ce n'est pas la limite d'usage d'un abonnement
/// (KOB-369). Ses messages viennent de `kobi-brain` (« HTTP 429 : … »,
/// « rate_limit_error : … »), jamais de Claude Code.
fn limite_par_minute(m: &str) -> bool {
    m.contains("http 429") || m.starts_with("rate_limit_error")
}

/// « 429 » seul (un code), pas au milieu d'un nombre (« 204291 tokens »).
fn code_429(m: &str) -> bool {
    m.match_indices("429").any(|(i, _)| {
        let avant = m[..i].chars().next_back();
        let apres = m[i + 3..].chars().next();
        !avant.is_some_and(|c| c.is_ascii_digit()) && !apres.is_some_and(|c| c.is_ascii_digit())
    })
}

/// Classe un message d'erreur (texte d'une [`BrainError`] ou d'un résultat).
pub fn classer_message(message: &str) -> Panne {
    let m = message.to_lowercase();
    let contient = |mots: &[&str]| mots.iter().any(|mot| m.contains(mot));
    if limite_par_minute(&m) {
        Panne::Surcharge
    } else if contient(LIMITE) || code_429(&m) {
        Panne::LimiteUsage
    } else if contient(RESEAU) {
        Panne::HorsLigne
    } else if contient(SURCHARGE) {
        Panne::Surcharge
    } else if m.contains("clé api refusée") || m.contains("accès refusé") {
        Panne::CleRefusee
    } else if m.contains("pas connecté") || m.contains("auth login") || m.contains("not logged in")
    {
        Panne::Deconnecte
    } else if m.contains("cerveau indisponible")
        || m.contains("introuvable")
        || m.contains("notfound")
        || m.contains("ne répond pas")
    {
        Panne::Absent
    } else if m.contains("cerveau arrêté") {
        Panne::Plantage
    } else if m.contains("pas de réponse en") {
        Panne::Lenteur
    } else if m.contains("durée maximale") {
        Panne::DureeMax
    } else {
        Panne::Autre
    }
}

pub fn classer(e: &BrainError) -> Panne {
    match e {
        // Le message prime : un process qui s'arrête faute de réseau est une
        // coupure, pas un plantage.
        BrainError::Crashed(m) => match classer_message(m) {
            p @ (Panne::HorsLigne | Panne::LimiteUsage) => p,
            _ => Panne::Plantage,
        },
        BrainError::Timeout(_) => Panne::Lenteur,
        BrainError::Unavailable(m) => match classer_message(m) {
            p @ (Panne::Deconnecte | Panne::HorsLigne | Panne::CleRefusee) => p,
            _ => Panne::Absent,
        },
        BrainError::Api(m) => match classer_message(m) {
            p @ (Panne::LimiteUsage | Panne::HorsLigne | Panne::Surcharge) => p,
            _ => Panne::Autre,
        },
        BrainError::Busy => Panne::Autre,
    }
}

/// Seule annonce d'une tâche longue, quand elle tarde (15 s, KOB-215), dans
/// la langue de l'interface (KOB-210).
pub fn patience() -> String {
    kobi_i18n::tr("coque.pannes.patience", &[])
}

/// Annonce d'une session relancée après un plantage.
pub fn relance() -> String {
    kobi_i18n::tr("coque.pannes.relance", &[])
}

/// Un texte traduit en `&'static str`, gardé une fois : les titres d'Oups
/// sont en nombre fini (une poignée par langue), la mémoire reste bornée.
fn statique(texte: String) -> &'static str {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};
    static GARDES: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut gardes = GARDES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(&s) = gardes.get(texte.as_str()) {
        return s;
    }
    let s: &'static str = Box::leak(texte.into_boxed_str());
    gardes.insert(s);
    s
}

/// Titre et message de l'Oups : la cause en une phrase, puis la marche à
/// suivre. L'écoute et l'enregistrement continuent, on le rappelle.
pub fn expliquer(panne: &Panne, detail: &str) -> (&'static str, String) {
    let continue_ = kobi_i18n::tr("coque.pannes.continue", &[]);
    let c = continue_.as_str();
    let (titre, message) = match panne {
        Panne::LimiteUsage => {
            let jusqu_a = reprise(detail)
                .map(|h| kobi_i18n::tr("coque.pannes.jusquA", &[("heure", &h)]))
                .unwrap_or_default();
            (
                kobi_i18n::tr("coque.pannes.limiteTitre", &[]),
                kobi_i18n::tr(
                    "coque.pannes.limiteMessage",
                    &[("jusquA", &jusqu_a), ("suite", c)],
                ),
            )
        }
        Panne::Deconnecte => (
            kobi_i18n::tr("coque.pannes.deconnecteTitre", &[]),
            kobi_i18n::tr("coque.pannes.deconnecteMessage", &[("suite", c)]),
        ),
        Panne::Absent => (
            kobi_i18n::tr("coque.pannes.absentTitre", &[]),
            kobi_i18n::tr("coque.pannes.absentMessage", &[("suite", c)]),
        ),
        Panne::CleRefusee => (
            kobi_i18n::tr("coque.pannes.cleRefuseeTitre", &[]),
            kobi_i18n::tr("coque.pannes.cleRefuseeMessage", &[("suite", c)]),
        ),
        Panne::Surcharge => (
            kobi_i18n::tr("coque.pannes.surchargeTitre", &[]),
            kobi_i18n::tr("coque.pannes.surchargeMessage", &[("suite", c)]),
        ),
        Panne::HorsLigne => (
            kobi_i18n::tr("coque.pannes.horsLigneTitre", &[]),
            kobi_i18n::tr("coque.pannes.horsLigneMessage", &[("suite", c)]),
        ),
        Panne::Lenteur => {
            let duree =
                attente(detail).unwrap_or_else(|| kobi_i18n::tr("coque.pannes.tropLongtemps", &[]));
            (
                kobi_i18n::tr("coque.pannes.lenteurTitre", &[]),
                kobi_i18n::tr(
                    "coque.pannes.lenteurMessage",
                    &[("duree", &duree), ("suite", c)],
                ),
            )
        }
        Panne::DureeMax => {
            let duree = entre_parentheses(detail)
                .map(|d| format!(" ({d})"))
                .unwrap_or_default();
            (
                kobi_i18n::tr("coque.pannes.dureeMaxTitre", &[]),
                kobi_i18n::tr(
                    "coque.pannes.dureeMaxMessage",
                    &[("duree", &duree), ("suite", c)],
                ),
            )
        }
        Panne::Plantage => (
            kobi_i18n::tr("coque.pannes.plantageTitre", &[]),
            kobi_i18n::tr("coque.pannes.plantageMessage", &[("suite", c)]),
        ),
        Panne::Autre => (
            kobi_i18n::tr("coque.pannes.autreTitre", &[]),
            // Le détail est une phrase à part : un point s'il n'en a pas
            // (KOB-362).
            if detail.trim_end().ends_with(['.', '!', '?', '…']) {
                format!("{} {continue_}", detail.trim_end())
            } else {
                format!("{}. {continue_}", detail.trim_end())
            },
        ),
    };
    (statique(titre), message)
}

/// Heure de reprise d'une limite d'usage, si Claude Code la donne
/// (« …limit reached|1792339200 », horodatage Unix en secondes).
/// « pas de réponse en 300 s » → « 5 minutes » ; « … en 45 s » → « 45 secondes ».
fn attente(detail: &str) -> Option<String> {
    let apres = detail.split("pas de réponse en ").nth(1)?;
    let secondes: u64 = apres.split_whitespace().next()?.parse().ok()?;
    Some(if secondes >= 120 && secondes.is_multiple_of(60) {
        kobi_i18n::tr("commun.minutes", &[("n", &(secondes / 60).to_string())])
    } else {
        kobi_i18n::tr("coque.pannes.secondes", &[("n", &secondes.to_string())])
    })
}

/// Le texte entre les dernières parenthèses : « … atteinte (15 min) » → « 15 min ».
fn entre_parentheses(detail: &str) -> Option<&str> {
    let debut = detail.rfind('(')?;
    let fin = detail[debut..].find(')')? + debut;
    Some(&detail[debut + 1..fin])
}

fn reprise(detail: &str) -> Option<String> {
    let secondes: i64 = detail.rsplit('|').next()?.trim().parse().ok()?;
    let t = jiff::Timestamp::from_second(secondes).ok()?;
    let local = t.to_zoned(jiff::tz::TimeZone::system());
    Some(kobi_i18n::tr(
        "coque.pannes.heure",
        &[
            ("h", &format!("{:02}", local.hour())),
            ("m", &format!("{:02}", local.minute())),
        ],
    ))
}

/// Le message vu par l'utilisateur pour une erreur de tour.
pub fn pour_l_utilisateur(message: &str) -> (&'static str, String) {
    expliquer(&classer_message(message), message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// KOB-243 : l'abandon dit la vraie durée, l'arrêt à la durée maximale
    /// a son propre message.
    #[test]
    fn abandon_et_duree_maximale_expliques() {
        let abandon = BrainError::Timeout(Duration::from_secs(300)).to_string();
        let (titre, explication) = pour_l_utilisateur(&abandon);
        assert_eq!(titre, "Claude ne répond pas");
        assert!(
            explication.contains("Rien depuis 5 minutes"),
            "{explication}"
        );
        let court = BrainError::Timeout(Duration::from_secs(45)).to_string();
        assert!(pour_l_utilisateur(&court)
            .1
            .contains("Rien depuis 45 secondes"));
        let (titre, explication) =
            pour_l_utilisateur("durée maximale d'une tâche atteinte (15 min)");
        assert_eq!(titre, "Tâche trop longue");
        assert!(explication.contains("(15 min)") && explication.contains("taches.duree_max_s"));
    }

    #[test]
    fn classement_des_pannes() {
        let api = |m: &str| BrainError::Api(m.into());
        assert_eq!(
            classer(&api("Claude AI usage limit reached|1792339200")),
            Panne::LimiteUsage
        );
        assert_eq!(
            classer(&api("API Error: Connection error.")),
            Panne::HorsLigne
        );
        assert_eq!(
            classer(&api("getaddrinfo ENOTFOUND api.anthropic.com")),
            Panne::HorsLigne
        );
        assert_eq!(classer(&api("Invalid request")), Panne::Autre);
        assert_eq!(
            classer(&BrainError::Crashed("exit status 3".into())),
            Panne::Plantage
        );
        assert_eq!(
            classer(&BrainError::Crashed("fetch failed".into())),
            Panne::HorsLigne
        );
        assert_eq!(
            classer(&BrainError::Timeout(Duration::from_secs(45))),
            Panne::Lenteur
        );
        assert_eq!(
            classer(&BrainError::Unavailable(
                "Claude Code n'est pas connecté : lancer « claude auth login »".into()
            )),
            Panne::Deconnecte
        );
        assert_eq!(
            classer(&BrainError::Unavailable(
                "Claude Code indisponible : NotFound".into()
            )),
            Panne::Absent
        );
        // Depuis le texte d'une erreur déjà affichée (Display de BrainError).
        let texte = BrainError::Api("Claude AI usage limit reached".into()).to_string();
        assert_eq!(classer_message(&texte), Panne::LimiteUsage);
        let texte = BrainError::Unavailable("Claude Code n'est pas connecté".into()).to_string();
        assert_eq!(classer_message(&texte), Panne::Deconnecte);
        let texte = BrainError::Crashed("exit status 3".into()).to_string();
        assert_eq!(classer_message(&texte), Panne::Plantage);
    }

    #[test]
    fn messages_d_oups() {
        let (titre, message) =
            pour_l_utilisateur("erreur d'API : Claude AI usage limit reached|1792339200");
        assert_eq!(titre, "Limite d'usage atteinte");
        assert!(
            message.contains("jusqu'à ") && message.contains(" h "),
            "{message}"
        );
        assert!(message.ends_with("L'écoute et l'enregistrement continuent."));
        let (titre, message) =
            pour_l_utilisateur("cerveau indisponible : Claude Code n'est pas connecté");
        assert_eq!(titre, "Claude Code n'est pas connecté");
        assert!(message.contains("claude auth login"));
        let (titre, _) = pour_l_utilisateur("erreur d'API : API Error: Connection error.");
        assert_eq!(titre, "Pas d'Internet");
        // Une erreur inconnue garde son détail.
        let (titre, message) = pour_l_utilisateur("erreur d'API : Invalid request");
        assert_eq!(titre, "Claude n'a pas répondu");
        assert!(message.starts_with("erreur d'API : Invalid request"));
    }

    /// KOB-361 : backend clé API. Une clé refusée renvoie aux Réglages, pas
    /// à Claude Code ; l'API injoignable est une coupure de réseau (question
    /// gardée). Messages tels que les rend `kobi-brain`.
    #[test]
    fn pannes_du_backend_cle_api() {
        use kobi_i18n::Langue;
        assert_eq!(
            classer(&BrainError::Unavailable(
                "clé API refusée : invalid x-api-key".into()
            )),
            Panne::CleRefusee
        );
        assert_eq!(
            classer(&BrainError::Unavailable("accès refusé : forbidden".into())),
            Panne::CleRefusee
        );
        assert_eq!(
            classer(&BrainError::Api(
                "API injoignable : error sending request for url (https://api.anthropic.com/v1/messages)"
                    .into()
            )),
            Panne::HorsLigne
        );
        // Claude Code absent reste « introuvable ».
        assert_eq!(
            classer(&BrainError::Unavailable(
                "Claude Code indisponible : NotFound".into()
            )),
            Panne::Absent
        );
        let (titre, message) =
            pour_l_utilisateur("cerveau indisponible : clé API refusée : invalid x-api-key");
        assert_eq!(titre, "Clé API refusée");
        assert!(message.contains("Réglages") && !message.contains("Claude Code"));
        assert!(message.ends_with("L'écoute et l'enregistrement continuent."));
        let (titre, _) =
            pour_l_utilisateur("erreur d'API : API injoignable : error sending request");
        assert_eq!(titre, "Pas d'Internet");
        let suite = kobi_i18n::t(Langue::En, "coque.pannes.continue", &[]);
        assert_eq!(
            kobi_i18n::t(
                Langue::En,
                "coque.pannes.cleRefuseeMessage",
                &[("suite", &suite)]
            ),
            "The Anthropic API refuses the saved key. Check or replace it in Settings, API key \
             section, then ask your question again. Listening and recording carry on."
        );
    }

    /// KOB-362 : une surcharge (529, 5xx) a son Oups, sans le message brut
    /// de l'API ; une erreur inconnue garde son détail, suivi d'un point.
    #[test]
    fn surcharge_et_ponctuation_de_l_oups() {
        use kobi_i18n::Langue;
        for m in [
            "erreur d'API : HTTP 529 : Overloaded",
            "erreur d'API : overloaded_error : Overloaded",
            "erreur d'API : HTTP 503 : Service Unavailable",
        ] {
            assert_eq!(classer_message(m), Panne::Surcharge, "{m}");
            let (titre, message) = pour_l_utilisateur(m);
            assert_eq!(titre, "Claude est surchargé");
            assert!(
                !message.contains("Overloaded") && !message.contains("HTTP"),
                "{message}"
            );
        }
        assert_eq!(
            classer(&BrainError::Api("HTTP 529 : Overloaded".into())),
            Panne::Surcharge
        );
        let (_, message) = pour_l_utilisateur("erreur d'API : HTTP 400 : prompt is too long");
        assert_eq!(
            message,
            "erreur d'API : HTTP 400 : prompt is too long. L'écoute et l'enregistrement continuent."
        );
        let (_, message) = pour_l_utilisateur("erreur d'API : Invalid request.");
        assert_eq!(
            message,
            "erreur d'API : Invalid request. L'écoute et l'enregistrement continuent."
        );
        let suite = kobi_i18n::t(Langue::En, "coque.pannes.continue", &[]);
        assert_eq!(
            kobi_i18n::t(
                Langue::En,
                "coque.pannes.surchargeMessage",
                &[("suite", &suite)]
            ),
            "Anthropic's servers are very busy: ask your question again in a moment. \
             Listening and recording carry on."
        );
    }

    /// KOB-369 : la limite par minute de la clé API passe d'elle-même
    /// (surcharge passagère) ; « 429 » au milieu d'un nombre ne dit rien ;
    /// la limite d'usage de Claude Code reste reconnue.
    #[test]
    fn limite_par_minute_et_429_dans_un_nombre() {
        let api = |m: &str| BrainError::Api(m.into());
        assert_eq!(
            classer(&api(
                "HTTP 429 : Number of request tokens has exceeded your per-minute rate limit"
            )),
            Panne::Surcharge
        );
        assert_eq!(
            classer(&api(
                "rate_limit_error : This request would exceed the rate limit"
            )),
            Panne::Surcharge
        );
        let (titre, message) = pour_l_utilisateur(
            "erreur d'API : HTTP 400 : prompt is too long: 204291 tokens > 200000 maximum",
        );
        assert_eq!(titre, "Claude n'a pas répondu");
        assert!(message.contains("204291 tokens"), "{message}");
        // Claude Code : limite d'usage du compte.
        assert_eq!(
            classer(&api(
                r#"API Error: 429 {"type":"error","error":{"type":"rate_limit_error"}}"#
            )),
            Panne::LimiteUsage
        );
        assert_eq!(
            classer(&api("Claude AI usage limit reached|1792339200")),
            Panne::LimiteUsage
        );
        assert!(code_429("error 429"));
        assert!(!code_429("204291 tokens"));
        assert!(!code_429("14290"));
    }

    /// KOB-210 : l'Oups en anglais, mêmes variables qu'en français.
    #[test]
    fn oups_en_anglais() {
        use kobi_i18n::Langue;
        let suite = kobi_i18n::t(Langue::En, "coque.pannes.continue", &[]);
        assert_eq!(suite, "Listening and recording carry on.");
        assert_eq!(
            kobi_i18n::t(
                Langue::En,
                "coque.pannes.lenteurMessage",
                &[("duree", "5 minutes"), ("suite", &suite)]
            ),
            "Nothing for 5 minutes: I'm dropping this question. Ask it again in a moment. \
             Listening and recording carry on."
        );
        assert_eq!(
            kobi_i18n::t(Langue::En, "coque.pannes.secondes", &[("n", "1")]),
            "1 second"
        );
    }
}
