//! Une seule instance de Kobi (KOB-175). Un second lancement (raccourci du
//! menu Démarrer, `kobi --app`, double clic sur l'exécutable…) ne démarre pas
//! un deuxième Kobi : il passe sa demande à celui qui tourne, par le point
//! d'accès de contrôle (KOB-161), puis s'arrête. Deux Kobi en même temps se
//! disputaient ce point d'accès, les raccourcis globaux et le journal, et le
//! MCP de Claude parlait au mauvais.
//!
//! La CLI (`kobi status`, `kobi mcp`…) n'est pas concernée : elle parle déjà
//! à l'instance qui tourne.

use serde_json::{json, Value};

/// Section de l'app ouverte par `--app` : la commande `app.open` du point
/// d'accès exige une section (KOB-186).
const SECTION_APP: &str = "notes";

/// Commande de contrôle qui rend, dans l'instance qui tourne, ce que le
/// second lancement demandait par ses arguments : l'app (ou une de ses
/// sections), sinon Kobi lui-même, remis à l'écran.
pub fn demande(args: &[String]) -> (&'static str, Value) {
    for arg in args {
        let section = match arg.as_str() {
            "--app" | "--carnet" => SECTION_APP,
            "--reglages" => "reglages",
            "--bienvenue" => "accueil",
            _ => continue,
        };
        return ("app.open", json!({ "section": section }));
    }
    ("mascot.show", json!({}))
}

/// Ce que l'instance qui tourne a fait de la demande de ce lancement.
#[derive(Debug)]
pub enum Passage {
    /// Aucune instance ne répond : ce lancement démarre Kobi.
    Absente,
    /// L'instance a pris la demande : ce lancement s'arrête.
    Prise,
    /// L'instance répond mais refuse la demande. Ce lancement s'arrête
    /// quand même : un second Kobi ne doit jamais démarrer (KOB-186).
    Refusee(kobi_ctl::Erreur),
}

/// Passe la demande de ce lancement à l'instance qui tourne déjà, s'il y en
/// a une. Seule l'absence de réponse (`NotRunning`) laisse démarrer Kobi.
pub fn passer(point: &kobi_ctl::Point, args: &[String]) -> Passage {
    let (commande, params) = demande(args);
    #[cfg(windows)]
    ceder_premier_plan();
    match kobi_ctl::client::appeler(point, commande, params, kobi_ctl::Origine::Cli) {
        Ok(_) => Passage::Prise,
        Err(e) if e.code == kobi_ctl::Code::NotRunning => Passage::Absente,
        Err(e) => Passage::Refusee(e),
    }
}

/// Windows ne laisse passer au premier plan que le processus lancé par
/// l'action de l'utilisateur (raccourci « App Kobi », double clic) : c'est ce
/// second lancement, pas l'instance qui tourne, et il s'arrête aussitôt. Sans
/// ce droit cédé, l'app s'ouvrait derrière la fenêtre active (KOB-201). Le
/// pid de l'instance n'est pas connu ici : tout processus peut le prendre,
/// jusqu'à la prochaine action de l'utilisateur.
#[cfg(windows)]
fn ceder_premier_plan() {
    #[link(name = "user32")]
    extern "system" {
        fn AllowSetForegroundWindow(pid: u32) -> i32;
    }
    const ASFW_ANY: u32 = u32::MAX;
    // Refusé si ce lancement n'a pas lui-même le droit (lancé en arrière-plan) :
    // rien à faire de plus, l'app s'ouvre comme avant.
    if unsafe { AllowSetForegroundWindow(ASFW_ANY) } == 0 {
        tracing::debug!("premier plan non cédé à l'instance en cours");
    }
}

