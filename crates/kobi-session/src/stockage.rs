//! Stockage local de Kobi, source de vérité (KOB-141) : réunions,
//! conversations du quotidien et rappels, sous un dossier racine réglable.
//!
//! ```text
//! <racine>/reunions/AAAA-MM-JJ-titre/   reunion.json, transcript.jsonl, notes.md, compte-rendu.md
//! <racine>/conversations/AAAA-MM-JJ/    HHMMSS.jsonl (une conversation, un tour par ligne)
//! <racine>/rappels.json                 liste des rappels, réécrite en entier
//! ```
//!
//! Tout est en markdown ou en JSON lisible : le dossier se lit sans Kobi, se
//! sauvegarde et se synchronise comme n'importe quel dossier. Les écritures au
//! fil de l'eau (transcription, tours de conversation) sont une ligne par
//! appel, poussée sur le disque ; les autres passent par un fichier
//! temporaire renommé. Le Carnet (KOB-140) relit tout d'ici.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use kobi_stt::{Segment, Source};
use serde::{Deserialize, Serialize};

use crate::config::{Config, Publication, Retention};
use crate::donnees::{ErreurDonnees, Session};
use crate::reunion::{
    ecrire_atomique, lire_transcript, Date, Reunion, COMPTE_RENDU, NOTES, TRANSCRIPT,
};

/// Sous-dossier des réunions.
pub const REUNIONS: &str = "reunions";
/// Ancien nom du sous-dossier des réunions (avant KOB-141), renommé une fois.
pub const ANCIEN_REUNIONS: &str = "meetings";
/// Sous-dossier des conversations du quotidien.
pub const CONVERSATIONS: &str = "conversations";
/// Fichier des rappels.
pub const RAPPELS: &str = "rappels.json";
/// Méta d'une réunion, à côté de sa transcription.
pub const META: &str = "reunion.json";

/// Version du format écrit dans `reunion.json`.
pub const VERSION: u32 = 1;

/// Heure courante, en millisecondes depuis 1970 (UTC).
pub fn maintenant_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Date et heure locales d'un instant, d'après le fuseau du système.
pub fn heure_locale(ms: u64) -> (Date, u8, u8, u8) {
    let zone = jiff::tz::TimeZone::system();
    let instant =
        jiff::Timestamp::from_millisecond(ms as i64).unwrap_or(jiff::Timestamp::UNIX_EPOCH);
    let dt = instant.to_zoned(zone).datetime();
    (
        Date {
            annee: dt.year() as u16,
            mois: dt.month() as u8,
            jour: dt.day() as u8,
        },
        dt.hour() as u8,
        dt.minute() as u8,
        dt.second() as u8,
    )
}

/// Méta d'une réunion (`reunion.json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetaReunion {
    pub version: u32,
    pub titre: Option<String>,
    pub debut_ms: u64,
    pub fin_ms: Option<u64>,
    pub statut: Statut,
    /// Résultat de la publication par MCP, quand elle sera active.
    #[serde(default)]
    pub publication: Option<serde_json::Value>,
    /// Titre donné par Kobi d'après ce qui a été dit (KOB-223) ; `false` pour
    /// un titre donné à la main, que Kobi ne remplace jamais.
    #[serde(default)]
    pub titre_auto: bool,
    /// Titres automatiques déjà donnés : au plus [`TITRES_AUTO_MAX`].
    #[serde(default)]
    pub changements_auto: u8,
}

/// Titres automatiques au plus par réunion (le premier, puis un affinage).
pub const TITRES_AUTO_MAX: u8 = 2;

/// Qui donne un titre à une réunion ou à une conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origine {
    /// Kobi, d'après ce qui a été dit.
    Auto,
    /// L'utilisateur (app, voix, `kobi meeting rename`).
    Manuel,
}

/// Fichier des renommages, dans le dossier des réunions : ancien nom →
/// nouveau, pour que les liens vers un ancien nom mènent toujours à la
/// réunion.
const RENOMMAGES: &str = ".renommages.json";

fn noter_renommage(reunions: &Path, ancien: &str, nouveau: &str) -> io::Result<()> {
    let chemin = reunions.join(RENOMMAGES);
    let mut table: std::collections::BTreeMap<String, String> = fs::read(&chemin)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    table.insert(ancien.to_owned(), nouveau.to_owned());
    let json = serde_json::to_vec_pretty(&table).map_err(io::Error::other)?;
    ecrire_atomique(&chemin, &json)
}

/// Nom actuel d'une réunion renommée (en suivant les renommages successifs).
fn nom_actuel(reunions: &Path, nom: &str) -> Option<String> {
    let table: std::collections::BTreeMap<String, String> =
        serde_json::from_slice(&fs::read(reunions.join(RENOMMAGES)).ok()?).ok()?;
    let mut nom = table.get(nom)?.clone();
    for _ in 0..8 {
        match table.get(&nom) {
            Some(suivant) => nom = suivant.clone(),
            None => break,
        }
    }
    Some(nom)
}

