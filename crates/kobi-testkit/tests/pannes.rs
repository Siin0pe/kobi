//! Pannes du cerveau (KOB-68) contre le faux `claude` : chaque panne d'un
//! vrai process donne l'erreur que l'acteur de conversation de kobi-app sait
//! traiter (plantage → relance, limite d'usage → Oups, réseau → attente,
//! lenteur → patience puis abandon).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use kobi_brain::{Ask, Brain, BrainError, BrainEvent, ClaudeCodeBrain, MeetingConfig};

const FAKE_CLAUDE: &str = env!("CARGO_BIN_EXE_fake-claude");

/// Dossier propre au test, avec sa copie du faux `claude` : les fichiers
/// témoins (`.plante`, `.hors-ligne`) vivent à côté de l'exécutable.
fn faux_claude(test: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("kobi-pannes-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let exe = dir.join("claude");
    std::fs::copy(FAKE_CLAUDE, &exe).unwrap();
    (dir, exe)
}

fn config(exe: &Path, dir: &Path) -> MeetingConfig {
    MeetingConfig {
        program: exe.to_owned(),
        cwd: Some(dir.to_owned()),
        ..Default::default()
    }
}

/// Démarre la session. Les tests tournent en parallèle : un autre test qui
/// lance un process pendant qu'on copie l'exécutable peut garder un instant
/// son descripteur ouvert (« Text file busy ») ; on réessaie alors.
async fn demarrer(exe: &Path, dir: &Path) -> ClaudeCodeBrain {
    for _ in 0..20 {
        match ClaudeCodeBrain::start(config(exe, dir)).await {
            Err(BrainError::Unavailable(m)) if m.contains("Text file busy") => {
                tokio::time::sleep(Duration::from_millis(50)).await
            }
            r => return r.unwrap(),
        }
    }
    panic!("{} reste occupé", exe.display());
}

fn ask(question: &str) -> Ask {
    Ask {
        question: question.into(),
        ..Default::default()
    }
}

/// Réponse du tour, ou son erreur.
async fn tour(brain: &mut ClaudeCodeBrain, question: &str) -> Result<String, BrainError> {
    let mut turn = brain.ask(ask(question));
    let mut texte = String::new();
    while let Some(e) = turn.next().await {
        match e {
            BrainEvent::Text(t) => texte.push_str(&t),
            BrainEvent::Done(fin) => return Ok(if fin.text.is_empty() { texte } else { fin.text }),
            BrainEvent::Error(e) => return Err(e),
            _ => {}
        }
    }
    Err(BrainError::Crashed("flux fermé".into()))
}

#[tokio::test]
async fn plantage_puis_relance_qui_repond() {
    let (dir, exe) = faux_claude("plantage");
    let mut brain = demarrer(&exe, &dir).await;
    let e = tour(&mut brain, "Qu'en dis-tu ? #plante")
        .await
        .unwrap_err();
    assert!(matches!(e, BrainError::Crashed(_)), "{e:?}");
    // Session relancée, même question : elle passe.
    let mut relance = demarrer(&exe, &dir).await;
    let reponse = tour(&mut relance, "Qu'en dis-tu ? #plante").await.unwrap();
    assert!(reponse.contains("Qu'en dis-tu"), "{reponse}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn limite_d_usage_en_erreur_d_api() {
    let (dir, exe) = faux_claude("limite");
    let mut brain = demarrer(&exe, &dir).await;
    let e = tour(&mut brain, "Résume #limite").await.unwrap_err();
    assert!(
        matches!(&e, BrainError::Api(m) if m.contains("usage limit reached")),
        "{e:?}"
    );
    // La session reste utilisable.
    assert!(tour(&mut brain, "Et maintenant ?").await.is_ok());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn reseau_coupe_puis_revenu() {
    let (dir, exe) = faux_claude("reseau");
    std::fs::write(exe.with_extension("hors-ligne"), b"").unwrap();
    let mut brain = demarrer(&exe, &dir).await;
    let e = tour(&mut brain, "Une question #reseau").await.unwrap_err();
    assert!(
        matches!(&e, BrainError::Api(m) if m.contains("Connection error")),
        "{e:?}"
    );
    std::fs::remove_file(exe.with_extension("hors-ligne")).unwrap();
    let reponse = tour(&mut brain, "Une question #reseau").await.unwrap();
    assert!(reponse.contains("Une question"), "{reponse}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn lenteur_rien_avant_la_reponse() {
    let (dir, exe) = faux_claude("lenteur");
    std::env::set_var("KOBI_FAUX_LENT", "3");
    let mut brain = demarrer(&exe, &dir).await;
    let debut = Instant::now();
    let mut turn = brain.ask(ask("Cherche #lent"));
    // Aucun événement avant la réponse : c'est ce silence que l'acteur
    // de conversation surveille (patience à 8 s, abandon à 45 s).
    let premier = turn.next().await.unwrap();
    assert!(
        debut.elapsed() >= Duration::from_secs(3),
        "{:?}",
        debut.elapsed()
    );
    assert!(matches!(premier, BrainEvent::Text(_)), "{premier:?}");
    while turn.next().await.is_some() {}
    let _ = std::fs::remove_dir_all(&dir);
}
