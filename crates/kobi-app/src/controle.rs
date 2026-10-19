//! Interface de contrôle (KOB-161) : exécute dans l'app les commandes de la
//! CLI (`kobi <commande>`), du MCP (`kobi mcp`) et de l'agent de Kobi.
//!
//! Le point d'accès local, le jeton et la vérification des paramètres sont
//! dans `kobi-ctl` ; ici, chaque commande devient un appel aux composants de
//! l'app (chef, écoute, stockage, fenêtres). Rien ne dépend du harness.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kobi_ctl::{Code, Commande, Effet, Erreur, Origine};
use kobi_session::cles::{self, Application, ErreurCle};
use kobi_session::config::{self, Config};
use kobi_session::etat::Evenement;
use kobi_session::stockage::{horodatage, maintenant_ms, ReunionLue, Stockage};
use kobi_stt::Source;
use serde_json::{json, Map, Value};
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::app::Section;
use crate::chef::{ChefPartage, Enregistrement};
use crate::intentions::Intention;

/// Mode discret demandé (interface : pastille de 32 px).
pub const EVENEMENT_DISCRET: &str = "kobi://discret";
/// Réglage changé : `{key, value}`.
pub const EVENEMENT_REGLAGE: &str = "reglage";

/// Changements de réglages gardés pour `config.undo`.
const HISTORIQUE: usize = 20;

/// État de l'interface de contrôle.
pub struct Controle {
    /// `config.toml` (absent seulement sans dossier personnel).
    config: Option<PathBuf>,
    /// (clé, valeur avant) des derniers `config.set`.
    historique: Mutex<Vec<(String, Value)>>,
    discret: AtomicBool,
    /// Le serveur vit autant que l'app.
    serveur: Mutex<Option<kobi_ctl::serveur::Serveur>>,
}

impl Controle {
    pub fn new(config: Option<PathBuf>) -> Self {
        Self {
            config,
            historique: Mutex::default(),
            discret: AtomicBool::new(false),
            serveur: Mutex::default(),
        }
    }
}

/// Ouvre le point d'accès local. Une autre instance qui le tient déjà
/// n'empêche pas Kobi de démarrer (journalisé).
pub fn installer<R: Runtime>(app: &AppHandle<R>, config: Option<PathBuf>) {
    app.manage(Controle::new(config));
    let Some(point) = kobi_ctl::Point::defaut() else {
        tracing::warn!("contrôle : dossier personnel introuvable");
        return;
    };
    match kobi_ctl::serveur::demarrer(point.clone(), gestionnaire(app.clone())) {
        Ok(serveur) => {
            tracing::info!(adresse = %point.adresse, "contrôle : point d'accès ouvert");
            if let Some(c) = app.try_state::<Controle>() {
                *c.serveur.lock().unwrap_or_else(|e| e.into_inner()) = Some(serveur);
            }
        }
        Err(e) => {
            // Deux lancements presque en même temps (KOB-175) : l'autre tient
            // déjà le point d'accès ; on lui passe la demande et on s'arrête.
            let args: Vec<String> = std::env::args().skip(1).collect();
            if crate::instance::transmettre(&point, &args).is_ok() {
                tracing::info!(
                    erreur = %e,
                    "Kobi tourne déjà : demande passée à l'instance en cours, ce lancement s'arrête"
                );
                app.exit(0);
                return;
            }
            tracing::warn!(erreur = %e, adresse = %point.adresse, "contrôle indisponible");
        }
    }
}

pub fn gestionnaire<R: Runtime>(app: AppHandle<R>) -> kobi_ctl::serveur::Gestionnaire {
    Arc::new(move |commande, params, origine| {
        let resultat = executer(&app, commande, params, origine);
        match &resultat {
            Ok(_) => tracing::info!(commande = commande.nom, ?origine, "contrôle"),
            Err(e) => {
                tracing::info!(commande = commande.nom, ?origine, code = ?e.code, erreur = %e.message, "contrôle refusé")
            }
        }
        resultat
    })
}

/// Peut-on exécuter ? La CLI et un MCP branché par l'utilisateur passent
/// toujours. L'agent de Kobi passe seul pour lire, afficher et mener la
/// réunion ; ce qui écrit attend un « oui » de l'utilisateur (KOB-156).
fn autoriser<R: Runtime>(
    app: &AppHandle<R>,
    commande: &Commande,
    params: &Map<String, Value>,
    origine: Origine,
) -> Result<(), Erreur> {
    // Commande interne (le hook des commandes de Claude, KOB-156) : un MCP
    // ne doit pas pouvoir ouvrir de carte en se faisant passer pour Kobi.
    if commande.interne && origine != Origine::Cli {
        return Err(erreur(
            Code::Unauthorized,
            format!("{} est réservée à Kobi", commande.nom),
        ));
    }
    match (origine, commande.effet) {
        (Origine::Cli | Origine::Mcp, _) | (Origine::Agent, Effet::Lecture | Effet::Affichage) => {
            return Ok(())
        }
        // La réunion, sans carte : Kobi annonce l'action et l'utilisateur peut
        // l'annuler (décision du propriétaire, KOB-165).
        _ if commande.pilote_la_reunion() => return Ok(()),
        _ => {}
    }
    let reglage = commande.nom.starts_with("config.");
    if reglage
        && lire_config(app)
            .is_ok_and(|(_, c)| c.permissions.reglage == config::Politique::SansDemander)
    {
        return Ok(());
    }
    match crate::validation::demander(app, carte_de(app, commande, params, reglage)) {
        crate::validation::Reponse::Accepte => Ok(()),
        crate::validation::Reponse::Refuse => Err(erreur(
            Code::NeedsValidation,
            format!(
                "{} : refusé par l'utilisateur, rien n'a été fait",
                commande.nom
            ),
        )),
        crate::validation::Reponse::Delai => Err(erreur(
            Code::NeedsValidation,
            format!(
                "{} : pas de réponse en {} s, refusé, rien n'a été fait",
                commande.nom,
                crate::validation::ECHEANCE.as_secs()
            ),
        )),
    }
}

