//! Réglages de Kobi, lus et écrits dans `~/Kobi/config.toml`.
//!
//! Le fichier est créé avec les valeurs par défaut au premier lancement. Une
//! config partielle est complétée par les défauts ; les clés inconnues
//! (réglages d'une version plus récente, notes de l'utilisateur) sont
//! conservées à l'écriture. Un fichier illisible ne bloque jamais Kobi : il
//! tourne avec les valeurs par défaut et un message clair, et le fichier n'est
//! pas écrasé sans être d'abord mis de côté.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Nom du fichier de réglages dans le dossier de Kobi.
pub const FILE_NAME: &str = "config.toml";

/// Copie d'un fichier illisible, faite avant de l'écraser.
pub const BACKUP_SUFFIX: &str = "illisible";

/// Dossier de Kobi : `~/Kobi`.
pub fn kobi_dir() -> Option<PathBuf> {
    std::env::home_dir().map(|home| home.join("Kobi"))
}

/// Chemin par défaut du fichier de réglages : `~/Kobi/config.toml`.
pub fn default_path() -> Option<PathBuf> {
    kobi_dir().map(|dir| dir.join(FILE_NAME))
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error(
        "{path} : erreur de syntaxe ou valeur invalide, valeurs par défaut utilisées. {source}"
    )]
    Parse {
        path: PathBuf,
        #[source]
        source: Box<toml::de::Error>,
    },
    #[error(
        "{path} : {field} = {value} n'est pas accepté ({expected}), valeurs par défaut utilisées"
    )]
    Invalid {
        path: PathBuf,
        field: &'static str,
        value: String,
        expected: &'static str,
    },
    #[error("{path} : lecture ou écriture impossible : {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("réglages impossibles à écrire en TOML : {0}")]
    Serialize(#[from] toml::ser::Error),
}

/// Cerveau de Kobi.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Brain {
    /// Claude Code s'il est détecté, sinon la clé API.
    #[default]
    Auto,
    ClaudeCode,
    Api,
}

/// Sources audio écoutées en réunion.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AudioSources {
    Mic,
    System,
    #[default]
    Both,
}

/// Mode de prise de notes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NotesMode {
    #[default]
    Blocks,
    End,
    Continuous,
}

/// Durée de conservation des réunions ou des conversations : 30, 90 ou 365
/// jours, ou illimitée (le défaut).
///
/// En TOML : un entier (`retention_days = 90`) ou `"unlimited"`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Retention {
    Days(u16),
    #[default]
    Unlimited,
}

impl Retention {
    pub const ALLOWED_DAYS: [u16; 3] = [30, 90, 365];
    const UNLIMITED: &'static str = "unlimited";
}

impl Serialize for Retention {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Retention::Days(days) => serializer.serialize_u16(*days),
            Retention::Unlimited => serializer.serialize_str(Self::UNLIMITED),
        }
    }
}

