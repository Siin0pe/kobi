//! Traduction côté Rust (KOB-207, KOB-210) : les mêmes fichiers que
//! l'interface (`ui/src/i18n/*.json`), le même format de clés.
//! `t(Langue::En, "cle", &[("n", "3")])` ; le français en repli, la clé
//! elle-même si elle n'existe nulle part.
//!
//! La langue courante du processus ([`courante`]) suit `ui.language` (ou
//! celle du système) : l'app la règle au démarrage et à chaque changement,
//! la CLI la lit dans `config.toml`. [`tr`] traduit dans cette langue.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Langue de l'interface (KOB-207).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Langue {
    Fr,
    En,
}

impl Langue {
    /// Les langues proposées, dans l'ordre du choix.
    pub const TOUTES: [Langue; 2] = [Langue::Fr, Langue::En];

    /// Code de la langue (`fr`, `en`), celui des fichiers de traduction.
    pub fn code(self) -> &'static str {
        match self {
            Langue::Fr => "fr",
            Langue::En => "en",
        }
    }

    pub fn depuis_code(code: &str) -> Option<Langue> {
        Self::TOUTES.into_iter().find(|l| l.code() == code)
    }

    /// Langue d'une étiquette de langue (`en-US`, `fr_FR.UTF-8`), si proposée.
    pub fn depuis_etiquette(etiquette: &str) -> Option<Langue> {
        let code = etiquette
            .split(['-', '_', '.', '@'])
            .next()?
            .to_ascii_lowercase();
        Self::depuis_code(&code)
    }
}

/// Langue quand ni la config ni le système n'en proposent une connue
/// (`REPLI` de l'interface).
pub const REPLI: Langue = Langue::Fr;

/// Langue du système : la première proposée parmi ses préférences, sinon
/// le français, comme `langueDuSysteme()` de l'interface.
pub fn systeme() -> Langue {
    preferences_du_systeme()
        .iter()
        .find_map(|e| Langue::depuis_etiquette(e))
        .unwrap_or(REPLI)
}

#[cfg(windows)]
fn preferences_du_systeme() -> Vec<String> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let international =
        RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Control Panel\International");
    let mut v = Vec::new();
    if let Ok(cle) =
        RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Control Panel\International\User Profile")
    {
        if let Ok(langues) = cle.get_value::<Vec<String>, _>("Languages") {
            v.extend(langues);
        }
    }
    if let Ok(cle) = international {
        if let Ok(nom) = cle.get_value::<String, _>("LocaleName") {
            v.push(nom);
        }
    }
    v
}

#[cfg(not(windows))]
fn preferences_du_systeme() -> Vec<String> {
    // Ordre de gettext : LANGUAGE (liste), puis LC_ALL, LC_MESSAGES, LANG.
    let mut v: Vec<String> = std::env::var("LANGUAGE")
        .map(|l| l.split(':').map(str::to_owned).collect())
        .unwrap_or_default();
    for nom in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(e) = std::env::var(nom) {
            if !e.is_empty() && e != "C" && e != "POSIX" {
                v.push(e);
            }
        }
    }
    v
}

/// `[ui] language` d'un `config.toml`, sans autre exigence sur le fichier.
pub fn reglee_dans(config: &Path) -> Option<Langue> {
    let texte = std::fs::read_to_string(config).ok()?;
    let table: toml::Table = texte.parse().ok()?;
    Langue::depuis_code(table.get("ui")?.get("language")?.as_str()?)
}

/// Langue du processus ; 0 : jamais réglée, le français ([`REPLI`]) — les
/// tests ne dépendent pas de la langue de la machine. L'app et la CLI la
/// règlent au démarrage ([`regler`]).
static COURANTE: AtomicU8 = AtomicU8::new(0);

fn numero(l: Langue) -> u8 {
    match l {
        Langue::Fr => 1,
        Langue::En => 2,
    }
}