/// Titre d'une réunion sans assez de parole pour être nommée :
/// « Réunion du 19/10 à 14 h » (KOB-223).
pub fn titre_par_defaut(debut_ms: u64) -> String {
    let (date, h, m, _) = heure_locale(debut_ms);
    let jour = format!("{:02}", date.jour);
    let mois = format!("{:02}", date.mois);
    let h = h.to_string();
    if m == 0 {
        kobi_i18n::tr(
            "session.stockage.titreParDefaut",
            &[("jour", &jour), ("mois", &mois), ("h", &h)],
        )
    } else {
        kobi_i18n::tr(
            "session.stockage.titreParDefautMinutes",
            &[
                ("jour", &jour),
                ("mois", &mois),
                ("h", &h),
                ("m", &format!("{m:02}")),
            ],
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Statut {
    EnCours,
    Terminee,
}

/// Réunion ouverte par le stockage : la transcription, les notes, le compte
/// rendu et la méta.
#[derive(Debug)]
pub struct ReunionOuverte {
    reunion: Reunion,
    meta: MetaReunion,
}

impl ReunionOuverte {
    pub fn reunion(&self) -> &Reunion {
        &self.reunion
    }

    pub fn dossier(&self) -> &Path {
        self.reunion.dossier()
    }

    pub fn nom(&self) -> &str {
        self.reunion.nom()
    }

    pub fn meta(&self) -> &MetaReunion {
        &self.meta
    }

    pub fn ecrire_segment(&mut self, segment: &Segment) -> io::Result<bool> {
        self.reunion.ecrire_segment(segment)
    }

    pub fn ecrire_notes(&self, notes: &str) -> io::Result<()> {
        self.reunion.ecrire_notes(notes)
    }

    pub fn ecrire_compte_rendu(&self, texte: &str) -> io::Result<()> {
        self.reunion.ecrire_compte_rendu(texte)
    }

    /// Donne un titre à la main (le dossier suit, voir [`Self::nommer`]).
    pub fn renommer_titre(&mut self, titre: &str) -> io::Result<()> {
        self.nommer(titre, Origine::Manuel).map(|_| ())
    }

    /// Donne un titre à la réunion et renomme son dossier en conséquence
    /// (KOB-223), sans perdre la transcription en cours d'écriture. Un titre
    /// automatique ne remplace jamais un titre donné à la main, et il n'y en
    /// a pas plus de [`TITRES_AUTO_MAX`]. Renvoie l'ancien nom du dossier
    /// s'il a changé ; `Ok(None)` si rien n'a changé.
    pub fn nommer(&mut self, titre: &str, origine: Origine) -> io::Result<Option<String>> {
        let titre = titre.trim();
        if titre.is_empty() {
            return Ok(None);
        }
        if origine == Origine::Auto {
            let manuel = self.meta.titre.is_some() && !self.meta.titre_auto;
            if manuel || self.meta.changements_auto >= TITRES_AUTO_MAX {
                return Ok(None);
            }
        }
        // Le titre ne change qu'une fois le dossier déplacé. Le titre et la
        // table des renommages s'écrivent chacun même si l'autre échoue : le
        // dossier a bougé, l'ancien nom doit mener à lui (KOB-266, KOB-292).
        let ancien = self.reunion.deplacer(titre)?;
        if origine == Origine::Auto {
            self.meta.changements_auto += 1;
        }
        self.meta.titre = Some(titre.to_owned());
        self.meta.titre_auto = origine == Origine::Auto;
        let meta = self.ecrire_meta();
        if let Some(ancien) = &ancien {
            if let Some(reunions) = self.dossier().parent() {
                if let Err(erreur) = noter_renommage(reunions, ancien, self.reunion.nom()) {
                    tracing::warn!(%erreur, ancien, "renommage de réunion non noté : l'ancien nom ne mènera plus à elle");
                }
            }
        }
        meta?;
        Ok(ancien)
    }

    /// Marque la réunion terminée, à l'heure `fin_ms`. Sans titre (pas assez
    /// de parole), elle prend le titre par défaut, sans renommer le dossier.
    pub fn terminer(&mut self, fin_ms: u64) -> io::Result<()> {
        self.meta.fin_ms = Some(fin_ms);
        self.meta.statut = Statut::Terminee;
        if self.meta.titre.is_none() {
            self.meta.titre = Some(titre_par_defaut(self.meta.debut_ms));
            self.meta.titre_auto = true;
        }
        self.ecrire_meta()
    }

    fn ecrire_meta(&self) -> io::Result<()> {
        let json = serde_json::to_vec_pretty(&self.meta).map_err(io::Error::other)?;
        ecrire_atomique(&self.dossier().join(META), &json)
    }
}

/// Qui parle dans une conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Qui {
    Moi,
    Kobi,
}

/// Un tour de conversation : une ligne de `HHMMSS.jsonl`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tour {
    pub at_ms: u64,
    pub qui: Qui,
    pub texte: String,
    /// Dit à la voix (sinon tapé, ou réponse en texte seul).
    #[serde(default)]
    pub voix: bool,
}

/// Conversation du quotidien ouverte : un fichier, un tour par ligne.
#[derive(Debug)]
pub struct Conversation {
    id: String,
    chemin: PathBuf,
    fichier: Option<File>,
}

impl Conversation {
    /// Identifiant `AAAA-MM-JJ/HHMMSS`, à passer à [`Stockage::lire_conversation`].
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn chemin(&self) -> &Path {
        &self.chemin
    }

    /// Ajoute un tour, écrit d'un seul appel et poussé sur le disque. Le
    /// fichier n'est créé qu'au premier tour : une conversation vide ne
    /// laisse rien.
    pub fn ajouter(&mut self, qui: Qui, texte: &str, voix: bool) -> io::Result<()> {
        self.ajouter_tour(&Tour {
            at_ms: maintenant_ms(),
            qui,
            texte: texte.to_owned(),
            voix,
        })
    }

    pub fn ajouter_tour(&mut self, tour: &Tour) -> io::Result<()> {
        if self.fichier.is_none() {
            if let Some(parent) = self.chemin.parent() {
                fs::create_dir_all(parent)?;
            }
            let f = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.chemin)?;
            self.fichier = Some(f);
        }
        let f = self.fichier.as_mut().expect("ouvert juste au-dessus");
        let mut ligne = serde_json::to_vec(tour).map_err(io::Error::other)?;
        ligne.push(b'\n');
        f.write_all(&ligne)?;
        f.sync_data()
    }
}

/// Un rappel (« Kobi, rappelle-moi … », KOB-145).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rappel {
    pub id: String,
    pub texte: String,
    pub echeance_ms: Option<u64>,
    pub cree_ms: u64,
    #[serde(default)]
    pub fait: bool,
    /// D'où vient le rappel : nom de réunion ou id de conversation.
    #[serde(default)]
    pub source: Option<String>,
}

/// Réunion vue du Carnet : sa méta et ce qu'on montre dans la liste.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResumeReunion {
    pub nom: String,
    pub date: Option<String>,
    pub titre: Option<String>,
    pub debut_ms: Option<u64>,
    pub fin_ms: Option<u64>,
    pub en_cours: bool,
    /// Durée transcrite, en secondes (fin du dernier segment).
    pub duree_s: f64,
    pub a_compte_rendu: bool,
    /// Premières lignes du résumé, ou des notes.
    pub apercu: String,
}

/// Compte rendu découpé par ses titres de section.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct CompteRendu {
    pub resume: String,
    pub decisions: String,
    pub actions: String,
    pub questions: String,
    /// Sections non reconnues (« Interpellations »…), dans l'ordre.
    pub autres: Vec<(String, String)>,
    /// Le texte complet, tel qu'écrit.
    pub brut: String,
}

impl CompteRendu {
    pub fn depuis_markdown(texte: &str) -> Self {
        let mut cr = CompteRendu {
            brut: texte.to_owned(),
            ..Default::default()
        };
        let mut titre: Option<String> = None;
        let mut corps = String::new();
        let ranger = |titre: Option<String>, corps: &mut String, cr: &mut CompteRendu| {
            let texte = corps.trim().to_owned();
            corps.clear();
            let Some(titre) = titre else { return };
            match cle_de_section(&titre).as_str() {
                "resume" | "synthese" | "summary" => cr.resume = texte,
                "decisions" | "decision" => cr.decisions = texte,
                "actions" | "actions-a-faire" | "a-faire" | "todo" => cr.actions = texte,
                "questions-ouvertes" | "questions" | "open-questions" => cr.questions = texte,
                _ => cr.autres.push((titre, texte)),
            }
        };
        for ligne in texte.lines() {
            if let Some(t) = ligne.strip_prefix("## ") {
                ranger(titre.take(), &mut corps, &mut cr);
                titre = Some(t.trim().to_owned());
            } else if titre.is_some() {
                corps.push_str(ligne);
                corps.push('\n');
            }
        }
        ranger(titre, &mut corps, &mut cr);
        cr
    }
}

fn cle_de_section(titre: &str) -> String {
    crate::reunion::nettoyer_titre(titre)
}

/// Réunion relue en entier, pour la vue du Carnet.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReunionLue {
    pub resume: ResumeReunion,
    pub dossier: PathBuf,
    pub segments: Vec<Segment>,
    pub notes: String,
    pub compte_rendu: Option<CompteRendu>,
}

/// Conversation vue du Carnet.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResumeConversation {
    pub id: String,
    pub date: String,
    pub debut_ms: Option<u64>,
    pub fin_ms: Option<u64>,
    pub tours: usize,
    /// Première question.
    pub apercu: String,
    /// Titre donné à la main dans l'app (KOB-223), à côté de la
    /// conversation (`HHMMSS.titre`).
    pub titre: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConversationLue {
    pub resume: ResumeConversation,
    pub chemin: PathBuf,
    pub tours: Vec<Tour>,
}

/// Où se trouve un mot cherché.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Trouve {
    Reunion {
        nom: String,
        titre: Option<String>,
        /// `transcription`, `notes` ou `compte-rendu`.
        partie: &'static str,
        /// Début du segment, pour la transcription.
        start: Option<f64>,
        extrait: String,
    },
    Conversation {
        id: String,
        at_ms: u64,
        extrait: String,
    },
}

/// Document prêt à publier par MCP (KOB-141) : titre et markdown complet.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DocumentAPublier {
    pub titre: String,
    pub markdown: String,
}

/// Où en est la publication d'un type de document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "etat", rename_all = "kebab-case")]
pub enum EtatPublication {
    /// Aucune destination réglée : tout reste en local.
    Aucune,
    /// Destination réglée : le bouton « Publier » du Carnet y envoie le
    /// compte rendu, après confirmation (KOB-75).
    Reglee { mcp: String, destination: String },
}

pub fn etat_publication(p: &Publication) -> EtatPublication {
    if p.est_reglee() {
        EtatPublication::Reglee {
            mcp: p.mcp.trim().to_owned(),
            destination: p.destination.trim().to_owned(),
        }
    } else {
        EtatPublication::Aucune
    }
}

/// Le stockage : un dossier racine et ce qu'on y range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stockage {
    racine: PathBuf,
}

impl Stockage {
    pub fn new(racine: impl Into<PathBuf>) -> Self {
        Self {
            racine: racine.into(),
        }
    }

