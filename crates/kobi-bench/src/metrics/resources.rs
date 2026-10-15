//! CPU et mémoire résidente du process, échantillonnés pendant une mesure.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

/// Consommation du process pendant la mesure.
///
/// Le CPU est exprimé en pourcentage d'un cœur logique (200 % = deux cœurs
/// pleins) ; `logical_cores` permet de le ramener à la machine.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ResourceStats {
    pub samples: usize,
    pub cpu_mean_pct: f64,
    pub cpu_peak_pct: f64,
    pub logical_cores: usize,
    /// Mémoire résidente au démarrage de la mesure, en Mo.
    pub rss_start_mb: f64,
    pub rss_mean_mb: f64,
    pub rss_peak_mb: f64,
}

/// Échantillonne le process courant dans un thread jusqu'à [`stop`].
///
/// [`stop`]: ResourceMonitor::stop
pub struct ResourceMonitor {
    running: Arc<AtomicBool>,
    handle: JoinHandle<ResourceStats>,
}

const MB: f64 = 1024.0 * 1024.0;

impl ResourceMonitor {
    /// Démarre l'échantillonnage toutes les `interval` (au moins
    /// [`sysinfo::MINIMUM_CPU_UPDATE_INTERVAL`] pour un CPU fiable).
    ///
    /// Rend la main une fois la mémoire de départ relevée : ce qui suit
    /// l'appel (chargement d'un modèle…) est bien compté.
    pub fn start(interval: Duration) -> Self {
        let interval = interval.max(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        let running = Arc::new(AtomicBool::new(true));
        let flag = Arc::clone(&running);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || sample_loop(&flag, interval, ready_tx));
        // Si le thread meurt avant, `recv` échoue et on continue sans attendre.
        let _ = ready_rx.recv();
        Self { running, handle }
    }

    /// Arrête l'échantillonnage et renvoie les statistiques.
    pub fn stop(self) -> ResourceStats {
        self.running.store(false, Ordering::Relaxed);
        self.handle.join().unwrap_or_default()
    }
}

fn sample_loop(
    running: &AtomicBool,
    interval: Duration,
    ready: std::sync::mpsc::Sender<()>,
) -> ResourceStats {
    let Ok(pid) = sysinfo::get_current_pid() else {
        return ResourceStats::default();
    };
    let mut sys = System::new();
    let kind = ProcessRefreshKind::nothing().with_cpu().with_memory();
    let refresh = |sys: &mut System| {
        sys.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), false, kind);
        sys.process(pid)
            .map(|p| (p.cpu_usage() as f64, p.memory() as f64 / MB))
    };

    let mut stats = ResourceStats {
        logical_cores: std::thread::available_parallelism().map_or(1, |n| n.get()),
        ..Default::default()
    };
    // Le premier relevé ne fait qu'initialiser le compteur CPU.
    if let Some((_, rss)) = refresh(&mut sys) {
        stats.rss_start_mb = rss;
        stats.rss_peak_mb = rss;
    }
    let _ = ready.send(());
    let (mut cpu_sum, mut rss_sum) = (0.0, 0.0);
    let mut last_tick = std::time::Instant::now();

    loop {
        // Attente par petits pas pour réagir vite à l'arrêt.
        while running.load(Ordering::Relaxed) && last_tick.elapsed() < interval {
            std::thread::sleep(Duration::from_millis(10));
        }
        let stopping = !running.load(Ordering::Relaxed);
        // Un dernier relevé à l'arrêt s'il couvre assez de temps, et toujours
        // au moins un relevé, même si l'arrêt arrive tôt ou que le thread a
        // été privé de CPU.
        if stopping
            && stats.samples > 0
            && last_tick.elapsed() < sysinfo::MINIMUM_CPU_UPDATE_INTERVAL
        {
            break;
        }
        last_tick = std::time::Instant::now();
        if let Some((cpu, rss)) = refresh(&mut sys) {
            stats.samples += 1;
            cpu_sum += cpu;
            rss_sum += rss;
            stats.cpu_peak_pct = stats.cpu_peak_pct.max(cpu);
            stats.rss_peak_mb = stats.rss_peak_mb.max(rss);
        }
        if stopping {
            break;
        }
    }

    if stats.samples > 0 {
        stats.cpu_mean_pct = cpu_sum / stats.samples as f64;
        stats.rss_mean_mb = rss_sum / stats.samples as f64;
    }
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn immediate_stop_still_gives_one_sample() {
        for _ in 0..5 {
            let stats = ResourceMonitor::start(Duration::from_millis(200)).stop();
            assert_eq!(stats.samples, 1, "{stats:?}");
            assert!(stats.rss_start_mb > 0.0 && stats.rss_peak_mb >= stats.rss_start_mb);
        }
    }

    #[test]
    fn measures_a_busy_thread_and_an_allocation() {
        let monitor = ResourceMonitor::start(Duration::from_millis(200));
        let started = Instant::now();
        let mut buf: Vec<u8> = Vec::new();
        let mut x = 0u64;
        while started.elapsed() < Duration::from_millis(1200) {
            if buf.is_empty() && started.elapsed() > Duration::from_millis(300) {
                // 64 Mo réellement écrits, après le relevé de départ, pour
                // compter dans la mémoire résidente.
                buf = vec![1u8; 64 << 20];
            }
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
        }
        std::hint::black_box((x, &buf));
        let stats = monitor.stop();
        drop(buf);

        // Sans seuil de CPU absolu : sur un poste chargé, le thread actif peut
        // être privé de cœur.
        assert!(stats.samples >= 1, "{stats:?}");
        assert!(stats.cpu_peak_pct > 0.0, "{stats:?}");
        assert!(stats.cpu_mean_pct >= 0.0 && stats.cpu_mean_pct <= stats.cpu_peak_pct);
        assert!(stats.rss_peak_mb >= stats.rss_start_mb + 32.0, "{stats:?}");
        assert!(stats.logical_cores >= 1);
    }
}