/// Note dans le journal de Kobi (celui de l'instance qui tourne, ouvert en
/// ajout) que ce lancement a passé la main, ou que sa demande a été refusée.
pub fn noter_passage(args: &[String], passage: &Passage) {
    if let Some(dir) = kobi_session::logs::default_dir() {
        let _ = kobi_session::logs::init(&dir);
    }
    let (commande, _) = demande(args);
    match passage {
        Passage::Refusee(e) => tracing::warn!(
            commande,
            args = ?args,
            erreur = %e.message,
            "Kobi tourne déjà mais a refusé la demande de ce second lancement, qui s'arrête"
        ),
        _ => tracing::info!(
            commande,
            args = ?args,
            "Kobi tourne déjà : demande passée à l'instance en cours, ce second lancement s'arrête"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn arguments_du_lancement_traduits_en_commande() {
        let section = |s: &str| ("app.open", json!({ "section": s }));
        assert_eq!(demande(&args(&["--app"])), section("notes"));
        assert_eq!(demande(&args(&["--carnet"])), section("notes"));
        assert_eq!(demande(&args(&["--reglages"])), section("reglages"));
        assert_eq!(demande(&args(&["--bienvenue"])), section("accueil"));
        // Sans argument (ou des inconnus) : Kobi revient à l'écran.
        assert_eq!(demande(&[]), ("mascot.show", json!({})));
        assert_eq!(demande(&args(&["--inconnu"])), ("mascot.show", json!({})));
        // Le premier argument reconnu compte.
        assert_eq!(
            demande(&args(&["--inconnu", "--reglages", "--app"])),
            section("reglages")
        );
    }

    /// Commandes reçues par le serveur d'essai : nom et paramètres.
    type Recu = std::sync::Arc<std::sync::Mutex<Vec<(String, Value)>>>;

    /// Serveur de contrôle réel (avec la vérification des paramètres de
    /// kobi-ctl) qui note les commandes reçues ; `refus` : il répond par une
    /// erreur.
    fn serveur(point: &kobi_ctl::Point, refus: bool) -> (kobi_ctl::serveur::Serveur, Recu) {
        use std::sync::Arc;
        let recu: Recu = Arc::default();
        let r = recu.clone();
        let serveur = kobi_ctl::serveur::demarrer(
            point.clone(),
            Arc::new(
                move |commande: &'static kobi_ctl::Commande, params, _origine| {
                    r.lock()
                        .unwrap()
                        .push((commande.nom.to_owned(), Value::Object(params)));
                    if refus {
                        Err(kobi_ctl::Erreur::new(kobi_ctl::Code::Internal, "refusé"))
                    } else {
                        Ok(json!({}))
                    }
                },
            ),
        )
        .unwrap();
        (serveur, recu)
    }

    /// Non-régression de KOB-186 : chaque argument du lancement passe la
    /// vérification des paramètres du point d'accès et arrive à l'instance.
    #[test]
    fn chaque_argument_est_pris_par_l_instance_qui_tourne() {
        let dossier = tempfile::tempdir().unwrap();
        let point = kobi_ctl::Point::dans(dossier.path(), "instance");
        assert!(matches!(
            passer(&point, &args(&["--app"])),
            Passage::Absente
        ));

        let (serveur, recu) = serveur(&point, false);
        for a in [
            &["--app"][..],
            &["--reglages"],
            &["--carnet"],
            &["--bienvenue"],
            &[],
        ] {
            let passage = passer(&point, &args(a));
            assert!(matches!(passage, Passage::Prise), "{a:?} : {passage:?}");
        }
        let recu = recu.lock().unwrap();
        assert_eq!(
            recu.iter()
                .map(|(c, p)| (c.as_str(), p.clone()))
                .collect::<Vec<_>>(),
            [
                ("app.open", json!({ "section": "notes" })),
                ("app.open", json!({ "section": "reglages" })),
                ("app.open", json!({ "section": "notes" })),
                ("app.open", json!({ "section": "accueil" })),
                ("mascot.show", json!({})),
            ]
        );
        drop(serveur);
    }

    /// Une instance qui répond par une erreur tourne bien : le second
    /// lancement ne doit pas démarrer un deuxième Kobi.
    #[test]
    fn instance_qui_refuse_n_est_pas_absente() {
        let dossier = tempfile::tempdir().unwrap();
        let point = kobi_ctl::Point::dans(dossier.path(), "refus");
        let (serveur, _) = serveur(&point, true);
        assert!(matches!(
            passer(&point, &args(&["--app"])),
            Passage::Refusee(_)
        ));
        drop(serveur);
    }
}