    /// Stockage réglé dans la config (`[stockage] dossier`, défaut `~/Kobi`).
    pub fn depuis_config(config: &Config) -> Option<Self> {
        config.stockage.racine().map(Self::new)
    }

    pub fn racine(&self) -> &Path {
        &self.racine
    }

    pub fn dossier_reunions(&self) -> PathBuf {
        self.racine.join(REUNIONS)
    }

    pub fn dossier_conversations(&self) -> PathBuf {
        self.racine.join(CONVERSATIONS)
    }

    pub fn fichier_rappels(&self) -> PathBuf {
        self.racine.join(RAPPELS)
    }

    /// Crée la racine et ses sous-dossiers ; renomme l'ancien `meetings/` en
    /// `reunions/` s'il est seul. Retourne `true` si un renommage a eu lieu.
    pub fn preparer(&self) -> io::Result<bool> {
        fs::create_dir_all(&self.racine)?;
        let ancien = self.racine.join(ANCIEN_REUNIONS);
        let nouveau = self.dossier_reunions();
        let migre = if ancien.is_dir() && !nouveau.exists() {
            fs::rename(&ancien, &nouveau)?;
            true
        } else {
            false
        };
        fs::create_dir_all(&nouveau)?;
        fs::create_dir_all(self.dossier_conversations())?;
        Ok(migre)
    }

    // ----- Écriture -----

    /// Crée `reunions/AAAA-MM-JJ-titre/` (date locale de `debut_ms`), sa
    /// transcription vide et son `reunion.json`.
    pub fn nouvelle_reunion(
        &self,
        debut_ms: u64,
        titre: Option<&str>,
    ) -> io::Result<ReunionOuverte> {
        let (date, ..) = heure_locale(debut_ms);
        let reunion = Reunion::creer(&self.dossier_reunions(), date, titre)?;
        let ouverte = ReunionOuverte {
            reunion,
            meta: MetaReunion {
                version: VERSION,
                titre: titre
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .map(str::to_owned),
                debut_ms,
                fin_ms: None,
                statut: Statut::EnCours,
                publication: None,
                titre_auto: false,
                changements_auto: 0,
            },
        };
        ouverte.ecrire_meta()?;
        Ok(ouverte)
    }

    /// Rouvre une réunion (reprise après un arrêt brutal).
    pub fn rouvrir_reunion(&self, nom: &str) -> Result<ReunionOuverte, ErreurDonnees> {
        let dossier = self.dossier_de_reunion(nom)?;
        let meta = lire_meta(&dossier).unwrap_or_else(|| meta_par_defaut(&dossier));
        Ok(ReunionOuverte {
            reunion: Reunion::ouvrir(dossier)?,
            meta,
        })
    }

    /// Nouvelle conversation datée de `debut_ms` (heure locale). Rien n'est
    /// écrit avant le premier tour.
    pub fn nouvelle_conversation(&self, debut_ms: u64) -> Conversation {
        let (date, h, m, s) = heure_locale(debut_ms);
        let jour = self.dossier_conversations().join(date.to_string());
        let mut n = 1;
        loop {
            let base = format!("{h:02}{m:02}{s:02}");
            let nom = if n == 1 { base } else { format!("{base}-{n}") };
            let chemin = jour.join(format!("{nom}.jsonl"));
            if !chemin.exists() {
                return Conversation {
                    id: format!("{date}/{nom}"),
                    chemin,
                    fichier: None,
                };
            }
            n += 1;
        }
    }

    // ----- Lecture -----

    /// Réunions, la plus récente d'abord.
    pub fn reunions(&self) -> io::Result<Vec<ResumeReunion>> {
        let mut liste = Vec::new();
        for dossier in sous_dossiers(&self.dossier_reunions())? {
            liste.push(resume_reunion(&dossier));
        }
        liste.sort_by(|a, b| (b.debut_ms, &b.date, &b.nom).cmp(&(a.debut_ms, &a.date, &a.nom)));
        Ok(liste)
    }

