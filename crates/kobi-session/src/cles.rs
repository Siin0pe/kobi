//! Réglages par clé pointée (`voice.assistant`, `notes.interval_min`), pour
//! l'interface de contrôle (KOB-161) : lire, modifier avec vérification,
//! dire si le changement s'applique tout de suite ou au redémarrage.
//!
//! La vérification passe par [`Config::parse`] : une valeur que Kobi
//! refuserait au chargement est refusée ici, avec la même règle.

use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use crate::config::{Config, ConfigError};

/// Quand un changement prend effet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Application {
    /// Tout de suite.
    #[serde(rename = "hot")]
    AChaud,
    /// À la prochaine session de Claude ou au redémarrage de Kobi.
    #[serde(rename = "restart")]
    AuRedemarrage,
}

use Application::{AChaud, AuRedemarrage};

/// Réglages connus : clé, application, description.
pub const CLES: &[(&str, Application, &str)] = &[
    (
        "brain",
        AuRedemarrage,
        "Cerveau : auto, claude-code ou api.",
    ),
    ("wake_word", AuRedemarrage, "Mot qui réveille Kobi."),
    (
        "hotkey",
        AChaud,
        "Raccourci qui interpelle Kobi (Ctrl+Shift+Space).",
    ),
    (
        "hotkey_record",
        AChaud,
        "Raccourci qui démarre ou arrête l'enregistrement (Ctrl+Shift+R).",
    ),
    (
        "hotkey_write",
        AChaud,
        "Raccourci qui ouvre la bulle pour écrire à Kobi (Ctrl+Alt+K).",
    ),
    (
        "hotkey_view",
        AChaud,
        "Raccourci qui ouvre ou range la conversation en grand (Ctrl+Alt+G).",
    ),
    (
        "codebases",
        AuRedemarrage,
        "Dossiers de code que Kobi peut lire (chemins complets).",
    ),
    (
        "audio.sources",
        AChaud,
        "Son de l'ordinateur en réunion (la voix des autres en visio) : mic (micro seul, défaut) ou both.",
    ),
    (
        "audio.sortie",
        AChaud,
        "Sortie audio captée pour le son de l'ordinateur ; vide : celle du système.",
    ),
    (
        "stt.engine",
        AuRedemarrage,
        "Moteur de transcription (auto : le gagnant du benchmark).",
    ),
    (
        "notes.mode",
        AuRedemarrage,
        "Prise de notes : blocks, end ou continuous.",
    ),
    (
        "notes.interval_min",
        AuRedemarrage,
        "Intervalle des notes en blocs, de 1 à 10 minutes.",
    ),
    (
        "taches.abandon_s",
        AuRedemarrage,
        "Une tâche de Claude sans signe de vie (ni texte, ni outil en cours, ni carte en attente) est abandonnée après ce délai, de 30 à 3600 secondes (300 par défaut).",
    ),
    (
        "taches.duree_max_s",
        AuRedemarrage,
        "Durée maximale d'une tâche de Claude, même active, de 60 à 7200 secondes (900 par défaut).",
    ),
    // Appliqués à chaud par l'app quand la voix tourne (KOB-142).
    (
        "voice.assistant",
        AChaud,
        "Réponses à voix haute hors réunion.",
    ),
    ("voice.meeting", AChaud, "Réponses à voix haute en réunion."),
    (
        "voice.verbosite",
        AChaud,
        "Combien Kobi parle : bavard, normal (défaut) ou discret.",
    ),
    (
        "ui.language",
        AChaud,
        "Langue de l'interface : \"fr\" ou \"en\" (null : celle du système).",
    ),
    (
        "meetings.retention_days",
        AuRedemarrage,
        "Conservation des réunions : 30, 90, 365 ou \"unlimited\".",
    ),
    (
        "meetings.titre_auto",
        AChaud,
        "Titre de la réunion donné par Kobi d'après ce qui est dit au début.",
    ),
    (
        "conversations.retention_days",
        AuRedemarrage,
        "Conservation des conversations : 30, 90, 365 ou \"unlimited\".",
    ),
    (
        "stockage.dossier",
        AChaud,
        "Dossier des réunions, conversations et rappels (null : ~/Kobi).",
    ),
    (
        "stockage.publication.reunions.mcp",
        AChaud,
        "Serveur MCP où publier les comptes rendus.",
    ),
    (
        "stockage.publication.reunions.destination",
        AChaud,
        "Où publier les comptes rendus chez ce serveur.",
    ),
    (
        "stockage.publication.conversations.mcp",
        AChaud,
        "Serveur MCP où publier les conversations.",
    ),
    (
        "stockage.publication.conversations.destination",
        AChaud,
        "Où publier les conversations chez ce serveur.",
    ),
    (
        "permissions.fichier",
        AChaud,
        "Écriture de fichiers : demander (défaut) ou sans_demander.",
    ),
    (
        "permissions.commande",
        AChaud,
        "Commandes qui modifient : demander (défaut) ou sans_demander.",
    ),
    // Appliqués à chaud par toutes les fenêtres (KOB-200).
    (
        "animations.fps",
        AChaud,
        "Images par seconde des animations : 5, 15, 30, 60 (défaut), 120 ou \"ecran\" (fréquence de l'écran).",
    ),
    (
        "animations.economie",
        AChaud,
        "Économie d'énergie : au repos prolongé, la mascotte ralentit à 3 puis 2 images/s (désactivée par défaut).",
    ),
    (
        "permissions.reglage",
        AChaud,
        "Changements de réglages de Kobi : demander (défaut) ou sans_demander.",
    ),
];

