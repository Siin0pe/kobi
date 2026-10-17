//! Workspaces : `~/Kobi/workspaces/<nom>.toml`.
//!
//! Un workspace réunit les serveurs MCP, les codebases, les outils autorisés
//! sans confirmation et la destination du bouton Publier. On le choisit au
//! démarrage d'une réunion ; le cerveau en tire `--mcp-config` et `--add-dir`.
//!
//! [`importer_claude_code`] reprend la config MCP de Claude Code : serveurs de
//! `~/.claude.json` (portée utilisateur et portée locale du projet) et du
//! `.mcp.json` du projet, plus les outils MCP déjà autorisés.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Dossier des workspaces : `~/Kobi/workspaces`.
pub fn default_dir() -> Option<PathBuf> {
    crate::config::kobi_dir().map(|dir| dir.join("workspaces"))
}

/// Préfixe des outils MCP dans Claude Code (`mcp__serveur__outil`).
const PREFIXE_OUTIL_MCP: &str = "mcp__";

#[derive(Debug, thiserror::Error)]
pub enum ErreurWorkspace {
    #[error(
        "nom de workspace refusé « {0} » : lettres minuscules, chiffres, `-` et `_` seulement"
    )]
    Nom(String),
    #[error("{chemin} : {detail}")]
    Lecture { chemin: PathBuf, detail: String },
    #[error("{chemin} : {source}")]
    Io {
        chemin: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("workspace impossible à écrire en TOML : {0}")]
    Toml(#[from] toml::ser::Error),
}

/// Transport d'un serveur MCP, comme dans la config de Claude Code.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    #[default]
    Stdio,
    Http,
    Sse,
}

/// Un serveur MCP. Les valeurs `${VAR}` sont gardées telles quelles : Claude
/// Code les remplace lui-même.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServeurMcp {
    #[serde(rename = "type", default)]
    pub transport: Transport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
}

impl ServeurMcp {
    /// Pourquoi ce serveur est inutilisable, s'il l'est.
    pub fn probleme(&self) -> Option<&'static str> {
        match self.transport {
            Transport::Stdio if self.command.as_deref().is_none_or(str::is_empty) => {
                Some("serveur stdio sans `command`")
            }
            Transport::Http | Transport::Sse if self.url.as_deref().is_none_or(str::is_empty) => {
                Some("serveur http ou sse sans `url`")
            }
            _ => None,
        }
    }
}

/// Où publier le compte rendu : un serveur MCP du workspace et une consigne
/// libre pour Claude (« page dans l'espace Projets », « commentaire sur
/// l'épique »…).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Publication {
    pub serveur: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub consigne: String,
}

/// Contenu d'un fichier `workspaces/<nom>.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Workspace {
    /// Dossiers de code passés à Claude Code (`--add-dir`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub codebases: Vec<PathBuf>,
    /// Outils autorisés sans carte de confirmation (`mcp__serveur__outil`),
    /// en plus des outils en lecture seule.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub outils_autorises: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publier: Option<Publication>,
    /// Serveurs MCP, par nom.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub mcp: BTreeMap<String, ServeurMcp>,
}

impl Workspace {
    pub fn parse(texte: &str, chemin: &Path) -> Result<Self, ErreurWorkspace> {
        let ws: Workspace = toml::from_str(texte).map_err(|e| ErreurWorkspace::Lecture {
            chemin: chemin.to_owned(),
            detail: e.to_string(),
        })?;
        for (nom, serveur) in &ws.mcp {
            if let Some(probleme) = serveur.probleme() {
                return Err(ErreurWorkspace::Lecture {
                    chemin: chemin.to_owned(),
                    detail: format!("mcp.{nom} : {probleme}"),
                });
            }
        }
        if let Some(p) = &ws.publier {
            if !ws.mcp.contains_key(&p.serveur) {
                return Err(ErreurWorkspace::Lecture {
                    chemin: chemin.to_owned(),
                    detail: format!("publier.serveur = « {} » absent de [mcp]", p.serveur),
                });
            }
        }
        Ok(ws)
    }

