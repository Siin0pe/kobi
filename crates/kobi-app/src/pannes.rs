//! Pannes du cerveau (KOB-68) : ce qui peut arriver à Claude Code pendant
//! un tour, et ce qu'on en dit à l'utilisateur. Table « Erreurs et cas
//! limites » du document « parcours utilisateur et règles d'UX » :
//!
//! | Panne | Réaction |
//! | -- | -- |
//! | Claude Code plante | session relancée, question renvoyée une fois |
//! | rien pendant longtemps | « je cherche encore » à 8 s, abandon à 45 s sans activité |
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
    /// Aucune activité trop longtemps.
    Lenteur,
    /// Limite d'usage du compte atteinte.
    LimiteUsage,
    /// Claude Code installé mais pas connecté.
    Deconnecte,
    /// Claude Code introuvable ou qui ne répond pas.
    Absent,
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
];

/// Signes d'une limite d'usage.
const LIMITE: &[&str] = &[
    "usage limit",
    "rate limit",
    "limit reached",
    "rate_limit",
    "too many requests",
    "429",
    "quota",
];

/// Classe un message d'erreur (texte d'une [`BrainError`] ou d'un résultat).
pub fn classer_message(message: &str) -> Panne {
    let m = message.to_lowercase();
    let contient = |mots: &[&str]| mots.iter().any(|mot| m.contains(mot));
    if contient(LIMITE) {
        Panne::LimiteUsage
    } else if contient(RESEAU) {
        Panne::HorsLigne
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
            Panne::Deconnecte => Panne::Deconnecte,
            Panne::HorsLigne => Panne::HorsLigne,
            _ => Panne::Absent,
        },
        BrainError::Api(m) => match classer_message(m) {
            p @ (Panne::LimiteUsage | Panne::HorsLigne) => p,
            _ => Panne::Autre,
        },
        BrainError::Busy => Panne::Autre,
    }
}

/// Phrase de patience, quand un tour tarde (8 s).
pub const PATIENCE: &str = "Je cherche encore…";

/// Annonce d'une session relancée après un plantage.
pub const RELANCE: &str = "Claude s'est arrêté, je le relance et je repose ta question.";

/// Titre et message de l'Oups : la cause en une phrase, puis la marche à
/// suivre. L'écoute et l'enregistrement continuent, on le rappelle.
pub fn expliquer(panne: &Panne, detail: &str) -> (&'static str, String) {
    let continue_ = "L'écoute et l'enregistrement continuent.";
    match panne {
        Panne::LimiteUsage => (
            "Limite d'usage atteinte",
            format!(
                "Ton compte Claude a atteint sa limite d'usage{}. Réessaie plus tard, ou passe à une offre supérieure. {continue_}",
                reprise(detail).map(|h| format!(" jusqu'à {h}")).unwrap_or_default()
            ),
        ),
        Panne::Deconnecte => (
            "Claude Code n'est pas connecté",
            format!("Ouvre un terminal et lance `claude auth login`, puis repose ta question. {continue_}"),
        ),
        Panne::Absent => (
            "Claude Code est introuvable",
            format!("Installe Claude Code (claude.ai/code) ou vérifie qu'il démarre dans un terminal, puis repose ta question. {continue_}"),
        ),
        Panne::HorsLigne => (
            "Pas d'Internet",
            format!("Je garde ta question et je la pose dès que la connexion revient. {continue_}"),
        ),
        Panne::Lenteur => (
            "Claude ne répond pas",
            format!("Rien depuis 45 secondes : j'abandonne cette question. Repose-la dans un instant. {continue_}"),
        ),
        Panne::Plantage => (
            "Claude s'est arrêté",
            format!("Il s'est arrêté deux fois de suite sur cette question. Repose-la dans un instant. {continue_}"),
        ),
        Panne::Autre => ("Claude n'a pas répondu", format!("{detail} {continue_}")),
    }
}

/// Heure de reprise d'une limite d'usage, si Claude Code la donne
/// (« …limit reached|1792339200 », horodatage Unix en secondes).
fn reprise(detail: &str) -> Option<String> {
    let secondes: i64 = detail.rsplit('|').next()?.trim().parse().ok()?;
    let t = jiff::Timestamp::from_second(secondes).ok()?;
    let local = t.to_zoned(jiff::tz::TimeZone::system());
    Some(format!("{:02} h {:02}", local.hour(), local.minute()))
}

/// Le message vu par l'utilisateur pour une erreur de tour.
pub fn pour_l_utilisateur(message: &str) -> (&'static str, String) {
    expliquer(&classer_message(message), message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

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
}
