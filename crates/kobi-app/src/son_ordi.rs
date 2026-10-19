//! Son de l'ordinateur (KOB-164) : en réunion, et seulement si l'utilisateur
//! l'a demandé (`[audio] sources = "both"`), une deuxième piste « eux »
//! capte la sortie audio en loopback (la voix des autres en visio) et la
//! transcrit à côté du micro. Désactivé par défaut : la sortie n'est alors
//! jamais ouverte.
//!
//! Un fil à part, indépendant de l'écoute du micro ([`crate::ecoute`]) :
//! le son de l'ordinateur ne déclenche jamais « Kobi ». Le réglage est suivi
//! à chaud (commande de l'app, ou `config.toml` modifié par la CLI) : la
//! piste s'ouvre ou se ferme pendant la réunion sans couper l'enregistrement.
//! La voix de Kobi (KOB-142), qui sort elle aussi par la sortie audio, est
//! remplacée par du silence et ses échos écartés.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use kobi_audio::{Capture, CaptureOptions};
use kobi_session::config::{self, AudioSources, Config};
use kobi_stt::nemotron::NemotronEngine;
use kobi_stt::SttEngine;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::chef::{publier, Message};
use crate::interpellation::charger_stt;
use crate::porte::Porte;
use crate::session::etat;

/// Événement envoyé à l'interface quand la capture s'ouvre ou se ferme.
pub const EVENEMENT: &str = "son_ordi";
/// Période de vérification du réglage et de l'état de la réunion.
const PERIODE: Duration = Duration::from_millis(500);
/// Après une phrase de Kobi, sa voix peut encore traîner dans la sortie.
const TRAINE_VOIX: Duration = Duration::from_millis(800);
/// Après un échec d'ouverture de la sortie, nouvel essai après ce délai.
const REESSAI: Duration = Duration::from_secs(10);

/// Réglage du son de l'ordinateur, tel que l'app le montre.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Reglage {
    pub actif: bool,
    /// Sortie choisie ; `None` : celle du système.
    pub sortie: Option<String>,
}

impl Reglage {
    fn depuis(config: &Config) -> Self {
        Self {
            actif: config.audio.sources.son_ordi(),
            sortie: config.audio.sortie_choisie().map(String::from),
        }
    }
}

/// État partagé : réglage courant et capture en cours.
pub struct SonOrdi {
    reglage: Mutex<Reglage>,
    capte: AtomicBool,
    chemin: Option<PathBuf>,
}

/// Lance le fil du son de l'ordinateur.
pub fn installer<R: Runtime>(app: &AppHandle<R>, config: &Config, chemin: Option<PathBuf>) {
    let reglage = Reglage::depuis(config);
    tracing::info!(
        actif = reglage.actif,
        sortie = ?reglage.sortie,
        "son de l'ordinateur : {}",
        if reglage.actif {
            "capté en réunion"
        } else {
            "désactivé, aucune capture de la sortie"
        }
    );
    app.manage(SonOrdi {
        reglage: Mutex::new(reglage),
        capte: AtomicBool::new(false),
        chemin,
    });
    let app = app.clone();
    std::thread::Builder::new()
        .name("son-ordi".into())
        .spawn(move || Fil::new(app).boucle())
        .expect("fil du son de l'ordinateur");
}

fn reglage<R: Runtime>(app: &AppHandle<R>) -> Option<Reglage> {
    let s = app.try_state::<SonOrdi>()?;
    let r = s.reglage.lock().unwrap_or_else(|e| e.into_inner());
    Some(r.clone())
}

fn date(chemin: &Path) -> Option<SystemTime> {
    std::fs::metadata(chemin).and_then(|m| m.modified()).ok()
}

/// Piste « eux » ouverte : capture et transcription.
struct Piste {
    capture: Capture,
    porte: Porte,
}

