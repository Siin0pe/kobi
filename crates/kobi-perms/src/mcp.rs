//! Classement des outils MCP (KOB-155), côté Kobi et indépendant du harness
//! (KOB-157) : un outil est identifié par son serveur et son nom, jamais par
//! le nom qu'un harness lui donne (`mcp__<serveur>__<outil>` n'est que la
//! traduction de l'adaptateur Claude Code).
//!
//! Contrat, repris en commentaire de KOB-156 :
//!
//! * la nature d'un outil vient de ses annotations MCP (`tools/list`) :
//!   `readOnlyHint: true` → [`Nature::Lecture`] ; sinon
//!   `destructiveHint: true` → [`Nature::Suppression`] ; sinon
//!   [`Nature::Ecriture`]. Sans annotation, ou outil absent du catalogue :
//!   prudence, [`Nature::Ecriture`] ;
//! * la lecture passe sans carte ; l'écriture demande une carte si le
//!   serveur est réglé « toujours demander » ([`Politique::Demander`], le
//!   défaut), passe seule s'il est réglé « faire sans demander » ; la
//!   suppression demande toujours une carte, comme un outil que Kobi n'a
//!   pas encore vu (absent du dernier `tools/list`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Rules;

/// Ce que fait un outil MCP.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Nature {
    /// Lit sans rien changer (`readOnlyHint`).
    Lecture,
    /// Modifie ou envoie quelque chose.
    Ecriture,
    /// Supprime ou écrase (`destructiveHint`).
    Suppression,
}

impl Nature {
    /// Nature d'après les `annotations` d'un outil de `tools/list`.
    pub fn depuis_annotations(annotations: Option<&Value>) -> Self {
        let indice = |cle: &str| {
            annotations
                .and_then(|a| a.get(cle))
                .and_then(Value::as_bool)
        };
        if indice("readOnlyHint") == Some(true) {
            Nature::Lecture
        } else if indice("destructiveHint") == Some(true) {
            Nature::Suppression
        } else {
            Nature::Ecriture
        }
    }
}

/// Réglage d'un serveur pour ce qui écrit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Politique {
    /// « Toujours demander » : carte pour chaque écriture.
    #[default]
    Demander,
    /// « Faire sans demander » : l'écriture passe seule, pas la suppression.
    SansDemander,
}

/// Ce que Kobi fait d'un appel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Libre,
    Carte,
}

/// Outils d'un serveur et leur nature, lus dans une réponse `tools/list`.
pub fn outils(tools_list: &Value) -> BTreeMap<String, Nature> {
    tools_list
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|outil| {
            let nom = outil.get("name")?.as_str()?;
            Some((
                nom.to_owned(),
                Nature::depuis_annotations(outil.get("annotations")),
            ))
        })
        .collect()
}

/// Un serveur connu de Kobi : sa politique et la nature de ses outils.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Serveur {
    pub politique: Politique,
    pub outils: BTreeMap<String, Nature>,
}

/// Serveurs MCP des sessions, par nom.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Catalogue {
    pub serveurs: BTreeMap<String, Serveur>,
}

impl Catalogue {
    /// Nature de `outil` sur `serveur` ; inconnu = écriture.
    pub fn nature(&self, serveur: &str, outil: &str) -> Nature {
        self.serveurs
            .get(serveur)
            .and_then(|s| s.outils.get(outil))
            .copied()
            .unwrap_or(Nature::Ecriture)
    }

    pub fn verdict(&self, serveur: &str, outil: &str) -> Verdict {
        let Some(s) = self.serveurs.get(serveur) else {
            return Verdict::Carte;
        };
        match (s.outils.get(outil), s.politique) {
            (Some(Nature::Lecture), _) | (Some(Nature::Ecriture), Politique::SansDemander) => {
                Verdict::Libre
            }
            _ => Verdict::Carte,
        }
    }

