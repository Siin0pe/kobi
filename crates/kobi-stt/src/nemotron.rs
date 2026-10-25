//! Adaptateur Nemotron 3.5 ASR Streaming (NVIDIA) via sherpa-onnx.
//!
//! Utilise les paquets ONNX publiés par sherpa-onnx
//! (`sherpa-onnx-nemotron-3.5-asr-streaming-0.6b-<bloc>[-int8]-<date>`), un
//! par taille de bloc : le bloc est fixé à l'export, pas à l'exécution. Un flux
//! sherpa-onnx par [`Source`] ; l'énoncé devient final quand les règles de fin
//! de phrase de sherpa-onnx se déclenchent.
//!
//! CPU : paquet int8 et bibliothèques statiques (feature `nemotron`). GPU :
//! paquet fp32 (les opérateurs int8 n'ont pas de noyau CUDA dans onnxruntime)
//! et bibliothèques sherpa-onnx CUDA partagées (feature `nemotron-shared`,
//! `SHERPA_ONNX_LIB_DIR`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use sherpa_onnx::{OnlineRecognizer, OnlineRecognizerConfig, OnlineStream};

use crate::{AudioFrame, Result, Segment, Source, SttEngine, SttError, SAMPLE_RATE};

/// Silence ajouté en fin de flux pour vider le contexte droit de l'encodeur.
const TAIL_PADDING_SECS: f64 = 0.3;

/// Parole entendue au plus avant que le premier mot d'un énoncé sorte du
/// modèle, en plus d'un bloc de l'encodeur (KOB-311) : large, pour ne
/// jamais placer le début après la parole.
const PREMIER_MOT_SECS: f64 = 2.0;

/// Réglages du moteur.
#[derive(Debug, Clone)]
pub struct NemotronConfig {
    /// Dossier du paquet sherpa-onnx (encoder, decoder, joiner, tokens.txt).
    pub model_dir: PathBuf,
    /// Langue forcée (`fr`, `en`, `fr-FR`…) ; `None` = détection automatique.
    pub language: Option<String>,
    pub num_threads: i32,
    /// Silence qui clôt un énoncé non vide, en secondes.
    pub endpoint_silence: f32,
    pub provider: Provider,
    /// Pénalité du symbole vide du transducteur : au-dessus de 0, moins de
    /// mots sautés (les noms propres inconnus du modèle), au prix de
    /// quelques insertions.
    pub blank_penalty: f32,
}

/// Fournisseur d'exécution onnxruntime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Cpu,
    /// Exige des bibliothèques sherpa-onnx compilées avec CUDA ; sinon
    /// sherpa-onnx retombe sur le CPU.
    Cuda,
}

impl Provider {
    fn as_str(self) -> &'static str {
        match self {
            Provider::Cpu => "cpu",
            Provider::Cuda => "cuda",
        }
    }
}

impl std::str::FromStr for Provider {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, String> {
        match s {
            "cpu" => Ok(Provider::Cpu),
            "cuda" => Ok(Provider::Cuda),
            other => Err(format!("fournisseur inconnu : {other} (cpu, cuda)")),
        }
    }
}

impl NemotronConfig {
    pub fn new(model_dir: impl Into<PathBuf>) -> Self {
        Self {
            model_dir: model_dir.into(),
            language: None,
            num_threads: 4,
            endpoint_silence: 0.8,
            provider: Provider::Cpu,
            blank_penalty: 0.0,
        }
    }

    /// Taille de bloc lue dans le nom du dossier (`…-560ms-…`), si présente.
    pub fn chunk_ms(&self) -> Option<u32> {
        let name = self.model_dir.file_name()?.to_str()?;
        name.split('-')
            .find_map(|part| part.strip_suffix("ms")?.parse().ok())
    }
}

