//! L'app Kobi (KOB-159) : la seule fenêtre en plus de la mascotte. Elle
//! réunit le Carnet (notes et réunions), les rappels, l'activité, les
//! réglages et l'à-propos ; au premier démarrage, elle s'ouvre sur l'écran
//! d'accueil (le premier lancement).
//!
//! [`ouvrir`] est le point d'entrée unique : menu du clic droit, barre
//! système, intention vocale, `kobi app open --section …` (KOB-161) et les
//! anciennes commandes (`ouvrir_carnet`, `ouvrir_reglages`,
//! `ouvrir_bienvenue`) passent toutes par elle.

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};

/// Libellé de la fenêtre de l'app.
pub const FENETRE: &str = "app";

/// Événement envoyé à l'app déjà ouverte pour changer de section ; la
/// charge est l'adresse de la section ([`Section::adresse`]).
pub const EVENEMENT_SECTION: &str = "app://section";

/// Taille de la fenêtre, en pixels logiques.
const TAILLE: (f64, f64) = (1180.0, 760.0);

/// Section de l'app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Section {
    /// Notes et réunions (le Carnet), éventuellement sur une réunion
    /// (son compte rendu, carte « Compte rendu prêt » de KOB-74).
    Notes { reunion: Option<String> },
    Rappels,
    Activite,
    /// Réglages, éventuellement sur un panneau (`stockage`, `micro`…).
    Reglages { panneau: Option<String> },
    APropos,
    /// Écran d'accueil du premier lancement.
    Accueil,
}

/// Identifiant sûr dans une URL : lettres, chiffres, tirets et soulignés.
fn identifiant_valide(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 120
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

impl Section {
    /// Lit une adresse (`notes`, `notes/<réunion>`, `rappels`, `activite`,
    /// `reglages`, `reglages/<panneau>`, `a-propos`, `accueil`) ; `None` si
    /// elle est inconnue ou si son détail n'est pas un identifiant sûr.
    pub fn lire(adresse: &str) -> Option<Self> {
        let (tete, detail) = match adresse.split_once('/') {
            Some((t, d)) => (t, Some(d)),
            None => (adresse, None),
        };
        let detail = match detail {
            Some(d) if identifiant_valide(d) => Some(d.to_owned()),
            Some(_) => return None,
            None => None,
        };
        Some(match tete {
            "notes" | "carnet" => Section::Notes { reunion: detail },
            "rappels" => Section::Rappels,
            "activite" => Section::Activite,
            "reglages" => Section::Reglages { panneau: detail },
            "a-propos" => Section::APropos,
            "accueil" | "bienvenue" => Section::Accueil,
            _ => return None,
        })
    }

    /// Adresse de la section, comprise par l'interface (ancre de l'URL).
    pub fn adresse(&self) -> String {
        match self {
            Section::Notes { reunion: Some(r) } => format!("notes/{r}"),
            Section::Notes { reunion: None } => "notes".into(),
            Section::Rappels => "rappels".into(),
            Section::Activite => "activite".into(),
            Section::Reglages { panneau: Some(p) } => format!("reglages/{p}"),
            Section::Reglages { panneau: None } => "reglages".into(),
            Section::APropos => "a-propos".into(),
            Section::Accueil => "accueil".into(),
        }
    }
}

/// Ouvre l'app sur `section` (sur la section en cours, ou les notes, si
/// `None`), ou la ramène devant si elle est déjà ouverte.
pub fn ouvrir<R: Runtime>(app: &AppHandle<R>, section: Option<Section>) -> tauri::Result<()> {
    if let Some(w) = app.get_webview_window(FENETRE) {
        if let Some(s) = &section {
            app.emit_to(FENETRE, EVENEMENT_SECTION, s.adresse())?;
        }
        let _ = w.unminimize();
        w.show()?;
        return w.set_focus();
    }
    let adresse = section
        .as_ref()
        .map_or_else(|| "notes".to_owned(), Section::adresse);
    crate::placement::a_cote_de_kobi_ou_centre(
        app,
        WebviewWindowBuilder::new(
            app,
            FENETRE,
            WebviewUrl::App(format!("app.html#{adresse}").into()),
        )
        .title("Kobi")
        .inner_size(TAILLE.0, TAILLE.1)
        .min_inner_size(760.0, 500.0)
        .visible(true)
        .focused(true),
        TAILLE,
    )
    .build()?;
    tracing::info!(%adresse, "app ouverte");
    Ok(())
}

/// Ouvre l'app sur une section donnée par son adresse (voir [`Section::lire`]).
#[tauri::command]
pub fn ouvrir_app<R: Runtime>(app: AppHandle<R>, section: Option<String>) -> Result<(), String> {
    let section = match section.as_deref() {
        Some(a) => Some(Section::lire(a).ok_or_else(|| format!("section inconnue : {a}"))?),
        None => None,
    };
    ouvrir(&app, section).map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Serialize)]
pub struct APropos {
    pub version: &'static str,
    pub dossier: Option<std::path::PathBuf>,
}

/// Version de Kobi et dossier de travail, pour la section À propos.
#[tauri::command]
pub fn a_propos<R: Runtime>(app: AppHandle<R>) -> APropos {
    APropos {
        version: env!("CARGO_PKG_VERSION"),
        dossier: crate::stockage::actuel(&app)
            .map(|s| s.racine().to_owned())
            .or_else(kobi_session::config::kobi_dir),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adresses_des_sections() {
        for a in [
            "notes",
            "notes/2026-10-18-point-hebdo",
            "rappels",
            "activite",
            "reglages",
            "reglages/stockage",
            "a-propos",
            "accueil",
        ] {
            assert_eq!(Section::lire(a).unwrap().adresse(), a);
        }
        // Anciens noms des fenêtres.
        assert_eq!(
            Section::lire("carnet"),
            Some(Section::Notes { reunion: None })
        );
        assert_eq!(Section::lire("bienvenue"), Some(Section::Accueil));
        // Inconnu ou détail douteux : refusé.
        assert_eq!(Section::lire("ailleurs"), None);
        assert_eq!(Section::lire("notes/../config"), None);
        assert_eq!(Section::lire("reglages/a b"), None);
    }

    /// L'app s'ouvre une seule fois ; les anciennes commandes y mènent.
    #[test]
    fn une_seule_fenetre_pour_tout() {
        use crate::test_ipc::appeler;
        use serde_json::json;
        use tauri::test::{mock_builder, mock_context, noop_assets};

        let app = crate::session::installer(mock_builder())
            .build(mock_context(noop_assets()))
            .unwrap();
        let main = WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        appeler(&main, "ouvrir_carnet", json!({})).unwrap();
        appeler(&main, "ouvrir_carnet", json!({ "reunion": "2026-10-18-point" })).unwrap();
        appeler(&main, "ouvrir_reglages", json!({ "panneau": "stockage" })).unwrap();
        appeler(&main, "ouvrir_bienvenue", json!({})).unwrap();
        appeler(&main, "ouvrir_app", json!({ "section": "rappels" })).unwrap();
        appeler(&main, "ouvrir_app", json!({})).unwrap();
        assert!(appeler(&main, "ouvrir_app", json!({ "section": "ailleurs" })).is_err());
        let mut fenetres: Vec<_> = app.webview_windows().into_keys().collect();
        fenetres.sort();
        assert_eq!(fenetres, ["app", "main"]);
        let a = appeler(&main, "a_propos", json!({})).unwrap();
        assert_eq!(a["version"], env!("CARGO_PKG_VERSION"));
    }
}
