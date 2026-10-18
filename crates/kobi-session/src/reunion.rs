//! Dossier d'une réunion (`~/Kobi/meetings/AAAA-MM-JJ-titre/`) et
//! transcription écrite au fil de l'eau dans `transcript.jsonl`.
//!
//! Chaque segment final est écrit en une ligne JSON, d'un seul appel, puis
//! poussé sur le disque : un arrêt brutal ne perd que le segment en cours.
//! À la relecture, une dernière ligne tronquée est ignorée.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use kobi_stt::Segment;

/// Nom du fichier de transcription dans le dossier de réunion.
pub const TRANSCRIPT: &str = "transcript.jsonl";

/// Nom de dossier quand la réunion n'a pas de titre exploitable.
pub const TITRE_PAR_DEFAUT: &str = "reunion";

/// Longueur maximale de la partie titre du nom de dossier.
pub const TITRE_MAX: usize = 60;

/// Dossier des réunions : `~/Kobi/meetings`.
pub fn default_dir() -> Option<PathBuf> {
    crate::config::kobi_dir().map(|dir| dir.join("meetings"))
}

/// Date du jour de la réunion (calendrier local, fourni par l'appelant).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Date {
    pub annee: u16,
    pub mois: u8,
    pub jour: u8,
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.annee, self.mois, self.jour)
    }
}

/// Titre nettoyé pour un nom de dossier : minuscules ASCII, accents retirés,
/// mots séparés par des tirets, au plus [`TITRE_MAX`] caractères.
pub fn nettoyer_titre(titre: &str) -> String {
    let mut sortie = String::new();
    for c in titre.chars().flat_map(sans_accent) {
        if c.is_ascii_alphanumeric() {
            sortie.push(c.to_ascii_lowercase());
        } else if !sortie.is_empty() && !sortie.ends_with('-') {
            sortie.push('-');
        }
    }
    if sortie.len() > TITRE_MAX {
        sortie.truncate(TITRE_MAX);
    }
    let sortie = sortie.trim_end_matches('-');
    if sortie.is_empty() {
        TITRE_PAR_DEFAUT.to_owned()
    } else {
        sortie.to_owned()
    }
}

fn sans_accent(c: char) -> impl Iterator<Item = char> {
    let remplace: &str = match c {
        'à' | 'á' | 'â' | 'ä' | 'ã' | 'å' | 'À' | 'Á' | 'Â' | 'Ä' | 'Ã' | 'Å' => "a",
        'ç' | 'Ç' => "c",
        'è' | 'é' | 'ê' | 'ë' | 'È' | 'É' | 'Ê' | 'Ë' => "e",
        'ì' | 'í' | 'î' | 'ï' | 'Ì' | 'Í' | 'Î' | 'Ï' => "i",
        'ñ' | 'Ñ' => "n",
        'ò' | 'ó' | 'ô' | 'ö' | 'õ' | 'Ò' | 'Ó' | 'Ô' | 'Ö' | 'Õ' => "o",
        'ù' | 'ú' | 'û' | 'ü' | 'Ù' | 'Ú' | 'Û' | 'Ü' => "u",
        'ý' | 'ÿ' | 'Ý' => "y",
        'œ' | 'Œ' => "oe",
        'æ' | 'Æ' => "ae",
        'ß' => "ss",
        _ => "",
    };
    let garde = remplace.is_empty().then_some(c);
    remplace.chars().chain(garde)
}

/// Nom du dossier : `AAAA-MM-JJ-titre`.
pub fn nom_de_dossier(date: Date, titre: Option<&str>) -> String {
    format!("{date}-{}", nettoyer_titre(titre.unwrap_or("")))
}

/// Réunion ouverte : son dossier et sa transcription.
#[derive(Debug)]
pub struct Reunion {
    dossier: PathBuf,
    transcript: File,
}

impl Reunion {
    /// Crée `meetings/AAAA-MM-JJ-titre/` (suffixe `-2`, `-3`… si le nom
    /// est pris) et un `transcript.jsonl` vide.
    pub fn creer(meetings: &Path, date: Date, titre: Option<&str>) -> io::Result<Self> {
        fs::create_dir_all(meetings)?;
        let base = nom_de_dossier(date, titre);
        let mut n = 1;
        let dossier = loop {
            let nom = if n == 1 {
                base.clone()
            } else {
                format!("{base}-{n}")
            };
            let chemin = meetings.join(nom);
            match fs::create_dir(&chemin) {
                Ok(()) => break chemin,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => n += 1,
                Err(e) => return Err(e),
            }
        };
        Self::ouvrir(dossier)
    }

