//! Détection de Claude Code contre le faux `claude` : présent, absent,
//! qui échoue, qui ne répond pas, pas connecté.

use std::path::{Path, PathBuf};
use std::time::Duration;

use kobi_brain::claude_code::detect::{find_in, probe, PROBE_TIMEOUT};
use kobi_brain::Detection;

const FAKE_CLAUDE: &str = env!("CARGO_BIN_EXE_fake-claude");

/// Les tests copient un exécutable puis le lancent : si un autre test fait
/// un fork pendant la copie, le fichier encore ouvert en écriture ne peut
/// pas être exécuté (`ETXTBSY` sous Linux). Ils passent donc un par un.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Copie le faux `claude` dans un dossier propre au test, sous le nom du
/// vrai, avec le mode demandé.
fn install(test: &str, mode: Option<&str>) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("kobi-detect-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let exe = dir.join(if cfg!(windows) {
        "claude.exe"
    } else {
        "claude"
    });
    std::fs::copy(FAKE_CLAUDE, &exe).unwrap();
    if let Some(mode) = mode {
        std::fs::write(exe.with_extension("mode"), mode).unwrap();
    }
    (dir, exe)
}

fn path_of(dirs: &[&Path]) -> std::ffi::OsString {
    std::env::join_paths(dirs).unwrap()
}

#[tokio::test]
async fn present_et_connecte() {
    let _serial = SERIAL.lock().await;
    let (dir, exe) = install("present", None);
    let found = find_in(Some(&path_of(&[&dir])), &[]);
    assert_eq!(found, std::slice::from_ref(&exe));
    let d = probe(&found[0], PROBE_TIMEOUT).await;
    let Detection::Ready {
        install,
        auth_method,
    } = &d
    else {
        panic!("attendu Ready, obtenu {d:?}")
    };
    assert_eq!(install.program, exe);
    assert_eq!(install.version, "2.1.302 (Claude Code)");
    assert_eq!(auth_method.as_deref(), Some("claude.ai"));
    assert!(d.advice().is_none());
}

#[tokio::test]
async fn absent() {
    let _serial = SERIAL.lock().await;
    let (dir, _) = install("absent-vide", None);
    std::fs::remove_dir_all(&dir).unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    assert!(find_in(Some(&path_of(&[&dir])), &[]).is_empty());
    let d = probe(&dir.join("claude"), PROBE_TIMEOUT).await;
    assert!(matches!(d, Detection::Broken { .. }), "{d:?}");
    assert!(d.advice().unwrap().contains("clé API"));
}

#[tokio::test]
async fn binaire_qui_echoue() {
    let _serial = SERIAL.lock().await;
    let (_, exe) = install("echoue", Some("broken"));
    let d = probe(&exe, PROBE_TIMEOUT).await;
    let Detection::Broken { program, reason } = &d else {
        panic!("attendu Broken, obtenu {d:?}")
    };
    assert_eq!(program, &exe);
    assert!(reason.contains("--version"), "{reason}");
    assert!(d.advice().unwrap().contains("clé API"));
}

#[tokio::test]
async fn binaire_qui_ne_repond_pas() {
    let _serial = SERIAL.lock().await;
    let (_, exe) = install("bloque", Some("hang"));
    let d = probe(&exe, Duration::from_millis(500)).await;
    assert!(
        matches!(&d, Detection::Broken { reason, .. } if reason.contains("pas de réponse")),
        "{d:?}"
    );
}

#[tokio::test]
async fn installe_mais_pas_connecte() {
    let _serial = SERIAL.lock().await;
    let (_, exe) = install("deconnecte", Some("logged-out"));
    let d = probe(&exe, PROBE_TIMEOUT).await;
    assert!(
        matches!(&d, Detection::NotLoggedIn(i) if i.program == exe),
        "{d:?}"
    );
    assert!(d.advice().unwrap().contains("claude auth login"));
}

#[tokio::test]
async fn ancienne_version_sans_auth_status() {
    let _serial = SERIAL.lock().await;
    let (_, exe) = install("ancienne", Some("old"));
    let d = probe(&exe, PROBE_TIMEOUT).await;
    assert!(
        matches!(
            &d,
            Detection::Ready {
                auth_method: None,
                ..
            }
        ),
        "{d:?}"
    );
}

#[tokio::test]
async fn le_premier_du_path_gagne() {
    let _serial = SERIAL.lock().await;
    let (a, exe_a) = install("ordre-a", None);
    let (b, _) = install("ordre-b", Some("broken"));
    assert_eq!(find_in(Some(&path_of(&[&a, &b])), &[])[0], exe_a);
}

#[tokio::test]
async fn auth_status_bloque_connexion_inconnue() {
    let _serial = SERIAL.lock().await;
    let (_, exe) = install("auth-bloque", Some("auth-hang"));
    let d = probe(&exe, Duration::from_millis(500)).await;
    assert!(
        matches!(
            &d,
            Detection::Ready {
                auth_method: None,
                ..
            }
        ),
        "{d:?}"
    );
}
