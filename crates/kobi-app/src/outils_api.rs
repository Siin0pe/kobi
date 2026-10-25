//! Outils du backend clé API (KOB-102) : ce que Claude Code fournit de
//! lui-même, refait pour [`kobi_brain::ApiBrain`].
//!
//! * **Lecture des dossiers** déclarés (codebases, mémoire des réunions) :
//!   `Read`, `Grep` et `Glob`, mêmes noms et mêmes entrées que chez Claude
//!   Code, pour que la traduction en [`kobi_brain::Action`] et le prompt
//!   restent communs. Rien ne sort des dossiers : chaque chemin est résolu
//!   (liens compris) puis comparé aux dossiers permis.
//! * **Serveurs MCP** du workspace, par le client `rmcp` de Kobi : outils
//!   nommés `mcp__<serveur>__<outil>` comme chez Claude Code. Seuls les
//!   outils annoncés en lecture (`readOnlyHint`) sont proposés ; les
//!   écritures attendent la carte de confirmation (KOB-103).

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use kobi_perms::mcp::Nature;
use rmcp::service::RunningService;
use rmcp::RoleClient;
use serde_json::{json, Value};

/// Lignes lues par défaut (`Read`).
const LIGNES_LUES: usize = 2000;
/// Taille maximale d'un résultat rendu à Claude.
const TAILLE_MAX: usize = 100_000;
/// Fichiers plus gros : ignorés par `Grep`.
const FICHIER_MAX: u64 = 2_000_000;
/// Lignes trouvées au plus (`Grep`) et chemins au plus (`Glob`).
const RESULTATS_MAX: usize = 300;
/// Dossiers jamais parcourus.
const DOSSIERS_IGNORES: &[&str] = &[".git", "node_modules", "target", ".venv", "__pycache__"];

/// Lecture seule dans des dossiers permis.
#[derive(Debug, Clone)]
pub struct Lecteur {
    /// Dossiers permis, résolus.
    dossiers: Vec<PathBuf>,
}

impl Lecteur {
    /// Les dossiers absents sont ignorés.
    pub fn new(dossiers: &[PathBuf]) -> Self {
        Self {
            dossiers: dossiers
                .iter()
                .filter_map(|d| d.canonicalize().ok())
                .filter(|d| d.is_dir())
                .collect(),
        }
    }

