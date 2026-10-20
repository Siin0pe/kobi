//! Dépôts de code sous les dossiers déclarés (KOB-153) : leur nom, leur
//! branche et leur langage principal. Ils forment la carte ajoutée au prompt
//! de Claude, pour qu'il cherche au bon endroit, et la liste proposée au
//! premier lancement (`lister_depots`).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

/// Profondeur de recherche des dépôts sous un dossier déclaré (le dossier
/// lui-même compris).
const PROFONDEUR_DEPOTS: usize = 2;
/// Profondeur et nombre de fichiers lus pour deviner le langage.
const PROFONDEUR_LANGAGE: usize = 4;
const FICHIERS_LANGAGE: usize = 3000;
/// Dossiers jamais parcourus : dépendances, sorties de compilation.
const IGNORES: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    "out",
    "vendor",
    "venv",
    "__pycache__",
];

/// Un dépôt git trouvé.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Depot {
    pub nom: String,
    pub chemin: PathBuf,
    pub branche: Option<String>,
    pub langage: Option<String>,
}

fn ignore(nom: &str) -> bool {
    nom.starts_with('.') || IGNORES.contains(&nom)
}

fn sous_dossiers(dossier: &Path) -> Vec<PathBuf> {
    let mut sous: Vec<PathBuf> = fs::read_dir(dossier)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| !ignore(&e.file_name().to_string_lossy()))
        .map(|e| e.path())
        .collect();
    sous.sort();
    sous
}

/// Dossier `.git` du dépôt (suit le fichier `.git` d'un worktree ou d'un
/// sous-module).
fn dossier_git(depot: &Path) -> Option<PathBuf> {
    let git = depot.join(".git");
    if git.is_dir() {
        return Some(git);
    }
    let contenu = fs::read_to_string(&git).ok()?;
    let cible = contenu.trim().strip_prefix("gitdir:")?.trim();
    let cible = PathBuf::from(cible);
    Some(if cible.is_absolute() {
        cible
    } else {
        depot.join(cible)
    })
}

/// Branche courante, ou début du commit si la tête est détachée.
fn branche(depot: &Path) -> Option<String> {
    let head = fs::read_to_string(dossier_git(depot)?.join("HEAD")).ok()?;
    let head = head.trim();
    match head.strip_prefix("ref:") {
        Some(r) => Some(r.trim().trim_start_matches("refs/heads/").to_owned()),
        None => Some(head.chars().take(7).collect()).filter(|h: &String| !h.is_empty()),
    }
}

fn langage_de(extension: &str) -> Option<&'static str> {
    Some(match extension {
        "rs" => "Rust",
        "ts" | "tsx" => "TypeScript",
        "js" | "jsx" | "mjs" | "cjs" => "JavaScript",
        "py" => "Python",
        "go" => "Go",
        "java" => "Java",
        "kt" | "kts" => "Kotlin",
        "cs" => "C#",
        "cpp" | "cc" | "cxx" | "hpp" | "hh" => "C++",
        "c" | "h" => "C",
        "rb" => "Ruby",
        "php" => "PHP",
        "swift" => "Swift",
        "dart" => "Dart",
        "vue" => "Vue",
        "svelte" => "Svelte",
        "html" | "htm" => "HTML",
        "css" | "scss" => "CSS",
        "sh" | "bash" => "Shell",
        "ps1" => "PowerShell",
        _ => return None,
    })
}

