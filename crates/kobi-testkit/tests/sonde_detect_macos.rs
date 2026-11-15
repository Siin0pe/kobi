//! Sonde temporaire KOB-519 (PR fermée sans merge) : durée du premier et du second
//! `--version` d'un faux `claude` fraîchement copié, sous macOS. Rapport par échec
//! voulu.
#![cfg(target_os = "macos")]
use std::process::Command;
use std::time::{Duration, Instant};

use kobi_brain::claude_code::detect::probe;
use kobi_brain::Detection;

const FAKE_CLAUDE: &str = env!("CARGO_BIN_EXE_fake-claude");

fn copie(cas: &str, mode: Option<&str>) -> (kobi_testkit::DossierTemp, std::path::PathBuf) {
    let dir = kobi_testkit::DossierTemp::nouveau("sonde519", cas);
    let exe = dir.join("claude");
    std::fs::copy(FAKE_CLAUDE, &exe).unwrap();
    if let Some(mode) = mode {
        std::fs::write(exe.with_extension("mode"), mode).unwrap();
    }
    (dir, exe)
}

fn duree(exe: &std::path::Path) -> Duration {
    let t = Instant::now();
    Command::new(exe).arg("--version").output().unwrap();
    t.elapsed()
}

#[tokio::test]
async fn sonde_premier_lancement_macos() {
    let mut r = String::new();
    for k in 0..10 {
        let (_d, exe) = copie(&format!("froid-{k}"), None);
        let premier = duree(&exe);
        let second = duree(&exe);
        r.push_str(&format!(
            "copie {k} : premier {premier:?}, second {second:?}\n"
        ));
    }
    let mut ok = 0;
    for k in 0..10 {
        let (_d, exe) = copie(&format!("chaud-{k}"), Some("auth-hang"));
        duree(&exe);
        let t = Instant::now();
        let d = probe(&exe, Duration::from_millis(500)).await;
        let bon = matches!(
            &d,
            Detection::Ready {
                auth_method: None,
                ..
            }
        );
        ok += usize::from(bon);
        r.push_str(&format!("réchauffé {k} : {:?} en {:?}\n", bon, t.elapsed()));
    }
    panic!("RAPPORT DE SONDE (échec voulu) ; réchauffé bon {ok}/10\n{r}");
}
