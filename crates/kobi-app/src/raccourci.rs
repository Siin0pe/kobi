//! Raccourcis clavier globaux, réglables dans `~/Kobi/config.toml` :
//!
//! * `hotkey` (`Ctrl+Shift+Space` par défaut), secours du mot-clé : il
//!   interpelle Kobi comme « Kobi, … », la phrase suivante est la question
//!   (voir [`crate::ecoute`]). Il ne démarre jamais d'enregistrement ;
//! * `hotkey_record` (`Ctrl+Shift+R` par défaut) : démarre l'enregistrement
//!   de la réunion, ou l'arrête s'il tourne (puis compte rendu) ;
//! * `hotkey_write` (`Ctrl+Alt+K` par défaut) : montre Kobi et ouvre sa bulle
//!   pour lui écrire (KOB-147) ;
//! * `hotkey_view` (`Ctrl+Alt+G` par défaut) : ouvre ou range la vue agrandie
//!   de la conversation (KOB-169).
//!
//! Un changement de réglage (à la voix, par Claude, `kobi config set`)
//! s'applique à chaud (KOB-214) : l'ancien raccourci est libéré, le nouveau
//! inscrit ; une combinaison déjà prise ne change rien.

use std::str::FromStr;
use std::sync::Mutex;

use tauri::{AppHandle, Emitter, Manager, Runtime};
use tauri_plugin_global_shortcut::{GlobalShortcut, GlobalShortcutExt, Shortcut, ShortcutState};

use crate::chef::{publier, ChefPartage, Message};
use crate::intentions::Intention;

/// Raccourci de la config, ou `defaut` s'il est illisible (avec la raison, à
/// journaliser). `cle` nomme le réglage dans le message.
pub fn lire(cle: &str, valeur: &str, defaut: &str) -> (Shortcut, Option<String>) {
    match Shortcut::from_str(valeur) {
        Ok(s) => (s, None),
        Err(e) => {
            let s = Shortcut::from_str(defaut).expect("raccourci par défaut valide");
            (
                s,
                Some(kobi_i18n::tr(
                    "coque.raccourci.illisible",
                    &[
                        ("cle", cle),
                        ("valeur", valeur),
                        ("erreur", &e.to_string()),
                        ("defaut", defaut),
                    ],
                )),
            )
        }
    }
}

/// Raccourcis enregistrés, pour reconnaître celui qui est pressé.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Raccourcis {
    interpeller: Shortcut,
    enregistrer: Shortcut,
    ecrire: Shortcut,
    vue: Shortcut,
}

impl Raccourcis {
    /// Les quatre raccourcis de la config ; un raccourci illisible est
    /// remplacé par son défaut (avec la raison, à journaliser).
    fn depuis(config: &kobi_session::config::Config) -> (Self, Vec<String>) {
        let defaut = kobi_session::config::Config::default();
        let (interpeller, p1) = lire("hotkey", &config.hotkey, &defaut.hotkey);
        let (enregistrer, p2) = lire(
            "hotkey_record",
            &config.hotkey_record,
            &defaut.hotkey_record,
        );
        let (ecrire, p3) = lire("hotkey_write", &config.hotkey_write, &defaut.hotkey_write);
        let (vue, p4) = lire("hotkey_view", &config.hotkey_view, &defaut.hotkey_view);
        let r = Raccourcis {
            interpeller,
            enregistrer,
            ecrire,
            vue,
        };
        (r, [p1, p2, p3, p4].into_iter().flatten().collect())
    }

    fn tous(&self) -> [Shortcut; 4] {
        [self.interpeller, self.enregistrer, self.ecrire, self.vue]
    }
}

/// Raccourcis actifs, changés à chaud.
struct Actifs(Mutex<Raccourcis>);

/// Inscription des raccourcis auprès du système (le plugin ; un faux en test).
trait Clavier {
    fn inscrire(&self, r: Shortcut) -> Result<(), String>;
    fn desinscrire(&self, r: Shortcut);
}

impl<R: Runtime> Clavier for GlobalShortcut<R> {
    fn inscrire(&self, r: Shortcut) -> Result<(), String> {
        self.register(r).map_err(|e| e.to_string())
    }
    fn desinscrire(&self, r: Shortcut) {
        let _ = self.unregister(r);
    }
}