impl<'de> Deserialize<'de> for Retention {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Days(i64),
            Text(String),
        }
        match Raw::deserialize(deserializer)? {
            Raw::Days(days) => u16::try_from(days)
                .ok()
                .filter(|d| Self::ALLOWED_DAYS.contains(d))
                .map(Retention::Days)
                .ok_or_else(|| {
                    serde::de::Error::custom(format!(
                        "retention_days = {days} : attendu 30, 90, 365 ou \"unlimited\""
                    ))
                }),
            Raw::Text(text) if text == Self::UNLIMITED => Ok(Retention::Unlimited),
            Raw::Text(text) => Err(serde::de::Error::custom(format!(
                "retention_days = \"{text}\" : attendu 30, 90, 365 ou \"unlimited\""
            ))),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    pub sources: AudioSources,
    #[serde(flatten)]
    pub extra: toml::Table,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SttConfig {
    /// Moteur de transcription ; `"auto"` = le gagnant du benchmark.
    pub engine: String,
    #[serde(flatten)]
    pub extra: toml::Table,
}

impl Default for SttConfig {
    fn default() -> Self {
        Self {
            engine: "auto".into(),
            extra: toml::Table::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotesConfig {
    pub mode: NotesMode,
    /// Intervalle du mode blocs, en minutes (1 à 10).
    pub interval_min: u8,
    #[serde(flatten)]
    pub extra: toml::Table,
}

impl NotesConfig {
    pub const INTERVAL_RANGE: std::ops::RangeInclusive<u8> = 1..=10;
}

impl Default for NotesConfig {
    fn default() -> Self {
        Self {
            mode: NotesMode::default(),
            interval_min: 3,
            extra: toml::Table::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceConfig {
    /// Réponses lues à voix haute hors réunion.
    pub assistant: bool,
    /// Réponses lues à voix haute en réunion.
    pub meeting: bool,
    #[serde(flatten)]
    pub extra: toml::Table,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            assistant: true,
            meeting: false,
            extra: toml::Table::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MeetingsConfig {
    pub retention_days: Retention,
    #[serde(flatten)]
    pub extra: toml::Table,
}

/// Conversations du quotidien, rangées à part des réunions (KOB-141).
/// Conservées sans limite par défaut (décision du propriétaire : « on
/// conserve »), la durée restant réglable.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConversationsConfig {
    pub retention_days: Retention,
    #[serde(flatten)]
    pub extra: toml::Table,
}

/// Destination de publication d'un type de document, par un serveur MCP.
///
/// Prête mais inactive tant que Kobi est en lecture seule (KOB-138) : le
/// réglage est lu et gardé, rien n'est envoyé.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Publication {
    /// Nom du serveur MCP (`linear`, `colanode`, `notion`…) ; vide = aucune.
    pub mcp: String,
    /// Où écrire chez lui : projet, dossier, base… (texte libre pour Claude).
    pub destination: String,
    #[serde(flatten)]
    pub extra: toml::Table,
}

impl Publication {
    pub fn est_reglee(&self) -> bool {
        !self.mcp.trim().is_empty()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PublicationConfig {
    pub reunions: Publication,
    pub conversations: Publication,
    #[serde(flatten)]
    pub extra: toml::Table,
}

/// Où Kobi range réunions, conversations et rappels.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StockageConfig {
    /// Dossier racine ; absent = `~/Kobi`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dossier: Option<PathBuf>,
    pub publication: PublicationConfig,
    #[serde(flatten)]
    pub extra: toml::Table,
}

impl StockageConfig {
    /// Dossier racine effectif : le réglage, sinon `~/Kobi`.
    pub fn racine(&self) -> Option<PathBuf> {
        match &self.dossier {
            Some(d) if !d.as_os_str().is_empty() => Some(d.clone()),
            _ => kobi_dir(),
        }
    }
}

/// Que faire d'une action qui écrit (KOB-156, contrat commun des
/// permissions, section 3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Politique {
    /// Carte de validation, à la voix ou d'un clic (défaut).
    #[default]
    Demander,
    /// Fait sans demander (jamais pour une suppression).
    SansDemander,
}

/// `[permissions]` : politique par genre d'action qui écrit. La suppression
/// de fichiers est toujours demandée, sans réglage.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PermissionsConfig {
    pub fichier: Politique,
    pub commande: Politique,
    pub reglage: Politique,
    #[serde(flatten)]
    pub extra: toml::Table,
}

/// Réglages de Kobi (dossier de conception, section « Configuration et
/// stockage »).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub brain: Brain,
    pub wake_word: String,
    /// Raccourci global, au format des accélérateurs Tauri.
    pub hotkey: String,
    /// Raccourci qui démarre ou arrête l'enregistrement de la réunion.
    pub hotkey_record: String,
    /// Raccourci qui ouvre la bulle pour écrire à Kobi (KOB-147).
    pub hotkey_write: String,
    /// Raccourci qui ouvre ou range la vue agrandie de la conversation (KOB-169).
    pub hotkey_view: String,
    /// Dossiers de code que Kobi peut lire (KOB-138), passés à Claude Code en
    /// `--add-dir`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub codebases: Vec<PathBuf>,
    pub audio: AudioConfig,
    pub stt: SttConfig,
    pub notes: NotesConfig,
    pub voice: VoiceConfig,
    pub meetings: MeetingsConfig,
    pub conversations: ConversationsConfig,
    pub stockage: StockageConfig,
    /// Serveurs MCP connectés (KOB-155).
    pub mcp: crate::mcp::McpConfig,
    /// Actions qui écrivent : demander ou non (KOB-156).
    pub permissions: PermissionsConfig,
    /// Clés inconnues, conservées telles quelles à l'écriture.
    #[serde(flatten)]
    pub extra: toml::Table,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            brain: Brain::default(),
            wake_word: "Kobi".into(),
            hotkey: "Ctrl+Shift+Space".into(),
            hotkey_record: "Ctrl+Shift+R".into(),
            hotkey_write: "Ctrl+Alt+K".into(),
            hotkey_view: "Ctrl+Alt+L".into(),
            codebases: Vec::new(),
            audio: AudioConfig::default(),
            stt: SttConfig::default(),
            notes: NotesConfig::default(),
            voice: VoiceConfig::default(),
            meetings: MeetingsConfig::default(),
            conversations: ConversationsConfig::default(),
            stockage: StockageConfig::default(),
            mcp: crate::mcp::McpConfig::default(),
            permissions: PermissionsConfig::default(),
            extra: toml::Table::new(),
        }
    }
}

impl Config {
    /// Lit une config TOML ; `path` ne sert qu'aux messages d'erreur.
    pub fn parse(text: &str, path: &Path) -> Result<Self, ConfigError> {
        let config: Config = toml::from_str(text).map_err(|e| ConfigError::Parse {
            path: path.to_owned(),
            source: Box::new(e),
        })?;
        config.validate(path)?;
        Ok(config)
    }

    fn validate(&self, path: &Path) -> Result<(), ConfigError> {
        let invalid = |field, value: &dyn fmt::Display, expected| ConfigError::Invalid {
            path: path.to_owned(),
            field,
            value: value.to_string(),
            expected,
        };
        if !NotesConfig::INTERVAL_RANGE.contains(&self.notes.interval_min) {
            return Err(invalid(
                "notes.interval_min",
                &self.notes.interval_min,
                "de 1 à 10",
            ));
        }
        if self.wake_word.trim().is_empty() {
            return Err(invalid("wake_word", &"\"\"", "un mot non vide"));
        }
        Ok(())
    }

    /// Config au format TOML, clés inconnues comprises.
    pub fn to_toml(&self) -> Result<String, ConfigError> {
        Ok(toml::to_string(self)?)
    }
}

/// Résultat de [`load_or_create`] : une config toujours utilisable.
#[derive(Debug)]
pub struct Loaded {
    pub config: Config,
    /// Le fichier n'existait pas et vient d'être créé.
    pub created: bool,
    /// Problème à montrer à l'utilisateur ; `config` vaut alors les défauts.
    pub problem: Option<ConfigError>,
}

/// Lit `path`, ou le crée avec les valeurs par défaut s'il n'existe pas.
///
/// N'échoue jamais : un fichier illisible donne les valeurs par défaut et un
/// `problem`, sans toucher au fichier.
pub fn load_or_create(path: &Path) -> Loaded {
    match fs::read_to_string(path) {
        Ok(text) => match Config::parse(&text, path) {
            Ok(config) => Loaded {
                config,
                created: false,
                problem: None,
            },
            Err(problem) => Loaded {
                config: Config::default(),
                created: false,
                problem: Some(problem),
            },
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let config = Config::default();
            let problem = save(&config, path).err();
            Loaded {
                created: problem.is_none(),
                config,
                problem,
            }
        }
        Err(source) => Loaded {
            config: Config::default(),
            created: false,
            problem: Some(ConfigError::Io {
                path: path.to_owned(),
                source,
            }),
        },
    }
}

/// Écrit la config dans `path` (dossier parent créé au besoin).
///
/// L'écriture passe par un fichier temporaire renommé, pour ne jamais laisser
/// un fichier à moitié écrit. Si le fichier en place est illisible, il est
/// d'abord copié en `config.toml.illisible` pour ne pas perdre les réglages
/// de l'utilisateur.
pub fn save(config: &Config, path: &Path) -> Result<(), ConfigError> {
    let io_err = |source| ConfigError::Io {
        path: path.to_owned(),
        source,
    };
    let text = config.to_toml()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(io_err)?;
    }
    if let Ok(current) = fs::read_to_string(path) {
        if Config::parse(&current, path).is_err() {
            fs::copy(path, with_suffix(path, BACKUP_SUFFIX)).map_err(io_err)?;
        }
    }
    let tmp = with_suffix(path, "tmp");
    fs::write(&tmp, text).map_err(io_err)?;
    fs::rename(&tmp, path).map_err(io_err)
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_owned();
    name.push(".");
    name.push(suffix);
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path_in(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("Kobi").join(FILE_NAME)
    }

    #[test]
    fn absente_creee_avec_les_defauts() {
        let dir = tempfile::tempdir().unwrap();
        let path = path_in(&dir);

        let loaded = load_or_create(&path);

        assert!(loaded.created);
        assert!(loaded.problem.is_none());
        assert_eq!(loaded.config, Config::default());
        let written = fs::read_to_string(&path).unwrap();
        assert_eq!(Config::parse(&written, &path).unwrap(), Config::default());

        // Second lancement : relu, pas recréé.
        let again = load_or_create(&path);
        assert!(!again.created);
        assert!(again.problem.is_none());
        assert_eq!(again.config, Config::default());
    }

    #[test]
    fn defauts_du_dossier_de_conception() {
        let c = Config::default();
        assert_eq!(c.brain, Brain::Auto);
        assert_eq!(c.wake_word, "Kobi");
        assert_eq!(c.hotkey, "Ctrl+Shift+Space");
        assert_eq!(c.hotkey_record, "Ctrl+Shift+R");
        assert_eq!(c.hotkey_write, "Ctrl+Alt+K");
        assert_eq!(c.hotkey_view, "Ctrl+Alt+L");
        assert_eq!(c.audio.sources, AudioSources::Both);
        assert_eq!(c.stt.engine, "auto");
        assert_eq!(c.notes.mode, NotesMode::Blocks);
        assert_eq!(c.notes.interval_min, 3);
        assert!(c.voice.assistant);
        assert!(!c.voice.meeting);
        assert_eq!(c.meetings.retention_days, Retention::Unlimited);
        assert_eq!(c.conversations.retention_days, Retention::Unlimited);
        assert_eq!(c.stockage.dossier, None);
        assert_eq!(c.stockage.racine(), kobi_dir());
        assert!(!c.stockage.publication.reunions.est_reglee());
    }

    #[test]
    fn stockage_et_publication_lus_et_reecrits() {
        let text = r#"
[conversations]
retention_days = 30

[stockage]
dossier = "D:/Notes/Kobi"

[stockage.publication.reunions]
mcp = "linear"
destination = "Projet Kobi"
"#;
        let c = Config::parse(text, Path::new("config.toml")).unwrap();
        assert_eq!(c.conversations.retention_days, Retention::Days(30));
        assert_eq!(c.stockage.racine(), Some(PathBuf::from("D:/Notes/Kobi")));
        assert!(c.stockage.publication.reunions.est_reglee());
        assert_eq!(c.stockage.publication.reunions.destination, "Projet Kobi");
        assert!(!c.stockage.publication.conversations.est_reglee());
        let back = Config::parse(&c.to_toml().unwrap(), Path::new("c")).unwrap();
        assert_eq!(back, c);
        // Dossier vide : retour au défaut.
        let vide = Config::parse("[stockage]\ndossier = \"\"", Path::new("c")).unwrap();
        assert_eq!(vide.stockage.racine(), kobi_dir());
    }

    #[test]
    fn partielle_completee_par_les_defauts() {
        let text = r#"
brain = "api"

[notes]
interval_min = 5

[meetings]
retention_days = 90
"#;
        let c = Config::parse(text, Path::new("config.toml")).unwrap();
        assert_eq!(c.brain, Brain::Api);
        assert_eq!(c.notes.interval_min, 5);
        assert_eq!(c.notes.mode, NotesMode::Blocks);
        assert_eq!(c.meetings.retention_days, Retention::Days(90));
        assert_eq!(c.wake_word, "Kobi");
        assert_eq!(c.audio, AudioConfig::default());
        assert_eq!(c.voice, VoiceConfig::default());
    }

    #[test]
    fn ancien_reglage_skin_toujours_lisible() {
        // Il n'y a plus qu'un skin (KOB-136) : la clé `skin` d'une ancienne
        // config est lue sans erreur et conservée à l'écriture.
        let text = "skin = \"kobi\"\nbrain = \"api\"\n";
        let c = Config::parse(text, Path::new("config.toml")).unwrap();
        assert_eq!(c.brain, Brain::Api);
        assert_eq!(c.extra["skin"].as_str(), Some("kobi"));
        let back: toml::Table = toml::from_str(&c.to_toml().unwrap()).unwrap();
        assert_eq!(back["skin"].as_str(), Some("kobi"));
    }

    #[test]
    fn vide_donne_les_defauts() {
        let c = Config::parse("", Path::new("config.toml")).unwrap();
        assert_eq!(c, Config::default());
    }

    #[test]
    fn champs_inconnus_conserves_a_l_ecriture() {
        let dir = tempfile::tempdir().unwrap();
        let path = path_in(&dir);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"
ancien_reglage = "x"
futur_reglage = 42

[notes]
mode = "end"
couleur = "vert"

[audio]
sources = "mic"
gain = { mic = 1.5 }

[plugin]
nom = "essai"
liste = [1, 2, 3]
"#,
        )
        .unwrap();

        let mut loaded = load_or_create(&path);
        assert!(loaded.problem.is_none(), "{:?}", loaded.problem);
        loaded.config.hotkey = "Ctrl+Alt+K".into();
        save(&loaded.config, &path).unwrap();

        let back: toml::Table = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(back["hotkey"].as_str(), Some("Ctrl+Alt+K"));
        assert_eq!(back["ancien_reglage"].as_str(), Some("x"));
        assert_eq!(back["futur_reglage"].as_integer(), Some(42));
        assert_eq!(back["notes"]["mode"].as_str(), Some("end"));
        assert_eq!(back["notes"]["couleur"].as_str(), Some("vert"));
        assert_eq!(back["audio"]["sources"].as_str(), Some("mic"));
        assert_eq!(back["audio"]["gain"]["mic"].as_float(), Some(1.5));
        assert_eq!(back["plugin"]["nom"].as_str(), Some("essai"));
        assert_eq!(back["plugin"]["liste"].as_array().map(Vec::len), Some(3));
    }

    #[test]
    fn syntaxe_invalide_message_clair_et_defauts() {
        let dir = tempfile::tempdir().unwrap();
        let path = path_in(&dir);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let broken = "brain = \"api\"\n[notes\ninterval_min = 4\n";
        fs::write(&path, broken).unwrap();

        let loaded = load_or_create(&path);

        assert_eq!(loaded.config, Config::default());
        assert!(!loaded.created);
        let message = loaded.problem.expect("problème signalé").to_string();
        assert!(message.contains("config.toml"), "{message}");
        assert!(message.contains("valeurs par défaut"), "{message}");
        assert!(message.contains("line 2"), "{message}");
        // Le fichier de l'utilisateur n'est pas touché à la lecture.
        assert_eq!(fs::read_to_string(&path).unwrap(), broken);
    }

    #[test]
    fn valeurs_invalides_refusees() {
        let p = Path::new("config.toml");
        for (text, needle) in [
            ("brain = \"gpt\"", "brain"),
            ("[audio]\nsources = \"casque\"", "casque"),
            ("[notes]\ninterval_min = 0", "notes.interval_min"),
            ("[notes]\ninterval_min = 42", "notes.interval_min"),
            ("[notes]\ninterval_min = \"trois\"", "trois"),
            ("[meetings]\nretention_days = 7", "retention_days"),
            ("[meetings]\nretention_days = \"jamais\"", "jamais"),
            ("[conversations]\nretention_days = 12", "retention_days"),
            ("wake_word = \"  \"", "wake_word"),
            ("[voice]\nmeeting = \"oui\"", "oui"),
        ] {
            let message = Config::parse(text, p).expect_err(text).to_string();
            assert!(message.contains(needle), "{text} → {message}");
            assert!(message.contains("valeurs par défaut"), "{text} → {message}");
        }
    }

    #[test]
    fn retention_aller_retour() {
        let mut c = Config::default();
        for retention in [
            Retention::Days(30),
            Retention::Days(365),
            Retention::Unlimited,
        ] {
            c.meetings.retention_days = retention;
            let text = c.to_toml().unwrap();
            assert_eq!(Config::parse(&text, Path::new("c")).unwrap(), c, "{text}");
        }
    }

    #[test]
    fn ecriture_sur_fichier_illisible_le_met_de_cote() {
        let dir = tempfile::tempdir().unwrap();
        let path = path_in(&dir);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "brain = = \"api\"").unwrap();

        save(&Config::default(), &path).unwrap();

        let backup = with_suffix(&path, BACKUP_SUFFIX);
        assert_eq!(fs::read_to_string(backup).unwrap(), "brain = = \"api\"");
        let written = fs::read_to_string(&path).unwrap();
        assert_eq!(Config::parse(&written, &path).unwrap(), Config::default());
        assert!(!with_suffix(&path, "tmp").exists());
    }

    #[test]
    fn dossier_kobi_sous_le_home() {
        if let Some(path) = default_path() {
            assert!(path.ends_with(Path::new("Kobi").join("config.toml")));
        }
    }
}
