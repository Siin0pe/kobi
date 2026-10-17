//! Modèles téléchargés à la demande dans `~/Kobi/models/` (ils ne sont pas
//! dans l'installeur).
//!
//! Un manifeste donne pour chaque modèle son URL, sa taille et son SHA-256.
//! Le téléchargement écrit dans `<fichier>.part`, reprend là où il s'était
//! arrêté (en-tête `Range`), vérifie taille et somme, puis renomme le fichier.
//! Un fichier dont la somme est fausse n'est jamais gardé.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

/// Dossier des modèles : `~/Kobi/models`.
pub fn default_dir() -> Option<PathBuf> {
    crate::config::kobi_dir().map(|dir| dir.join("models"))
}

/// Suffixe du fichier en cours de téléchargement.
pub const PART_SUFFIX: &str = "part";

const BLOC: usize = 64 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ErreurModele {
    #[error(
        "hors ligne : impossible de joindre {hote} ({detail}). Vérifiez la connexion \
         puis relancez ; le téléchargement reprendra où il s'est arrêté"
    )]
    HorsLigne { hote: String, detail: String },
    #[error(
        "connexion coupée après {recu} octets sur {total} ; relancez, le \
         téléchargement reprendra à cet endroit ({detail})"
    )]
    Interrompu {
        recu: u64,
        total: u64,
        detail: String,
    },
    #[error("{url} : le serveur répond {statut}")]
    Http { url: String, statut: u16 },
    #[error(
        "{fichier} : somme SHA-256 fausse (attendue {attendue}, obtenue {obtenue}) ; \
         fichier supprimé"
    )]
    SommeFausse {
        fichier: String,
        attendue: String,
        obtenue: String,
    },
    #[error("{fichier} : {obtenue} octets reçus au lieu de {attendue} ; fichier supprimé")]
    TailleFausse {
        fichier: String,
        attendue: u64,
        obtenue: u64,
    },
    #[error("téléchargement annulé")]
    Annule,
    #[error("modèle inconnu du manifeste : {0}")]
    Inconnu(String),
    #[error("manifeste des modèles invalide : {0}")]
    Manifeste(String),
    #[error("{chemin} : {source}")]
    Io {
        chemin: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Un modèle téléchargeable.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Modele {
    /// Identifiant stable (`nemotron-int8`, `wake-kobi`…).
    pub id: String,
    /// Nom du fichier dans `models/`, sans dossier.
    pub fichier: String,
    pub url: String,
    /// Taille exacte en octets.
    pub taille: u64,
    /// SHA-256 en hexadécimal.
    pub sha256: String,
}

/// Manifeste des modèles, en TOML :
///
/// ```toml
/// [[modele]]
/// id = "wake-kobi"
/// fichier = "kobi.onnx"
/// url = "https://…/kobi.onnx"
/// taille = 1234
/// sha256 = "…"
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Manifeste {
    #[serde(default, rename = "modele")]
    pub modeles: Vec<Modele>,
}

impl Manifeste {
    pub fn parse(texte: &str) -> Result<Self, ErreurModele> {
        let manifeste: Manifeste =
            toml::from_str(texte).map_err(|e| ErreurModele::Manifeste(e.to_string()))?;
        for m in &manifeste.modeles {
            let nom_simple = !m.fichier.is_empty()
                && m.fichier != "."
                && m.fichier != ".."
                && !m.fichier.contains(['/', '\\', ':']);
            if !nom_simple {
                return Err(ErreurModele::Manifeste(format!(
                    "{} : nom de fichier refusé « {} »",
                    m.id, m.fichier
                )));
            }
            if m.sha256.len() != 64 || !m.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(ErreurModele::Manifeste(format!(
                    "{} : sha256 doit faire 64 caractères hexadécimaux",
                    m.id
                )));
            }
        }
        Ok(manifeste)
    }

    pub fn modele(&self, id: &str) -> Result<&Modele, ErreurModele> {
        self.modeles
            .iter()
            .find(|m| m.id == id)
            .ok_or_else(|| ErreurModele::Inconnu(id.to_owned()))
    }
}