    pub fn lire_reunion(&self, nom: &str) -> Result<ReunionLue, ErreurDonnees> {
        let dossier = self.dossier_de_reunion(nom)?;
        let segments = match lire_transcript(&dossier.join(TRANSCRIPT)) {
            Ok(r) => r.segments,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e.into()),
        };
        let notes = lire_texte(&dossier.join(NOTES));
        let compte_rendu = fs::read_to_string(dossier.join(COMPTE_RENDU))
            .ok()
            .map(|t| CompteRendu::depuis_markdown(&t));
        Ok(ReunionLue {
            resume: resume_reunion(&dossier),
            dossier,
            segments,
            notes,
            compte_rendu,
        })
    }

    /// Conversations, la plus récente d'abord.
    pub fn conversations(&self) -> io::Result<Vec<ResumeConversation>> {
        let mut liste = Vec::new();
        for jour in sous_dossiers(&self.dossier_conversations())? {
            let Some(date) = nom_de(&jour) else { continue };
            if Date::depuis_nom_de_dossier(&date).is_none() {
                continue;
            }
            for entree in fs::read_dir(&jour)? {
                let chemin = entree?.path();
                if chemin.extension().is_some_and(|e| e == "jsonl") {
                    let tours = lire_tours(&chemin)?;
                    liste.push(resume_conversation(&date, &chemin, &tours));
                }
            }
        }
        liste.sort_by(|a, b| b.id.cmp(&a.id));
        Ok(liste)
    }

    pub fn lire_conversation(&self, id: &str) -> Result<ConversationLue, ErreurDonnees> {
        let chemin = self.chemin_de_conversation(id)?;
        let tours = lire_tours(&chemin)?;
        let date = id.split('/').next().unwrap_or_default();
        Ok(ConversationLue {
            resume: resume_conversation(date, &chemin, &tours),
            chemin,
            tours,
        })
    }

    /// Recherche plein texte (sans casse ni accents) dans les transcriptions,
    /// notes, comptes rendus et conversations. Au plus `max` résultats, les
    /// plus récents d'abord.
    pub fn chercher(&self, requete: &str, max: usize) -> io::Result<Vec<Trouve>> {
        let aiguille = normaliser(requete.trim());
        let mut trouves = Vec::new();
        if aiguille.is_empty() {
            return Ok(trouves);
        }
        let plein = |t: &Vec<Trouve>| t.len() >= max;
        for r in self.reunions()? {
            if plein(&trouves) {
                break;
            }
            let Ok(lue) = self.lire_reunion(&r.nom) else {
                continue;
            };
            let mut pousse = |partie, start, texte: &str| {
                trouves.push(Trouve::Reunion {
                    nom: r.nom.clone(),
                    titre: r.titre.clone(),
                    partie,
                    start,
                    extrait: extrait(texte, &aiguille),
                });
            };
            if let Some(cr) = &lue.compte_rendu {
                for ligne in cr
                    .brut
                    .lines()
                    .filter(|l| normaliser(l).contains(&aiguille))
                {
                    pousse("compte-rendu", None, ligne);
                }
            }
            for ligne in lue
                .notes
                .lines()
                .filter(|l| normaliser(l).contains(&aiguille))
            {
                pousse("notes", None, ligne);
            }
            for s in lue
                .segments
                .iter()
                .filter(|s| normaliser(&s.text).contains(&aiguille))
            {
                pousse("transcription", Some(s.start), &s.text);
            }
        }
        for c in self.conversations()? {
            if plein(&trouves) {
                break;
            }
            let Ok(lue) = self.lire_conversation(&c.id) else {
                continue;
            };
            for t in lue
                .tours
                .iter()
                .filter(|t| normaliser(&t.texte).contains(&aiguille))
            {
                trouves.push(Trouve::Conversation {
                    id: c.id.clone(),
                    at_ms: t.at_ms,
                    extrait: extrait(&t.texte, &aiguille),
                });
            }
        }
        trouves.truncate(max);
        Ok(trouves)
    }

    // ----- Rappels -----

    /// Rappels, à faire d'abord, par échéance.
    pub fn rappels(&self) -> io::Result<Vec<Rappel>> {
        let mut liste: Vec<Rappel> = match fs::read(self.fichier_rappels()) {
            Ok(octets) => serde_json::from_slice(&octets).map_err(io::Error::other)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e),
        };
        liste.sort_by_key(|r| (r.fait, r.echeance_ms.unwrap_or(u64::MAX), r.cree_ms));
        Ok(liste)
    }

    fn ecrire_rappels(&self, rappels: &[Rappel]) -> io::Result<()> {
        fs::create_dir_all(&self.racine)?;
        let json = serde_json::to_vec_pretty(rappels).map_err(io::Error::other)?;
        ecrire_atomique(&self.fichier_rappels(), &json)
    }

    pub fn ajouter_rappel(
        &self,
        texte: &str,
        echeance_ms: Option<u64>,
        source: Option<&str>,
    ) -> io::Result<Rappel> {
        let mut rappels = self.rappels()?;
        let cree_ms = maintenant_ms();
        let mut id = format!("r{cree_ms}");
        let mut n = 1;
        while rappels.iter().any(|r| r.id == id) {
            n += 1;
            id = format!("r{cree_ms}-{n}");
        }
        let rappel = Rappel {
            id,
            texte: texte.trim().to_owned(),
            echeance_ms,
            cree_ms,
            fait: false,
            source: source.map(str::to_owned),
        };
        rappels.push(rappel.clone());
        self.ecrire_rappels(&rappels)?;
        Ok(rappel)
    }

    /// Marque un rappel fait ; `false` s'il n'existe pas.
    pub fn marquer_fait(&self, id: &str) -> io::Result<bool> {
        let mut rappels = self.rappels()?;
        let Some(r) = rappels.iter_mut().find(|r| r.id == id) else {
            return Ok(false);
        };
        r.fait = true;
        self.ecrire_rappels(&rappels)?;
        Ok(true)
    }

    pub fn supprimer_rappel(&self, id: &str) -> io::Result<bool> {
        let mut rappels = self.rappels()?;
        let avant = rappels.len();
        rappels.retain(|r| r.id != id);
        if rappels.len() == avant {
            return Ok(false);
        }
        self.ecrire_rappels(&rappels)?;
        Ok(true)
    }

    /// Rappels pas faits dont l'échéance est passée à `maintenant_ms`.
    pub fn rappels_dus(&self, maintenant_ms: u64) -> io::Result<Vec<Rappel>> {
        Ok(self
            .rappels()?
            .into_iter()
            .filter(|r| !r.fait && r.echeance_ms.is_some_and(|e| e <= maintenant_ms))
            .collect())
    }

    // ----- Suppression, purge, publication -----

    pub fn supprimer_reunion(
        &self,
        nom: &str,
        session: Option<&Session>,
    ) -> Result<(), ErreurDonnees> {
        crate::donnees::supprimer_reunion(&self.dossier_reunions(), nom, session)
    }

    /// Supprime une conversation ; le dossier du jour part s'il est vide.
    pub fn supprimer_conversation(&self, id: &str) -> Result<(), ErreurDonnees> {
        let chemin = self.chemin_de_conversation(id)?;
        fs::remove_file(&chemin)?;
        if let Some(jour) = chemin.parent() {
            let _ = fs::remove_dir(jour); // échoue s'il reste des fichiers : voulu
        }
        Ok(())
    }

    /// Applique les deux durées de conservation. Retourne ce qui est parti.
    pub fn purger(
        &self,
        reunions: Retention,
        conversations: Retention,
        aujourdhui: Date,
        session: Option<&Session>,
    ) -> Result<Vec<PathBuf>, ErreurDonnees> {
        let mut partis =
            crate::donnees::purger(&self.dossier_reunions(), reunions, aujourdhui, session)?;
        partis.extend(crate::donnees::purger(
            &self.dossier_conversations(),
            conversations,
            aujourdhui,
            None,
        )?);
        Ok(partis)
    }

    /// Document markdown d'une réunion, pour l'export : en-tête, compte
    /// rendu (ou notes), puis transcription horodatée.
    pub fn document_reunion(&self, nom: &str) -> Result<DocumentAPublier, ErreurDonnees> {
        self.document_reunion_avec(nom, true)
    }

    /// Document à publier par MCP (KOB-75) : en-tête et compte rendu (ou
    /// notes), sans la transcription.
    pub fn compte_rendu_a_publier(&self, nom: &str) -> Result<DocumentAPublier, ErreurDonnees> {
        self.document_reunion_avec(nom, false)
    }

    fn document_reunion_avec(
        &self,
        nom: &str,
        transcription: bool,
    ) -> Result<DocumentAPublier, ErreurDonnees> {
        let lue = self.lire_reunion(nom)?;
        let titre = match (&lue.resume.titre, &lue.resume.date) {
            (Some(t), Some(d)) => format!("{t} — {d}"),
            (Some(t), None) => t.clone(),
            (None, Some(d)) => kobi_i18n::tr("session.export.reunionDu", &[("date", d)]),
            (None, None) => lue.resume.nom.clone(),
        };
        let mut md = format!("# {titre}\n\n");
        match &lue.compte_rendu {
            Some(cr) => md.push_str(cr.brut.trim()),
            None if !lue.notes.trim().is_empty() => {
                md.push_str(&format!(
                    "## {}\n\n",
                    kobi_i18n::tr("session.export.notes", &[])
                ));
                md.push_str(lue.notes.trim());
            }
            None => md.push_str(&format!(
                "_{}_",
                kobi_i18n::tr("session.export.pasDeCompteRendu", &[])
            )),
        }
        if !transcription {
            md.push('\n');
            return Ok(DocumentAPublier {
                titre,
                markdown: md,
            });
        }
        md.push_str(&format!(
            "\n\n## {}\n\n",
            kobi_i18n::tr("session.export.transcription", &[])
        ));
        let moi = kobi_i18n::tr("session.export.moi", &[]);
        let eux = kobi_i18n::tr("session.export.eux", &[]);
        for s in &lue.segments {
            let qui = match s.source {
                Source::Moi => &moi,
                Source::Eux => &eux,
            };
            md.push_str(&format!(
                "- `{}` **{qui}** : {}\n",
                horodatage(s.start),
                s.text.trim()
            ));
        }
        Ok(DocumentAPublier {
            titre,
            markdown: md,
        })
    }

    /// Document markdown d'une conversation.
    pub fn document_conversation(&self, id: &str) -> Result<DocumentAPublier, ErreurDonnees> {
        let lue = self.lire_conversation(id)?;
        let titre = kobi_i18n::tr(
            "session.export.conversationDu",
            &[("date", &lue.resume.date)],
        );
        let mut md = format!("# {titre}\n\n");
        let moi = kobi_i18n::tr("session.export.moi", &[]);
        for t in &lue.tours {
            let (_, h, m, _) = heure_locale(t.at_ms);
            let qui = match t.qui {
                Qui::Moi => moi.as_str(),
                Qui::Kobi => "Kobi",
            };
            md.push_str(&format!(
                "**{qui}** ({h:02}:{m:02}) : {}\n\n",
                t.texte.trim()
            ));
        }
        Ok(DocumentAPublier {
            titre,
            markdown: md,
        })
    }

    // ----- Chemins -----

    fn dossier_de_reunion(&self, nom: &str) -> Result<PathBuf, ErreurDonnees> {
        if !nom_simple(nom) {
            return Err(ErreurDonnees::Nom(nom.to_owned()));
        }
        let dossier = self.dossier_reunions().join(nom);
        if !dossier.is_dir() {
            // Renommée depuis (KOB-223) : l'ancien nom mène à la réunion.
            if let Some(actuel) = nom_actuel(&self.dossier_reunions(), nom) {
                let dossier = self.dossier_reunions().join(actuel);
                if dossier.is_dir() {
                    return Ok(dossier);
                }
            }
            return Err(ErreurDonnees::Introuvable(nom.to_owned()));
        }
        Ok(dossier)
    }

    /// Donne un titre à une conversation (KOB-223), à côté de son fichier ;
    /// un titre vide le retire.
    pub fn renommer_conversation(&self, id: &str, titre: &str) -> Result<(), ErreurDonnees> {
        let chemin = self.chemin_de_conversation(id)?;
        let fichier = chemin.with_extension("titre");
        let titre = titre.trim();
        if titre.is_empty() {
            match fs::remove_file(&fichier) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e.into()),
                _ => return Ok(()),
            }
        }
        ecrire_atomique(&fichier, titre.as_bytes())?;
        Ok(())
    }

    /// Renomme une réunion terminée à la main (app, `kobi meeting rename`) :
    /// titre manuel, dossier renommé. Renvoie le nouveau nom du dossier. La
    /// réunion en cours se renomme par l'écrivain ([`ReunionOuverte::nommer`]).
    pub fn renommer_reunion(&self, nom: &str, titre: &str) -> Result<String, ErreurDonnees> {
        let dossier = self.dossier_de_reunion(nom)?;
        let mut ouverte = ReunionOuverte {
            meta: lire_meta(&dossier).unwrap_or_else(|| meta_par_defaut(&dossier)),
            reunion: Reunion::ouvrir(dossier)?,
        };
        ouverte.nommer(titre, Origine::Manuel)?;
        Ok(ouverte.nom().to_owned())
    }

    fn chemin_de_conversation(&self, id: &str) -> Result<PathBuf, ErreurDonnees> {
        let mut parties = id.split('/');
        let (Some(jour), Some(nom), None) = (parties.next(), parties.next(), parties.next()) else {
            return Err(ErreurDonnees::Nom(id.to_owned()));
        };
        if !nom_simple(jour) || !nom_simple(nom) || Date::depuis_nom_de_dossier(jour).is_none() {
            return Err(ErreurDonnees::Nom(id.to_owned()));
        }
        let chemin = self
            .dossier_conversations()
            .join(jour)
            .join(format!("{nom}.jsonl"));
        if !chemin.is_file() {
            return Err(ErreurDonnees::Introuvable(id.to_owned()));
        }
        Ok(chemin)
    }
}

