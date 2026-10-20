//! Traduction côté Rust (KOB-207) : les mêmes fichiers que l'interface
//! (`ui/src/i18n/*.json`), le même format de clés. `t(Langue::En, "cle",
//! &[("n", "3")])` ; le français en repli, la clé elle-même si elle n'existe
//! nulle part.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use kobi_session::config::Langue;
use serde_json::Value;

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
#[cfg_attr(
    not(test),
    allow(dead_code, reason = "socle pour les sous-tickets suivants de KOB-206")
)]
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
}
