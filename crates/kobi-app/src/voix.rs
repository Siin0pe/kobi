//! Réponse à voix haute ou en texte (KOB-142).
//!
//! Le réglage est double : quotidien et réunion (`[voice]` de `config.toml`,
//! `assistant` et `meeting`). Quand il est actif pour le contexte courant, la
//! réponse de Claude est dite phrase par phrase pendant qu'elle s'écrit dans
//! la bulle (voix Piper locale, `kobi-tts`).
//!
//! La voix se tait :
//! - dès que l'utilisateur parle : un segment transcrit qui n'est pas l'écho
//!   de ce que Kobi est en train de dire ([`entendu`]) ;
//! - sur « Kobi », le raccourci, Échap (commande `interrompre` du chef),
//!   « stop » ou « tais-toi » ([`taire`]).
//!
//! Kobi ne s'entend pas lui-même : l'écoute ignore les segments qui sont
//! l'écho de sa réponse, pendant qu'il parle et peu après ([`est_ma_voix`]).
//!
//! « Kobi, parle » et « Kobi, en silence » règlent le contexte courant
//! ([`commande_vocale`]) ; la barre système et le menu du clic droit aussi.
//!
//! Variables d'essai : `KOBI_VOIX=<paquet vits-piper-…>`,
//! `KOBI_VOIX_WAV=<fichier.wav>` (la voix est écrite au lieu d'être jouée),
//! `KOBI_VOIX_SORTIE=<nom de la sortie audio>`.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use kobi_session::config::VoiceConfig;
use kobi_tts::texte::Diction;
use kobi_tts::{Evenement as EvenementVoix, Parleur, Sortie, Synthese};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::session::etat;

/// État de la voix envoyé à l'interface.
pub const EVENEMENT_VOIX: &str = "voix";
/// Après la fin de la voix, l'écho peut encore arriver : retard de la
/// transcription (Nemotron sur un CPU lent) et réverbération de la pièce.
const ECHO_APRES: Duration = Duration::from_secs(6);
/// Une réponse confiée à la voix mais pas encore finie (voix qui se charge,
/// phrases en file) compte comme dite pendant ce temps au plus.
const ECHO_EN_FILE: Duration = Duration::from_secs(30);

/// Réglage et état courant, tels que les voit l'interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct EtatVoix {
    /// Réponses dites hors réunion.
    pub quotidien: bool,
    /// Réponses dites en réunion.
    pub reunion: bool,
    /// Réglage du contexte courant (réunion ou non).
    pub actif: bool,
    /// La voix joue en ce moment.
    pub parle: bool,
}

/// Commande vocale reconnue dans une question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandeVoix {
    /// « Kobi, parle » : réponses à voix haute (contexte courant).
    Parle,
    /// « Kobi, en silence » : réponses en texte seul (contexte courant).
    Silence,
    /// « Kobi, stop » : la voix se tait, rien n'est demandé à Claude.
    Stop,
}

pub struct Voix {
    reglage: Mutex<(bool, bool)>,
    parleur: Option<Parleur>,
    config: Option<PathBuf>,
    /// Ce que Kobi dit (ou vient de dire) : on n'en coupe pas la voix et
    /// l'écoute ne le prend pas pour de la parole.
    recent: Mutex<Recent>,
}

/// Réponse dite en dernier.
#[derive(Default)]
struct Recent {
    texte: String,
    /// Première phrase confiée à la voix.
    debut: Option<Instant>,
    /// Fin de la voix, si elle est finie.
    fin: Option<Instant>,
}

impl Recent {
    /// L'écho de ce texte peut-il arriver maintenant ?
    fn actif(&self, parle: bool) -> bool {
        parle
            || self.fin.is_some_and(|f| f.elapsed() < ECHO_APRES)
            || (self.fin.is_none() && self.debut.is_some_and(|d| d.elapsed() < ECHO_EN_FILE))
    }
}