/// Avancement, pour la barre de progression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progression {
    pub recu: u64,
    pub total: u64,
}

/// Flux ouvert par une [`Source`].
pub struct Flux {
    /// Octet à partir duquel le flux commence : la valeur demandée si le
    /// serveur accepte la reprise, 0 sinon.
    pub depuis: u64,
    pub lecteur: Box<dyn Read + Send>,
}

/// D'où viennent les octets : HTTP en vrai, une source simulée en test.
pub trait Source {
    /// Ouvre `url` à partir de l'octet `depuis`.
    fn ouvrir(&self, url: &str, depuis: u64) -> Result<Flux, ErreurModele>;
}

/// Source HTTP(S), avec reprise par `Range`.
#[derive(Debug, Clone)]
pub struct Http {
    agent: ureq::Agent,
}

impl Default for Http {
    fn default() -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .timeout_connect(std::time::Duration::from_secs(15))
                .timeout_read(std::time::Duration::from_secs(60))
                .user_agent(concat!("kobi/", env!("CARGO_PKG_VERSION")))
                .build(),
        }
    }
}

fn hote(url: &str) -> String {
    let sans_schema = url.split_once("://").map_or(url, |(_, reste)| reste);
    sans_schema
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(sans_schema)
        .to_owned()
}

impl Source for Http {
    fn ouvrir(&self, url: &str, depuis: u64) -> Result<Flux, ErreurModele> {
        let mut requete = self.agent.get(url);
        if depuis > 0 {
            requete = requete.set("Range", &format!("bytes={depuis}-"));
        }
        match requete.call() {
            Ok(reponse) => {
                let depuis = if reponse.status() == 206 { depuis } else { 0 };
                Ok(Flux {
                    depuis,
                    lecteur: Box::new(reponse.into_reader()),
                })
            }
            Err(ureq::Error::Status(statut, _)) => Err(ErreurModele::Http {
                url: url.to_owned(),
                statut,
            }),
            Err(ureq::Error::Transport(t)) => Err(ErreurModele::HorsLigne {
                hote: hote(url),
                detail: t.to_string(),
            }),
        }
    }
}

/// Résultat d'un téléchargement réussi.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fait {
    /// Le modèle était déjà là et vérifié.
    DejaPresent(PathBuf),
    Telecharge(PathBuf),
}

impl Fait {
    pub fn chemin(&self) -> &Path {
        match self {
            Fait::DejaPresent(p) | Fait::Telecharge(p) => p,
        }
    }
}

struct Hex<'a>(&'a [u8]);

impl fmt::Display for Hex<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|b| write!(f, "{b:02x}"))
    }
}

fn io_err(chemin: &Path) -> impl FnOnce(io::Error) -> ErreurModele + '_ {
    move |source| ErreurModele::Io {
        chemin: chemin.to_owned(),
        source,
    }
}

/// Hache un fichier existant (fichier final ou début d'un `.part`).
fn hacher(chemin: &Path, hasher: &mut Sha256) -> Result<u64, ErreurModele> {
    let mut fichier = File::open(chemin).map_err(io_err(chemin))?;
    let mut buf = vec![0u8; BLOC];
    let mut total = 0;
    loop {
        let n = fichier.read(&mut buf).map_err(io_err(chemin))?;
        if n == 0 {
            return Ok(total);
        }
        hasher.update(&buf[..n]);
        total += n as u64;
    }
}

fn verifie(modele: &Modele, taille: u64, hasher: Sha256) -> Result<(), ErreurModele> {
    if taille != modele.taille {
        return Err(ErreurModele::TailleFausse {
            fichier: modele.fichier.clone(),
            attendue: modele.taille,
            obtenue: taille,
        });
    }
    let obtenue = Hex(&hasher.finalize()).to_string();
    if !obtenue.eq_ignore_ascii_case(&modele.sha256) {
        return Err(ErreurModele::SommeFausse {
            fichier: modele.fichier.clone(),
            attendue: modele.sha256.to_ascii_lowercase(),
            obtenue,
        });
    }
    Ok(())
}

