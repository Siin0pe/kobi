//! Intentions vocales locales (KOB-134) : démarrer, mettre en pause,
//! reprendre et terminer la réunion se reconnaissent dans la question, sans
//! Claude, pour agir en moins d'une seconde.
//!
//! L'enregistrement ne démarre que sur une demande explicite (« Kobi,
//! enregistre la réunion », « Kobi, assiste-nous pour la réunion ») : jamais
//! tout seul, jamais par le raccourci.

/// Commande reconnue dans une question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intention {
    DemarrerReunion,
    Pause,
    Reprendre,
    TerminerReunion,
    /// « Stop », « attends », « tais-toi » : coupe la réponse en cours.
    Interrompre,
}

/// Question en minuscules, sans accents ni ponctuation, mots séparés par une
/// espace, avec une espace de chaque côté (pour chercher des mots entiers).
fn normaliser(texte: &str) -> String {
    let mut out = String::from(" ");
    for c in texte.chars().flat_map(char::to_lowercase) {
        let c = match c {
            'à' | 'â' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' => 'i',
            'ô' | 'ö' => 'o',
            'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            c if c.is_alphanumeric() => c,
            _ => ' ',
        };
        if c != ' ' || !out.ends_with(' ') {
            out.push(c);
        }
    }
    if !out.ends_with(' ') {
        out.push(' ');
    }
    out
}

const DEMARRER: &[&str] = &[
    " assiste nous ",
    " assiste moi pour la reunion ",
    " enregistre la reunion ",
    " enregistre notre reunion ",
    " enregistre cette reunion ",
    " demarre la reunion ",
    " demarre une reunion ",
    " demarre l enregistrement ",
    " commence la reunion ",
    " commence l enregistrement ",
    " lance la reunion ",
    " lance l enregistrement ",
    " prends des notes de la reunion ",
    " start the meeting ",
    " record the meeting ",
];
/// « Kobi, enregistre » tout court : seulement en ordre bref.
const ENREGISTRE: &[&str] = &[" enregistre ", " enregistre nous ", " record "];
const TERMINER: &[&str] = &[
    " termine la reunion ",
    " fin de la reunion ",
    " la reunion est terminee ",
    " la reunion est finie ",
    " arrete la reunion ",
    " arrete l enregistrement ",
    " arrete d enregistrer ",
    " stop the meeting ",
    " end the meeting ",
];
/// « C'est fini », « on a fini » : seulement en ordre bref.
const FINI: &[&str] = &[
    " c est fini ",
    " c est termine ",
    " on a fini ",
    " on a termine ",
];
const PAUSE: &[&str] = &[
    " pause ",
    " mets en pause ",
    " met en pause ",
    " mets la reunion en pause ",
];
const REPRENDRE: &[&str] = &[
    " reprends ",
    " reprend ",
    " reprends l enregistrement ",
    " reprends la reunion ",
];

/// Arrêt de la réponse : seulement en ordre très bref.
const STOP: &[&str] = &[
    " stop ",
    " attends ",
    " attend ",
    " tais toi ",
    " chut ",
    " arrete ",
    " arrete toi ",
    " silence ",
];

/// Mots qui font d'une phrase une vraie question, pas un ordre
/// (« comment démarrer la réunion ? »).
const QUESTIONS: &[&str] = &[
    " comment ",
    " pourquoi ",
    " quand ",
    " est ce que ",
    " how ",
    " why ",
];

/// Intention de la question, s'il y en a une. Les ordres sont courts : au
/// delà de 12 mots, c'est une question pour Claude.
pub fn intention(question: &str) -> Option<Intention> {
    let n = normaliser(question);
    let mots = n.split_whitespace().count();
    if mots > 12 || QUESTIONS.iter().any(|q| n.contains(q)) {
        return None;
    }
    let contient = |motifs: &[&str]| motifs.iter().any(|m| n.contains(m));
    // Terminer avant démarrer : « arrête d'enregistrer » contient « enregistre ».
    if contient(TERMINER) || (mots <= 5 && contient(FINI)) {
        Some(Intention::TerminerReunion)
    } else if mots <= 3 && contient(STOP) {
        Some(Intention::Interrompre)
    } else if contient(DEMARRER) || (mots <= 2 && contient(ENREGISTRE)) {
        Some(Intention::DemarrerReunion)
    // « Pause » et « reprends » seuls sont des mots courants : seulement
    // dans un ordre bref (« on fait une pause café ? » reste une question).
    } else if mots <= 5 && contient(REPRENDRE) {
        Some(Intention::Reprendre)
    } else if mots <= 5 && contient(PAUSE) {
        Some(Intention::Pause)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Intention::*;

    #[test]
    fn ordres_reconnus() {
        for (texte, attendu) in [
            ("assiste-nous pour la réunion", DemarrerReunion),
            ("Assiste-nous pour la réunion.", DemarrerReunion),
            ("enregistre", DemarrerReunion),
            ("enregistre la réunion s'il te plaît", DemarrerReunion),
            ("démarre la réunion", DemarrerReunion),
            ("prends des notes de la réunion", DemarrerReunion),
            ("enregistre nous", DemarrerReunion),
            ("start the meeting", DemarrerReunion),
            ("mets en pause", Pause),
            ("pause", Pause),
            ("reprends", Reprendre),
            ("reprends l'enregistrement", Reprendre),
            ("termine la réunion", TerminerReunion),
            ("arrête l'enregistrement", TerminerReunion),
            ("arrête d'enregistrer", TerminerReunion),
            ("stop", Interrompre),
            ("Attends !", Interrompre),
            ("tais-toi", Interrompre),
            ("arrête", Interrompre),
            ("c'est la fin de la réunion, merci", TerminerReunion),
            ("c'est fini", TerminerReunion),
            ("bon, on a fini", TerminerReunion),
        ] {
            assert_eq!(intention(texte), Some(attendu), "{texte}");
        }
    }

    #[test]
    fn questions_pour_claude() {
        for texte in [
            "quelle heure est-il ?",
            "comment démarrer la réunion ?",
            "est-ce que tu peux enregistrer ?",
            "résume ce qu'on vient de dire",
            "qui a parlé du budget et de la pause café pendant la réunion de ce matin ?",
            "combien font douze fois sept",
            "enregistre ça dans mes notes",
            "prends des notes sur ce ticket",
            "on fait une pause café dans dix minutes ?",
            "reprends l'explication du budget depuis le début",
            "",
        ] {
            assert_eq!(intention(texte), None, "{texte}");
        }
    }
}