/// Paquet de voix : `KOBI_VOIX`, sinon le premier `vits-piper-*` de
/// `~/Kobi/models` (la voix française par défaut en tête si elle est là).
pub fn paquet_voix() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("KOBI_VOIX") {
        return Some(PathBuf::from(dir));
    }
    let models = kobi_session::config::kobi_dir()?.join("models");
    let defaut = models.join(kobi_tts::piper::VOIX_PAR_DEFAUT);
    if defaut.is_dir() {
        return Some(defaut);
    }
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(models)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_dir()
                && p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("vits-piper-"))
        })
        .collect();
    dirs.sort();
    dirs.into_iter().next()
}

fn fabrique_voix() -> kobi_tts::parleur::Fabrique<Box<dyn Synthese>> {
    Box::new(|| {
        let dir = paquet_voix().ok_or_else(|| {
            kobi_tts::Erreur::VoixAbsente(format!(
                "placer le paquet {} dans ~/Kobi/models",
                kobi_tts::piper::VOIX_PAR_DEFAUT
            ))
        })?;
        let mut voix = kobi_tts::piper::Piper::new(&kobi_tts::piper::PiperConfig::new(&dir))?;
        tracing::info!(voix = %dir.display(), "voix Piper");
        // Le premier passage du modèle est lent (plusieurs secondes sous
        // charge) : on le fait au chargement plutôt qu'à la première réponse.
        let debut = std::time::Instant::now();
        let _ = voix.synthetiser("Bonjour.");
        tracing::info!(ms = debut.elapsed().as_millis() as u64, "voix préchauffée");
        Ok(Box::new(voix) as Box<dyn Synthese>)
    })
}

fn fabrique_sortie() -> kobi_tts::parleur::Fabrique<Box<dyn Sortie>> {
    Box::new(|| {
        if let Some(wav) = std::env::var_os("KOBI_VOIX_WAV") {
            let wav = PathBuf::from(wav);
            tracing::info!(fichier = %wav.display(), "voix écrite dans un fichier");
            return Ok(Box::new(kobi_tts::sortie::Fichier::new(&wav, true)) as Box<dyn Sortie>);
        }
        let nom = std::env::var("KOBI_VOIX_SORTIE").ok();
        let hp = kobi_tts::sortie::HautParleur::ouvrir(nom.as_deref())?;
        tracing::info!(sortie = %hp.description, "voix : sortie audio");
        Ok(Box::new(hp) as Box<dyn Sortie>)
    })
}

/// Lance le parleur (la voix se charge en arrière-plan) et garde le réglage.
pub fn installer<R: Runtime>(app: &AppHandle<R>, reglage: &VoiceConfig, config: Option<PathBuf>) {
    let h = app.clone();
    let parleur = Parleur::new(fabrique_voix(), fabrique_sortie(), move |e| {
        match &e {
            EvenementVoix::Debut { premier_son } => {
                tracing::info!(
                    premier_son_ms = premier_son.as_millis() as u64,
                    "voix : premier son"
                );
            }
            EvenementVoix::Fin { interrompu } => tracing::info!(interrompu, "voix : fin"),
            EvenementVoix::Erreur(m) => tracing::warn!(erreur = %m, "voix"),
        }
        let parle = matches!(e, EvenementVoix::Debut { .. });
        if let Some(v) = h.try_state::<Voix>() {
            let mut r = v.recent.lock().unwrap_or_else(|e| e.into_inner());
            match e {
                EvenementVoix::Debut { .. } => r.fin = None,
                EvenementVoix::Fin { .. } => r.fin = Some(Instant::now()),
                EvenementVoix::Erreur(_) => {}
            }
        }
        if !matches!(e, EvenementVoix::Erreur(_)) {
            let mut etat = etat_voix(&h);
            etat.parle = parle;
            let _ = h.emit(EVENEMENT_VOIX, etat);
        }
    });
    app.manage(Voix {
        reglage: Mutex::new((reglage.assistant, reglage.meeting)),
        parleur: Some(parleur),
        config,
        recent: Mutex::default(),
    });
}

/// Sans parleur (tests) : seul le réglage existe.
#[cfg(test)]
pub fn installer_sans_parleur<R: Runtime>(app: &AppHandle<R>, reglage: &VoiceConfig) {
    app.manage(Voix {
        reglage: Mutex::new((reglage.assistant, reglage.meeting)),
        parleur: None,
        config: None,
        recent: Mutex::default(),
    });
}

fn en_reunion<R: Runtime>(app: &AppHandle<R>) -> bool {
    matches!(etat(app).etat, "reunion" | "pause")
}

