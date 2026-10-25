//! Règles de mesure communes à tous les benchmarks du poste :
//!
//! - **une mesure à la fois** : verrou `~/Kobi-bench/.bench.lock`.
//!   - Sous Unix, c'est un `flock` exclusif sur ce fichier, le même que
//!     `flock ~/Kobi-bench/.bench.lock …` (train, agents) ; le détenteur est
//!     écrit à côté, dans `.bench.lock.owner`. Le fichier n'est jamais
//!     supprimé, et le noyau relâche le verrou d'un processus mort (KOB-312).
//!   - Sous Windows, c'est un dossier créé de façon atomique (avec un fichier
//!     `owner`) et supprimé à la fin, comme les autres outils du poste. Un
//!     verrou pris par [`BenchLock`] dont le processus est mort est repris ;
//!     ceux des autres outils ne le sont jamais, leur pid pouvant être celui
//!     de MSYS et non celui du système.
//!
//!   Sur Ctrl-C ou SIGTERM, [`BenchLock::release_on_interrupt`] le libère
//!   avant de quitter ;
//! - **CPU partagé** : priorité « en dessous de la normale » ; le
//!   nombre de threads des moteurs est fixé par `kobi-bench stt --threads`
//!   ([`DEFAULT_THREADS`] par défaut), reporté sur rayon et OpenMP par
//!   [`limit_threads`].

use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::time::{Duration, UNIX_EPOCH};

#[cfg(windows)]
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// Threads par défaut d'un moteur (protocole du 17/10/2026, sur les 12 du
/// poste de référence).
pub const DEFAULT_THREADS: u32 = 4;

/// Marque du pid système dans le fichier `owner` écrit par [`BenchLock`].
const OS_PID: &str = "ospid";

/// Verrou de mesure ; libéré à la destruction.
#[derive(Debug)]
pub struct BenchLock {
    path: PathBuf,
    /// Le fichier dont le `flock` est tenu, jusqu'à la destruction.
    #[cfg(unix)]
    _file: File,
}

