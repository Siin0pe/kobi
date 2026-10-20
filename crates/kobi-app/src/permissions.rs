//! Politique `[permissions]` réglable depuis l'interface (KOB-215) : la case
//! « Ne plus demander » d'une carte de validation, et le panneau Permissions
//! des Réglages, qui la montre et la rend révocable. Une suppression se
//! demande toujours : elle n'a pas de réglage (contrat des permissions, §3).

use std::path::Path;

use kobi_session::config::{self, Politique};
use serde::Serialize;
use tauri::{AppHandle, Runtime};

/// Ce que montre Réglages › Permissions : `true` = fait sans demander.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Permissions {
    pub fichier: bool,
    pub commande: bool,
    pub reglage: bool,
}

impl Permissions {
    fn depuis(p: &config::PermissionsConfig) -> Self {
        let sans = |p: &Politique| *p == Politique::SansDemander;
        Self {
            fichier: sans(&p.fichier),
            commande: sans(&p.commande),
            reglage: sans(&p.reglage),
        }
    }
}

pub fn lire(chemin: &Path) -> Permissions {
    Permissions::depuis(&config::load_or_create(chemin).config.permissions)
}

/// Règle un genre d'action (`fichier`, `commande` ou `reglage`).
pub fn regler(chemin: &Path, genre: &str, sans_demander: bool) -> Result<Permissions, String> {
    let mut lu = config::load_or_create(chemin);
    if let Some(probleme) = lu.problem {
        return Err(kobi_i18n::tr(
            "coque.commun.reglageNonEnregistre",
            &[("probleme", &probleme.to_string())],
        ));
    }
    let p = &mut lu.config.permissions;
    let cible = match genre {
        "fichier" => &mut p.fichier,
        "commande" => &mut p.commande,
        "reglage" => &mut p.reglage,
        // La suppression, un service (MCP, réglé par serveur) ou autre chose :
        // pas de réglage ici.
        _ => {
            return Err(kobi_i18n::tr(
                "coque.permissions.toujoursDemande",
                &[("genre", genre)],
            ))
        }
    };
    *cible = if sans_demander {
        Politique::SansDemander
    } else {
        Politique::Demander
    };
    config::save(&lu.config, chemin).map_err(|e| e.to_string())?;
    Ok(Permissions::depuis(&lu.config.permissions))
}

#[tauri::command]
pub fn permissions() -> Result<Permissions, String> {
    let chemin = config::default_path()
        .ok_or_else(|| kobi_i18n::tr("coque.commun.dossierPersoIntrouvable", &[]))?;
    Ok(lire(&chemin))
}

/// Choix de l'utilisateur lui-même (case de la carte, panneau des Réglages) :
/// il ne passe pas par une carte de validation.
#[tauri::command]
pub fn regler_permission<R: Runtime>(
    _app: AppHandle<R>,
    genre: String,
    sans_demander: bool,
) -> Result<Permissions, String> {
    let chemin = config::default_path()
        .ok_or_else(|| kobi_i18n::tr("coque.commun.dossierPersoIntrouvable", &[]))?;
    let p = regler(&chemin, &genre, sans_demander)?;
    tracing::info!(%genre, sans_demander, "permission réglée depuis l'interface");
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reglee_puis_relue_et_revocable() {
        let dir = tempfile::tempdir().unwrap();
        let chemin = dir.path().join("config.toml");
        let tout_demande = Permissions {
            fichier: false,
            commande: false,
            reglage: false,
        };
        assert_eq!(lire(&chemin), tout_demande);
        let p = regler(&chemin, "commande", true).unwrap();
        assert_eq!(
            p,
            Permissions {
                commande: true,
                ..tout_demande
            }
        );
        assert_eq!(lire(&chemin), p);
        let texte = std::fs::read_to_string(&chemin).unwrap();
        assert!(texte.contains("commande = \"sans_demander\""), "{texte}");
        assert_eq!(regler(&chemin, "commande", false).unwrap(), tout_demande);
    }

    #[test]
    fn suppression_et_inconnus_se_demandent_toujours() {
        let dir = tempfile::tempdir().unwrap();
        let chemin = dir.path().join("config.toml");
        for genre in ["suppression", "mcp", "", "fichiers"] {
            assert!(regler(&chemin, genre, true).is_err(), "{genre}");
        }
        assert!(!lire(&chemin).fichier);
    }
}
