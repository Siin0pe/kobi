//! Règles de mesure communes à tous les benchmarks du poste :
//!
//! - **une mesure à la fois** : verrou `~/Kobi-bench/.bench.lock`, un dossier
//!   créé de façon atomique (avec un fichier `owner`) et supprimé à la fin.
//!   Un verrou pris par [`BenchLock`] dont le processus est mort (tué avant
//!   d'avoir pu le supprimer) est repris ; ceux des autres outils ne le sont
//!   jamais, leur pid pouvant être celui de MSYS et non celui du système ;
//! - **CPU partagé** : priorité « en dessous de la normale » ; le
//!   nombre de threads des moteurs est fixé par `kobi-bench stt --threads`
//!   ([`DEFAULT_THREADS`] par défaut), reporté sur rayon et OpenMP par
//!   [`limit_threads`].

use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// Threads par défaut d'un moteur (protocole du 17/10/2026, sur les 12 du
/// poste de référence).
pub const DEFAULT_THREADS: u32 = 4;

/// Marque du pid système dans le fichier `owner` écrit par [`BenchLock`].
const OS_PID: &str = "ospid";

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

    /// Attend que le verrou soit libre (relevé toutes les 15 s) puis le
    /// prend. `owner` est écrit dans le verrou pour que les autres sachent
    /// qui mesure.
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
                    writeln!(file, "{owner} {OS_PID} {}", std::process::id())?;
                    return Ok(lock);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if is_stale(&path) {
                        eprintln!("verrou {} orphelin, repris", path.display());
                        let _ = std::fs::remove_dir_all(&path);
                        continue;
                    }
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

/// Pid système du propriétaire, s'il a été écrit par [`BenchLock`].
fn owner_pid(owner: &str) -> Option<u32> {
    let rest = &owner[owner.find(OS_PID)? + OS_PID.len()..];
    let digits: String = rest
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// Vrai si le verrou a été pris par [`BenchLock`] et que son processus
/// n'existe plus, ou que son pid a été réattribué à un processus démarré
/// après la prise du verrou. Dans le doute (fichier `owner` absent,
/// illisible ou écrit par un autre outil), le verrou est considéré vivant.
fn is_stale(path: &Path) -> bool {
    let owner_file = path.join("owner");
    let (Ok(owner), Ok(meta)) = (
        std::fs::read_to_string(&owner_file),
        std::fs::metadata(&owner_file),
    ) else {
        return false;
    };
    let (Some(pid), Ok(taken)) = (owner_pid(&owner), meta.modified()) else {
        return false;
    };
    let taken = taken.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let pid = Pid::from_u32(pid);
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        false,
        ProcessRefreshKind::nothing(),
    );
    match sys.process(pid) {
        None => true,
        // Marge de 5 s : horloges et arrondis.
        Some(p) => p.start_time() > taken + 5,
    }
}

/// Limite les pools de threads des bibliothèques (rayon pour candle, OpenMP)
/// à `threads`, sauf valeur déjà fixée. À appeler avant tout calcul.
pub fn limit_threads(threads: u32) {
    for var in ["RAYON_NUM_THREADS", "OMP_NUM_THREADS"] {
        if std::env::var_os(var).is_none() {
            std::env::set_var(var, threads.to_string());
        }
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

    fn lock_with_owner(name: &str, owner: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("kobi-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("owner"), owner).unwrap();
        path
    }

    #[test]
    fn verrou_exclusif_et_libere() {
        let dir = std::env::temp_dir().join(format!("kobi-lock-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join(".bench.lock");
        {
            let _lock = BenchLock::acquire(&path, "test").unwrap();
            let owner = std::fs::read_to_string(path.join("owner")).unwrap();
            assert!(owner.starts_with("test ospid "), "{owner}");
            // Pris : une seconde création échoue.
            assert!(std::fs::create_dir(&path).is_err());
        }
        assert!(!path.exists(), "verrou libéré à la destruction");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ne_lit_que_le_pid_systeme_de_bench_lock() {
        assert_eq!(owner_pid("KOB-18 stt ospid 42"), Some(42));
        // Autres outils : pid peut-être MSYS, jamais interprété.
        assert_eq!(owner_pid("KOB-20 nemotron (pid 2896) depuis 2026"), None);
        assert_eq!(owner_pid("inconnu"), None);
    }

    #[test]
    fn reprend_un_verrou_dont_le_proprietaire_est_mort() {
        // Pid improbable : aucun processus ne le porte.
        let path = lock_with_owner("stale", "KOB-0 mort ospid 4000000000");
        assert!(is_stale(&path));
        let _lock = BenchLock::acquire(&path, "KOB-0 repreneur").unwrap();
        let owner = std::fs::read_to_string(path.join("owner")).unwrap();
        assert!(owner.starts_with("KOB-0 repreneur"));
    }

    #[test]
    fn garde_les_verrous_vivants_et_etrangers() {
        let alive = lock_with_owner(
            "alive",
            &format!("KOB-0 vivant ospid {}", std::process::id()),
        );
        assert!(!is_stale(&alive));
        let foreign = lock_with_owner("foreign", "KOB-20 nemotron (pid 4000000000)");
        assert!(!is_stale(&foreign));
        for p in [alive, foreign] {
            std::fs::remove_dir_all(p).unwrap();
        }
    }
}