    pub fn definitions() -> Vec<Value> {
        vec![
            json!({
                "name": "Read",
                "description": "Lit un fichier texte d'une codebase ou des réunions passées. Rend les lignes numérotées.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "file_path": { "type": "string", "description": "Chemin absolu du fichier." },
                        "offset": { "type": "integer", "description": "Première ligne lue (à partir de 1)." },
                        "limit": { "type": "integer", "description": "Nombre de lignes lues (2000 par défaut)." },
                    },
                    "required": ["file_path"],
                },
            }),
            json!({
                "name": "Grep",
                "description": "Cherche une expression régulière dans les fichiers des codebases et des réunions passées. Rend les lignes trouvées, avec fichier et numéro de ligne.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string", "description": "Expression régulière (syntaxe Rust)." },
                        "path": { "type": "string", "description": "Dossier ou fichier où chercher ; tous les dossiers permis par défaut." },
                        "glob": { "type": "string", "description": "Filtre sur le nom des fichiers, par exemple « *.md »." },
                        "-i": { "type": "boolean", "description": "Sans tenir compte de la casse." },
                    },
                    "required": ["pattern"],
                },
            }),
            json!({
                "name": "Glob",
                "description": "Liste les fichiers dont le chemin correspond à un motif (« **/*.rs », « src/*.ts »).",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string", "description": "Motif, relatif au dossier." },
                        "path": { "type": "string", "description": "Dossier de départ ; tous les dossiers permis par défaut." },
                    },
                    "required": ["pattern"],
                },
            }),
        ]
    }

    /// Chemin résolu, s'il est dans un dossier permis.
    fn permis(&self, chemin: &str) -> Result<PathBuf, String> {
        let brut = Path::new(chemin);
        let brut = match (brut.is_absolute(), self.dossiers.first()) {
            (false, Some(base)) => base.join(brut),
            _ => brut.to_path_buf(),
        };
        let resolu = brut.canonicalize().map_err(|e| format!("{chemin} : {e}"))?;
        if self.dossiers.iter().any(|d| resolu.starts_with(d)) {
            Ok(resolu)
        } else {
            Err(format!(
                "{chemin} est hors des dossiers permis ({})",
                self.liste()
            ))
        }
    }

    fn liste(&self) -> String {
        self.dossiers
            .iter()
            .map(|d| d.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Point de départ d'une recherche : `path` ou tous les dossiers.
    fn departs(&self, entree: &Value) -> Result<Vec<PathBuf>, String> {
        match entree["path"].as_str().filter(|p| !p.is_empty()) {
            Some(p) => Ok(vec![self.permis(p)?]),
            None if self.dossiers.is_empty() => Err("aucun dossier à lire".into()),
            None => Ok(self.dossiers.clone()),
        }
    }

    /// Fichiers sous `depart`, sans les dossiers ignorés ni les liens qui
    /// sortent des dossiers permis.
    fn fichiers(&self, depart: &Path) -> impl Iterator<Item = PathBuf> + '_ {
        walkdir::WalkDir::new(depart)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| {
                !(e.file_type().is_dir()
                    && DOSSIERS_IGNORES.contains(&e.file_name().to_string_lossy().as_ref()))
            })
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file())
            .map(|e| e.into_path())
    }

    pub fn lire(&self, entree: &Value) -> Result<String, String> {
        let chemin = self.permis(texte(entree, "file_path")?)?;
        let octets = std::fs::read(&chemin).map_err(|e| format!("{} : {e}", chemin.display()))?;
        if octets.contains(&0) {
            return Err(format!("{} n'est pas un fichier texte", chemin.display()));
        }
        let contenu = String::from_utf8_lossy(&octets);
        let debut = entree["offset"].as_u64().unwrap_or(1).max(1) as usize;
        let nombre = entree["limit"].as_u64().map_or(LIGNES_LUES, |n| n as usize);
        let mut sortie = String::new();
        for (i, ligne) in contenu.lines().enumerate().skip(debut - 1).take(nombre) {
            sortie.push_str(&format!("{}\t{ligne}\n", i + 1));
        }
        if sortie.is_empty() {
            sortie = "(fichier vide ou fin de fichier dépassée)".into();
        }
        Ok(couper(sortie))
    }

    pub fn chercher(&self, entree: &Value) -> Result<String, String> {
        let motif = regex::RegexBuilder::new(texte(entree, "pattern")?)
            .case_insensitive(entree["-i"].as_bool().unwrap_or(false))
            .build()
            .map_err(|e| format!("expression invalide : {e}"))?;
        let filtre = filtre(entree["glob"].as_str())?;
        let mut trouve = Vec::new();
        'tout: for depart in self.departs(entree)? {
            for fichier in self.fichiers(&depart) {
                let nom = fichier.file_name().unwrap_or_default().to_string_lossy();
                if filtre.as_ref().is_some_and(|f| !f.matches(&nom))
                    || fichier.metadata().map_or(true, |m| m.len() > FICHIER_MAX)
                {
                    continue;
                }
                let Ok(octets) = std::fs::read(&fichier) else {
                    continue;
                };
                if octets.contains(&0) {
                    continue;
                }
                for (i, ligne) in String::from_utf8_lossy(&octets).lines().enumerate() {
                    if motif.is_match(ligne) {
                        trouve.push(format!("{}:{}:{ligne}", fichier.display(), i + 1));
                        if trouve.len() >= RESULTATS_MAX {
                            trouve.push(format!("… arrêté à {RESULTATS_MAX} lignes"));
                            break 'tout;
                        }
                    }
                }
            }
        }
        Ok(if trouve.is_empty() {
            "Aucun résultat.".into()
        } else {
            couper(trouve.join("\n"))
        })
    }

    pub fn lister(&self, entree: &Value) -> Result<String, String> {
        let motif = glob::Pattern::new(texte(entree, "pattern")?)
            .map_err(|e| format!("motif invalide : {e}"))?;
        let options = glob::MatchOptions {
            require_literal_separator: true,
            ..Default::default()
        };
        let mut trouve = Vec::new();
        for depart in self.departs(entree)? {
            for fichier in self.fichiers(&depart) {
                let relatif = fichier.strip_prefix(&depart).unwrap_or(&fichier);
                if motif.matches_path_with(relatif, options) {
                    trouve.push(fichier.display().to_string());
                }
            }
        }
        trouve.sort();
        let total = trouve.len();
        trouve.truncate(RESULTATS_MAX);
        if total > RESULTATS_MAX {
            trouve.push(format!("… {total} fichiers, {RESULTATS_MAX} montrés"));
        }
        Ok(if trouve.is_empty() {
            "Aucun fichier.".into()
        } else {
            couper(trouve.join("\n"))
        })
    }
}

fn texte<'a>(entree: &'a Value, cle: &str) -> Result<&'a str, String> {
    entree[cle]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("paramètre « {cle} » manquant"))
}

fn filtre(glob: Option<&str>) -> Result<Option<glob::Pattern>, String> {
    glob.filter(|g| !g.is_empty())
        .map(|g| glob::Pattern::new(g).map_err(|e| format!("filtre invalide : {e}")))
        .transpose()
}