/// Télécharge `modele` dans `dossier`, ou le reprend, puis le vérifie.
///
/// `progression` est appelé après chaque bloc reçu ; il arrête le
/// téléchargement en renvoyant `ControlFlow::Break` (le `.part` est gardé
/// pour une reprise).
pub fn telecharger(
    source: &dyn Source,
    modele: &Modele,
    dossier: &Path,
    progression: &mut dyn FnMut(Progression) -> ControlFlow<()>,
) -> Result<Fait, ErreurModele> {
    fs::create_dir_all(dossier).map_err(io_err(dossier))?;
    let final_ = dossier.join(&modele.fichier);
    let part = dossier.join(format!("{}.{PART_SUFFIX}", modele.fichier));

    // Déjà là : vérifié avant d'être déclaré présent.
    if final_.is_file() {
        let mut hasher = Sha256::new();
        let taille = hacher(&final_, &mut hasher)?;
        if verifie(modele, taille, hasher).is_ok() {
            return Ok(Fait::DejaPresent(final_));
        }
        fs::remove_file(&final_).map_err(io_err(&final_))?;
    }

    // Reprise : on rehache ce qui est déjà reçu.
    let mut hasher = Sha256::new();
    let mut recu = if part.is_file() {
        hacher(&part, &mut hasher)?
    } else {
        0
    };
    if recu > modele.taille {
        hasher = Sha256::new();
        recu = 0;
    }

    if recu < modele.taille {
        let mut flux = source.ouvrir(&modele.url, recu)?;
        if flux.depuis != recu {
            // Le serveur ignore la reprise : on repart de zéro.
            hasher = Sha256::new();
            recu = 0;
        }
        let mut fichier = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(recu == 0)
            .append(recu > 0)
            .open(&part)
            .map_err(io_err(&part))?;
        let mut buf = vec![0u8; BLOC];
        loop {
            let n = match flux.lecteur.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    fichier.flush().map_err(io_err(&part))?;
                    return Err(ErreurModele::Interrompu {
                        recu,
                        total: modele.taille,
                        detail: e.to_string(),
                    });
                }
            };
            fichier.write_all(&buf[..n]).map_err(io_err(&part))?;
            hasher.update(&buf[..n]);
            recu += n as u64;
            if recu > modele.taille {
                break;
            }
            let avancement = Progression {
                recu,
                total: modele.taille,
            };
            if progression(avancement).is_break() {
                fichier.flush().map_err(io_err(&part))?;
                return Err(ErreurModele::Annule);
            }
        }
        fichier.sync_all().map_err(io_err(&part))?;
    }

    if let Err(e) = verifie(modele, recu, hasher) {
        // Somme ou taille fausse : on ne garde rien, la prochaine tentative
        // repartira de zéro.
        fs::remove_file(&part).map_err(io_err(&part))?;
        return Err(e);
    }
    fs::rename(&part, &final_).map_err(io_err(&final_))?;
    Ok(Fait::Telecharge(final_))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn donnees(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i * 31 % 251) as u8).collect()
    }

    fn sha(data: &[u8]) -> String {
        Hex(&Sha256::digest(data)).to_string()
    }

    fn modele(data: &[u8]) -> Modele {
        Modele {
            id: "essai".into(),
            fichier: "essai.bin".into(),
            url: "https://modeles.exemple/essai.bin".into(),
            taille: data.len() as u64,
            sha256: sha(data),
        }
    }

    /// Source simulée : sert `data`, peut refuser la reprise, couper la
    /// connexion après un nombre d'octets, ou être hors ligne.
    #[derive(Default)]
    struct Simulee {
        data: Vec<u8>,
        sans_reprise: bool,
        coupe_apres: Option<usize>,
        hors_ligne: bool,
        demandes: Arc<Mutex<Vec<u64>>>,
    }

    struct Coupe {
        reste: io::Cursor<Vec<u8>>,
        avant_coupure: Option<usize>,
    }

    impl Read for Coupe {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let max = match self.avant_coupure {
                Some(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::ConnectionReset,
                        "connexion réinitialisée",
                    ))
                }
                Some(n) => n.min(buf.len()),
                None => buf.len(),
            };
            let n = self.reste.read(&mut buf[..max])?;
            if let Some(reste) = &mut self.avant_coupure {
                *reste -= n;
            }
            Ok(n)
        }
    }

    impl Source for Simulee {
        fn ouvrir(&self, url: &str, depuis: u64) -> Result<Flux, ErreurModele> {
            self.demandes.lock().unwrap().push(depuis);
            if self.hors_ligne {
                return Err(ErreurModele::HorsLigne {
                    hote: hote(url),
                    detail: "nom d'hôte introuvable".into(),
                });
            }
            let depuis = if self.sans_reprise { 0 } else { depuis };
            Ok(Flux {
                depuis,
                lecteur: Box::new(Coupe {
                    reste: io::Cursor::new(self.data[depuis as usize..].to_vec()),
                    avant_coupure: self.coupe_apres,
                }),
            })
        }
    }

    fn continuer(_: Progression) -> ControlFlow<()> {
        ControlFlow::Continue(())
    }

    #[test]
    fn telechargement_simule_complet() {
        let dir = tempfile::tempdir().unwrap();
        let data = donnees(300_000);
        let source = Simulee {
            data: data.clone(),
            ..Default::default()
        };
        let mut vus = Vec::new();
        let fait = telecharger(&source, &modele(&data), dir.path(), &mut |p| {
            vus.push(p);
            ControlFlow::Continue(())
        })
        .unwrap();

        assert_eq!(fait, Fait::Telecharge(dir.path().join("essai.bin")));
        assert_eq!(fs::read(fait.chemin()).unwrap(), data);
        assert!(!dir.path().join("essai.bin.part").exists());
        assert!(vus.windows(2).all(|w| w[0].recu < w[1].recu));
        assert_eq!(
            vus.last(),
            Some(&Progression {
                recu: 300_000,
                total: 300_000
            })
        );

        // Second appel : rien n'est retéléchargé.
        let fait = telecharger(&source, &modele(&data), dir.path(), &mut continuer).unwrap();
        assert!(matches!(fait, Fait::DejaPresent(_)));
        assert_eq!(*source.demandes.lock().unwrap(), vec![0]);
    }

    #[test]
    fn somme_fausse_refusee() {
        let dir = tempfile::tempdir().unwrap();
        let data = donnees(10_000);
        let mut m = modele(&data);
        m.sha256 = sha(b"autre chose");
        let source = Simulee {
            data,
            ..Default::default()
        };

        let erreur = telecharger(&source, &m, dir.path(), &mut continuer).unwrap_err();

        assert!(
            matches!(erreur, ErreurModele::SommeFausse { .. }),
            "{erreur}"
        );
        assert!(erreur.to_string().contains("somme SHA-256 fausse"));
        assert!(!dir.path().join("essai.bin").exists());
        assert!(!dir.path().join("essai.bin.part").exists());
    }

    #[test]
    fn fichier_present_corrompu_retelecharge() {
        let dir = tempfile::tempdir().unwrap();
        let data = donnees(10_000);
        fs::write(dir.path().join("essai.bin"), b"abime").unwrap();
        let source = Simulee {
            data: data.clone(),
            ..Default::default()
        };
        let fait = telecharger(&source, &modele(&data), dir.path(), &mut continuer).unwrap();
        assert!(matches!(fait, Fait::Telecharge(_)));
        assert_eq!(fs::read(fait.chemin()).unwrap(), data);
    }

    #[test]
    fn reprise_apres_coupure() {
        let dir = tempfile::tempdir().unwrap();
        let data = donnees(200_000);
        let m = modele(&data);
        let demandes = Arc::new(Mutex::new(Vec::new()));

        let coupee = Simulee {
            data: data.clone(),
            coupe_apres: Some(70_000),
            demandes: Arc::clone(&demandes),
            ..Default::default()
        };
        let erreur = telecharger(&coupee, &m, dir.path(), &mut continuer).unwrap_err();
        assert!(
            matches!(erreur, ErreurModele::Interrompu { recu: 70_000, .. }),
            "{erreur}"
        );
        assert!(erreur.to_string().contains("reprendra"));
        let part = dir.path().join("essai.bin.part");
        assert_eq!(fs::metadata(&part).unwrap().len(), 70_000);

        let reprise = Simulee {
            data: data.clone(),
            demandes: Arc::clone(&demandes),
            ..Default::default()
        };
        let mut premier = None;
        let fait = telecharger(&reprise, &m, dir.path(), &mut |p| {
            premier.get_or_insert(p.recu);
            ControlFlow::Continue(())
        })
        .unwrap();

        assert_eq!(*demandes.lock().unwrap(), vec![0, 70_000]);
        assert!(premier.unwrap() > 70_000, "la progression repart de 70 000");
        assert_eq!(fs::read(fait.chemin()).unwrap(), data);
        assert!(!part.exists());
    }

    #[test]
    fn serveur_sans_reprise_repart_de_zero() {
        let dir = tempfile::tempdir().unwrap();
        let data = donnees(50_000);
        fs::write(dir.path().join("essai.bin.part"), &data[..20_000]).unwrap();
        let source = Simulee {
            data: data.clone(),
            sans_reprise: true,
            ..Default::default()
        };
        let fait = telecharger(&source, &modele(&data), dir.path(), &mut continuer).unwrap();
        assert_eq!(*source.demandes.lock().unwrap(), vec![20_000]);
        assert_eq!(fs::read(fait.chemin()).unwrap(), data);
    }

    #[test]
    fn part_trop_long_ignore() {
        let dir = tempfile::tempdir().unwrap();
        let data = donnees(1_000);
        fs::write(dir.path().join("essai.bin.part"), donnees(5_000)).unwrap();
        let source = Simulee {
            data: data.clone(),
            ..Default::default()
        };
        let fait = telecharger(&source, &modele(&data), dir.path(), &mut continuer).unwrap();
        assert_eq!(*source.demandes.lock().unwrap(), vec![0]);
        assert_eq!(fs::read(fait.chemin()).unwrap(), data);
    }

    #[test]
    fn annulation_garde_le_part() {
        let dir = tempfile::tempdir().unwrap();
        let data = donnees(300_000);
        let source = Simulee {
            data: data.clone(),
            ..Default::default()
        };
        let erreur = telecharger(&source, &modele(&data), dir.path(), &mut |p| {
            if p.recu >= 100_000 {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        })
        .unwrap_err();
        assert!(matches!(erreur, ErreurModele::Annule));
        let garde = fs::metadata(dir.path().join("essai.bin.part"))
            .unwrap()
            .len();
        assert!((100_000..300_000).contains(&garde), "{garde}");
    }

    #[test]
    fn hors_ligne_erreur_claire() {
        let dir = tempfile::tempdir().unwrap();
        let data = donnees(1_000);
        let source = Simulee {
            data,
            hors_ligne: true,
            ..Default::default()
        };
        let erreur = telecharger(
            &source,
            &modele(&donnees(1_000)),
            dir.path(),
            &mut continuer,
        )
        .unwrap_err()
        .to_string();
        assert!(erreur.starts_with("hors ligne"), "{erreur}");
        assert!(erreur.contains("modeles.exemple"), "{erreur}");
        assert!(erreur.contains("reprendra"), "{erreur}");
    }

    #[test]
    fn manifeste() {
        let data = donnees(10);
        let texte = format!(
            r#"
[[modele]]
id = "essai"
fichier = "essai.bin"
url = "https://modeles.exemple/essai.bin"
taille = 10
sha256 = "{}"
"#,
            sha(&data)
        );
        let m = Manifeste::parse(&texte).unwrap();
        assert_eq!(m.modele("essai").unwrap(), &modele(&data));
        assert!(matches!(m.modele("absent"), Err(ErreurModele::Inconnu(_))));
        assert_eq!(Manifeste::parse("").unwrap(), Manifeste::default());

        for (fichier, sha256) in [
            ("../evil.bin", sha(&data)),
            ("dossier/essai.bin", sha(&data)),
            ("C:essai.bin", sha(&data)),
            ("", sha(&data)),
            ("essai.bin", "abc".into()),
        ] {
            let texte = format!(
                "[[modele]]\nid = \"x\"\nfichier = \"{fichier}\"\nurl = \"u\"\ntaille = 1\nsha256 = \"{sha256}\"\n"
            );
            assert!(
                matches!(Manifeste::parse(&texte), Err(ErreurModele::Manifeste(_))),
                "{fichier} {sha256}"
            );
        }
    }

    #[test]
    fn hote_extrait_de_l_url() {
        assert_eq!(hote("https://huggingface.co/a/b?x=1"), "huggingface.co");
        assert_eq!(hote("http://127.0.0.1:8080/m.bin"), "127.0.0.1:8080");
    }

    /// Mini serveur HTTP local (une connexion par requête, `Range` géré) pour
    /// tester la vraie source HTTP sans Internet.
    fn serveur(data: Vec<u8>, accepte_range: bool) -> (String, Arc<Mutex<Vec<String>>>) {
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;

        let ecoute = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/essai.bin", ecoute.local_addr().unwrap());
        let ranges = Arc::new(Mutex::new(Vec::new()));
        let vus = Arc::clone(&ranges);
        std::thread::spawn(move || {
            for flux in ecoute.incoming() {
                let Ok(mut flux) = flux else { return };
                let mut lecteur = BufReader::new(flux.try_clone().unwrap());
                let mut depuis = 0usize;
                loop {
                    let mut ligne = String::new();
                    if lecteur.read_line(&mut ligne).unwrap_or(0) == 0 || ligne == "\r\n" {
                        break;
                    }
                    let basse = ligne.to_ascii_lowercase();
                    if let Some(v) = basse.strip_prefix("range: bytes=") {
                        vus.lock().unwrap().push(v.trim().to_owned());
                        if accepte_range {
                            depuis = v.trim().trim_end_matches('-').parse().unwrap();
                        }
                    }
                }
                let corps = &data[depuis..];
                let statut = if depuis > 0 {
                    "206 Partial Content"
                } else {
                    "200 OK"
                };
                let _ = write!(
                    flux,
                    "HTTP/1.1 {statut}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    corps.len()
                );
                let _ = flux.write_all(corps);
            }
        });
        (url, ranges)
    }

    #[test]
    fn http_local_avec_reprise() {
        let dir = tempfile::tempdir().unwrap();
        let data = donnees(150_000);
        let (url, ranges) = serveur(data.clone(), true);
        let mut m = modele(&data);
        m.url = url;
        fs::write(dir.path().join("essai.bin.part"), &data[..40_000]).unwrap();

        let fait = telecharger(&Http::default(), &m, dir.path(), &mut continuer).unwrap();

        assert_eq!(*ranges.lock().unwrap(), vec!["40000-".to_owned()]);
        assert_eq!(fs::read(fait.chemin()).unwrap(), data);
    }

    #[test]
    fn http_local_sans_reprise() {
        let dir = tempfile::tempdir().unwrap();
        let data = donnees(50_000);
        let (url, _) = serveur(data.clone(), false);
        let mut m = modele(&data);
        m.url = url;
        fs::write(dir.path().join("essai.bin.part"), &data[..10_000]).unwrap();

        let fait = telecharger(&Http::default(), &m, dir.path(), &mut continuer).unwrap();
        assert_eq!(fs::read(fait.chemin()).unwrap(), data);
    }

    #[test]
    fn http_port_ferme_hors_ligne() {
        // Un port fermé sur la machine : connexion refusée, sans Internet.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let erreur = Http::default()
            .ouvrir(&format!("http://127.0.0.1:{port}/m.bin"), 0)
            .err()
            .unwrap();
        assert!(matches!(erreur, ErreurModele::HorsLigne { .. }), "{erreur}");
    }
}