/// Genre, résumé et détail de la carte de validation d'une commande.
fn carte_de<R: Runtime>(
    app: &AppHandle<R>,
    commande: &Commande,
    params: &Map<String, Value>,
    reglage: bool,
) -> crate::validation::CarteValidation {
    use crate::validation::{CarteValidation, Genre, ECHEANCE};
    if commande.nom == "config.set" {
        let cle = texte(params, "key").unwrap_or_default().to_owned();
        let avant = lire_config(app)
            .ok()
            .and_then(|(_, c)| cles::lire(&c, Some(&cle)).ok())
            .unwrap_or(Value::Null);
        return CarteValidation::reglage(0, &cle, avant, valeur_de(params), ECHEANCE);
    }
    let genre = if reglage { Genre::Reglage } else { Genre::Mcp };
    let outil = format!("mcp__kobi__{}", commande.outil());
    let mut carte =
        CarteValidation::depuis_appel(0, genre, &outil, &Value::Object(params.clone()), ECHEANCE);
    carte.resume = format!(
        "{} ({})",
        commande.description.trim_end_matches('.'),
        commande.nom
    );
    carte
}

/// Valeur de `config.set`. Un agent envoie parfois une chaîne JSON encore
/// encodée (`"\"Ctrl+Alt+J\""`) : on la décode.
fn valeur_de(params: &Map<String, Value>) -> Value {
    match params.get("value") {
        Some(Value::String(t)) if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') => {
            serde_json::from_str::<String>(t)
                .map_or_else(|_| Value::String(t.clone()), Value::String)
        }
        Some(v) => v.clone(),
        None => Value::Null,
    }
}

fn erreur(code: Code, message: impl Into<String>) -> Erreur {
    Erreur::new(code, message)
}

fn texte<'a>(params: &'a Map<String, Value>, nom: &str) -> Option<&'a str> {
    params.get(nom).and_then(Value::as_str)
}

fn limite(params: &Map<String, Value>, defaut: usize) -> usize {
    params
        .get("limit")
        .and_then(Value::as_u64)
        .map_or(defaut, |n| n as usize)
}

fn en_json<T: serde::Serialize>(v: T) -> Result<Value, Erreur> {
    serde_json::to_value(v).map_err(|e| erreur(Code::Internal, e.to_string()))
}

/// Exécute une commande dont les paramètres ont été vérifiés.
pub fn executer<R: Runtime>(
    app: &AppHandle<R>,
    commande: &Commande,
    params: Map<String, Value>,
    origine: Origine,
) -> Result<Value, Erreur> {
    autoriser(app, commande, &params, origine)?;
    match commande.nom {
        "status" => Ok(statut(app)),
        "app.open" => ouvrir_section(
            app,
            texte(&params, "section").unwrap_or("notes"),
            texte(&params, "panneau"),
            texte(&params, "reunion"),
        ),
        "mascot.show" | "mascot.hide" => {
            let w = app
                .get_webview_window("main")
                .ok_or_else(|| erreur(Code::Unavailable, "fenêtre de la mascotte absente"))?;
            let montrer = commande.nom == "mascot.show";
            if montrer { w.show() } else { w.hide() }
                .map_err(|e| erreur(Code::Internal, e.to_string()))?;
            Ok(json!({ "visible": montrer }))
        }
        "mascot.discreet" => {
            let on = params.get("on").and_then(Value::as_bool).unwrap_or(true);
            controle(app)?.discret.store(on, Ordering::Relaxed);
            let _ = app.emit(EVENEMENT_DISCRET, on);
            Ok(json!({ "discret": on }))
        }
        // Vue agrandie de la conversation (KOB-169).
        "view.open" | "view.close" | "view.toggle" => {
            let action = match commande.nom {
                "view.open" => "ouvrir",
                "view.close" => "fermer",
                _ => "basculer",
            };
            crate::agrandir::vue(app, action);
            Ok(json!({ "vue": action }))
        }
        "app.quit" => {
            // Répondre d'abord, quitter ensuite.
            let app = app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(300));
                tracing::info!("contrôle : Kobi quitte");
                app.exit(0);
            });
            Ok(json!({}))
        }
        "config.get" => {
            let c = lire_config(app)?.1;
            cles::lire(&c, texte(&params, "key")).map_err(erreur_cle)
        }
        "config.list" => en_json(cles::liste(&lire_config(app)?.1)),
        "config.set" => {
            let cle = texte(&params, "key").unwrap_or_default();
            let (avant, r) = regler(app, cle, valeur_de(&params))?;
            let c = controle(app)?;
            let mut h = c.historique.lock().unwrap_or_else(|e| e.into_inner());
            h.push((cle.to_owned(), avant));
            let trop = h.len().saturating_sub(HISTORIQUE);
            h.drain(..trop);
            Ok(r)
        }
        "config.undo" => {
            let c = controle(app)?;
            let (cle, avant) = c
                .historique
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .pop()
                .ok_or_else(|| erreur(Code::Conflict, "aucun changement de réglage à annuler"))?;
            regler(app, &cle, avant).map(|(_, r)| r)
        }
        "meeting.start" => reunion(app, Intention::DemarrerReunion, origine),
        "meeting.stop" => reunion(app, Intention::TerminerReunion, origine),
        "meeting.pause" => reunion(app, Intention::Pause, origine),
        "meeting.resume" => reunion(app, Intention::Reprendre, origine),
        "meeting.status" => en_json(etat_reunion(app)),
        "meetings.list" => {
            let mut l = stockage(app)?.reunions().map_err(io)?;
            l.truncate(limite(&params, 20));
            en_json(l)
        }
        "meetings.get" => {
            let st = stockage(app)?;
            let nom = nom_reunion(&st, texte(&params, "id").unwrap_or("last"))?;
            lire_reunion(
                &st,
                &nom,
                texte(&params, "part").unwrap_or("all"),
                texte(&params, "format").unwrap_or("json"),
            )
        }
        "notes.last" => {
            let st = stockage(app)?;
            let nom = nom_reunion(&st, "last")?;
            let lue = st.lire_reunion(&nom).map_err(donnees)?;
            if texte(&params, "format") == Some("json") {
                return Ok(json!({
                    "nom": lue.resume.nom,
                    "titre": lue.resume.titre,
                    "date": lue.resume.date,
                    "en_cours": lue.resume.en_cours,
                    "notes": lue.notes,
                    "compte_rendu": lue.compte_rendu.as_ref().map(|c| &c.brut),
                }));
            }
            Ok(Value::String(notes_markdown(&st, &lue)?))
        }
        "meetings.export" => {
            let st = stockage(app)?;
            let nom = nom_reunion(&st, texte(&params, "id").unwrap_or("last"))?;
            en_json(st.document_reunion(&nom).map_err(donnees)?)
        }
        "search" => en_json(
            stockage(app)?
                .chercher(
                    texte(&params, "query").unwrap_or_default(),
                    limite(&params, 50),
                )
                .map_err(io)?,
        ),
        "conversations.list" => {
            let mut l = stockage(app)?.conversations().map_err(io)?;
            l.truncate(limite(&params, 20));
            en_json(l)
        }
        "conversations.get" => {
            let st = stockage(app)?;
            let id = match texte(&params, "id").unwrap_or("last") {
                "last" => st
                    .conversations()
                    .map_err(io)?
                    .first()
                    .map(|c| c.id.clone())
                    .ok_or_else(|| erreur(Code::NotFound, "aucune conversation enregistrée"))?,
                id => id.to_owned(),
            };
            if texte(&params, "format") == Some("md") {
                return Ok(Value::String(
                    st.document_conversation(&id).map_err(donnees)?.markdown,
                ));
            }
            en_json(st.lire_conversation(&id).map_err(donnees)?)
        }
        "reminders.list" => {
            let tous = params.get("all").and_then(Value::as_bool).unwrap_or(false);
            let l: Vec<_> = stockage(app)?
                .rappels()
                .map_err(io)?
                .into_iter()
                .filter(|r| tous || !r.fait)
                .collect();
            en_json(l)
        }
        "reminders.add" => {
            let texte_rappel = texte(&params, "text").unwrap_or_default().trim();
            if texte_rappel.is_empty() {
                return Err(erreur(Code::InvalidValue, "rappel vide"));
            }
            let echeance = texte(&params, "due").map(echeance_ms).transpose()?;
            en_json(
                stockage(app)?
                    .ajouter_rappel(texte_rappel, echeance, None)
                    .map_err(io)?,
            )
        }
        "reminders.done" => {
            let id = texte(&params, "id").unwrap_or_default();
            if !stockage(app)?.marquer_fait(id).map_err(io)? {
                return Err(erreur(Code::NotFound, format!("rappel introuvable : {id}")));
            }
            Ok(json!({ "id": id, "fait": true }))
        }
        "listen.pause" | "listen.resume" => {
            let pause = commande.nom == "listen.pause";
            if app.try_state::<crate::ecoute::Ecoute>().is_none() {
                return Err(erreur(Code::Unavailable, "l'écoute n'est pas lancée"));
            }
            crate::ecoute::commander(app, crate::ecoute::Commande::Pause(pause))
                .map_err(|e| erreur(Code::Internal, e))?;
            Ok(json!({ "pause": pause }))
        }
        "voice.say" => {
            let texte_dit = texte(&params, "text").unwrap_or_default().trim();
            if texte_dit.is_empty() {
                return Err(erreur(Code::InvalidValue, "texte vide"));
            }
            if app.try_state::<crate::voix::Voix>().is_none() {
                return Err(erreur(
                    Code::Unavailable,
                    "la voix de Kobi n'est pas lancée",
                ));
            }
            if !crate::voix::etat_voix(app).actif {
                return Err(erreur(
                    Code::Conflict,
                    "Kobi répond en texte seul ici : kobi config set voice.assistant true (ou voice.meeting en réunion)",
                ));
            }
            crate::voix::dire(app, texte_dit);
            Ok(json!({ "dit": texte_dit }))
        }
        "validation.ask" => {
            use crate::validation::{CarteValidation, Genre, ECHEANCE};
            // Le genre vient d'une liste fermée (`choix`, vérifiée avant).
            let genre = match texte(&params, "kind").unwrap_or_default() {
                "fichier" => Genre::Fichier,
                "suppression" => Genre::Suppression,
                "mcp" => Genre::Mcp,
                "reglage" => Genre::Reglage,
                _ => Genre::Commande,
            };
            let outil = texte(&params, "tool").unwrap_or_default();
            let entree = params.get("input").cloned().unwrap_or(Value::Null);
            let mut carte = CarteValidation::depuis_appel(0, genre, outil, &entree, ECHEANCE);
            if let Some(resume) = texte(&params, "summary").filter(|r| !r.trim().is_empty()) {
                carte.resume = resume.to_owned();
            }
            let r = crate::validation::demander(app, carte);
            Ok(json!({ "accepte": r.acceptee(), "decision": r.decision() }))
        }
        autre => Err(erreur(
            Code::UnknownCommand,
            format!("commande sans exécution dans l'app : {autre}"),
        )),
    }
}

