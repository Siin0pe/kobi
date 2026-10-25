//! Table des commandes, la même pour la CLI, le MCP et l'agent de Kobi.
//!
//! Chaque commande déclare son effet : c'est sur lui que s'appuient le MCP
//! (`readOnlyHint`, outils exposés à l'agent) et la validation (KOB-156).

use serde_json::{json, Map, Value};

use crate::protocole::{Code, Erreur};

/// Ce que change une commande.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effet {
    /// Ne change rien.
    Lecture,
    /// Change seulement ce qui est montré (fenêtres, mascotte).
    Affichage,
    /// Modifie des données, des réglages ou l'état (réunion, écoute, son).
    Ecriture,
}

impl Effet {
    pub fn cle(self) -> &'static str {
        match self {
            Effet::Lecture => "lecture",
            Effet::Affichage => "affichage",
            Effet::Ecriture => "ecriture",
        }
    }

    /// Étiquette en tête de la description d'un outil MCP.
    pub fn etiquette(self) -> &'static str {
        match self {
            Effet::Lecture => "[lecture]",
            Effet::Affichage => "[affichage]",
            Effet::Ecriture => "[écriture]",
        }
    }

    /// [`Effet::etiquette`] dans la langue de l'interface, pour `kobi help`
    /// (le MCP garde l'étiquette française, lue par Claude).
    pub fn etiquette_affichee(self) -> String {
        match self {
            Effet::Lecture => kobi_i18n::tr("ctl.commandes.effetLecture", &[]),
            Effet::Affichage => kobi_i18n::tr("ctl.commandes.effetAffichage", &[]),
            Effet::Ecriture => kobi_i18n::tr("ctl.commandes.effetEcriture", &[]),
        }
    }
}

/// Type d'un paramètre.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Genre {
    Texte,
    Booleen,
    Entier,
    /// N'importe quelle valeur JSON (`config.set`).
    Json,
}

#[derive(Debug, Clone, Copy)]
pub struct Param {
    pub nom: &'static str,
    pub genre: Genre,
    pub requis: bool,
    /// Se donne sans `--nom` dans la CLI (dans l'ordre de la table).
    pub positionnel: bool,
    /// Valeurs permises (vide : libre).
    pub choix: &'static [&'static str],
    pub description: &'static str,
}

const fn p(nom: &'static str, genre: Genre, description: &'static str) -> Param {
    Param {
        nom,
        genre,
        requis: false,
        positionnel: false,
        choix: &[],
        description,
    }
}

const fn requis(mut param: Param) -> Param {
    param.requis = true;
    param
}

const fn positionnel(mut param: Param) -> Param {
    param.positionnel = true;
    param
}

const fn choix(mut param: Param, choix: &'static [&'static str]) -> Param {
    param.choix = choix;
    param
}

#[derive(Debug, Clone, Copy)]
pub struct Commande {
    /// Nom dans le protocole : mots séparés par des points (`meeting.start`).
    pub nom: &'static str,
    pub effet: Effet,
    pub description: &'static str,
    pub params: &'static [Param],
    /// Effet difficile à annuler (`app.quit`).
    pub destructive: bool,
    /// Réservée à Kobi lui-même (le hook de ses commandes) : absente des MCP.
    pub interne: bool,
}

const fn c(
    nom: &'static str,
    effet: Effet,
    description: &'static str,
    params: &'static [Param],
) -> Commande {
    Commande {
        nom,
        effet,
        description,
        params,
        destructive: false,
        interne: false,
    }
}

use Effet::{Affichage, Ecriture, Lecture};
use Genre::{Booleen, Entier, Json, Texte};

/// Sections de l'app Kobi (KOB-159).
pub const SECTIONS: &[&str] = &[
    "notes",
    "reunions",
    "conversations",
    "rappels",
    "activite",
    "reglages",
    "a-propos",
    "accueil",
    // Ancien nom de « a-propos » (contrat v1).
    "apropos",
];

/// Panneaux de la section Réglages, dans l'ordre du menu de l'app
/// (`PANNEAUX` de ui/src/reglages/panneaux.ts, vérifié par un test).
pub const PANNEAUX: &[&str] = &[
    "stockage",
    "dossiers-code",
    "mcp",
    "permissions",
    "micro",
    "son-ordi",
    "modele",
    "claude",
    "demarrage",
    "langue",
    "animations",
    "affichage",
    "raccourcis",
    "premier-lancement",
];

