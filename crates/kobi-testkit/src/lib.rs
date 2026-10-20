//! kobi-testkit : doublures de test du cerveau (faux `claude`, faux serveur
//! MCP) et cas de non-régression du prompt système.

pub mod cases;

use std::path::{Path, PathBuf};

/// Dossier temporaire d'un test, supprimé à sa fin, même en échec (KOB-244 :
/// des centaines de `kobi-*-<pid>-<cas>` restaient dans le dossier
/// temporaire, 3,5 Go pour ceux de la détection).
pub struct DossierTemp(PathBuf);

impl DossierTemp {
    /// `<temp>/kobi-<prefixe>-<pid>-<cas>`, vidé puis créé.
    pub fn nouveau(prefixe: &str, cas: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("kobi-{prefixe}-{}-{cas}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl std::ops::Deref for DossierTemp {
    type Target = PathBuf;
    fn deref(&self) -> &PathBuf {
        &self.0
    }
}

impl AsRef<Path> for DossierTemp {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<std::ffi::OsStr> for DossierTemp {
    fn as_ref(&self) -> &std::ffi::OsStr {
        self.0.as_os_str()
    }
}

impl Drop for DossierTemp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
