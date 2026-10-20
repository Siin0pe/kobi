//! Détection de Claude Code au démarrage : où est `claude`, répond-il, et
//! l'utilisateur est-il connecté ? Sans Claude Code prêt, Kobi propose la
//! clé API.
//!
//! Aucune de ces vérifications n'appelle l'API Anthropic : seulement
//! `claude --version` et `claude auth status --json`.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use tokio::process::Command;

/// Délai par défaut de chaque commande de vérification.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Un `claude` trouvé et qui répond.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeInstall {
    pub program: PathBuf,
    /// Sortie de `claude --version` (`2.1.302 (Claude Code)`).
    pub version: String,
}

/// Résultat de la détection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Detection {
    /// Installé et connecté : le backend Claude Code est utilisable.
    /// `auth_method` vaut `None` si l'état de connexion n'a pas pu être lu.
    Ready {
        install: ClaudeInstall,
        auth_method: Option<String>,
    },
    /// Installé mais pas connecté (`claude auth login` à faire).
    NotLoggedIn(ClaudeInstall),
    /// Aucun `claude` dans le PATH ni aux emplacements connus.
    NotFound,
    /// Trouvé, mais il ne répond pas correctement.
    Broken { program: PathBuf, reason: String },
}

impl Detection {
    pub fn is_ready(&self) -> bool {
        matches!(self, Detection::Ready { .. })
    }

    /// Phrase à montrer à l'utilisateur quand Claude Code n'est pas prêt,
    /// avec la proposition de la clé API.
    pub fn advice(&self) -> Option<String> {
        let reason = match self {
            Detection::Ready { .. } => return None,
            Detection::NotLoggedIn(_) => {
                "Claude Code est installé mais pas connecté : lancez `claude auth login`.".into()
            }
            Detection::NotFound => "Claude Code est introuvable.".into(),
            Detection::Broken { program, reason } => {
                format!(
                    "Claude Code ({}) ne répond pas : {reason}.",
                    program.display()
                )
            }
        };
        Some(format!("{reason} Vous pouvez aussi utiliser une clé API."))
    }
}

/// Noms de l'exécutable selon l'OS.
fn exe_names() -> &'static [&'static str] {
    if cfg!(windows) {
        &["claude.exe", "claude.cmd"]
    } else {
        &["claude"]
    }
}

/// Emplacements d'installation connus, hors PATH (une app lancée depuis le
/// bureau n'a souvent pas le PATH du terminal).
pub fn known_locations() -> Vec<PathBuf> {
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    let mut dirs = Vec::new();
    if let Some(home) = env("HOME").or_else(|| env("USERPROFILE")) {
        // Installeur natif, ancienne installation locale, npm global.
        dirs.push(home.join(".local").join("bin"));
        dirs.push(home.join(".claude").join("local"));
        dirs.push(home.join(".npm-global").join("bin"));
    }
    if cfg!(windows) {
        if let Some(appdata) = env("APPDATA") {
            dirs.push(appdata.join("npm"));
        }
    } else {
        dirs.push(PathBuf::from("/usr/local/bin"));
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
    }
    dirs
}

/// Exécutables `claude` présents dans `path_var` (format de `PATH`) puis
/// dans `extra`, dans cet ordre, sans doublon.
pub fn find_in(path_var: Option<&OsStr>, extra: &[PathBuf]) -> Vec<PathBuf> {
    let dirs = path_var
        .map(|p| std::env::split_paths(p).collect::<Vec<_>>())
        .unwrap_or_default();
    let mut seen = HashSet::new();
    let mut found = Vec::new();
    for dir in dirs.iter().chain(extra) {
        for name in exe_names() {
            let candidate = dir.join(name);
            if is_executable(&candidate) {
                let key = candidate
                    .canonicalize()
                    .unwrap_or_else(|_| candidate.clone());
                if seen.insert(key) {
                    found.push(candidate);
                }
            }
        }
    }
    found
}

