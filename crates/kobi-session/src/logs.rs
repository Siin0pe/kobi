//! Journal local de Kobi : `~/Kobi/logs/kobi.log`, rotation à 10 Mo, 3 fichiers.
//!
//! Règle : jamais le contenu des conversations dans les logs. Les champs
//! `tracing` dont le nom figure dans [`SENSITIVE_FIELDS`] (texte transcrit,
//! question, réponse, notes…) sont remplacés par leur seule longueur. Le
//! message d'un événement ne doit contenir que du texte fixe : le contenu passe
//! toujours par un champ nommé.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tracing::field::{Field, Visit};
use tracing_subscriber::field::{MakeVisitor, VisitFmt, VisitOutput};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::MakeWriter;

/// Nom du fichier de log courant.
pub const FILE_NAME: &str = "kobi.log";
/// Taille à partir de laquelle le fichier courant est tourné.
pub const MAX_BYTES: u64 = 10 * 1024 * 1024;
/// Nombre de fichiers gardés : `kobi.log`, `kobi.log.1`, `kobi.log.2`.
pub const MAX_FILES: usize = 3;

/// Noms de champs dont la valeur n'est jamais écrite.
pub const SENSITIVE_FIELDS: &[&str] = &[
    "text",
    "transcript",
    "segment",
    "question",
    "answer",
    "response",
    "reply",
    "prompt",
    "notes",
    "content",
    "message_text",
    "title",
];

/// Dossier des logs : `~/Kobi/logs`.
pub fn default_dir() -> Option<PathBuf> {
    crate::config::kobi_dir().map(|dir| dir.join("logs"))
}

/// Fichier tournant par taille, partagé entre threads.
#[derive(Clone)]
pub struct RotatingFile {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    dir: PathBuf,
    max_bytes: u64,
    max_files: usize,
    file: File,
    len: u64,
}

impl RotatingFile {
    /// Ouvre (ou crée) `dir/kobi.log` en ajout.
    pub fn open(dir: &Path) -> io::Result<Self> {
        Self::with_limits(dir, MAX_BYTES, MAX_FILES)
    }

    pub fn with_limits(dir: &Path, max_bytes: u64, max_files: usize) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let file = open_append(&dir.join(FILE_NAME))?;
        let len = file.metadata()?.len();
        Ok(Self {
            inner: Arc::new(Mutex::new(Inner {
                dir: dir.to_owned(),
                max_bytes,
                max_files: max_files.max(1),
                file,
                len,
            })),
        })
    }
}

fn open_append(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

fn numbered(dir: &Path, n: usize) -> PathBuf {
    if n == 0 {
        dir.join(FILE_NAME)
    } else {
        dir.join(format!("{FILE_NAME}.{n}"))
    }
}

impl Inner {
    fn rotate(&mut self) -> io::Result<()> {
        self.file.flush()?;
        // Le plus ancien disparaît, les autres reculent d'un rang.
        let last = numbered(&self.dir, self.max_files - 1);
        if last.exists() {
            fs::remove_file(&last)?;
        }
        for n in (0..self.max_files - 1).rev() {
            let from = numbered(&self.dir, n);
            if from.exists() {
                fs::rename(&from, numbered(&self.dir, n + 1))?;
            }
        }
        if self.max_files == 1 {
            // Un seul fichier : on repart de zéro.
            let _ = fs::remove_file(numbered(&self.dir, 0));
        }
        self.file = open_append(&numbered(&self.dir, 0))?;
        self.len = 0;
        Ok(())
    }

    fn write_line(&mut self, buf: &[u8]) -> io::Result<()> {
        if self.len > 0 && self.len + buf.len() as u64 > self.max_bytes {
            self.rotate()?;
        }
        self.file.write_all(buf)?;
        self.len += buf.len() as u64;
        Ok(())
    }
}

/// Un événement formaté, écrit d'un bloc à la fin pour ne jamais couper une
/// ligne entre deux fichiers.
pub struct LineWriter {
    target: Arc<Mutex<Inner>>,
    buf: Vec<u8>,
}

impl Write for LineWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let mut inner = self.target.lock().unwrap_or_else(|e| e.into_inner());
        let result = inner.write_line(&self.buf);
        self.buf.clear();
        result
    }
}

impl Drop for LineWriter {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

impl<'a> MakeWriter<'a> for RotatingFile {
    type Writer = LineWriter;

