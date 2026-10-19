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
const FINS: [&str; 6] = ["i", "y", "ie", "e", "ey", "is"];

/// Sans aucun mot après l'interpellation, l'écoute s'arrête.
pub const SANS_PAROLE: f64 = 6.0;
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

/// Retire la ponctuation et les espaces qui entourent un morceau de phrase.
fn nettoyer(morceau: &str) -> &str {
    morceau.trim_matches(|c: char| c.is_whitespace() || matches!(c, ',' | '.' | '!' | ';' | ':'))
}

/// Ce qui suit « Kobi » dans le texte, ou `None` s'il n'y est pas.
pub fn reperer(texte: &str) -> Option<&str> {
    trouver(texte).map(|(_, fin)| nettoyer(&texte[fin..]))
}

/// Question d'une phrase dite après l'interpellation : un « Kobi » répété en
/// tête est retiré.
fn question(texte: &str) -> &str {
    match trouver(texte) {
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
    let (debut, fin) = trouver(texte)?;
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
    let sujet = !ponctuation_apres && suivant.is_some_and(|m| APRES_CITATION.contains(&m));
    let en_tete = mots_avant
        .iter()
        .all(|m| INTERJECTIONS.contains(&m.as_str()));
    let en_fin = mots_apres.is_empty();
    // « Est-ce que Kobi… » : le « que » de la question n'est pas celui de
    // « ce que Kobi dit ».
    let est_ce_que = mots_avant.len() >= 3
        && mots_avant[mots_avant.len() - 3..]
            .iter()
            .map(String::as_str)
            .eq(["est", "ce", "que"]);
    let cite_avant = !virgule_avant
        && !est_ce_que
        && precedent.is_some_and(|m| AVANT_CITATION.contains(&m));

    let adresse = if en_tete {
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
    } else if toi {
        Adresse::Appel
    } else {
        Adresse::Doute
    };

    let question = if en_tete {
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
    /// peut ne rien répondre. Jamais en réunion : Kobi s'y tait.
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
}

impl Default for Ecouteur {
    fn default() -> Self {
        Self {
            attente: Attente::Nom,
            derniere: String::new(),
            conversation: true,
            precedent: None,
            debut: None,
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
            // « Merci Kobi » n'est pas une interpellation.
            Attente::Nom if fin_de_conversation(texte) => self.phrase_sans_nom(texte, finale, t),
            Attente::Nom | Attente::Phrase { .. } => match interpellation(texte) {
                Some((adresse, q)) => {
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
                if finale {
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

    /// Le temps passe (appelé à chaque trame audio) : délais de l'écoute.
    pub fn tick(&mut self, t: f64) -> Vec<Action> {
        match self.attente {
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
                if self.derniere.is_empty() {
                    self.attente = Attente::Nom;
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
        let reunion = !self.conversation;
        let ecoute = match adresse {
            // En réunion, une phrase en cours ne fait écouter Kobi que si elle
            // commence par son nom : la fin de la phrase décide du reste.
            Adresse::Appel => finale || !reunion || nom_en_tete(texte),
            Adresse::Doute => !reunion,
            Adresse::Cite => false,
        };
        let annonce = match self.attente {
            Attente::Phrase { annonce, .. } => annonce,
            _ => false,
        };
        if ecoute && !annonce {
            actions.push(Action::Interpeller);
        }
        let debut = match self.attente {
            Attente::Phrase { t, .. } => t,
            _ => t,
        };
        self.attente = Attente::Phrase {
            t: debut,
            annonce: annonce || ecoute,
        };
        // La phrase précédente, toute proche, en fait partie quand le nom vient
        // seul, ou quand elle s'arrêtait en plein milieu.
        let precedent = self
            .precedent
            .take()
            .filter(|(_, fin)| self.debut.unwrap_or(t) - fin <= PHRASE_PRECEDENTE)
            .filter(|(p, _)| q.is_empty() || !p.ends_with(['.', '?', '!', '…']))
            .map(|(p, _)| p);
        let question = match (&precedent, q.is_empty()) {
            (Some(p), true) => p.clone(),
            (Some(p), false) => format!("{p} {q}"),
            (None, _) => q.to_string(),
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
            // Citation, ou doute en réunion : Kobi ne répond pas.
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
        } else if adresse == Adresse::Doute {
            self.attente = self.apres(t);
            self.derniere.clear();
            actions.push(Action::EnvoyerPeutEtre(question));
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
    fn merci_kobi_n_interpelle_pas() {
        let mut e = Ecouteur::default();
        assert!(e.segment("Merci Kobi.", true, 1.0).is_empty());
        assert!(e.tick(10.0).is_empty());
        assert!(fin_de_conversation("C'est bon, merci."));
        assert!(!fin_de_conversation("merci de résumer le budget"));
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
            // Nom au milieu, entre virgules ou avec « tu » : appel.
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
                Appel,
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
                Appel,
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
        assert_eq!(
            actions,
            [
                Interpeller,
                Question("tu en penses quoi".into()),
                Envoyer("Tu en penses quoi ?".into()),
            ]
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
        // Une phrase d'abord ambiguë, qui se révèle une citation : Kobi se tait.
        let mut e = Ecouteur::default();
        let actions = rejouer(
            &mut e,
            &[
                ("normalement Kobi", false, 1.0),
                ("Normalement Kobi a tout noté.", true, 2.0),
            ],
        );
        assert_eq!(
            actions,
            [Interpeller, Question("normalement".into()), Abandonner]
        );
    }

    #[test]
    fn doute_hors_reunion_silence_en_reunion() {
        let mut e = Ecouteur::default();
        let actions = rejouer(&mut e, &[("Le truc Kobi pour les réunions", true, 1.0)]);
        assert_eq!(
            actions,
            [
                Interpeller,
                EnvoyerPeutEtre("Le truc pour les réunions".into())
            ]
        );
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
}