/// Candidats de la machine : PATH puis emplacements connus.
pub fn candidates() -> Vec<PathBuf> {
    find_in(std::env::var_os("PATH").as_deref(), &known_locations())
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// Cherche `claude` et vérifie le premier candidat qui répond. Si aucun ne
/// répond, renvoie l'échec du premier.
pub async fn detect(timeout: Duration) -> Detection {
    let mut first_failure = None;
    for program in candidates() {
        match probe(&program, timeout).await {
            d @ Detection::Broken { .. } => {
                first_failure.get_or_insert(d);
            }
            d => return d,
        }
    }
    first_failure.unwrap_or(Detection::NotFound)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthStatus {
    logged_in: bool,
    #[serde(default)]
    auth_method: Option<String>,
}

/// Vérifie un exécutable précis.
pub async fn probe(program: &Path, timeout: Duration) -> Detection {
    let broken = |reason: String| Detection::Broken {
        program: program.to_owned(),
        reason,
    };
    let version = match run(program, &["--version"], timeout).await {
        Ok((true, stdout)) if !stdout.trim().is_empty() => stdout.trim().to_owned(),
        Ok((true, _)) => return broken("`--version` ne renvoie rien".into()),
        Ok((false, _)) => return broken("`--version` échoue".into()),
        Err(reason) => return broken(reason),
    };
    let install = ClaudeInstall {
        program: program.to_owned(),
        version,
    };
    // `auth status` sort en erreur quand on n'est pas connecté, mais écrit
    // quand même son JSON. Sans réponse lisible (version trop ancienne, ou
    // commande bloquée, vu le 17/10/2026 avec plusieurs sessions ouvertes),
    // l'état de connexion reste inconnu : la première question le dira.
    let status = run(program, &["auth", "status", "--json"], timeout)
        .await
        .ok()
        .and_then(|(_, stdout)| serde_json::from_str::<AuthStatus>(&stdout).ok());
    match status {
        Some(status) if !status.logged_in => Detection::NotLoggedIn(install),
        Some(status) => Detection::Ready {
            install,
            auth_method: status.auth_method,
        },
        None => Detection::Ready {
            install,
            auth_method: None,
        },
    }
}

/// Lance `program args`, renvoie (succès, stdout).
async fn run(program: &Path, args: &[&str], timeout: Duration) -> Result<(bool, String), String> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    // Pas de fenêtre de console depuis une app sans console (Windows).
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let child = cmd
        .spawn()
        .map_err(|e| format!("lancement impossible : {e}"))?;
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(out)) => Ok((
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
        )),
        Ok(Err(e)) => Err(format!("lecture de la sortie : {e}")),
        Err(_) => Err(format!(
            "pas de réponse en {} s à `{}`",
            timeout.as_secs_f32(),
            args.join(" ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> crate::dossier_test::DossierTemp {
        crate::dossier_test::DossierTemp::nouveau("detect", name)
    }

    fn touch_exe(dir: &Path) -> PathBuf {
        let path = dir.join(exe_names()[0]);
        std::fs::write(&path, b"").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    #[test]
    fn path_avant_les_emplacements_connus_sans_doublon() {
        let a = temp_dir("a");
        let b = temp_dir("b");
        let vide = temp_dir("vide");
        let in_a = touch_exe(&a);
        let in_b = touch_exe(&b);
        let path = std::env::join_paths([&vide, &a]).unwrap();
        assert_eq!(find_in(Some(&path), &[b.clone(), a.clone()]), [in_a, in_b]);
    }

    #[test]
    fn rien_dans_un_path_vide() {
        let vide = temp_dir("rien");
        let path = std::env::join_paths([&vide]).unwrap();
        assert!(find_in(Some(&path), &[]).is_empty());
        assert!(find_in(None, &[]).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn ignore_un_fichier_non_executable() {
        let dir = temp_dir("non-exe");
        std::fs::write(dir.join("claude"), b"").unwrap();
        assert!(find_in(None, &[dir.to_path_buf()]).is_empty());
    }

    #[tokio::test]
    async fn binaire_absent() {
        let d = probe(Path::new("kobi-claude-introuvable"), PROBE_TIMEOUT).await;
        assert!(matches!(d, Detection::Broken { .. }));
        assert!(d.advice().unwrap().contains("clé API"));
    }

    #[test]
    fn conseils() {
        assert_eq!(
            Detection::NotFound.advice().unwrap(),
            "Claude Code est introuvable. Vous pouvez aussi utiliser une clé API."
        );
        let install = ClaudeInstall {
            program: "claude".into(),
            version: "2.1.302 (Claude Code)".into(),
        };
        assert!(Detection::NotLoggedIn(install.clone())
            .advice()
            .unwrap()
            .contains("claude auth login"));
        assert!(Detection::Ready {
            install,
            auth_method: None
        }
        .advice()
        .is_none());
    }
}