const FORMAT_JSON_MD: &[&str] = &["json", "md"];
const ID_REUNION: Param = positionnel(p(
    "id",
    Texte,
    "Nom de la réunion (dossier du Carnet), ou « last » pour la dernière.",
));
const LIMITE: Param = p("limit", Entier, "Nombre maximal d'éléments.");

pub const COMMANDES: &[Commande] = &[
    c(
        "status",
        Lecture,
        "État de Kobi : session, mascotte, réunion en cours, écoute, fenêtre.",
        &[],
    ),
    c(
        "app.open",
        Affichage,
        "Ouvre l'app Kobi sur une section (notes et réunions, rappels, activité, réglages, à propos, accueil).",
        &[
            requis(positionnel(choix(
                p("section", Texte, "Section à ouvrir."),
                SECTIONS,
            ))),
            choix(
                p("panneau", Texte, "Panneau des réglages (section « reglages »)."),
                PANNEAUX,
            ),
            p("reunion", Texte, "Réunion à ouvrir dans les notes (son nom, voir « meetings list »)."),
        ],
    ),
    c("mascot.show", Affichage, "Montre la mascotte.", &[]),
    c("mascot.hide", Affichage, "Cache la mascotte (Kobi reste dans la barre système).", &[]),
    c(
        "mascot.discreet",
        Affichage,
        "Mode discret : la mascotte devient une petite pastille, ou revient.",
        &[requis(positionnel(p("on", Booleen, "true : discret ; false : mascotte entière.")))],
    ),
    c("view.open", Affichage, "Ouvre la conversation en grand à l'écran (vue agrandie).", &[]),
    c("view.close", Affichage, "Range la vue agrandie de la conversation.", &[]),
    c("view.toggle", Affichage, "Ouvre ou range la vue agrandie de la conversation.", &[]),
    Commande {
        destructive: true,
        ..c("app.quit", Ecriture, "Quitte Kobi.", &[])
    },
    c(
        "config.get",
        Lecture,
        "Lit un réglage (clé pointée, par exemple « voice.assistant »), ou tous les réglages.",
        &[positionnel(p("key", Texte, "Clé pointée ; absente : tous les réglages."))],
    ),
    c(
        "config.list",
        Lecture,
        "Liste les réglages : clé, valeur, application à chaud ou au redémarrage, description.",
        &[],
    ),
    c(
        "config.set",
        Ecriture,
        "Modifie un réglage : valeur vérifiée, écrite dans config.toml, appliquée à chaud quand c'est possible.",
        &[
            requis(positionnel(p("key", Texte, "Clé pointée (par exemple « voice.assistant »)."))),
            requis(positionnel(p("value", Json, "Nouvelle valeur (JSON : true, 3, \"texte\", [\"C:/code\"])."))),
        ],
    ),
    c("config.undo", Ecriture, "Annule le dernier changement de réglage.", &[]),
    c("meeting.start", Ecriture, "Démarre l'enregistrement de la réunion (prise de notes).", &[]),
    c("meeting.stop", Ecriture, "Termine la réunion ; le compte rendu s'écrit ensuite.", &[]),
    c("meeting.pause", Ecriture, "Met l'enregistrement de la réunion en pause.", &[]),
    c("meeting.resume", Ecriture, "Reprend l'enregistrement de la réunion.", &[]),
    c(
        "meeting.rename",
        Ecriture,
        "Donne un titre à la réunion en cours (ou à une réunion passée avec --id) ; Kobi ne le remplace plus.",
        &[
            requis(positionnel(p("titre", Texte, "Nouveau titre."))),
            p("id", Texte, "Réunion passée (dossier du Carnet) ; la réunion en cours sinon."),
        ],
    ),
    c(
        "meeting.status",
        Lecture,
        "État de la réunion en cours : actif, pause, début, durée, nombre de notes, dernière note.",
        &[],
    ),
    c(
        "meetings.list",
        Lecture,
        "Liste les réunions enregistrées, la plus récente d'abord.",
        &[LIMITE],
    ),
    c(
        "meetings.get",
        Lecture,
        "Lit une réunion : compte rendu, notes, transcription, ou tout.",
        &[
            ID_REUNION,
            choix(
                p("part", Texte, "Partie : all (défaut), summary, notes, transcript."),
                &["all", "summary", "notes", "transcript"],
            ),
            choix(p("format", Texte, "json (défaut) ou md."), FORMAT_JSON_MD),
        ],
    ),
    c(
        "notes.last",
        Lecture,
        "Notes et compte rendu de la dernière réunion.",
        &[choix(p("format", Texte, "md (défaut) ou json."), FORMAT_JSON_MD)],
    ),
    c(
        "meetings.export",
        Lecture,
        "Exporte une réunion en markdown (titre et texte complet).",
        &[requis(ID_REUNION)],
    ),
    c(
        "search",
        Lecture,
        "Cherche un mot dans les réunions (transcription, notes, compte rendu) et les conversations.",
        &[
            requis(positionnel(p("query", Texte, "Texte cherché."))),
            LIMITE,
        ],
    ),
    c(
        "conversations.list",
        Lecture,
        "Liste les conversations du quotidien, la plus récente d'abord.",
        &[LIMITE],
    ),
    c(
        "conversations.get",
        Lecture,
        "Lit une conversation.",
        &[
            positionnel(p("id", Texte, "Identifiant, ou « last » pour la dernière.")),
            choix(p("format", Texte, "json (défaut) ou md."), FORMAT_JSON_MD),
        ],
    ),
    c(
        "reminders.list",
        Lecture,
        "Liste les rappels et tâches à faire.",
        &[p("all", Booleen, "Inclure les rappels faits.")],
    ),
    c(
        "reminders.add",
        Ecriture,
        "Ajoute un rappel ou une tâche.",
        &[
            requis(positionnel(p("text", Texte, "Texte du rappel."))),
            p("due", Texte, "Échéance : date RFC 3339 (2026-10-19T09:00:00+02:00) ou ms Unix."),
        ],
    ),
    c(
        "reminders.done",
        Ecriture,
        "Marque un rappel comme fait.",
        &[requis(positionnel(p("id", Texte, "Identifiant du rappel.")))],
    ),
    c("listen.pause", Ecriture, "Met l'écoute en pause : micro coupé, plus de transcription.", &[]),
    c("listen.resume", Ecriture, "Reprend l'écoute.", &[]),
    c(
        "voice.say",
        Ecriture,
        "Kobi dit un texte à voix haute.",
        &[requis(positionnel(p("text", Texte, "Texte à dire.")))],
    ),
    // KOB-156 : le hook des commandes de Claude (processus à part) demande à
    // l'app la carte de validation d'une écriture et attend la réponse.
    Commande {
        interne: true,
        ..c(
            "validation.ask",
            Affichage,
            "Demande à l'utilisateur de valider un appel d'outil qui écrit (carte, oui ou non, refus au bout de 60 s) ; répond accepte: true ou false.",
            &[
                requis(choix(
                    p("kind", Texte, "Genre de l'action."),
                    &["fichier", "commande", "suppression", "mcp", "reglage"],
                )),
                requis(p("tool", Texte, "Outil appelé (Write, Edit, Bash…).")),
                requis(p("input", Json, "Entrée de l'outil, telle que l'agent l'a envoyée.")),
                p("summary", Texte, "Résumé en une phrase, à la place de celui de Kobi."),
            ],
        )
    },
];

