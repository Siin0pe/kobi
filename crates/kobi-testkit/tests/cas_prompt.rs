//! Le harnais des cas du prompt système fonctionne de bout en bout, joué
//! contre le faux `claude` en mode écho (qui ne sait évidemment pas les
//! réussir) : 10 cas joués, aucun tour en erreur, rapport écrit.

use std::process::Command;

#[test]
fn harnais_des_dix_cas_contre_le_faux_claude() {
    let out = std::env::temp_dir().join(format!("kobi-cas-prompt-{}.json", std::process::id()));
    let status = Command::new(env!("CARGO_BIN_EXE_cas-prompt"))
        .args(["--program", env!("CARGO_BIN_EXE_fake-claude"), "--out"])
        .arg(&out)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&status.stdout);
    assert_eq!(
        status.status.code(),
        Some(1),
        "l'écho ne passe pas les cas :\n{stdout}"
    );
    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    let _ = std::fs::remove_file(&out);
    let cases = report["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 10);
    for case in cases {
        assert!(
            case["outcome"]["error"].is_null(),
            "harnais en erreur : {case}"
        );
        assert!(
            case["outcome"]["text"]
                .as_str()
                .unwrap()
                .starts_with("reçu : ["),
            "le message part bien dans la session : {case}"
        );
    }
    // Recopier le message ne suffit à passer aucun cas.
    assert!(stdout.contains("10 cas, 10 échec(s)"), "{stdout}");
}