/// Passe des raccourcis `anciens` aux `nouveaux` : libère ceux qui changent,
/// inscrit les nouveaux. Si l'un est déjà pris (autre application, ou un
/// autre raccourci de Kobi), tout revient comme avant et l'erreur le dit.
fn remplacer(
    clavier: &impl Clavier,
    anciens: Raccourcis,
    nouveaux: Raccourcis,
) -> Result<(), String> {
    let changes: Vec<(Shortcut, Shortcut)> = anciens
        .tous()
        .into_iter()
        .zip(nouveaux.tous())
        .filter(|(a, n)| a != n)
        .collect();
    for (ancien, _) in &changes {
        clavier.desinscrire(*ancien);
    }
    let mut inscrits = Vec::new();
    for (ancien, nouveau) in &changes {
        if let Err(e) = clavier.inscrire(*nouveau) {
            for n in &inscrits {
                clavier.desinscrire(*n);
            }
            for (a, _) in &changes {
                let _ = clavier.inscrire(*a);
            }
            return Err(kobi_i18n::tr(
                "coque.raccourci.dejaUtilise",
                &[
                    ("nouveau", &nouveau.into_string()),
                    ("ancien", &ancien.into_string()),
                    ("erreur", &e),
                ],
            ));
        }
        inscrits.push(*nouveau);
    }
    Ok(())
}

/// Applique à chaud les raccourcis de `config` (KOB-214). Sans raccourcis
/// installés (essais), il n'y a rien à changer.
pub fn appliquer<R: Runtime>(
    app: &AppHandle<R>,
    config: &kobi_session::config::Config,
) -> Result<(), String> {
    let Some(actifs) = app.try_state::<Actifs>() else {
        return Ok(());
    };
    let (nouveaux, problemes) = Raccourcis::depuis(config);
    if let Some(p) = problemes.first() {
        return Err(p.clone());
    }
    let mut actuels = actifs.0.lock().unwrap_or_else(|e| e.into_inner());
    remplacer(app.global_shortcut(), *actuels, nouveaux)?;
    if *actuels != nouveaux {
        tracing::info!(raccourcis = ?nouveaux.tous().map(|r| r.into_string()), "raccourcis globaux changés à chaud");
    }
    *actuels = nouveaux;
    Ok(())
}

/// Événement qui ouvre la bulle d'écriture dans l'interface.
pub const EVENEMENT_ECRIRE: &str = "ecrire";

/// Montre Kobi (même replié ou masqué), lui donne le focus clavier et ouvre
/// sa bulle : on peut taper tout de suite.
fn ouvrir_bulle<R: Runtime>(app: &AppHandle<R>) {
    let Some(fenetre) = app.get_webview_window("main") else {
        return;
    };
    let _ = fenetre.show();
    let _ = fenetre.unminimize();
    let _ = fenetre.set_focus();
    let _ = fenetre.emit(EVENEMENT_ECRIRE, ());
    tracing::info!("raccourci global : écrire à Kobi");
}

/// Démarre l'enregistrement, ou l'arrête s'il tourne.
fn basculer_enregistrement<R: Runtime>(app: &AppHandle<R>) {
    let actif = app.try_state::<ChefPartage<R>>().is_some_and(|c| {
        c.0.lock()
            .map(|c| c.enregistrement().actif)
            .unwrap_or(false)
    });
    let intention = if actif {
        Intention::TerminerReunion
    } else {
        Intention::DemarrerReunion
    };
    tracing::info!(?intention, "raccourci d'enregistrement");
    publier(app, Message::Commande(intention));
}

