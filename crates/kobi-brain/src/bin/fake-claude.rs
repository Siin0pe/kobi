//! Faux `claude -p --input-format stream-json --output-format stream-json`
//! pour les tests : répond « reçu : <message> » en deux deltas par message.
//! Le message `crash` arrête le process en erreur, `erreur` donne un
//! `result` avec `is_error`.

use std::io::{BufRead, Write};

use serde_json::{json, Value};

fn main() {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    let mut emit = |v: Value| {
        writeln!(out, "{v}").unwrap();
        out.flush().unwrap();
    };
    let session = "fake-session";
    for line in stdin.lock().lines() {
        let line = line.unwrap();
        let msg: Value = serde_json::from_str(&line).expect("ligne JSON");
        assert_eq!(msg["type"], "user");
        let content = msg["message"]["content"].as_str().unwrap_or_default();
        if content == "crash" {
            eprintln!("faux plantage");
            std::process::exit(3);
        }
        emit(json!({"type":"system","subtype":"init","session_id":session,"model":"fake"}));
        // Les hooks et événements inconnus doivent être tolérés.
        emit(json!({"type":"rate_limit_event","rate_limit_info":{}}));
        let answer = format!("reçu : {content}");
        for part in ["reçu : ", content] {
            emit(
                json!({"type":"stream_event","parent_tool_use_id":null,"session_id":session,
                "event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":part}}}),
            );
        }
        emit(json!({"type":"assistant","session_id":session,
            "message":{"role":"assistant","content":[{"type":"text","text":answer}]}}));
        let is_error = content == "erreur";
        emit(
            json!({"duration_api_ms":1,"session_id":session,"type":"result",
            "subtype": if is_error { "error_during_execution" } else { "success" },
            "is_error":is_error,"result":answer,"num_turns":1}),
        );
    }
}
