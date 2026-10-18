//! Premier lancement guidé (KOB-144) : la fenêtre « bienvenue » vérifie
//! Claude Code, le micro, le modèle de transcription (téléchargé avec sa
//! progression), les dossiers de code, le démarrage avec le système, fait
//! dire « Kobi, bonjour » et rappelle les raccourcis.
//!
//! Elle s'ouvre tant que le parcours n'est pas terminé (fichier
//! `~/Kobi/premier-lancement.fait`), jamais quand Kobi est lancé par le
//! système à l'ouverture de session, et se rejoue depuis les réglages.

use std::fs::{self, File};
use std::io::{self, Read};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use kobi_session::config;
use kobi_session::modeles::{self, Http, Modele, Progression};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};

/// Libellé de la fenêtre du premier lancement.
pub const FENETRE: &str = "bienvenue";

/// Fichier posé dans `~/Kobi` quand le parcours est terminé.
const MARQUE: &str = "premier-lancement.fait";

/// Clé de `[audio]` dans `config.toml` : nom du micro choisi.
const CLE_MICRO: &str = "micro";

/// Événements du téléchargement du modèle.
pub const EVENEMENT_PROGRESSION: &str = "modele://progression";
pub const EVENEMENT_FINI: &str = "modele://fini";
pub const EVENEMENT_ERREUR: &str = "modele://erreur";

/// Paquet Nemotron int8 560 ms (KOB-132), archive de sherpa-onnx.
const NOM_MODELE: &str = "sherpa-onnx-nemotron-3.5-asr-streaming-0.6b-560ms-int8-2026-06-11";
const URL_MODELE: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemotron-3.5-asr-streaming-0.6b-560ms-int8-2026-06-11.tar.bz2";
const TAILLE_MODELE: u64 = 475_271_763;
const SHA_MODELE: &str = "c6bf5e0df765f9d5b43bc9e0536d4b4b3e7d40bdf5ecf13e45f134c51c05ae3a";

/// Le modèle à télécharger. `KOBI_MODELE_URL` remplace l'URL (miroir,
/// essais hors ligne) ; la taille et la somme restent celles de l'archive.
fn modele() -> Modele {
    Modele {
        id: "nemotron-int8".into(),
        fichier: format!("{NOM_MODELE}.tar.bz2"),
        url: std::env::var("KOBI_MODELE_URL").unwrap_or_else(|_| URL_MODELE.into()),
        taille: TAILLE_MODELE,
        sha256: SHA_MODELE.into(),
    }
}

// ----- Ouverture -----

/// Le parcours reste à faire dans ce dossier de Kobi.
pub fn a_faire(kobi_dir: &Path) -> bool {
    !kobi_dir.join(MARQUE).exists()
}

/// Au démarrage : ouvre la bienvenue sur un profil neuf.
pub fn installer<R: Runtime>(app: &AppHandle<R>) {
    app.manage(Telechargement::default());
    let Some(dir) = config::kobi_dir() else {
        return;
    };
    if a_faire(&dir) && !crate::demarrage::lance_par_le_systeme() {
        // Kobi apparaît d'abord, puis se présente : la bienvenue s'ouvre une
        // fois la mascotte posée à l'écran, pour se ranger à côté d'elle.
        let app = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(800));
            let h = app.clone();
            let _ = app.run_on_main_thread(move || {
                if let Err(e) = ouvrir(&h) {
                    tracing::warn!(erreur = %e, "bienvenue");
                }
            });
        });
    }
}

/// Ouvre la bienvenue, ou la ramène devant.
pub fn ouvrir<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    if let Some(w) = app.get_webview_window(FENETRE) {
        let _ = w.unminimize();
        w.show()?;
        return w.set_focus();
    }
    crate::placement::a_cote_de_kobi_ou_centre(
        app,
        WebviewWindowBuilder::new(app, FENETRE, WebviewUrl::App("bienvenue.html".into()))
            .title("Bienvenue — Kobi")
            .inner_size(900.0, 640.0)
            .min_inner_size(680.0, 520.0)
            .visible(true)
            .focused(true),
        (900.0, 640.0),
    )
    .build()?;
    tracing::info!("premier lancement ouvert");
    Ok(())
}

#[tauri::command]
pub fn ouvrir_bienvenue<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    ouvrir(&app).map_err(|e| e.to_string())
}

