//! Clé API Anthropic (KOB-104), pour le backend « clé API » de qui n'a pas
//! Claude Code (KOB-101) : gardée dans le gestionnaire d'identifiants du
//! système (Windows : Gestionnaire d'identifiants ; macOS : trousseau ;
//! Linux : Secret Service), jamais dans `config.toml`, jamais dans les
//! journaux. L'interface n'en voit que les quatre derniers caractères.

use serde::Serialize;

use crate::mcp::{Coffre, CoffreSysteme};

/// Entrée de la clé dans le coffre de Kobi (`:` ne peut pas être le nom
/// d'un serveur MCP : pas de collision avec leurs secrets).
const SERVEUR: &str = ":anthropic";
const NOM: &str = "cle-api";
/// Préfixe des clés de l'API Anthropic.
const PREFIXE: &str = "sk-ant-";

/// Ce que l'interface sait de la clé.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EtatCle {
    pub presente: bool,
    /// Quatre derniers caractères, pour reconnaître la clé sans la montrer.
    pub fin: Option<String>,
}

fn etat_de(cle: Option<&str>) -> EtatCle {
    EtatCle {
        presente: cle.is_some(),
        fin: cle.map(|c| {
            let n = c.chars().count();
            c.chars().skip(n.saturating_sub(4)).collect()
        }),
    }
}

/// La clé elle-même, pour le backend clé API (jamais envoyée à
/// l'interface ni écrite dans un journal).
pub fn lire(coffre: &dyn Coffre) -> Result<Option<String>, String> {
    coffre.lire(SERVEUR, NOM)
}

/// État de la clé enregistrée.
pub fn etat(coffre: &dyn Coffre) -> Result<EtatCle, String> {
    Ok(etat_de(coffre.lire(SERVEUR, NOM)?.as_deref()))
}

/// Enregistre (ou remplace) la clé, après une vérification de forme.
pub fn enregistrer(coffre: &dyn Coffre, cle: &str) -> Result<EtatCle, String> {
    let cle = cle.trim();
    let valide = cle.starts_with(PREFIXE)
        && cle.len() > PREFIXE.len() + 8
        && cle
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
    if !valide {
        return Err(kobi_i18n::tr(
            "coque.cleApi.invalide",
            &[("prefixe", PREFIXE)],
        ));
    }
    coffre.ecrire(SERVEUR, NOM, cle)?;
    tracing::info!("clé API enregistrée dans le gestionnaire d'identifiants");
    Ok(etat_de(Some(cle)))
}

/// Supprime la clé (sans erreur si elle n'y était pas).
pub fn supprimer(coffre: &dyn Coffre) -> Result<EtatCle, String> {
    coffre.effacer(SERVEUR, NOM)?;
    tracing::info!("clé API supprimée du gestionnaire d'identifiants");
    Ok(etat_de(None))
}

#[tauri::command]
pub fn cle_api_etat() -> Result<EtatCle, String> {
    etat(&CoffreSysteme)
}

#[tauri::command]
pub fn cle_api_enregistrer(cle: String) -> Result<EtatCle, String> {
    enregistrer(&CoffreSysteme, &cle)
}

#[tauri::command]
pub fn cle_api_supprimer() -> Result<EtatCle, String> {
    supprimer(&CoffreSysteme)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    #[derive(Default)]
    struct CoffreMemoire(Mutex<BTreeMap<String, String>>);

    impl Coffre for CoffreMemoire {
        fn lire(&self, serveur: &str, nom: &str) -> Result<Option<String>, String> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .get(&format!("{serveur}/{nom}"))
                .cloned())
        }
        fn ecrire(&self, serveur: &str, nom: &str, valeur: &str) -> Result<(), String> {
            self.0
                .lock()
                .unwrap()
                .insert(format!("{serveur}/{nom}"), valeur.into());
            Ok(())
        }
        fn effacer(&self, serveur: &str, nom: &str) -> Result<(), String> {
            self.0.lock().unwrap().remove(&format!("{serveur}/{nom}"));
            Ok(())
        }
    }

    const CLE: &str = "sk-ant-api03-ABCDEFGHIJKLMNOP_qrstuv-wxyz1234";

    /// KOB-104 : saisie, remplacement, suppression ; l'état ne montre que
    /// la fin de la clé.
    #[test]
    fn saisie_remplacement_suppression() {
        let coffre = CoffreMemoire::default();
        assert_eq!(
            etat(&coffre).unwrap(),
            EtatCle {
                presente: false,
                fin: None
            }
        );
        let e = enregistrer(&coffre, &format!("  {CLE}\n")).unwrap();
        assert_eq!(e.fin.as_deref(), Some("1234"));
        assert_eq!(coffre.lire(SERVEUR, NOM).unwrap().as_deref(), Some(CLE));
        let autre = "sk-ant-api03-ZZZZZZZZZZZZZZZZ-9876";
        assert_eq!(
            enregistrer(&coffre, autre).unwrap().fin.as_deref(),
            Some("9876")
        );
        assert_eq!(coffre.lire(SERVEUR, NOM).unwrap().as_deref(), Some(autre));
        assert!(!supprimer(&coffre).unwrap().presente);
        assert!(!etat(&coffre).unwrap().presente);
        // Supprimer deux fois n'est pas une erreur.
        assert!(supprimer(&coffre).is_ok());
    }

    /// Une valeur qui n'a pas la forme d'une clé Anthropic est refusée,
    /// et rien n'est écrit.
    #[test]
    fn cle_mal_formee_refusee() {
        let coffre = CoffreMemoire::default();
        for mauvaise in [
            "",
            "sk-ant-",
            "sk-proj-abcdefghijklmnop",
            "sk-ant-abc def ghijklmnop",
            "bonjour",
        ] {
            assert!(enregistrer(&coffre, mauvaise).is_err(), "{mauvaise:?}");
        }
        assert!(!etat(&coffre).unwrap().presente);
    }

    /// KOB-104 : la clé n'apparaît dans aucun fichier de `~/Kobi` : ni
    /// réglages, ni journaux, ni données. L'état envoyé à l'interface ne la
    /// contient pas non plus.
    #[test]
    fn cle_jamais_sur_le_disque_ni_dans_l_etat() {
        let dir = tempfile::tempdir().unwrap();
        let kobi = dir.path().join("Kobi");
        // Un profil complet : réglages, journal, une réunion.
        let chemin = kobi.join("config.toml");
        let charge = kobi_session::config::load_or_create(&chemin);
        assert!(charge.problem.is_none());
        let coffre = CoffreMemoire::default();
        let e = enregistrer(&coffre, CLE).unwrap();
        let json = serde_json::to_string(&e).unwrap();
        assert!(
            !json.contains(CLE) && !json.contains("ABCDEFGHIJ"),
            "{json}"
        );
        // Réglages réécrits après l'enregistrement : la clé n'y est pas.
        kobi_session::config::save(&charge.config, &chemin).unwrap();
        for entree in walk(&kobi) {
            let contenu = std::fs::read(&entree).unwrap();
            assert!(
                !String::from_utf8_lossy(&contenu).contains("sk-ant-"),
                "clé trouvée dans {}",
                entree.display()
            );
        }
    }

    fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut fichiers = Vec::new();
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                fichiers.extend(walk(&p));
            } else {
                fichiers.push(p);
            }
        }
        fichiers
    }
}