/// Fichier du paquet : int8 d'abord sur CPU, fp32 d'abord sur GPU.
fn model_file(dir: &Path, stem: &str, provider: Provider) -> Result<String> {
    let mut names = [format!("{stem}.int8.onnx"), format!("{stem}.onnx")];
    if provider == Provider::Cuda {
        names.reverse();
    }
    names
        .iter()
        .map(|name| dir.join(name))
        .find(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
        .ok_or_else(|| SttError::Engine(format!("{stem}.onnx introuvable dans {}", dir.display())))
}

struct Track {
    stream: OnlineStream,
    /// Fin de l'audio reçu, en secondes.
    pos: f64,
    /// Début de l'énoncé en cours : position du dernier `reset`, avancée à
    /// la reprise après un silence non transmis et au premier mot reconnu
    /// (KOB-311).
    utterance_start: f64,
    /// Dernier texte émis pour l'énoncé en cours.
    last_text: String,
}

/// Moteur Nemotron, voir le module.
pub struct NemotronEngine {
    name: String,
    config: NemotronConfig,
    recognizer: OnlineRecognizer,
    tracks: HashMap<Source, Track>,
    /// [`PREMIER_MOT_SECS`] plus un bloc de l'encodeur, en secondes.
    premier_mot: f64,
}

impl NemotronEngine {
    pub fn new(config: NemotronConfig) -> Result<Self> {
        let dir = &config.model_dir;
        let tokens = dir.join("tokens.txt");
        if !tokens.is_file() {
            return Err(SttError::Engine(format!(
                "tokens.txt introuvable dans {}",
                dir.display()
            )));
        }

        let mut rc = OnlineRecognizerConfig::default();
        rc.model_config.transducer.encoder = Some(model_file(dir, "encoder", config.provider)?);
        rc.model_config.transducer.decoder = Some(model_file(dir, "decoder", config.provider)?);
        rc.model_config.transducer.joiner = Some(model_file(dir, "joiner", config.provider)?);
        rc.model_config.tokens = Some(tokens.to_string_lossy().into_owned());
        rc.model_config.num_threads = config.num_threads;
        rc.model_config.provider = Some(config.provider.as_str().into());
        rc.decoding_method = Some("greedy_search".into());
        rc.blank_penalty = config.blank_penalty;
        rc.enable_endpoint = true;
        // Règle 1 : silence long sans aucun mot ; règle 2 : silence après un
        // énoncé non vide ; règle 3 : énoncé trop long.
        rc.rule1_min_trailing_silence = 2.4;
        rc.rule2_min_trailing_silence = config.endpoint_silence;
        rc.rule3_min_utterance_length = 30.0;

        let recognizer = OnlineRecognizer::create(&rc).ok_or_else(|| {
            SttError::Engine(format!(
                "chargement du modèle impossible : {}",
                dir.display()
            ))
        })?;

        let mut name = "nemotron".to_string();
        if let Some(ms) = config.chunk_ms() {
            name += &format!("-{ms}ms");
        }
        if !rc
            .model_config
            .transducer
            .encoder
            .iter()
            .any(|e| e.contains(".int8."))
        {
            name += "-fp32";
        }
        if config.provider == Provider::Cuda {
            name += "-cuda";
        }
        let bloc = f64::from(config.chunk_ms().unwrap_or(1120)) / 1000.0;
        Ok(Self {
            name,
            config,
            recognizer,
            tracks: HashMap::new(),
            premier_mot: PREMIER_MOT_SECS + bloc,
        })
    }

    /// Oublie l'audio et les énoncés en cours : la prochaine trame repart
    /// d'un flux neuf, sans recharger le modèle (une question après l'autre).
    pub fn reset(&mut self) {
        self.tracks.clear();
    }

    fn track(&mut self, source: Source) -> &mut Track {
        let (recognizer, language) = (&self.recognizer, &self.config.language);
        self.tracks.entry(source).or_insert_with(|| {
            let stream = recognizer.create_stream();
            stream.set_option("language", language.as_deref().unwrap_or("auto"));
            Track {
                stream,
                pos: 0.0,
                utterance_start: 0.0,
                last_text: String::new(),
            }
        })
    }

    /// Décode tout ce qui est prêt et renvoie les segments produits.
    fn drain(&mut self, source: Source) -> Vec<Segment> {
        let recognizer = &self.recognizer;
        let premier_mot = self.premier_mot;
        let track = self.tracks.get_mut(&source).expect("flux créé");
        while recognizer.is_ready(&track.stream) {
            recognizer.decode(&track.stream);
        }

        let text = recognizer
            .get_result(&track.stream)
            .map(|r| clean_text(&r.text))
            .unwrap_or_default();
        if !text.is_empty() && track.last_text.is_empty() {
            track.utterance_start =
                debut_au_premier_mot(track.utterance_start, track.pos, premier_mot);
        }

        let mut out = Vec::new();
        let segment = |text: &str, is_final| Segment {
            start: track.utterance_start,
            end: track.pos,
            source,
            text: text.to_string(),
            is_final,
        };
        if recognizer.is_endpoint(&track.stream) {
            if !text.is_empty() {
                out.push(segment(&text, true));
            }
            recognizer.reset(&track.stream);
            track.utterance_start = track.pos;
            track.last_text.clear();
        } else if text != track.last_text {
            if !text.is_empty() {
                out.push(segment(&text, false));
            }
            track.last_text = text;
        }
        out
    }
}

/// Début de l'énoncé quand arrive une trame qui commence à `t`, l'audio
/// reçu finissant à `pos` : après un silence non transmis (`t > pos`), un
/// énoncé encore `sans_mot` commence au plus tôt à `t` (KOB-311). Le début
/// posé au `reset` précède sinon tout le silence.
fn debut_apres_un_trou(debut: f64, pos: f64, sans_mot: bool, t: f64) -> f64 {
    if sans_mot && t > pos {
        debut.max(t)
    } else {
        debut
    }
}

/// Début de l'énoncé quand son premier mot sort du modèle, la fin de
/// l'audio reçu étant `pos` : au plus `premier_mot` secondes avant (KOB-311).
/// Le silence transmis avant la parole n'en fait plus partie.
fn debut_au_premier_mot(debut: f64, pos: f64, premier_mot: f64) -> f64 {
    debut.max(pos - premier_mot)
}

/// Retire les balises de langue (`<fr-FR>`) et normalise les espaces.
pub fn clean_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        match rest[open..].find('>') {
            Some(close) => rest = &rest[open + close + 1..],
            None => {
                rest = &rest[open..];
                break;
            }
        }
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl SttEngine for NemotronEngine {
    fn name(&self) -> &str {
        &self.name
    }

    fn push(&mut self, frame: &AudioFrame) -> Result<Vec<Segment>> {
        let track = self.track(frame.source);
        track.utterance_start = debut_apres_un_trou(
            track.utterance_start,
            track.pos,
            track.last_text.is_empty(),
            frame.t,
        );
        track
            .stream
            .accept_waveform(SAMPLE_RATE as i32, &frame.samples);
        track.pos = track.pos.max(frame.end());
        Ok(self.drain(frame.source))
    }

    /// KOB-190 : après un silence non transmis, une phrase qui arrive dans
    /// le flux de la précédente (contexte de l'encodeur, découpage en blocs
    /// décalé) peut perdre son premier mot (« Kobi, enregistre la réunion »
    /// → « Enregistre la réunion »). Si la phrase précédente est conclue,
    /// elle repart d'un flux neuf, comme la toute première ; la position est
    /// gardée (horodatage des segments).
    fn nouvel_enonce(&mut self, source: Source) {
        let Some(track) = self.tracks.get_mut(&source) else {
            return;
        };
        if !track.last_text.is_empty() {
            return;
        }
        track.stream = self.recognizer.create_stream();
        track.stream.set_option(
            "language",
            self.config.language.as_deref().unwrap_or("auto"),
        );
        track.utterance_start = track.pos;
    }

    /// KOB-253 : l'écoute a pris le dernier partiel pour final ; le flux
    /// repart à neuf pour que le moteur ne le redonne pas avec la parole
    /// suivante.
    fn conclure(&mut self, source: Source) {
        let Some(track) = self.tracks.get_mut(&source) else {
            return;
        };
        track.stream = self.recognizer.create_stream();
        track.stream.set_option(
            "language",
            self.config.language.as_deref().unwrap_or("auto"),
        );
        track.utterance_start = track.pos;
        track.last_text.clear();
    }

    /// KOB-197 : un mot d'une syllabe dit seul (« oui », « non ») ne sort
    /// jamais du modèle, même sur un flux neuf, avec ou sans langue imposée ;
    /// redoublé (« oui… oui »), il sort (« Oui, oui »). Flux à part, mot
    /// répété deux fois (trois si rien ne sort), silence autour ; le flux de
    /// l'écoute n'est pas touché.
    fn transcrire_extrait(&mut self, samples: &[f32]) -> Option<String> {
        let silence = |s: f64| vec![0.0f32; (s * SAMPLE_RATE as f64) as usize];
        let stream = self.recognizer.create_stream();
        stream.set_option(
            "language",
            self.config.language.as_deref().unwrap_or("auto"),
        );
        stream.accept_waveform(SAMPLE_RATE as i32, &silence(0.3));
        stream.accept_waveform(SAMPLE_RATE as i32, samples);
        stream.accept_waveform(SAMPLE_RATE as i32, &silence(1.0));
        stream.input_finished();
        while self.recognizer.is_ready(&stream) {
            self.recognizer.decode(&stream);
        }
        let text = self
            .recognizer
            .get_result(&stream)
            .map(|r| clean_text(&r.text))
            .unwrap_or_default();
        (!text.is_empty()).then_some(text)
    }

    fn transcrire_court(&mut self, samples: &[f32]) -> Option<String> {
        let silence = |s: f64| vec![0.0f32; (s * SAMPLE_RATE as f64) as usize];
        // Redoublé d'abord ; triplé si rien ne sort (certaines voix).
        for fois in [2, 3] {
            let stream = self.recognizer.create_stream();
            stream.set_option(
                "language",
                self.config.language.as_deref().unwrap_or("auto"),
            );
            stream.accept_waveform(SAMPLE_RATE as i32, &silence(0.3));
            for _ in 0..fois {
                stream.accept_waveform(SAMPLE_RATE as i32, samples);
                stream.accept_waveform(SAMPLE_RATE as i32, &silence(0.2));
            }
            stream.accept_waveform(SAMPLE_RATE as i32, &silence(1.5));
            stream.input_finished();
            while self.recognizer.is_ready(&stream) {
                self.recognizer.decode(&stream);
            }
            let text = self
                .recognizer
                .get_result(&stream)
                .map(|r| clean_text(&r.text))
                .unwrap_or_default();
            if !text.is_empty() {
                return Some(text);
            }
        }
        None
    }

    fn finish(&mut self) -> Result<Vec<Segment>> {
        let mut sources: Vec<Source> = self.tracks.keys().copied().collect();
        sources.sort_by_key(|s| *s as u8);

        let padding = vec![0.0f32; (TAIL_PADDING_SECS * SAMPLE_RATE as f64) as usize];
        let mut out = Vec::new();
        for source in sources {
            let track = self.tracks.get_mut(&source).expect("flux créé");
            track.stream.accept_waveform(SAMPLE_RATE as i32, &padding);
            track.stream.input_finished();
            let mut segments = self.drain(source);
            let track = self.tracks.get_mut(&source).expect("flux créé");
            // Ce qui reste après la dernière fin de phrase devient final.
            if !track.last_text.is_empty() {
                segments.retain(|s| s.is_final);
                segments.push(Segment {
                    start: track.utterance_start,
                    end: track.pos,
                    source,
                    text: std::mem::take(&mut track.last_text),
                    is_final: true,
                });
            }
            out.extend(segments.into_iter().filter(|s| s.is_final));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_tags_are_removed() {
        assert_eq!(clean_text("Bonjour Kobi. <fr-FR>"), "Bonjour Kobi.");
        assert_eq!(clean_text("  a <en-US>  b "), "a b");
        assert_eq!(clean_text("x < y"), "x < y");
    }

    /// KOB-311 : le début d'un énoncé ne précède plus le silence d'avant
    /// la parole, transmis ou non.
    #[test]
    fn debut_d_enonce_apres_le_silence() {
        // Silence non transmis : la trame suivante arrive à 40 s, le dernier
        // énoncé s'est conclu à 12 s.
        assert_eq!(debut_apres_un_trou(12.0, 12.0, true, 40.0), 40.0);
        // Pas de trou : rien ne bouge.
        assert_eq!(debut_apres_un_trou(12.0, 12.0, true, 12.0), 12.0);
        // Énoncé déjà commencé (un mot reconnu) : un trou ne le coupe pas.
        assert_eq!(debut_apres_un_trou(12.0, 20.0, false, 40.0), 12.0);
        // Silence transmis : au premier mot, à 40 s, au plus 2,56 s avant.
        assert!((debut_au_premier_mot(12.0, 40.0, 2.56) - 37.44).abs() < 1e-9);
        // Parole sans silence avant : le début reste celui du reset.
        assert_eq!(debut_au_premier_mot(39.0, 40.0, 2.56), 39.0);
    }

    #[test]
    fn chunk_size_is_read_from_the_package_name() {
        let c = NemotronConfig::new(
            "m/sherpa-onnx-nemotron-3.5-asr-streaming-0.6b-160ms-int8-2026-06-11",
        );
        assert_eq!(c.chunk_ms(), Some(160));
        assert_eq!(NemotronConfig::new("m/autre").chunk_ms(), None);
    }

    /// KOB-127 : avec whisper.cpp lié dans le même binaire, la création du
    /// moteur plantait sous Linux (« free(): invalid pointer » dans le
    /// `std::regex` d'onnxruntime). Ignoré sans modèle :
    /// `KOBI_NEMOTRON_DIR=<dossier du paquet> cargo test -p kobi-stt
    /// --features whisper,nemotron -- --ignored`.
    #[test]
    #[cfg(feature = "whisper")]
    #[ignore = "demande KOBI_NEMOTRON_DIR"]
    fn loads_with_whisper_in_the_same_binary() {
        // Force l'édition de liens de whisper.cpp sans l'appeler.
        std::hint::black_box(whisper_rs::print_system_info as fn() -> &'static str);
        let dir = PathBuf::from(std::env::var("KOBI_NEMOTRON_DIR").expect("KOBI_NEMOTRON_DIR"));
        NemotronEngine::new(NemotronConfig::new(&dir)).unwrap();
    }

    /// Transcrit le WAV français fourni avec le paquet. Ignoré sans modèle :
    /// `KOBI_NEMOTRON_DIR=<dossier du paquet> cargo test -p kobi-stt
    /// --features nemotron -- --ignored`.
    #[test]
    #[ignore = "demande KOBI_NEMOTRON_DIR"]
    fn transcribes_the_packaged_french_sample() {
        let dir = PathBuf::from(std::env::var("KOBI_NEMOTRON_DIR").expect("KOBI_NEMOTRON_DIR"));
        let samples = crate::wav::read_wav_16k_mono(dir.join("test_wavs/fr.wav")).unwrap();
        let mut config = NemotronConfig::new(&dir);
        config.language = Some("fr".into());
        let mut engine = NemotronEngine::new(config).unwrap();
        let out = crate::replay::WavReplayer::new(vec![crate::replay::Track {
            source: Source::Moi,
            samples,
        }])
        .speed(0.0)
        .run(&mut engine)
        .unwrap();
        let text: Vec<_> = out.finals().map(|s| s.text.as_str()).collect();
        println!("{text:?}");
        assert!(!text.is_empty());
        assert!(out.emissions.iter().any(|e| !e.segment.is_final));
    }

    /// KOB-311, avec le modèle : une phrase dite après 8 s de silence,
    /// transmis puis non transmis, commence au plus `premier_mot` avant la
    /// parole, et pas après. Ignoré sans modèle :
    /// `KOBI_NEMOTRON_DIR=<dossier du paquet> cargo test -p kobi-stt
    /// --features nemotron -- --ignored`.
    #[test]
    #[ignore = "demande KOBI_NEMOTRON_DIR"]
    fn debut_d_enonce_apres_le_silence_avec_le_modele() {
        let dir = PathBuf::from(std::env::var("KOBI_NEMOTRON_DIR").expect("KOBI_NEMOTRON_DIR"));
        let parole = crate::wav::read_wav_16k_mono(dir.join("test_wavs/fr.wav")).unwrap();
        let duree = parole.len() as f64 / SAMPLE_RATE as f64;
        for transmis in [true, false] {
            let mut config = NemotronConfig::new(&dir);
            config.language = Some("fr".into());
            let mut engine = NemotronEngine::new(config).unwrap();
            let premier_mot = engine.premier_mot;
            let mut finals = Vec::new();
            let mut pousser = |engine: &mut NemotronEngine, samples: &[f32], debut: f64| {
                for (i, bloc) in samples.chunks(1600).enumerate() {
                    let frame = AudioFrame {
                        source: Source::Moi,
                        samples: bloc.to_vec(),
                        t: debut + i as f64 * 0.1,
                    };
                    finals.extend(
                        engine
                            .push(&frame)
                            .unwrap()
                            .into_iter()
                            .filter(|s| s.is_final),
                    );
                }
            };
            // La porte de l'écoute laisse passer un peu de silence après la
            // parole : la phrase se conclut. Puis 8 s de silence, transmis ou
            // non ; sans, l'écoute demande un énoncé neuf (KOB-190).
            pousser(&mut engine, &parole, 0.0);
            let fin_de_phrase = vec![0.0f32; 3 * SAMPLE_RATE as usize];
            pousser(&mut engine, &fin_de_phrase, duree);
            let silence = vec![0.0f32; 8 * SAMPLE_RATE as usize];
            if transmis {
                pousser(&mut engine, &silence, duree + 3.0);
            } else {
                engine.nouvel_enonce(Source::Moi);
            }
            let reprise = duree + 11.0;
            pousser(&mut engine, &parole, reprise);
            finals.extend(engine.finish().unwrap());
            let seconde = finals
                .iter()
                .find(|s| s.end > reprise)
                .unwrap_or_else(|| panic!("pas de phrase après le silence : {finals:?}"));
            println!("transmis={transmis} reprise={reprise:.2} {seconde:?}");
            assert!(
                seconde.start >= reprise - premier_mot && seconde.start <= reprise + 0.5,
                "transmis={transmis} : début {} pour une parole à {reprise}",
                seconde.start
            );
        }
    }
}