/// Règle la langue du processus : celle choisie, sinon celle du système.
/// Rend la langue retenue.
pub fn regler(choisie: Option<Langue>) -> Langue {
    let l = choisie.unwrap_or_else(systeme);
    COURANTE.store(numero(l), Ordering::Relaxed);
    l
}

/// Langue courante du processus.
pub fn courante() -> Langue {
    match COURANTE.load(Ordering::Relaxed) {
        1 => Langue::Fr,
        2 => Langue::En,
        _ => REPLI,
    }
}

/// [`t`] dans la langue courante.
pub fn tr(cle: &str, vars: &[(&str, &str)]) -> String {
    t(courante(), cle, vars)
}

type Catalogue = BTreeMap<String, Value>;

fn source(l: Langue) -> &'static str {
    match l {
        Langue::Fr => include_str!("../../../ui/src/i18n/fr.json"),
        Langue::En => include_str!("../../../ui/src/i18n/en.json"),
    }
}

fn catalogue(l: Langue) -> &'static Catalogue {
    static FR: OnceLock<Catalogue> = OnceLock::new();
    static EN: OnceLock<Catalogue> = OnceLock::new();
    let cellule = match l {
        Langue::Fr => &FR,
        Langue::En => &EN,
    };
    cellule.get_or_init(|| serde_json::from_str(source(l)).expect("fichier de traduction valide"))
}

/// Catégorie de pluriel (`Intl.PluralRules`) pour les langues proposées.
fn pluriel(l: Langue, n: f64) -> &'static str {
    let un = match l {
        Langue::Fr => (0.0..2.0).contains(&n),
        Langue::En => n == 1.0,
    };
    if un {
        "one"
    } else {
        "other"
    }
}