/// Une commande se désigne par son nom.
impl PartialEq for Commande {
    fn eq(&self, autre: &Self) -> bool {
        self.nom == autre.nom
    }
}

/// Commandes de réunion que l'agent de Kobi lance sans validation (KOB-165,
/// exception à KOB-156) : c'est lui qui démarre et arrête l'enregistrement
/// d'après ce qu'il entend ; Kobi l'annonce et un arrêt s'annule 30 s.
pub const PILOTAGE_REUNION: &[&str] = &[
    "meeting.start",
    "meeting.stop",
    "meeting.pause",
    "meeting.resume",
];

/// Texte de `cle` dans la langue courante ; le français de la table si la
/// clé manque.
fn traduite(cle: &str, francais: &str) -> String {
    let texte = kobi_i18n::tr(cle, &[]);
    if texte == cle {
        francais.to_owned()
    } else {
        texte
    }
}

pub fn trouver(nom: &str) -> Option<&'static Commande> {
    COMMANDES.iter().find(|c| c.nom == nom)
}

/// Commande d'après le nom de son outil MCP (`meeting_start`).
pub fn par_outil(outil: &str) -> Option<&'static Commande> {
    COMMANDES.iter().find(|c| c.outil() == outil)
}

impl Commande {
    /// L'agent de Kobi la lance-t-il sans validation ([`PILOTAGE_REUNION`]) ?
    pub fn pilote_la_reunion(&self) -> bool {
        PILOTAGE_REUNION.contains(&self.nom)
    }

