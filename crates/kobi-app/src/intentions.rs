//! Intentions vocales locales (KOB-134) : démarrer, mettre en pause,
//! reprendre et terminer la réunion se reconnaissent dans la question, sans
//! Claude, pour agir en moins d'une seconde.
//!
//! L'enregistrement ne démarre que sur une demande explicite (« Kobi,
//! enregistre la réunion », « Kobi, assiste-nous pour la réunion ») : jamais
//! tout seul, jamais par le raccourci.
//!
//! Ce n'est qu'un chemin rapide pour les formules évidentes (KOB-165) : toute
//! autre formulation part à Claude, qui pilote la réunion avec les outils
//! `meeting_*` du MCP de Kobi d'après la question et la transcription
//! récente. Pas de liste de phrases à faire grossir ici.

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
    /// « Montre en grand » : la vue agrandie de la conversation (KOB-169).
    MontrerEnGrand,
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
/// Ordres d'arrêt sans ambiguïté. « Fin de la réunion », « c'est fini »,
/// « la réunion est finie » sont aussi des mentions (« on en reparle à la fin
/// de la réunion ») : Claude en juge (KOB-165).
const TERMINER: &[&str] = &[
    " termine la reunion ",
    " arrete la reunion ",
    " arrete l enregistrement ",
    " arrete d enregistrer ",
    " stop l enregistrement ",
    " stoppe l enregistrement ",
    " coupe l enregistrement ",
    " termine l enregistrement ",
    " mets fin a l enregistrement ",
    " mets fin a la reunion ",
    " stoppe la reunion ",
    " stop la reunion ",
    " stop the meeting ",
    " end the meeting ",
    " stop recording ",
    " stop the recording ",
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

/// Vue agrandie de la conversation (KOB-169) : un verbe pour montrer, et
/// « en grand » (ou « agrandis »).
const MONTRER: &[&str] = &[
    " montre ",
    " montre moi ",
    " affiche ",
    " affiche moi ",
    " ouvre ",
    " mets ",
    " show ",
];
const GRAND: &[&str] = &[
    " en grand ",
    " en plus grand ",
    " plus gros ",
    " en gros ",
    " bigger ",
    " en grand ecran ",
];
const AGRANDIS: &[&str] = &[" agrandis ", " agrandit ", " agrandis la ", " zoome "];

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

/// Réponse à une carte MCP (KOB-336) : seulement les mots de décision nets
/// (« oui », « non », « valide », « refuse »). « Stop » ou « annule » font
/// taire Kobi ou annulent la fin, pas la carte ; « ok » ou « d'accord » se
/// disent trop souvent à autre chose pour valider une écriture.
pub fn reponse_carte_mcp(texte: &str) -> Option<bool> {
    const NETS: &[&str] = &["oui", "non", "valide", "refuse", "yes", "no"];
    let reponse = reponse_validation(texte)?;
    let n = normaliser(texte);
    let premier = n.split_whitespace().find(|m| *m != "kobi")?;
    NETS.contains(&premier).then_some(reponse)
}

/// Réponse à une carte de validation, tant qu'elle attend (KOB-156) :
/// `Some(true)` pour « oui », `Some(false)` pour « non ». La phrase entière
/// doit être la réponse : « oui mais change le titre » n'en est pas une.
pub fn reponse_validation(texte: &str) -> Option<bool> {
    const OUI: &[&str] = &[
        "oui",
        "oui vas y",
        "vas y",
        "valide",
        "ok",
        "okay",
        "d accord",
        "oui valide",
        "c est bon",
        "oui c est bon",
        "fais le",
        "oui fais le",
        "go",
        "yes",
        "oui merci",
    ];
    const NON: &[&str] = &[
        "non",
        "non laisse tomber",
        "non annule",
        "non merci",
        "annule",
        "attends",
        "refuse",
        "pas maintenant",
        "laisse tomber",
        "non attends",
        "stop",
        "no",
    ];
    let mut n = normaliser(texte);
    for p in [" kobi ", " s il te plait "] {
        n = n.replace(p, " ");
    }
    let n = n.trim();
    // La transcription ajoute parfois un mot parasite à la fin (« oui vas y
    // grec ») : on le tolère, sauf s'il nuance la réponse.
    let court = match n.rsplit_once(' ') {
        Some((debut, fin)) if !["mais", "pas", "sauf", "si"].contains(&fin) => debut,
        _ => n,
    };
    if OUI.contains(&n) || OUI.contains(&court) {
        Some(true)
    } else if NON.contains(&n) || NON.contains(&court) {
        Some(false)
    } else {
        None
    }
}

/// Juste après un arrêt de la réunion (KOB-165), la phrase demande-t-elle
/// de reprendre ? « Reprends », « non, continue », « c'était une erreur »,
/// en phrase courte.
pub fn annule_la_fin(texte: &str) -> bool {
    let n = normaliser(texte);
    n.split_whitespace().count() <= 8
        && [
            " reprends ",
            " reprend ",
            " reprendre ",
            " continue ",
            " continuons ",
            " annule ",
            " c etait une erreur ",
            " c est une erreur ",
            " pas fini ",
            " resume ",
            " undo ",
        ]
        .iter()
        .any(|m| n.contains(m))
}

/// Fins de réunion dites en entier (KOB-203) : « la réunion est terminée »,
/// « tu peux arrêter de noter ». La phrase entière doit être l'ordre, avec au
/// plus « bon », « ok »… devant et « merci » derrière. En mention (« on en
/// reparle à la fin de la réunion »), elle va à Claude. Le chef ne les prend
/// que pendant une réunion.
const FIN_DITE: &[&str] = &[
    "la reunion est terminee",
    "la reunion est finie",
    "la reunion est close",
    "fin de la reunion",
    "fin de reunion",
    "c est la fin de la reunion",
    "c est fini",
    "c est termine",
    "on a fini",
    "on a termine",
    "on a fini la reunion",
    "on a termine la reunion",
    "la reunion est finie on a fini",
    "on s arrete la",
    "on arrete la",
    "on peut arreter la",
    "on peut s arreter la",
    "arrete de noter",
    "tu peux arreter de noter",
    "tu peux arreter la reunion",
    "arrete de prendre des notes",
    "tu peux arreter de prendre des notes",
    "plus besoin de noter",
    "c est bon tu peux arreter",
];
const AVANT_FIN: &[&str] = &[
    "bon", "ok", "okay", "allez", "alors", "donc", "bien", "voila", "parfait", "ben", "eh", "kobi",
];
const APRES_FIN: &[&str] = &[
    "merci",
    "a tous",
    "beaucoup",
    "kobi",
    "pour aujourd hui",
    "c est bon",
];

/// La phrase entière demande la fin de la réunion (voir [`FIN_DITE`]).
pub fn fin_de_reunion_dite(question: &str) -> bool {
    let n = normaliser(question);
    let mut q = n.trim().to_owned();
    for p in POLITESSE {
        q = format!(" {q} ").replace(p, " ").trim().to_owned();
    }
    loop {
        let avant = q.clone();
        for m in AVANT_FIN {
            if let Some(reste) = q.strip_prefix(&format!("{m} ")) {
                q = reste.trim().to_owned();
            }
        }
        for m in APRES_FIN {
            if let Some(reste) = q.strip_suffix(&format!(" {m}")) {
                q = reste.trim().to_owned();
            }
        }
        if q == avant {
            break;
        }
    }
    FIN_DITE.contains(&q.as_str())
}

/// « Kobi, appelle cette réunion Budget 2027 » (KOB-223) : le titre donné à
/// la main, tel qu'il a été dit. `None` si la phrase n'en donne pas.
pub fn titre_de_reunion_dit(question: &str) -> Option<String> {
    const DEBUTS: &[&str] = &[
        "appelle cette reunion",
        "appelle la reunion",
        "nomme cette reunion",
        "nomme la reunion",
        "renomme cette reunion en",
        "renomme cette reunion",
        "renomme la reunion en",
        "renomme la reunion",
        "intitule cette reunion",
        "intitule la reunion",
        "le titre de la reunion c est",
        "le titre de la reunion est",
        "titre de la reunion",
        "call this meeting",
        "name this meeting",
        "rename this meeting to",
        "rename this meeting",
    ];
    // Même nombre de caractères que l'original : le titre se recopie tel quel.
    let sans_accents: String = question
        .chars()
        .map(|c| {
            let c = c.to_lowercase().next().unwrap_or(c);
            match c {
                'à' | 'â' | 'ä' => 'a',
                'é' | 'è' | 'ê' | 'ë' => 'e',
                'î' | 'ï' => 'i',
                'ô' | 'ö' => 'o',
                'ù' | 'û' | 'ü' => 'u',
                'ç' => 'c',
                '\'' | '’' | '-' | ',' => ' ',
                c => c,
            }
        })
        .collect();
    let mut debut = 0;
    let caracteres: Vec<char> = sans_accents.chars().collect();
    let mots_depuis = |i: usize| -> String { caracteres[i..].iter().collect() };
    // « Kobi » en tête (tapé au clavier) ne compte pas.
    while debut < caracteres.len() && !caracteres[debut].is_alphanumeric() {
        debut += 1;
    }
    if mots_depuis(debut).starts_with("kobi ") {
        debut += 5;
    }
    while debut < caracteres.len() && !caracteres[debut].is_alphanumeric() {
        debut += 1;
    }
    let reste = mots_depuis(debut);
    let reste_norm = reste.split_whitespace().collect::<Vec<_>>().join(" ");
    let motif = DEBUTS
        .iter()
        .find(|d| reste_norm.starts_with(&format!("{d} ")))?;
    // Saute autant de mots que le motif en compte, dans l'original.
    let a_sauter = motif.split(' ').count();
    let original: Vec<char> = question.chars().collect();
    let mut i = debut;
    for _ in 0..a_sauter {
        while i < caracteres.len() && caracteres[i].is_whitespace() {
            i += 1;
        }
        while i < caracteres.len() && !caracteres[i].is_whitespace() {
            i += 1;
        }
    }
    let titre: String = original[i.min(original.len())..].iter().collect();
    let titre = titre
        .trim()
        .trim_start_matches([':', '«', '"', ' '])
        .trim_end_matches(['»', '"', '.', '!', ' '])
        .trim();
    let mots = titre.split_whitespace().count();
    (1..=12).contains(&mots).then(|| titre.to_owned())
}

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
    if contient(TERMINER) {
        Some(Intention::TerminerReunion)
    } else if mots <= 3 && contient(STOP) {
        Some(Intention::Interrompre)
    } else if mots <= 8 && contient(CARNET) {
        Some(Intention::OuvrirCarnet)
    } else if mots <= 7 && ((contient(MONTRER) && contient(GRAND)) || contient(AGRANDIS)) {
        Some(Intention::MontrerEnGrand)
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

    /// KOB-223 : « appelle cette réunion … » donne le titre tel qu'il est dit.
    #[test]
    fn titres_de_reunion_dits() {
        for (texte, titre) in [
            (
                "Kobi, appelle cette réunion Budget du séminaire",
                "Budget du séminaire",
            ),
            ("appelle la réunion « Point équipe ».", "Point équipe"),
            ("renomme la réunion en Rétro du sprint", "Rétro du sprint"),
            (
                "Kobi, le titre de la réunion c'est lancement de la fusée",
                "lancement de la fusée",
            ),
            (
                "nomme cette réunion : Recrutement mobile",
                "Recrutement mobile",
            ),
            ("call this meeting Q3 planning", "Q3 planning"),
        ] {
            assert_eq!(
                titre_de_reunion_dit(texte).as_deref(),
                Some(titre),
                "{texte}"
            );
        }
        for texte in [
            "comment appelle-t-on cette réunion ?",
            "appelle Paul après la réunion",
            "appelle cette réunion",
            "la réunion est terminée",
            "",
        ] {
            assert_eq!(titre_de_reunion_dit(texte), None, "{texte}");
        }
    }

    /// KOB-203 : les fins de réunion dites en entier sont des ordres ; une
    /// mention dans une phrase plus longue n'en est pas un.
    #[test]
    fn fins_de_reunion_dites() {
        for texte in [
            "Kobi, la réunion est terminée.",
            "la réunion est terminée, merci",
            "Bon, la réunion est finie.",
            "Kobi, tu peux arrêter de noter.",
            "tu peux arrêter de noter s'il te plaît",
            "Kobi, fin de la réunion.",
            "fin de réunion",
            "ok c'est fini, merci à tous",
            "c'est terminé",
            "on a fini",
            "bon, on s'arrête là",
            "arrête de prendre des notes",
            "c'est la fin de la réunion",
            "allez, on a fini la réunion, merci beaucoup",
        ] {
            assert!(fin_de_reunion_dite(texte), "{texte}");
        }
        for texte in [
            "on en reparle à la fin de la réunion",
            "la réunion est terminée à quelle heure ?",
            "est-ce que la réunion est finie ?",
            "quand est-ce que la réunion est terminée",
            "c'est fini pour le budget, on passe au planning",
            "on a fini le prototype hier",
            "Marc dit que la réunion est terminée pour lui",
            "tu peux arrêter de parler",
            "note que la réunion est terminée à midi",
            "on arrête le projet ?",
            "",
        ] {
            assert!(!fin_de_reunion_dite(texte), "{texte}");
        }
    }
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
            ("stop l'enregistrement", TerminerReunion),
            ("Kobi, stop l'enregistrement", TerminerReunion),
            ("coupe l'enregistrement s'il te plaît", TerminerReunion),
            ("mets fin à la réunion", TerminerReunion),
            ("stop recording", TerminerReunion),
            ("Kobi, stoppe la réunion.", TerminerReunion),
            ("stop", Interrompre),
            ("Attends !", Interrompre),
            ("tais-toi", Interrompre),
            ("arrête", Interrompre),
            ("ouvre le carnet", OuvrirCarnet),
            ("Montre-moi le carnet s'il te plaît.", OuvrirCarnet),
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
            ("montre en grand", MontrerEnGrand),
            ("Montre-moi ça en grand.", MontrerEnGrand),
            ("affiche la réponse en plus grand", MontrerEnGrand),
            ("agrandis", MontrerEnGrand),
            ("show it bigger", MontrerEnGrand),
        ] {
            assert_eq!(intention(texte), Some(attendu), "{texte}");
        }
    }

    #[test]
    fn reponses_a_une_carte() {
        for t in [
            "Oui",
            "oui, vas-y",
            "Vas-y !",
            "Valide.",
            "OK",
            "d'accord",
            "Kobi, oui",
            "Oui, vas y grec",
        ] {
            assert_eq!(reponse_validation(t), Some(true), "{t}");
        }
        for t in [
            "Non",
            "non merci",
            "Annule",
            "Attends !",
            "pas maintenant",
            "Non, laisse tomber",
        ] {
            assert_eq!(reponse_validation(t), Some(false), "{t}");
        }
        for t in [
            "oui mais change le titre",
            "oui mais",
            "non pas",
            "non, c'est le dossier site",
            "quelle heure est-il ?",
            "",
        ] {
            assert_eq!(reponse_validation(t), None, "{t}");
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
            // Mentions ou formules ambiguës : Claude en juge (KOB-165).
            "on en reparle à la fin de la réunion",
            "c'est la fin de la réunion, merci",
            "c'est fini pour le point démo",
            "bon, on a fini",
            "la réunion est finie",
            "montre-moi comment on voit en grand les étoiles avec un télescope",
            "quel est le plus grand pays du monde",
            "",
        ] {
            assert_eq!(intention(texte), None, "{texte}");
        }
    }

    #[test]
    fn reprise_apres_un_arret() {
        for texte in [
            "reprends",
            "Ah non, reprends !",
            "non continue",
            "c'était une erreur",
            "annule, on n'a pas fini",
            "reprends l'enregistrement",
        ] {
            assert!(annule_la_fin(texte), "{texte}");
        }
        for texte in [
            "merci à tous, bonne soirée",
            "on se voit jeudi",
            "non",
            "il reste des croissants si vous voulez, servez-vous avant de partir",
        ] {
            assert!(!annule_la_fin(texte), "{texte}");
        }
    }

    /// KOB-336 : sur une carte MCP, seuls les mots de décision nets comptent.
    #[test]
    fn reponse_nette_pour_une_carte_mcp() {
        for (texte, attendu) in [
            ("Oui", Some(true)),
            ("Kobi, oui vas-y.", Some(true)),
            ("valide", Some(true)),
            ("Non merci", Some(false)),
            ("refuse", Some(false)),
            ("ok", None),
            ("d'accord", None),
            ("stop", None),
            ("annule", None),
            ("oui mais change le titre", None),
        ] {
            assert_eq!(reponse_carte_mcp(texte), attendu, "{texte}");
        }
    }
}
