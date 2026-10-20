//! Stockage partagé de l'application (KOB-141) : le dossier racine des
//! réunions, conversations et rappels, réglable à chaud.
//!
//! Le chef d'orchestre prend un [`Stockage`] avec [`actuel`] au moment
//! d'ouvrir une réunion ou une conversation : un changement de dossier vaut
//! pour la suivante, celle en cours finit là où elle a commencé.

use std::path::{Path, PathBuf};
use std::sync::RwLock;

use kobi_session::config::{self, Config, Publication, Retention};
use kobi_session::reunion::Date;
use kobi_session::stockage::{etat_publication, EtatPublication, Stockage};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Runtime};

/// Stockage courant et fichier de réglages où le dossier est écrit.
pub struct StockagePartage {
    stockage: RwLock<Option<Stockage>>,
    config: Option<PathBuf>,
}

impl StockagePartage {
    pub fn new(stockage: Option<Stockage>, config: Option<PathBuf>) -> Self {
        Self {
            stockage: RwLock::new(stockage),
            config,
        }
    }
}

/// Stockage courant (absent seulement sans dossier personnel).
pub fn actuel<R: Runtime>(app: &AppHandle<R>) -> Option<Stockage> {
    let partage = app.try_state::<StockagePartage>()?;
    let s = partage.stockage.read().unwrap_or_else(|e| e.into_inner());
    s.clone()
}

/// Au démarrage : prépare le dossier (renomme l'ancien `meetings/`) et purge
/// ce qui a dépassé sa durée de conservation, sans retarder l'ouverture.
pub fn installer<R: Runtime>(app: &AppHandle<R>, config: &Config, chemin: Option<PathBuf>) {
    let stockage = Stockage::depuis_config(config);
    app.manage(StockagePartage::new(stockage.clone(), chemin));
    let Some(stockage) = stockage else {
        tracing::warn!("dossier personnel introuvable : rien ne sera rangé");
        return;
    };
    let (reunions, conversations) = (
        config.meetings.retention_days,
        config.conversations.retention_days,
    );
    std::thread::spawn(move || {
        match stockage.preparer() {
            Ok(true) => tracing::info!("meetings/ renommé en reunions/"),
            Ok(false) => {}
            Err(e) => {
                tracing::warn!(erreur = %e, racine = %stockage.racine().display(), "stockage")
            }
        }
        match stockage.purger(reunions, conversations, Date::aujourdhui_utc(), None) {
            Ok(partis) if !partis.is_empty() => {
                tracing::info!(n = partis.len(), "purge : durée de conservation dépassée")
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(erreur = %e, "purge"),
        }
        tracing::info!(racine = %stockage.racine().display(), "stockage prêt");
    });
}

/// Réglages du stockage, montrés à l'utilisateur.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Reglages {
    /// Dossier racine effectif.
    pub dossier: Option<PathBuf>,
    /// Le dossier est celui par défaut (`~/Kobi`).
    pub par_defaut: bool,
    pub retention_reunions: Retention,
    pub retention_conversations: Retention,
    pub publication_reunions: EtatPublication,
    pub publication_conversations: EtatPublication,
}

/// Changement demandé par l'interface ; un champ absent reste tel quel.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Changement {
    /// Nouveau dossier racine ; chaîne vide = retour au défaut.
    pub dossier: Option<String>,
    pub retention_reunions: Option<Retention>,
    pub retention_conversations: Option<Retention>,
    pub publication_reunions: Option<PublicationUi>,
    pub publication_conversations: Option<PublicationUi>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PublicationUi {
    pub mcp: String,
    pub destination: String,
}

fn reglages(config: &Config) -> Reglages {
    Reglages {
        dossier: config.stockage.racine(),
        par_defaut: config.stockage.dossier.is_none(),
        retention_reunions: config.meetings.retention_days,
        retention_conversations: config.conversations.retention_days,
        publication_reunions: etat_publication(&config.stockage.publication.reunions),
        publication_conversations: etat_publication(&config.stockage.publication.conversations),
    }
}

fn lire_config(chemin: Option<&Path>) -> Config {
    chemin
        .map(|p| config::load_or_create(p).config)
        .unwrap_or_default()
}

/// Applique `changement` à `config` ; refuse un dossier relatif.
pub fn appliquer(config: &mut Config, changement: Changement) -> Result<(), String> {
    if let Some(dossier) = changement.dossier {
        let dossier = dossier.trim();
        config.stockage.dossier = if dossier.is_empty() {
            None
        } else {
            let p = PathBuf::from(dossier);
            if !p.is_absolute() {
                return Err(kobi_i18n::tr(
                    "coque.stockage.cheminComplet",
                    &[("dossier", dossier)],
                ));
            }
            Some(p)
        };
    }
    if let Some(r) = changement.retention_reunions {
        config.meetings.retention_days = r;
    }
    if let Some(r) = changement.retention_conversations {
        config.conversations.retention_days = r;
    }
    let publication = |p: PublicationUi, cible: &mut Publication| {
        cible.mcp = p.mcp.trim().to_owned();
        cible.destination = p.destination.trim().to_owned();
    };
    if let Some(p) = changement.publication_reunions {
        publication(p, &mut config.stockage.publication.reunions);
    }
    if let Some(p) = changement.publication_conversations {
        publication(p, &mut config.stockage.publication.conversations);
    }
    Ok(())
}

#[tauri::command]
pub fn reglages_stockage<R: Runtime>(app: AppHandle<R>) -> Reglages {
    let chemin = app
        .try_state::<StockagePartage>()
        .and_then(|p| p.config.clone());
    reglages(&lire_config(chemin.as_deref()))
}