fn marquer_fait(kobi_dir: &Path) -> io::Result<()> {
    fs::create_dir_all(kobi_dir)?;
    fs::write(kobi_dir.join(MARQUE), b"")
}

/// Fin du parcours : il ne se rouvrira plus tout seul.
#[tauri::command]
pub fn terminer_bienvenue<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    let dir = config::kobi_dir().ok_or("dossier personnel introuvable")?;
    marquer_fait(&dir).map_err(|e| format!("{} : {e}", dir.display()))?;
    tracing::info!("premier lancement terminé");
    if let Some(w) = app.get_webview_window(FENETRE) {
        let _ = w.close();
    }
    Ok(())
}

// ----- Claude Code -----

/// État de Claude Code, montré à l'étape « Claude ».
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "etat", rename_all = "kebab-case")]
pub enum EtatClaude {
    Pret { version: String, programme: PathBuf },
    NonConnecte { version: String, programme: PathBuf },
    Introuvable,
    Casse { programme: PathBuf, raison: String },
}

impl From<kobi_brain::claude_code::detect::Detection> for EtatClaude {
    fn from(d: kobi_brain::claude_code::detect::Detection) -> Self {
        use kobi_brain::claude_code::detect::Detection;
        match d {
            Detection::Ready { install, .. } => EtatClaude::Pret {
                version: install.version,
                programme: install.program,
            },
            Detection::NotLoggedIn(install) => EtatClaude::NonConnecte {
                version: install.version,
                programme: install.program,
            },
            Detection::NotFound => EtatClaude::Introuvable,
            Detection::Broken { program, reason } => EtatClaude::Casse {
                programme: program,
                raison: reason,
            },
        }
    }
}

/// Cherche `claude` et lit son état de connexion (sans appeler l'API).
#[tauri::command]
pub async fn etat_claude() -> EtatClaude {
    use kobi_brain::claude_code::detect;
    detect::detect(detect::PROBE_TIMEOUT).await.into()
}

// ----- Micro -----

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Micro {
    pub nom: String,
    pub defaut: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Micros {
    pub micros: Vec<Micro>,
    /// Micro choisi dans `config.toml`, sinon celui par défaut du système.
    pub choisi: Option<String>,
    /// La liste n'a pas pu être lue (pas de pilote audio…).
    pub erreur: Option<String>,
}

fn lire_config() -> config::Config {
    config::default_path()
        .map(|p| config::load_or_create(&p).config)
        .unwrap_or_default()
}

/// Micro choisi (`[audio] micro = "…"`), `None` pour celui du système.
pub fn micro_de(config: &config::Config) -> Option<String> {
    config
        .audio
        .extra
        .get(CLE_MICRO)
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .filter(|s| !s.is_empty())
}

/// Micro choisi dans la configuration courante.
pub fn micro_choisi() -> Option<String> {
    micro_de(&lire_config())
}

/// Écrit le micro choisi dans `config.toml` (`None` : celui du système).
pub fn regler_micro(chemin: &Path, nom: Option<&str>) -> Result<(), String> {
    let mut lu = config::load_or_create(chemin);
    if let Some(probleme) = lu.problem {
        return Err(format!("réglage non enregistré : {probleme}"));
    }
    match nom.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => {
            lu.config
                .audio
                .extra
                .insert(CLE_MICRO.into(), toml::Value::String(n.into()));
        }
        None => {
            lu.config.audio.extra.remove(CLE_MICRO);
        }
    }
    config::save(&lu.config, chemin).map_err(|e| e.to_string())
}

/// Sous Linux, cpal liste aussi les greffons ALSA (convertisseurs, JACK,
/// OSS…) comme des entrées : ce ne sont pas des micros à proposer.
fn micro_proposable(nom: &str) -> bool {
    const GREFFONS: [&str; 5] = [
        "Plugin",
        "Plug-in",
        "JACK Audio",
        "Open Sound System",
        "Discard all samples",
    ];
    !(cfg!(target_os = "linux") && GREFFONS.iter().any(|g| nom.contains(g)))
}