fn nom_simple(nom: &str) -> bool {
    !nom.is_empty() && nom != "." && nom != ".." && !nom.contains(['/', '\\', ':'])
}

fn nom_de(chemin: &Path) -> Option<String> {
    chemin.file_name()?.to_str().map(str::to_owned)
}

fn sous_dossiers(dossier: &Path) -> io::Result<Vec<PathBuf>> {
    let entrees = match fs::read_dir(dossier) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut v = Vec::new();
    for e in entrees {
        let e = e?;
        if e.file_type()?.is_dir() {
            v.push(e.path());
        }
    }
    Ok(v)
}

fn lire_texte(chemin: &Path) -> String {
    fs::read_to_string(chemin).unwrap_or_default()
}

fn lire_meta(dossier: &Path) -> Option<MetaReunion> {
    serde_json::from_slice(&fs::read(dossier.join(META)).ok()?).ok()
}

/// Méta d'une réunion écrite avant KOB-141 (pas de `reunion.json`).
fn meta_par_defaut(dossier: &Path) -> MetaReunion {
    let fini = dossier.join(COMPTE_RENDU).is_file();
    MetaReunion {
        version: VERSION,
        titre: None,
        debut_ms: 0,
        fin_ms: None,
        statut: if fini {
            Statut::Terminee
        } else {
            Statut::EnCours
        },
        publication: None,
        titre_auto: false,
        changements_auto: 0,
    }
}

fn resume_reunion(dossier: &Path) -> ResumeReunion {
    let nom = nom_de(dossier).unwrap_or_default();
    let meta = lire_meta(dossier);
    let date = Date::depuis_nom_de_dossier(&nom).map(|d| d.to_string());
    let duree_s = lire_transcript(&dossier.join(TRANSCRIPT))
        .ok()
        .and_then(|r| r.segments.last().map(|s| s.end))
        .unwrap_or(0.0);
    let cr = fs::read_to_string(dossier.join(COMPTE_RENDU)).ok();
    let apercu_source = cr
        .as_deref()
        .map(|t| CompteRendu::depuis_markdown(t).resume)
        .filter(|r| !r.is_empty())
        .unwrap_or_else(|| lire_texte(&dossier.join(NOTES)));
    let titre = meta.as_ref().and_then(|m| m.titre.clone()).or_else(|| {
        // Titre déduit du nom de dossier, pour les réunions sans méta.
        nom.get(11..)
            .filter(|t| !t.is_empty() && *t != crate::reunion::TITRE_PAR_DEFAUT)
            .map(|t| t.replace('-', " "))
    });
    ResumeReunion {
        date,
        titre,
        debut_ms: meta.as_ref().map(|m| m.debut_ms).filter(|&d| d > 0),
        fin_ms: meta.as_ref().and_then(|m| m.fin_ms),
        en_cours: meta.as_ref().is_some_and(|m| m.statut == Statut::EnCours),
        duree_s,
        a_compte_rendu: cr.is_some(),
        apercu: apercu(&apercu_source),
        nom,
    }
}

fn resume_conversation(date: &str, chemin: &Path, tours: &[Tour]) -> ResumeConversation {
    let nom = chemin
        .file_stem()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    ResumeConversation {
        id: format!("{date}/{nom}"),
        date: date.to_owned(),
        debut_ms: tours.first().map(|t| t.at_ms),
        fin_ms: tours.last().map(|t| t.at_ms),
        tours: tours.len(),
        apercu: tours
            .iter()
            .find(|t| t.qui == Qui::Moi)
            .or(tours.first())
            .map(|t| apercu(&t.texte))
            .unwrap_or_default(),
        titre: fs::read_to_string(chemin.with_extension("titre"))
            .ok()
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty()),
    }
}

/// Tours lisibles d'une conversation ; une ligne illisible est sautée.
fn lire_tours(chemin: &Path) -> io::Result<Vec<Tour>> {
    let mut tours = Vec::new();
    for ligne in BufReader::new(File::open(chemin)?).split(b'\n') {
        let ligne = ligne?;
        if let Ok(t) = serde_json::from_slice(&ligne) {
            tours.push(t);
        }
    }
    Ok(tours)
}

/// Deux premières lignes utiles, sans marques de liste, 160 caractères au plus.
fn apercu(texte: &str) -> String {
    let lignes: Vec<&str> = texte
        .lines()
        .map(|l| l.trim().trim_start_matches(['-', '*', '#', ' ']).trim())
        .filter(|l| !l.is_empty())
        .take(2)
        .collect();
    tronquer(&lignes.join(" "), 160)
}

fn tronquer(texte: &str, max: usize) -> String {
    if texte.chars().count() <= max {
        return texte.to_owned();
    }
    let mut s: String = texte.chars().take(max - 1).collect();
    s.push('…');
    s
}

/// Minuscules sans accents, caractère par caractère (même longueur en
/// caractères que l'original pour les lettres latines courantes).
fn normaliser(texte: &str) -> String {
    texte
        .chars()
        .map(|c| match c {
            'à' | 'á' | 'â' | 'ä' | 'ã' | 'å' | 'À' | 'Á' | 'Â' | 'Ä' | 'Ã' | 'Å' => {
                'a'
            }
            'ç' | 'Ç' => 'c',
            'è' | 'é' | 'ê' | 'ë' | 'È' | 'É' | 'Ê' | 'Ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' | 'Ì' | 'Í' | 'Î' | 'Ï' => 'i',
            'ñ' | 'Ñ' => 'n',
            'ò' | 'ó' | 'ô' | 'ö' | 'õ' | 'Ò' | 'Ó' | 'Ô' | 'Ö' | 'Õ' => 'o',
            'ù' | 'ú' | 'û' | 'ü' | 'Ù' | 'Ú' | 'Û' | 'Ü' => 'u',
            'ý' | 'ÿ' | 'Ý' => 'y',
            c => c.to_lowercase().next().unwrap_or(c),
        })
        .collect()
}

/// Extrait de ~120 caractères autour de la première occurrence.
fn extrait(texte: &str, aiguille: &str) -> String {
    let chars: Vec<char> = texte.trim().chars().collect();
    let norm: Vec<char> = normaliser(&chars.iter().collect::<String>())
        .chars()
        .collect();
    let cible: Vec<char> = aiguille.chars().collect();
    let pos = norm
        .windows(cible.len().max(1))
        .position(|w| w == cible.as_slice())
        .unwrap_or(0);
    let debut = pos.saturating_sub(50);
    let fin = (pos + cible.len() + 70).min(chars.len());
    let mut s: String = chars[debut..fin].iter().collect();
    if debut > 0 {
        s.insert(0, '…');
    }
    if fin < chars.len() {
        s.push('…');
    }
    s
}