    fn make_writer(&'a self) -> Self::Writer {
        LineWriter {
            target: Arc::clone(&self.inner),
            buf: Vec::new(),
        }
    }
}

/// Formateur de champs qui masque les champs sensibles.
#[derive(Debug, Clone, Copy, Default)]
pub struct RedactFields;

impl<'a> MakeVisitor<Writer<'a>> for RedactFields {
    type Visitor = RedactVisitor<'a>;

    fn make_visitor(&self, writer: Writer<'a>) -> Self::Visitor {
        RedactVisitor {
            writer,
            first: true,
            result: Ok(()),
        }
    }
}

pub struct RedactVisitor<'a> {
    writer: Writer<'a>,
    first: bool,
    result: fmt::Result,
}

/// Le champ `name` est-il du contenu de conversation ?
pub fn is_sensitive(name: &str) -> bool {
    SENSITIVE_FIELDS.contains(&name)
}

impl RedactVisitor<'_> {
    fn record(&mut self, field: &Field, value: fmt::Arguments<'_>, len: Option<usize>) {
        if self.result.is_err() {
            return;
        }
        let sep = if self.first { "" } else { " " };
        self.first = false;
        let name = field.name();
        self.result = if is_sensitive(name) {
            match len {
                Some(len) => write!(self.writer, "{sep}{name}=<masqué, {len} car.>"),
                None => write!(self.writer, "{sep}{name}=<masqué>"),
            }
        } else if name == "message" {
            write!(self.writer, "{sep}{value}")
        } else {
            write!(self.writer, "{sep}{name}={value}")
        };
    }
}

impl Visit for RedactVisitor<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.record(
            field,
            format_args!("{value:?}"),
            Some(value.chars().count()),
        );
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.record(field, format_args!("{value:?}"), None);
    }
}

impl VisitOutput<fmt::Result> for RedactVisitor<'_> {
    fn finish(self) -> fmt::Result {
        self.result
    }
}

impl VisitFmt for RedactVisitor<'_> {
    fn writer(&mut self) -> &mut dyn fmt::Write {
        &mut self.writer
    }
}

/// Abonné `tracing` qui écrit dans `dir/kobi.log` en masquant les champs
/// sensibles. Sans couleurs ; niveau par défaut `info`.
pub fn subscriber(dir: &Path) -> io::Result<impl tracing::Subscriber + Send + Sync> {
    let file = RotatingFile::open(dir)?;
    Ok(tracing_subscriber::fmt()
        .with_writer(file)
        .with_ansi(false)
        .fmt_fields(RedactFields)
        .with_max_level(tracing::Level::INFO)
        .finish())
}

/// Installe le journal comme abonné global. À appeler une fois au démarrage.
pub fn init(dir: &Path) -> io::Result<()> {
    tracing::subscriber::set_global_default(subscriber(dir)?)
        .map_err(|e| io::Error::other(e.to_string()))
}

/// Crochet de panique (KOB-167) : sans lui, une panique dans l'exécutable
/// graphique de Windows ne laisse aucune trace. La panique va au journal, et
/// avec son fil et sa trace dans `dir/crash-<horodatage>.txt` ; le crochet
/// d'avant suit (message sur la sortie d'erreur).
pub fn installer_crochet_panique(dir: &Path) {
    let dir = dir.to_owned();
    let precedent = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let fil = std::thread::current()
            .name()
            .unwrap_or("sans nom")
            .to_owned();
        let trace = std::backtrace::Backtrace::force_capture();
        let horodatage = jiff::Zoned::now().strftime("%Y%m%d-%H%M%S").to_string();
        let rapport = rapport_de_panique(&fil, &info.to_string(), &trace);
        // Deux paniques dans la même seconde (deux fils) : la seconde prend
        // `-2`, `-3`… au lieu d'écraser le premier rapport (KOB-376).
        let mut fichier = dir.join(format!("crash-{horodatage}.txt"));
        for n in 2..100 {
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&fichier)
            {
                Ok(mut f) => {
                    let _ = f.write_all(rapport.as_bytes());
                    break;
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    fichier = dir.join(format!("crash-{horodatage}-{n}.txt"));
                }
                Err(_) => break,
            }
        }
        tracing::error!(fil = %fil, panique = %info, fichier = %fichier.display(), "panique");
        precedent(info);
    }));
}

