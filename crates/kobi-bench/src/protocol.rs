//! Règles de mesure communes à tous les benchmarks du poste :
//!
//! - **une mesure à la fois** : verrou `~/Kobi-bench/.bench.lock`, un dossier
//!   créé de façon atomique (avec un fichier `owner`) et supprimé à la fin ;
//! - **la moitié du CPU au plus** : priorité « en dessous de la normale » ; le
//!   nombre de threads des moteurs est fixé par `kobi-bench stt --threads`.

use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Verrou de mesure ; libéré à la destruction.
#[derive(Debug)]
pub struct BenchLock(PathBuf);

impl BenchLock {
    /// Emplacement commun : `~/Kobi-bench/.bench.lock`, ou `KOBI_BENCH_LOCK`
    /// (tests).
    pub fn default_path(home: &Path) -> PathBuf {
        std::env::var_os("KOBI_BENCH_LOCK")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("Kobi-bench").join(".bench.lock"))
    }

    /// Attend que le verrou soit libre puis le prend. `owner` est écrit dans
    /// le verrou pour que les autres sachent qui mesure.
    pub fn acquire(path: impl Into<PathBuf>, owner: &str) -> std::io::Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut announced = false;
        loop {
            match std::fs::create_dir(&path) {
                Ok(()) => {
                    let lock = Self(path);
                    let mut file = File::create(lock.0.join("owner"))?;
                    writeln!(file, "{owner} pid {}", std::process::id())?;
                    return Ok(lock);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if !announced {
                        let who = std::fs::read_to_string(path.join("owner")).unwrap_or_default();
                        eprintln!("verrou {} pris ({}), attente…", path.display(), who.trim());
                        announced = true;
                    }
                    std::thread::sleep(Duration::from_secs(15));
                }
                Err(e) => return Err(e),
            }
        }
    }
}

impl Drop for BenchLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Passe le processus courant en priorité « en dessous de la normale »
/// (héritée par les threads et les processus enfants). Sans effet hors
/// Windows : lancer alors la commande avec `nice`.
pub fn lower_priority() -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{
            GetCurrentProcess, SetPriorityClass, BELOW_NORMAL_PRIORITY_CLASS,
        };
        // SAFETY : pseudo-handle du processus courant, toujours valide.
        unsafe { SetPriorityClass(GetCurrentProcess(), BELOW_NORMAL_PRIORITY_CLASS) != 0 }
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verrou_exclusif_et_libere() {
        let dir = std::env::temp_dir().join(format!("kobi-lock-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join(".bench.lock");
        {
            let _lock = BenchLock::acquire(&path, "test").unwrap();
            let owner = std::fs::read_to_string(path.join("owner")).unwrap();
            assert!(owner.starts_with("test pid "), "{owner}");
            // Pris : une seconde création échoue.
            assert!(std::fs::create_dir(&path).is_err());
        }
        assert!(!path.exists(), "verrou libéré à la destruction");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