/// Sections réglées par d'autres tickets (MCP connectés KOB-155,
/// permissions KOB-156) : leurs clés existantes se modifient aussi.
const SECTIONS_OUVERTES: &[&str] = &["mcp", "permissions"];

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Entree {
    pub key: &'static str,
    pub value: Value,
    pub apply: Application,
    pub description: &'static str,
}

/// Changement vérifié, pas encore écrit.
#[derive(Debug, Clone, PartialEq)]
pub struct Modification {
    pub config: Config,
    pub cle: String,
    pub avant: Value,
    pub apres: Value,
    pub application: Application,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ErreurCle {
    #[error("réglage inconnu : {0} (voir « kobi config list »)")]
    Inconnue(String),
    #[error("{cle} : {raison}")]
    Refusee { cle: String, raison: String },
}

fn table(config: &Config) -> toml::Table {
    toml::Table::try_from(config).unwrap_or_default()
}

fn chercher<'a>(table: &'a toml::Table, cle: &str) -> Option<&'a toml::Value> {
    let mut morceaux = cle.split('.');
    let mut v = table.get(morceaux.next()?)?;
    for m in morceaux {
        v = v.as_table()?.get(m)?;
    }
    Some(v)
}

fn json(v: &toml::Value) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

/// Application d'une clé, ou `None` si elle n'est pas modifiable.
fn application(config: &Config, cle: &str) -> Option<Application> {
    if let Some((_, a, _)) = CLES.iter().find(|(c, _, _)| *c == cle) {
        return Some(*a);
    }
    let section = cle.split('.').next()?;
    (SECTIONS_OUVERTES.contains(&section) && chercher(&table(config), cle).is_some())
        .then_some(AuRedemarrage)
}

/// Valeur d'une clé, ou tous les réglages sans clé.
pub fn lire(config: &Config, cle: Option<&str>) -> Result<Value, ErreurCle> {
    let t = table(config);
    let Some(cle) = cle else {
        return Ok(json(&toml::Value::Table(t)));
    };
    match chercher(&t, cle) {
        Some(v) => Ok(json(v)),
        // Absentes du fichier quand elles valent leur défaut.
        None if cle == "codebases" => Ok(Value::Array(Vec::new())),
        None if application(config, cle).is_some() => Ok(Value::Null),
        None => Err(ErreurCle::Inconnue(cle.into())),
    }
}

/// Tous les réglages modifiables, avec leur valeur.
pub fn liste(config: &Config) -> Vec<Entree> {
    CLES.iter()
        .map(|(cle, apply, description)| Entree {
            key: cle,
            value: lire(config, Some(cle)).unwrap_or(Value::Null),
            apply: *apply,
            description,
        })
        .collect()
}

fn raison(e: ConfigError) -> String {
    match e {
        ConfigError::Parse { source, .. } => source.message().trim().to_owned(),
        ConfigError::Invalid { expected, .. } => format!("attendu {expected}"),
        autre => autre.to_string(),
    }
}