fn controle<R: Runtime>(app: &AppHandle<R>) -> Result<tauri::State<'_, Controle>, Erreur> {
    app.try_state::<Controle>()
        .ok_or_else(|| erreur(Code::Unavailable, "interface de contrôle non installée"))
}

fn io(e: std::io::Error) -> Erreur {
    erreur(Code::Internal, e.to_string())
}

fn donnees(e: kobi_session::donnees::ErreurDonnees) -> Erreur {
    use kobi_session::donnees::ErreurDonnees as E;
    let code = match e {
        E::Introuvable(_) | E::Nom(_) => Code::NotFound,
        E::EnCours(_) | E::Transition(_) => Code::Conflict,
        E::Io(ref io) if io.kind() == std::io::ErrorKind::NotFound => Code::NotFound,
        E::Io(_) => Code::Internal,
    };
    erreur(code, e.to_string())
}

fn erreur_cle(e: ErreurCle) -> Erreur {
    let code = match e {
        ErreurCle::Inconnue(_) => Code::NotFound,
        ErreurCle::Refusee { .. } => Code::InvalidValue,
    };
    erreur(code, e.to_string())
}

fn stockage<R: Runtime>(app: &AppHandle<R>) -> Result<Stockage, Erreur> {
    crate::stockage::actuel(app)
        .ok_or_else(|| erreur(Code::Unavailable, "dossier de Kobi introuvable"))
}

// ----- État -----

fn enregistrement<R: Runtime>(app: &AppHandle<R>) -> Enregistrement {
    match app.try_state::<ChefPartage<R>>() {
        Some(c) => {
            c.0.lock()
                .unwrap_or_else(|e| e.into_inner())
                .enregistrement()
        }
        // Sans chef (tests) : d'après l'état de la session.
        None => {
            let etat = crate::session::etat(app).etat;
            Enregistrement {
                actif: matches!(etat, "reunion" | "pause"),
                pause: etat == "pause",
                ..Default::default()
            }
        }
    }
}

fn etat_reunion<R: Runtime>(app: &AppHandle<R>) -> Value {
    let e = enregistrement(app);
    let duree = if e.actif && e.debut_ms > 0 {
        maintenant_ms().saturating_sub(e.debut_ms) as f64 / 1000.0
    } else {
        0.0
    };
    let mut v = serde_json::to_value(e).unwrap_or_default();
    v["duree_s"] = json!(duree);
    v
}