#[tauri::command]
pub fn micros() -> Micros {
    let choisi = micro_choisi();
    match kobi_audio::list_devices() {
        Ok(liste) => Micros {
            micros: liste
                .inputs
                .into_iter()
                .filter(|d| d.is_default || micro_proposable(&d.name))
                .map(|d| Micro {
                    nom: d.name,
                    defaut: d.is_default,
                })
                .collect(),
            choisi,
            erreur: None,
        },
        Err(e) => Micros {
            micros: Vec::new(),
            choisi,
            erreur: Some(format!("{e:#}")),
        },
    }
}

#[tauri::command]
pub fn choisir_micro(nom: Option<String>) -> Result<(), String> {
    let chemin = config::default_path().ok_or("dossier personnel introuvable")?;
    regler_micro(&chemin, nom.as_deref())?;
    tracing::info!(micro = ?nom, "micro choisi");
    Ok(())
}

// ----- Raccourcis -----

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Raccourcis {
    /// Parler à Kobi (`hotkey` de `config.toml`).
    pub parler: String,
    /// Enregistrer une réunion.
    pub reunion: String,
}

#[tauri::command]
pub fn raccourcis() -> Raccourcis {
    let config = lire_config();
    let defaut = config::Config::default();
    // Un raccourci illisible est remplacé par le défaut : on montre celui
    // qui marche vraiment.
    let valide = |cle: &str, valeur: &str, defaut: &str| match crate::raccourci::lire(cle, valeur, defaut) {
        (_, None) => valeur.to_owned(),
        (_, Some(_)) => defaut.to_owned(),
    };
    Raccourcis {
        parler: valide("hotkey", &config.hotkey, &defaut.hotkey),
        reunion: valide("hotkey_record", &config.hotkey_record, &defaut.hotkey_record),
    }
}

// ----- Modèle de transcription -----

/// Téléchargement en cours (un seul à la fois) et son annulation.
#[derive(Default)]
pub struct Telechargement {
    en_cours: AtomicBool,
    annule: Arc<AtomicBool>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EtatModele {
    /// Dossier du modèle, s'il est installé.
    pub dossier: Option<PathBuf>,
    pub nom: String,
    /// Taille du téléchargement, en octets.
    pub taille: u64,
    pub en_cours: bool,
}

/// Un paquet Nemotron complet (l'encodeur est le plus gros fichier).
fn paquet_complet(dossier: &Path) -> bool {
    dossier.join("encoder.int8.onnx").is_file() && dossier.join("tokens.txt").is_file()
}

#[tauri::command]
pub fn etat_modele<R: Runtime>(app: AppHandle<R>) -> EtatModele {
    EtatModele {
        dossier: crate::interpellation::modele_nemotron().filter(|d| paquet_complet(d)),
        nom: NOM_MODELE.into(),
        taille: TAILLE_MODELE,
        en_cours: app
            .try_state::<Telechargement>()
            .is_some_and(|t| t.en_cours.load(Ordering::SeqCst)),
    }
}

/// Avancement envoyé à l'interface.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Avancement {
    /// `telechargement` puis `extraction`.
    pub etape: &'static str,
    pub recu: u64,
    pub total: u64,
}

/// Lecteur qui compte les octets lus (avancement de l'extraction).
struct Compteur<L, F> {
    lecteur: L,
    lu: u64,
    rappel: F,
}

impl<L: Read, F: FnMut(u64)> Read for Compteur<L, F> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.lecteur.read(buf)?;
        self.lu += n as u64;
        (self.rappel)(self.lu);
        Ok(n)
    }
}

/// Extrait l'archive `.tar.bz2` dans `models/` en passant par un dossier
/// temporaire : un paquet à moitié extrait n'est jamais pris pour bon.
pub fn extraire(
    archive: &Path,
    models: &Path,
    avancement: &mut dyn FnMut(u64, u64),
) -> io::Result<PathBuf> {
    let total = fs::metadata(archive)?.len();
    let temp = models.join(".extraction");
    let _ = fs::remove_dir_all(&temp);
    fs::create_dir_all(&temp)?;
    let mut dernier = 0;
    let lecteur = Compteur {
        lecteur: File::open(archive)?,
        lu: 0,
        rappel: |lu: u64| {
            // Un événement par mégaoctet suffit à la barre.
            if lu - dernier >= 1 << 20 || lu == total {
                dernier = lu;
                avancement(lu, total);
            }
        },
    };
    // `unpack` refuse les chemins qui sortent du dossier.
    if let Err(e) = tar::Archive::new(bzip2::read::BzDecoder::new(lecteur)).unpack(&temp) {
        let _ = fs::remove_dir_all(&temp);
        return Err(e);
    }
    let mut paquets = fs::read_dir(&temp)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir());
    let paquet = paquets
        .next()
        .ok_or_else(|| io::Error::other("archive vide"))?;
    let nom = paquet
        .file_name()
        .ok_or_else(|| io::Error::other("archive sans dossier"))?;
    let final_ = models.join(nom);
    let _ = fs::remove_dir_all(&final_);
    fs::rename(&paquet, &final_)?;
    let _ = fs::remove_dir_all(&temp);
    Ok(final_)
}