    pub fn to_toml(&self) -> Result<String, ErreurWorkspace> {
        Ok(toml::to_string(self)?)
    }

    /// Contenu du fichier passé à `claude --mcp-config`.
    pub fn mcp_config_json(&self) -> String {
        serde_json::json!({ "mcpServers": self.mcp }).to_string()
    }

    /// Reprend les serveurs et les outils MCP autorisés d'un import ; les
    /// serveurs déjà présents sont remplacés par ceux de l'import.
    pub fn ajouter_import(&mut self, import: &Import) {
        self.mcp.extend(import.serveurs.clone());
        for outil in &import.outils_autorises {
            if !self.outils_autorises.contains(outil) {
                self.outils_autorises.push(outil.clone());
            }
        }
    }
}

/// Vérifie un nom de workspace (il devient un nom de fichier).
pub fn verifier_nom(nom: &str) -> Result<(), ErreurWorkspace> {
    let ok = !nom.is_empty()
        && nom.len() <= 64
        && nom
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
    if ok {
        Ok(())
    } else {
        Err(ErreurWorkspace::Nom(nom.to_owned()))
    }
}

fn chemin_de(dossier: &Path, nom: &str) -> Result<PathBuf, ErreurWorkspace> {
    verifier_nom(nom)?;
    Ok(dossier.join(format!("{nom}.toml")))
}

/// Charge `dossier/<nom>.toml`.
pub fn charger(dossier: &Path, nom: &str) -> Result<Workspace, ErreurWorkspace> {
    let chemin = chemin_de(dossier, nom)?;
    let texte = fs::read_to_string(&chemin).map_err(|source| ErreurWorkspace::Io {
        chemin: chemin.clone(),
        source,
    })?;
    Workspace::parse(&texte, &chemin)
}

/// Écrit `dossier/<nom>.toml` (fichier temporaire puis renommage).
pub fn enregistrer(dossier: &Path, nom: &str, ws: &Workspace) -> Result<(), ErreurWorkspace> {
    let chemin = chemin_de(dossier, nom)?;
    let io_err = |source| ErreurWorkspace::Io {
        chemin: chemin.clone(),
        source,
    };
    fs::create_dir_all(dossier).map_err(io_err)?;
    let tmp = dossier.join(format!("{nom}.toml.tmp"));
    fs::write(&tmp, ws.to_toml()?).map_err(io_err)?;
    fs::rename(&tmp, &chemin).map_err(io_err)
}

/// Workspaces proposés au démarrage d'une réunion, triés par nom. Un fichier
/// illisible apparaît avec son erreur, sans masquer les autres.
pub fn lister(dossier: &Path) -> Vec<(String, Result<Workspace, ErreurWorkspace>)> {
    let Ok(entrees) = fs::read_dir(dossier) else {
        return Vec::new();
    };
    let mut liste: Vec<_> = entrees
        .filter_map(Result::ok)
        .filter_map(|e| {
            let chemin = e.path();
            if chemin.extension()? != "toml" {
                return None;
            }
            let nom = chemin.file_stem()?.to_str()?.to_owned();
            verifier_nom(&nom).ok()?;
            Some(nom)
        })
        .map(|nom| {
            let ws = charger(dossier, &nom);
            (nom, ws)
        })
        .collect();
    liste.sort_by(|a, b| a.0.cmp(&b.0));
    liste
}

