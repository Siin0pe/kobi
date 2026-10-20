//! Menu de Kobi : les mêmes entrées au clic droit sur la mascotte (dessiné
//! par l'interface) et dans la barre système (menu natif, `tray.rs`).
//!
//! Chaque entrée passe par [`executer`] : journal, événement `kobi://menu`
//! vers l'interface, puis l'effet. La réunion (enregistrer, pause) passe par
//! le chef d'orchestre (KOB-134) ; hors réunion, la pause coupe le micro
//! (KOB-132). Le Carnet et les réglages ouvrent leur fenêtre (KOB-140) ; le
//! mode discret et la confirmation avant de quitter sont faits par
//! l'interface à la réception de l'événement.

use std::sync::atomic::{AtomicBool, Ordering};

use kobi_session::etat::Evenement;
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::app::Section;
use crate::chef::ChefPartage;
use crate::intentions::Intention;
use crate::session::{etat, par_le_chef};

/// Événement envoyé à l'interface pour chaque entrée exécutée.
pub const EVENEMENT_MENU: &str = "kobi://menu";
/// Écoute en pause hors réunion (`true`) ou reprise (`false`).
pub const EVENEMENT_ECOUTE: &str = "ecoute";

/// Entrées du menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entree {
    Parler,
    /// Bulle d'écriture de Kobi (KOB-147).
    Ecrire,
    Enregistrer,
    Pause,
    /// L'app Kobi, sur sa section par défaut (KOB-159).
    App,
    Carnet,
    Reglages,
    /// Réponse à voix haute ou en texte seul, contexte courant (KOB-142).
    Voix,
    Discret,
    /// Barre système seulement : la mascotte n'a pas à se masquer elle-même.
    Afficher,
    Quitter,
    /// Confirmation de l'interface : arrêter la réunion en cours, puis quitter.
    QuitterConfirme,
}

impl Entree {
    pub const TOUTES: [Entree; 12] = [
        Entree::Parler,
        Entree::Ecrire,
        Entree::Enregistrer,
        Entree::Pause,
        Entree::App,
        Entree::Carnet,
        Entree::Reglages,
        Entree::Voix,
        Entree::Discret,
        Entree::Afficher,
        Entree::Quitter,
        Entree::QuitterConfirme,
    ];

    /// Menu de la barre système, dans l'ordre ; `None` : séparateur.
    pub const BARRE_SYSTEME: [Option<Entree>; 14] = [
        Some(Entree::Parler),
        Some(Entree::Ecrire),
        Some(Entree::Enregistrer),
        Some(Entree::Pause),
        None,
        Some(Entree::App),
        Some(Entree::Carnet),
        Some(Entree::Reglages),
        Some(Entree::Voix),
        None,
        Some(Entree::Discret),
        Some(Entree::Afficher),
        None,
        Some(Entree::Quitter),
    ];

    /// Identifiant stable, partagé avec `ui/src/menu.ts`.
    pub fn id(self) -> &'static str {
        match self {
            Entree::Parler => "parler",
            Entree::Ecrire => "ecrire",
            Entree::Enregistrer => "enregistrer",
            Entree::Pause => "pause",
            Entree::App => "app",
            Entree::Carnet => "carnet",
            Entree::Reglages => "reglages",
            Entree::Voix => "voix",
            Entree::Discret => "discret",
            Entree::Afficher => "afficher",
            Entree::Quitter => "quitter",
            Entree::QuitterConfirme => "quitter-confirme",
        }
    }

    /// Libellé du menu natif, qui ne suit pas l'état (celui de l'interface,
    /// si), dans la langue de l'interface (`barre.<id>`, KOB-210).
    pub fn libelle(self) -> String {
        kobi_i18n::tr(&format!("barre.{}", self.id()), &[])
    }

    pub fn depuis_id(id: &str) -> Option<Entree> {
        Entree::TOUTES.into_iter().find(|e| e.id() == id)
    }
}

/// Écoute en pause hors réunion (micro coupé, plus de « Kobi » repéré). En
/// réunion, la pause est celle de la session.
#[derive(Default)]
pub struct EcouteEnPause(AtomicBool);