    /// Rouvre une réunion existante (reprise après un arrêt brutal) : les
    /// segments suivants s'ajoutent à la fin. Une dernière ligne tronquée est
    /// close par un retour à la ligne pour ne pas souder le segment suivant.
    pub fn ouvrir(dossier: PathBuf) -> io::Result<Self> {
        let mut transcript = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(dossier.join(TRANSCRIPT))?;
        let len = transcript.metadata()?.len();
        if len > 0 {
            let mut dernier = [0u8];
            transcript.seek(SeekFrom::Start(len - 1))?;
            transcript.read_exact(&mut dernier)?;
            if dernier[0] != b'\n' {
                transcript.write_all(b"\n")?;
            }
        }
        Ok(Self {
            dossier,
            transcript,
        })
    }

    pub fn dossier(&self) -> &Path {
        &self.dossier
    }

    /// Nom du dossier (`AAAA-MM-JJ-titre`) : l'identifiant de la réunion.
    pub fn nom(&self) -> &str {
        self.dossier
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
    }

    /// Réécrit `notes.md` en entier (écriture atomique).
    pub fn ecrire_notes(&self, notes: &str) -> io::Result<()> {
        ecrire_atomique(&self.dossier.join(NOTES), notes.as_bytes())
    }

    /// Écrit `compte-rendu.md` (écriture atomique).
    pub fn ecrire_compte_rendu(&self, texte: &str) -> io::Result<()> {
        ecrire_atomique(&self.dossier.join(COMPTE_RENDU), texte.as_bytes())
    }
}

/// Notes prises pendant la réunion, réécrites à chaque bloc.
pub const NOTES: &str = "notes.md";

/// Compte rendu écrit à la fin.
pub const COMPTE_RENDU: &str = "compte-rendu.md";

/// Écrit `chemin` via un fichier temporaire renommé : jamais à moitié écrit.
pub(crate) fn ecrire_atomique(chemin: &Path, contenu: &[u8]) -> io::Result<()> {
    let mut tmp = chemin.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, contenu)?;
    fs::rename(&tmp, chemin)
}

impl Reunion {
    /// Écrit un segment final et le pousse sur le disque. Les segments
    /// provisoires sont ignorés (retourne `false`).
    pub fn ecrire_segment(&mut self, segment: &Segment) -> io::Result<bool> {
        if !segment.is_final {
            return Ok(false);
        }
        let mut ligne = serde_json::to_vec(segment).map_err(io::Error::other)?;
        ligne.push(b'\n');
        // Un seul appel : la ligne est entière ou absente du fichier.
        self.transcript.write_all(&ligne)?;
        self.transcript.sync_data()?;
        Ok(true)
    }
}

/// Transcription relue : les segments lisibles et le nombre de lignes
/// ignorées (tronquées ou corrompues).
#[derive(Debug, Default)]
pub struct Relue {
    pub segments: Vec<Segment>,
    pub ignorees: usize,
}

