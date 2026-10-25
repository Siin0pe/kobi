//! Repérage de « Kobi » dans la transcription continue (KOB-132).
//!
//! Le micro est transcrit en permanence par Nemotron ; [`reperer`] cherche le
//! nom de Kobi (et ses variantes de reconnaissance) dans chaque segment, et
//! [`Ecouteur`] en déduit quoi faire : passer en écoute dès le premier
//! segment partiel qui contient le nom, afficher la question au fil de la
//! parole, l'envoyer à la fin de la phrase. La question est ce qui suit le
//! nom ; « Kobi. » seul attend la phrase suivante, comme le raccourci.
//!
//! Tout est pur : le temps est celui de l'audio (secondes), sans horloge ni
//! micro, pour se tester sur des transcriptions écrites.

/// Fins du nom produites par la reconnaissance (après « kob », « cob »…).
/// « ai » : « Kobi » prononcé à l'anglaise, « Ko-bye » (KOB-269).
const FINS: [&str; 8] = ["i", "y", "ie", "e", "ey", "is", "ye", "ai"];

/// Sans aucun mot après l'interpellation, l'écoute s'arrête.
pub const SANS_PAROLE: f64 = 6.0;
/// Longueur maximale d'une question à Kobi quand son nom n'ouvre pas la
/// phrase (KOB-237) : au-delà, c'est un flux de parole (conversation de
/// groupe, jeu, visio), pas une question.
pub const QUESTION_CARACTERES: usize = 200;
/// Même borne pour un appel net en tête (« Kobi, … ») : une question dictée
/// longuement passe, environ 70 mots.
pub const QUESTION_CARACTERES_APPEL: usize = 400;
/// Durée maximale d'une question : au-delà, ce qui a été dit part.
pub const QUESTION_MAX: f64 = 15.0;

/// Mot sans accents ni majuscules, pour la comparaison aux variantes.
fn normaliser(mot: &str) -> String {
    mot.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'à' | 'â' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' => 'i',
            'ô' | 'ö' => 'o',
            'ù' | 'û' | 'ü' => 'u',
            'ÿ' => 'y',
            c => c,
        })
        .collect()
}

/// Le mot est-il « Kobi » tel que Nemotron l'écrit ? Nom inconnu de son
/// vocabulaire, il sort en graphies variables : Kobi, Coby, Kobé, Coobby,
/// Koobi, Koubi, Gobi… Lettres doublées réduites, puis initiale k, c, q ou g,
/// « o » ou « ou », « b » et une fin de [`FINS`].
fn est_kobi(mot: &str) -> bool {
    let mut lettres: Vec<char> = normaliser(mot).chars().collect();
    lettres.dedup();
    let n: String = lettres.into_iter().collect();
    let Some(reste) = n.strip_prefix(['k', 'c', 'q', 'g']) else {
        return false;
    };
    let reste = reste.strip_prefix('h').unwrap_or(reste);
    let Some(reste) = reste.strip_prefix("ou").or_else(|| reste.strip_prefix('o')) else {
        return false;
    };
    reste
        .strip_prefix('b')
        .is_some_and(|fin| FINS.contains(&fin))
}

/// Position du premier « Kobi » du texte : `(début, fin)` en octets.
fn trouver(texte: &str) -> Option<(usize, usize)> {
    let mut debut = None;
    // Mot précédent, quand c'est « que » : le « K » de « Kobi » s'y colle
    // souvent (« dis-moi que obey », « alors que Oobi »).
    let mut que: Option<usize> = None;
    for (i, c) in texte.char_indices().chain([(texte.len(), ' ')]) {
        if c.is_alphanumeric() {
            debut.get_or_insert(i);
        } else if let Some(d) = debut.take() {
            let mot = &texte[d..i];
            if est_kobi(mot) {
                return Some((d, i));
            }
            if let Some(q) = que {
                if est_kobi(&format!("k{mot}")) {
                    return Some((q, i));
                }
            }
            que = matches!(normaliser(mot).as_str(), "que" | "qu").then_some(d);
        }
    }
    None
}

/// Autres graphies de « Kobi » sorties par Nemotron (KOB-179) : des mots
/// qui existent par ailleurs (« Kirby », « Cody »). Comptées seulement en
/// position d'appel ([`trouver_variante`]).
const ENTENDUS: &[&str] = &["kirby", "kirbi", "cody", "codi", "cogi"];

/// Petits mots que Nemotron colle devant le nom (« Etkobi », « Okeikobi »,
/// « Wicoby » pour « oui Kobi », « Makobi »).
const COLLES: &[&str] = &[
    "et", "ok", "oke", "okei", "okey", "okay", "hockey", "oui", "wi", "ma", "eh", "he", "hey", "e",
    "ve",
];

/// Graphie douteuse de « Kobi » (KOB-179) en position d'appel nette, en
/// tête de phrase (après des [`INTERJECTIONS`]) : un nom collé à un mot de
/// [`COLLES`], ou un mot de [`ENTENDUS`] suivi d'une virgule (« Kirby, tu
/// peux… »). Ailleurs (« Kirby est un jeu », « mon collègue Cody », en fin
/// de phrase), rien : en conversation de groupe, ces mots existent (KOB-237).
fn trouver_variante(texte: &str) -> Option<(usize, usize)> {
    let mut avant_seulement_interjections = true;
    let mut debut = None;
    let mut spans = Vec::new();
    for (i, c) in texte.char_indices().chain([(texte.len(), ' ')]) {
        if c.is_alphanumeric() {
            debut.get_or_insert(i);
        } else if let Some(d) = debut.take() {
            spans.push((d, i));
        }
    }
    for &(d, f) in &spans {
        let n = normaliser(&texte[d..f]);
        let entendu = ENTENDUS.contains(&n.as_str());
        let colle = COLLES.iter().any(|p| {
            n.strip_prefix(p)
                .is_some_and(|reste| reste.chars().count() >= 4 && est_kobi(reste))
        });
        // En tête seulement (KOB-237) ; un mot qui existe (« Kirby ») doit en
        // plus être suivi d'une virgule d'apostrophe.
        let vocatif = texte[f..].trim_start().starts_with(',');
        if avant_seulement_interjections && (colle || (entendu && vocatif)) {
            return Some((d, f));
        }
        avant_seulement_interjections &= INTERJECTIONS.contains(&n.as_str());
    }
    None
}

/// Retire la ponctuation et les espaces qui entourent un morceau de phrase,
/// et en tête les apostrophes et guillemets détachés que la reconnaissance
/// colle au nom (« Kobe,', what day… », « Kobai '' Tell me… », KOB-267). Un
/// guillemet collé au mot qui suit (« "Hello" en espagnol ») reste.
fn nettoyer(morceau: &str) -> &str {
    let ponctuation = |c: char| c.is_whitespace() || matches!(c, ',' | '.' | '!' | ';' | ':');
    let mut reste = morceau.trim_matches(ponctuation);
    loop {
        let sans = reste.trim_start_matches(['\'', '’', '"', '“', '”', '`']);
        if sans.len() == reste.len() || sans.starts_with(char::is_alphanumeric) {
            return reste;
        }
        reste = sans.trim_start_matches(ponctuation);
    }
}

/// La phrase qui porte le nom, quand il l'ouvre après une autre phrase du même
/// segment, et qu'une suite le suit : « … le mentionnera. Kobi, termine la
/// réunion. » → « Kobi, termine la réunion. ». Sous charge, ou quand on
/// enchaîne sans s'arrêter, Nemotron rend les deux phrases en un segment, et
/// le nom n'y était ni en tête ni en fin (KOB-276). Le nom seul en fin
/// (« Quelle heure est-il ? Kobi. ») garde la phrase d'avant pour question ;
/// une abréviation (« M. Kobi ») ne coupe pas.
fn phrase_du_nom(texte: &str) -> &str {
    let Some((debut, fin)) = trouver(texte).or_else(|| trouver_variante(texte)) else {
        return texte;
    };
    if nettoyer(&texte[fin..]).is_empty() {
        return texte;
    }
    let avant = &texte[..debut];
    let coupe = avant
        .char_indices()
        .filter(|&(i, c)| {
            matches!(c, '.' | '?' | '!' | '…')
                && avant[i + c.len_utf8()..].starts_with(char::is_whitespace)
                // Le mot qui finit la phrase d'avant : pas une initiale.
                && avant[..i]
                    .trim_end()
                    .split(|c: char| !c.is_alphanumeric())
                    .next_back()
                    .is_some_and(|m| m.chars().count() > 1)
        })
        .map(|(i, c)| i + c.len_utf8())
        .next_back();
    match coupe {
        Some(i) => texte[i..].trim_start(),
        None => texte,
    }
}

/// Mots au plus d'un ordre sur la réunion collé à la phrase d'avant
/// (KOB-276).
const ORDRE_COLLE_MOTS: usize = 6;

/// Ordre court sur la réunion (la terminer, la mettre en pause, la reprendre),
/// qui est tout ce que dit la suite du nom (KOB-276).
fn ordre_de_reunion(suite: &str) -> bool {
    use crate::intentions::{fin_de_reunion_dite, intention, Intention};
    mots(suite).len() <= ORDRE_COLLE_MOTS
        && (fin_de_reunion_dite(suite)
            || matches!(
                intention(suite),
                Some(Intention::TerminerReunion | Intention::Pause | Intention::Reprendre)
            ))
}

/// L'ordre court sur la réunion qui suit le nom et finit le segment, s'il y
/// en a un (KOB-276).
fn ordre_colle(texte: &str) -> Option<&str> {
    reperer(texte).filter(|suite| ordre_de_reunion(suite))
}

