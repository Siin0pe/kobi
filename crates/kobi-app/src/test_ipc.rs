//! Outils des tests IPC : appeler une commande comme le ferait la webview.

use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{get_ipc_response, MockRuntime, INVOKE_KEY};
use tauri::webview::InvokeRequest;
use tauri::WebviewWindow;

pub fn requete(cmd: &str, corps: serde_json::Value) -> InvokeRequest {
    InvokeRequest {
        cmd: cmd.into(),
        callback: CallbackFn(0),
        error: CallbackFn(1),
        url: if cfg!(any(windows, target_os = "android")) {
            "http://tauri.localhost"
        } else {
            "tauri://localhost"
        }
        .parse()
        .unwrap(),
        body: InvokeBody::Json(corps),
        headers: Default::default(),
        invoke_key: INVOKE_KEY.to_string(),
    }
}

/// Appelle `cmd` depuis `webview` ; la réponse ou le refus, en JSON.
pub fn appeler(
    webview: &WebviewWindow<MockRuntime>,
    cmd: &str,
    corps: serde_json::Value,
) -> Result<serde_json::Value, serde_json::Value> {
    get_ipc_response(webview, requete(cmd, corps))
        .map(|b| b.deserialize::<serde_json::Value>().unwrap())
}
