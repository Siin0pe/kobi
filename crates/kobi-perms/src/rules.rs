//! Règles de permission : lecture libre, écriture confirmée.
//!
//! Autorisés d'office : les outils MCP annotés `readOnlyHint` et ceux de la
//! liste blanche du workspace. Tout le reste passe par une carte ; sans
//! réponse dans le délai, c'est un refus. Un outil sans annotation n'est pas
//! présumé en lecture seule.

use std::collections::BTreeSet;

use serde_json::Value;

/// Ce que décident les règles, avant toute carte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Autorisé sans demander, avec la raison (pour les journaux).
    Allow(&'static str),
    /// Carte de confirmation.
    Ask,
}

/// Règles d'une réunion.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rules {
    /// Outils en lecture seule (`mcp__serveur__outil`), d'après les
    /// annotations des serveurs.
    read_only: BTreeSet<String>,
    /// Liste blanche du workspace : noms exacts, ou `mcp__serveur__*` pour
    /// tout un serveur.
    allowlist: Vec<String>,
    /// Outils d'écriture d'un serveur réglé « faire sans demander » (KOB-155).
    sans_demander: BTreeSet<String>,
}

impl Rules {
    /// `allowlist` : `outils_autorises` du workspace.
    pub fn new(allowlist: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            read_only: BTreeSet::new(),
            allowlist: allowlist.into_iter().map(Into::into).collect(),
            sans_demander: BTreeSet::new(),
        }
    }

    /// Lit la réponse `tools/list` du serveur MCP `server` et retient les
    /// outils annotés `readOnlyHint: true`. Renvoie leur nombre.
    pub fn add_tools_list(&mut self, server: &str, tools_list: &Value) -> usize {
        let before = self.read_only.len();
        let tools = tools_list
            .get("tools")
            .and_then(Value::as_array)
            .into_iter()
            .flatten();
        for tool in tools {
            let read_only = tool
                .pointer("/annotations/readOnlyHint")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if let (true, Some(name)) = (read_only, tool.get("name").and_then(Value::as_str)) {
                self.read_only.insert(format!("mcp__{server}__{name}"));
            }
        }
        self.read_only.len() - before
    }

    /// Outil en lecture seule connu.
    pub fn add_read_only(&mut self, tool_name: impl Into<String>) {
        self.read_only.insert(tool_name.into());
    }

    /// Outil d'écriture autorisé sans carte (serveur « faire sans demander »).
    pub fn add_sans_demander(&mut self, tool_name: impl Into<String>) {
        self.sans_demander.insert(tool_name.into());
    }

    pub fn verdict(&self, tool_name: &str) -> Verdict {
        if self.read_only.contains(tool_name) {
            return Verdict::Allow("outil en lecture seule (readOnlyHint)");
        }
        if self.sans_demander.contains(tool_name) {
            return Verdict::Allow("serveur MCP réglé « faire sans demander »");
        }
        let listed = self
            .allowlist
            .iter()
            .any(|entry| match entry.strip_suffix('*') {
                // `mcp__linear__*` : tout le serveur, pas `mcp__linear_bis__…`.
                Some(prefix) => {
                    let server = prefix
                        .strip_prefix("mcp__")
                        .and_then(|rest| rest.strip_suffix("__"));
                    server.is_some_and(|s| !s.is_empty() && !s.contains("__"))
                        && tool_name.starts_with(prefix)
                }
                None => entry == tool_name,
            });
        if listed {
            return Verdict::Allow("liste blanche du workspace");
        }
        Verdict::Ask
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn linear_tools() -> Value {
        json!({ "tools": [
            { "name": "list_issues", "annotations": { "readOnlyHint": true } },
            { "name": "save_issue", "annotations": { "readOnlyHint": false, "destructiveHint": false } },
            { "name": "delete_issue", "annotations": { "destructiveHint": true } },
            { "name": "get_team" },
        ]})
    }

    #[test]
    fn lecture_seule_autorisee_d_office() {
        let mut rules = Rules::default();
        assert_eq!(rules.add_tools_list("linear", &linear_tools()), 1);
        assert_eq!(
            rules.verdict("mcp__linear__list_issues"),
            Verdict::Allow("outil en lecture seule (readOnlyHint)")
        );
    }

    #[test]
    fn ecriture_et_suppression_demandent_confirmation() {
        let mut rules = Rules::default();
        rules.add_tools_list("linear", &linear_tools());
        assert_eq!(rules.verdict("mcp__linear__save_issue"), Verdict::Ask);
        assert_eq!(rules.verdict("mcp__linear__delete_issue"), Verdict::Ask);
    }

    #[test]
    fn outil_sans_annotation_demande_confirmation() {
        let mut rules = Rules::default();
        rules.add_tools_list("linear", &linear_tools());
        assert_eq!(rules.verdict("mcp__linear__get_team"), Verdict::Ask);
    }

    #[test]
    fn liste_blanche_du_workspace() {
        let rules = Rules::new(["mcp__linear__save_issue", "mcp__notes__*"]);
        assert_eq!(
            rules.verdict("mcp__linear__save_issue"),
            Verdict::Allow("liste blanche du workspace")
        );
        assert_eq!(
            rules.verdict("mcp__notes__write_note"),
            Verdict::Allow("liste blanche du workspace")
        );
        assert_eq!(rules.verdict("mcp__linear__delete_issue"), Verdict::Ask);
        // Le joker couvre un serveur entier, pas un préfixe de nom.
        assert_eq!(rules.verdict("mcp__notes_perso__write_note"), Verdict::Ask);
    }

    #[test]
    fn joker_sans_serveur_ignore() {
        let rules = Rules::new(["mcp__*", "*"]);
        assert_eq!(rules.verdict("mcp__linear__save_issue"), Verdict::Ask);
        assert_eq!(rules.verdict("Write"), Verdict::Ask);
    }

    #[test]
    fn outil_inconnu_ou_interne_demande_confirmation() {
        let rules = Rules::default();
        assert_eq!(rules.verdict("Write"), Verdict::Ask);
        assert_eq!(rules.verdict("mcp__inconnu__x"), Verdict::Ask);
    }
}