/// Ce qui suit « Kobi » dans le texte, ou `None` s'il n'y est pas.
pub fn reperer(texte: &str) -> Option<&str> {
    trouver(texte)
        .or_else(|| trouver_variante(texte))
        .map(|(_, fin)| nettoyer(&texte[fin..]))
}

/// Question d'une phrase dite après l'interpellation : un « Kobi » répété en
/// tête est retiré.
fn question(texte: &str) -> &str {
    match trouver(texte).or_else(|| trouver_variante(texte)) {
        Some((debut, fin)) if nettoyer(&texte[..debut]).is_empty() => nettoyer(&texte[fin..]),
        _ => nettoyer(texte),
    }
}

/// À qui s'adresse une phrase qui contient le nom (KOB-168).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Adresse {
    /// On parle à Kobi : « Kobi, … », « …, Kobi ? », « Est-ce que, Kobi, … ».
    Appel,
    /// On parle de Kobi à quelqu'un d'autre : « ce que Kobi dit », « avec Kobi ».
    Cite,
    /// Ni l'un ni l'autre à coup sûr.
    Doute,
}

/// Petits mots qui peuvent précéder l'appel sans en faire une citation.
const INTERJECTIONS: &[&str] = &[
    "euh", "eh", "he", "hey", "ho", "oh", "ah", "bah", "ben", "bon", "alors", "oui", "non", "ok",
    "okay", "dis", "dites", "moi", "allo", "salut", "bonjour", "bonsoir", "coucou", "tiens",
    "ecoute", "pardon", "hm", "hmm", "mais", "et", "donc",
    // KOB-179 : « dis » écrit « di », et d'autres débuts d'appel.
    "di", "dit", "oke", "okei", "okey", "voila", "allez", "bref", "attends",
];

/// Mots qui, juste avant le nom, en font une citation : « à Kobi », « que Kobi ».
const AVANT_CITATION: &[&str] = &[
    "a", "au", "aux", "de", "d", "du", "des", "que", "qu", "avec", "pour", "par", "sur", "chez",
    "le", "la", "les", "l", "comme", "sans", "contre", "entre", "vers", "selon", "dont", "ce",
    "cet", "mon", "ton", "son", "notre", "votre", "leur", "ou", "si",
];

/// Verbes qui, juste après le nom sans virgule, en font le sujet d'une
/// phrase sur lui : « Kobi dit », « Kobi a planté ». Pas ceux qui peuvent
/// être un ordre (« Kobi note ça », « Kobi raconte ») : en tête, c'est un appel.
const APRES_CITATION: &[&str] = &[
    "dit", "disait", "a", "avait", "aura", "est", "etait", "sera", "fait", "faisait", "pense",
    "peut", "pourrait", "sait", "va", "allait", "veut", "voulait", "doit", "devrait", "ne", "n",
    "aime", "comprend", "marche", "plante", "bugue", "entend", "connait", "semble", "s", "se", "m",
    "nous", "lui", "y", "en",
];

/// Mots qui, après « Kobi est » en tête, trahissent un « Kobi, et … » mal
/// transcrit (KOB-249) : « Kobe est la date du lancement ».
const EST_POUR_ET: &[&str] = &[
    "la", "le", "les", "l", "du", "des", "ce", "cet", "cette", "ces", "quand", "combien",
    "comment", "pourquoi", "quoi", "demain", "apres", "ensuite",
];

/// Deuxième personne : la phrase parle à quelqu'un.
const TOI: &[&str] = &[
    "tu", "t", "toi", "te", "ton", "ta", "tes", "vous", "votre", "vos",
];

/// Mots (sans accents ni majuscules) d'un morceau de phrase.
fn mots(morceau: &str) -> Vec<String> {
    morceau
        .split(|c: char| !c.is_alphanumeric())
        .filter(|m| !m.is_empty())
        .map(normaliser)
        .collect()
}

/// Phrase qui contient le nom : à qui elle s'adresse, et la question pour
/// Kobi. Nom en tête (après « euh, dis… ») : la question est ce qui suit,
/// comme avant ; ailleurs, c'est la phrase entière sans le nom, avec ses
/// virgules d'apostrophe (« Tu en penses quoi, Kobi ? » → « Tu en penses
/// quoi ? »). `None` sans le nom.
pub fn interpellation(texte: &str) -> Option<(Adresse, String)> {
    let texte = phrase_du_nom(texte);
    let (debut, fin) = trouver(texte).or_else(|| trouver_variante(texte))?;
    let avant = &texte[..debut];
    let apres = &texte[fin..];
    let mots_avant = mots(avant);
    let mots_apres = mots(apres);
    let virgule_avant = avant.trim_end().ends_with(',');
    let ponctuation_apres = apres
        .trim_start()
        .chars()
        .next()
        .is_none_or(|c| matches!(c, ',' | '.' | '?' | '!' | ';' | ':' | '…'));
    let virgule_apres = apres.trim_start().starts_with(',');
    let precedent = mots_avant.last().map(String::as_str);
    let suivant = mots_apres.first().map(String::as_str);
    let toi = mots_avant
        .iter()
        .chain(&mots_apres)
        .any(|m| TOI.contains(&m.as_str()));
    // « A Kobe, c'était quoi… » : le « hé » transcrit « a », suivi d'une
    // virgule d'apostrophe (KOB-179).
    let en_tete = mots_avant
        .iter()
        .all(|m| INTERJECTIONS.contains(&m.as_str()) || (virgule_apres && m == "a"));
    // « Kobi, et la date ? » transcrit « Kobe est la date » : une question
    // de suite, pas une phrase sur Kobi (KOB-249).
    let est_pour_et = en_tete
        && !virgule_apres
        && suivant == Some("est")
        // « Kobi est-ce que… » n'en est pas un.
        && apres
            .trim_start()
            .get(3..)
            .is_some_and(|r| r.starts_with(char::is_whitespace))
        && mots_apres
            .get(1)
            .is_some_and(|m| EST_POUR_ET.contains(&m.as_str()));
    let sujet =
        !ponctuation_apres && !est_pour_et && suivant.is_some_and(|m| APRES_CITATION.contains(&m));
    // « Est-ce que Kobi, … » : la question s'ouvre sur l'appel (KOB-237), et
    // la garde entière.
    let appel_en_tete = en_tete
        || mots_avant
            .iter()
            .map(String::as_str)
            .eq(["est", "ce", "que"]);
    let en_fin = mots_apres.is_empty();
    // « Est-ce que Kobi… » : le « que » de la question n'est pas celui de
    // « ce que Kobi dit ».
    let est_ce_que = mots_avant.len() >= 3
        && mots_avant[mots_avant.len() - 3..]
            .iter()
            .map(String::as_str)
            .eq(["est", "ce", "que"]);
    let cite_avant =
        !virgule_avant && !est_ce_que && precedent.is_some_and(|m| AVANT_CITATION.contains(&m));

    let adresse = if appel_en_tete {
        if sujet && !toi {
            Adresse::Cite
        } else {
            Adresse::Appel
        }
    } else if en_fin {
        if cite_avant {
            Adresse::Cite
        } else {
            Adresse::Appel
        }
    } else if virgule_avant && virgule_apres {
        Adresse::Appel
    } else if cite_avant || (sujet && !toi) {
        Adresse::Cite
    } else {
        // Au milieu d'une phrase, sans virgules autour : pas un appel net,
        // même avec « tu » (une conversation de groupe en est pleine, KOB-237).
        Adresse::Doute
    };

    let question = if est_pour_et {
        let apres = apres.trim_start();
        nettoyer(&format!("et{}", apres.get(3..).unwrap_or_default())).to_string()
    } else if en_tete {
        nettoyer(apres).to_string()
    } else {
        // Le nom et ses virgules d'apostrophe sortent ; la ponctuation de fin
        // reste (« quoi ? »).
        let avant = avant.trim_end();
        let avant = avant.strip_suffix(',').unwrap_or(avant).trim_end();
        let apres = apres.trim_start();
        let apres = apres.strip_prefix(',').unwrap_or(apres).trim_start();
        let phrase = if apres.is_empty() || apres.starts_with(['.', ',', '…']) {
            format!("{avant}{apres}")
        } else {
            format!("{avant} {apres}")
        };
        nettoyer(&phrase).to_string()
    };
    // Rien que de la ponctuation (« Kobi ? ») : pas de question.
    let question = if question.chars().any(char::is_alphanumeric) {
        question
    } else {
        String::new()
    };
    Some((adresse, question))
}

/// Mots au plus d'un début de phrase coupé par une pause, repris devant un
/// appel en tête (KOB-237).
const DEBUT_COUPE_MOTS: usize = 6;

/// Phrase courte, sans fin de phrase à l'intérieur : un début coupé, pas la
/// phrase entière de quelqu'un d'autre.
fn debut_coupe(phrase: &str) -> bool {
    !phrase.contains(['.', '?', '!', '…']) && mots(phrase).len() <= DEBUT_COUPE_MOTS
}

/// Le nom vient-il en tête de la phrase (après « euh, dis… ») ?
fn nom_en_tete(texte: &str) -> bool {
    trouver(texte).is_some_and(|(debut, _)| {
        mots(&texte[..debut])
            .iter()
            .all(|m| INTERJECTIONS.contains(&m.as_str()))
    })
}

