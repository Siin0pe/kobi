//! Réglages : section de l'app Kobi (KOB-159), faite des panneaux de
//! `ui/src/reglages/panneaux.ts` (stockage, dossiers de code, micro,
//! démarrage…). `ouvrir_reglages` reste pour le menu et la barre système.

use tauri::{AppHandle, Runtime};

/// Identifiant de panneau sûr dans une URL : lettres, chiffres et tirets.
fn panneau_valide(panneau: &str) -> bool {
    !panneau.is_empty()
        && panneau.len() <= 40
        && panneau
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Ouvre la section Réglages de l'app (KOB-159), sur `panneau` s'il est
/// donné et sûr.
pub fn ouvrir<R: Runtime>(app: &AppHandle<R>, panneau: Option<&str>) -> tauri::Result<()> {
    let panneau = panneau.filter(|p| panneau_valide(p)).map(str::to_owned);
    crate::app::ouvrir(app, Some(crate::app::Section::Reglages { panneau }))
}

#[tauri::command]
pub fn ouvrir_reglages<R: Runtime>(
    app: AppHandle<R>,
    panneau: Option<String>,
) -> Result<(), String> {
    ouvrir(&app, panneau.as_deref()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiant_de_panneau() {
        assert!(panneau_valide("stockage"));
        assert!(panneau_valide("dossiers-code"));
        assert!(!panneau_valide(""));
        assert!(!panneau_valide("../index"));
        assert!(!panneau_valide("a&b=c"));
    }
}