/// Ce qui a été repris de la config de Claude Code.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Import {
    pub serveurs: BTreeMap<String, ServeurMcp>,
    /// Outils `mcp__…` déjà autorisés dans Claude Code pour ce projet.
    pub outils_autorises: Vec<String>,
    /// Serveurs écartés, avec la raison.
    pub ignores: Vec<(String, String)>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ClaudeJson {
    #[serde(default)]
    mcp_servers: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    projects: BTreeMap<String, ProjetClaude>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ProjetClaude {
    #[serde(default)]
    mcp_servers: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    allowed_tools: Vec<String>,
    #[serde(default)]
    disabled_mcpjson_servers: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct McpJson {
    #[serde(default)]
    mcp_servers: BTreeMap<String, serde_json::Value>,
}

fn lire_json<T: for<'de> Deserialize<'de> + Default>(chemin: &Path) -> Result<T, ErreurWorkspace> {
    match fs::read_to_string(chemin) {
        Ok(texte) => serde_json::from_str(&texte).map_err(|e| ErreurWorkspace::Lecture {
            chemin: chemin.to_owned(),
            detail: format!("JSON illisible : {e}"),
        }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(T::default()),
        Err(source) => Err(ErreurWorkspace::Io {
            chemin: chemin.to_owned(),
            source,
        }),
    }
}

impl Import {
    fn ajouter(&mut self, serveurs: BTreeMap<String, serde_json::Value>, portee: &str) {
        for (nom, valeur) in serveurs {
            let serveur = serde_json::from_value::<ServeurMcp>(valeur)
                .map_err(|e| e.to_string())
                .and_then(|s| match s.probleme() {
                    Some(p) => Err(p.to_owned()),
                    None => Ok(s),
                });
            match serveur {
                Ok(serveur) => {
                    self.ignores.retain(|(n, _)| *n != nom);
                    self.serveurs.insert(nom, serveur);
                }
                Err(raison) => self.ignores.push((nom, format!("{portee} : {raison}"))),
            }
        }
    }
}

/// Importe la config MCP de Claude Code.
///
/// `claude_json` est en général `~/.claude.json` ; `projet`, le dossier d'une
/// codebase. Comme dans Claude Code, la portée locale (le projet dans
/// `~/.claude.json`) l'emporte sur `.mcp.json`, qui l'emporte sur la portée
/// utilisateur. Un fichier absent n'est pas une erreur.
pub fn importer_claude_code(
    claude_json: &Path,
    projet: Option<&Path>,
) -> Result<Import, ErreurWorkspace> {
    let claude: ClaudeJson = lire_json(claude_json)?;
    let mut import = Import::default();
    import.ajouter(claude.mcp_servers, "utilisateur");

    let Some(projet) = projet else {
        return Ok(import);
    };
    let local = claude
        .projects
        .iter()
        .find(|(cle, _)| Path::new(cle) == projet)
        .map(|(_, p)| p);

    let mcp_json: McpJson = lire_json(&projet.join(".mcp.json"))?;
    let desactives = local.map(|p| &p.disabled_mcpjson_servers);
    let mcp_json: BTreeMap<_, _> = mcp_json
        .mcp_servers
        .into_iter()
        .filter(|(nom, _)| {
            let off = desactives.is_some_and(|d| d.contains(nom));
            if off {
                import
                    .ignores
                    .push((nom.clone(), "projet : désactivé dans Claude Code".into()));
            }
            !off
        })
        .collect();
    import.ajouter(mcp_json, "projet");

    if let Some(local) = local {
        import.ajouter(local.mcp_servers.clone(), "local");
        import.outils_autorises = local
            .allowed_tools
            .iter()
            .filter(|o| o.starts_with(PREFIXE_OUTIL_MCP))
            .cloned()
            .collect();
    }
    Ok(import)
}

/// `~/.claude.json`, s'il y a un dossier personnel.
pub fn claude_json_par_defaut() -> Option<PathBuf> {
    std::env::home_dir().map(|home| home.join(".claude.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORKSPACE: &str = r#"
codebases = ["/home/moi/code/api", "/home/moi/code/web"]
outils_autorises = ["mcp__linear__list_issues"]

[publier]
serveur = "colanode"
consigne = "page dans l'espace Projets"

[mcp.linear]
type = "http"
url = "https://mcp.linear.app/mcp"
headers = { Authorization = "Bearer ${LINEAR_API_KEY}" }

[mcp.colanode]
command = "npx"
args = ["-y", "@colanode/mcp"]
env = { COLANODE_TOKEN = "${COLANODE_TOKEN}" }
"#;

    #[test]
    fn lecture_d_un_workspace() {
        let ws = Workspace::parse(WORKSPACE, Path::new("api.toml")).unwrap();
        assert_eq!(ws.codebases.len(), 2);
        assert_eq!(ws.outils_autorises, ["mcp__linear__list_issues"]);
        assert_eq!(ws.publier.as_ref().unwrap().serveur, "colanode");
        let linear = &ws.mcp["linear"];
        assert_eq!(linear.transport, Transport::Http);
        assert_eq!(linear.headers["Authorization"], "Bearer ${LINEAR_API_KEY}");
        let colanode = &ws.mcp["colanode"];
        assert_eq!(colanode.transport, Transport::Stdio);
        assert_eq!(colanode.command.as_deref(), Some("npx"));
    }

    #[test]
    fn workspace_vide_accepte() {
        assert_eq!(
            Workspace::parse("", Path::new("vide.toml")).unwrap(),
            Workspace::default()
        );
    }

    #[test]
    fn workspaces_invalides() {
        for (texte, attendu) in [
            ("[mcp.x]\ntype = \"stdio\"", "sans `command`"),
            ("[mcp.x]\ntype = \"http\"", "sans `url`"),
            ("[mcp.x]\ntype = \"ws\"\nurl = \"u\"", "ws"),
            (
                "[publier]\nserveur = \"absent\"\n[mcp.x]\ncommand = \"c\"",
                "absent de [mcp]",
            ),
            ("codebases = \"/un/seul\"", "codebases"),
        ] {
            let e = Workspace::parse(texte, Path::new("w.toml"))
                .unwrap_err()
                .to_string();
            assert!(e.contains(attendu) && e.contains("w.toml"), "{texte} → {e}");
        }
    }

    #[test]
    fn enregistrer_puis_charger_et_lister() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::parse(WORKSPACE, Path::new("api.toml")).unwrap();
        enregistrer(dir.path(), "api", &ws).unwrap();
        enregistrer(dir.path(), "perso", &Workspace::default()).unwrap();
        fs::write(dir.path().join("casse.toml"), "codebases = [").unwrap();
        fs::write(dir.path().join("Pas Valide.toml"), "").unwrap();
        fs::write(dir.path().join("notes.txt"), "").unwrap();

        assert_eq!(charger(dir.path(), "api").unwrap(), ws);
        let liste = lister(dir.path());
        let noms: Vec<_> = liste.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(noms, ["api", "casse", "perso"]);
        assert!(liste[1].1.is_err());
        assert!(lister(&dir.path().join("absent")).is_empty());
    }

    #[test]
    fn noms_de_workspace() {
        for nom in ["api", "client-x", "perso_2"] {
            verifier_nom(nom).unwrap();
        }
        for nom in ["", "../x", "a/b", "Majuscule", "é", "a.b"] {
            assert!(verifier_nom(nom).is_err(), "{nom}");
        }
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            enregistrer(dir.path(), "../evil", &Workspace::default()),
            Err(ErreurWorkspace::Nom(_))
        ));
    }

    #[test]
    fn config_mcp_pour_claude() {
        let ws = Workspace::parse(WORKSPACE, Path::new("api.toml")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&ws.mcp_config_json()).unwrap();
        assert_eq!(json["mcpServers"]["linear"]["type"], "http");
        assert_eq!(
            json["mcpServers"]["linear"]["url"],
            "https://mcp.linear.app/mcp"
        );
        assert_eq!(json["mcpServers"]["colanode"]["command"], "npx");
        assert_eq!(json["mcpServers"]["colanode"]["args"][0], "-y");
        assert!(json["mcpServers"]["colanode"].get("url").is_none());
    }

    fn faux_claude(dir: &Path, projet: &Path) -> PathBuf {
        let claude_json = dir.join(".claude.json");
        let projet_cle = projet.to_str().unwrap();
        fs::write(
            &claude_json,
            serde_json::json!({
                "numStartups": 12,
                "oauthAccount": { "emailAddress": "ignore@exemple.fr" },
                "mcpServers": {
                    "linear": {
                        "type": "http",
                        "url": "https://mcp.linear.app/mcp",
                        "headers": { "Authorization": "Bearer user" }
                    },
                    "github": { "command": "gh-mcp", "args": ["--stdio"] },
                    "casse": { "type": "stdio" },
                    "bizarre": { "type": "websocket", "url": "ws://x" }
                },
                "projects": {
                    "/autre/projet": {
                        "mcpServers": { "autre": { "command": "x" } },
                        "allowedTools": ["mcp__autre__lire"]
                    },
                    projet_cle: {
                        "mcpServers": {
                            "linear": {
                                "type": "http",
                                "url": "https://mcp.linear.app/mcp",
                                "headers": { "Authorization": "Bearer local" }
                            }
                        },
                        "allowedTools": [
                            "Bash(git status)",
                            "mcp__linear__list_issues",
                            "Edit",
                            "mcp__colanode__colanode_search"
                        ],
                        "disabledMcpjsonServers": ["desactive"]
                    }
                }
            })
            .to_string(),
        )
        .unwrap();
        fs::create_dir_all(projet).unwrap();
        fs::write(
            projet.join(".mcp.json"),
            serde_json::json!({
                "mcpServers": {
                    "github": { "type": "stdio", "command": "gh-mcp-projet" },
                    "colanode": {
                        "type": "sse",
                        "url": "https://colanode.exemple/sse"
                    },
                    "desactive": { "command": "jamais" }
                }
            })
            .to_string(),
        )
        .unwrap();
        claude_json
    }

    #[test]
    fn import_de_claude_code() {
        let dir = tempfile::tempdir().unwrap();
        let projet = dir.path().join("code").join("api");
        let claude_json = faux_claude(dir.path(), &projet);

        let import = importer_claude_code(&claude_json, Some(&projet)).unwrap();

        let noms: Vec<_> = import.serveurs.keys().map(String::as_str).collect();
        assert_eq!(noms, ["colanode", "github", "linear"]);
        // Local > projet > utilisateur.
        assert_eq!(
            import.serveurs["linear"].headers["Authorization"],
            "Bearer local"
        );
        assert_eq!(
            import.serveurs["github"].command.as_deref(),
            Some("gh-mcp-projet")
        );
        assert_eq!(import.serveurs["colanode"].transport, Transport::Sse);
        assert_eq!(
            import.outils_autorises,
            ["mcp__linear__list_issues", "mcp__colanode__colanode_search"]
        );
        let ignores: Vec<_> = import.ignores.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(ignores, ["bizarre", "casse", "desactive"]);
        assert!(import.ignores[1].1.contains("sans `command`"));

        // Sans projet : la portée utilisateur seule.
        let user = importer_claude_code(&claude_json, None).unwrap();
        assert_eq!(
            user.serveurs.keys().map(String::as_str).collect::<Vec<_>>(),
            ["github", "linear"]
        );
        assert!(user.outils_autorises.is_empty());

        // Import dans un workspace, puis aller-retour TOML.
        let mut ws = Workspace {
            codebases: vec![projet.clone()],
            ..Default::default()
        };
        ws.ajouter_import(&import);
        ws.ajouter_import(&import);
        assert_eq!(ws.outils_autorises.len(), 2);
        enregistrer(dir.path(), "api", &ws).unwrap();
        assert_eq!(charger(dir.path(), "api").unwrap(), ws);
    }

    #[test]
    fn import_sans_claude_code() {
        let dir = tempfile::tempdir().unwrap();
        let import =
            importer_claude_code(&dir.path().join(".claude.json"), Some(dir.path())).unwrap();
        assert_eq!(import, Import::default());
    }

    #[test]
    fn import_json_illisible() {
        let dir = tempfile::tempdir().unwrap();
        let chemin = dir.path().join(".claude.json");
        fs::write(&chemin, "{ pas du json").unwrap();
        let e = importer_claude_code(&chemin, None).unwrap_err().to_string();
        assert!(
            e.contains(".claude.json") && e.contains("JSON illisible"),
            "{e}"
        );
    }
}