/// Modifie `cle` dans une copie de `config`, vérifie le résultat et le
/// renvoie avec l'avant et l'après. `null` remet la valeur par défaut.
pub fn modifier(config: &Config, cle: &str, valeur: Value) -> Result<Modification, ErreurCle> {
    let application = application(config, cle).ok_or_else(|| ErreurCle::Inconnue(cle.into()))?;
    let refus = |raison: String| ErreurCle::Refusee {
        cle: cle.into(),
        raison,
    };
    let mut t = table(config);
    let morceaux: Vec<&str> = cle.split('.').collect();
    let (dernier, parents) = morceaux.split_last().expect("clé non vide");
    let mut courant = &mut t;
    for m in parents {
        courant = courant
            .entry(m.to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .ok_or_else(|| refus(format!("{m} n'est pas une section")))?;
    }
    if valeur.is_null() {
        courant.remove(*dernier);
    } else {
        let v = toml::Value::try_from(&valeur).map_err(|e| refus(e.to_string()))?;
        courant.insert(dernier.to_string(), v);
    }
    let texte = toml::to_string(&t).map_err(|e| refus(e.to_string()))?;
    let nouvelle = Config::parse(&texte, Path::new("config.toml")).map_err(|e| refus(raison(e)))?;
    match cle {
        "codebases" => {
            if let Some(d) = nouvelle
                .codebases
                .iter()
                .find(|d| !d.is_absolute() || !d.is_dir())
            {
                return Err(refus(format!("dossier introuvable : {}", d.display())));
            }
        }
        "stockage.dossier" => {
            if let Some(d) = nouvelle
                .stockage
                .dossier
                .as_ref()
                .filter(|d| !d.as_os_str().is_empty())
            {
                if !d.is_absolute() {
                    return Err(refus(format!(
                        "« {} » : il faut un chemin complet",
                        d.display()
                    )));
                }
            }
        }
        _ => {}
    }
    Ok(Modification {
        avant: lire(config, Some(cle))?,
        apres: lire(&nouvelle, Some(cle))?,
        config: nouvelle,
        cle: cle.into(),
        application,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{NotesMode, Retention};
    use serde_json::json;

    #[test]
    fn lire_une_cle_ou_tout() {
        let c = Config::default();
        assert_eq!(lire(&c, Some("voice.assistant")).unwrap(), json!(true));
        assert_eq!(lire(&c, Some("notes.interval_min")).unwrap(), json!(3));
        assert_eq!(lire(&c, Some("hotkey")).unwrap(), json!("Ctrl+Shift+Space"));
        assert_eq!(lire(&c, Some("codebases")).unwrap(), json!([]));
        assert_eq!(lire(&c, Some("stockage.dossier")).unwrap(), Value::Null);
        assert_eq!(
            lire(&c, Some("meetings.retention_days")).unwrap(),
            json!("unlimited")
        );
        let tout = lire(&c, None).unwrap();
        assert_eq!(tout["voice"]["meeting"], json!(false));
        assert!(matches!(
            lire(&c, Some("voix.forte")),
            Err(ErreurCle::Inconnue(_))
        ));
        let l = liste(&c);
        assert_eq!(l.len(), CLES.len());
        assert!(l
            .iter()
            .any(|e| e.key == "voice.assistant" && e.value == json!(true)));
    }

    #[test]
    fn modifier_avec_verification() {
        let c = Config::default();
        let m = modifier(&c, "voice.assistant", json!(false)).unwrap();
        assert_eq!(
            (m.avant.clone(), m.apres.clone()),
            (json!(true), json!(false))
        );
        assert!(!m.config.voice.assistant);
        assert_eq!(m.application, AChaud);

        // KOB-215 : combien Kobi parle, à chaud ; une valeur inconnue est refusée.
        // KOB-207 : langue de l'interface, une langue connue ou null.
        let m = modifier(&c, "ui.language", json!("en")).unwrap();
        assert_eq!(m.config.ui.language, Some(crate::config::Langue::En));
        assert!(modifier(&c, "ui.language", json!("de")).is_err());
        let m = modifier(&m.config, "ui.language", Value::Null).unwrap();
        assert_eq!(m.config.ui.language, None);
        let m = modifier(&c, "voice.verbosite", json!("discret")).unwrap();
        assert_eq!(m.config.voice.verbosite, crate::config::Verbosite::Discret);
        assert_eq!(m.application, AChaud);
        assert!(modifier(&c, "voice.verbosite", json!("bavarde")).is_err());

        let m = modifier(&c, "notes.mode", json!("end")).unwrap();
        assert_eq!(m.config.notes.mode, NotesMode::End);
        let m = modifier(&c, "meetings.retention_days", json!(90)).unwrap();
        assert_eq!(m.config.meetings.retention_days, Retention::Days(90));

        for (cle, v, attendu) in [
            ("notes.interval_min", json!(42), "de 1 à 10"),
            ("voice.assistant", json!("oui"), "bool"),
            ("brain", json!("gpt"), "gpt"),
            ("meetings.retention_days", json!(7), "30, 90, 365"),
            ("wake_word", json!("  "), "non vide"),
            ("codebases", json!(["Z:/nulle/part"]), "introuvable"),
            ("stockage.dossier", json!("relatif"), "chemin complet"),
        ] {
            match modifier(&c, cle, v.clone()) {
                Err(ErreurCle::Refusee { raison, .. }) => {
                    assert!(raison.contains(attendu), "{cle} = {v} → {raison}")
                }
                autre => panic!("{cle} = {v} → {autre:?}"),
            }
        }
        assert!(matches!(
            modifier(&c, "skin", json!("autre")),
            Err(ErreurCle::Inconnue(_))
        ));
    }

    /// Images par seconde (KOB-200) : 60 par défaut, une des valeurs
    /// proposées ou « ecran », appliquées à chaud.
    #[test]
    fn images_par_seconde() {
        let c = Config::default();
        assert_eq!(lire(&c, Some("animations.fps")).unwrap(), json!(60));
        assert_eq!(lire(&c, Some("animations.economie")).unwrap(), json!(false));
        for v in [json!(5), json!(15), json!(30), json!(120), json!("ecran")] {
            let m = modifier(&c, "animations.fps", v.clone()).unwrap();
            assert_eq!(m.apres, v);
            assert_eq!(m.application, AChaud);
        }
        assert_eq!(
            modifier(&c, "animations.fps", json!("ecran"))
                .unwrap()
                .config
                .animations
                .fps,
            crate::config::Fps::Ecran
        );
        for v in [json!(45), json!(0), json!(-5), json!("vite")] {
            assert!(
                matches!(
                    modifier(&c, "animations.fps", v.clone()),
                    Err(ErreurCle::Refusee { .. })
                ),
                "{v}"
            );
        }
        let m = modifier(&c, "animations.economie", json!(true)).unwrap();
        assert!(m.config.animations.economie);
    }

    #[test]
    fn dossiers_et_valeur_par_defaut() {
        let dir = std::env::temp_dir();
        let c = Config::default();
        let m = modifier(&c, "codebases", json!([dir.display().to_string()])).unwrap();
        assert_eq!(m.config.codebases, std::slice::from_ref(&dir));
        assert_eq!(m.avant, json!([]));
        let m = modifier(&c, "stockage.dossier", json!(dir.display().to_string())).unwrap();
        assert_eq!(m.application, AChaud);
        assert_eq!(m.config.stockage.dossier.as_deref(), Some(dir.as_path()));
        // null : retour au défaut.
        let m2 = modifier(&m.config, "stockage.dossier", Value::Null).unwrap();
        assert_eq!(m2.config.stockage.dossier, None);
        assert_eq!(m2.apres, Value::Null);
    }

    #[test]
    fn cles_inconnues_conservees() {
        let c = Config::parse(
            "ancien = 1\n[mcp.serveurs.linear]\necriture = \"demander\"\n",
            Path::new("c"),
        )
        .unwrap();
        // Section d'un autre ticket : ses clés existantes se modifient.
        let m = modifier(&c, "mcp.serveurs.linear.ecriture", json!("sans_demander")).unwrap();
        assert_eq!(m.apres, json!("sans_demander"));
        assert!(matches!(
            modifier(&c, "mcp.serveurs.notion.ecriture", json!("x")),
            Err(ErreurCle::Inconnue(_))
        ));
        let m = modifier(&c, "voice.meeting", json!(true)).unwrap();
        assert_eq!(m.config.extra["ancien"].as_integer(), Some(1));
    }
}