pub fn ecoute_en_pause<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.try_state::<EcouteEnPause>()
        .is_some_and(|p| p.0.load(Ordering::SeqCst))
}

fn regler_ecoute<R: Runtime>(app: &AppHandle<R>, pause: bool) {
    let Some(p) = app.try_state::<EcouteEnPause>() else {
        return;
    };
    if p.0.swap(pause, Ordering::SeqCst) == pause {
        return;
    }
    // Le fil d'écoute (KOB-132) coupe ou rouvre le micro.
    if app.try_state::<crate::ecoute::Ecoute>().is_some() {
        if let Err(e) = crate::ecoute::commander(app, crate::ecoute::Commande::BasculerPause) {
            tracing::warn!(erreur = %e, "menu : pause de l'écoute impossible");
        }
    }
    tracing::info!(pause, "écoute");
    let _ = app.emit(EVENEMENT_ECOUTE, pause);
}

/// Le fil d'écoute a repris de lui-même (raccourci clavier) : l'état affiché suit.
pub fn ecoute_reprise<R: Runtime>(app: &AppHandle<R>) {
    if let Some(p) = app.try_state::<EcouteEnPause>() {
        if p.0.swap(false, Ordering::SeqCst) {
            tracing::info!(pause = false, "écoute");
            let _ = app.emit(EVENEMENT_ECOUTE, false);
        }
    }
}

/// Réunion ouverte, et en pause ? Le chef fait foi ; sans lui (tests),
/// l'état de la session.
fn reunion<R: Runtime>(app: &AppHandle<R>) -> Option<bool> {
    match app.try_state::<ChefPartage<R>>() {
        Some(chef) => {
            let e = chef
                .0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .enregistrement();
            e.actif.then_some(e.pause)
        }
        None => match etat(app).etat {
            "reunion" => Some(false),
            "pause" => Some(true),
            _ => None,
        },
    }
}

fn en_reunion<R: Runtime>(app: &AppHandle<R>) -> bool {
    reunion(app).is_some()
}

/// Réunion ouverte, compte rendu en cours d'écriture compris : quitter doit
/// alors l'attendre. REC (`en_reunion`) s'éteint dès la fin demandée
/// (KOB-166), avant que le compte rendu soit écrit (KOB-226).
fn reunion_ouverte<R: Runtime>(app: &AppHandle<R>) -> bool {
    match app.try_state::<ChefPartage<R>>() {
        Some(chef) => chef
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .reunion_ouverte(),
        None => en_reunion(app),
    }
}

/// Termine la réunion par le chef (fichiers de la réunion fermés).
fn terminer<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    par_le_chef(
        app,
        Intention::TerminerReunion,
        &[Evenement::TerminerReunion, Evenement::CompteRenduEcrit],
    )
    .map(|_| ())
}

fn montrer<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
    }
}

/// Ce que l'entrée a fait, pour les tests et le journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effet {
    /// Fait par la coque (ou rien à faire de plus).
    Fait,
    /// Laissé à l'interface (mode discret).
    Interface,
    /// Une réunion est en cours : l'interface demande confirmation.
    Confirmer,
    /// L'application se ferme.
    Fermer,
    /// L'application se ferme quand la réunion est close (compte rendu écrit).
    FermerApresCompteRendu,
}

/// Au-delà, l'application se ferme même si le compte rendu n'est pas fini.
const ATTENTE_COMPTE_RENDU: std::time::Duration =
    crate::chef::VIDANGE.saturating_add(std::time::Duration::from_secs(90));