fn statut<R: Runtime>(app: &AppHandle<R>) -> Value {
    let session = crate::session::etat(app);
    let visible = app
        .get_webview_window("main")
        .and_then(|w| w.is_visible().ok());
    json!({
        "etat": session.etat,
        "activite": session.activite,
        "mascotte": session.mascotte,
        "transcrit": session.transcrit,
        "reunion": etat_reunion(app),
        "ecoute": {
            "pause": app
                .try_state::<crate::ecoute::EnPause>()
                .map(|p| p.0.load(Ordering::Relaxed)),
        },
        "fenetre": {
            "visible": visible,
            "discret": app.try_state::<Controle>().map(|c| c.discret.load(Ordering::Relaxed)),
        },
        "version": env!("CARGO_PKG_VERSION"),
    })
}

// ----- App -----

/// Section de l'app Kobi (KOB-159) d'après la commande `app.open`. Les noms
/// du contrat (`reunions`, `conversations`, `apropos`) mènent à leur
/// section.
fn section_de(section: &str, panneau: Option<&str>, reunion: Option<&str>) -> Option<Section> {
    let detail = |d: Option<&str>| d.map(|d| format!("/{d}")).unwrap_or_default();
    let adresse = match section {
        "notes" | "reunions" | "conversations" => format!("notes{}", detail(reunion)),
        "reglages" => format!("reglages{}", detail(panneau)),
        "apropos" => "a-propos".into(),
        autre => autre.into(),
    };
    Section::lire(&adresse)
}

/// Ouvre l'app Kobi sur une section, ou la ramène devant sur cette section.
fn ouvrir_section<R: Runtime>(
    app: &AppHandle<R>,
    section: &str,
    panneau: Option<&str>,
    reunion: Option<&str>,
) -> Result<Value, Erreur> {
    let cible = section_de(section, panneau, reunion).ok_or_else(|| {
        erreur(
            Code::InvalidValue,
            "panneau ou réunion illisible : lettres, chiffres, tirets et soulignés seulement",
        )
    })?;
    let adresse = cible.adresse();
    crate::app::ouvrir(app, Some(cible)).map_err(|e| erreur(Code::Internal, e.to_string()))?;
    Ok(json!({ "fenetre": crate::app::FENETRE, "section": adresse }))
}

// ----- Réunion -----

/// Ordre de réunion. Venant de l'agent de Kobi (Claude, KOB-165), c'est un
/// pilotage : un arrêt s'annule pendant 30 s, et Claude l'annonce lui-même.
fn reunion<R: Runtime>(
    app: &AppHandle<R>,
    intention: Intention,
    origine: Origine,
) -> Result<Value, Erreur> {
    let chef = app.try_state::<ChefPartage<R>>();
    let annulable = chef.as_ref().is_some_and(|c| {
        c.0.lock()
            .unwrap_or_else(|e| e.into_inner())
            .fin_annulable()
    });
    let e = enregistrement(app);
    let refus = match intention {
        // « Reprends » juste après un arrêt : la même réunion continue.
        Intention::Reprendre if annulable => None,
        Intention::DemarrerReunion if e.actif => Some("J'enregistre déjà la réunion."),
        Intention::TerminerReunion | Intention::Pause | Intention::Reprendre if !e.actif => {
            Some("Aucune réunion en cours.")
        }
        Intention::Pause if e.pause => Some("La réunion est déjà en pause."),
        Intention::Reprendre if !e.pause => Some("J'enregistre déjà."),
        _ => None,
    };
    if let Some(r) = refus {
        return Err(erreur(Code::Conflict, r));
    }
    let message = match chef {
        Some(chef) => {
            let mut chef = chef.0.lock().unwrap_or_else(|e| e.into_inner());
            match origine {
                Origine::Agent => chef.piloter(intention, false),
                _ => chef.commande(intention),
            }
        }
        None => {
            let evenements: &[Evenement] = match intention {
                Intention::DemarrerReunion => &[Evenement::DemarrerReunion],
                Intention::Pause => &[Evenement::Pause],
                Intention::Reprendre => &[Evenement::Reprendre],
                _ => &[Evenement::TerminerReunion, Evenement::CompteRenduEcrit],
            };
            crate::session::appliquer(app, evenements).map_err(|e| erreur(Code::Conflict, e))?;
            "OK".into()
        }
    };
    Ok(json!({ "message": message, "reunion": etat_reunion(app) }))
}

// ----- Réunions et notes -----

/// Nom d'une réunion, `last` pour la plus récente.
fn nom_reunion(st: &Stockage, id: &str) -> Result<String, Erreur> {
    if id != "last" {
        return Ok(id.to_owned());
    }
    st.reunions()
        .map_err(io)?
        .first()
        .map(|r| r.nom.clone())
        .ok_or_else(|| erreur(Code::NotFound, "aucune réunion enregistrée"))
}

fn transcription_markdown(lue: &ReunionLue) -> String {
    lue.segments
        .iter()
        .map(|s| {
            let qui = match s.source {
                Source::Moi => "Moi",
                Source::Eux => "Eux",
            };
            format!(
                "- `{}` **{qui}** : {}\n",
                horodatage(s.start),
                s.text.trim()
            )
        })
        .collect()
}

fn lire_reunion(st: &Stockage, nom: &str, partie: &str, format: &str) -> Result<Value, Erreur> {
    let lue = st.lire_reunion(nom).map_err(donnees)?;
    let md = format == "md";
    Ok(match (partie, md) {
        ("all", false) => en_json(&lue)?,
        ("all", true) => Value::String(st.document_reunion(nom).map_err(donnees)?.markdown),
        ("summary", false) => en_json(&lue.compte_rendu)?,
        ("summary", true) => Value::String(
            lue.compte_rendu
                .map(|c| c.brut)
                .unwrap_or_else(|| "_Pas encore de compte rendu._".into()),
        ),
        ("notes", false) => json!({ "notes": lue.notes }),
        ("notes", true) => Value::String(lue.notes),
        ("transcript", false) => en_json(&lue.segments)?,
        (_, _) => Value::String(transcription_markdown(&lue)),
    })
}

