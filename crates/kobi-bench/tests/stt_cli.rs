//! `kobi-bench stt --engine fake` sur un petit corpus généré.

use std::process::Command;

use kobi_bench::report::Report;
use kobi_stt::wav::write_wav_16k_mono;

#[test]
fn fake_engine_report_is_written_and_readable() {
    let root = std::env::temp_dir().join(format!("kobi-bench-cli-{}", std::process::id()));
    let meeting = root.join("corpus").join("fr");
    std::fs::create_dir_all(&meeting).unwrap();
    let silence = vec![0.0f32; 3 * 16_000];
    write_wav_16k_mono(meeting.join("moi.wav"), &silence).unwrap();
    write_wav_16k_mono(meeting.join("eux.wav"), &silence).unwrap();
    std::fs::write(
        meeting.join("reference.jsonl"),
        concat!(
            r#"{"start": 0.2, "end": 1.2, "source": "moi", "lang": "fr", "kobi": true, "text": "Kobi, il reste 3 tickets ?"}"#,
            "\n",
            r#"{"start": 1.5, "end": 2.6, "source": "eux", "lang": "fr", "kobi": false, "text": "Oui, trois."}"#,
            "\n"
        ),
    )
    .unwrap();
    let out = root.join("rapports");

    let status = Command::new(env!("CARGO_BIN_EXE_kobi-bench"))
        .args([
            "stt",
            "--engine",
            "fake",
            "--speed",
            "10",
            "--machine",
            "ci",
        ])
        .arg("--corpus")
        .arg(root.join("corpus"))
        .arg("--out")
        .arg(&out)
        // Verrou local au test : ne bloque pas les vraies mesures du poste.
        .env("KOBI_BENCH_LOCK", root.join(".bench.lock"))
        .status()
        .unwrap();
    assert!(status.success());

    let report = Report::load_or_default(&out.join("rapport.json")).unwrap();
    assert_eq!(report.rows.len(), 1);
    let row = &report.rows[0];
    assert_eq!((row.engine.as_str(), row.meeting.as_str()), ("fake", "fr"));
    assert_eq!(row.threads, Some(kobi_bench::protocol::DEFAULT_THREADS));
    assert!(
        kobi_bench::protocol::is_free(&root.join(".bench.lock")),
        "verrou libéré"
    );
    assert_eq!(row.metrics.wer, 0.0);
    assert!(row.metrics.delays.as_ref().unwrap().first_word.is_some());

    let md = std::fs::read_to_string(out.join("rapport.md")).unwrap();
    assert!(md.contains("| fake | fr | ci | cpu | 4 | 0,0 % |"), "{md}");
    assert!(md.contains("| Rust "), "{md}");
    std::fs::remove_dir_all(&root).ok();
}

/// Ctrl-C pendant une mesure : le verrou disparaît et le code de sortie est 130.
#[cfg(unix)]
#[test]
fn interruption_libere_le_verrou() {
    use std::time::{Duration, Instant};

    let root = std::env::temp_dir().join(format!("kobi-bench-int-{}", std::process::id()));
    let meeting = root.join("corpus").join("fr");
    std::fs::create_dir_all(&meeting).unwrap();
    // 60 s rejouées en temps réel : la mesure dure bien plus que le test.
    let silence = vec![0.0f32; 60 * 16_000];
    write_wav_16k_mono(meeting.join("moi.wav"), &silence).unwrap();
    write_wav_16k_mono(meeting.join("eux.wav"), &silence).unwrap();
    std::fs::write(
        meeting.join("reference.jsonl"),
        r#"{"start": 0.2, "end": 1.2, "source": "moi", "lang": "fr", "kobi": false, "text": "Bonjour."}"#,
    )
    .unwrap();
    let lock = root.join(".bench.lock");

    let mut child = Command::new(env!("CARGO_BIN_EXE_kobi-bench"))
        .args(["stt", "--engine", "fake", "--speed", "1"])
        .arg("--corpus")
        .arg(root.join("corpus"))
        .arg("--out")
        .arg(root.join("rapports"))
        .env("KOBI_BENCH_LOCK", &lock)
        .spawn()
        .unwrap();
    let start = Instant::now();
    let owner = kobi_bench::protocol::owner_path(&lock);
    while !owner.exists() {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "verrou jamais pris"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    // Laisse le gestionnaire s'installer et la relecture démarrer.
    std::thread::sleep(Duration::from_millis(500));
    let kill = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(kill.success());
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(130), "{status:?}");
    assert!(
        kobi_bench::protocol::is_free(&lock),
        "verrou libéré après Ctrl-C"
    );
    assert!(!owner.exists(), "détenteur effacé après Ctrl-C");
    std::fs::remove_dir_all(&root).ok();
}