/// `MM:SS` ou `H:MM:SS`.
pub fn horodatage(secondes: f64) -> String {
    let t = secondes.max(0.0) as u64;
    let (h, m, s) = (t / 3600, (t / 60) % 60, t % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-10-18 09:30:00 UTC.
    const T0: u64 = 1_792_315_800_000;

    fn segment(start: f64, source: Source, text: &str) -> Segment {
        Segment {
            start,
            end: start + 2.0,
            source,
            text: text.into(),
            is_final: true,
        }
    }

    fn stockage() -> (tempfile::TempDir, Stockage) {
        let dir = tempfile::tempdir().unwrap();
        let st = Stockage::new(dir.path().join("Kobi"));
        st.preparer().unwrap();
        (dir, st)
    }

    /// KOB-223 : le titre automatique renomme le dossier pendant
    /// l'écriture, sans perdre de segment ; l'ancien nom mène toujours à la
    /// réunion.
    #[test]
    fn titre_auto_renomme_le_dossier_sans_perte() {
        let (_d, st) = stockage();
        let mut r = st.nouvelle_reunion(T0, None).unwrap();
        assert_eq!(r.nom(), "2026-10-18-reunion");
        r.ecrire_segment(&segment(
            0.0,
            Source::Moi,
            "On parle du budget du séminaire",
        ))
        .unwrap();
        let ancien = r
            .nommer("Budget du séminaire d'Annecy", Origine::Auto)
            .unwrap();
        assert_eq!(ancien.as_deref(), Some("2026-10-18-reunion"));
        assert_eq!(r.nom(), "2026-10-18-budget-du-seminaire-d-annecy");
        r.ecrire_segment(&segment(3.0, Source::Eux, "Le devis arrive vendredi"))
            .unwrap();
        r.ecrire_notes("- budget\n").unwrap();
        let lue = st.lire_reunion(r.nom()).unwrap();
        assert_eq!(lue.segments.len(), 2, "aucun segment perdu");
        assert_eq!(
            lue.resume.titre.as_deref(),
            Some("Budget du séminaire d'Annecy")
        );
        assert!(r.meta().titre_auto);
        // L'ancien nom (lien de l'app ouvert avant) mène à la réunion.
        let par_ancien = st.lire_reunion("2026-10-18-reunion").unwrap();
        assert_eq!(par_ancien.dossier, lue.dossier);
        assert!(!st.dossier_reunions().join("2026-10-18-reunion").exists());
    }

    /// KOB-223 : un titre donné à la main n'est jamais remplacé ; au plus
    /// deux titres automatiques ; conflit de noms géré.
    #[test]
    fn titre_manuel_jamais_ecrase_et_conflits() {
        let (_d, st) = stockage();
        let mut r = st.nouvelle_reunion(T0, None).unwrap();
        r.nommer("Point équipe", Origine::Manuel).unwrap();
        assert_eq!(r.nommer("Autre chose", Origine::Auto).unwrap(), None);
        assert_eq!(r.meta().titre.as_deref(), Some("Point équipe"));
        assert!(!r.meta().titre_auto);
        assert_eq!(r.nom(), "2026-10-18-point-equipe");

        let mut r2 = st.nouvelle_reunion(T0 + 3_600_000, None).unwrap();
        r2.nommer("Point équipe", Origine::Auto).unwrap();
        assert_eq!(r2.nom(), "2026-10-18-point-equipe-2", "conflit : suffixe");
        r2.nommer("Budget", Origine::Auto).unwrap();
        assert_eq!(r2.nommer("Troisième", Origine::Auto).unwrap(), None);
        assert_eq!(r2.meta().titre.as_deref(), Some("Budget"));
        // La main reprend toujours la main.
        r2.nommer("Budget final", Origine::Manuel).unwrap();
        assert_eq!(r2.nom(), "2026-10-18-budget-final");
    }

    /// KOB-266 : un nombre à la fin de l'ancien titre n'est pas un suffixe
    /// de conflit ; un vrai suffixe de conflit reste tant que le nom est pris.
    #[test]
    fn nombre_du_titre_n_est_pas_un_suffixe_de_conflit() {
        let (_d, st) = stockage();
        let mut r = st.nouvelle_reunion(T0, None).unwrap();
        r.nommer("Budget 2027", Origine::Manuel).unwrap();
        assert_eq!(r.nom(), "2026-10-18-budget-2027");
        r.nommer("Budget", Origine::Manuel).unwrap();
        assert_eq!(r.nom(), "2026-10-18-budget", "le dossier suit le titre");

        let mut r2 = st.nouvelle_reunion(T0 + 3_600_000, None).unwrap();
        r2.nommer("Budget", Origine::Manuel).unwrap();
        assert_eq!(r2.nom(), "2026-10-18-budget-2");
        // Même titre redonné : « budget » est toujours pris, rien ne bouge.
        assert_eq!(r2.nommer("Budget", Origine::Manuel).unwrap(), None);
        assert_eq!(r2.nom(), "2026-10-18-budget-2");
    }

    /// KOB-292 : « Budget 2027 » renommé « Budget » alors qu'une autre
    /// réunion occupe « budget » : le dossier devient « budget-2 », il ne
    /// garde pas « budget-2027 », qui venait de l'ancien titre.
    #[test]
    fn nombre_du_titre_n_est_pas_un_suffixe_meme_si_le_nom_est_pris() {
        let (_d, st) = stockage();
        let mut autre = st.nouvelle_reunion(T0, None).unwrap();
        autre.nommer("Budget", Origine::Manuel).unwrap();
        assert_eq!(autre.nom(), "2026-10-18-budget");
        let mut r = st.nouvelle_reunion(T0 + 3_600_000, None).unwrap();
        r.nommer("Budget 2027", Origine::Manuel).unwrap();
        assert_eq!(r.nom(), "2026-10-18-budget-2027");
        assert_eq!(
            r.nommer("Budget", Origine::Manuel).unwrap().as_deref(),
            Some("2026-10-18-budget-2027")
        );
        assert_eq!(r.nom(), "2026-10-18-budget-2");
    }

    /// KOB-301 : « Budget 2 » dans `budget-2` (« budget » pris), renommé
    /// « Budget » : son nom est déjà celui que Kobi choisirait, rien ne bouge.
    #[test]
    fn suffixe_valide_pour_le_nouveau_titre_garde() {
        let (_d, st) = stockage();
        let mut autre = st.nouvelle_reunion(T0, None).unwrap();
        autre.nommer("Budget", Origine::Manuel).unwrap();
        let mut r = st.nouvelle_reunion(T0 + 3_600_000, None).unwrap();
        r.nommer("Budget 2", Origine::Manuel).unwrap();
        assert_eq!(r.nom(), "2026-10-18-budget-2");
        assert_eq!(r.nommer("Budget", Origine::Manuel).unwrap(), None);
        assert_eq!(r.nom(), "2026-10-18-budget-2");
        assert_eq!(r.meta().titre.as_deref(), Some("Budget"));
    }

    /// KOB-292 : `reunion.json` illisible en écriture après le déplacement
    /// du dossier : l'erreur remonte, mais l'ancien nom est quand même noté
    /// et mène à la réunion.
    #[test]
    fn ancien_nom_note_meme_si_le_titre_n_est_pas_ecrit() {
        let (_d, st) = stockage();
        let mut r = st.nouvelle_reunion(T0, None).unwrap();
        // `reunion.json` est un dossier, qui suit le déplacement : son
        // écriture atomique échoue. Le dossier de la réunion ne bouge pas
        // ici : sous Windows, la transcription ouverte l'en empêche (KOB-376).
        fs::remove_file(r.dossier().join(META)).ok();
        fs::create_dir_all(r.dossier().join(META)).unwrap();
        assert!(r.nommer("Rétro du sprint", Origine::Manuel).is_err());
        assert_eq!(r.nom(), "2026-10-18-retro-du-sprint", "le dossier a bougé");
        assert_eq!(
            nom_actuel(&st.dossier_reunions(), "2026-10-18-reunion").as_deref(),
            Some("2026-10-18-retro-du-sprint")
        );
    }

    /// KOB-266 : la table des renommages illisible en écriture n'empêche
    /// pas le nouveau titre d'être écrit dans `reunion.json`.
    #[test]
    fn titre_ecrit_meme_si_la_table_des_renommages_echoue() {
        let (_d, st) = stockage();
        let mut r = st.nouvelle_reunion(T0, None).unwrap();
        // Un dossier à la place du fichier : son écriture échoue.
        fs::create_dir_all(st.dossier_reunions().join(RENOMMAGES)).unwrap();
        let ancien = r.nommer("Rétro du sprint", Origine::Auto).unwrap();
        assert_eq!(ancien.as_deref(), Some("2026-10-18-reunion"));
        let lue = st.lire_reunion(r.nom()).unwrap();
        assert_eq!(lue.resume.titre.as_deref(), Some("Rétro du sprint"));
        assert_eq!(r.meta().changements_auto, 1);
    }

    /// KOB-223 : sans titre (pas assez de parole), « Réunion du 18/10 à
    /// 11 h 30 » ; une réunion terminée se renomme à la main ; une
    /// conversation aussi.
    #[test]
    fn titre_par_defaut_et_renommage_apres_coup() {
        let (_d, st) = stockage();
        let mut r = st.nouvelle_reunion(T0, None).unwrap();
        r.terminer(T0 + 30_000).unwrap();
        let titre = r.meta().titre.clone().unwrap();
        assert!(titre.starts_with("Réunion du 18/10 à "), "{titre}");
        assert_eq!(r.nom(), "2026-10-18-reunion");
        // Comme dans l'app, le stockage ne renomme qu'une réunion fermée (la
        // réunion en cours passe par `nommer`) : sous Windows, la
        // transcription ouverte bloquerait le dossier (KOB-376).
        drop(r);
        let nouveau = st
            .renommer_reunion("2026-10-18-reunion", "Rétro du sprint")
            .unwrap();
        assert_eq!(nouveau, "2026-10-18-retro-du-sprint");
        let lue = st.lire_reunion(&nouveau).unwrap();
        assert_eq!(lue.resume.titre.as_deref(), Some("Rétro du sprint"));

        let mut c = st.nouvelle_conversation(T0);
        c.ajouter(Qui::Moi, "Où est le refresh token ?", true)
            .unwrap();
        let id = st.conversations().unwrap()[0].id.clone();
        st.renommer_conversation(&id, "Refresh token").unwrap();
        assert_eq!(
            st.conversations().unwrap()[0].titre.as_deref(),
            Some("Refresh token")
        );
        assert_eq!(
            st.conversations().unwrap().len(),
            1,
            "le titre n'est pas une conversation"
        );
        st.renommer_conversation(&id, " ").unwrap();
        assert_eq!(st.conversations().unwrap()[0].titre, None);
    }

    const CR: &str = "## Résumé\nOn a validé le budget du séminaire.\n\n## Décisions\n- Budget validé\n\n## Actions\n- [ ] Sophie : devis\n\n## Questions ouvertes\n- Date ?\n\n## Interpellations\n- Kobi, c'était quoi le chiffre ?\n";

    #[test]
    fn reunions_et_conversations_rangees_a_part() {
        let (_d, st) = stockage();
        let mut r = st.nouvelle_reunion(T0, Some("Point budget")).unwrap();
        r.ecrire_segment(&segment(0.0, Source::Moi, "Bonjour à tous"))
            .unwrap();
        r.ecrire_segment(&segment(3.0, Source::Eux, "On parle du séminaire à Annecy"))
            .unwrap();
        r.ecrire_notes("- budget en cours\n").unwrap();
        r.ecrire_compte_rendu(CR).unwrap();
        r.terminer(T0 + 60_000).unwrap();

        let mut c = st.nouvelle_conversation(T0 + 3_600_000);
        assert!(
            !c.chemin().exists(),
            "rien n'est écrit avant le premier tour"
        );
        c.ajouter(Qui::Moi, "Où est géré le refresh token ?", true)
            .unwrap();
        c.ajouter(Qui::Kobi, "Dans auth/session.rs, ligne 42.", false)
            .unwrap();

        assert!(r.dossier().starts_with(st.racine().join(REUNIONS)));
        assert!(c.chemin().starts_with(st.racine().join(CONVERSATIONS)));
        let jour = heure_locale(T0 + 3_600_000).0.to_string();
        assert!(c.id().starts_with(&format!("{jour}/")), "{}", c.id());

        let reunions = st.reunions().unwrap();
        assert_eq!(reunions.len(), 1);
        let rr = &reunions[0];
        assert_eq!(rr.titre.as_deref(), Some("Point budget"));
        assert!(!rr.en_cours && rr.a_compte_rendu);
        assert_eq!(rr.duree_s, 5.0);
        assert_eq!(rr.apercu, "On a validé le budget du séminaire.");

        let lue = st.lire_reunion(r.nom()).unwrap();
        assert_eq!(lue.segments.len(), 2);
        assert_eq!(lue.notes, "- budget en cours\n");
        let cr = lue.compte_rendu.unwrap();
        assert_eq!(cr.decisions, "- Budget validé");
        assert_eq!(cr.actions, "- [ ] Sophie : devis");
        assert_eq!(cr.questions, "- Date ?");
        assert_eq!(cr.autres.len(), 1);
        assert_eq!(cr.autres[0].0, "Interpellations");

        let convs = st.conversations().unwrap();
        assert_eq!(convs.len(), 1);
        assert_eq!(convs[0].tours, 2);
        assert_eq!(convs[0].apercu, "Où est géré le refresh token ?");
        let lue = st.lire_conversation(c.id()).unwrap();
        assert_eq!(lue.tours[1].qui, Qui::Kobi);
        assert!(lue.tours[0].voix && !lue.tours[1].voix);
    }

    #[test]
    fn changer_de_racine_sans_redemarrer() {
        let (d, st) = stockage();
        let mut en_cours = st.nouvelle_reunion(T0, Some("avant")).unwrap();

        // L'utilisateur change de dossier : la réunion en cours finit là où
        // elle a commencé, la suivante va dans le nouveau.
        let autre = Stockage::new(d.path().join("Ailleurs"));
        autre.preparer().unwrap();
        en_cours
            .ecrire_segment(&segment(0.0, Source::Moi, "fin"))
            .unwrap();
        en_cours.terminer(T0 + 1000).unwrap();
        let apres = autre.nouvelle_reunion(T0, Some("après")).unwrap();
        let mut c = autre.nouvelle_conversation(T0);
        c.ajouter(Qui::Moi, "salut", false).unwrap();

        assert_eq!(st.reunions().unwrap().len(), 1);
        assert_eq!(st.conversations().unwrap().len(), 0);
        assert_eq!(autre.reunions().unwrap()[0].nom, apres.nom());
        assert_eq!(autre.conversations().unwrap().len(), 1);
    }

    #[test]
    fn ancien_dossier_meetings_renomme_une_fois() {
        let dir = tempfile::tempdir().unwrap();
        let racine = dir.path().join("Kobi");
        let ancienne = racine.join(ANCIEN_REUNIONS).join("2026-10-17-reunion");
        fs::create_dir_all(&ancienne).unwrap();
        fs::write(ancienne.join(TRANSCRIPT), "").unwrap();
        fs::write(ancienne.join(COMPTE_RENDU), CR).unwrap();

        let st = Stockage::new(&racine);
        assert!(st.preparer().unwrap());
        assert!(!st.preparer().unwrap());
        let liste = st.reunions().unwrap();
        assert_eq!(liste.len(), 1);
        assert_eq!(liste[0].date.as_deref(), Some("2026-10-17"));
        assert_eq!(liste[0].titre, None);
        assert!(!liste[0].en_cours, "compte rendu présent : terminée");
        // Reprise d'une réunion sans méta.
        let mut r = st.rouvrir_reunion("2026-10-17-reunion").unwrap();
        r.ecrire_segment(&segment(1.0, Source::Eux, "reprise"))
            .unwrap();
        assert_eq!(
            st.lire_reunion("2026-10-17-reunion")
                .unwrap()
                .segments
                .len(),
            1
        );
    }

    #[test]
    fn recherche_sans_casse_ni_accents() {
        let (_d, st) = stockage();
        let mut r = st.nouvelle_reunion(T0, Some("Séminaire")).unwrap();
        r.ecrire_segment(&segment(
            65.0,
            Source::Eux,
            "Le séminaire sera à ANNECY en mars",
        ))
        .unwrap();
        r.ecrire_compte_rendu(CR).unwrap();
        let mut c = st.nouvelle_conversation(T0);
        c.ajouter(Qui::Moi, "Rappelle-moi le séminaire", true)
            .unwrap();

        let t = st.chercher("annecy", 50).unwrap();
        assert_eq!(t.len(), 1);
        match &t[0] {
            Trouve::Reunion {
                partie,
                start,
                extrait,
                ..
            } => {
                assert_eq!(*partie, "transcription");
                assert_eq!(*start, Some(65.0));
                assert!(extrait.contains("ANNECY"), "{extrait}");
            }
            autre => panic!("{autre:?}"),
        }
        let t = st.chercher("SEMINAIRE", 50).unwrap();
        assert!(t.iter().any(|x| matches!(x, Trouve::Conversation { .. })));
        assert!(t.iter().any(|x| matches!(
            x,
            Trouve::Reunion {
                partie: "compte-rendu",
                ..
            }
        )));
        assert_eq!(st.chercher("seminaire", 2).unwrap().len(), 2);
        assert!(st.chercher("  ", 10).unwrap().is_empty());
        assert!(st.chercher("introuvable", 10).unwrap().is_empty());
    }

    #[test]
    fn rappels() {
        let (_d, st) = stockage();
        assert!(st.rappels().unwrap().is_empty());
        let a = st.ajouter_rappel("appeler Sophie", Some(T0), None).unwrap();
        let b = st
            .ajouter_rappel("relire la PR", None, Some("2026-10-18/093000"))
            .unwrap();
        let c = st.ajouter_rappel("devis", Some(T0 + 10_000), None).unwrap();
        assert_ne!(a.id, b.id);
        let ids: Vec<_> = st.rappels().unwrap().into_iter().map(|r| r.id).collect();
        assert_eq!(ids, [a.id.clone(), c.id.clone(), b.id.clone()]);
        assert_eq!(st.rappels_dus(T0 + 1).unwrap(), vec![a.clone()]);

        assert!(st.marquer_fait(&a.id).unwrap());
        assert!(!st.marquer_fait("inconnu").unwrap());
        assert!(st.rappels_dus(T0 + 1).unwrap().is_empty());
        assert_eq!(
            st.rappels().unwrap().last().unwrap().id,
            a.id,
            "faits en dernier"
        );
        assert!(st.supprimer_rappel(&b.id).unwrap());
        assert_eq!(st.rappels().unwrap().len(), 2);
    }

    #[test]
    fn purge_des_conversations_a_part() {
        let (_d, st) = stockage();
        let aujourdhui = Date {
            annee: 2026,
            mois: 10,
            jour: 18,
        };
        let vieux = st.dossier_conversations().join("2026-06-01");
        fs::create_dir_all(&vieux).unwrap();
        fs::write(vieux.join("100000.jsonl"), "").unwrap();
        let recent = st.dossier_conversations().join("2026-10-10");
        fs::create_dir_all(&recent).unwrap();
        let r = st.nouvelle_reunion(T0 - 200 * 86_400_000, None).unwrap();
        drop(r);

        let partis = st
            .purger(Retention::Unlimited, Retention::Days(90), aujourdhui, None)
            .unwrap();
        assert_eq!(partis, vec![vieux.clone()]);
        assert!(recent.exists());
        assert_eq!(st.reunions().unwrap().len(), 1, "réunions illimitées");
    }

    #[test]
    fn par_defaut_rien_n_est_purge() {
        let (_d, st) = stockage();
        let aujourdhui = Date {
            annee: 2026,
            mois: 10,
            jour: 18,
        };
        let vieux = st.dossier_conversations().join("2020-01-01");
        fs::create_dir_all(&vieux).unwrap();
        fs::write(vieux.join("100000.jsonl"), "").unwrap();
        let config = Config::default();
        let partis = st
            .purger(
                config.meetings.retention_days,
                config.conversations.retention_days,
                aujourdhui,
                None,
            )
            .unwrap();
        assert!(partis.is_empty(), "conservation illimitée par défaut");
        assert!(vieux.exists());
    }

    #[test]
    fn suppressions_et_noms_refuses() {
        let (_d, st) = stockage();
        let mut c = st.nouvelle_conversation(T0);
        c.ajouter(Qui::Moi, "x", false).unwrap();
        let id = c.id().to_owned();
        drop(c);
        for mauvais in [
            "../x",
            "2026-10-18",
            "2026-10-18/../../a",
            "a/b",
            "2026-10-18/a:b",
        ] {
            assert!(
                matches!(st.lire_conversation(mauvais), Err(ErreurDonnees::Nom(_))),
                "{mauvais}"
            );
        }
        st.supprimer_conversation(&id).unwrap();
        assert!(st.conversations().unwrap().is_empty());
        assert!(matches!(
            st.lire_conversation(&id),
            Err(ErreurDonnees::Introuvable(_))
        ));

        let r = st.nouvelle_reunion(T0, None).unwrap();
        let nom = r.nom().to_owned();
        drop(r);
        st.supprimer_reunion(&nom, None).unwrap();
        assert!(st.reunions().unwrap().is_empty());
        assert!(matches!(st.lire_reunion(".."), Err(ErreurDonnees::Nom(_))));
    }

    #[test]
    fn documents_prets_a_publier() {
        let (_d, st) = stockage();
        let mut r = st.nouvelle_reunion(T0, Some("Point budget")).unwrap();
        r.ecrire_segment(&segment(3725.0, Source::Eux, "Budget validé"))
            .unwrap();
        r.ecrire_compte_rendu(CR).unwrap();
        let doc = st.document_reunion(r.nom()).unwrap();
        assert!(
            doc.titre.starts_with("Point budget — 2026-10-"),
            "{}",
            doc.titre
        );
        assert!(doc.markdown.contains("## Décisions\n- Budget validé"));
        assert!(doc.markdown.contains("- `1:02:05` **Eux** : Budget validé"));
        // À publier (KOB-75) : le compte rendu, sans la transcription.
        let publie = st.compte_rendu_a_publier(r.nom()).unwrap();
        assert_eq!(publie.titre, doc.titre);
        assert!(publie.markdown.contains("## Décisions\n- Budget validé"));
        assert!(!publie.markdown.contains("1:02:05"), "{}", publie.markdown);

        let mut c = st.nouvelle_conversation(T0);
        c.ajouter(Qui::Moi, "bonjour", true).unwrap();
        let doc = st.document_conversation(c.id()).unwrap();
        assert!(doc.markdown.contains("**Moi** ("), "{}", doc.markdown);

        let mut p = Publication::default();
        assert_eq!(etat_publication(&p), EtatPublication::Aucune);
        p.mcp = "linear".into();
        p.destination = "Kobi".into();
        assert_eq!(
            etat_publication(&p),
            EtatPublication::Reglee {
                mcp: "linear".into(),
                destination: "Kobi".into()
            }
        );
    }

    #[test]
    fn deux_conversations_meme_seconde() {
        let (_d, st) = stockage();
        let mut a = st.nouvelle_conversation(T0);
        a.ajouter(Qui::Moi, "a", false).unwrap();
        let mut b = st.nouvelle_conversation(T0);
        b.ajouter(Qui::Moi, "b", false).unwrap();
        assert_ne!(a.id(), b.id());
        assert_eq!(st.conversations().unwrap().len(), 2);
    }

    #[test]
    fn horodatages() {
        assert_eq!(horodatage(0.0), "00:00");
        assert_eq!(horodatage(65.4), "01:05");
        assert_eq!(horodatage(3725.0), "1:02:05");
    }
}