pub fn etat_voix<R: Runtime>(app: &AppHandle<R>) -> EtatVoix {
    let Some(v) = app.try_state::<Voix>() else {
        return EtatVoix {
            quotidien: false,
            reunion: false,
            actif: false,
            parle: false,
        };
    };
    let (quotidien, reunion) = *v.reglage.lock().unwrap_or_else(|e| e.into_inner());
    EtatVoix {
        quotidien,
        reunion,
        actif: if en_reunion(app) { reunion } else { quotidien },
        parle: v.parleur.as_ref().is_some_and(Parleur::parle),
    }
}

/// Règle le quotidien et/ou la réunion, enregistre `config.toml` et prévient
/// l'interface.
pub fn regler<R: Runtime>(
    app: &AppHandle<R>,
    quotidien: Option<bool>,
    reunion: Option<bool>,
) -> EtatVoix {
    let Some(v) = app.try_state::<Voix>() else {
        return etat_voix(app);
    };
    let (q, r) = {
        let mut reglage = v.reglage.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(q) = quotidien {
            reglage.0 = q;
        }
        if let Some(r) = reunion {
            reglage.1 = r;
        }
        *reglage
    };
    if let Some(chemin) = &v.config {
        let mut lu = kobi_session::config::load_or_create(chemin);
        lu.config.voice.assistant = q;
        lu.config.voice.meeting = r;
        if let Err(e) = kobi_session::config::save(&lu.config, chemin) {
            tracing::warn!(erreur = %e, "voix : réglage non enregistré");
        }
    }
    let etat = etat_voix(app);
    if !etat.actif {
        taire(app, "réglage en texte seul");
    }
    tracing::info!(quotidien = q, reunion = r, "voix : réglage");
    let _ = app.emit(EVENEMENT_VOIX, etat);
    etat
}

/// Voix ou texte seul pour le contexte courant (quotidien ou réunion).
pub fn regler_contexte<R: Runtime>(app: &AppHandle<R>, actif: bool) -> EtatVoix {
    if en_reunion(app) {
        regler(app, None, Some(actif))
    } else {
        regler(app, Some(actif), None)
    }
}

/// Bascule le contexte courant (menu).
pub fn basculer<R: Runtime>(app: &AppHandle<R>) -> EtatVoix {
    let actif = etat_voix(app).actif;
    regler_contexte(app, !actif)
}

/// Exécute `f` sur le parleur, s'il existe.
fn avec_parleur<R: Runtime, T>(app: &AppHandle<R>, f: impl FnOnce(&Parleur) -> T) -> Option<T> {
    let v = app.try_state::<Voix>()?;
    v.parleur.as_ref().map(f)
}

/// Coupe la voix tout de suite.
pub fn taire<R: Runtime>(app: &AppHandle<R>, raison: &str) {
    avec_parleur(app, |p| {
        if p.parle() {
            tracing::info!(%raison, "voix interrompue");
        }
        p.interrompre();
    });
}

/// Tout ce que Kobi dit ou vient de dire, s'il parle encore, et si son
/// écho peut arriver maintenant.
fn texte_dit<R: Runtime>(app: &AppHandle<R>) -> Option<(String, bool, bool)> {
    let v = app.try_state::<Voix>()?;
    let parle = v.parleur.as_ref().is_some_and(Parleur::parle);
    let en_cours = v
        .parleur
        .as_ref()
        .map(Parleur::texte_en_cours)
        .unwrap_or_default();
    let r = v.recent.lock().unwrap_or_else(|e| e.into_inner());
    Some((format!("{} {en_cours}", r.texte), parle, r.actif(parle)))
}

/// Ce segment transcrit est-il la voix de Kobi (son écho dans le micro),
/// pendant qu'il parle ou juste après ? L'écoute l'ignore alors : Kobi ne
/// se prend ni pour une question ni pour un participant.
pub fn est_ma_voix<R: Runtime>(app: &AppHandle<R>, texte: &str) -> bool {
    if normaliser(texte).is_empty() {
        return false;
    }
    let Some((dit, _, actif)) = texte_dit(app) else {
        return false;
    };
    actif && est_echo(texte, &dit)
}