/// Notes et compte rendu d'une réunion, en markdown.
fn notes_markdown(st: &Stockage, lue: &ReunionLue) -> Result<String, Erreur> {
    let titre = st.document_reunion(&lue.resume.nom).map_err(donnees)?.titre;
    let mut md = format!("# {titre}\n\n");
    if lue.resume.en_cours {
        md.push_str("_Réunion en cours._\n\n");
    }
    md.push_str("## Notes\n\n");
    md.push_str(match lue.notes.trim() {
        "" => "_Pas encore de notes._",
        n => n,
    });
    md.push_str("\n\n## Compte rendu\n\n");
    md.push_str(
        lue.compte_rendu
            .as_ref()
            .map(|c| c.brut.trim())
            .filter(|c| !c.is_empty())
            .unwrap_or("_Pas encore de compte rendu._"),
    );
    md.push('\n');
    Ok(md)
}

// ----- Rappels -----

/// Échéance : ms Unix, date RFC 3339, ou date et heure locales
/// (`2026-10-19T09:00`).
fn echeance_ms(texte: &str) -> Result<u64, Erreur> {
    let texte = texte.trim();
    if let Ok(ms) = texte.parse::<u64>() {
        return Ok(ms);
    }
    let ts = texte.parse::<jiff::Timestamp>().or_else(|_| {
        texte
            .parse::<jiff::civil::DateTime>()
            .and_then(|d| d.to_zoned(jiff::tz::TimeZone::system()))
            .map(|z| z.timestamp())
    });
    ts.ok()
        .and_then(|t| u64::try_from(t.as_millisecond()).ok())
        .ok_or_else(|| {
            erreur(
                Code::InvalidValue,
                format!("échéance illisible : « {texte} » (2026-10-19T09:00 ou 2026-10-19T09:00:00+02:00)"),
            )
        })
}

// ----- Réglages -----

fn lire_config<R: Runtime>(app: &AppHandle<R>) -> Result<(PathBuf, Config), Erreur> {
    let chemin = controle(app)?
        .config
        .clone()
        .ok_or_else(|| erreur(Code::Unavailable, "fichier de réglages introuvable"))?;
    let lu = config::load_or_create(&chemin);
    if let Some(probleme) = lu.problem {
        return Err(erreur(
            Code::Internal,
            format!("config.toml illisible, rien n'est changé : {probleme}"),
        ));
    }
    Ok((chemin, lu.config))
}

