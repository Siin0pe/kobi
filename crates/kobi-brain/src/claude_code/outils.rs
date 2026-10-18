//! Table de traduction des outils de Claude Code en [`Action`] normalisées
//! (contrat commun des permissions, section 0). Seul cet adaptateur connaît
//! les noms `Read`, `Grep`, `Bash`, `mcp__<serveur>__<outil>`…

use std::path::PathBuf;

use serde_json::Value;

use crate::outils::{Action, Shell};

fn texte(entree: &Value, cle: &str) -> Option<String> {
    entree
        .get(cle)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn chemin(entree: &Value, cle: &str) -> Option<PathBuf> {
    texte(entree, cle).map(PathBuf::from)
}

/// Action normalisée d'un `tool_use` de Claude Code.
pub fn action(nom: &str, entree: &Value) -> Action {
    let autre = || Action::Autre {
        nom: nom.to_owned(),
        entree: entree.clone(),
    };
    if let Some(reste) = nom.strip_prefix("mcp__") {
        return match reste.split_once("__") {
            Some((serveur, outil)) => Action::Mcp {
                serveur: serveur.to_owned(),
                outil: outil.to_owned(),
                params: entree.clone(),
            },
            None => autre(),
        };
    }
    match nom {
        "Read" => chemin(entree, "file_path").map(|chemin| Action::LireFichier { chemin }),
        "Grep" => texte(entree, "pattern").map(|motif| Action::Chercher {
            motif,
            dossier: chemin(entree, "path"),
        }),
        // Recherche de fichiers par nom : on montre le motif dans son dossier.
        "Glob" => texte(entree, "pattern").map(|motif| Action::Lister {
            dossier: chemin(entree, "path")
                .map_or_else(|| motif.clone().into(), |d| d.join(&motif)),
        }),
        "LS" => chemin(entree, "path").map(|dossier| Action::Lister { dossier }),
        "Bash" | "PowerShell" => texte(entree, "command").map(|ligne| Action::Commande {
            ligne,
            dossier: None,
            shell: if nom == "Bash" {
                Shell::Bash
            } else {
                Shell::PowerShell
            },
        }),
        "Edit" => chemin(entree, "file_path").map(|chemin| Action::EcrireFichier {
            chemin,
            avant: Some(texte(entree, "old_string").unwrap_or_default()),
            apres: texte(entree, "new_string").unwrap_or_default(),
        }),
        "MultiEdit" => chemin(entree, "file_path").map(|chemin| {
            let edits = entree.get("edits").and_then(|e| e.as_array());
            let joindre = |cle: &str| {
                edits
                    .into_iter()
                    .flatten()
                    .filter_map(|e| texte(e, cle))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            Action::EcrireFichier {
                chemin,
                avant: Some(joindre("old_string")),
                apres: joindre("new_string"),
            }
        }),
        "Write" => chemin(entree, "file_path").map(|chemin| Action::EcrireFichier {
            chemin,
            avant: None,
            apres: texte(entree, "content").unwrap_or_default(),
        }),
        "NotebookEdit" => chemin(entree, "notebook_path").map(|chemin| Action::EcrireFichier {
            chemin,
            avant: Some(String::new()),
            apres: texte(entree, "new_source").unwrap_or_default(),
        }),
        _ => None,
    }
    .unwrap_or_else(autre)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn table_de_traduction() {
        assert_eq!(
            action("Read", &json!({"file_path": "C:/k/src/chef.rs"})),
            Action::LireFichier {
                chemin: "C:/k/src/chef.rs".into()
            }
        );
        assert_eq!(
            action("Grep", &json!({"pattern": "fabrique", "path": "C:/k"})),
            Action::Chercher {
                motif: "fabrique".into(),
                dossier: Some("C:/k".into())
            }
        );
        assert_eq!(
            action("Glob", &json!({"pattern": "**/*.rs", "path": "C:/k"})),
            Action::Lister {
                dossier: PathBuf::from("C:/k").join("**/*.rs")
            }
        );
        assert!(matches!(
            action("PowerShell", &json!({"command": "Get-ChildItem"})),
            Action::Commande { shell: Shell::PowerShell, ligne, .. } if ligne == "Get-ChildItem"
        ));
        assert!(matches!(
            action("Write", &json!({"file_path": "a.txt", "content": "x"})),
            Action::EcrireFichier { avant: None, apres, .. } if apres == "x"
        ));
        assert_eq!(
            action("mcp__linear__list_issues", &json!({"limit": 3})),
            Action::Mcp {
                serveur: "linear".into(),
                outil: "list_issues".into(),
                params: json!({"limit": 3})
            }
        );
        // Inconnu, ou entrée incomplète : jamais deviné.
        assert!(
            matches!(action("WebFetch", &json!({})), Action::Autre { nom, .. } if nom == "WebFetch")
        );
        assert!(matches!(action("Read", &json!({})), Action::Autre { .. }));
        assert!(matches!(
            action("mcp__seul", &json!({})),
            Action::Autre { .. }
        ));
    }
}