/// Langage le plus représenté parmi les premiers fichiers du dépôt.
fn langage(depot: &Path) -> Option<String> {
    let mut comptes: HashMap<&'static str, usize> = HashMap::new();
    let mut vus = 0;
    let mut a_voir = vec![(depot.to_path_buf(), 0)];
    while let Some((dossier, profondeur)) = a_voir.pop() {
        let Ok(entrees) = fs::read_dir(&dossier) else {
            continue;
        };
        for e in entrees.flatten() {
            if vus >= FICHIERS_LANGAGE {
                break;
            }
            let Ok(t) = e.file_type() else { continue };
            let nom = e.file_name().to_string_lossy().into_owned();
            if t.is_dir() {
                if profondeur < PROFONDEUR_LANGAGE && !ignore(&nom) {
                    a_voir.push((e.path(), profondeur + 1));
                }
            } else if t.is_file() {
                vus += 1;
                if let Some(l) = Path::new(&nom)
                    .extension()
                    .and_then(|x| langage_de(&x.to_string_lossy().to_lowercase()))
                {
                    *comptes.entry(l).or_default() += 1;
                }
            }
        }
    }
    comptes
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(a.0)))
        .map(|(l, _)| l.to_owned())
}

fn depot(chemin: &Path) -> Depot {
    Depot {
        nom: chemin
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| chemin.display().to_string()),
        chemin: chemin.to_path_buf(),
        branche: branche(chemin),
        langage: langage(chemin),
    }
}

/// Dépôts git sous `dossier`, sur [`PROFONDEUR_DEPOTS`] niveaux, le dossier
/// lui-même compris. On ne descend pas dans un dépôt trouvé.
pub fn lister(dossier: &Path) -> Vec<Depot> {
    let mut trouves = Vec::new();
    let mut a_voir = vec![(dossier.to_path_buf(), 0)];
    while let Some((d, profondeur)) = a_voir.pop() {
        if dossier_git(&d).is_some() {
            trouves.push(depot(&d));
            continue;
        }
        if profondeur < PROFONDEUR_DEPOTS {
            a_voir.extend(sous_dossiers(&d).into_iter().map(|s| (s, profondeur + 1)));
        }
    }
    trouves.sort_by_key(|d| d.nom.to_lowercase());
    trouves
}

/// Carte des dossiers de code pour le prompt système de Claude.
pub fn carte(dossiers: &[PathBuf]) -> String {
    let mut texte = String::from("## Dossiers de code\n\n");
    let Some(travail) = dossiers.first() else {
        texte.push_str(
            "Aucun dossier de code n'est déclaré : si on te pose une question sur du code, \
             dis en une phrase qu'il faut d'abord choisir des dossiers dans les réglages.",
        );
        return texte;
    };
    texte.push_str(&format!(
        "Ton dossier de travail est `{}`. Tu peux lire ces dossiers et les dépôts qu'ils contiennent :\n",
        travail.display()
    ));
    for dossier in dossiers {
        texte.push_str(&format!("\n- `{}`", dossier.display()));
        let depots = lister(dossier);
        if depots.is_empty() {
            texte.push_str(" (aucun dépôt git trouvé)");
        }
        for d in depots {
            let mut details = Vec::new();
            if let Some(b) = &d.branche {
                details.push(format!("branche {b}"));
            }
            if let Some(l) = &d.langage {
                details.push(l.clone());
            }
            let details = if details.is_empty() {
                String::new()
            } else {
                format!(", {}", details.join(", "))
            };
            texte.push_str(&format!(
                "\n  - {} : `{}`{details}",
                d.nom,
                d.chemin.display()
            ));
        }
    }
    texte.push_str(
        "\n\nCherche toujours avec un chemin explicite : le paramètre `path` de Grep et Glob, \
         un chemin absolu pour Read, dans le dépôt concerné. Si la question nomme un projet \
         (« dans Kobi », « sur le site »), cherche d'abord dans le dépôt de ce nom (sans tenir \
         compte de la casse), puis ailleurs seulement s'il n'y est pas.",
    );
    texte
}

