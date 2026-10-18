//! Serveurs MCP connectés à Kobi (KOB-155), section `[mcp]` de
//! `config.toml` :
//!
//! ```toml
//! [mcp.serveurs.linear]
//! transport = "http"
//! url = "https://mcp.linear.app/mcp"
//! ecriture = "demander"            # ou "sans_demander"
//! origine = "Claude Code (utilisateur)"
//! [mcp.serveurs.linear.outils]     # nature des outils au dernier test
//! list_issues = "lecture"
//! save_issue = "ecriture"
//!
//! [mcp.serveurs.notes]
//! transport = "stdio"
//! commande = "npx"
//! args = ["-y", "notes-mcp"]
//! env = { NOTES_DIR = "C:/notes" }
//! secrets = ["NOTES_TOKEN"]        # valeurs dans le gestionnaire d'identifiants
//! ```
//!
//! Aucun secret dans ce fichier : `secrets` ne liste que les noms des
//! variables (stdio) ou des en-têtes (HTTP) dont la valeur est gardée par le
//! système (voir `kobi-app`, module `mcp`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Comment Kobi joint le serveur.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    #[default]
    Stdio,
    Http,
    Sse,
}

/// Réglage d'un serveur pour ce qui modifie ou envoie quelque chose.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ecriture {
    /// « Toujours demander » : carte de confirmation.
    #[default]
    Demander,
    /// « Faire sans demander » (sauf suppression, toujours demandée).
    SansDemander,
}

/// Nature d'un outil, lue dans ses annotations au dernier test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NatureOutil {
    Lecture,
    Ecriture,
    Suppression,
}

fn vrai() -> bool {
    true
}

/// Un serveur MCP.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Serveur {
    /// Passé aux sessions de Kobi.
    #[serde(default = "vrai")]
    pub actif: bool,
    pub transport: Transport,
    /// stdio : programme et arguments.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub commande: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// stdio : variables d'environnement non secrètes.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// HTTP ou SSE.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub url: String,
    /// En-têtes non secrets.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub entetes: BTreeMap<String, String>,
    /// Noms des variables (stdio) ou en-têtes (HTTP) secrets.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub secrets: Vec<String>,
    pub ecriture: Ecriture,
    /// D'où vient le serveur (« Claude Code (utilisateur) », « ajouté »…).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origine: Option<String>,
    /// Outils vus au dernier test, et leur nature.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub outils: BTreeMap<String, NatureOutil>,
    /// Dernier test réussi, en ms depuis l'époque Unix.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub teste_ms: Option<u64>,
    #[serde(flatten)]
    pub extra: toml::Table,
}

impl Serveur {
    pub fn stdio(commande: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            actif: true,
            commande: commande.into(),
            args,
            ..Default::default()
        }
    }

    pub fn http(url: impl Into<String>) -> Self {
        Self {
            actif: true,
            transport: Transport::Http,
            url: url.into(),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct McpConfig {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub serveurs: BTreeMap<String, Serveur>,
    #[serde(flatten)]
    pub extra: toml::Table,
}

/// Nom de serveur accepté : lettres, chiffres, `-` et `_`, sans `__`
/// (séparateur des noms d'outils `mcp__<serveur>__<outil>`), et pas un nom
/// réservé à Kobi.
pub fn nom_valide(nom: &str) -> Result<(), String> {
    const RESERVES: [&str; 2] = ["kobi", "kobi-perms"];
    if nom.is_empty() || nom.len() > 64 {
        return Err("le nom fait de 1 à 64 caractères".into());
    }
    if !nom
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!(
            "« {nom} » : lettres, chiffres, - et _ seulement, sans espace"
        ));
    }
    if nom.contains("__") {
        return Err(format!("« {nom} » : pas de double _"));
    }
    if RESERVES.contains(&nom) {
        return Err(format!("« {nom} » est réservé à Kobi"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_lisible_sans_secret() {
        let mut linear = Serveur::http("https://mcp.linear.app/mcp");
        linear
            .outils
            .insert("list_issues".into(), NatureOutil::Lecture);
        let mut notes = Serveur::stdio("npx", vec!["-y".into(), "notes-mcp".into()]);
        notes.secrets.push("NOTES_TOKEN".into());
        notes.ecriture = Ecriture::SansDemander;
        let config = McpConfig {
            serveurs: BTreeMap::from([("linear".into(), linear), ("notes".into(), notes)]),
            ..Default::default()
        };
        let texte = toml::to_string(&config).unwrap();
        assert!(texte.contains("[serveurs.linear]"), "{texte}");
        assert!(texte.contains("transport = \"http\""), "{texte}");
        assert!(texte.contains("list_issues = \"lecture\""), "{texte}");
        assert!(texte.contains("ecriture = \"sans_demander\""), "{texte}");
        assert!(texte.contains("secrets = [\"NOTES_TOKEN\"]"), "{texte}");
        assert_eq!(toml::from_str::<McpConfig>(&texte).unwrap(), config);
    }

    #[test]
    fn defauts_d_un_serveur_partiel() {
        let s: Serveur = toml::from_str("url = \"https://x/mcp\"\ntransport = \"http\"").unwrap();
        assert!(s.actif);
        assert_eq!(s.ecriture, Ecriture::Demander);
    }

    #[test]
    fn noms_de_serveur() {
        assert!(nom_valide("linear").is_ok());
        assert!(nom_valide("linear-server").is_ok());
        assert!(nom_valide("analytics_mcp").is_ok());
        for faux in ["", "a b", "a__b", "kobi", "kobi-perms", "é"] {
            assert!(nom_valide(faux).is_err(), "{faux}");
        }
    }
}
