//! Langue de l'interface (KOB-207) : `ui.language` de la config, la langue
//! du système quand elle est absente. Changée depuis une fenêtre (Réglages,
//! premier lancement) ou par `kobi config set ui.language en`, elle est
//! annoncée à toutes les fenêtres, qui se traduisent à chaud.

use std::path::Path;

use kobi_session::config::{self, Langue};
use tauri::{AppHandle, Emitter, Runtime};

/// Événement de changement de langue : le code (`fr`, `en`), ou `null` pour
/// la langue du système.
pub const EVENEMENT_LANGUE: &str = "langue";

/// Langue réglée dans `config.toml`, `None` pour celle du système.
fn lue(chemin: &Path) -> Option<Langue> {
    config::load_or_create(chemin).config.ui.language
}

/// Écrit la langue dans `config.toml`, sans jamais réécrire une config
/// illisible (les autres réglages y seraient perdus).
fn ecrire(chemin: &Path, langue: Option<Langue>) -> Result<(), String> {
    let mut lu = config::load_or_create(chemin);
    if let Some(probleme) = lu.problem {
        return Err(kobi_i18n::tr(
            "coque.langue.configIllisible",
            &[("probleme", &probleme.to_string())],
        ));
    }
    lu.config.ui.language = langue;
    config::save(&lu.config, chemin).map_err(|e| e.to_string())
}

/// Règle la langue de la coque et l'annonce à toutes les fenêtres.
pub fn annoncer<R: Runtime>(app: &AppHandle<R>, langue: Option<Langue>) {
    let retenue = kobi_i18n::regler(langue);
    tracing::info!(
        langue = langue.map(Langue::code),
        retenue = retenue.code(),
        "langue de l'interface"
    );
    crate::tray::retraduire(app);
    let _ = app.emit(EVENEMENT_LANGUE, langue.map(Langue::code));
}

/// La langue réglée (`null` : celle du système, que la fenêtre détecte).
#[tauri::command]
pub fn langue_ui() -> Option<&'static str> {
    config::default_path()
        .and_then(|p| lue(&p))
        .map(Langue::code)
}

/// Choix de la langue dans une fenêtre (`null` : celle du système) :
/// enregistrée, puis annoncée à toutes. Rend la langue retenue.
#[tauri::command]
pub fn regler_langue_ui<R: Runtime>(
    app: AppHandle<R>,
    langue: Option<String>,
) -> Result<Option<&'static str>, String> {
    let l = match langue.as_deref() {
        None => None,
        Some(code) => Some(
            Langue::depuis_code(code)
                .ok_or_else(|| kobi_i18n::tr("coque.langue.inconnue", &[("code", code)]))?,
        ),
    };
    let chemin = config::default_path()
        .ok_or_else(|| kobi_i18n::tr("coque.commun.dossierPersoIntrouvable", &[]))?;
    ecrire(&chemin, l)?;
    annoncer(&app, l);
    Ok(l.map(Langue::code))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn langue_ecrite_puis_relue_config_illisible_intacte() {
        let dir = tempfile::tempdir().unwrap();
        let chemin = dir.path().join("config.toml");
        std::fs::write(&chemin, "hotkey = \"Alt+K\"\n").unwrap();
        assert_eq!(lue(&chemin), None, "absente : celle du système");
        ecrire(&chemin, Some(Langue::En)).unwrap();
        assert_eq!(lue(&chemin), Some(Langue::En));
        let relu = config::load_or_create(&chemin).config;
        assert_eq!(relu.hotkey, "Alt+K", "le reste de la config est gardé");
        let illisible = "hotkey = \"Alt+K\"\n[ui\nlanguage = \"fr\"\n";
        std::fs::write(&chemin, illisible).unwrap();
        assert!(ecrire(&chemin, Some(Langue::Fr)).is_err());
        assert_eq!(std::fs::read_to_string(&chemin).unwrap(), illisible);
    }
}