/// Relit `transcript.jsonl` ; les lignes illisibles sont comptées, pas fatales.
pub fn lire_transcript(chemin: &Path) -> io::Result<Relue> {
    let mut relue = Relue::default();
    for ligne in BufReader::new(File::open(chemin)?).split(b'\n') {
        let ligne = ligne?;
        if ligne.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match serde_json::from_slice(&ligne) {
            Ok(segment) => relue.segments.push(segment),
            Err(_) => relue.ignorees += 1,
        }
    }
    Ok(relue)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobi_stt::Source;

    const DATE: Date = Date {
        annee: 2026,
        mois: 10,
        jour: 17,
    };

    fn segment(i: usize) -> Segment {
        Segment {
            start: i as f64,
            end: i as f64 + 0.9,
            source: if i.is_multiple_of(2) {
                Source::Moi
            } else {
                Source::Eux
            },
            text: format!("phrase numéro {i}, avec « guillemets » et \"échappements\"\n"),
            is_final: true,
        }
    }

    #[test]
    fn titres_nettoyes() {
        for (titre, attendu) in [
            ("Point d'équipe", "point-d-equipe"),
            ("  Rétro   Sprint #42 !! ", "retro-sprint-42"),
            ("Œuvre / ça marche ?", "oeuvre-ca-marche"),
            ("../../etc/passwd", "etc-passwd"),
            ("C:\\Windows\\con", "c-windows-con"),
            ("🎉🎉", TITRE_PAR_DEFAUT),
            ("", TITRE_PAR_DEFAUT),
        ] {
            assert_eq!(nettoyer_titre(titre), attendu, "{titre}");
        }
        let long = nettoyer_titre(&"mot ".repeat(40));
        assert!(long.len() <= TITRE_MAX && !long.ends_with('-'), "{long}");
    }

    #[test]
    fn nom_avec_et_sans_titre() {
        assert_eq!(
            nom_de_dossier(DATE, Some("Démo client")),
            "2026-10-17-demo-client"
        );
        assert_eq!(nom_de_dossier(DATE, None), "2026-10-17-reunion");
    }

    #[test]
    fn deux_reunions_meme_nom() {
        let dir = tempfile::tempdir().unwrap();
        let a = Reunion::creer(dir.path(), DATE, Some("Point")).unwrap();
        let b = Reunion::creer(dir.path(), DATE, Some("point")).unwrap();
        assert!(a.dossier().ends_with("2026-10-17-point"));
        assert!(b.dossier().ends_with("2026-10-17-point-2"));
        assert!(a.dossier().join(TRANSCRIPT).is_file());
    }

    #[test]
    fn segments_finals_ecrits_au_fil_de_l_eau() {
        let dir = tempfile::tempdir().unwrap();
        let mut reunion = Reunion::creer(dir.path(), DATE, None).unwrap();
        let chemin = reunion.dossier().join(TRANSCRIPT);

        let mut provisoire = segment(0);
        provisoire.is_final = false;
        assert!(!reunion.ecrire_segment(&provisoire).unwrap());

        for i in 0..3 {
            assert!(reunion.ecrire_segment(&segment(i)).unwrap());
            // Lisible tout de suite, sans fermer le fichier.
            let relue = lire_transcript(&chemin).unwrap();
            assert_eq!(relue.segments.len(), i + 1);
            assert_eq!(relue.segments[i], segment(i));
        }
        let texte = fs::read_to_string(&chemin).unwrap();
        assert_eq!(texte.lines().count(), 3);
    }

    /// Processus enfant du test d'arrêt brutal : écrit 3 segments, commence
    /// le 4e sans le finir, puis meurt sans rien fermer.
    #[test]
    fn enfant_arret_brutal() {
        let Some(dossier) = std::env::var_os("KOBI_TEST_ARRET_BRUTAL") else {
            return;
        };
        let mut reunion = Reunion::ouvrir(PathBuf::from(dossier)).unwrap();
        for i in 0..3 {
            reunion.ecrire_segment(&segment(i)).unwrap();
        }
        let ligne = serde_json::to_string(&segment(3)).unwrap();
        reunion
            .transcript
            .write_all(&ligne.as_bytes()[..ligne.len() / 2])
            .unwrap();
        std::process::abort();
    }

    #[test]
    fn arret_brutal_ne_perd_que_le_segment_en_cours() {
        let dir = tempfile::tempdir().unwrap();
        let dossier = Reunion::creer(dir.path(), DATE, Some("crash"))
            .unwrap()
            .dossier()
            .to_owned();

        let statut = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "reunion::tests::enfant_arret_brutal",
                "--exact",
                "--nocapture",
            ])
            .env("KOBI_TEST_ARRET_BRUTAL", &dossier)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(!statut.success(), "l'enfant devait mourir brutalement");

        let chemin = dossier.join(TRANSCRIPT);
        let relue = lire_transcript(&chemin).unwrap();
        assert_eq!(relue.segments, (0..3).map(segment).collect::<Vec<_>>());
        assert_eq!(relue.ignorees, 1, "seul le segment en cours est perdu");

        // Reprise : le segment suivant ne se soude pas à la ligne tronquée.
        let mut reprise = Reunion::ouvrir(dossier).unwrap();
        reprise.ecrire_segment(&segment(4)).unwrap();
        let relue = lire_transcript(&chemin).unwrap();
        assert_eq!(relue.segments.len(), 4);
        assert_eq!(relue.segments[3], segment(4));
        assert_eq!(relue.ignorees, 1);
    }
}