struct Fil<R: Runtime> {
    app: AppHandle<R>,
    /// Chargé à la première réunion avec le son de l'ordinateur, libéré à la
    /// fin de la réunion.
    stt: Option<NemotronEngine>,
    piste: Option<Piste>,
    /// Dernière modification connue de `config.toml`.
    config_lue: Option<SystemTime>,
    /// Dernière fois que Kobi parlait.
    voix_kobi: Option<Instant>,
    echec: Option<Instant>,
}

impl<R: Runtime> Fil<R> {
    fn new(app: AppHandle<R>) -> Self {
        let config_lue = app
            .try_state::<SonOrdi>()
            .and_then(|s| s.chemin.as_deref().and_then(date));
        Self {
            app,
            stt: None,
            piste: None,
            config_lue,
            voix_kobi: None,
            echec: None,
        }
    }

    fn boucle(mut self) {
        let mut verifie = Instant::now() - PERIODE;
        loop {
            if verifie.elapsed() >= PERIODE {
                verifie = Instant::now();
                self.relire_config();
                self.suivre();
            }
            match &self.piste {
                Some(_) => self.trame(),
                None => std::thread::sleep(PERIODE),
            }
        }
    }

    /// `config.toml` modifié hors de l'app (CLI, à la main) : le réglage suit.
    fn relire_config(&mut self) {
        let Some(chemin) = self
            .app
            .try_state::<SonOrdi>()
            .and_then(|s| s.chemin.clone())
        else {
            return;
        };
        let lue = date(&chemin);
        if lue == self.config_lue {
            return;
        }
        self.config_lue = lue;
        let charge = config::load_or_create(&chemin);
        if charge.problem.is_some() {
            return;
        }
        appliquer(&self.app, Reglage::depuis(&charge.config), "config.toml");
    }

    /// Ouvre ou ferme la piste selon le réglage et la réunion.
    fn suivre(&mut self) {
        let Some(reglage) = reglage(&self.app) else {
            return;
        };
        let session = etat(&self.app).etat;
        let en_reunion = matches!(session, "reunion" | "pause" | "fin");
        let voulu = reglage.actif && session == "reunion";
        match (&self.piste, voulu) {
            (None, true) if self.echec.is_none_or(|t| t.elapsed() >= REESSAI) => {
                self.ouvrir(reglage.sortie)
            }
            (Some(_), false) => self.fermer(if reglage.actif {
                "réunion en pause ou terminée"
            } else {
                "réglage désactivé"
            }),
            _ => {}
        }
        if !en_reunion && self.stt.take().is_some() {
            tracing::info!("son de l'ordinateur : modèle libéré, fin de réunion");
        }
    }

    fn ouvrir(&mut self, sortie: Option<String>) {
        if self.stt.is_none() {
            match charger_stt() {
                Ok(stt) => self.stt = Some(stt),
                Err(e) => {
                    tracing::warn!(erreur = %e, "son de l'ordinateur : transcription indisponible");
                    self.echec = Some(Instant::now());
                    return;
                }
            }
        }
        let capture = Capture::start(&CaptureOptions {
            sources: vec![kobi_audio::Source::Eux],
            mic: None,
            output: sortie.clone(),
        });
        match capture {
            Ok(capture) => {
                let sortie_ouverte = capture
                    .sources()
                    .iter()
                    .map(|s| s.device.clone())
                    .collect::<Vec<_>>()
                    .join(", ");
                tracing::info!(sortie = %sortie_ouverte, "son de l'ordinateur : capture ouverte (piste « eux »)");
                if let Some(stt) = &mut self.stt {
                    stt.reset();
                }
                self.piste = Some(Piste {
                    capture,
                    porte: Porte::new(kobi_stt::SAMPLE_RATE),
                });
                self.echec = None;
                self.signaler(true);
            }
            Err(e) => {
                tracing::warn!(erreur = %format!("{e:#}"), ?sortie, "son de l'ordinateur : sortie indisponible");
                self.echec = Some(Instant::now());
            }
        }
    }

    fn fermer(&mut self, raison: &str) {
        if self.piste.take().is_some() {
            tracing::info!(%raison, "son de l'ordinateur : capture fermée");
            self.signaler(false);
        }
    }