/// Télécharge puis extrait le modèle, en signalant l'avancement.
fn installer_modele<R: Runtime>(
    app: &AppHandle<R>,
    annule: &AtomicBool,
) -> Result<PathBuf, String> {
    let models = modeles::default_dir().ok_or("dossier personnel introuvable")?;
    let modele = modele();
    let mut envoye = 0u64;
    let fait = modeles::telecharger(&Http::default(), &modele, &models, &mut |p: Progression| {
        if annule.load(Ordering::SeqCst) {
            return ControlFlow::Break(());
        }
        if p.recu - envoye >= 1 << 20 || p.recu == p.total {
            envoye = p.recu;
            let _ = app.emit(
                EVENEMENT_PROGRESSION,
                Avancement {
                    etape: "telechargement",
                    recu: p.recu,
                    total: p.total,
                },
            );
        }
        ControlFlow::Continue(())
    })
    .map_err(|e| e.to_string())?;
    let archive = fait.chemin().to_owned();
    let dossier = extraire(&archive, &models, &mut |recu, total| {
        let _ = app.emit(
            EVENEMENT_PROGRESSION,
            Avancement {
                etape: "extraction",
                recu,
                total,
            },
        );
    })
    .map_err(|e| format!("extraction : {e}"))?;
    let _ = fs::remove_file(&archive);
    Ok(dossier)
}