/// Ce que l'écoute demande à la session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Kobi a entendu son nom (ou le raccourci) : passer en écoute.
    Interpeller,
    /// Question en cours de transcription, à afficher.
    Question(String),
    /// Question finie, à envoyer à Claude.
    Envoyer(String),
    /// Phrase finie qui cite Kobi sans qu'on sache si elle lui parle
    /// (KOB-168) : Claude la reçoit marquée « peut-être pas pour toi » et
    /// peut ne rien répondre. Plus émise depuis KOB-237 (un doute n'envoie
    /// rien) ; gardée tant que le chef traite `QuestionPeutEtre`.
    #[allow(dead_code, reason = "plus émise depuis KOB-237")]
    EnvoyerPeutEtre(String),
    /// Rien n'a suivi l'interpellation : retour au calme.
    Abandonner,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Attente {
    /// Personne n'a appelé Kobi.
    Nom,
    /// Nom entendu dans la phrase en cours (segments partiels) ; `t` est
    /// l'instant où il l'a été. `annonce` : Kobi s'est mis à écouter (pas
    /// quand la phrase, pour l'instant, le cite sans lui parler).
    Phrase { t: f64, annonce: bool },
    /// Interpellé (« Kobi. » seul ou raccourci) : la phrase suivante est la
    /// question. `parle` est l'instant où elle a commencé.
    Suite { depuis: f64, parle: Option<f64> },
    /// Conversation à deux (KOB-133) : chaque phrase est la suite de
    /// l'échange, sans redire « Kobi », jusqu'à [`CONVERSATION`] secondes de
    /// silence ou « merci Kobi ».
    Conversation { depuis: f64, parle: Option<f64> },
    /// Question envoyée avant la fin de la phrase (trop longue) : le reste de
    /// la phrase est ignoré jusqu'à sa fin.
    Ignore,
    /// Réunion arrêtée à la voix (KOB-165) : de `depuis` (l'arrêt) à
    /// `jusqu`, « reprends » part sans le nom ; toute autre phrase est
    /// ignorée, comme ce qui a été dit avant l'arrêt (KOB-265).
    Annulation { depuis: f64, jusqu: f64 },
}

/// Silence qui clôt une conversation, en secondes.
pub const CONVERSATION: f64 = 20.0;

/// La phrase clôt-elle la conversation (« merci Kobi », « c'est bon ») ?
pub fn fin_de_conversation(texte: &str) -> bool {
    let mots: Vec<String> = texte
        .split(|c: char| !c.is_alphanumeric())
        .filter(|m| !m.is_empty() && !est_kobi(m))
        .map(normaliser)
        .collect();
    matches!(
        mots.join(" ").as_str(),
        "merci"
            | "merci beaucoup"
            | "non merci"
            | "merci c est bon"
            | "c est bon"
            | "c est bon merci"
            | "ca ira"
            | "ca ira merci"
            | "c est tout"
            | "c est tout merci"
            | "laisse tomber"
            | "stop"
            | "thanks"
            | "thank you"
            | "that s all"
    )
}

/// Automate d'écoute, voir le module.
#[derive(Debug, Clone)]
pub struct Ecouteur {
    attente: Attente,
    /// Dernière question affichée.
    derniere: String,
    /// Mode conversation permis. Hors réunion seulement : en réunion, les
    /// participants se parlent entre eux (et une phrase qui cite Kobi sans
    /// lui parler à coup sûr ne le dérange pas).
    pub conversation: bool,
    /// Dernière phrase finie sans le nom, et l'instant où elle a fini : la
    /// question quand le nom vient seul juste après (« Tu en penses quoi ? »
    /// … « Kobi ? »), ou son début quand elle a été coupée.
    precedent: Option<(String, f64)>,
    /// Début de la phrase en cours (premier segment partiel).
    debut: Option<f64>,
    /// Fin de l'attente d'une carte de validation (`reponse_attendue`).
    carte: Option<f64>,
    /// La transcription est en retard sur la parole (KOB-250) : Nemotron
    /// colle alors les phrases, souvent sans ponctuation (KOB-276, KOB-289).
    pub sous_charge: bool,
}

impl Default for Ecouteur {
    fn default() -> Self {
        Self {
            attente: Attente::Nom,
            derniere: String::new(),
            conversation: true,
            precedent: None,
            debut: None,
            carte: None,
            sous_charge: false,
        }
    }
}

/// Écart maximal entre la phrase précédente et celle qui contient le nom
/// pour qu'elles forment une seule question, en secondes.
pub const PHRASE_PRECEDENTE: f64 = 2.5;

impl Ecouteur {
    /// Écouteur neuf ; `conversation` : voir le champ du même nom.
    pub fn new(conversation: bool) -> Self {
        Self {
            conversation,
            ..Self::default()
        }
    }

    /// Un segment de la transcription, à l'instant `t` de l'audio.
    pub fn segment(&mut self, texte: &str, finale: bool, t: f64) -> Vec<Action> {
        let mut actions = Vec::new();
        match self.attente {
            Attente::Ignore => {
                if finale {
                    self.attente = self.apres(t);
                }
            }
            Attente::Annulation { depuis, .. }
                if finale
                    && t >= depuis
                    && reperer(texte).is_none()
                    && crate::intentions::annule_la_fin(texte) =>
            {
                actions.push(Action::Interpeller);
                actions.push(self.envoyer(texte.trim(), t));
            }
            // Le nom, dit après l'arrêt, ramène à l'écoute ordinaire ; le
            // reste est ignoré. Dit avant (l'ordre de fin lui-même, transcrit
            // en retard après une fin anticipée), il ne ferme pas « Annuler »
            // (KOB-287).
            Attente::Annulation { depuis, jusqu } => {
                if t >= depuis && reperer(texte).is_some() {
                    self.attente = Attente::Nom;
                    actions = self.segment(texte, finale, t);
                } else if finale && reperer(texte).is_some() {
                    // Dit avant l'arrêt, transcrit pendant « Annuler » : un
                    // ordre comme un autre (« Kobi, note l'action X »),
                    // puis l'annulation reprend. L'ordre de fin lui-même est
                    // écarté plus loin, déjà exécuté (KOB-265, KOB-299).
                    self.attente = Attente::Nom;
                    actions = self.segment(texte, finale, t);
                    self.attente = Attente::Annulation { depuis, jusqu };
                }
            }
            // « Merci Kobi » n'est pas une interpellation.
            Attente::Nom if fin_de_conversation(texte) => self.phrase_sans_nom(texte, finale, t),
            Attente::Nom | Attente::Phrase { .. } => match interpellation(texte) {
                // Sous charge, un ordre court sur la réunion collé à la phrase
                // d'avant, sans ponctuation (« … le mentionnera Kobe termine
                // la réunion »), sur un segment final : un appel. Hors charge,
                // une phrase de cette forme décrit Kobi (« Normalement Kobi
                // termine la réunion ») et reste un doute (KOB-276, KOB-289).
                Some((Adresse::Doute, _))
                    if finale && self.sous_charge && ordre_colle(texte).is_some() =>
                {
                    let q = ordre_colle(texte).unwrap_or_default().to_owned();
                    let texte = phrase_du_nom(texte);
                    self.phrase_avec_nom(texte, Adresse::Appel, &q, finale, t, &mut actions)
                }
                // L'adresse se juge sur la phrase qui porte le nom (KOB-276).
                Some((adresse, q)) => {
                    let texte = phrase_du_nom(texte);
                    self.phrase_avec_nom(texte, adresse, &q, finale, t, &mut actions)
                }
                // La reconnaissance a révisé la phrase : le nom n'y est plus.
                None if finale && self.attente != Attente::Nom => {
                    let annonce = matches!(self.attente, Attente::Phrase { annonce: true, .. });
                    self.attente = Attente::Nom;
                    self.derniere.clear();
                    if annonce {
                        actions.push(Action::Abandonner);
                    }
                    self.phrase_sans_nom(texte, finale, t);
                }
                None => self.phrase_sans_nom(texte, finale, t),
            },
            Attente::Suite { depuis, parle } | Attente::Conversation { depuis, parle } => {
                let conversation = matches!(self.attente, Attente::Conversation { .. });
                let q = question(texte);
                if finale && q.chars().count() > self.borne() {
                    // Long flux de parole : pas une suite pour Kobi (KOB-237).
                    self.attente = Attente::Nom;
                    self.derniere.clear();
                    if parle.is_some() {
                        actions.push(Action::Abandonner);
                    }
                } else if finale {
                    if conversation && fin_de_conversation(texte) {
                        self.attente = Attente::Nom;
                        self.derniere.clear();
                        if parle.is_some() {
                            actions.push(Action::Abandonner);
                        }
                    } else if !q.is_empty() {
                        if conversation && parle.is_none() {
                            actions.push(Action::Interpeller);
                        }
                        actions.push(self.envoyer(q, t));
                    }
                } else if !q.is_empty() {
                    // En conversation, Kobi se remet à écouter dès que l'on parle.
                    if conversation && parle.is_none() {
                        actions.push(Action::Interpeller);
                    }
                    let parle = parle.or(Some(t));
                    self.attente = if conversation {
                        Attente::Conversation { depuis, parle }
                    } else {
                        Attente::Suite { depuis, parle }
                    };
                    self.afficher(q, &mut actions);
                }
            }
        }
        actions
    }

    /// Longueur maximale de la question en cours (KOB-237) : après un appel
    /// net (« Kobi. » seul, raccourci, nom entendu), celle d'un appel en
    /// tête ; en conversation, sans redire le nom, la borne stricte.
    fn borne(&self) -> usize {
        if matches!(self.attente, Attente::Conversation { .. }) {
            QUESTION_CARACTERES
        } else {
            QUESTION_CARACTERES_APPEL
        }
    }

