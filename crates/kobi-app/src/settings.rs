//! Réglages de l'interface : le skin choisi (clé `skin` de `~/Kobi/config.toml`,
//! lue et écrite par `kobi_session::config`) et la fenêtre des réglages.

use std::path::Path;

use kobi_session::config::{self, Config};
use tauri::{AppHandle, Emitter, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};

/// Skins proposés (identifiants de `ui/src/skins/registry.ts`, hors skin de test).
pub const SKINS: [&str; 6] = [
    "kobi",
    "gribouille",
    "mercure",
    "matrice",
    "gelee",
    "essaim",
];
pub const DEFAULT_SKIN: &str = "kobi";
/// Événement envoyé à toutes les fenêtres quand le skin change.
pub const SKIN_EVENT: &str = "kobi://skin";

/// Skin de la config, ou le skin par défaut s'il est inconnu.
pub fn skin_of(config: &Config) -> &'static str {
    SKINS
        .into_iter()
        .find(|s| *s == config.skin)
        .unwrap_or(DEFAULT_SKIN)
}

/// Enregistre `skin` dans la config `path`, sans toucher aux autres réglages.
/// Refuse un skin inconnu, et un fichier illisible (il n'est pas écrasé).
pub fn apply_skin(path: &Path, skin: &str) -> Result<(), String> {
    if !SKINS.contains(&skin) {
        return Err(format!("skin inconnu : {skin}"));
    }
    let mut loaded = config::load_or_create(path);
    if let Some(problem) = loaded.problem {
        return Err(format!("réglage non enregistré : {problem}"));
    }
    loaded.config.skin = skin.to_owned();
    config::save(&loaded.config, path).map_err(|e| e.to_string())
}

fn config_path() -> Result<std::path::PathBuf, String> {
    config::default_path().ok_or_else(|| "dossier personnel introuvable".to_owned())
}

#[tauri::command]
pub fn get_skin() -> Result<String, String> {
    let loaded = config::load_or_create(&config_path()?);
    Ok(skin_of(&loaded.config).to_owned())
}

#[tauri::command]
pub fn set_skin<R: Runtime>(app: AppHandle<R>, skin: String) -> Result<(), String> {
    apply_skin(&config_path()?, &skin)?;
    tracing::info!(skin = %skin, "réglages : skin");
    app.emit(SKIN_EVENT, &skin).map_err(|e| e.to_string())
}

/// Ouvre (ou ramène au premier plan) la fenêtre des réglages.
pub fn open<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window("reglages") {
        window.show()?;
        return window.set_focus();
    }
    WebviewWindowBuilder::new(app, "reglages", WebviewUrl::App("reglages.html".into()))
        .title("Kobi · Réglages")
        .inner_size(1000.0, 680.0)
        .min_inner_size(560.0, 480.0)
        .build()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_config(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("kobi-settings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("config.toml")
    }

    #[test]
    fn skin_par_defaut_si_inconnu() {
        let mut c = Config::default();
        assert_eq!(skin_of(&c), "kobi");
        c.skin = "licorne".into();
        assert_eq!(skin_of(&c), "kobi");
        c.skin = "matrice".into();
        assert_eq!(skin_of(&c), "matrice");
    }

    #[test]
    fn enregistrer_garde_les_autres_reglages() {
        let path = temp_config("garde");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "brain = \"api\"\n").unwrap();
        apply_skin(&path, "essaim").unwrap();
        let loaded = config::load_or_create(&path);
        assert!(loaded.problem.is_none());
        assert_eq!(loaded.config.skin, "essaim");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("brain = \"api\""), "{text}");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn skin_inconnu_refuse() {
        let path = temp_config("inconnu");
        assert!(apply_skin(&path, "licorne").is_err());
        assert!(apply_skin(&path, "factice").is_err());
        assert!(!path.exists());
    }

    #[test]
    fn fichier_illisible_jamais_ecrase() {
        let path = temp_config("illisible");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "skin = = =").unwrap();
        assert!(apply_skin(&path, "gelee").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "skin = = =");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