    fn signaler(&self, capte: bool) {
        if let Some(s) = self.app.try_state::<SonOrdi>() {
            s.capte.store(capte, Ordering::Relaxed);
        }
        let _ = self.app.emit(EVENEMENT, capte);
    }

    fn trame(&mut self) {
        let Some(piste) = &mut self.piste else { return };
        let frame = match piste
            .capture
            .frames()
            .recv_timeout(Duration::from_millis(200))
        {
            Ok(frame) => frame,
            // Rien ne joue : WASAPI n'envoie rien en loopback.
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => return,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                tracing::warn!("son de l'ordinateur : la sortie s'est arrêtée");
                self.piste = None;
                self.signaler(false);
                self.echec = Some(Instant::now());
                return;
            }
        };
        // La voix de Kobi passe par la sortie : silence à sa place.
        if crate::voix::parle(&self.app) {
            self.voix_kobi = Some(Instant::now());
        }
        let kobi_parle = self.voix_kobi.is_some_and(|t| t.elapsed() < TRAINE_VOIX);
        let samples = if kobi_parle {
            vec![0.0; frame.samples.len()]
        } else {
            frame.samples
        };
        let Some(stt) = &mut self.stt else { return };
        for (debut, samples) in piste.porte.trame(frame.t.as_secs_f64(), samples) {
            let morceau = kobi_stt::AudioFrame {
                source: kobi_stt::Source::Eux,
                samples,
                t: debut,
            };
            match stt.push(&morceau) {
                Ok(segments) => {
                    for s in segments.into_iter().filter(|s| s.is_final) {
                        if crate::voix::est_ma_voix(&self.app, &s.text) {
                            continue;
                        }
                        publier(
                            &self.app,
                            Message::Segment(kobi_stt::Segment {
                                source: kobi_stt::Source::Eux,
                                ..s
                            }),
                        );
                    }
                }
                Err(e) => tracing::warn!(erreur = %e, "son de l'ordinateur : transcription"),
            }
        }
    }
}

/// Change le réglage partagé (le fil ouvre ou ferme la piste à son prochain
/// passage) et le journalise s'il change.
fn appliquer<R: Runtime>(app: &AppHandle<R>, nouveau: Reglage, origine: &str) {
    let Some(s) = app.try_state::<SonOrdi>() else {
        return;
    };
    let mut r = s.reglage.lock().unwrap_or_else(|e| e.into_inner());
    if *r != nouveau {
        tracing::info!(actif = nouveau.actif, sortie = ?nouveau.sortie, %origine, "son de l'ordinateur : réglage");
        *r = nouveau;
    }
}

/// Réglage changé par l'interface de contrôle (`kobi config set
/// audio.sources both`) : appliqué tout de suite. `false` si le fil ne
/// tourne pas.
pub fn recharger<R: Runtime>(app: &AppHandle<R>, config: &Config) -> bool {
    if app.try_state::<SonOrdi>().is_none() {
        return false;
    }
    appliquer(app, Reglage::depuis(config), "interface de contrôle");
    true
}

/// Écrit le réglage dans `config.toml`.
pub fn enregistrer(chemin: &Path, actif: bool, sortie: Option<&str>) -> Result<Reglage, String> {
    let mut lu = config::load_or_create(chemin);
    if let Some(probleme) = lu.problem {
        return Err(format!("réglage non enregistré : {probleme}"));
    }
    lu.config.audio.sources = if actif {
        AudioSources::Both
    } else {
        AudioSources::Mic
    };
    lu.config.audio.sortie = Some(sortie.map(str::trim).unwrap_or_default().to_owned());
    config::save(&lu.config, chemin).map_err(|e| e.to_string())?;
    Ok(Reglage::depuis(&lu.config))
}

/// Une sortie proposée : `nom` est enregistré dans le réglage, `libelle`
/// est affiché.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SortieUi {
    pub nom: String,
    pub libelle: String,
}