    /// Permise à une session en lecture seule (KOB-307) : une lecture, hors
    /// réunion (`meeting.*`).
    pub fn lecture_seule(&self) -> bool {
        self.effet == Effet::Lecture && !self.nom.starts_with("meeting.")
    }

    /// Clé de traduction de la description (`ctl.commandes.meetingStart`).
    fn cle(&self) -> String {
        let mut cle = String::from("ctl.commandes.");
        for (i, mot) in self.nom.split('.').enumerate() {
            let mut lettres = mot.chars();
            if let (true, Some(premiere)) = (i > 0, lettres.next()) {
                cle.extend(premiere.to_uppercase());
                cle.push_str(lettres.as_str());
            } else {
                cle.push_str(mot);
            }
        }
        cle
    }

    /// Description montrée par `kobi help`, dans la langue de l'interface.
    /// Le MCP garde [`Commande::description`] (lue par Claude). Clé formée
    /// d'après le nom : une par commande, vérifiée par un test.
    pub fn description_affichee(&self) -> String {
        traduite(&self.cle(), self.description)
    }

    /// Description d'un de ses paramètres pour `kobi help`
    /// (`ctl.commandes.meetingsGet.part`).
    pub fn description_du_param(&self, param: &Param) -> String {
        traduite(&format!("{}.{}", self.cle(), param.nom), param.description)
    }

    /// Mots de la CLI (`meeting start`).
    pub fn mots(&self) -> String {
        self.nom.replace('.', " ")
    }

    /// Nom de l'outil MCP (`meeting_start`) ; Claude Code le voit comme
    /// `mcp__kobi__meeting_start`.
    pub fn outil(&self) -> String {
        self.nom.replace('.', "_")
    }

    /// Schéma JSON des paramètres (MCP `inputSchema`).
    pub fn schema(&self) -> Value {
        let mut proprietes = Map::new();
        for param in self.params {
            let mut s = match param.genre {
                Texte => json!({"type": "string"}),
                Booleen => json!({"type": "boolean"}),
                Entier => json!({"type": "integer", "minimum": 1}),
                Json => json!({}),
            };
            s["description"] = param.description.into();
            if !param.choix.is_empty() {
                s["enum"] = json!(param.choix);
            }
            proprietes.insert(param.nom.into(), s);
        }
        let requis: Vec<&str> = self
            .params
            .iter()
            .filter(|p| p.requis)
            .map(|p| p.nom)
            .collect();
        json!({
            "type": "object",
            "properties": proprietes,
            "required": requis,
            "additionalProperties": false,
        })
    }