/// Dépôts trouvés sous `dossier`, à proposer au premier lancement et dans
/// les réglages (KOB-153, KOB-151).
#[tauri::command]
pub async fn lister_depots(dossier: String) -> Result<Vec<Depot>, String> {
    let dossier = PathBuf::from(dossier.trim());
    if !dossier.is_dir() {
        return Err(kobi_i18n::tr(
            "coque.commun.dossierIntrouvable",
            &[("chemin", &dossier.display().to_string())],
        ));
    }
    tauri::async_runtime::spawn_blocking(move || lister(&dossier))
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn depot_git(chemin: &Path, head: &str, fichiers: &[&str]) {
        fs::create_dir_all(chemin.join(".git")).unwrap();
        fs::write(chemin.join(".git/HEAD"), head).unwrap();
        for f in fichiers {
            let p = chemin.join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, "x").unwrap();
        }
    }

    /// Un « Bureau » avec plusieurs dépôts, comme celui du propriétaire.
    fn bureau() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let b = dir.path();
        depot_git(
            &b.join("kobi"),
            "ref: refs/heads/main\n",
            &[
                "Cargo.toml",
                "crates/kobi-app/src/chef.rs",
                "crates/a.rs",
                "ui/src/main.ts",
            ],
        );
        depot_git(
            &b.join("Projets/site"),
            "ref: refs/heads/prod\n",
            &["index.html", "app.ts", "page.ts", "style.css"],
        );
        // Tête détachée.
        depot_git(&b.join("outil"), "0123456789abcdef\n", &["main.py"]);
        // Worktree : `.git` est un fichier qui pointe ailleurs.
        let wt_git = b.join("kobi/.git/worktrees/voix");
        fs::create_dir_all(&wt_git).unwrap();
        fs::write(wt_git.join("HEAD"), "ref: refs/heads/feat/voix\n").unwrap();
        fs::create_dir_all(b.join("kobi-voix")).unwrap();
        fs::write(
            b.join("kobi-voix/.git"),
            format!("gitdir: {}\n", wt_git.display()),
        )
        .unwrap();
        // Trop profond (3 niveaux) et dossiers ignorés.
        depot_git(&b.join("a/b/c/profond"), "ref: refs/heads/main\n", &[]);
        depot_git(&b.join("node_modules/dep"), "ref: refs/heads/main\n", &[]);
        fs::create_dir_all(b.join("Documents")).unwrap();
        dir
    }

    #[test]
    fn depots_sur_deux_niveaux() {
        let dir = bureau();
        let depots = lister(dir.path());
        let noms: Vec<_> = depots.iter().map(|d| d.nom.as_str()).collect();
        assert_eq!(noms, ["kobi", "kobi-voix", "outil", "site"]);
        let kobi = &depots[0];
        assert_eq!(kobi.branche.as_deref(), Some("main"));
        assert_eq!(kobi.langage.as_deref(), Some("Rust"));
        assert_eq!(depots[1].branche.as_deref(), Some("feat/voix"));
        assert_eq!(depots[2].branche.as_deref(), Some("0123456"));
        assert_eq!(depots[2].langage.as_deref(), Some("Python"));
        assert_eq!(depots[3].branche.as_deref(), Some("prod"));
        assert_eq!(depots[3].langage.as_deref(), Some("TypeScript"));
        // Un dossier déclaré qui est lui-même un dépôt.
        let seul = lister(&dir.path().join("kobi"));
        assert_eq!(seul.len(), 1);
        assert_eq!(seul[0].nom, "kobi");
    }

    #[test]
    fn carte_pour_le_prompt() {
        let dir = bureau();
        let autre = tempfile::tempdir().unwrap();
        let c = carte(&[dir.path().to_path_buf(), autre.path().to_path_buf()]);
        assert!(c.contains(&format!(
            "dossier de travail est `{}`",
            dir.path().display()
        )));
        assert!(c.contains(&format!(
            "kobi : `{}`, branche main, Rust",
            dir.path().join("kobi").display()
        )));
        assert!(c.contains("site : "));
        assert!(c.contains("(aucun dépôt git trouvé)"));
        assert!(c.contains("chemin explicite"));
        assert!(c.contains("dans Kobi"));
        assert!(carte(&[]).contains("Aucun dossier de code"));
    }
}