/// Le texte de `cle` dans `langue`, variables `{nom}` remplacées.
pub fn t(langue: Langue, cle: &str, vars: &[(&str, &str)]) -> String {
    let Some((l, brut)) = [langue, Langue::Fr]
        .into_iter()
        .find_map(|l| catalogue(l).get(cle).map(|v| (l, v)))
    else {
        return cle.to_owned();
    };
    let gabarit = match brut {
        Value::String(s) => s.as_str(),
        Value::Object(formes) => {
            let n = vars
                .iter()
                .find(|(nom, _)| *nom == "n")
                .and_then(|(_, v)| v.parse().ok())
                .unwrap_or(0.0);
            formes
                .get(pluriel(l, n))
                .or_else(|| formes.get("other"))
                .and_then(Value::as_str)
                .unwrap_or_default()
        }
        _ => "",
    };
    let mut texte = gabarit.to_owned();
    for (nom, valeur) in vars {
        texte = texte.replace(&format!("{{{nom}}}"), valeur);
    }
    texte
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Les variables `{nom}` d'un texte (ou de toutes ses formes de pluriel).
    fn variables(v: &Value) -> std::collections::BTreeSet<String> {
        let textes: Vec<&str> = match v {
            Value::String(s) => vec![s],
            Value::Object(f) => f.values().filter_map(Value::as_str).collect(),
            _ => vec![],
        };
        textes
            .iter()
            .flat_map(|t| {
                t.split('{')
                    .skip(1)
                    .filter_map(|r| r.split_once('}').map(|(n, _)| n.to_owned()))
            })
            .collect()
    }

    /// Une clé ou une variable qui manque dans une langue fait échouer.
    #[test]
    fn memes_cles_et_variables_dans_chaque_langue() {
        let fr = catalogue(Langue::Fr);
        for l in Langue::TOUTES {
            let autre = catalogue(l);
            for (cle, v) in fr {
                let w = autre
                    .get(cle)
                    .unwrap_or_else(|| panic!("{} : clé « {cle} » absente", l.code()));
                assert_eq!(
                    variables(v),
                    variables(w),
                    "{} : variables de « {cle} »",
                    l.code()
                );
            }
            for cle in autre.keys() {
                assert!(
                    fr.contains_key(cle),
                    "{} : clé « {cle} » absente du français",
                    l.code()
                );
            }
        }
    }

    #[test]
    fn traduction_pluriels_et_repli() {
        assert_eq!(t(Langue::En, "langue.titre", &[]), "Language");
        assert_eq!(t(Langue::Fr, "commun.minutes", &[("n", "1")]), "1 minute");
        assert_eq!(t(Langue::Fr, "commun.minutes", &[("n", "3")]), "3 minutes");
        assert_eq!(t(Langue::En, "commun.minutes", &[("n", "1")]), "1 minute");
        assert_eq!(
            t(Langue::En, "langue.choisie", &[("nom", "English")]),
            "Language: English"
        );
        assert_eq!(t(Langue::En, "cle.inconnue", &[]), "cle.inconnue");
    }

    /// Clés `tr("…")` / `t(…, "…")` des sources Rust de `dossier`, récursivement.
    fn cles_utilisees(dossier: &Path, cles: &mut Vec<(String, String)>) {
        for e in std::fs::read_dir(dossier).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                cles_utilisees(&p, cles);
            } else if p.extension().is_some_and(|x| x == "rs") {
                let texte = std::fs::read_to_string(&p).unwrap();
                for motif in [
                    "tr(\"",
                    "i18n::t(Langue::Fr, \"",
                    "i18n::t(Langue::En, \"",
                    "i18n::t(l, \"",
                    "i18n::t(langue, \"",
                ] {
                    for (debut, _) in texte.match_indices(motif) {
                        let reste = &texte[debut + motif.len()..];
                        let Some(fin) = reste.find('"') else { continue };
                        let cle = &reste[..fin];
                        // `tr(` d'un autre sens (`str(`, `attr(`…) : pas une clé à points.
                        let avant = texte[..debut].chars().last();
                        if cle.contains('.')
                            && !cle.contains(' ')
                            && !avant.is_some_and(|c| c.is_alphanumeric() || c == '_')
                        {
                            cles.push((p.display().to_string(), cle.to_owned()));
                        }
                    }
                }
            }
        }
    }

    /// Chaque clé employée par le Rust existe dans chaque langue (KOB-210).
    #[test]
    fn cles_du_rust_dans_les_catalogues() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let mut cles = Vec::new();
        cles_utilisees(crates, &mut cles);
        let manquantes: Vec<String> = cles
            .iter()
            .flat_map(|(f, c)| {
                Langue::TOUTES
                    .into_iter()
                    .filter(|l| !catalogue(*l).contains_key(c))
                    .map(move |l| format!("{f} : « {c} » absente en {}", l.code()))
            })
            .collect();
        assert!(manquantes.is_empty(), "{}", manquantes.join("\n"));
    }

    #[test]
    fn langue_d_une_etiquette_et_de_la_config() {
        assert_eq!(Langue::depuis_etiquette("en-US"), Some(Langue::En));
        assert_eq!(Langue::depuis_etiquette("fr_FR.UTF-8"), Some(Langue::Fr));
        assert_eq!(Langue::depuis_etiquette("de-DE"), None);
        let dir = std::env::temp_dir().join(format!("kobi-i18n-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("config.toml");
        std::fs::write(&config, "hotkey = \"Alt+K\"\n[ui]\nlanguage = \"en\"\n").unwrap();
        assert_eq!(reglee_dans(&config), Some(Langue::En));
        std::fs::write(&config, "[ui\n").unwrap();
        assert_eq!(
            reglee_dans(&config),
            None,
            "illisible : la langue du système"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn langue_courante_reglee() {
        assert_eq!(regler(Some(Langue::En)), Langue::En);
        assert_eq!(courante(), Langue::En);
        assert_eq!(tr("langue.titre", &[]), "Language");
        regler(Some(Langue::Fr));
        assert_eq!(tr("langue.titre", &[]), "Langue");
    }
}
