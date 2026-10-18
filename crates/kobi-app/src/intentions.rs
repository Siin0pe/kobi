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
    /// « Ouvre le carnet » (KOB-140).
    OuvrirCarnet,
    /// « Répète » : la dernière réponse, sans Claude.
    Repeter,
    /// « Plus court », « développe », « en anglais » : Claude reprend sa
    /// dernière réponse.
    PlusCourt,
    Developpe,
    EnAnglais,
    /// « Où en est… ? » : avancement des tâches longues.
    OuEnEst,
    /// « Annule » : arrête la dernière tâche longue.
    Annuler,
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

const CARNET: &[&str] = &[
    " ouvre le carnet ",
    " ouvre mon carnet ",
    " ouvre ton carnet ",
    " montre le carnet ",
    " montre moi le carnet ",
    " affiche le carnet ",
    " open the notebook ",
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

/// Formules retirées avant de comparer aux ordres brefs.
const POLITESSE: &[&str] = &[" s il te plait ", " s il vous plait ", " stp ", " please "];

/// Ordres brefs reconnus tels quels (après la politesse) : reprise de la
/// dernière réponse et suivi des tâches longues. Une phrase plus longue
/// (« annule ma réunion de demain », « où en est le projet ? ») va à Claude.
const BREFS: &[(&str, Intention)] = &[
    ("repete", Intention::Repeter),
    ("repete ca", Intention::Repeter),
    ("tu peux repeter", Intention::Repeter),
    ("peux tu repeter", Intention::Repeter),
    ("redis", Intention::Repeter),
    ("redis le", Intention::Repeter),
    ("redis moi", Intention::Repeter),
    ("repeat", Intention::Repeter),
    ("say that again", Intention::Repeter),
    ("plus court", Intention::PlusCourt),
    ("plus courte", Intention::PlusCourt),
    ("en plus court", Intention::PlusCourt),
    ("fais plus court", Intention::PlusCourt),
    ("version courte", Intention::PlusCourt),
    ("shorter", Intention::PlusCourt),
    ("developpe", Intention::Developpe),
    ("developpe un peu", Intention::Developpe),
    ("developpe plus", Intention::Developpe),
    ("detaille", Intention::Developpe),
    ("plus de details", Intention::Developpe),
    ("en detail", Intention::Developpe),
    ("tell me more", Intention::Developpe),
    ("en anglais", Intention::EnAnglais),
    ("dis le en anglais", Intention::EnAnglais),
    ("redis le en anglais", Intention::EnAnglais),
    ("traduis en anglais", Intention::EnAnglais),
    ("in english", Intention::EnAnglais),
    ("ou en est", Intention::OuEnEst),
    ("ou en es tu", Intention::OuEnEst),
    ("tu en es ou", Intention::OuEnEst),
    ("t en es ou", Intention::OuEnEst),
    ("ou ca en est", Intention::OuEnEst),
    ("ca avance", Intention::OuEnEst),
    ("alors ca avance", Intention::OuEnEst),
    ("ou en est la tache", Intention::OuEnEst),
    ("ou en est ta tache", Intention::OuEnEst),
    ("ou en est la recherche", Intention::OuEnEst),
    ("ou en est ta recherche", Intention::OuEnEst),
    ("annule", Intention::Annuler),
    ("annule ca", Intention::Annuler),
    ("annule la tache", Intention::Annuler),
    ("annule la recherche", Intention::Annuler),
    ("cancel", Intention::Annuler),
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
    let mut bref = n.clone();
    for p in POLITESSE {
        bref = bref.replace(p, " ");
    }
    if let Some((_, i)) = BREFS.iter().find(|(b, _)| *b == bref.trim()) {
        return Some(*i);
    }
    let mots_bref: Vec<&str> = bref.split_whitespace().collect();
    // « Annule » en dernier mot d'un ordre bref : Nemotron écorche souvent ce
    // qui précède (« Covid annule » pour « Kobi, annule »).
    if mots_bref.len() <= 3 && mots_bref.last() == Some(&"annule") {
        return Some(Intention::Annuler);
    }
    // « Où en est la tâche ? » mal transcrit (« où on est la tâche ») : la
    // tâche ou la recherche, et « où » ou « avance », en phrase courte.
    if mots_bref.len() <= 6
        && mots_bref.iter().any(|m| *m == "tache" || *m == "recherche")
        && mots_bref
            .iter()
            .any(|m| *m == "ou" || m.starts_with("avance"))
    {
        return Some(Intention::OuEnEst);
    }
    let contient = |motifs: &[&str]| motifs.iter().any(|m| n.contains(m));
    // Terminer avant démarrer : « arrête d'enregistrer » contient « enregistre ».
    if contient(TERMINER) || (mots <= 5 && contient(FINI)) {
        Some(Intention::TerminerReunion)
    } else if mots <= 3 && contient(STOP) {
        Some(Intention::Interrompre)
    } else if mots <= 8 && contient(CARNET) {
        Some(Intention::OuvrirCarnet)
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
            ("ouvre le carnet", OuvrirCarnet),
            ("Montre-moi le carnet s'il te plaît.", OuvrirCarnet),
            ("c'est la fin de la réunion, merci", TerminerReunion),
            ("c'est fini", TerminerReunion),
            ("bon, on a fini", TerminerReunion),
            ("Répète.", Repeter),
            ("tu peux répéter s'il te plaît ?", Repeter),
            ("redis-le", Repeter),
            ("plus court", PlusCourt),
            ("Développe.", Developpe),
            ("développe un peu", Developpe),
            ("dis-le en anglais", EnAnglais),
            ("en anglais", EnAnglais),
            ("où en est la tâche ?", OuEnEst),
            ("t'en es où ?", OuEnEst),
            ("ça avance ?", OuEnEst),
            ("annule", Annuler),
            ("annule la recherche", Annuler),
            ("Covid annule", Annuler),
            ("où on est la tâche", OuEnEst),
            ("la recherche avance ?", OuEnEst),
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
            "qu'est-ce que tu ranges dans le carnet de la réunion de lundi ?",
            "développe une application mobile",
            "répète après moi : bonjour",
            "annule ma réunion de demain",
            "où en est le projet Kobi ?",
            "explique-moi ça en anglais et en espagnol",
            "",
        ] {
            assert_eq!(intention(texte), None, "{texte}");
        }
    }
}