/// Segment transcrit pendant que Kobi parle : s'il n'est pas l'écho de sa
/// propre voix, l'utilisateur parle et la voix se tait.
pub fn entendu<R: Runtime>(app: &AppHandle<R>, texte: &str) {
    let Some((dit, parle, _)) = texte_dit(app) else {
        return;
    };
    if parle && !est_echo(texte, &dit) {
        taire(app, "l'utilisateur parle");
    }
}

/// Une réponse en cours : les phrases partent à la voix au fil du flux.
pub struct Reponse {
    enonce: Option<u64>,
    diction: Diction,
}

impl Reponse {
    /// Nouvelle réponse ; muette si le contexte courant est en texte seul.
    pub fn new<R: Runtime>(app: &AppHandle<R>) -> Self {
        let enonce = if etat_voix(app).actif {
            avec_parleur(app, Parleur::nouvel_enonce)
        } else {
            None
        };
        if enonce.is_some() {
            if let Some(v) = app.try_state::<Voix>() {
                *v.recent.lock().unwrap_or_else(|e| e.into_inner()) = Recent::default();
            }
        }
        Self {
            enonce,
            diction: Diction::new(),
        }
    }

    /// Texte accumulé de la bulle : les phrases complètes partent.
    pub fn morceau<R: Runtime>(&mut self, app: &AppHandle<R>, accumule: &str) {
        if self.enonce.is_some() {
            let phrases = self.diction.suite(accumule);
            self.dire(app, &phrases);
        }
    }

    /// Fin du flux : le reste part, puis la fin de l'énoncé.
    pub fn fin<R: Runtime>(&mut self, app: &AppHandle<R>, accumule: &str) {
        if let Some(e) = self.enonce {
            let phrases = self.diction.fin(accumule);
            self.dire(app, &phrases);
            avec_parleur(app, |p| p.finir(e));
        }
    }

    fn dire<R: Runtime>(&self, app: &AppHandle<R>, phrases: &[String]) {
        if let Some(e) = self.enonce {
            if let Some(v) = app.try_state::<Voix>() {
                let mut r = v.recent.lock().unwrap_or_else(|e| e.into_inner());
                if !phrases.is_empty() && r.debut.is_none() {
                    r.debut = Some(Instant::now());
                }
                for phrase in phrases {
                    r.texte.push(' ');
                    r.texte.push_str(phrase);
                }
            }
            avec_parleur(app, |p| phrases.iter().for_each(|phrase| p.dire(e, phrase)));
        }
    }
}

/// Dit une phrase courte (confirmation), si la voix est active.
pub fn dire<R: Runtime>(app: &AppHandle<R>, texte: &str) {
    let mut r = Reponse::new(app);
    r.fin(app, texte);
}

/// La voix joue-t-elle ? (le retour au calme attend la fin).
pub fn parle<R: Runtime>(app: &AppHandle<R>) -> bool {
    etat_voix(app).parle
}

/// Minuscules, sans accents ni ponctuation, mots séparés par une espace.
fn normaliser(texte: &str) -> String {
    let sans_accents: String = texte
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'à' | 'â' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' => 'i',
            'ô' | 'ö' => 'o',
            'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            c if c.is_alphanumeric() => c,
            _ => ' ',
        })
        .collect();
    sans_accents
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// « Kobi, parle », « Kobi, en silence », « stop »… La phrase entière doit
/// être la commande : « parle-moi du refresh token » reste une question.
pub fn commande_vocale(question: &str) -> Option<CommandeVoix> {
    let q = normaliser(question);
    let q = q.strip_prefix("kobi").map(str::trim_start).unwrap_or(&q);
    let q = q
        .strip_suffix("s il te plait")
        .map(str::trim_end)
        .unwrap_or(q);
    const PARLE: &[&str] = &[
        "parle",
        "parle moi",
        "parle a voix haute",
        "a voix haute",
        "reponds a voix haute",
        "lis tes reponses",
        "tu peux parler",
        "active la voix",
        "mode voix",
    ];
    const SILENCE: &[&str] = &[
        "en silence",
        "silence",
        "reste en silence",
        "arrete de parler",
        "reponds par ecrit",
        "par ecrit",
        "ecris seulement",
        "coupe la voix",
        "mode texte",
    ];
    // « Tais-toi » coupe la réponse en cours (« stop »), sans passer en
    // texte seul (document d'UX : « attends », « stop », « tais-toi »).
    const STOP: &[&str] = &[
        "stop",
        "tais toi",
        "arrete",
        "attends",
        "chut",
        "ca suffit",
        "stop stop",
    ];
    if PARLE.contains(&q) {
        Some(CommandeVoix::Parle)
    } else if SILENCE.contains(&q) {
        Some(CommandeVoix::Silence)
    } else if STOP.contains(&q) {
        Some(CommandeVoix::Stop)
    } else {
        None
    }
}