/// Branche le plugin et enregistre les deux raccourcis. Un raccourci déjà
/// pris par une autre application n'empêche pas Kobi de démarrer.
pub fn installer<R: Runtime>(
    app: &AppHandle<R>,
    config: &kobi_session::config::Config,
) -> tauri::Result<()> {
    let (raccourcis, problemes) = Raccourcis::depuis(config);
    for probleme in problemes {
        tracing::warn!(%probleme, "raccourci de la config ignoré");
    }
    app.manage(Actifs(Mutex::new(raccourcis)));
    app.plugin(
        tauri_plugin_global_shortcut::Builder::new()
            .with_handler(|app, raccourci, evenement| {
                if evenement.state() != ShortcutState::Pressed {
                    return;
                }
                let r = *app
                    .state::<Actifs>()
                    .0
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if *raccourci == r.enregistrer {
                    basculer_enregistrement(app);
                } else if *raccourci == r.ecrire {
                    ouvrir_bulle(app);
                } else if *raccourci == r.vue {
                    crate::agrandir::vue(app, "basculer");
                } else if *raccourci == r.interpeller {
                    match crate::interpellation::declencher(app) {
                        Ok(()) => tracing::info!("raccourci global : interpellation"),
                        Err(refus) => tracing::info!(%refus, "raccourci global ignoré"),
                    }
                }
            })
            .build(),
    )?;
    for raccourci in raccourcis.tous() {
        if let Err(e) = app.global_shortcut().register(raccourci) {
            tracing::warn!(raccourci = ?raccourci, erreur = %e, "raccourci global non enregistré");
        } else {
            tracing::info!(raccourci = ?raccourci, "raccourci global enregistré");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobi_session::config::Config;
    use std::cell::RefCell;
    use std::collections::HashSet;
    use tauri_plugin_global_shortcut::{Code, Modifiers};

    /// Faux système : les raccourcis inscrits, et ceux pris ailleurs.
    #[derive(Default)]
    struct FauxClavier {
        inscrits: RefCell<HashSet<Shortcut>>,
        pris: HashSet<Shortcut>,
    }

    impl Clavier for FauxClavier {
        fn inscrire(&self, r: Shortcut) -> Result<(), String> {
            if self.pris.contains(&r) || !self.inscrits.borrow_mut().insert(r) {
                return Err("déjà enregistré".into());
            }
            Ok(())
        }
        fn desinscrire(&self, r: Shortcut) {
            self.inscrits.borrow_mut().remove(&r);
        }
    }

    fn raccourcis(c: &Config) -> Raccourcis {
        Raccourcis::depuis(c).0
    }

    fn installe(c: &Config) -> FauxClavier {
        let clavier = FauxClavier::default();
        for r in raccourcis(c).tous() {
            clavier.inscrire(r).unwrap();
        }
        clavier
    }

    /// KOB-214 : « change ton raccourci en Ctrl+Alt+J » s'applique tout de
    /// suite ; l'ancien ne fait plus rien.
    #[test]
    fn changement_a_chaud() {
        let avant = Config::default();
        let clavier = installe(&avant);
        let mut apres = avant.clone();
        apres.hotkey = "Ctrl+Alt+J".into();
        apres.hotkey_view = "Ctrl+Alt+V".into();
        remplacer(&clavier, raccourcis(&avant), raccourcis(&apres)).unwrap();
        let inscrits = clavier.inscrits.borrow();
        assert_eq!(*inscrits, raccourcis(&apres).tous().into_iter().collect());
        assert!(!inscrits.contains(&raccourcis(&avant).interpeller));
        assert!(inscrits.contains(&Shortcut::new(
            Some(Modifiers::CONTROL | Modifiers::ALT),
            Code::KeyJ
        )));
    }

    /// Combinaison déjà prise (autre application, ou un autre raccourci de
    /// Kobi) : rien ne change, et l'erreur dit laquelle.
    #[test]
    fn combinaison_prise_rien_ne_change() {
        let avant = Config::default();
        let mut clavier = installe(&avant);
        let prise = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyJ);
        clavier.pris.insert(prise);
        let mut apres = avant.clone();
        apres.hotkey_record = "Ctrl+Alt+Q".into();
        apres.hotkey = "Ctrl+Alt+J".into();
        let e = remplacer(&clavier, raccourcis(&avant), raccourcis(&apres)).unwrap_err();
        assert!(
            e.contains("déjà utilisé") && e.contains("reste le raccourci"),
            "{e}"
        );
        assert_eq!(
            *clavier.inscrits.borrow(),
            raccourcis(&avant).tous().into_iter().collect()
        );
        // Le raccourci d'un autre réglage de Kobi : refusé de même.
        let mut double = avant.clone();
        double.hotkey_view = avant.hotkey_write.clone();
        assert!(remplacer(&clavier, raccourcis(&avant), raccourcis(&double)).is_err());
        assert_eq!(
            *clavier.inscrits.borrow(),
            raccourcis(&avant).tous().into_iter().collect()
        );
    }

    #[test]
    fn raccourcis_par_defaut_et_reglables() {
        let c = Config::default();
        let (defaut, probleme) = lire("hotkey", &c.hotkey, &c.hotkey);
        assert!(probleme.is_none());
        assert_eq!(
            defaut,
            Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::Space)
        );
        let (record, _) = lire("hotkey_record", &c.hotkey_record, &c.hotkey_record);
        assert_eq!(
            record,
            Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyR)
        );
        assert_ne!(defaut, record);
        let (ecrire, probleme) = lire("hotkey_write", &c.hotkey_write, &c.hotkey_write);
        assert!(probleme.is_none());
        assert_eq!(
            ecrire,
            Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyK)
        );
        assert_ne!(ecrire, defaut);
        assert_ne!(ecrire, record);
        let (vue, probleme) = lire("hotkey_view", &c.hotkey_view, &c.hotkey_view);
        assert!(probleme.is_none());
        assert_eq!(
            vue,
            Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyG)
        );
        assert!(![defaut, record, ecrire].contains(&vue));
        let (autre, probleme) = lire("hotkey", "Alt+K", &c.hotkey);
        assert!(probleme.is_none());
        assert_eq!(autre, Shortcut::new(Some(Modifiers::ALT), Code::KeyK));

        let (repli, probleme) = lire("hotkey", "Ctrl+Truc", &c.hotkey);
        assert_eq!(repli, defaut);
        assert!(probleme.unwrap().contains("Ctrl+Truc"));
    }
}