    /// Le temps passe (appelé à chaque trame audio) : délais de l'écoute.
    pub fn tick(&mut self, t: f64) -> Vec<Action> {
        match self.attente {
            Attente::Annulation { jusqu, .. } if t > jusqu => {
                self.attente = Attente::Nom;
                Vec::new()
            }
            Attente::Suite {
                depuis,
                parle: None,
            } if t - depuis > SANS_PAROLE => {
                self.attente = Attente::Nom;
                vec![Action::Abandonner]
            }
            Attente::Conversation {
                depuis,
                parle: None,
            } if t - depuis > CONVERSATION => {
                self.attente = Attente::Nom;
                Vec::new()
            }
            Attente::Phrase {
                t: debut,
                annonce: false,
            } if t - debut > QUESTION_MAX => {
                self.attente = Attente::Nom;
                Vec::new()
            }
            Attente::Phrase { t: debut, .. }
            | Attente::Suite {
                parle: Some(debut), ..
            }
            | Attente::Conversation {
                parle: Some(debut), ..
            } if t - debut > QUESTION_MAX => {
                if self.derniere.is_empty() || self.derniere.chars().count() > self.borne() {
                    self.attente = Attente::Nom;
                    self.derniere.clear();
                    vec![Action::Abandonner]
                } else {
                    let q = self.derniere.clone();
                    let envoi = self.envoyer(&q, t);
                    // Le reste de la phrase ne repart pas une seconde fois.
                    self.attente = Attente::Ignore;
                    vec![envoi]
                }
            }
            _ => Vec::new(),
        }
    }

    /// Raccourci clavier : la phrase suivante est la question.
    pub fn raccourci(&mut self, t: f64) -> Vec<Action> {
        self.attente = Attente::Suite {
            depuis: t,
            parle: None,
        };
        self.derniere.clear();
        vec![Action::Interpeller]
    }

    /// Kobi vient de finir de répondre : la conversation attend la suite
    /// pendant [`CONVERSATION`] secondes à partir de maintenant.
    pub fn suite(&mut self, t: f64) {
        if !self.conversation {
            return;
        }
        if matches!(
            self.attente,
            Attente::Nom | Attente::Conversation { parle: None, .. }
        ) {
            self.attente = Attente::Conversation {
                depuis: t,
                parle: None,
            };
        }
    }

    /// Kobi attend une réponse à sa question pendant `duree` secondes (carte
    /// de validation, KOB-197) : « oui » ou « non » s'entend sans le nom tout
    /// ce temps, pas seulement les [`CONVERSATION`] secondes habituelles.
    pub fn reponse_attendue(&mut self, t: f64, duree: f64) {
        if !self.conversation {
            return;
        }
        self.carte = Some(t + duree);
        if matches!(
            self.attente,
            Attente::Nom
                | Attente::Conversation { parle: None, .. }
                | Attente::Suite { parle: None, .. }
        ) {
            self.attente = Attente::Conversation {
                depuis: t + (duree - CONVERSATION).max(0.0),
                parle: None,
            };
        }
    }

    /// Plus aucune carte n'attend (réponse au clic, à la voix, délai) : la
    /// fenêtre de la carte se ferme avec elle, et une question de suite
    /// coupée par une pause n'est plus tranchée (KOB-273). La conversation
    /// continue, comme après une réponse ([`Self::suite`]).
    pub fn cartes_fermees(&mut self, t: f64) {
        self.carte = None;
        self.suite(t);
    }

    /// Une carte de validation attend-elle sa réponse à l'instant `t`
    /// (`reponse_attendue`), et pas seulement une suite de conversation ?
    pub fn attend_carte(&self, t: f64) -> bool {
        self.carte.is_some_and(|fin| t <= fin) && self.attend_reponse()
    }

    /// Une phrase sans le nom compte-t-elle en ce moment (suite, conversation) ?
    pub fn attend_reponse(&self) -> bool {
        matches!(
            self.attente,
            Attente::Suite { .. } | Attente::Conversation { .. }
        )
    }

    /// La réunion vient d'être arrêtée, à l'instant `t` de l'audio : pendant
    /// `duree` secondes, « reprends » suffit, sans le nom.
    pub fn annulable(&mut self, t: f64, duree: f64) {
        if matches!(
            self.attente,
            Attente::Nom | Attente::Conversation { parle: None, .. }
        ) {
            self.attente = Attente::Annulation {
                depuis: t,
                jusqu: t + duree,
            };
        }
    }

    /// « Annuler » est fait ou clos (KOB-302) : la fenêtre se ferme, une
    /// phrase sans le nom n'y compte plus.
    pub fn fin_annulation(&mut self) {
        if matches!(self.attente, Attente::Annulation { .. }) {
            self.attente = Attente::Nom;
        }
    }

    /// Arrêt de la réunion annulable en cours ([`Ecouteur::annulable`]) ?
    pub fn en_annulation(&self) -> bool {
        matches!(self.attente, Attente::Annulation { .. })
    }

    /// Fin de la conversation imposée (entrée en réunion).
    pub fn oublier(&mut self) {
        if matches!(self.attente, Attente::Conversation { .. }) {
            self.attente = Attente::Nom;
        }
    }

    /// Segment d'une phrase qui ne contient pas le nom (hors interpellation) :
    /// retenue pour la question d'un nom dit seul juste après.
    fn phrase_sans_nom(&mut self, texte: &str, finale: bool, t: f64) {
        if finale {
            let texte = nettoyer(texte);
            self.precedent = (!texte.is_empty()).then(|| (texte.to_string(), t));
            self.debut = None;
        } else {
            self.debut.get_or_insert(t);
        }
    }

    /// Phrase qui contient le nom (KOB-168) : Kobi se met à écouter s'il est
    /// appelé (ou peut-être) ; la phrase finie part à Claude, se tait si elle
    /// le cite seulement. En réunion, un doute vaut silence.
    fn phrase_avec_nom(
        &mut self,
        texte: &str,
        adresse: Adresse,
        q: &str,
        finale: bool,
        t: f64,
        actions: &mut Vec<Action>,
    ) {
        let ecoute = match adresse {
            // Une phrase en cours ne fait écouter Kobi que si elle commence par
            // son nom : la fin de la phrase décide du reste. Hors réunion aussi
            // (KOB-237) : une transcription partielle s'arrête souvent juste
            // après le nom (« … et du coup Kobe »), qui semble alors en fin.
            Adresse::Appel => finale || nom_en_tete(texte),
            // Un doute ne fait pas écouter Kobi, et rien ne part vers Claude
            // (KOB-237).
            Adresse::Doute | Adresse::Cite => false,
        };
        let annonce = match self.attente {
            Attente::Phrase { annonce, .. } => annonce,
            _ => false,
        };
        let debut = match self.attente {
            Attente::Phrase { t, .. } => t,
            _ => t,
        };
        // La phrase précédente, toute proche, en fait partie quand le nom vient
        // seul, ou quand elle s'arrêtait en plein milieu. Devant un appel en
        // tête suivi de sa question, seulement un début coupé (« Est-ce que tu
        // pourrais »… « Kobi, me rappeler… ») : en conversation de groupe, la
        // phrase d'avant est souvent celle d'un autre (KOB-237).
        // Un ordre sur la réunion collé à la phrase d'avant compte comme un
        // appel en tête (KOB-276).
        let en_tete = nom_en_tete(texte) || ordre_de_reunion(q);
        let appel_complet = !q.is_empty() && en_tete;
        let precedent = self
            .precedent
            .take()
            .filter(|(_, fin)| self.debut.unwrap_or(t) - fin <= PHRASE_PRECEDENTE)
            .filter(|(p, _)| q.is_empty() || !p.ends_with(['.', '?', '!', '…']))
            .filter(|(p, _)| !appel_complet || debut_coupe(p))
            .map(|(p, _)| p);
        let question = match (&precedent, q.is_empty()) {
            (Some(p), true) => p.clone(),
            (Some(p), false) => format!("{p} {q}"),
            (None, _) => q.to_string(),
        };
        let borne = if en_tete {
            QUESTION_CARACTERES_APPEL
        } else {
            QUESTION_CARACTERES
        };
        // Long flux de parole (KOB-237) : même un nom entre virgules n'en fait
        // pas une question à Kobi, qui ne se met pas à écouter.
        let ecoute = ecoute && question.chars().count() <= borne;
        if ecoute && !annonce {
            actions.push(Action::Interpeller);
        }
        self.attente = Attente::Phrase {
            t: debut,
            annonce: annonce || ecoute,
        };
        if !finale {
            if ecoute {
                self.afficher(&question, actions);
            }
            self.precedent = precedent.map(|p| (p, t));
            self.debut.get_or_insert(t);
            return;
        }
        self.debut = None;
        if !ecoute {
            // Citation, doute, ou long flux de parole (KOB-237) : Kobi ne se
            // met pas à répondre, rien ne part vers Claude (même pas « peut-être
            // pas pour toi » : en conversation de groupe, chaque doute ouvrait
            // un tour), et aucune conversation ne s'ouvre derrière.
            self.attente = Attente::Nom;
            self.derniere.clear();
            if annonce {
                actions.push(Action::Abandonner);
            }
            return;
        }
        if question.is_empty() {
            // « Kobi. » seul : la phrase suivante est la question.
            self.attente = Attente::Suite {
                depuis: t,
                parle: None,
            };
        } else {
            actions.push(self.envoyer(&question, t));
        }
    }

    fn apres(&self, t: f64) -> Attente {
        if self.conversation {
            Attente::Conversation {
                depuis: t,
                parle: None,
            }
        } else {
            Attente::Nom
        }
    }

    fn afficher(&mut self, q: &str, actions: &mut Vec<Action>) {
        if !q.is_empty() && q != self.derniere {
            self.derniere = q.to_string();
            actions.push(Action::Question(q.to_string()));
        }
    }

