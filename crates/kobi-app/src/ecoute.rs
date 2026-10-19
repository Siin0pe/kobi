//! Écoute continue du micro (KOB-132) : Nemotron transcrit en local, en
//! permanence tant que Kobi n'est pas en pause, et [`Ecouteur`] repère
//! « Kobi » dans la transcription. Rien ne sort de la machine : le fil
//! publie segments et questions au chef d'orchestre ([`crate::chef`]), qui
//! seul écrit la réunion (quand on l'a demandé) et interroge Claude. Le fil ne
//! s'arrête jamais pour attendre une réponse.
//!
//! Le fil garde le micro ouvert et le modèle chargé. Le raccourci clavier
//! reste un déclencheur de secours : il lui envoie [`Commande::Raccourci`].
//!
//! `KOBI_DIAG=1` journalise le niveau du micro et les segments à part
//! ([`crate::diag`]) ; `KOBI_ECOUTE_WAV=<fichier.wav>` remplace le micro par
//! ce fichier, joué en temps réel puis suivi de silence (essais sans voix).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use kobi_audio::{AudioFrame, Capture, CaptureOptions};
use kobi_stt::nemotron::NemotronEngine;
use kobi_stt::SttEngine;
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::chef::{publier, Message};
use crate::diag::Diag;
use crate::interpellation::{charger_stt, erreur};
use crate::mot_cle::{Action, Ecouteur};
use crate::porte::Porte;
use crate::session::etat;

/// Après un échec d'ouverture du micro, nouvel essai après ce délai.
const REESSAI_MICRO: Duration = Duration::from_secs(5);
/// Période du bilan de charge dans le journal.
const BILAN: Duration = Duration::from_secs(300);
/// Événement du niveau du micro (0 à 1), pendant l'écoute.
pub const EVENEMENT_NIVEAU: &str = "niveau";
/// Période du niveau envoyé à l'interface (20 par seconde).
const PERIODE_NIVEAU: f64 = 0.05;

/// Ce que l'application demande au fil d'écoute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Commande {
    /// Raccourci clavier : la phrase suivante est une question.
    Raccourci,
    /// Met l'écoute en pause, ou la reprend.
    BasculerPause,
    /// Met l'écoute en pause (`true`) ou la reprend (`false`), interface de
    /// contrôle (KOB-161).
    Pause(bool),
    /// Kobi vient de répondre : la conversation attend la suite.
    Suite,
    /// La réunion vient d'être arrêtée à la voix ou par Claude : « reprends »
    /// s'entend sans redire « Kobi » pendant l'annulation (KOB-165).
    Annulable,
}

/// Canal vers le fil d'écoute.
pub struct Ecoute(Mutex<Sender<Commande>>);

/// L'écoute est en pause (micro coupé), tenu à jour par le fil d'écoute.
#[derive(Default)]
pub struct EnPause(pub AtomicBool);

/// Lance le fil d'écoute.
pub fn installer<R: Runtime>(app: &AppHandle<R>) {
    let (tx, rx) = mpsc::channel();
    app.manage(Ecoute(Mutex::new(tx)));
    app.manage(EnPause::default());
    let app = app.clone();
    std::thread::Builder::new()
        .name("ecoute".into())
        .spawn(move || Fil::new(app).boucle(rx))
        .expect("fil d'écoute");
}

/// Envoie une commande au fil d'écoute.
pub fn commander<R: Runtime>(app: &AppHandle<R>, commande: Commande) -> Result<(), String> {
    app.state::<Ecoute>()
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .send(commande)
        .map_err(|e| e.to_string())
}

/// D'où vient l'audio.
enum Entree {
    Micro(Capture),
    /// Fichier injecté (`KOBI_ECOUTE_WAV`), joué par un fil à part.
    Fichier(Receiver<AudioFrame>),
}

impl Entree {
    fn frames(&self) -> &Receiver<AudioFrame> {
        match self {
            Entree::Micro(capture) => capture.frames(),
            Entree::Fichier(rx) => rx,
        }
    }