fn couper(mut s: String) -> String {
    if s.len() > TAILLE_MAX {
        let mut fin = TAILLE_MAX;
        while !s.is_char_boundary(fin) {
            fin -= 1;
        }
        s.truncate(fin);
        s.push_str("\n… (coupé)");
    }
    s
}

/// Serveur MCP connecté pour la réunion.
struct ServeurMcp {
    nom: String,
    client: RunningService<RoleClient, ()>,
    /// Outils de lecture proposés à Claude (nom côté serveur).
    outils: Vec<String>,
}

/// Outils de la réunion : lecture des dossiers et serveurs MCP.
pub struct OutillageKobi {
    lecteur: Lecteur,
    serveurs: Vec<ServeurMcp>,
    definitions: Vec<Value>,
}

impl OutillageKobi {
    /// `serveurs` : clients déjà connectés (voir [`crate::mcp::connecter`]),
    /// avec leur `tools/list`.
    pub fn new(
        dossiers: &[PathBuf],
        serveurs: Vec<(String, RunningService<RoleClient, ()>, Vec<Value>)>,
    ) -> Self {
        let mut definitions = Lecteur::definitions();
        let mut connectes = Vec::new();
        for (nom, client, outils) in serveurs {
            let mut lecture = Vec::new();
            for o in &outils {
                let Some(outil) = o["name"].as_str() else {
                    continue;
                };
                if Nature::depuis_annotations(o.get("annotations")) != Nature::Lecture {
                    continue;
                }
                definitions.push(json!({
                    "name": format!("mcp__{nom}__{outil}"),
                    "description": o["description"].as_str().unwrap_or_default(),
                    "input_schema": o.get("inputSchema").cloned()
                        .unwrap_or_else(|| json!({ "type": "object" })),
                }));
                lecture.push(outil.to_owned());
            }
            tracing::info!(serveur = %nom, outils = ?lecture, "outils MCP de lecture du backend clé API");
            connectes.push(ServeurMcp {
                nom,
                client,
                outils: lecture,
            });
        }
        Self {
            lecteur: Lecteur::new(dossiers),
            serveurs: connectes,
            definitions,
        }
    }

    /// Ferme les connexions MCP (fin de réunion).
    pub async fn fermer(self) {
        for s in self.serveurs {
            let _ = s.client.cancel().await;
        }
    }

    async fn appel_mcp(
        &self,
        serveur: &str,
        outil: &str,
        entree: &Value,
    ) -> Result<String, String> {
        let s = self
            .serveurs
            .iter()
            .find(|s| s.nom == serveur && s.outils.iter().any(|o| o == outil))
            .ok_or_else(|| format!("outil inconnu : mcp__{serveur}__{outil}"))?;
        let mut params = rmcp::model::CallToolRequestParams::new(outil.to_owned());
        params.arguments = entree.as_object().cloned();
        let resultat = s
            .client
            .call_tool(params)
            .await
            .map_err(|e| format!("{serveur} : {e}"))?;
        let texte: Vec<String> = resultat
            .content
            .iter()
            .filter_map(|c| serde_json::to_value(c).ok())
            .map(|c| match c["text"].as_str() {
                Some(t) if c["type"] == "text" => t.to_owned(),
                _ => c.to_string(),
            })
            .collect();
        let texte = couper(texte.join("\n"));
        if resultat.is_error == Some(true) {
            Err(texte)
        } else {
            Ok(texte)
        }
    }
}

impl kobi_brain::Outillage for OutillageKobi {
    fn definitions(&self) -> Vec<Value> {
        self.definitions.clone()
    }

    fn action(&self, nom: &str, entree: &Value) -> kobi_brain::Action {
        kobi_brain::claude_code::outils::action(nom, entree)
    }

    fn appeler<'a>(
        &'a self,
        nom: &'a str,
        entree: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(async move {
            if let Some((serveur, outil)) =
                nom.strip_prefix("mcp__").and_then(|r| r.split_once("__"))
            {
                return self.appel_mcp(serveur, outil, entree).await;
            }
            let lecteur = self.lecteur.clone();
            let (nom, entree) = (nom.to_owned(), entree.clone());
            // Parcours de disque : hors du runtime.
            tokio::task::spawn_blocking(move || match nom.as_str() {
                "Read" => lecteur.lire(&entree),
                "Grep" => lecteur.chercher(&entree),
                "Glob" => lecteur.lister(&entree),
                _ => Err(format!("outil inconnu : {nom}")),
            })
            .await
            .unwrap_or_else(|e| Err(e.to_string()))
        })
    }
}

#[cfg(test)]
mod tests;