/// Modifie, vérifie, écrit et applique un réglage. Renvoie la valeur d'avant
/// et le résultat de la commande.
fn regler<R: Runtime>(
    app: &AppHandle<R>,
    cle: &str,
    valeur: Value,
) -> Result<(Value, Value), Erreur> {
    let (chemin, actuelle) = lire_config(app)?;
    let m = cles::modifier(&actuelle, cle, valeur).map_err(erreur_cle)?;
    if matches!(cle, "hotkey" | "hotkey_record") {
        use std::str::FromStr;
        let texte = m.apres.as_str().unwrap_or_default();
        tauri_plugin_global_shortcut::Shortcut::from_str(texte).map_err(|e| {
            erreur(
                Code::InvalidValue,
                format!("{cle} : raccourci « {texte} » illisible ({e})"),
            )
        })?;
    }
    config::save(&m.config, &chemin).map_err(|e| erreur(Code::Internal, e.to_string()))?;
    let mut application = m.application;
    // Voix (KOB-142) : le réglage se change à chaud.
    if let (Some(actif), true) = (
        m.apres.as_bool(),
        app.try_state::<crate::voix::Voix>().is_some(),
    ) {
        match cle {
            "voice.assistant" => {
                crate::voix::regler(app, Some(actif), None);
                application = Application::AChaud;
            }
            "voice.meeting" => {
                crate::voix::regler(app, None, Some(actif));
                application = Application::AChaud;
            }
            _ => {}
        }
    }
    if cle.starts_with("stockage.") {
        if let Err(e) = crate::stockage::basculer(app, &m.config) {
            tracing::warn!(erreur = %e, "contrôle : dossier écrit, bascule au redémarrage");
            application = Application::AuRedemarrage;
        }
    }
    tracing::info!(cle, avant = %m.avant, apres = %m.apres, ?application, "réglage changé");
    let _ = app.emit(EVENEMENT_REGLAGE, json!({ "key": cle, "value": m.apres }));
    Ok((
        m.avant.clone(),
        json!({ "key": cle, "before": m.avant, "after": m.apres, "apply": application }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobi_ctl::commandes::trouver;
    use kobi_session::stockage::Qui;
    use kobi_stt::Segment;
    use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};

    struct Essai {
        app: tauri::App<MockRuntime>,
        dir: tempfile::TempDir,
        nom: String,
    }

    fn essai() -> Essai {
        let dir = tempfile::tempdir().unwrap();
        let racine = dir.path().join("donnees");
        let mut cfg = Config::default();
        cfg.stockage.dossier = Some(racine.clone());
        let chemin = dir.path().join("config.toml");
        config::save(&cfg, &chemin).unwrap();
        let st = Stockage::new(&racine);
        st.preparer().unwrap();
        let mut r = st
            .nouvelle_reunion(1_792_315_800_000, Some("Point budget"))
            .unwrap();
        r.ecrire_segment(&Segment {
            start: 0.0,
            end: 4.0,
            source: Source::Eux,
            text: "On valide le budget.".into(),
            is_final: true,
        })
        .unwrap();
        r.ecrire_notes("## Décisions\n- budget validé\n").unwrap();
        r.ecrire_compte_rendu("## Résumé\nBudget validé.\n")
            .unwrap();
        r.terminer(1_792_316_400_000).unwrap();
        let nom = r.nom().to_owned();
        let mut c = st.nouvelle_conversation(1_792_320_000_000);
        c.ajouter(Qui::Moi, "Quelle heure à Tokyo ?", true).unwrap();
        let app = crate::session::installer(mock_builder())
            .build(mock_context(noop_assets()))
            .unwrap();
        crate::stockage::installer(app.handle(), &cfg, Some(chemin.clone()));
        app.manage(Controle::new(Some(chemin)));
        Essai { app, dir, nom }
    }

    fn lancer(e: &Essai, nom: &str, params: Value, origine: Origine) -> Result<Value, Erreur> {
        let c = trouver(nom).unwrap();
        let params = c.verifier(&params)?;
        executer(e.app.handle(), c, params, origine)
    }

    fn cli(e: &Essai, nom: &str, params: Value) -> Result<Value, Erreur> {
        lancer(e, nom, params, Origine::Cli)
    }

    #[test]
    fn reunion_demarree_arretee() {
        let e = essai();
        assert_eq!(
            cli(&e, "meeting.status", json!({})).unwrap()["actif"],
            false
        );
        let r = cli(&e, "meeting.start", json!({})).unwrap();
        assert_eq!(r["reunion"]["actif"], true);
        assert_eq!(cli(&e, "status", json!({})).unwrap()["etat"], "reunion");
        assert_eq!(
            cli(&e, "meeting.start", json!({})).unwrap_err().code,
            Code::Conflict
        );
        cli(&e, "meeting.pause", json!({})).unwrap();
        assert_eq!(
            cli(&e, "meeting.pause", json!({})).unwrap_err().code,
            Code::Conflict
        );
        cli(&e, "meeting.resume", json!({})).unwrap();
        cli(&e, "meeting.stop", json!({})).unwrap();
        assert_eq!(cli(&e, "status", json!({})).unwrap()["etat"], "veille");
        assert_eq!(
            cli(&e, "meeting.stop", json!({})).unwrap_err().code,
            Code::Conflict
        );
    }

    #[test]
    fn notes_et_reunions() {
        let e = essai();
        let liste = cli(&e, "meetings.list", json!({})).unwrap();
        assert_eq!(liste[0]["nom"], e.nom);
        let md = cli(&e, "notes.last", json!({})).unwrap();
        let md = md.as_str().unwrap();
        assert!(md.starts_with("# Point budget"), "{md}");
        assert!(
            md.contains("- budget validé") && md.contains("Budget validé."),
            "{md}"
        );
        let j = cli(&e, "notes.last", json!({"format": "json"})).unwrap();
        assert_eq!(j["titre"], "Point budget");
        let t = cli(
            &e,
            "meetings.get",
            json!({"id": "last", "part": "transcript", "format": "md"}),
        )
        .unwrap();
        assert!(t
            .as_str()
            .unwrap()
            .contains("**Eux** : On valide le budget."));
        let s = cli(&e, "meetings.get", json!({"id": e.nom, "part": "summary"})).unwrap();
        assert_eq!(s["resume"], "Budget validé.");
        let tout = cli(&e, "meetings.get", json!({})).unwrap();
        assert_eq!(tout["segments"].as_array().unwrap().len(), 1);
        let ex = cli(&e, "meetings.export", json!({"id": "last"})).unwrap();
        assert!(ex["markdown"]
            .as_str()
            .unwrap()
            .contains("## Transcription"));
        assert_eq!(
            cli(&e, "meetings.get", json!({"id": "absente"}))
                .unwrap_err()
                .code,
            Code::NotFound
        );
        let trouves = cli(&e, "search", json!({"query": "budget"})).unwrap();
        assert!(!trouves.as_array().unwrap().is_empty());
        let conv = cli(&e, "conversations.get", json!({"format": "md"})).unwrap();
        assert!(conv.as_str().unwrap().contains("Tokyo"));
        assert_eq!(
            cli(&e, "conversations.list", json!({}))
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn rappels() {
        let e = essai();
        assert_eq!(cli(&e, "reminders.list", json!({})).unwrap(), json!([]));
        let r = cli(
            &e,
            "reminders.add",
            json!({"text": "Appeler Léa", "due": "2026-10-19T09:00:00+02:00"}),
        )
        .unwrap();
        assert_eq!(r["echeance_ms"], 1_792_393_200_000u64);
        let id = r["id"].as_str().unwrap().to_owned();
        assert_eq!(
            cli(&e, "reminders.list", json!({})).unwrap()[0]["texte"],
            "Appeler Léa"
        );
        cli(&e, "reminders.done", json!({"id": id})).unwrap();
        assert_eq!(cli(&e, "reminders.list", json!({})).unwrap(), json!([]));
        assert_eq!(
            cli(&e, "reminders.list", json!({"all": true}))
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            cli(&e, "reminders.done", json!({"id": "r0"}))
                .unwrap_err()
                .code,
            Code::NotFound
        );
        assert_eq!(
            cli(&e, "reminders.add", json!({"text": "x", "due": "demain"}))
                .unwrap_err()
                .code,
            Code::InvalidValue
        );
        assert!(echeance_ms("2026-10-19T09:00").is_ok());
        assert_eq!(echeance_ms("1792393200000").unwrap(), 1_792_393_200_000);
    }

    #[test]
    fn reglages_modifies_et_annules() {
        let e = essai();
        assert_eq!(
            cli(&e, "config.get", json!({"key": "voice.assistant"})).unwrap(),
            json!(true)
        );
        let r = cli(
            &e,
            "config.set",
            json!({"key": "voice.assistant", "value": false}),
        )
        .unwrap();
        assert_eq!(
            (r["before"].clone(), r["after"].clone()),
            (json!(true), json!(false))
        );
        let ecrit = config::load_or_create(&e.dir.path().join("config.toml")).config;
        assert!(!ecrit.voice.assistant);
        let r = cli(
            &e,
            "config.set",
            json!({"key": "notes.interval_min", "value": 42}),
        )
        .unwrap_err();
        assert_eq!(r.code, Code::InvalidValue);
        assert_eq!(
            cli(
                &e,
                "config.set",
                json!({"key": "hotkey", "value": "Ctrl+Pomme"})
            )
            .unwrap_err()
            .code,
            Code::InvalidValue
        );
        assert_eq!(
            cli(&e, "config.get", json!({"key": "voix"}))
                .unwrap_err()
                .code,
            Code::NotFound
        );
        cli(&e, "config.undo", json!({})).unwrap();
        assert_eq!(
            cli(&e, "config.get", json!({"key": "voice.assistant"})).unwrap(),
            json!(true)
        );
        assert_eq!(
            cli(&e, "config.undo", json!({})).unwrap_err().code,
            Code::Conflict
        );

        // Dossier changé à chaud : la suite va dans le nouveau dossier.
        let ailleurs = e.dir.path().join("ailleurs");
        let r = cli(
            &e,
            "config.set",
            json!({"key": "stockage.dossier", "value": ailleurs}),
        )
        .unwrap();
        assert_eq!(r["apply"], "hot");
        assert_eq!(
            crate::stockage::actuel(e.app.handle()).unwrap().racine(),
            ailleurs
        );
        assert_eq!(cli(&e, "meetings.list", json!({})).unwrap(), json!([]));
        let liste = cli(&e, "config.list", json!({})).unwrap();
        assert!(liste
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["key"] == "codebases"));
    }

    #[test]
    fn valeur_encodee_deux_fois() {
        let p = |v: Value| json!({ "value": v }).as_object().unwrap().clone();
        assert_eq!(valeur_de(&p(json!("\"Ctrl+Alt+J\""))), json!("Ctrl+Alt+J"));
        assert_eq!(valeur_de(&p(json!("Ctrl+Alt+J"))), json!("Ctrl+Alt+J"));
        assert_eq!(valeur_de(&p(json!(true))), json!(true));
        assert_eq!(valeur_de(&p(json!("\""))), json!("\""));
        assert_eq!(valeur_de(&Map::new()), Value::Null);
    }

    /// Répond aux cartes de validation de l'essai : `Some(oui)`, ou rien.
    fn repondre_aux_cartes(e: &Essai, reponse: Option<bool>) {
        use tauri::Listener;
        let h = e.app.handle().clone();
        e.app.listen(crate::chef::EVENEMENT_CARTE, move |ev| {
            let carte: Value = serde_json::from_str(ev.payload()).unwrap();
            // Une carte qui attend n'a pas encore de `statut`.
            if let (Some(oui), None) = (reponse, carte.get("statut")) {
                let id = carte["id"].as_str().unwrap().to_owned();
                let h = h.clone();
                std::thread::spawn(move || crate::validation::decider(&h, &id, oui));
            }
        });
    }

    #[test]
    fn validation_demandee_par_le_hook() {
        let demande =
            json!({"kind": "commande", "tool": "Bash", "input": {"command": "cargo fmt"}});
        let essai_avec = |reponse: Option<bool>| {
            let e = essai();
            e.app
                .manage(crate::validation::Validations::new(Duration::from_millis(
                    300,
                )));
            repondre_aux_cartes(&e, reponse);
            e
        };
        let e = essai_avec(Some(true));
        // Réservée à Kobi : ni l'agent ni un MCP externe n'ouvrent de carte.
        for origine in [Origine::Agent, Origine::Mcp] {
            let err = lancer(&e, "validation.ask", demande.clone(), origine).unwrap_err();
            assert_eq!(err.code, Code::Unauthorized);
        }
        assert_eq!(
            cli(&e, "validation.ask", demande.clone()).unwrap(),
            json!({"accepte": true, "decision": "accepte"})
        );
        let e = essai_avec(Some(false));
        assert_eq!(
            cli(&e, "validation.ask", demande.clone()).unwrap()["decision"],
            "refuse"
        );
        let e = essai_avec(None);
        assert_eq!(
            cli(&e, "validation.ask", demande).unwrap(),
            json!({"accepte": false, "decision": "delai"})
        );
    }

    #[test]
    fn l_agent_ecrit_apres_validation() {
        let e = essai();
        e.app
            .manage(crate::validation::Validations::new(Duration::from_millis(
                300,
            )));
        let agent = |nom, p| lancer(&e, nom, p, Origine::Agent);
        // Lecture et affichage passent seuls.
        assert!(agent("notes.last", json!({})).is_ok());
        assert!(agent("status", json!({})).is_ok());
        assert!(agent("mascot.discreet", json!({"on": true})).is_ok());
        // La réunion passe sans carte (décision du propriétaire) : aucune
        // demande de validation, quel que soit le résultat ici sans chef.
        for nom in [
            "meeting.start",
            "meeting.pause",
            "meeting.resume",
            "meeting.stop",
        ] {
            if let Err(err) = agent(nom, json!({})) {
                assert_ne!(err.code, Code::NeedsValidation, "{nom}");
            }
        }
        // Sans réponse : refus à l'échéance, rien n'est écrit.
        let mettre = json!({"key": "voice.assistant", "value": false});
        for (nom, p) in [("config.set", mettre.clone()), ("app.quit", json!({}))] {
            let err = agent(nom, p).unwrap_err();
            assert_eq!(err.code, Code::NeedsValidation, "{nom}");
            assert!(err.message.contains("pas de réponse"), "{}", err.message);
        }
        // Rien n'a bougé.
        assert_eq!(cli(&e, "status", json!({})).unwrap()["etat"], "veille");
        assert_eq!(
            cli(&e, "config.get", json!({"key": "voice.assistant"})).unwrap(),
            json!(true)
        );
        // « oui » : appliqué ; « non » : refusé.
        repondre_aux_cartes(&e, Some(true));
        assert!(agent("config.set", mettre).is_ok());
        assert_eq!(
            cli(&e, "config.get", json!({"key": "voice.assistant"})).unwrap(),
            json!(false)
        );
    }

    #[test]
    fn refus_de_l_utilisateur_et_reglage_sans_demander() {
        let e = essai();
        e.app
            .manage(crate::validation::Validations::new(Duration::from_secs(5)));
        repondre_aux_cartes(&e, Some(false));
        let agent = |nom, p| lancer(&e, nom, p, Origine::Agent);
        let err = agent(
            "config.set",
            json!({"key": "voice.assistant", "value": false}),
        )
        .unwrap_err();
        assert_eq!(err.code, Code::NeedsValidation);
        assert!(err.message.contains("refusé par l'utilisateur"));
        assert_eq!(
            cli(&e, "config.get", json!({"key": "voice.assistant"})).unwrap(),
            json!(true)
        );
        // `permissions.reglage = "sans_demander"` : les réglages passent seuls.
        cli(
            &e,
            "config.set",
            json!({"key": "permissions.reglage", "value": "sans_demander"}),
        )
        .unwrap();
        assert!(agent(
            "config.set",
            json!({"key": "voice.assistant", "value": false})
        )
        .is_ok());
        // L'utilisateur et un agent branché par lui passent.
        assert!(lancer(&e, "mascot.discreet", json!({"on": true}), Origine::Mcp).is_ok());
        assert_eq!(
            cli(&e, "status", json!({})).unwrap()["fenetre"]["discret"],
            true
        );
    }

    /// KOB-165 : l'agent de Kobi pilote la réunion sans validation.
    #[test]
    fn l_agent_pilote_la_reunion() {
        let e = essai();
        let agent = |nom| lancer(&e, nom, json!({}), Origine::Agent);
        assert_eq!(agent("meeting.start").unwrap()["reunion"]["actif"], true);
        agent("meeting.pause").unwrap();
        assert_eq!(cli(&e, "status", json!({})).unwrap()["etat"], "pause");
        agent("meeting.resume").unwrap();
        agent("meeting.stop").unwrap();
        assert_eq!(
            agent("meeting.stop").unwrap_err().code,
            Code::Conflict,
            "plus de réunion à arrêter"
        );
    }

    #[test]
    fn sans_composant_reponse_claire() {
        let e = essai();
        assert_eq!(
            cli(&e, "listen.pause", json!({})).unwrap_err().code,
            Code::Unavailable
        );
        assert_eq!(
            cli(&e, "voice.say", json!({"text": "bonjour"}))
                .unwrap_err()
                .code,
            Code::Unavailable
        );
        assert_eq!(
            cli(&e, "mascot.show", json!({})).unwrap_err().code,
            Code::Unavailable
        );
    }

    /// `app.open` vise les sections de l'app (KOB-159), noms du contrat compris.
    #[test]
    fn sections_de_l_app() {
        let s =
            |section, panneau, reunion| section_de(section, panneau, reunion).map(|s| s.adresse());
        assert_eq!(s("notes", None, None).as_deref(), Some("notes"));
        assert_eq!(
            s("reunions", None, Some("2026-10-18-point")).as_deref(),
            Some("notes/2026-10-18-point")
        );
        assert_eq!(s("conversations", None, None).as_deref(), Some("notes"));
        assert_eq!(s("rappels", None, None).as_deref(), Some("rappels"));
        assert_eq!(s("activite", None, None).as_deref(), Some("activite"));
        assert_eq!(
            s("reglages", Some("stockage"), None).as_deref(),
            Some("reglages/stockage")
        );
        assert_eq!(s("apropos", None, None).as_deref(), Some("a-propos"));
        assert_eq!(s("a-propos", None, None).as_deref(), Some("a-propos"));
        assert_eq!(s("accueil", None, None).as_deref(), Some("accueil"));
        assert_eq!(s("reglages", Some("../index"), None), None);
        // Toutes les sections permises par la CLI mènent quelque part.
        for section in kobi_ctl::commandes::SECTIONS {
            assert!(section_de(section, None, None).is_some(), "{section}");
        }
    }

    #[test]
    fn app_ouverte_sur_la_section() {
        let e = essai();
        let r = cli(
            &e,
            "app.open",
            json!({"section": "reglages", "panneau": "stockage"}),
        )
        .unwrap();
        assert_eq!(r, json!({"fenetre": "app", "section": "reglages/stockage"}));
        assert!(e.app.get_webview_window(crate::app::FENETRE).is_some());
        // Déjà ouverte : ramenée devant, sur la nouvelle section.
        let r = cli(&e, "app.open", json!({"section": "rappels"})).unwrap();
        assert_eq!(r["section"], "rappels");
        assert_eq!(
            cli(
                &e,
                "app.open",
                json!({"section": "notes", "reunion": "a b"})
            )
            .unwrap_err()
            .code,
            Code::InvalidValue
        );
    }

    /// Voix (KOB-142) : le réglage change à chaud, `voice say` suit le réglage.
    #[test]
    fn voix_reglee_a_chaud() {
        let e = essai();
        crate::voix::installer_sans_parleur(e.app.handle(), &Config::default().voice);
        let r = cli(
            &e,
            "config.set",
            json!({"key": "voice.assistant", "value": false}),
        )
        .unwrap();
        assert_eq!(r["apply"], "hot");
        assert!(!crate::voix::etat_voix(e.app.handle()).quotidien);
        assert_eq!(
            cli(&e, "voice.say", json!({"text": "bonjour"}))
                .unwrap_err()
                .code,
            Code::Conflict
        );
        cli(&e, "config.undo", json!({})).unwrap();
        assert!(crate::voix::etat_voix(e.app.handle()).quotidien);
        assert_eq!(
            cli(&e, "voice.say", json!({"text": "bonjour"})).unwrap()["dit"],
            "bonjour"
        );
    }

    /// De bout en bout : le vrai point d'accès, le client de la CLI et le
    /// serveur MCP, sur l'app (simulée) et ses données.
    #[test]
    fn cli_et_mcp_par_le_point_d_acces() {
        let e = essai();
        let point = kobi_ctl::Point::dans(e.dir.path(), &format!("app-{}", std::process::id()));
        let _serveur =
            kobi_ctl::serveur::demarrer(point.clone(), gestionnaire(e.app.handle().clone()))
                .unwrap();
        let appel = |nom: &str, p: Value, o| kobi_ctl::client::appeler(&point, nom, p, o);
        // CLI : `kobi meeting start`, `kobi notes last --format md`.
        let args: Vec<String> = ["notes", "last", "--format", "md"].map(String::from).into();
        let kobi_ctl::cli::Demande::Commande {
            commande, params, ..
        } = kobi_ctl::cli::analyser(&args).unwrap()
        else {
            panic!()
        };
        let md = appel(commande.nom, params, Origine::Cli).unwrap();
        assert!(md.as_str().unwrap().contains("budget validé"));
        assert_eq!(
            appel("meeting.start", json!({}), Origine::Cli).unwrap()["reunion"]["actif"],
            true
        );
        assert_eq!(
            appel("meeting.start", json!({}), Origine::Cli)
                .unwrap_err()
                .code
                .sortie(),
            1
        );
        // MCP en mode agent : lit les notes ; ce qui écrit attend une
        // validation, refusée ici (pas de carte possible dans cet essai).
        let mcp = |message: Value| {
            kobi_ctl::mcp::traiter(&message, kobi_ctl::mcp::Mode::Agent, &|c, p, o| {
                kobi_ctl::client::appeler(&point, c.nom, p, o)
            })
            .unwrap()
        };
        let r = mcp(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "notes_last", "arguments": {}}}));
        assert_eq!(r["result"]["isError"], false);
        assert!(r["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Point budget"));
        let r = mcp(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": "config_set", "arguments": {"key": "voice.assistant", "value": false}}}));
        assert_eq!(r["result"]["isError"], true, "{r}");
        // Même outil en mode externe : passe.
        let r = kobi_ctl::mcp::traiter(
            &json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
                "params": {"name": "meeting_stop", "arguments": {}}}),
            kobi_ctl::mcp::Mode::Externe,
            &|c, p, o| kobi_ctl::client::appeler(&point, c.nom, p, o),
        )
        .unwrap();
        assert_eq!(r["result"]["isError"], false, "{r}");
        assert_eq!(
            appel("status", json!({}), Origine::Cli).unwrap()["etat"],
            "veille"
        );
        // Un agent qui contourne le MCP (`origin: agent` à la main) passe
        // aussi par la validation.
        let e2 = appel(
            "config.set",
            json!({"key": "voice.assistant", "value": false}),
            Origine::Agent,
        )
        .unwrap_err();
        assert_eq!(e2.code, Code::NeedsValidation);
    }
}