fn rapport_de_panique(fil: &str, panique: &str, trace: &impl fmt::Display) -> String {
    format!(
        "Kobi {}\nfil : {fil}\n{panique}\n\n{trace}\n",
        jiff::Zoned::now().strftime("%Y-%m-%d %H:%M:%S %:z")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "le budget du projet Hermès est de 42 millions";

    fn lire_tout(dir: &Path) -> String {
        let mut tout = String::new();
        for n in 0..MAX_FILES {
            if let Ok(texte) = fs::read_to_string(numbered(dir, n)) {
                tout.push_str(&texte);
            }
        }
        tout
    }

    /// KOB-167 : une panique provoquée exprès apparaît dans le journal et
    /// dans un `crash-*.txt`.
    #[test]
    fn panique_dans_le_journal() {
        let dir = tempfile::tempdir().unwrap();
        let chemin = dir.path().to_owned();
        std::thread::Builder::new()
            .name("essai-panique".into())
            .spawn(move || {
                let abonne = subscriber(&chemin).unwrap();
                tracing::subscriber::with_default(abonne, || {
                    installer_crochet_panique(&chemin);
                    let r = std::panic::catch_unwind(|| panic!("panique provoquée exprès"));
                    let _ = std::panic::take_hook();
                    assert!(r.is_err());
                });
            })
            .unwrap()
            .join()
            .unwrap();
        let log = lire_tout(dir.path());
        assert!(log.contains("panique provoquée exprès"), "{log}");
        assert!(log.contains("essai-panique"), "{log}");
        // Le crochet est global : la panique d'un autre test lancé en
        // parallèle peut aussi laisser un rapport ici. Seul compte celui de
        // ce fil (KOB-376).
        let crash: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("crash-"))
            .map(|e| fs::read_to_string(e.path()).unwrap())
            .filter(|texte| texte.contains("fil : essai-panique"))
            .collect();
        assert_eq!(crash.len(), 1);
        assert!(crash[0].contains("panique provoquée exprès"));
    }

    #[test]
    fn texte_transcrit_jamais_dans_le_log() {
        let dir = tempfile::tempdir().unwrap();
        let abonne = subscriber(dir.path()).unwrap();
        tracing::subscriber::with_default(abonne, || {
            tracing::info!(text = SECRET, source = "eux", start = 12.5, "segment final");
            tracing::info!(question = SECRET, "question envoyée");
            tracing::warn!(answer = %SECRET, "réponse reçue");
            tracing::error!(notes = ?SECRET, prompt = SECRET, "notes mises à jour");
            let span = tracing::info_span!("reunion", title = SECRET);
            let _e = span.enter();
            tracing::info!(content = SECRET, transcript = SECRET, "dans la réunion");
        });

        let log = lire_tout(dir.path());
        assert!(!log.contains(SECRET), "{log}");
        assert!(!log.contains("Hermès"), "{log}");
        assert!(log.contains("segment final"), "{log}");
        assert!(log.contains("source=\"eux\""), "{log}");
        assert!(log.contains("start=12.5"), "{log}");
        assert!(log.contains("text=<masqué, 45 car.>"), "{log}");
        assert!(log.contains("answer=<masqué>"), "{log}");
        assert_eq!(log.lines().count(), 5, "{log}");
    }

    #[test]
    fn rotation_par_taille_garde_trois_fichiers() {
        let dir = tempfile::tempdir().unwrap();
        let fichier = RotatingFile::with_limits(dir.path(), 1000, MAX_FILES).unwrap();
        let ligne = format!("{}\n", "x".repeat(99));
        for i in 0..100 {
            let mut w = fichier.make_writer();
            write!(w, "{i:03}{}", &ligne[3..]).unwrap();
        }

        assert!(numbered(dir.path(), 0).is_file());
        assert!(numbered(dir.path(), 1).is_file());
        assert!(numbered(dir.path(), 2).is_file());
        assert!(!numbered(dir.path(), 3).exists());
        for n in 0..MAX_FILES {
            let taille = fs::metadata(numbered(dir.path(), n)).unwrap().len();
            assert!(taille <= 1000, "kobi.log.{n} : {taille} octets");
        }
        // Les lignes les plus récentes sont dans kobi.log, entières.
        let courant = fs::read_to_string(numbered(dir.path(), 0)).unwrap();
        assert!(courant.ends_with(&format!("099{}", &ligne[3..])));
        assert!(courant.lines().all(|l| l.len() == 99));
        let ancien = fs::read_to_string(numbered(dir.path(), 2)).unwrap();
        assert!(ancien.starts_with("070"), "{ancien}");
    }

    #[test]
    fn reprise_en_ajout() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..2 {
            let fichier = RotatingFile::open(dir.path()).unwrap();
            writeln!(fichier.make_writer(), "lancement {i}").unwrap();
        }
        let log = fs::read_to_string(dir.path().join(FILE_NAME)).unwrap();
        assert_eq!(log, "lancement 0\nlancement 1\n");
    }
}
