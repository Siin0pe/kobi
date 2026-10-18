//! Fenêtre Réglages : remplace l'ancienne fenêtre du choix du skin (retirée
//! par KOB-137). Elle montre des panneaux (`ui/src/reglages/panneaux.ts`) ;
//! le premier est « Stockage » (KOB-141), les suivants viendront du premier
//! lancement (KOB-144) et du démarrage avec Windows (KOB-143).

use tauri::{AppHandle, Emitter, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};

/// Libellé de la fenêtre Réglages.
pub const FENETRE: &str = "reglages";

/// Événement envoyé à une fenêtre Réglages déjà ouverte pour changer de
/// panneau.
pub const EVENEMENT_PANNEAU: &str = "reglages://panneau";

/// Identifiant de panneau sûr dans une URL : lettres, chiffres et tirets.
fn panneau_valide(panneau: &str) -> bool {
    !panneau.is_empty()
        && panneau.len() <= 40
        && panneau
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Ouvre les Réglages (sur `panneau` s'il est donné), ou les ramène devant
/// s'ils sont déjà ouverts.
pub fn ouvrir<R: Runtime>(app: &AppHandle<R>, panneau: Option<&str>) -> tauri::Result<()> {
    let panneau = panneau.filter(|p| panneau_valide(p));
    if let Some(w) = app.get_webview_window(FENETRE) {
        if let Some(p) = panneau {
            app.emit_to(FENETRE, EVENEMENT_PANNEAU, p)?;
        }
        let _ = w.unminimize();
        w.show()?;
        return w.set_focus();
    }
    let page = match panneau {
        Some(p) => format!("reglages.html?panneau={p}"),
        None => "reglages.html".to_owned(),
    };
    WebviewWindowBuilder::new(app, FENETRE, WebviewUrl::App(page.into()))
        .title("Réglages de Kobi")
        .inner_size(860.0, 600.0)
        .min_inner_size(620.0, 440.0)
        .center()
        .visible(true)
        .focused(true)
        .build()?;
    tracing::info!(panneau, "réglages ouverts");
    Ok(())
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