    fn envoyer(&mut self, q: &str, t: f64) -> Action {
        self.attente = self.apres(t);
        self.derniere.clear();
        Action::Envoyer(q.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Action::*;

    #[test]
    fn nom_et_variantes() {
        assert_eq!(
            reperer("Kobi, quelle heure est-il ?"),
            Some("quelle heure est-il ?")
        );
        assert_eq!(
            reperer("kobe quelle heure est-il"),
            Some("quelle heure est-il")
        );
        assert_eq!(reperer("Coby. Il fait beau ?"), Some("Il fait beau ?"));
        assert_eq!(reperer("Kobié !"), Some(""));
        assert_eq!(
            reperer("dis-moi Cobi, c'est quoi la météo"),
            Some("c'est quoi la météo")
        );
        assert_eq!(reperer("KOBI"), Some(""));
        // Graphies relevées sur Nemotron avec la pénalité du symbole vide.
        assert_eq!(
            reperer("Coobby, quelle heure est-il?"),
            Some("quelle heure est-il?")
        );
        assert_eq!(
            reperer("Bonjour, Koobi, quelle heure est-il?"),
            Some("quelle heure est-il?")
        );
        assert_eq!(
            reperer("Dis-moi que obey, combien font douze fois sept"),
            Some("combien font douze fois sept")
        );
        assert_eq!(reperer("Bon, alors que Oobi, tu peux"), Some("tu peux"));
        assert_eq!(reperer("Bon, alors que hobie, tu peux"), Some("tu peux"));
        assert_eq!(reperer("je pense que oui"), None);
        assert_eq!(reperer("ce que Bob a dit"), None);
        for mot in ["Koubi", "Gobi", "Kobbie", "Qobi", "Khobi", "kobey"] {
            assert_eq!(reperer(mot), Some(""), "{mot}");
        }
    }

    #[test]
    fn mots_proches_ignores() {
        for texte in [
            "on a fait une copie du fichier",
            "le kobold du jeu",
            "Kobayashi est arrivé",
            "un robinet qui fuit",
            "le cobalt et le cuivre",
            "le cobaye de l'équipe commerciale",
            "il faut gober ça",
            "un coup de bol",
            "le Cobra et la Kombi",
            // « ai » accepté (KOB-269) : les mots proches restent ignorés.
            "je gobais tout",
            "un combat de boxe",
            "",
        ] {
            assert_eq!(reperer(texte), None, "{texte}");
        }
    }

    fn rejouer(e: &mut Ecouteur, segments: &[(&str, bool, f64)]) -> Vec<Action> {
        segments
            .iter()
            .flat_map(|&(texte, finale, t)| {
                let mut a = e.tick(t);
                a.extend(e.segment(texte, finale, t));
                a
            })
            .collect()
    }

    #[test]
    fn question_dans_la_meme_phrase() {
        let mut e = Ecouteur::default();
        let actions = rejouer(
            &mut e,
            &[
                ("Kobi", false, 1.0),
                ("Kobi quelle", false, 1.5),
                ("Kobi quelle heure", false, 2.0),
                ("Kobi, quelle heure est-il ?", true, 3.0),
            ],
        );
        assert_eq!(
            actions,
            [
                Interpeller,
                Question("quelle".into()),
                Question("quelle heure".into()),
                Envoyer("quelle heure est-il ?".into()),
            ]
        );
    }

    #[test]
    fn nom_en_milieu_de_phrase() {
        let mut e = Ecouteur::default();
        let actions = rejouer(
            &mut e,
            &[
                ("bon alors", false, 1.0),
                ("bon alors Kobi tu", false, 1.5),
                ("Bon alors Kobi, tu peux résumer ?", true, 2.5),
            ],
        );
        assert_eq!(
            actions,
            [
                Interpeller,
                Question("tu".into()),
                Envoyer("tu peux résumer ?".into())
            ]
        );
    }

    #[test]
    fn nom_seul_puis_question() {
        let mut e = Ecouteur::default();
        let actions = rejouer(
            &mut e,
            &[
                ("Kobi.", true, 1.0),
                ("quelle", false, 3.0),
                ("Quelle heure est-il ?", true, 4.0),
            ],
        );
        assert_eq!(
            actions,
            [
                Interpeller,
                Question("quelle".into()),
                Envoyer("Quelle heure est-il ?".into())
            ]
        );
    }

    #[test]
    fn nom_seul_puis_silence() {
        let mut e = Ecouteur::default();
        assert_eq!(e.segment("Kobi.", true, 1.0), [Interpeller]);
        assert!(e.tick(6.5).is_empty());
        assert_eq!(e.tick(7.5), [Abandonner]);
    }

    #[test]
    fn conversation_sans_le_nom() {
        let mut e = Ecouteur::default();
        let actions = rejouer(
            &mut e,
            &[
                ("on se voit", false, 1.0),
                ("On se voit demain ?", true, 2.0),
                ("oui à dix heures", true, 4.0),
                ("pense à la copie du contrat", true, 30.0),
            ],
        );
        assert!(actions.is_empty(), "{actions:?}");
    }

    #[test]
    fn nom_revise_par_la_reconnaissance() {
        let mut e = Ecouteur::default();
        let actions = rejouer(
            &mut e,
            &[("Kobi", false, 1.0), ("Robinet qui fuit", true, 2.0)],
        );
        assert_eq!(actions, [Interpeller, Abandonner]);
    }

    #[test]
    fn raccourci_puis_question() {
        let mut e = Ecouteur::default();
        assert_eq!(e.raccourci(10.0), [Interpeller]);
        let actions = rejouer(
            &mut e,
            &[
                ("Kobi c'est", false, 11.0),
                ("Kobi, c'est quoi ?", true, 12.0),
            ],
        );
        assert_eq!(
            actions,
            [Question("c'est".into()), Envoyer("c'est quoi ?".into())]
        );
    }

    #[test]
    fn raccourci_sans_question() {
        let mut e = Ecouteur::default();
        e.raccourci(10.0);
        assert_eq!(e.tick(17.0), [Abandonner]);
    }

    #[test]
    fn question_trop_longue_envoyee() {
        let mut e = Ecouteur::default();
        e.segment("Kobi raconte", false, 1.0);
        assert!(e.tick(15.0).is_empty());
        assert_eq!(e.tick(16.5), [Envoyer("raconte".into())]);
    }

    #[test]
    fn conversation_sans_redire_le_nom() {
        let mut e = Ecouteur::default();
        let actions = rejouer(
            &mut e,
            &[
                ("Kobi, quelle heure est-il ?", true, 2.0),
                // Kobi a répondu : la conversation attend la suite.
                ("et demain", false, 8.0),
                ("Et demain ?", true, 9.0),
                ("merci Kobi", true, 14.0),
                ("on se voit demain ?", true, 16.0),
            ],
        );
        assert_eq!(
            actions,
            [
                Interpeller,
                Envoyer("quelle heure est-il ?".into()),
                Interpeller,
                Question("et demain".into()),
                Envoyer("Et demain ?".into()),
            ]
        );
    }

    #[test]
    fn conversation_close_par_le_silence_compte_apres_la_reponse() {
        let mut e = Ecouteur::default();
        e.segment("Kobi, raconte une blague", true, 1.0);
        // La réponse s'affiche pendant 15 s ; la fenêtre part de sa fin.
        e.suite(16.0);
        assert!(e.tick(30.0).is_empty());
        assert_eq!(
            e.segment("encore une", true, 31.0),
            [Interpeller, Envoyer("encore une".into())]
        );
        e.suite(35.0);
        assert!(e.tick(56.0).is_empty());
        assert!(e.segment("on mange où ?", true, 57.0).is_empty());
    }

    #[test]
    fn pas_de_conversation_en_reunion() {
        let mut e = Ecouteur {
            conversation: false,
            ..Default::default()
        };
        e.segment("Kobi, quel est le budget ?", true, 1.0);
        e.suite(5.0);
        assert!(e.segment("on passe au point suivant", true, 6.0).is_empty());
    }

    #[test]
    fn reprends_sans_le_nom_apres_un_arret() {
        let mut e = Ecouteur::new(false);
        e.annulable(10.0, 30.0);
        // Les participants se disent au revoir : rien ne part.
        assert!(e
            .segment("Bon, merci à tous, bonne soirée", true, 12.0)
            .is_empty());
        assert_eq!(
            e.segment("Ah non, reprends !", true, 15.0),
            [Interpeller, Envoyer("Ah non, reprends !".into())]
        );
        // Une seule fois : ensuite il faut de nouveau le nom.
        assert!(e.segment("reprends", true, 18.0).is_empty());
    }

    /// KOB-265 : sous retard, un « on reprend » dit avant l'arrêt, encore en
    /// file, n'annule pas la fin ; celui dit après, si.
    #[test]
    fn reprends_dit_avant_l_arret_ne_compte_pas() {
        let mut e = Ecouteur::new(false);
        e.annulable(30.0, 10.0);
        assert!(e.tick(25.0).is_empty());
        assert!(e.segment("Bon, on reprend.", true, 25.0).is_empty());
        assert!(e.en_annulation());
        assert_eq!(
            e.segment("Ah non, reprends !", true, 38.0),
            [Interpeller, Envoyer("Ah non, reprends !".into())]
        );
    }

    /// KOB-267, segments de l'essai KOB-259 : la question envoyée (et titre
    /// de la conversation) ne commence pas par la ponctuation parasite.
    #[test]
    fn question_sans_ponctuation_parasite() {
        let mut e = Ecouteur::default();
        assert_eq!(
            e.segment("Kobe,', what day is it today", true, 1.0),
            [Interpeller, Envoyer("what day is it today".into())]
        );
        let mut e = Ecouteur::default();
        let actions = e.segment(
            "Kobi '' Tell me a long story about a little robot who learns to cook pancakes",
            true,
            1.0,
        );
        assert_eq!(
            actions.last(),
            Some(&Envoyer(
                "Tell me a long story about a little robot who learns to cook pancakes".into()
            ))
        );
    }

    /// KOB-287 : fin anticipée sous retard, l'ordre de fin, dit avant
    /// l'arrêt, arrive après : il ne ferme pas « Annuler » ; « reprends »
    /// sans le nom part encore.
    #[test]
    fn ordre_de_fin_en_retard_garde_annuler() {
        let mut e = Ecouteur::new(false);
        e.annulable(100.0, 10.0);
        assert!(e.segment("Kobi, termine la", false, 80.0).is_empty());
        // L'ordre de fin, déjà exécuté, est écarté plus loin
        // (`sans_ordres_deja_executes`) ; l'annulation reste ouverte.
        e.segment("Kobi, termine la réunion", true, 80.0);
        assert!(e.en_annulation());
        assert_eq!(
            e.segment("reprends", true, 105.0),
            [Interpeller, Envoyer("reprends".into())]
        );
    }

    /// KOB-299 : un autre ordre dit avant l'arrêt, transcrit pendant
    /// « Annuler », part encore ; l'annulation reste ouverte.
    #[test]
    fn ordre_dit_avant_l_arret_part_pendant_annuler() {
        let mut e = Ecouteur::new(false);
        e.annulable(100.0, 10.0);
        assert!(e.segment("Kobi, note l'action", false, 75.0).is_empty());
        assert_eq!(
            e.segment("Kobi, note l'action budget", true, 75.0),
            [Interpeller, Envoyer("note l'action budget".into())]
        );
        assert!(e.en_annulation());
        // Sans le nom, dit avant l'arrêt : rien.
        assert!(e.segment("on en reparle lundi", true, 78.0).is_empty());
        assert_eq!(
            e.segment("reprends", true, 105.0),
            [Interpeller, Envoyer("reprends".into())]
        );
    }

    /// KOB-302 : « Annuler » fait (clic) ou clos ; une phrase sans le nom
    /// avec « continue » ou « annule » ne part plus, le nom oui.
    #[test]
    fn fenetre_fermee_avec_l_annulation() {
        let mut e = Ecouteur::new(false);
        e.annulable(10.0, 10.0);
        e.fin_annulation();
        assert!(!e.en_annulation());
        assert!(e.segment("bon, on continue", true, 13.0).is_empty());
        assert!(e.segment("annule", true, 14.0).is_empty());
        assert_eq!(
            e.segment("Kobi, quelle heure est-il ?", true, 45.0),
            [Interpeller, Envoyer("quelle heure est-il ?".into())]
        );
        // Hors annulation, rien ne change.
        let mut e = Ecouteur::default();
        e.suite(0.0);
        e.fin_annulation();
        assert!(e.attend_reponse());
    }

    #[test]
    fn annulation_expiree_ou_nom() {
        let mut e = Ecouteur::new(false);
        e.annulable(10.0, 30.0);
        assert!(e.tick(41.0).is_empty());
        assert!(e.segment("reprends", true, 42.0).is_empty());
        // Le nom pendant l'annulation : question ordinaire.
        e.annulable(50.0, 30.0);
        assert_eq!(
            e.segment("Kobi, quelle heure est-il ?", true, 52.0),
            [Interpeller, Envoyer("quelle heure est-il ?".into())]
        );
    }

    #[test]
    fn merci_kobi_n_interpelle_pas() {
        let mut e = Ecouteur::default();
        assert!(e.segment("Merci Kobi.", true, 1.0).is_empty());
        assert!(e.tick(10.0).is_empty());
        assert!(fin_de_conversation("C'est bon, merci."));
        assert!(!fin_de_conversation("merci de résumer le budget"));
    }

    /// KOB-237 (bug vécu en conversation de groupe) : un long flux de parole
    /// avec le nom au milieu ne déclenche rien et n'envoie rien ; un vrai
    /// appel en tête passe jusqu'à 400 caractères, au-delà Kobi abandonne.
    #[test]
    fn conversation_de_groupe_et_borne_de_longueur() {
        let flux = format!(
            "{} et du coup Kobe il a dit que tu passes par la droite {}",
            "alors on part sur la carte du nord avec les autres ".repeat(5),
            "puis on revient au point de départ pour finir la manche ".repeat(4)
        );
        assert!(flux.chars().count() > 500);
        let mut e = Ecouteur::default();
        let actions = rejouer(&mut e, &[(flux.as_str(), true, 1.0)]);
        assert!(
            !actions
                .iter()
                .any(|a| matches!(a, Envoyer(_) | EnvoyerPeutEtre(_) | Interpeller)),
            "{actions:?}"
        );
        // Les phrases suivantes de la conversation (73 et 84 caractères dans le
        // journal) ne partent pas non plus : aucune conversation n'a été ouverte.
        let suite = rejouer(
            &mut e,
            &[
                (
                    "ouais mais attends il faut d'abord prendre le pont avant eux",
                    true,
                    6.0,
                ),
                (
                    "ok je te suis, tu prends à gauche et moi je reste derrière toi",
                    true,
                    11.0,
                ),
            ],
        );
        assert!(suite.is_empty(), "{suite:?}");
        // Rejeu du WAV façon journal : un segment partiel s'arrête juste après
        // le nom, qui semble en fin de phrase ; la phrase finie l'a perdu.
        // Kobi ne se met pas à écouter.
        let mut e = Ecouteur::default();
        let actions = rejouer(
            &mut e,
            &[
                (
                    "alors on part sur la carte du nord et du coup Kobe",
                    false,
                    1.0,
                ),
                (
                    "alors on part sur la carte du nord et du coup Kobe il a dit",
                    false,
                    1.5,
                ),
                (
                    "alors on part sur la carte du nord et du coup il a dit",
                    true,
                    2.0,
                ),
            ],
        );
        assert!(actions.is_empty(), "{actions:?}");
        // La phrase finie du même WAV : la ponctuation mise par Nemotron place
        // le nom entre virgules (« du coup, Kobe, il a dit »). Trop long pour
        // une question : Kobi ne s'allume pas.
        let ponctue = flux.replace("du coup Kobe il", "du coup, Kobe, il");
        let mut e = Ecouteur::default();
        let actions = rejouer(&mut e, &[(ponctue.as_str(), true, 1.0)]);
        assert!(actions.is_empty(), "{actions:?}");
        // Essai Xvfb : la phrase d'un autre, juste avant un vrai appel en tête,
        // ne se recolle pas à la question.
        let mut e = Ecouteur::default();
        let actions = rejouer(
            &mut e,
            &[
                (
                    "Ok. Je te suis, tu prends à gauche et moi, je reste derrière toi jusqu'à la fin de la manche",
                    true,
                    1.0,
                ),
                ("Koby, quelle heure est-il", true, 2.5),
            ],
        );
        assert_eq!(
            actions,
            [Interpeller, Envoyer("quelle heure est-il".into())]
        );
        // Un vrai appel en tête, long (≈ 250 caractères) : il part.
        let question = "résume-moi tout ce qu'on a dit sur le budget ".repeat(5);
        let appel = format!("Kobi, {question}");
        let mut e = Ecouteur::default();
        let actions = rejouer(&mut e, &[(appel.as_str(), true, 1.0)]);
        assert!(
            actions.iter().any(|a| matches!(a, Envoyer(_))),
            "{actions:?}"
        );
        // Au-delà de 400 caractères, même en tête : Kobi abandonne.
        let trop = format!("Kobi, {}", question.repeat(2));
        let mut e = Ecouteur::default();
        let actions = rejouer(&mut e, &[(trop.as_str(), true, 1.0)]);
        assert!(
            !actions.iter().any(|a| matches!(a, Envoyer(_))),
            "{actions:?}"
        );
        // Après le raccourci (appel net), la même question longue part ; en
        // conversation, sans redire le nom, elle ne part pas.
        let mut e = Ecouteur::default();
        e.raccourci(0.0);
        let actions = rejouer(&mut e, &[(question.as_str(), true, 1.0)]);
        assert!(
            actions.iter().any(|a| matches!(a, Envoyer(_))),
            "{actions:?}"
        );
        e.suite(2.0);
        let actions = rejouer(&mut e, &[(question.as_str(), true, 3.0)]);
        assert!(actions.is_empty(), "{actions:?}");
    }

    /// KOB-197 : pendant qu'une carte attend (60 s), une réponse sans le nom
    /// compte tout ce temps ; ensuite, retour à l'écoute du nom.
    #[test]
    fn reponse_attendue_le_temps_de_la_carte() {
        let mut e = Ecouteur::default();
        e.reponse_attendue(10.0, 60.0);
        assert!(e.tick(50.0).is_empty());
        assert!(e.attend_reponse());
        e.tick(71.0);
        assert!(!e.attend_reponse());
        // Une réponse dans la fenêtre part sans le nom.
        let mut e = Ecouteur::default();
        e.reponse_attendue(0.0, 60.0);
        e.tick(45.0);
        let actions = e.segment("Non", true, 45.0);
        assert!(
            actions.contains(&Action::Envoyer("Non".into())),
            "{actions:?}"
        );
    }

    /// KOB-273 : la carte fermée au clic, sa fenêtre se ferme avec elle ; la
    /// conversation, elle, continue sans le nom.
    #[test]
    fn fenetre_de_la_carte_fermee_avec_elle() {
        let mut e = Ecouteur::default();
        e.reponse_attendue(0.0, 60.0);
        assert!(e.attend_carte(5.0));
        e.cartes_fermees(5.0);
        assert!(!e.attend_carte(10.0), "plus de carte : rien n'est tranché");
        assert!(e.attend_reponse(), "la suite de la conversation reste");
        // Une nouvelle carte rouvre la fenêtre.
        e.reponse_attendue(20.0, 60.0);
        assert!(e.attend_carte(30.0));
    }

    /// Graphies de Nemotron relevées sur le corpus de KOB-179 : retrouvées en
    /// position d'appel, jamais ailleurs.
    #[test]
    fn variantes_de_nemotron_en_appel() {
        use Adresse::*;
        let appels: &[(&str, &str)] = &[
            ("Kirby, tu peux répéter", "tu peux répéter"),
            (
                "Voilà Cody,, tu peux arrêter la réunion",
                "tu peux arrêter la réunion",
            ),
            ("Etkobi ouvre et réclage", "ouvre et réclage"),
            (
                "Okeikobi lance la prise de notes",
                "lance la prise de notes",
            ),
            (
                "Wicoby met ça dans les décisions",
                "met ça dans les décisions",
            ),
            ("Makobi, résume la réunion", "résume la réunion"),
            (
                "Di Koby, combien on a de tickets ouverts",
                "combien on a de tickets ouverts",
            ),
            (
                "A Kobe, c'était quoi le nom du client",
                "c'était quoi le nom du client",
            ),
            (
                "Qu'est-ce que t'en penses, Kobye?",
                "Qu'est-ce que t'en penses ?",
            ),
            (
                "Dit Koby,, combien on a de tickets ouverts?",
                "combien on a de tickets ouverts?",
            ),
            (
                "Hockeykobi lance la prise de notes",
                "lance la prise de notes",
            ),
            ("Oke Kobi lance la prise de note", "lance la prise de note"),
            // Prononcé à l'anglaise, segments de l'essai KOB-259 (KOB-269).
            ("Kobai record the meeting", "record the meeting"),
            ("Kobai, record the meeting", "record the meeting"),
        ];
        for (texte, question) in appels {
            assert_eq!(
                interpellation(texte),
                Some((Appel, question.to_string())),
                "{texte}"
            );
        }
        // Ailleurs qu'en appel : rien, ou une citation (KOB-168 inchangé).
        for texte in [
            "Mon collègue Cody est en congé.",
            "On parle de Kirby ou de Mario ?",
            "Fais une copie du devis pour le client.",
            "Le cobra est le serpent le plus long d'Asie.",
            "J'ai acheté un combi pour les vacances.",
            "On a pris un coca et un Kirby en peluche.",
            // KOB-237 : variante sans virgule après, ou en fin de phrase.
            "Cogi explique moi, cette erreur",
            "C'est quoi le prochain point, Kirby ?",
            // Sans virgule après, plus une variante : rien (avant, citation).
            "Kirby est un personnage de jeu vidéo.",
            "Cody a envoyé le rapport ce matin.",
        ] {
            assert_eq!(interpellation(texte), None, "{texte}");
        }
        // En conversation, le nom collé en tête sort aussi de la question.
        assert_eq!(question("Etkobi ouvre et réclage"), "ouvre et réclage");
        assert_eq!(
            reperer("Wicoby, met ça dans les décisions"),
            Some("met ça dans les décisions")
        );
        // « à Kobi » sans virgule reste une citation.
        assert_eq!(
            interpellation("on le dit à Kobi").map(|(a, _)| a),
            Some(Cite)
        );
    }

    /// Plus de 40 phrases (KOB-168) : nom au début, au milieu, à la fin,
    /// cité sans être appelé, variantes de transcription, phrases réelles de
    /// la réunion du 18/10.
    #[test]
    fn appele_ou_cite() {
        use Adresse::*;
        let cas: &[(&str, Adresse, &str)] = &[
            // Nom au début : la question est ce qui suit.
            (
                "Kobi, quelle heure est-il ?",
                Appel,
                "quelle heure est-il ?",
            ),
            ("Kobi quelle heure est-il", Appel, "quelle heure est-il"),
            ("Euh, Kobi, tu peux résumer ?", Appel, "tu peux résumer ?"),
            (
                "Dis Kobi, c'est quoi la météo ?",
                Appel,
                "c'est quoi la météo ?",
            ),
            (
                "dis-moi Cobi, c'est quoi la météo",
                Appel,
                "c'est quoi la météo",
            ),
            ("Hé Koby, ouvre le carnet", Appel, "ouvre le carnet"),
            ("Bon alors Kobi, on en est où ?", Appel, "on en est où ?"),
            (
                "Kobi, est-ce que tu m'entends ?",
                Appel,
                "est-ce que tu m'entends ?",
            ),
            (
                "Kobi est-ce que tu peux noter ça",
                Appel,
                "est-ce que tu peux noter ça",
            ),
            ("Kobe, démarre la réunion", Appel, "démarre la réunion"),
            // Ponctuation parasite après le nom (KOB-267).
            (
                "Kobe,', what day is it today",
                Appel,
                "what day is it today",
            ),
            (
                "Kobi '' Tell me a long story about a little robot who learns to cook pancakes",
                Appel,
                "Tell me a long story about a little robot who learns to cook pancakes",
            ),
            (
                "Kobi, \"Hello\" en espagnol ?",
                Appel,
                "\"Hello\" en espagnol ?",
            ),
            ("Oui Kobé, vas-y", Appel, "vas-y"),
            ("Kobi.", Appel, ""),
            // Nom à la fin : la phrase entière.
            ("Tu en penses quoi, Kobi ?", Appel, "Tu en penses quoi ?"),
            (
                "Tu peux me dire l'heure, Kobi ?",
                Appel,
                "Tu peux me dire l'heure ?",
            ),
            (
                "Euh, qu'est-ce que t'en penses toi, Koby",
                Appel,
                "Euh, qu'est-ce que t'en penses toi",
            ),
            ("Quelle heure est-il Kobi ?", Appel, "Quelle heure est-il ?"),
            (
                "C'est quoi le prochain point, Cobi ?",
                Appel,
                "C'est quoi le prochain point ?",
            ),
            ("Résume la réunion, Kobi.", Appel, "Résume la réunion"),
            (
                "Rappelle-moi d'appeler Léa demain Kobi",
                Appel,
                "Rappelle-moi d'appeler Léa demain",
            ),
            (
                "On fait comment pour le devis, Kobi ?",
                Appel,
                "On fait comment pour le devis ?",
            ),
            // Nom au milieu : appel entre virgules ou après « est-ce que » ;
            // sans virgules, même avec « tu », doute (KOB-237).
            (
                "Est-ce que, Kobi, tu peux me rappeler l'heure ?",
                Appel,
                "Est-ce que tu peux me rappeler l'heure ?",
            ),
            (
                "Est-ce que tu peux, Kobi, me rappeler la réunion ?",
                Appel,
                "Est-ce que tu peux me rappeler la réunion ?",
            ),
            ("Et là, Kobi, on fait quoi ?", Appel, "Et là on fait quoi ?"),
            (
                "Est-ce que tu peux Kobi me rappeler ça",
                Doute,
                "Est-ce que tu peux me rappeler ça",
            ),
            // Transcription réelle (essai du 19/10) : la virgule avant le nom manque.
            (
                "Est-ce que Kobe, tu peux me rappeler le nom du projet",
                Appel,
                "Est-ce que tu peux me rappeler le nom du projet",
            ),
            (
                "Est-ce que Kobi tu as noté le budget ?",
                Appel,
                "Est-ce que tu as noté le budget ?",
            ),
            (
                "Je suis d'accord avec Kobi, tu sais",
                Cite,
                "Je suis d'accord avec tu sais",
            ),
            (
                "Alors toi Kobi tu en dis quoi ?",
                Doute,
                "Alors toi tu en dis quoi ?",
            ),
            (
                "Franchement, Koby, t'as une idée ?",
                Appel,
                "Franchement t'as une idée ?",
            ),
            // Cité sans être appelé : citation.
            (
                "Par rapport à ce que Cobi dit, je ne suis pas d'accord",
                Cite,
                "Par rapport à ce que dit, je ne suis pas d'accord",
            ),
            ("Je suis d'accord avec Kobi", Cite, "Je suis d'accord avec"),
            (
                "C'est ce que disait Kobi tout à l'heure",
                Doute,
                "C'est ce que disait tout à l'heure",
            ),
            ("Kobi a raison sur ce point", Cite, "a raison sur ce point"),
            // « Kobi, et … » transcrit « Kobe est … » (KOB-249) : une question.
            (
                "Kobe est la date du lancement",
                Appel,
                "et la date du lancement",
            ),
            (
                "Kobi est la date du tir d'essai ?",
                Appel,
                "et la date du tir d'essai ?",
            ),
            (
                "Euh Kobi est combien ça coûte",
                Appel,
                "et combien ça coûte",
            ),
            ("Kobi est vraiment pratique", Cite, "est vraiment pratique"),
            (
                "Kobi est en train d'écouter",
                Cite,
                "est en train d'écouter",
            ),
            ("Kobi dit n'importe quoi", Cite, "dit n'importe quoi"),
            (
                "Hier Kobi a planté pendant la démo",
                Cite,
                "Hier a planté pendant la démo",
            ),
            (
                "On en parlera à Kobi plus tard",
                Cite,
                "On en parlera à plus tard",
            ),
            (
                "Le compte rendu de Kobi est dans le carnet",
                Cite,
                "Le compte rendu de est dans le carnet",
            ),
            (
                "Je crois que Kobi note tout",
                Cite,
                "Je crois que note tout",
            ),
            (
                "Il faudrait montrer ça à Kobi",
                Cite,
                "Il faudrait montrer ça à",
            ),
            ("Tu as essayé avec Kobi ?", Cite, "Tu as essayé avec ?"),
            ("Sans Kobi on aurait oublié", Cite, "Sans on aurait oublié"),
            (
                "Kobi ne marche pas chez moi",
                Cite,
                "ne marche pas chez moi",
            ),
            // Ni l'un ni l'autre : doute.
            (
                "Le truc Kobi pour les réunions",
                Doute,
                "Le truc pour les réunions",
            ),
            ("Il y a Kobi qui écoute", Cite, "Il y a qui écoute"),
            (
                "Normalement Kobi enregistre aussi",
                Doute,
                "Normalement enregistre aussi",
            ),
        ];
        assert!(cas.len() >= 40);
        for (texte, adresse, question) in cas {
            assert_eq!(
                interpellation(texte),
                Some((*adresse, (*question).to_string())),
                "{texte}"
            );
        }
        assert_eq!(interpellation("Quelle heure est-il ?"), None);
    }

    #[test]
    fn nom_a_la_fin_de_la_phrase() {
        let mut e = Ecouteur::default();
        let actions = rejouer(
            &mut e,
            &[
                ("tu en penses", false, 1.0),
                ("tu en penses quoi Kobi", false, 1.8),
                ("Tu en penses quoi, Kobi ?", true, 2.5),
            ],
        );
        // Le nom en fin de phrase en cours ne fait pas écouter Kobi : la
        // phrase finie décide (KOB-237).
        assert_eq!(
            actions,
            [Interpeller, Envoyer("Tu en penses quoi ?".into())]
        );
    }

    #[test]
    fn nom_seul_juste_apres_la_question() {
        let mut e = Ecouteur::default();
        let actions = rejouer(
            &mut e,
            &[
                ("tu en penses quoi", false, 1.0),
                ("Tu en penses quoi ?", true, 2.0),
                ("Kobi ?", true, 3.2),
            ],
        );
        assert_eq!(
            actions,
            [Interpeller, Envoyer("Tu en penses quoi ?".into())]
        );
        // Trop loin de la phrase précédente : on attend la suivante.
        let mut e = Ecouteur::default();
        let actions = rejouer(
            &mut e,
            &[("Tu en penses quoi ?", true, 2.0), ("Kobi ?", true, 9.0)],
        );
        assert_eq!(actions, [Interpeller]);
    }

    #[test]
    fn phrase_coupee_puis_nom() {
        let mut e = Ecouteur::default();
        let actions = rejouer(
            &mut e,
            &[
                ("Est-ce que tu pourrais", true, 1.5),
                ("Kobi me rappeler", false, 2.4),
                ("Kobi, me rappeler l'heure du train ?", true, 3.5),
            ],
        );
        assert_eq!(
            actions,
            [
                Interpeller,
                Question("Est-ce que tu pourrais me rappeler".into()),
                Envoyer("Est-ce que tu pourrais me rappeler l'heure du train ?".into()),
            ]
        );
        // « Kobi raconte », « Kobi note ça » : des ordres, pas des citations.
        assert_eq!(
            interpellation("Kobi note ça").map(|(a, _)| a),
            Some(Adresse::Appel)
        );
        assert_eq!(
            interpellation("Kobi raconte").map(|(a, _)| a),
            Some(Adresse::Appel)
        );
    }

    #[test]
    fn citation_sans_interpellation() {
        let mut e = Ecouteur::default();
        let actions = rejouer(
            &mut e,
            &[
                ("par rapport à ce que Cobi", false, 1.0),
                (
                    "Par rapport à ce que Cobi dit, on garde le plan.",
                    true,
                    2.5,
                ),
            ],
        );
        assert!(actions.is_empty(), "{actions:?}");
        // Une phrase d'abord ambiguë, qui se révèle une citation : Kobi ne
        // s'est même pas mis à écouter (KOB-237).
        let mut e = Ecouteur::default();
        let actions = rejouer(
            &mut e,
            &[
                ("normalement Kobi", false, 1.0),
                ("Normalement Kobi a tout noté.", true, 2.0),
            ],
        );
        assert!(actions.is_empty(), "{actions:?}");
    }

    /// KOB-249 : les segments relevés à l'essai de KOB-196 (réunion, voix
    /// Piper) : « Kobi, et la date du lancement ? » part enfin.
    #[test]
    fn kobi_et_transcrit_kobi_est() {
        let mut e = Ecouteur::default();
        let actions = rejouer(
            &mut e,
            &[
                ("Kobe est", false, 1.0),
                ("Kobe est la date du", false, 2.0),
                ("Kobe est la date du lancement", false, 2.9),
                ("Kobe est la date du lancement", true, 3.6),
            ],
        );
        assert!(actions.contains(&Action::Interpeller), "{actions:?}");
        assert_eq!(
            actions.last(),
            Some(&Action::Envoyer("et la date du lancement".into())),
            "{actions:?}"
        );
    }

    #[test]
    fn doute_hors_reunion_silence_en_reunion() {
        let mut e = Ecouteur::default();
        let actions = rejouer(&mut e, &[("Le truc Kobi pour les réunions", true, 1.0)]);
        // Un doute ne fait pas écouter Kobi et n'envoie rien (KOB-237).
        assert_eq!(actions, []);
        // En réunion, un doute ne dérange personne, même quand la phrase en
        // cours semblait l'appeler.
        let mut e = Ecouteur::new(false);
        let actions = rejouer(
            &mut e,
            &[
                ("le truc Kobi", false, 0.5),
                ("Le truc Kobi pour les réunions", true, 1.0),
            ],
        );
        assert!(actions.is_empty(), "{actions:?}");
        // Mais un appel, oui.
        let actions = rejouer(&mut e, &[("Tu en penses quoi, Kobi ?", true, 5.0)]);
        assert_eq!(
            actions,
            [Interpeller, Envoyer("Tu en penses quoi ?".into())]
        );
    }

    /// KOB-276 : le nom ouvre une phrase après une autre du même segment
    /// (Nemotron les rend ensemble sous charge, ou quand on enchaîne) : un
    /// appel. Le nom au milieu de sa phrase reste un doute ; seul en fin, il
    /// garde la phrase d'avant pour question ; une initiale ne coupe pas.
    #[test]
    fn appel_qui_suit_une_autre_phrase() {
        for texte in [
            "Le budget est validé et le compte rendu le mentionnera. Kobe. termine la réunion.",
            "Le budget est validé et le compte rendu le mentionnera. Kobi, termine la réunion.",
            "Bon, on a fini ? Kobi, termine la réunion",
        ] {
            assert_eq!(
                interpellation(texte),
                Some((Adresse::Appel, "termine la réunion".into())),
                "{texte}"
            );
            let mut e = Ecouteur::new(false);
            assert_eq!(
                e.segment(texte, true, 10.0),
                vec![
                    Action::Interpeller,
                    Action::Envoyer("termine la réunion".into())
                ],
                "{texte}"
            );
        }
        assert_ne!(
            interpellation("On a fini. Alors du coup Kobe il a dit non à tout le monde.")
                .map(|(a, _)| a),
            Some(Adresse::Appel)
        );
        assert_eq!(
            interpellation("Quelle heure est-il ? Kobi."),
            Some((Adresse::Appel, "Quelle heure est-il ?".into()))
        );
        assert_eq!(
            phrase_du_nom("J'ai vu M. Kobi hier soir."),
            "J'ai vu M. Kobi hier soir."
        );
    }

    /// KOB-276, KOB-289 : un ordre court sur la réunion collé à la phrase
    /// d'avant, sans ponctuation, est un appel sous charge (transcription en
    /// retard, segment final) ; hors charge, ou sur un partiel, une phrase de
    /// cette forme décrit Kobi et ne fait rien.
    #[test]
    fn ordre_colle_seulement_sous_charge() {
        let ordre = vec![
            Action::Interpeller,
            Action::Envoyer("termine la réunion".into()),
        ];
        // Le segment réel de l'essai du 24/10, sous retard.
        let reel = "Le budget de la ligne vingt est validé, et le compte rendu, le mentionnera Kobe termine la réunion";
        let mut e = Ecouteur::new(false);
        e.sous_charge = true;
        assert_eq!(e.segment(reel, true, 10.0), ordre);
        // Précédé d'un segment tout proche, sans point final : la question
        // reste l'ordre seul.
        let mut e = Ecouteur::new(false);
        e.sous_charge = true;
        e.segment("Le budget de la ligne dix-neuf est validé", true, 5.0);
        assert_eq!(
            e.segment(
                "et le compte rendu le mentionnera Kobe termine la réunion",
                true,
                6.0
            ),
            ordre
        );
        // Sous charge, un partiel n'en fait pas un appel.
        let mut e = Ecouteur::new(false);
        e.sous_charge = true;
        assert!(!e.segment(reel, false, 10.0).contains(&Action::Interpeller));
        // Hors charge, des phrases qui décrivent Kobi ne font rien.
        for texte in [
            reel,
            "Normalement Kobi termine la réunion",
            "Quand on a fini, Kobi met la réunion en pause",
        ] {
            let mut e = Ecouteur::new(false);
            assert_eq!(e.segment(texte, true, 10.0), vec![], "{texte}");
            assert_ne!(
                interpellation(texte).map(|(a, _)| a),
                Some(Adresse::Appel),
                "{texte}"
            );
        }
        // Un ordre long, ou cité, reste dehors, même sous charge.
        for texte in [
            "il paraît que Kobe termine la réunion demain avec toute l'équipe",
            "j'ai dit à Kobi termine la réunion",
        ] {
            let mut e = Ecouteur::new(false);
            e.sous_charge = true;
            assert!(
                !e.segment(texte, true, 10.0).contains(&Action::Interpeller),
                "{texte}"
            );
        }
    }
}
