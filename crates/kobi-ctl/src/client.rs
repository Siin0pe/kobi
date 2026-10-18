//! Côté client (CLI, MCP) : envoie une requête à l'app et lit la réponse.

use std::io::{BufRead, BufReader, Read, Write};
#[cfg(windows)]
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::protocole::{Code, Erreur, Origine, Reponse, Requete, LIGNE_MAX, VERSION};
use crate::transport::Point;

/// Temps laissé au canal pour se libérer quand toutes ses instances sont
/// prises (requêtes simultanées).
#[cfg(windows)]
const CANAL_OCCUPE: Duration = Duration::from_secs(3);

fn pas_lance(detail: impl std::fmt::Display) -> Erreur {
    Erreur::new(
        Code::NotRunning,
        format!("Kobi ne tourne pas, lancez l'app Kobi ({detail})"),
    )
}

#[cfg(windows)]
fn connecter(adresse: &str) -> Result<std::fs::File, Erreur> {
    // ERROR_PIPE_BUSY : toutes les instances du canal servent déjà.
    const OCCUPE: i32 = 231;
    let debut = Instant::now();
    loop {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(adresse)
        {
            Ok(f) => return Ok(f),
            Err(e) if e.raw_os_error() == Some(OCCUPE) && debut.elapsed() < CANAL_OCCUPE => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(pas_lance(e)),
        }
    }
}

#[cfg(unix)]
fn connecter(adresse: &str) -> Result<std::os::unix::net::UnixStream, Erreur> {
    std::os::unix::net::UnixStream::connect(adresse).map_err(pas_lance)
}

/// Lit le jeton de l'app ; absent, Kobi ne tourne pas.
fn jeton(point: &Point) -> Result<String, Erreur> {
    std::fs::read_to_string(&point.jeton)
        .map(|t| t.trim().to_owned())
        .map_err(|e| pas_lance(format!("{} : {e}", point.jeton.display())))
}

/// Envoie `commande` à l'app et attend sa réponse. Les paramètres ne sont
/// pas vérifiés ici : l'app le fait (et la CLI ou le MCP avant elle).
pub fn appeler(
    point: &Point,
    commande: &str,
    params: Value,
    origine: Origine,
) -> Result<Value, Erreur> {
    let requete = Requete {
        v: VERSION,
        token: jeton(point)?,
        command: commande.to_owned(),
        params,
        origin: origine,
    };
    let mut ligne =
        serde_json::to_string(&requete).map_err(|e| Erreur::new(Code::Internal, e.to_string()))?;
    ligne.push('\n');
    let mut flux = connecter(&point.adresse)?;
    let coupe = |e: std::io::Error| {
        Erreur::new(Code::NotRunning, format!("liaison avec Kobi coupée : {e}"))
    };
    flux.write_all(ligne.as_bytes()).map_err(coupe)?;
    flux.flush().map_err(coupe)?;
    let mut reponse = String::new();
    BufReader::new(flux.take(LIGNE_MAX as u64))
        .read_line(&mut reponse)
        .map_err(coupe)?;
    if reponse.trim().is_empty() {
        return Err(Erreur::new(Code::NotRunning, "Kobi n'a pas répondu"));
    }
    serde_json::from_str::<Reponse>(&reponse)
        .map_err(|e| Erreur::new(Code::Internal, format!("réponse illisible : {e}")))?
        .resultat()
}