/// Exécute une commande vocale ; renvoie la phrase de confirmation à
/// afficher (et à dire, pour « parle »), ou rien pour « stop ».
pub fn executer<R: Runtime>(app: &AppHandle<R>, commande: CommandeVoix) -> Option<&'static str> {
    let reunion = en_reunion(app);
    match commande {
        CommandeVoix::Parle => {
            regler_contexte(app, true);
            Some(if reunion {
                "D'accord, je réponds à voix haute pendant la réunion."
            } else {
                "D'accord, je réponds à voix haute."
            })
        }
        CommandeVoix::Silence => {
            regler_contexte(app, false);
            Some(if reunion {
                "D'accord, en réunion je réponds par écrit."
            } else {
                "D'accord, je réponds par écrit."
            })
        }
        CommandeVoix::Stop => {
            taire(app, "stop");
            None
        }
    }
}

/// Le segment entendu est-il l'écho de ce que dit Kobi ? Oui si la plupart
/// de ses mots sont dans le texte en cours de lecture.
pub fn est_echo(entendu: &str, dit: &str) -> bool {
    let entendu = normaliser(entendu);
    let mots: Vec<&str> = entendu.split(' ').filter(|m| !m.is_empty()).collect();
    if mots.is_empty() {
        // Rien de lisible : ni parole ni écho, on ne coupe pas.
        return true;
    }
    let dit = normaliser(dit);
    let connus: std::collections::HashSet<&str> = dit.split(' ').collect();
    let communs = mots.iter().filter(|m| connus.contains(*m)).count();
    communs * 10 >= mots.len() * 6
}

#[tauri::command]
pub fn etat_voix_ui<R: Runtime>(app: AppHandle<R>) -> EtatVoix {
    etat_voix(&app)
}

/// Règle la voix (fenêtre des réglages) : `null` laisse un contexte inchangé.
#[tauri::command]
pub fn regler_voix<R: Runtime>(
    app: AppHandle<R>,
    quotidien: Option<bool>,
    reunion: Option<bool>,
) -> EtatVoix {
    regler(&app, quotidien, reunion)
}