/// Ce que montre Réglages → Son de l'ordinateur.
#[derive(Debug, Clone, Serialize)]
pub struct EtatSonOrdi {
    #[serde(flatten)]
    pub reglage: Reglage,
    /// La piste « eux » est ouverte en ce moment (réunion en cours).
    pub capte: bool,
    /// Sorties audio du système, capturables.
    pub sorties: Vec<SortieUi>,
    /// Libellé de la sortie par défaut.
    pub defaut: Option<String>,
}

/// Sorties capturables et libellé de celle par défaut. Sous Linux, les
/// sinks PulseAudio ou PipeWire, dont le monitor est capturé (KOB-193) :
/// les périphériques de cpal y sont les PCM ALSA (« Discard all samples »,
/// « Rate Converter Plugin »…), pas des sorties.
#[cfg(target_os = "linux")]
fn sorties() -> (Vec<SortieUi>, Option<String>) {
    let defaut = kobi_audio::pulse::detect()
        .ok()
        .and_then(|i| i.default_sink);
    match kobi_audio::pulse::sorties() {
        Ok(liste) => {
            let defaut = defaut.and_then(|d| {
                liste
                    .iter()
                    .find(|s| s.nom == d)
                    .map(|s| s.description.clone())
            });
            let sorties = liste
                .into_iter()
                .map(|s| SortieUi {
                    nom: s.nom,
                    libelle: s.description,
                })
                .collect();
            (sorties, defaut)
        }
        Err(_) => (Vec::new(), None),
    }
}

/// Sorties capturables en loopback (WASAPI, CoreAudio) et celle par défaut.
#[cfg(not(target_os = "linux"))]
fn sorties() -> (Vec<SortieUi>, Option<String>) {
    match kobi_audio::list_devices() {
        Ok(l) => (
            l.outputs
                .iter()
                .map(|d| SortieUi {
                    nom: d.name.clone(),
                    libelle: d.name.clone(),
                })
                .collect(),
            l.outputs
                .iter()
                .find(|d| d.is_default)
                .map(|d| d.name.clone()),
        ),
        Err(_) => (Vec::new(), None),
    }
}

fn etat_ui<R: Runtime>(app: &AppHandle<R>) -> EtatSonOrdi {
    let (sorties, defaut) = sorties();
    EtatSonOrdi {
        reglage: reglage(app).unwrap_or(Reglage {
            actif: false,
            sortie: None,
        }),
        capte: app
            .try_state::<SonOrdi>()
            .is_some_and(|s| s.capte.load(Ordering::Relaxed)),
        sorties,
        defaut,
    }
}

#[tauri::command]
pub fn son_ordi<R: Runtime>(app: AppHandle<R>) -> EtatSonOrdi {
    etat_ui(&app)
}

/// Active ou désactive le son de l'ordinateur ; vaut tout de suite, même en
/// pleine réunion.
#[tauri::command]
pub fn regler_son_ordi<R: Runtime>(
    app: AppHandle<R>,
    actif: bool,
    sortie: Option<String>,
) -> Result<EtatSonOrdi, String> {
    let chemin = config::default_path().ok_or("dossier personnel introuvable")?;
    let reglage = enregistrer(&chemin, actif, sortie.as_deref())?;
    appliquer(&app, reglage, "réglages de l'app");
    Ok(etat_ui(&app))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reglage_ecrit_et_relu() {
        let dir = tempfile::tempdir().unwrap();
        let chemin = dir.path().join("config.toml");
        // Profil neuf : désactivé.
        assert!(!Reglage::depuis(&config::load_or_create(&chemin).config).actif);
        let r = enregistrer(&chemin, true, Some(" Casque ")).unwrap();
        assert_eq!(
            r,
            Reglage {
                actif: true,
                sortie: Some("Casque".into())
            }
        );
        let texte = std::fs::read_to_string(&chemin).unwrap();
        assert!(texte.contains("sources = \"both\""), "{texte}");
        let r = enregistrer(&chemin, false, None).unwrap();
        assert_eq!(
            r,
            Reglage {
                actif: false,
                sortie: None
            }
        );
        assert_eq!(Reglage::depuis(&config::load_or_create(&chemin).config), r);
    }
}