/// Lance le téléchargement du modèle dans un fil à part ; l'interface suit
/// les événements `modele://…`. Sans effet s'il tourne déjà.
#[tauri::command]
pub fn telecharger_modele<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    let etat = app
        .try_state::<Telechargement>()
        .ok_or("téléchargement indisponible")?;
    if etat.en_cours.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    etat.annule.store(false, Ordering::SeqCst);
    let annule = Arc::clone(&etat.annule);
    let app = app.clone();
    std::thread::Builder::new()
        .name("modele".into())
        .spawn(move || {
            tracing::info!("téléchargement du modèle");
            let resultat = installer_modele(&app, &annule);
            if let Some(etat) = app.try_state::<Telechargement>() {
                etat.en_cours.store(false, Ordering::SeqCst);
            }
            match resultat {
                Ok(dossier) => {
                    tracing::info!(dossier = %dossier.display(), "modèle installé");
                    let _ = app.emit(EVENEMENT_FINI, &dossier);
                }
                Err(e) => {
                    tracing::warn!(erreur = %e, "modèle");
                    let _ = app.emit(EVENEMENT_ERREUR, &e);
                }
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Arrête le téléchargement ; la partie reçue est gardée pour reprendre.
#[tauri::command]
pub fn annuler_telechargement<R: Runtime>(app: AppHandle<R>) {
    if let Some(etat) = app.try_state::<Telechargement>() {
        etat.annule.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marque_du_parcours() {
        let dir = tempfile::tempdir().unwrap();
        let kobi = dir.path().join("Kobi");
        assert!(a_faire(&kobi));
        marquer_fait(&kobi).unwrap();
        assert!(!a_faire(&kobi));
    }

    #[test]
    fn micro_enregistre_dans_la_config() {
        let dir = tempfile::tempdir().unwrap();
        let chemin = dir.path().join("config.toml");
        regler_micro(&chemin, Some("Micro USB")).unwrap();
        let lu = config::load_or_create(&chemin).config;
        assert_eq!(micro_de(&lu).as_deref(), Some("Micro USB"));
        assert!(fs::read_to_string(&chemin)
            .unwrap()
            .contains("micro = \"Micro USB\""));
        // Le reste de la config est gardé ; vide = micro du système.
        regler_micro(&chemin, Some("  ")).unwrap();
        assert_eq!(micro_de(&config::load_or_create(&chemin).config), None);
    }

    #[test]
    fn greffons_alsa_pas_proposes_comme_micros() {
        assert!(micro_proposable("Micro USB"));
        let greffon = !micro_proposable("Rate Converter Plugin Using Speex Resampler");
        assert_eq!(greffon, cfg!(target_os = "linux"));
    }

    #[test]
    fn modele_connu_du_manifeste() {
        let m = modele();
        assert_eq!(m.taille, TAILLE_MODELE);
        assert_eq!(m.sha256.len(), 64);
        assert!(m.fichier.ends_with(".tar.bz2") && !m.fichier.contains('/'));
    }

    /// Archive `.tar.bz2` d'un faux paquet, comme celles de sherpa-onnx.
    fn archive(dir: &Path, nom: &str) -> PathBuf {
        let chemin = dir.join("paquet.tar.bz2");
        let bz = bzip2::write::BzEncoder::new(
            File::create(&chemin).unwrap(),
            bzip2::Compression::fast(),
        );
        let mut tar = tar::Builder::new(bz);
        for (fichier, taille) in [("encoder.int8.onnx", 3 << 20), ("tokens.txt", 10)] {
            let donnees = vec![7u8; taille];
            let mut entete = tar::Header::new_gnu();
            entete.set_size(donnees.len() as u64);
            entete.set_mode(0o644);
            entete.set_cksum();
            tar.append_data(&mut entete, format!("{nom}/{fichier}"), &donnees[..])
                .unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap();
        chemin
    }

    #[test]
    fn extraction_dans_models_avec_avancement() {
        let dir = tempfile::tempdir().unwrap();
        let models = dir.path().join("models");
        fs::create_dir_all(&models).unwrap();
        let archive = archive(dir.path(), "sherpa-onnx-nemotron-essai");
        let mut pas = Vec::new();
        let paquet = extraire(&archive, &models, &mut |recu, total| {
            pas.push((recu, total))
        })
        .unwrap();
        assert_eq!(paquet, models.join("sherpa-onnx-nemotron-essai"));
        assert!(paquet_complet(&paquet));
        assert!(!models.join(".extraction").exists());
        let total = fs::metadata(&archive).unwrap().len();
        assert_eq!(pas.last(), Some(&(total, total)));
        assert!(pas.windows(2).all(|w| w[0].0 <= w[1].0));
    }

    #[test]
    fn archive_corrompue_refusee_sans_paquet_partiel() {
        let dir = tempfile::tempdir().unwrap();
        let models = dir.path().join("models");
        fs::create_dir_all(&models).unwrap();
        let archive = dir.path().join("abime.tar.bz2");
        fs::write(&archive, b"pas une archive").unwrap();
        assert!(extraire(&archive, &models, &mut |_, _| {}).is_err());
        assert_eq!(fs::read_dir(&models).unwrap().flatten().count(), 0);
    }

    /// Commandes du premier lancement depuis la webview ; une commande pas
    /// encore livrée (KOB-138, KOB-143) répond « … not found », ce que
    /// l'interface reconnaît (`absente()` dans ui/src/ipc.ts).
    #[test]
    fn commandes_par_ipc() {
        use crate::test_ipc::appeler;
        use serde_json::json;
        use tauri::test::{mock_builder, mock_context, noop_assets};

        let app = crate::session::installer(mock_builder())
            .build(mock_context(noop_assets()))
            .unwrap();
        app.manage(Telechargement::default());
        let w = WebviewWindowBuilder::new(&app, FENETRE, Default::default())
            .build()
            .unwrap();

        let r = appeler(&w, "raccourcis", json!({})).unwrap();
        assert!(r["reunion"].as_str().is_some_and(|p| !p.is_empty()));
        assert!(r["parler"].as_str().is_some_and(|p| !p.is_empty()));

        let m = appeler(&w, "etat_modele", json!({})).unwrap();
        assert_eq!(m["taille"], TAILLE_MODELE);
        assert_eq!(m["en_cours"], false);

        let absente = appeler(&w, "commande_pas_encore_livree", json!({})).unwrap_err();
        assert!(
            absente
                .as_str()
                .is_some_and(|t| t.contains("commande_pas_encore_livree not found")),
            "{absente}"
        );
    }
}