/// Exécute une entrée sans fermer l'application : [`Effet::Fermer`] est
/// laissé à l'appelant.
pub fn appliquer_entree<R: Runtime>(app: &AppHandle<R>, entree: Entree) -> Result<Effet, String> {
    tracing::info!(entree = entree.id(), "menu");
    let _ = app.emit(EVENEMENT_MENU, entree.id());
    match entree {
        Entree::Parler => {
            montrer(app);
            crate::interpellation::declencher(app)?;
            Ok(Effet::Fait)
        }
        Entree::Enregistrer => {
            if en_reunion(app) {
                terminer(app)?;
            } else {
                par_le_chef(
                    app,
                    Intention::DemarrerReunion,
                    &[Evenement::DemarrerReunion],
                )?;
            }
            Ok(Effet::Fait)
        }
        Entree::Pause => {
            match reunion(app) {
                Some(false) => {
                    par_le_chef(app, Intention::Pause, &[Evenement::Pause])?;
                }
                Some(true) => {
                    par_le_chef(app, Intention::Reprendre, &[Evenement::Reprendre])?;
                }
                None => regler_ecoute(app, !ecoute_en_pause(app)),
            }
            Ok(Effet::Fait)
        }
        // L'app Kobi (KOB-159) : la section par défaut, le Carnet (notes et
        // réunions) ou les réglages.
        Entree::App => {
            crate::app::ouvrir(app, None).map_err(|e| format!("Kobi : {e}"))?;
            Ok(Effet::Fait)
        }
        Entree::Carnet => {
            crate::app::ouvrir(app, Some(Section::Notes { reunion: None }))
                .map_err(|e| format!("Carnet : {e}"))?;
            Ok(Effet::Fait)
        }
        Entree::Reglages => {
            crate::app::ouvrir(app, Some(Section::Reglages { panneau: None }))
                .map_err(|e| format!("Réglages : {e}"))?;
            Ok(Effet::Fait)
        }
        Entree::Ecrire => {
            // Comme le raccourci global : Kobi montré, focus clavier, bulle ouverte.
            if let Some(fenetre) = app.get_webview_window("main") {
                let _ = fenetre.show();
                let _ = fenetre.unminimize();
                let _ = fenetre.set_focus();
                let _ = fenetre.emit(crate::raccourci::EVENEMENT_ECRIRE, ());
            }
            Ok(Effet::Fait)
        }
        Entree::Voix => {
            crate::voix::basculer(app);
            Ok(Effet::Fait)
        }
        Entree::Discret => {
            montrer(app);
            Ok(Effet::Interface)
        }
        Entree::Afficher => {
            if let Some(window) = app.get_webview_window("main") {
                let visible = window.is_visible().unwrap_or(true);
                let _ = if visible {
                    window.hide()
                } else {
                    window.show()
                };
            }
            Ok(Effet::Fait)
        }
        Entree::Quitter if reunion_ouverte(app) => {
            montrer(app);
            Ok(Effet::Confirmer)
        }
        Entree::Quitter => Ok(Effet::Fermer),
        Entree::QuitterConfirme => {
            if !reunion_ouverte(app) {
                return Ok(Effet::Fermer);
            }
            terminer(app)?;
            // Quitter ne laisse pas « Annuler » ouvert (KOB-245) : la fin est
            // définitive, le compte rendu s'écrit dès qu'il revient.
            if let Some(chef) = app.try_state::<ChefPartage<R>>() {
                chef.0
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clore_annulation();
            }
            // Le chef écrit le compte rendu en arrière-plan (Claude).
            Ok(if reunion_ouverte(app) {
                Effet::FermerApresCompteRendu
            } else {
                Effet::Fermer
            })
        }
    }
}