impl BenchLock {
    /// Emplacement commun : `~/Kobi-bench/.bench.lock`, ou `KOBI_BENCH_LOCK`
    /// (tests).
    pub fn default_path(home: &Path) -> PathBuf {
        std::env::var_os("KOBI_BENCH_LOCK")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("Kobi-bench").join(".bench.lock"))
    }

    /// Attend que le verrou soit libre puis le prend. `owner` est écrit
    /// à côté du fichier verrou pour que les autres sachent qui mesure.
    #[cfg(unix)]
    pub fn acquire(path: impl Into<PathBuf>, owner: &str) -> std::io::Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if path.is_dir() {
            return Err(std::io::Error::other(format!(
                "{} est un dossier, ancien verrou de kobi-bench : le supprimer s'il n'est \
                 plus utilisé (le verrou est désormais un flock sur ce fichier)",
                path.display()
            )));
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        match file.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                let who = std::fs::read_to_string(owner_path(&path))
                    .unwrap_or_else(|_| "flock d'un autre outil".into());
                eprintln!("verrou {} pris ({}), attente…", path.display(), who.trim());
                file.lock()?;
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(e),
        }
        let mut detenteur = File::create(owner_path(&path))?;
        writeln!(detenteur, "{owner} {OS_PID} {}", std::process::id())?;
        Ok(Self { path, _file: file })
    }

    /// Attend que le verrou soit libre (relevé toutes les 15 s) puis le
    /// prend. `owner` est écrit dans le verrou pour que les autres sachent
    /// qui mesure.
    #[cfg(windows)]
    pub fn acquire(path: impl Into<PathBuf>, owner: &str) -> std::io::Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut announced = false;
        loop {
            match std::fs::create_dir(&path) {
                Ok(()) => {
                    let lock = Self { path };
                    let mut file = File::create(owner_path(&lock.path))?;
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
                        let who = std::fs::read_to_string(owner_path(&path)).unwrap_or_default();
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

/// Fichier du détenteur : `<verrou>.owner` à côté du fichier verrou sous
/// Unix, `<verrou>/owner` dans le dossier verrou sous Windows.
pub fn owner_path(lock: &Path) -> PathBuf {
    #[cfg(unix)]
    {
        let mut nom = lock.as_os_str().to_owned();
        nom.push(".owner");
        PathBuf::from(nom)
    }
    #[cfg(windows)]
    {
        lock.join("owner")
    }
}

/// Vrai si personne ne tient le verrou `path` (tests, diagnostic). Sous
/// Unix, le fichier reste après la libération : seul le `flock` compte.
pub fn is_free(path: &Path) -> bool {
    #[cfg(unix)]
    {
        match File::open(path) {
            Ok(file) => file.try_lock().is_ok(),
            Err(e) => e.kind() == std::io::ErrorKind::NotFound,
        }
    }
    #[cfg(windows)]
    {
        !path.exists()
    }
}

/// Libère le verrou de ce processus : le détenteur est effacé s'il est
/// toujours le sien ; sous Unix, le `flock` tombe avec le fichier fermé (ou
/// à la mort du processus), le fichier verrou reste ; sous Windows, le
/// dossier est supprimé.
fn release(path: &Path, pid: u32) {
    if owned_by(path, pid) {
        #[cfg(unix)]
        let _ = std::fs::remove_file(owner_path(path));
        #[cfg(windows)]
        let _ = std::fs::remove_dir_all(path);
    }
}

impl BenchLock {
    /// Sur Ctrl-C (et SIGTERM, SIGHUP sous Unix), supprime le verrou s'il est
    /// toujours à ce processus, puis quitte avec le code 130 : `Drop` ne
    /// s'exécute pas quand le processus est interrompu. Un seul gestionnaire
    /// par processus (limite de `ctrlc`).
    pub fn release_on_interrupt(&self) -> Result<(), ctrlc::Error> {
        let path = self.path.clone();
        let pid = std::process::id();
        ctrlc::set_handler(move || {
            release(&path, pid);
            eprintln!("interrompu : verrou {} libéré", path.display());
            std::process::exit(130);
        })
    }
}

/// Vrai si le verrou `path` porte le pid système `pid`.
fn owned_by(path: &Path, pid: u32) -> bool {
    std::fs::read_to_string(owner_path(path))
        .ok()
        .and_then(|owner| owner_pid(&owner))
        == Some(pid)
}

impl Drop for BenchLock {
    fn drop(&mut self) {
        // Avant la fermeture du fichier (champ détruit après) : personne
        // d'autre ne peut encore avoir écrit son nom.
        release(&self.path, std::process::id());
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
/// Sous Unix, inutile : le noyau relâche le `flock` d'un processus mort.
#[cfg(windows)]
fn is_stale(path: &Path) -> bool {
    let owner_file = owner_path(path);
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

    /// Dossier de test propre, hors du vrai verrou du poste.
    fn dossier(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kobi-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Un détenteur écrit à la main, comme le ferait un autre [`BenchLock`].
    fn lock_with_owner(name: &str, owner: &str) -> PathBuf {
        let path = dossier(name).join(".bench.lock");
        #[cfg(windows)]
        std::fs::create_dir(&path).unwrap();
        std::fs::write(owner_path(&path), owner).unwrap();
        path
    }

    #[test]
    fn verrou_exclusif_et_libere() {
        let dir = dossier("lock-test");
        let path = dir.join(".bench.lock");
        {
            let _lock = BenchLock::acquire(&path, "test").unwrap();
            let owner = std::fs::read_to_string(owner_path(&path)).unwrap();
            assert!(owner.starts_with("test ospid "), "{owner}");
            assert!(!is_free(&path), "pris");
        }
        assert!(is_free(&path), "verrou libéré à la destruction");
        assert!(!owner_path(&path).exists(), "détenteur effacé");
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
    fn interruption_ne_libere_que_son_propre_verrou() {
        let path = lock_with_owner("mien", "KOB-128 stt ospid 77 77");
        assert!(owned_by(&path, 77));
        assert!(!owned_by(&path, 78));
        let other = lock_with_owner("autre", "kyutai ospid 1234");
        assert!(!owned_by(&other, 77));
        assert!(!owned_by(&path.join("absent"), 77));
        // Le détenteur d'un autre n'est pas effacé.
        release(&other, 77);
        assert!(owner_path(&other).exists());
    }

    /// KOB-312 : le verrou est le `flock` commun. Un `flock` tenu ailleurs
    /// (autre ouverture du fichier, comme `flock(1)` du train) fait attendre
    /// [`BenchLock`], et son relâchement le laisse passer ; un détenteur
    /// laissé par un processus tué ne bloque rien.
    #[cfg(unix)]
    #[test]
    fn attend_le_flock_commun_puis_passe() {
        use std::sync::mpsc;
        use std::time::Duration;
        let dir = dossier("flock");
        let path = dir.join(".bench.lock");
        std::fs::write(owner_path(&path), "KOB-0 tué ospid 4000000000").unwrap();
        let train = File::create(&path).unwrap();
        train.lock().unwrap();
        let (pris, recu) = mpsc::channel();
        let chemin = path.clone();
        let attente = std::thread::spawn(move || {
            let lock = BenchLock::acquire(&chemin, "KOB-312 test").unwrap();
            pris.send(()).unwrap();
            lock
        });
        assert!(
            recu.recv_timeout(Duration::from_millis(500)).is_err(),
            "pris malgré le flock tenu"
        );
        train.unlock().unwrap();
        recu.recv_timeout(Duration::from_secs(5))
            .expect("toujours en attente après le relâchement");
        let lock = attente.join().unwrap();
        // Tenu à son tour : un autre `flock` ne passe pas.
        assert!(File::open(&path).unwrap().try_lock().is_err());
        let owner = std::fs::read_to_string(owner_path(&path)).unwrap();
        assert!(owner.starts_with("KOB-312 test ospid "), "{owner}");
        drop(lock);
        assert!(path.is_file(), "le fichier du flock commun reste");
        assert!(is_free(&path));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// KOB-312 : le même verrou que `flock(1)`, d'un autre processus.
    #[cfg(unix)]
    #[test]
    fn attend_flock_d_un_autre_processus() {
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};
        let dir = dossier("flock-cmd");
        let path = dir.join(".bench.lock");
        let pret = dir.join("pret");
        // `flock` absent : rien à vérifier ici (le test précédent couvre
        // le même verrou par une autre ouverture).
        let Ok(mut train) = Command::new("flock")
            .arg(&path)
            .args(["sh", "-c"])
            .arg(format!("touch '{}'; sleep 1", pret.display()))
            .stdin(Stdio::null())
            .spawn()
        else {
            return;
        };
        let debut = Instant::now();
        while !pret.exists() {
            assert!(
                debut.elapsed() < Duration::from_secs(10),
                "flock jamais pris"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let attente = Instant::now();
        let lock = BenchLock::acquire(&path, "KOB-312 test").unwrap();
        // Passé seulement après la seconde où `flock` le tient.
        assert!(
            attente.elapsed() >= Duration::from_millis(500),
            "pris pendant le flock ({:?})",
            attente.elapsed()
        );
        drop(lock);
        train.wait().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// L'ancien verrou (un dossier) n'est plus attendu sans fin : erreur claire.
    #[cfg(unix)]
    #[test]
    fn ancien_verrou_dossier_refuse() {
        let dir = dossier("ancien");
        let path = dir.join(".bench.lock");
        std::fs::create_dir(&path).unwrap();
        let e = BenchLock::acquire(&path, "test").unwrap_err();
        assert!(e.to_string().contains("ancien verrou"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn reprend_un_verrou_dont_le_proprietaire_est_mort() {
        // Pid improbable : aucun processus ne le porte.
        let path = lock_with_owner("stale", "KOB-0 mort ospid 4000000000");
        assert!(is_stale(&path));
        let _lock = BenchLock::acquire(&path, "KOB-0 repreneur").unwrap();
        let owner = std::fs::read_to_string(owner_path(&path)).unwrap();
        assert!(owner.starts_with("KOB-0 repreneur"));
    }

    #[cfg(windows)]
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