/// Voix ou texte seul pour le contexte courant (menu du clic droit).
#[tauri::command]
pub fn basculer_voix<R: Runtime>(app: AppHandle<R>) -> EtatVoix {
    basculer(&app)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{self, appliquer};
    use kobi_session::etat::Evenement;
    use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};
    use tauri::App;

    fn app(reglage: VoiceConfig) -> App<MockRuntime> {
        let app = session::installer(mock_builder())
            .build(mock_context(noop_assets()))
            .unwrap();
        installer_sans_parleur(app.handle(), &reglage);
        app
    }

    #[test]
    fn commandes_vocales() {
        use CommandeVoix::*;
        for (q, c) in [
            ("Kobi, parle.", Some(Parle)),
            ("Kobi, parle-moi", Some(Parle)),
            ("kobi réponds à voix haute s'il te plaît", Some(Parle)),
            ("Kobi, en silence.", Some(Silence)),
            ("Tais-toi", Some(Stop)),
            ("Kobi, réponds par écrit", Some(Silence)),
            ("Stop !", Some(Stop)),
            ("Kobi, arrête.", Some(Stop)),
            ("Ça suffit", Some(Stop)),
            // Des questions, pas des commandes.
            ("Kobi, parle-moi du refresh token", None),
            ("Kobi, arrête l'enregistrement", None),
            ("Où est géré le silence des notifications ?", None),
        ] {
            assert_eq!(commande_vocale(q), c, "{q}");
        }
    }

    #[test]
    fn echo_ou_parole() {
        let dit = "Le refresh token est géré dans auth token rs, ligne 42. Il expire au bout d'une heure.";
        // Nemotron réentend Kobi, sans accents ni ponctuation.
        assert!(est_echo("le refresh token est gere dans", dit));
        assert!(est_echo("il expire au bout d une heure", dit));
        // L'utilisateur coupe la parole.
        assert!(!est_echo("stop", dit));
        assert!(!est_echo("attends c'est pas ça", dit));
        assert!(!est_echo("et le access token alors", dit));
        assert!(est_echo("", dit));
    }

    #[test]
    fn ma_voix_reconnue_meme_en_file() {
        let app = app(VoiceConfig::default());
        let h = app.handle();
        let dit = "D'accord, je réponds à voix haute.";
        // Rien n'a été dit : aucun segment n'est pris pour un écho.
        assert!(!est_ma_voix(h, "d accord je reponds a voix haute"));
        {
            let v = h.state::<Voix>();
            let mut r = v.recent.lock().unwrap();
            r.texte = dit.into();
            r.debut = Some(Instant::now());
        }
        // Confiée à la voix (en file, ou en train d'être dite) : son écho est
        // ignoré, pas une vraie phrase de l'utilisateur.
        assert!(est_ma_voix(h, "d accord je reponds a voix haute"));
        assert!(!est_ma_voix(h, "et quelle heure est-il"));
        assert!(!est_ma_voix(h, ""));
        // Fini depuis longtemps : plus d'écho possible.
        h.state::<Voix>().recent.lock().unwrap().fin =
            Some(Instant::now() - Duration::from_secs(60));
        assert!(!est_ma_voix(h, "d accord je reponds a voix haute"));
    }

    #[test]
    fn reglage_par_contexte() {
        let app = app(VoiceConfig::default());
        let h = app.handle();
        // Par défaut : voix au quotidien, texte en réunion.
        let e = etat_voix(h);
        assert_eq!((e.quotidien, e.reunion, e.actif), (true, false, true));

        assert_eq!(
            executer(h, CommandeVoix::Silence),
            Some("D'accord, je réponds par écrit.")
        );
        assert!(!etat_voix(h).actif);
        assert!(!etat_voix(h).quotidien);

        // En réunion, la commande règle la réunion seulement.
        appliquer(h, &[Evenement::DemarrerReunion]).unwrap();
        assert!(!etat_voix(h).actif);
        executer(h, CommandeVoix::Parle);
        let e = etat_voix(h);
        assert_eq!((e.quotidien, e.reunion, e.actif), (false, true, true));
        // Le menu bascule le contexte courant.
        assert!(!basculer(h).actif);
        assert!(!etat_voix(h).reunion);
        assert_eq!(executer(h, CommandeVoix::Stop), None);
    }

    #[test]
    fn reponse_muette_en_texte_seul() {
        let app = app(VoiceConfig {
            assistant: false,
            ..VoiceConfig::default()
        });
        let h = app.handle();
        let mut r = Reponse::new(h);
        assert!(r.enonce.is_none());
        r.morceau(h, "Bonjour, voici la réponse. ");
        r.fin(h, "Bonjour, voici la réponse. Fin.");
        assert!(!parle(h));
    }

    #[test]
    fn reglage_enregistre_dans_la_config() {
        let dir = tempfile::tempdir().unwrap();
        let chemin = dir.path().join("config.toml");
        std::fs::write(
            &chemin,
            "hotkey = \"Alt+K\"\n[voice]\nassistant = true\nmeeting = false\n",
        )
        .unwrap();
        let app = session::installer(mock_builder())
            .build(mock_context(noop_assets()))
            .unwrap();
        app.manage(Voix {
            reglage: Mutex::new((true, false)),
            parleur: None,
            config: Some(chemin.clone()),
            recent: Mutex::default(),
        });
        regler(app.handle(), Some(false), Some(true));
        let lu = kobi_session::config::load_or_create(&chemin).config;
        assert!(!lu.voice.assistant);
        assert!(lu.voice.meeting);
        assert_eq!(lu.hotkey, "Alt+K", "le reste de la config est gardé");
    }
}