    /// Ouvre le fichier injecté, ou le micro par défaut ; renvoie aussi sa
    /// description pour le journal.
    fn ouvrir() -> Result<(Entree, String), String> {
        if let Some(wav) = std::env::var_os("KOBI_ECOUTE_WAV") {
            let wav = PathBuf::from(wav);
            let samples = kobi_stt::wav::read_wav_16k_mono(&wav)
                .map_err(|e| format!("{} : {e}", wav.display()))?;
            let description = format!(
                "fichier {} ({:.1} s, 16 kHz mono)",
                wav.display(),
                samples.len() as f64 / kobi_stt::SAMPLE_RATE as f64
            );
            return Ok((Entree::Fichier(jouer(samples)), description));
        }
        let capture = Capture::start(&CaptureOptions {
            sources: vec![kobi_audio::Source::Moi],
            // Micro choisi au premier lancement (KOB-144), sinon celui du système.
            mic: crate::bienvenue::micro_choisi(),
            output: None,
        })
        .map_err(|e| e.to_string())?;
        let description = capture
            .sources()
            .iter()
            .map(|s| {
                format!(
                    "micro « {} » ({} Hz, {} canaux, {:?}), rééchantillonné en 16 kHz mono",
                    s.device, s.sample_rate, s.channels, s.sample_format
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        Ok((Entree::Micro(capture), description))
    }
}

/// Joue des échantillons 16 kHz en temps réel, par trames de 100 ms, puis du
/// silence tant que le récepteur existe.
fn jouer(samples: Vec<f32>) -> Receiver<AudioFrame> {
    const TRAME: usize = 1_600;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let silence = vec![0.0; TRAME];
        let mut morceaux = samples.chunks(TRAME);
        let debut = Instant::now();
        for i in 0u64.. {
            let morceau = morceaux.next().unwrap_or(&silence);
            let t = Duration::from_millis(100 * i);
            if let Some(attente) = t.checked_sub(debut.elapsed()) {
                std::thread::sleep(attente);
            }
            let frame = AudioFrame {
                source: kobi_audio::Source::Moi,
                samples: morceau.to_vec(),
                t,
            };
            if tx.send(frame).is_err() {
                return;
            }
        }
    });
    rx
}

/// Transcrit une trame et en déduit les actions de l'écoute. La porte ne
/// laisse passer vers Nemotron que la parole ; l'écoute voit passer le temps
/// à chaque trame. `t` est la fin de la trame en temps du micro (celui du
/// moteur repart de zéro à chaque question).
/// `entendu` reçoit chaque segment (partiel ou final) et dit s'il faut
/// l'ignorer : c'est la voix de Kobi qui revient dans le micro (KOB-142).
fn transcrire(
    stt: &mut impl SttEngine,
    porte: &mut Porte,
    ecouteur: &mut Ecouteur,
    diag: &mut Option<Diag>,
    frame: kobi_stt::AudioFrame,
    t: f64,
    entendu: &dyn Fn(&str) -> bool,
) -> (Vec<Action>, Vec<kobi_stt::Segment>) {
    if let Some(d) = diag {
        d.trame(frame.t, &frame.samples);
    }
    let mut actions = ecouteur.tick(t);
    let mut finals = Vec::new();
    let etait_ouverte = porte.ouverte();
    let sorties = porte.trame(frame.t, frame.samples);
    // Une parole commence après un silence non transmis : flux neuf si la
    // phrase précédente est conclue (KOB-190).
    if porte.ouverte() && !etait_ouverte {
        stt.nouvel_enonce(kobi_stt::Source::Moi);
    }
    if let (Some(d), true) = (diag.as_mut(), porte.ouverte() != etait_ouverte) {
        d.note(if porte.ouverte() {
            "porte ouverte : parole"
        } else {
            "porte fermée : silence"
        });
    }
    for (debut, samples) in sorties {
        let frame = kobi_stt::AudioFrame {
            source: kobi_stt::Source::Moi,
            samples,
            t: debut,
        };
        match stt.push(&frame) {
            Ok(segments) => {
                for s in segments {
                    if entendu(&s.text) {
                        if let Some(d) = diag {
                            d.note(&format!("écho de Kobi ignoré : « {} »", s.text));
                        }
                        continue;
                    }
                    let suite = ecouteur.segment(&s.text, s.is_final, t);
                    if let Some(d) = diag {
                        d.segment(&s.text, s.is_final, &suite);
                    }
                    actions.extend(suite);
                    if s.is_final {
                        finals.push(s);
                    }
                }
            }
            Err(e) => tracing::warn!(erreur = %e, "écoute continue : transcription"),
        }
    }
    (actions, finals)
}

struct Fil<R: Runtime> {
    app: AppHandle<R>,
    stt: Result<NemotronEngine, String>,
    entree: Option<Entree>,
    diag: Option<Diag>,
    porte: Porte,
    ecouteur: Ecouteur,
    en_pause: bool,
    /// Fin de l'audio reçu, en secondes depuis l'ouverture du micro.
    t: f64,
    /// Charge : audio transcrit et temps passé à le transcrire.
    audio: f64,
    calcul: Duration,
    bilan: Instant,
    /// Une réunion est ouverte : pas de mode conversation, et la transcription
    /// n'est jamais remise à zéro.
    en_reunion: bool,
    /// Niveau du micro en cours d'accumulation : (somme des carrés, échantillons).
    niveau: (f64, usize),
}

impl<R: Runtime> Fil<R> {
    fn new(app: AppHandle<R>) -> Self {
        Self {
            app,
            stt: charger_stt(),
            entree: None,
            diag: Diag::ouvrir(),
            porte: Porte::new(kobi_stt::SAMPLE_RATE),
            ecouteur: Ecouteur::default(),
            en_pause: false,
            t: 0.0,
            audio: 0.0,
            calcul: Duration::ZERO,
            bilan: Instant::now(),
            en_reunion: false,
            niveau: (0.0, 0),
        }
    }

