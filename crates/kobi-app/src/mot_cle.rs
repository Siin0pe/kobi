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

/// Ce que l'écoute demande à la session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Kobi a entendu son nom (ou le raccourci) : passer en écoute.
    Interpeller,
    /// Question en cours de transcription, à afficher.
    Question(String),
    /// Question finie, à envoyer à Claude.
    Envoyer(String),
    /// Rien n'a suivi l'interpellation : retour au calme.
    Abandonner,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Attente {
    /// Personne n'a appelé Kobi.
    Nom,
    /// Nom entendu dans la phrase en cours (segments partiels) ; `t` est
    /// l'instant où il l'a été.
    Phrase { t: f64 },
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
    /// participants se parlent entre eux.
    pub conversation: bool,
}

impl Default for Ecouteur {
    fn default() -> Self {
        Self {
            attente: Attente::Nom,
            derniere: String::new(),
            conversation: true,
        }
    }
}

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
            Attente::Nom if fin_de_conversation(texte) => {}
            Attente::Nom | Attente::Phrase { .. } => match reperer(texte) {
                Some(suite) => {
                    if self.attente == Attente::Nom {
                        actions.push(Action::Interpeller);
                        self.attente = Attente::Phrase { t };
                    }
                    if finale {
                        if suite.is_empty() {
                            self.attente = Attente::Suite {
                                depuis: t,
                                parle: None,
                            };
                        } else {
                            actions.push(self.envoyer(suite, t));
                        }
                    } else {
                        self.afficher(suite, &mut actions);
                    }
                }
                // La reconnaissance a révisé la phrase : le nom n'y est plus.
                None if finale && self.attente != Attente::Nom => {
                    self.attente = Attente::Nom;
                    actions.push(Action::Abandonner);
                }
                None => {}
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
            Attente::Phrase { t: debut }
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
}