/// Exécute une entrée par son identifiant (menu natif, interface).
pub fn executer<R: Runtime>(app: &AppHandle<R>, id: &str) -> Result<(), String> {
    let Some(entree) = Entree::depuis_id(id) else {
        tracing::warn!(entree = id, "menu : entrée inconnue");
        return Err(format!("entrée de menu inconnue : {id}"));
    };
    match appliquer_entree(app, entree) {
        Ok(Effet::Fermer) => {
            app.exit(0);
            Ok(())
        }
        Ok(Effet::FermerApresCompteRendu) => {
            tracing::info!("quitter : attente du compte rendu");
            let app = app.clone();
            std::thread::spawn(move || {
                let debut = std::time::Instant::now();
                while reunion_ouverte(&app) && debut.elapsed() < ATTENTE_COMPTE_RENDU {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
                tracing::info!(
                    ms = debut.elapsed().as_millis() as u64,
                    compte_rendu = !reunion_ouverte(&app),
                    "quitter"
                );
                app.exit(0);
            });
            Ok(())
        }
        Ok(_) => Ok(()),
        Err(refus) => {
            tracing::info!(entree = id, %refus, "menu : entrée refusée");
            Err(refus)
        }
    }
}

/// Entrée choisie dans le menu du clic droit.
// Asynchrone : une commande synchrone tourne sur le fil principal, et y
// créer la fenêtre bloque WebView2 sous Windows (fenêtre blanche, KOB-167).
#[tauri::command(async)]
pub fn menu_kobi<R: Runtime>(app: AppHandle<R>, entree: String) -> Result<(), String> {
    executer(&app, &entree)
}

/// L'écoute est-elle en pause (au démarrage de l'interface) ?
#[tauri::command]
pub fn etat_ecoute<R: Runtime>(app: AppHandle<R>) -> bool {
    ecoute_en_pause(&app)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session;
    use std::sync::{Arc, Mutex};
    use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};
    use tauri::{App, Listener};

    fn app() -> App<MockRuntime> {
        let app = session::installer(mock_builder())
            .build(mock_context(noop_assets()))
            .unwrap();
        app.manage(EcouteEnPause::default());
        app
    }

    /// Chaque entrée du menu natif a son libellé dans chaque langue (KOB-210).
    #[test]
    fn libelles_traduits() {
        for e in Entree::TOUTES {
            let cle = format!("barre.{}", e.id());
            for l in kobi_i18n::Langue::TOUTES {
                assert_ne!(kobi_i18n::t(l, &cle, &[]), cle, "{} : {cle}", l.code());
            }
        }
        assert_eq!(
            kobi_i18n::t(kobi_i18n::Langue::En, "barre.quitter", &[]),
            "Quit Kobi"
        );
    }

    fn ecouter(app: &App<MockRuntime>, nom: &str) -> Arc<Mutex<Vec<String>>> {
        let recus = Arc::new(Mutex::new(Vec::new()));
        let r = Arc::clone(&recus);
        app.listen(nom, move |e| r.lock().unwrap().push(e.payload().to_owned()));
        recus
    }

    #[test]
    fn chaque_entree_a_un_identifiant_unique_et_reversible() {
        for e in Entree::TOUTES {
            assert_eq!(Entree::depuis_id(e.id()), Some(e));
        }
        let mut ids: Vec<_> = Entree::TOUTES.iter().map(|e| e.id()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), Entree::TOUTES.len());
        assert_eq!(Entree::depuis_id("supprimer-tout"), None);
    }

    #[test]
    fn la_barre_systeme_a_les_entrees_du_clic_droit() {
        let ids: Vec<_> = Entree::BARRE_SYSTEME
            .iter()
            .flatten()
            .map(|e| e.id())
            .collect();
        assert_eq!(
            ids,
            [
                "parler",
                "ecrire",
                "enregistrer",
                "pause",
                "app",
                "carnet",
                "reglages",
                "voix",
                "discret",
                "afficher",
                "quitter"
            ]
        );
    }

    #[test]
    fn enregistrer_demarre_puis_arrete_la_reunion() {
        let app = app();
        let h = app.handle();
        let menus = ecouter(&app, EVENEMENT_MENU);
        assert_eq!(appliquer_entree(h, Entree::Enregistrer), Ok(Effet::Fait));
        assert_eq!(etat(h).etat, "reunion");
        assert_eq!(appliquer_entree(h, Entree::Enregistrer), Ok(Effet::Fait));
        assert_eq!(etat(h).etat, "veille");
        assert_eq!(
            *menus.lock().unwrap(),
            ["\"enregistrer\"", "\"enregistrer\""]
        );
    }

    #[test]
    fn pause_en_reunion_puis_reprise() {
        let app = app();
        let h = app.handle();
        appliquer_entree(h, Entree::Enregistrer).unwrap();
        appliquer_entree(h, Entree::Pause).unwrap();
        assert_eq!(etat(h).etat, "pause");
        appliquer_entree(h, Entree::Pause).unwrap();
        assert_eq!(etat(h).etat, "reunion");
        assert!(
            !ecoute_en_pause(h),
            "la pause de réunion n'est pas celle de l'écoute"
        );
        // Arrêter pendant la pause est permis.
        appliquer_entree(h, Entree::Pause).unwrap();
        appliquer_entree(h, Entree::Enregistrer).unwrap();
        assert_eq!(etat(h).etat, "veille");
    }

    #[test]
    fn pause_hors_reunion_coupe_puis_rend_l_ecoute() {
        let app = app();
        let h = app.handle();
        let ecoute = ecouter(&app, EVENEMENT_ECOUTE);
        appliquer_entree(h, Entree::Pause).unwrap();
        assert!(ecoute_en_pause(h));
        assert_eq!(etat(h).etat, "veille");
        appliquer_entree(h, Entree::Pause).unwrap();
        assert!(!ecoute_en_pause(h));
        assert_eq!(*ecoute.lock().unwrap(), ["true", "false"]);
    }

    #[test]
    fn carnet_et_reglages_ouvrent_l_app_le_discret_est_a_l_interface() {
        let app = app();
        let h = app.handle();
        let menus = ecouter(&app, EVENEMENT_MENU);
        // Kobi, Carnet et Réglages sont des sections de l'app Kobi (KOB-159).
        assert_eq!(appliquer_entree(h, Entree::App), Ok(Effet::Fait));
        assert!(h.get_webview_window(crate::app::FENETRE).is_some());
        assert_eq!(appliquer_entree(h, Entree::Carnet), Ok(Effet::Fait));
        assert!(h.get_webview_window(crate::app::FENETRE).is_some());
        assert_eq!(appliquer_entree(h, Entree::Reglages), Ok(Effet::Fait));
        assert!(h.get_webview_window(crate::app::FENETRE).is_some());
        assert_eq!(appliquer_entree(h, Entree::Discret), Ok(Effet::Interface));
        assert_eq!(
            *menus.lock().unwrap(),
            ["\"app\"", "\"carnet\"", "\"reglages\"", "\"discret\""]
        );
    }

    #[test]
    fn quitter_demande_confirmation_pendant_une_reunion() {
        let app = app();
        let h = app.handle();
        assert_eq!(appliquer_entree(h, Entree::Quitter), Ok(Effet::Fermer));
        appliquer_entree(h, Entree::Enregistrer).unwrap();
        assert_eq!(appliquer_entree(h, Entree::Quitter), Ok(Effet::Confirmer));
        assert_eq!(
            etat(h).etat,
            "reunion",
            "rien n'est arrêté sans confirmation"
        );
        assert_eq!(
            appliquer_entree(h, Entree::QuitterConfirme),
            Ok(Effet::Fermer)
        );
        assert_eq!(
            etat(h).etat,
            "veille",
            "la réunion est arrêtée avant de quitter"
        );
    }

    /// KOB-226 : avec le chef, REC s'éteint dès la fin demandée (KOB-166),
    /// mais la réunion reste ouverte le temps du compte rendu : quitter
    /// l'attend au lieu de fermer tout de suite.
    #[test]
    fn quitter_attend_le_compte_rendu_du_chef() {
        let app = app();
        let h = app.handle();
        let dir = tempfile::tempdir().unwrap();
        app.manage(crate::stockage::StockagePartage::new(
            Some(kobi_session::stockage::Stockage::new(dir.path())),
            None,
        ));
        // Claude ne répond jamais : le compte rendu reste en cours d'écriture.
        let fabrique: crate::cerveau::Fabrique = Arc::new(|_role| Box::pin(std::future::pending()));
        crate::chef::installer(h, fabrique, 3, Default::default());
        appliquer_entree(h, Entree::Enregistrer).unwrap();
        assert!(reunion_ouverte(h));
        assert_eq!(appliquer_entree(h, Entree::Quitter), Ok(Effet::Confirmer));
        assert_eq!(
            appliquer_entree(h, Entree::QuitterConfirme),
            Ok(Effet::FermerApresCompteRendu)
        );
        assert!(!en_reunion(h), "REC éteint dès la fin demandée");
        // Compte rendu toujours en cours : quitter redemande, puis attend.
        assert_eq!(appliquer_entree(h, Entree::Quitter), Ok(Effet::Confirmer));
        assert_eq!(
            appliquer_entree(h, Entree::QuitterConfirme),
            Ok(Effet::FermerApresCompteRendu)
        );
    }

    #[test]
    fn entree_inconnue_refusee() {
        let app = app();
        let refus = executer(app.handle(), "supprimer-tout").unwrap_err();
        assert!(refus.contains("supprimer-tout"));
    }
}