    /// Vérifie les paramètres reçus ; `null` vaut un objet vide.
    pub fn verifier(&self, params: &Value) -> Result<Map<String, Value>, Erreur> {
        let invalide = |m: String| Erreur::new(Code::InvalidParams, m);
        let params = match params {
            Value::Null => Map::new(),
            Value::Object(m) => m.clone(),
            _ => {
                return Err(invalide(kobi_i18n::tr(
                    "ctl.commandes.parametresObjet",
                    &[],
                )))
            }
        };
        for nom in params.keys() {
            if !self.params.iter().any(|p| p.nom == nom) {
                return Err(invalide(kobi_i18n::tr(
                    "ctl.commandes.parametreInconnu",
                    &[("commande", self.nom), ("nom", nom)],
                )));
            }
        }
        for param in self.params {
            // `null` est une vraie valeur JSON (« revenir au défaut » pour
            // `config set`, KOB-256) ; pour les autres genres, il vaut absent.
            let Some(v) = params
                .get(param.nom)
                .filter(|v| !v.is_null() || matches!(param.genre, Json))
            else {
                if param.requis {
                    return Err(invalide(kobi_i18n::tr(
                        "ctl.commandes.parametreManquant",
                        &[("commande", self.nom), ("nom", param.nom)],
                    )));
                }
                continue;
            };
            let bon_type = match param.genre {
                Texte => v.is_string(),
                Booleen => v.is_boolean(),
                Entier => v.as_u64().is_some_and(|n| n >= 1),
                Json => true,
            };
            if !bon_type {
                let attendu = match param.genre {
                    Texte => kobi_i18n::tr("ctl.commandes.attendTexte", &[]),
                    Booleen => kobi_i18n::tr("ctl.commandes.attendBooleen", &[]),
                    Entier => kobi_i18n::tr("ctl.commandes.attendEntier", &[]),
                    Json => kobi_i18n::tr("ctl.commandes.attendJson", &[]),
                };
                return Err(invalide(kobi_i18n::tr(
                    "ctl.commandes.attend",
                    &[
                        ("commande", self.nom),
                        ("nom", param.nom),
                        ("attendu", &attendu),
                    ],
                )));
            }
            if let (false, Some(s)) = (param.choix.is_empty(), v.as_str()) {
                if !param.choix.contains(&s) {
                    return Err(invalide(kobi_i18n::tr(
                        "ctl.commandes.vaut",
                        &[
                            ("commande", self.nom),
                            ("nom", param.nom),
                            ("choix", &param.choix.join(", ")),
                        ],
                    )));
                }
            }
        }
        Ok(params)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// KOB-256 : `kobi config set <clé> null` revient au défaut ; `null`
    /// n'est pas un paramètre manquant pour une valeur JSON.
    #[test]
    fn null_est_une_valeur_json() {
        let set = trouver("config.set").unwrap();
        assert!(set
            .verifier(&json!({"key": "notes.interval_min", "value": null}))
            .is_ok());
        assert!(
            set.verifier(&json!({"key": "notes.interval_min"})).is_err(),
            "absente : manquante"
        );
        assert!(
            set.verifier(&json!({"key": null, "value": 1})).is_err(),
            "clé nulle : manquante"
        );
    }

    #[test]
    fn noms_uniques_et_outils_valides() {
        let mut noms: Vec<_> = COMMANDES.iter().map(|c| c.outil()).collect();
        noms.sort();
        noms.dedup();
        assert_eq!(noms.len(), COMMANDES.len());
        for c in COMMANDES {
            // Nom d'outil MCP accepté par Claude : [a-zA-Z0-9_-]{1,64}.
            let outil = c.outil();
            assert!(
                outil.len() <= 64
                    && outil
                        .chars()
                        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
            );
            assert_eq!(par_outil(&outil).unwrap().nom, c.nom);
            assert_eq!(trouver(c.nom).unwrap().nom, c.nom);
            assert!(!c.description.is_empty());
        }
    }

    #[test]
    fn effets_du_contrat() {
        let effet = |n| trouver(n).unwrap().effet;
        for n in [
            "status",
            "config.get",
            "config.list",
            "meeting.status",
            "meetings.list",
            "meetings.get",
            "notes.last",
            "meetings.export",
            "search",
            "conversations.list",
            "conversations.get",
            "reminders.list",
        ] {
            assert_eq!(effet(n), Lecture, "{n}");
        }
        for n in [
            "app.open",
            "mascot.show",
            "mascot.hide",
            "mascot.discreet",
            "view.open",
            "view.close",
            "view.toggle",
        ] {
            assert_eq!(effet(n), Affichage, "{n}");
        }
        for n in [
            "app.quit",
            "config.set",
            "config.undo",
            "meeting.start",
            "meeting.stop",
            "reminders.add",
            "reminders.done",
            "listen.pause",
            "voice.say",
        ] {
            assert_eq!(effet(n), Ecriture, "{n}");
        }
        assert!(trouver("app.quit").unwrap().destructive);
        assert!(COMMANDES.iter().filter(|c| c.destructive).count() == 1);
        assert!(trouver("validation.ask").unwrap().interne);
        assert!(COMMANDES.iter().filter(|c| c.interne).count() == 1);
    }

    #[test]
    fn verification_des_parametres() {
        let get = trouver("meetings.get").unwrap();
        assert!(get.verifier(&Value::Null).unwrap().is_empty());
        assert!(get.verifier(&json!({"id": "last", "format": "md"})).is_ok());
        let e = get.verifier(&json!({"format": "pdf"})).unwrap_err();
        assert_eq!(e.code, Code::InvalidParams);
        assert!(e.message.contains("json, md"), "{}", e.message);
        assert!(get.verifier(&json!({"inconnu": 1})).is_err());
        assert!(get.verifier(&json!([1])).is_err());
        let set = trouver("config.set").unwrap();
        assert!(set.verifier(&json!({"key": "voice.assistant"})).is_err());
        assert!(set
            .verifier(&json!({"key": "codebases", "value": ["C:/code"]}))
            .is_ok());
        let liste = trouver("meetings.list").unwrap();
        assert!(liste.verifier(&json!({"limit": 0})).is_err());
        assert!(liste.verifier(&json!({"limit": "3"})).is_err());
        assert!(liste.verifier(&json!({"limit": 3})).is_ok());
        // Un panneau inconnu est refusé comme une section inconnue (KOB-184).
        let ouvrir = trouver("app.open").unwrap();
        assert!(ouvrir
            .verifier(&json!({"section": "reglages", "panneau": "son-ordi"}))
            .is_ok());
        let e = ouvrir
            .verifier(&json!({"section": "reglages", "panneau": "inexistant"}))
            .unwrap_err();
        assert_eq!(e.code, Code::InvalidParams);
        assert!(
            e.message.contains("stockage, dossiers-code"),
            "{}",
            e.message
        );
    }

    /// Les panneaux du contrat sont ceux du menu Réglages de l'app, dans le
    /// même ordre : un panneau ajouté à l'UI doit l'être ici (KOB-184).
    #[test]
    fn panneaux_de_l_ui() {
        let ui = concat!(env!("CARGO_MANIFEST_DIR"), "/../../ui/src/reglages");
        let lire = |f: &str| std::fs::read_to_string(format!("{ui}/{f}")).unwrap();
        let panneaux = lire("panneaux.ts");
        let liste = panneaux
            .split_once("const liste: Panneau[] = [")
            .and_then(|(_, r)| r.split_once(']'))
            .expect("liste des panneaux dans panneaux.ts")
            .0;
        // Identifiant de chaque constante `export const NOM: Panneau = { id: '…'`.
        let mut ids = std::collections::HashMap::new();
        for f in std::fs::read_dir(ui).unwrap() {
            let texte = std::fs::read_to_string(f.unwrap().path()).unwrap();
            for bloc in texte.split("export const ").skip(1) {
                let Some((nom, reste)) = bloc.split_once(": Panneau = {") else {
                    continue;
                };
                let id = reste
                    .split_once("id: '")
                    .and_then(|(_, r)| r.split_once('\''))
                    .expect("id du panneau")
                    .0;
                ids.insert(nom.trim().to_owned(), id.to_owned());
            }
        }
        let ui: Vec<&str> = liste
            .split(',')
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(|n| ids.get(n).unwrap_or_else(|| panic!("panneau {n}")).as_str())
            .collect();
        assert_eq!(ui, PANNEAUX);
    }

    /// `kobi help` montre chaque description dans les deux langues ; le
    /// français du catalogue est celui de la table (lu aussi par le MCP).
    #[test]
    fn descriptions_traduites_pour_kobi_help() {
        use kobi_i18n::{t, Langue};
        let mut cles = vec![];
        for c in COMMANDES {
            cles.push((c.cle(), c.description));
            for p in c.params {
                cles.push((format!("{}.{}", c.cle(), p.nom), p.description));
            }
        }
        for (cle, francais) in cles {
            assert_eq!(t(Langue::Fr, &cle, &[]), francais, "{cle}");
            let anglais = t(Langue::En, &cle, &[]);
            assert!(anglais != cle && anglais != francais, "{cle} : {anglais}");
        }
        assert_eq!(
            trouver("meeting.start").unwrap().cle(),
            "ctl.commandes.meetingStart"
        );
        assert_eq!(
            t(Langue::En, "ctl.commandes.meetingStart", &[]),
            "Starts recording the meeting (note-taking)."
        );
    }

    #[test]
    fn schema_mcp() {
        let s = trouver("app.open").unwrap().schema();
        assert_eq!(s["required"], json!(["section"]));
        assert_eq!(s["properties"]["section"]["enum"][0], "notes");
        assert_eq!(s["properties"]["panneau"]["enum"][0], "stockage");
        assert_eq!(s["additionalProperties"], false);
        assert_eq!(trouver("status").unwrap().schema()["properties"], json!({}));
    }
}
