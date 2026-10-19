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

/// Commande de contrôle qui rend, dans l'instance qui tourne, ce que le
/// second lancement demandait par ses arguments : l'app (ou une de ses
/// sections), sinon Kobi lui-même, remis à l'écran.
pub fn demande(args: &[String]) -> (&'static str, Value) {
    for arg in args {
        let section = match arg.as_str() {
            "--app" => None,
            "--carnet" => Some("notes"),
            "--reglages" => Some("reglages"),
            "--bienvenue" => Some("accueil"),
            _ => continue,
        };
        return match section {
            Some(s) => ("app.open", json!({ "section": s })),
            None => ("app.open", json!({})),
        };
    }
    ("mascot.show", json!({}))
}

/// Passe la demande de ce lancement à l'instance qui tourne déjà. `Ok` :
/// elle l'a prise, ce lancement doit s'arrêter ; `Err` : aucune instance ne
/// répond (Kobi ne tourne pas), ce lancement démarre Kobi.
pub fn transmettre(point: &kobi_ctl::Point, args: &[String]) -> Result<Value, kobi_ctl::Erreur> {
    let (commande, params) = demande(args);
    kobi_ctl::client::appeler(point, commande, params, kobi_ctl::Origine::Cli)
}

/// Note dans le journal de Kobi que ce lancement a passé la main (le
/// journal de l'instance qui tourne, qu'on ouvre en ajout).
pub fn noter_passage(args: &[String]) {
    if let Some(dir) = kobi_session::logs::default_dir() {
        let _ = kobi_session::logs::init(&dir);
    }
    let (commande, _) = demande(args);
    tracing::info!(
        commande,
        args = ?args,
        "Kobi tourne déjà : demande passée à l'instance en cours, ce second lancement s'arrête"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn arguments_du_lancement_traduits_en_commande() {
        assert_eq!(demande(&args(&["--app"])), ("app.open", json!({})));
        assert_eq!(
            demande(&args(&["--reglages"])),
            ("app.open", json!({ "section": "reglages" }))
        );
        assert_eq!(
            demande(&args(&["--carnet"])),
            ("app.open", json!({ "section": "notes" }))
        );
        assert_eq!(
            demande(&args(&["--bienvenue"])),
            ("app.open", json!({ "section": "accueil" }))
        );
        // Sans argument (ou des inconnus) : Kobi revient à l'écran.
        assert_eq!(demande(&[]), ("mascot.show", json!({})));
        assert_eq!(demande(&args(&["--inconnu"])), ("mascot.show", json!({})));
        // Le premier argument reconnu compte.
        assert_eq!(
            demande(&args(&["--inconnu", "--reglages", "--app"])),
            ("app.open", json!({ "section": "reglages" }))
        );
    }

    /// Une instance qui tourne reçoit la demande du second lancement ;
    /// sans instance, rien ne répond et Kobi démarre.
    #[test]
    fn second_lancement_passe_la_main() {
        use std::sync::{Arc, Mutex};

        let dossier = tempfile::tempdir().unwrap();
        let point = kobi_ctl::Point::dans(dossier.path(), "instance");
        assert!(transmettre(&point, &args(&["--reglages"])).is_err());

        let recu: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
        let r = recu.clone();
        let serveur = kobi_ctl::serveur::demarrer(
            point.clone(),
            Arc::new(
                move |commande: &'static kobi_ctl::Commande, params, _origine| {
                    r.lock()
                        .unwrap()
                        .push((commande.nom.to_owned(), Value::Object(params)));
                    Ok(json!({ "section": "reglages" }))
                },
            ),
        )
        .unwrap();
        transmettre(&point, &args(&["--reglages"])).unwrap();
        assert_eq!(
            recu.lock().unwrap().as_slice(),
            [("app.open".to_owned(), json!({ "section": "reglages" }))]
        );
        drop(serveur);
    }
}