    /// Règles du serveur de permission : chaque outil libre y est autorisé
    /// d'office, sous le nom que lui donne Claude Code.
    pub fn regles(&self) -> Rules {
        let mut rules = Rules::default();
        for (serveur, s) in &self.serveurs {
            for outil in s.outils.keys() {
                if self.verdict(serveur, outil) == Verdict::Libre {
                    let nom = format!("mcp__{serveur}__{outil}");
                    match s.outils[outil] {
                        Nature::Lecture => rules.add_read_only(nom),
                        _ => rules.add_sans_demander(nom),
                    }
                }
            }
        }
        rules
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::Verdict as Regle;

    fn linear() -> Value {
        json!({ "tools": [
            { "name": "list_issues", "annotations": { "readOnlyHint": true } },
            { "name": "save_issue", "annotations": { "readOnlyHint": false, "destructiveHint": false } },
            { "name": "delete_comment", "annotations": { "destructiveHint": true } },
            { "name": "get_team" },
        ]})
    }

    fn catalogue(politique: Politique) -> Catalogue {
        Catalogue {
            serveurs: BTreeMap::from([(
                "linear".to_owned(),
                Serveur {
                    politique,
                    outils: outils(&linear()),
                },
            )]),
        }
    }

    #[test]
    fn nature_d_apres_les_annotations() {
        let o = outils(&linear());
        assert_eq!(o["list_issues"], Nature::Lecture);
        assert_eq!(o["save_issue"], Nature::Ecriture);
        assert_eq!(o["delete_comment"], Nature::Suppression);
        // Sans annotation : prudence.
        assert_eq!(o["get_team"], Nature::Ecriture);
        // readOnlyHint l'emporte : un outil qui ne fait que lire ne détruit rien.
        assert_eq!(
            Nature::depuis_annotations(Some(
                &json!({ "readOnlyHint": true, "destructiveHint": true })
            )),
            Nature::Lecture
        );
    }

    #[test]
    fn toujours_demander_par_defaut() {
        let c = catalogue(Politique::default());
        assert_eq!(c.verdict("linear", "list_issues"), Verdict::Libre);
        assert_eq!(c.verdict("linear", "save_issue"), Verdict::Carte);
        assert_eq!(c.verdict("linear", "delete_comment"), Verdict::Carte);
        assert_eq!(c.verdict("linear", "get_team"), Verdict::Carte);
        // Outil ou serveur inconnus : écriture, donc carte.
        assert_eq!(c.verdict("linear", "nouvel_outil"), Verdict::Carte);
        assert_eq!(c.verdict("inconnu", "list"), Verdict::Carte);
    }

    #[test]
    fn faire_sans_demander_sauf_suppression() {
        let c = catalogue(Politique::SansDemander);
        assert_eq!(c.verdict("linear", "save_issue"), Verdict::Libre);
        assert_eq!(c.verdict("linear", "get_team"), Verdict::Libre);
        assert_eq!(c.verdict("linear", "delete_comment"), Verdict::Carte);
        assert_eq!(c.verdict("linear", "nouvel_outil"), Verdict::Carte);
    }

    #[test]
    fn regles_du_serveur_de_permission() {
        let r = catalogue(Politique::Demander).regles();
        assert!(matches!(
            r.verdict("mcp__linear__list_issues"),
            Regle::Allow(_)
        ));
        assert_eq!(r.verdict("mcp__linear__save_issue"), Regle::Ask);
        assert_eq!(r.verdict("mcp__linear__delete_comment"), Regle::Ask);
        let r = catalogue(Politique::SansDemander).regles();
        assert!(matches!(
            r.verdict("mcp__linear__save_issue"),
            Regle::Allow(_)
        ));
        assert_eq!(r.verdict("mcp__linear__delete_comment"), Regle::Ask);
    }

    #[test]
    fn politique_en_toml() {
        assert_eq!(
            serde_json::to_value(Politique::SansDemander).unwrap(),
            "sans_demander"
        );
        assert_eq!(
            serde_json::from_value::<Politique>(json!("demander")).unwrap(),
            Politique::Demander
        );
    }
}