/// Change les réglages du stockage, les écrit dans `config.toml` et bascule
/// sur le nouveau dossier tout de suite, sans redémarrer.
#[tauri::command]
pub fn regler_stockage<R: Runtime>(
    app: AppHandle<R>,
    changement: Changement,
) -> Result<Reglages, String> {
    let partage = app
        .try_state::<StockagePartage>()
        .ok_or_else(|| kobi_i18n::tr("coque.stockage.indisponible", &[]))?;
    let mut config = lire_config(partage.config.as_deref());
    appliquer(&mut config, changement)?;
    let nouveau = Stockage::depuis_config(&config)
        .ok_or_else(|| kobi_i18n::tr("coque.commun.dossierPersoIntrouvable", &[]))?;
    nouveau
        .preparer()
        .map_err(|e| format!("{} : {e}", nouveau.racine().display()))?;
    if let Some(chemin) = &partage.config {
        config::save(&config, chemin).map_err(|e| e.to_string())?;
    }
    tracing::info!(racine = %nouveau.racine().display(), "stockage réglé");
    *partage.stockage.write().unwrap_or_else(|e| e.into_inner()) = Some(nouveau);
    Ok(reglages(&config))
}

/// Bascule sur le dossier de `config`, déjà écrite dans `config.toml`
/// (interface de contrôle, KOB-161).
pub fn basculer<R: Runtime>(app: &AppHandle<R>, config: &Config) -> Result<(), String> {
    let partage = app
        .try_state::<StockagePartage>()
        .ok_or_else(|| kobi_i18n::tr("coque.stockage.indisponible", &[]))?;
    let nouveau = Stockage::depuis_config(config)
        .ok_or_else(|| kobi_i18n::tr("coque.commun.dossierPersoIntrouvable", &[]))?;
    nouveau
        .preparer()
        .map_err(|e| format!("{} : {e}", nouveau.racine().display()))?;
    tracing::info!(racine = %nouveau.racine().display(), "stockage réglé (contrôle)");
    *partage.stockage.write().unwrap_or_else(|e| e.into_inner()) = Some(nouveau);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobi_session::stockage::Qui;
    use tauri::ipc::{CallbackFn, InvokeBody};
    use tauri::test::{get_ipc_response, mock_builder, mock_context, noop_assets, INVOKE_KEY};
    use tauri::webview::InvokeRequest;
    use tauri::WebviewWindowBuilder;

    fn requete(cmd: &str, corps: serde_json::Value) -> InvokeRequest {
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

    /// Le dossier change depuis l'interface : `config.toml` est écrit, et la
    /// conversation suivante va dans le nouveau dossier, sans redémarrer.
    #[test]
    fn changer_de_dossier_depuis_l_interface() {
        let dir = tempfile::tempdir().unwrap();
        let chemin = dir.path().join("Kobi").join(config::FILE_NAME);
        let mut config = Config::default();
        config.stockage.dossier = Some(dir.path().join("Kobi"));
        config::save(&config, &chemin).unwrap();

        let app = crate::session::installer(mock_builder())
            .build(mock_context(noop_assets()))
            .unwrap();
        installer(app.handle(), &config, Some(chemin.clone()));
        let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let appel = |cmd: &str, corps| {
            get_ipc_response(&webview, requete(cmd, corps))
                .map(|b| b.deserialize::<serde_json::Value>().unwrap())
        };

        let avant = actuel(app.handle()).unwrap();
        assert_eq!(avant.racine(), dir.path().join("Kobi"));

        let ailleurs = dir.path().join("Ailleurs");
        let r = appel(
            "regler_stockage",
            serde_json::json!({ "changement": {
                "dossier": ailleurs.to_str().unwrap(),
                "retention_conversations": 30,
                "publication_reunions": { "mcp": "linear", "destination": "Projet Kobi" }
            }}),
        )
        .unwrap();
        assert_eq!(r["dossier"], ailleurs.to_str().unwrap());
        assert_eq!(r["retention_conversations"], 30);
        assert_eq!(r["publication_reunions"]["etat"], "reglee");
        assert_eq!(r["publication_conversations"]["etat"], "aucune");

        let apres = actuel(app.handle()).unwrap();
        assert_eq!(apres.racine(), ailleurs);
        assert!(ailleurs.join("reunions").is_dir() && ailleurs.join("conversations").is_dir());
        let mut c = apres.nouvelle_conversation(kobi_session::stockage::maintenant_ms());
        c.ajouter(Qui::Moi, "bonjour", true).unwrap();
        assert!(c.chemin().starts_with(&ailleurs));

        let relu = config::load_or_create(&chemin).config;
        assert_eq!(relu.stockage.dossier, Some(ailleurs));
        assert_eq!(relu.conversations.retention_days, Retention::Days(30));
        assert_eq!(
            appel("reglages_stockage", serde_json::json!({})).unwrap(),
            r
        );

        // Chemin relatif refusé, rien ne change.
        let refus = appel(
            "regler_stockage",
            serde_json::json!({ "changement": { "dossier": "relatif" } }),
        )
        .unwrap_err();
        assert!(
            refus.as_str().unwrap().contains("chemin complet"),
            "{refus}"
        );
        assert_eq!(actuel(app.handle()).unwrap(), apres);

        // Chaîne vide : retour au dossier par défaut.
        let mut c = relu.clone();
        appliquer(
            &mut c,
            Changement {
                dossier: Some(String::new()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(c.stockage.dossier, None);
    }
}