    fn note(&mut self, texte: &str) {
        if let Some(d) = &mut self.diag {
            d.note(texte);
        }
    }

    fn boucle(mut self, rx: Receiver<Commande>) {
        if let Err(e) = &self.stt {
            tracing::warn!(erreur = %e, "écoute continue impossible : Nemotron absent");
            let e = e.clone();
            self.note(&format!("Nemotron absent : {e}"));
        }
        loop {
            while let Ok(commande) = rx.try_recv() {
                self.commande(commande);
            }
            let session = etat(&self.app).etat;
            let en_reunion = matches!(session, "reunion" | "pause" | "fin");
            if en_reunion != self.en_reunion {
                self.en_reunion = en_reunion;
                self.ecouteur.conversation = !en_reunion;
                if en_reunion {
                    self.ecouteur.oublier();
                }
            }
            // En pause (Kobi ou la réunion), ni micro ni transcription.
            if self.en_pause || session == "pause" || self.stt.is_err() {
                self.fermer_micro();
                match rx.recv_timeout(Duration::from_millis(500)) {
                    Ok(commande) => self.commande(commande),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
                continue;
            }
            if self.entree.is_none() && !self.ouvrir_micro() {
                match rx.recv_timeout(REESSAI_MICRO) {
                    Ok(commande) => self.commande(commande),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
                continue;
            }
            self.trame();
        }
    }

    fn ouvrir_micro(&mut self) -> bool {
        match Entree::ouvrir() {
            Ok((entree, description)) => {
                if let Ok(stt) = self.stt.as_mut() {
                    stt.reset();
                }
                self.ecouteur = Ecouteur::new(!self.en_reunion);
                self.porte = Porte::new(kobi_stt::SAMPLE_RATE);
                self.t = 0.0;
                self.entree = Some(entree);
                tracing::info!(entree = %description, "écoute continue : micro ouvert");
                if let Some(d) = &mut self.diag {
                    d.entree(&description);
                }
                true
            }
            Err(e) => {
                tracing::warn!(erreur = %e, "écoute continue : micro indisponible");
                self.note(&format!("micro indisponible : {e}"));
                false
            }
        }
    }

    fn fermer_micro(&mut self) {
        if self.entree.take().is_some() {
            tracing::info!("écoute continue : micro fermé");
            self.note("micro fermé");
        }
    }

    fn commande(&mut self, commande: Commande) {
        self.executer_commande(commande);
        if let Some(p) = self.app.try_state::<EnPause>() {
            p.0.store(self.en_pause, Ordering::Relaxed);
        }
    }

    fn executer_commande(&mut self, commande: Commande) {
        match commande {
            Commande::Suite => self.ecouteur.suite(self.t),
            Commande::Annulable => self
                .ecouteur
                .annulable(self.t, crate::chef::ANNULATION.as_secs_f64()),
            Commande::BasculerPause => {
                self.en_pause = !self.en_pause;
                tracing::info!(pause = self.en_pause, "écoute continue");
            }
            Commande::Pause(pause) => {
                self.en_pause = pause;
                tracing::info!(pause, "écoute continue (contrôle)");
            }
            Commande::Raccourci => {
                self.note("raccourci clavier");
                if self.en_pause {
                    self.en_pause = false;
                    tracing::info!(pause = false, "écoute continue : reprise par le raccourci");
                    crate::menu::ecoute_reprise(&self.app);
                }
                if self.stt.is_err() {
                    self.stt = charger_stt();
                }
                let probleme = match &self.stt {
                    Err(e) => Some(e.clone()),
                    Ok(_) => None,
                }
                .or_else(|| {
                    (self.entree.is_none() && !self.ouvrir_micro())
                        .then(|| "micro indisponible".to_string())
                });
                if let Some(message) = probleme {
                    let app = self.app.clone();
                    std::thread::spawn(move || erreur(&app, "Micro ou transcription", &message));
                    return;
                }
                // Hors réunion, la phrase en cours est oubliée : la suivante est
                // la question. En réunion, rien n'est perdu de la transcription.
                if !self.en_reunion {
                    if let Ok(stt) = self.stt.as_mut() {
                        stt.reset();
                    }
                }
                let actions = self.ecouteur.raccourci(self.t);
                self.executer(actions);
            }
        }
    }

    fn trame(&mut self) {
        let Some(entree) = &self.entree else { return };
        let frame = match entree.frames().recv_timeout(Duration::from_millis(200)) {
            Ok(frame) => frame,
            Err(RecvTimeoutError::Timeout) => return,
            Err(RecvTimeoutError::Disconnected) => {
                tracing::warn!("écoute continue : le micro s'est arrêté");
                self.note("le micro s'est arrêté");
                self.entree = None;
                return;
            }
        };
        self.niveau(&frame.samples);
        let Ok(stt) = self.stt.as_mut() else { return };
        let frame = kobi_stt::AudioFrame {
            source: kobi_stt::Source::Moi,
            samples: frame.samples,
            t: frame.t.as_secs_f64(),
        };
        self.t = frame.end();
        self.audio += frame.samples.len() as f64 / kobi_stt::SAMPLE_RATE as f64;
        let debut = Instant::now();
        let t = self.t;
        // Sa propre voix est ignorée ; une autre voix pendant qu'il parle le
        // fait taire (KOB-142).
        let app = &self.app;
        let annulation = self.ecouteur.en_annulation();
        let entendu = |texte: &str| {
            if crate::voix::est_ma_voix(app, texte)
                && !ordre_d_annulation(texte, annulation, crate::voix::parle(app))
            {
                return true;
            }
            crate::voix::entendu(app, texte);
            false
        };
        let (actions, finals) = transcrire(
            stt,
            &mut self.porte,
            &mut self.ecouteur,
            &mut self.diag,
            frame,
            t,
            &entendu,
        );
        self.calcul += debut.elapsed();
        for s in finals {
            publier(&self.app, Message::Segment(s));
        }
        self.executer(actions);

        if self.bilan.elapsed() >= BILAN {
            tracing::info!(
                audio_s = self.audio as u64,
                calcul_s = self.calcul.as_secs_f64() as u64,
                rtf = format!("{:.3}", self.calcul.as_secs_f64() / self.audio.max(1.0)),
                "écoute continue : charge"
            );
            self.audio = 0.0;
            self.calcul = Duration::ZERO;
            self.bilan = Instant::now();
        }
    }

    /// Niveau du micro, 20 fois par seconde, seulement quand Kobi écoute.
    fn niveau(&mut self, samples: &[f32]) {
        self.niveau.0 += samples
            .iter()
            .map(|&x| f64::from(x) * f64::from(x))
            .sum::<f64>();
        self.niveau.1 += samples.len();
        if (self.niveau.1 as f64) < PERIODE_NIVEAU * kobi_stt::SAMPLE_RATE as f64 {
            return;
        }
        let rms = (self.niveau.0 / self.niveau.1 as f64).sqrt();
        self.niveau = (0.0, 0);
        if etat(&self.app).mascotte == "listen" {
            let _ = self.app.emit(EVENEMENT_NIVEAU, niveau_normalise(rms));
        }
    }

    fn executer(&mut self, actions: Vec<Action>) {
        for action in actions {
            let message = match action {
                Action::Interpeller => {
                    // On appelle Kobi : il se tait pour écouter (KOB-142).
                    crate::voix::taire(&self.app, "interpellation");
                    tracing::info!("Kobi interpellé");
                    Message::Interpelle
                }
                Action::Question(q) => Message::QuestionPartielle(q),
                Action::Envoyer(q) => {
                    tracing::info!(longueur = q.len(), "question transcrite");
                    Message::Question {
                        texte: q,
                        voix: true,
                    }
                }
                Action::EnvoyerPeutEtre(q) => {
                    tracing::info!(
                        longueur = q.len(),
                        "phrase qui cite Kobi, peut-être pour lui"
                    );
                    Message::QuestionPeutEtre(q)
                }
                Action::Abandonner => {
                    tracing::info!("interpellation sans question");
                    Message::SansQuestion
                }
            };
            publier(&self.app, message);
        }
    }
}

/// « Reprends » juste après un arrêt (KOB-165) : Kobi vient de dire « Dis
/// « reprends » si c'était une erreur », et le filtre d'écho prendrait la
/// réponse pour sa propre voix. Une fois qu'il s'est tu, un ordre bref qui
/// annule l'arrêt passe ; l'écho de la phrase entière, plus longue, reste
/// ignoré.
fn ordre_d_annulation(texte: &str, annulation: bool, kobi_parle: bool) -> bool {
    annulation
        && !kobi_parle
        && texte.split_whitespace().count() <= 2
        && crate::intentions::annule_la_fin(texte)
}

/// Niveau RMS ramené entre 0 et 1 : -60 dBFS (silence) à -10 dBFS (voix forte).
pub fn niveau_normalise(rms: f64) -> f32 {
    let db = 20.0 * rms.max(1e-10).log10();
    ((db + 60.0) / 50.0).clamp(0.0, 1.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interpellation::modele_nemotron;
    use std::path::Path;

    /// « Reprends » après un arrêt n'est pas pris pour l'écho de « Dis
    /// « reprends » si c'était une erreur » (essai audio injecté, KOB-165).
    #[test]
    fn reprends_n_est_pas_un_echo() {
        assert!(ordre_d_annulation("Reprends.", true, false));
        assert!(ordre_d_annulation("non, continue", true, false));
        // Kobi parle encore, ou rien à annuler : filtre d'écho ordinaire.
        assert!(!ordre_d_annulation("Reprends.", true, true));
        assert!(!ordre_d_annulation("Reprends.", false, false));
        // L'écho de sa phrase, plus longue, reste un écho.
        assert!(!ordre_d_annulation(
            "Dis reprends si c'était une erreur",
            true,
            false
        ));
        assert!(!ordre_d_annulation("une erreur", true, false));
    }

    #[test]
    fn niveau_entre_zero_et_un() {
        assert_eq!(niveau_normalise(0.0), 0.0);
        assert_eq!(niveau_normalise(1.0), 1.0);
        let parole = niveau_normalise(0.03);
        assert!(parole > 0.4 && parole < 0.6, "{parole}");
    }

    /// Rejoue un WAV dans Nemotron et l'écoute comme le fil le fait avec le
    /// micro (trames de 10 ms, puis 3 s de silence). Renvoie les actions et
    /// les segments finals.
    fn rejouer(
        stt: &mut NemotronEngine,
        diag: &mut Option<Diag>,
        wav: &Path,
    ) -> (Vec<Action>, Vec<String>) {
        let samples = kobi_stt::wav::read_wav_16k_mono(wav).unwrap();
        stt.reset();
        let mut ecouteur = Ecouteur::default();
        let mut porte = Porte::new(kobi_stt::SAMPLE_RATE);
        let (mut actions, mut finals) = (Vec::new(), Vec::new());
        let silence = vec![0.0; 48_000];
        // Trames de 10 ms ; `KOBI_TRAME` (échantillons) pour celles du micro.
        let n = std::env::var("KOBI_TRAME")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(160);
        for (i, morceau) in samples.chunks(n).chain(silence.chunks(n)).enumerate() {
            let frame = kobi_stt::AudioFrame {
                source: kobi_stt::Source::Moi,
                samples: morceau.to_vec(),
                t: (i * n) as f64 / kobi_stt::SAMPLE_RATE as f64,
            };
            let t = frame.end();
            let (suite, _) = transcrire(stt, &mut porte, &mut ecouteur, diag, frame, t, &|_| false);
            if let Some(d) = diag {
                finals.extend(d.finals());
            }
            actions.extend(suite);
        }
        (actions, finals)
    }

    /// KOB-190 : chaque parole qui suit un silence (porte rouverte) repart
    /// d'un énoncé neuf du moteur ; jamais au milieu d'une parole.
    #[test]
    fn nouvel_enonce_a_chaque_parole() {
        #[derive(Default)]
        struct Faux {
            enonces: Vec<f64>,
            pos: f64,
        }
        impl SttEngine for Faux {
            fn name(&self) -> &str {
                "faux"
            }
            fn push(
                &mut self,
                frame: &kobi_stt::AudioFrame,
            ) -> kobi_stt::Result<Vec<kobi_stt::Segment>> {
                self.pos = frame.end();
                Ok(Vec::new())
            }
            fn finish(&mut self) -> kobi_stt::Result<Vec<kobi_stt::Segment>> {
                Ok(Vec::new())
            }
            fn nouvel_enonce(&mut self, _source: kobi_stt::Source) {
                self.enonces.push(self.pos);
            }
        }
        let mut stt = Faux::default();
        let mut porte = Porte::new(kobi_stt::SAMPLE_RATE);
        let mut ecouteur = Ecouteur::default();
        // 1 s de silence, 0,6 s de parole, 4 s de silence, 0,6 s de parole,
        // 3 s de silence ; trames de 100 ms, comme le micro.
        let parole: Vec<f32> = (0..1600).map(|i| 0.2 * (i as f32 * 0.3).sin()).collect();
        let silence = vec![0.0f32; 1600];
        let plan = [(10, false), (6, true), (40, false), (6, true), (30, false)];
        let mut t = 0.0;
        for (n, fort) in plan {
            for _ in 0..n {
                let frame = kobi_stt::AudioFrame {
                    source: kobi_stt::Source::Moi,
                    samples: if fort {
                        parole.clone()
                    } else {
                        silence.clone()
                    },
                    t,
                };
                t += 0.1;
                transcrire(
                    &mut stt,
                    &mut porte,
                    &mut ecouteur,
                    &mut None,
                    frame,
                    t,
                    &|_| false,
                );
            }
        }
        assert_eq!(stt.enonces.len(), 2, "{:?}", stt.enonces);
    }

    /// Un WAV : `KOBI_ECOUTE_WAV=<fichier.wav> cargo test -p kobi-app
    /// --release -- --ignored wav_injecte --nocapture` ;
    /// `KOBI_ECOUTE_ATTENDU=rien` pour un fichier qui ne doit rien déclencher.
    #[test]
    #[ignore = "demande KOBI_ECOUTE_WAV et le modèle Nemotron"]
    fn wav_injecte() {
        assert!(modele_nemotron().is_some(), "modèle Nemotron absent");
        let wav = PathBuf::from(std::env::var_os("KOBI_ECOUTE_WAV").expect("KOBI_ECOUTE_WAV"));
        let mut stt = charger_stt().unwrap();
        let (actions, finals) = rejouer(&mut stt, &mut Diag::ouvrir(), &wav);
        println!("{finals:#?}\n{actions:#?}");
        let envoyees = actions
            .iter()
            .filter(|a| matches!(a, Action::Envoyer(_)))
            .count();
        match std::env::var("KOBI_ECOUTE_ATTENDU").as_deref() {
            Ok("rien") => assert_eq!(envoyees, 0),
            _ => assert!(envoyees > 0, "aucune question envoyée"),
        }
    }

    /// Les transcriptions déjà faites par [`corpus`] (`*.txt`), rejouées
    /// comme phrases finales : mesure d'une règle de repérage sans Nemotron
    /// (KOB-179). `KOBI_ECOUTE_CORPUS=<dossier> cargo test -p kobi-app --
    /// --ignored corpus_texte --nocapture`.
    #[test]
    #[ignore = "demande KOBI_ECOUTE_CORPUS (transcriptions de corpus)"]
    fn corpus_texte() {
        let dir =
            PathBuf::from(std::env::var_os("KOBI_ECOUTE_CORPUS").expect("KOBI_ECOUTE_CORPUS"));
        let mut fichiers: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "txt"))
            .collect();
        fichiers.sort();
        let (mut pos, mut pos_ok, mut fausses) = (0, 0, 0);
        for f in &fichiers {
            let nom = f.file_name().unwrap().to_string_lossy().into_owned();
            let mut ecouteur = Ecouteur::default();
            let actions: Vec<Action> = std::fs::read_to_string(f)
                .unwrap()
                .lines()
                .enumerate()
                .flat_map(|(i, l)| ecouteur.segment(l, true, i as f64 * 5.0))
                .collect();
            let envoyee = actions.iter().any(|a| matches!(a, Action::Envoyer(_)));
            if nom.starts_with("pos") {
                pos += 1;
                pos_ok += usize::from(envoyee);
                if !envoyee {
                    println!("MANQUÉ {nom}");
                }
            } else if actions.contains(&Action::Interpeller) {
                fausses += 1;
                println!("FAUSSE {nom}");
            }
        }
        println!("BILAN texte : positifs {pos_ok}/{pos}, fausses interpellations {fausses}");
    }

    /// Un corpus : `pos*.wav` doivent envoyer une question, `neg*.wav` ne
    /// rien déclencher. `KOBI_ECOUTE_CORPUS=<dossier> cargo test -p kobi-app
    /// --release -- --ignored corpus --nocapture`.
    #[test]
    #[ignore = "demande KOBI_ECOUTE_CORPUS et le modèle Nemotron"]
    fn corpus() {
        let dir =
            PathBuf::from(std::env::var_os("KOBI_ECOUTE_CORPUS").expect("KOBI_ECOUTE_CORPUS"));
        let mut stt = charger_stt().unwrap();
        let mut fichiers: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "wav"))
            .collect();
        fichiers.sort();
        let (mut pos, mut pos_ok, mut fausses) = (0, 0, 0);
        // Temps de calcul et durée d'audio : le coût d'un réglage (KOB-179).
        let (mut calcul, mut audio) = (std::time::Duration::ZERO, 0.0);
        let mut manques = Vec::new();
        for wav in &fichiers {
            let nom = wav.file_name().unwrap().to_string_lossy().into_owned();
            audio += kobi_stt::wav::read_wav_16k_mono(wav).unwrap().len() as f64
                / kobi_stt::SAMPLE_RATE as f64;
            let debut = std::time::Instant::now();
            let (actions, finals) = rejouer(&mut stt, &mut Some(Diag::memoire()), wav);
            calcul += debut.elapsed();
            // Transcription complète, pour juger de sa qualité.
            std::fs::write(wav.with_extension("txt"), finals.join("\n")).unwrap();
            let envoyees: Vec<_> = actions
                .iter()
                .filter_map(|a| match a {
                    Action::Envoyer(q) => Some(q.as_str()),
                    _ => None,
                })
                .collect();
            let interpellations = actions
                .iter()
                .filter(|a| **a == Action::Interpeller)
                .count();
            if nom.starts_with("pos") {
                pos += 1;
                pos_ok += usize::from(!envoyees.is_empty());
                if envoyees.is_empty() {
                    manques.push(format!("{nom}: {finals:?}"));
                }
                println!("{nom}: {finals:?} -> {envoyees:?}");
            } else {
                fausses += interpellations;
                for f in finals
                    .iter()
                    .filter(|f| crate::mot_cle::reperer(f).is_some())
                {
                    println!("{nom}: fausse détection « {f} »");
                }
                println!(
                    "{nom}: {} segments, {interpellations} interpellations",
                    finals.len()
                );
            }
        }
        for m in &manques {
            println!("MANQUÉ {m}");
        }
        println!(
            "BILAN positifs {pos_ok}/{pos}, fausses interpellations {fausses}, \
             calcul {:.1} s pour {audio:.1} s d'audio (×{:.2} temps réel)",
            calcul.as_secs_f64(),
            calcul.as_secs_f64() / audio
        );
    }
}
