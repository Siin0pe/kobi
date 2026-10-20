//! Côté app : ouvre le point d'accès local, vérifie jeton et paramètres, et
//! passe chaque commande au gestionnaire de l'app.
//!
//! Le serveur tourne sur son propre fil, avec un petit runtime tokio ; le
//! gestionnaire est appelé hors du runtime (`spawn_blocking`) et peut donc
//! bloquer (ouvrir une fenêtre, lire le disque).

use std::io;
use std::sync::Arc;

use serde_json::{Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::oneshot;

use crate::commandes::{self, Commande};
use crate::protocole::{Code, Erreur, Origine, Reponse, Requete, LIGNE_MAX, VERSION};
use crate::transport::Point;

/// Exécute une commande dont les paramètres ont été vérifiés.
pub type Gestionnaire = Arc<
    dyn Fn(&'static Commande, Map<String, Value>, Origine) -> Result<Value, Erreur> + Send + Sync,
>;

/// Serveur en cours ; arrêté (jeton et socket effacés) quand on le lâche.
pub struct Serveur {
    point: Point,
    jeton: String,
    arret: Option<oneshot::Sender<()>>,
    fil: Option<std::thread::JoinHandle<()>>,
}

impl Serveur {
    pub fn point(&self) -> &Point {
        &self.point
    }
}

impl Drop for Serveur {
    fn drop(&mut self) {
        if let Some(arret) = self.arret.take() {
            let _ = arret.send(());
        }
        if let Some(fil) = self.fil.take() {
            let _ = fil.join();
        }
        // N'efface que notre jeton : une autre instance a pu le remplacer.
        if std::fs::read_to_string(&self.point.jeton).is_ok_and(|t| t.trim() == self.jeton) {
            let _ = std::fs::remove_file(&self.point.jeton);
        }
        #[cfg(unix)]
        let _ = std::fs::remove_file(&self.point.adresse);
    }
}

/// 32 octets aléatoires, en hexadécimal.
fn nouveau_jeton() -> io::Result<String> {
    let mut octets = [0u8; 32];
    getrandom::fill(&mut octets).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(octets.iter().map(|o| format!("{o:02x}")).collect())
}

fn ecrire_jeton(point: &Point, jeton: &str) -> io::Result<()> {
    if let Some(dossier) = point.jeton.parent() {
        std::fs::create_dir_all(dossier)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    io::Write::write_all(&mut options.open(&point.jeton)?, jeton.as_bytes())
}

/// Comparaison en temps constant.
fn meme_jeton(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

/// Vérifie la requête et l'exécute.
fn traiter(requete: Requete, jeton: &str, gestionnaire: &Gestionnaire) -> Result<Value, Erreur> {
    if !meme_jeton(&requete.token, jeton) {
        return Err(Erreur::new(
            Code::Unauthorized,
            kobi_i18n::tr("ctl.serveur.jetonRefuse", &[]),
        ));
    }
    if requete.v != VERSION {
        return Err(Erreur::new(
            Code::InvalidParams,
            kobi_i18n::tr(
                "ctl.serveur.protocole",
                &[
                    ("demandee", &requete.v.to_string()),
                    ("version", &VERSION.to_string()),
                ],
            ),
        ));
    }
    let commande = commandes::trouver(&requete.command).ok_or_else(|| {
        Erreur::new(
            Code::UnknownCommand,
            kobi_i18n::tr(
                "ctl.cli.commandeInconnue",
                &[("commande", &requete.command)],
            ),
        )
    })?;
    let params = commande.verifier(&requete.params)?;
    gestionnaire(commande, params, requete.origin)
}

async fn servir<S: AsyncRead + AsyncWrite + Unpin>(
    flux: S,
    jeton: Arc<String>,
    gestionnaire: Gestionnaire,
) {
    let (lecture, mut ecriture) = tokio::io::split(flux);
    let mut ligne = String::new();
    let lu = BufReader::new(lecture.take(LIGNE_MAX as u64))
        .read_line(&mut ligne)
        .await;
    let reponse = match lu {
        Err(e) => Reponse::de(Err(Erreur::new(Code::InvalidParams, e.to_string()))),
        Ok(_) => match serde_json::from_str::<Requete>(&ligne) {
            Err(e) => Reponse::de(Err(Erreur::new(
                Code::InvalidParams,
                format!("requête illisible : {e}"),
            ))),
            Ok(requete) => {
                let resultat =
                    tokio::task::spawn_blocking(move || traiter(requete, &jeton, &gestionnaire))
                        .await
                        .unwrap_or_else(|e| Err(Erreur::new(Code::Internal, e.to_string())));
                Reponse::de(resultat)
            }
        },
    };
    let mut texte = serde_json::to_string(&reponse).unwrap_or_else(|_| {
        r#"{"ok":false,"error":{"code":"internal","message":"réponse impossible à écrire"}}"#.into()
    });
    texte.push('\n');
    let _ = ecriture.write_all(texte.as_bytes()).await;
    let _ = ecriture.flush().await;
    let _ = ecriture.shutdown().await;
}

#[cfg(windows)]
async fn boucle(
    point: &Point,
    jeton: Arc<String>,
    gestionnaire: Gestionnaire,
    pret: std::sync::mpsc::Sender<io::Result<()>>,
    mut arret: oneshot::Receiver<()>,
) {
    use tokio::net::windows::named_pipe::ServerOptions;
    let creer = |premiere: bool| {
        ServerOptions::new()
            .first_pipe_instance(premiere)
            .reject_remote_clients(true)
            .create(&point.adresse)
    };
    // Première instance : personne n'a pu prendre le nom avant Kobi.
    let mut serveur = match creer(true) {
        Ok(s) => s,
        Err(e) => {
            let _ = pret.send(Err(e));
            return;
        }
    };
    if pret.send(ecrire_jeton(point, &jeton)).is_err() {
        return;
    }
    loop {
        tokio::select! {
            r = serveur.connect() => if let Err(e) = r {
                eprintln!("contrôle : connexion refusée : {e}");
                continue;
            },
            _ = &mut arret => return,
        }
        let client = std::mem::replace(
            &mut serveur,
            match creer(false) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("contrôle : canal fermé : {e}");
                    return;
                }
            },
        );
        tokio::spawn(servir(client, jeton.clone(), gestionnaire.clone()));
    }
}

#[cfg(unix)]
async fn boucle(
    point: &Point,
    jeton: Arc<String>,
    gestionnaire: Gestionnaire,
    pret: std::sync::mpsc::Sender<io::Result<()>>,
    mut arret: oneshot::Receiver<()>,
) {
    use std::os::unix::fs::PermissionsExt;
    let preparer = || -> io::Result<tokio::net::UnixListener> {
        if std::os::unix::net::UnixStream::connect(&point.adresse).is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                format!("{} : une autre instance de Kobi répond déjà", point.adresse),
            ));
        }
        let _ = std::fs::remove_file(&point.adresse);
        if let Some(dossier) = std::path::Path::new(&point.adresse).parent() {
            std::fs::create_dir_all(dossier)?;
        }
        let ecoute = tokio::net::UnixListener::bind(&point.adresse)?;
        std::fs::set_permissions(&point.adresse, std::fs::Permissions::from_mode(0o600))?;
        ecrire_jeton(point, &jeton)?;
        Ok(ecoute)
    };
    let ecoute = match preparer() {
        Ok(e) => {
            let _ = pret.send(Ok(()));
            e
        }
        Err(e) => {
            let _ = pret.send(Err(e));
            return;
        }
    };
    loop {
        tokio::select! {
            r = ecoute.accept() => match r {
                Ok((client, _)) => {
                    tokio::spawn(servir(client, jeton.clone(), gestionnaire.clone()));
                }
                Err(e) => eprintln!("contrôle : connexion refusée : {e}"),
            },
            _ = &mut arret => return,
        }
    }
}

/// Ouvre le point d'accès et écrit un nouveau jeton. Échoue si une autre
/// instance de Kobi tient déjà le point d'accès.
pub fn demarrer(point: Point, gestionnaire: Gestionnaire) -> io::Result<Serveur> {
    let jeton = nouveau_jeton()?;
    let (pret_tx, pret_rx) = std::sync::mpsc::channel();
    let (arret_tx, arret_rx) = oneshot::channel();
    let (p, j) = (point.clone(), Arc::new(jeton.clone()));
    let fil = std::thread::Builder::new()
        .name("controle".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = pret_tx.send(Err(e));
                    return;
                }
            };
            rt.block_on(boucle(&p, j, gestionnaire, pret_tx, arret_rx));
        })?;
    let pret = pret_rx
        .recv()
        .unwrap_or_else(|_| Err(io::Error::other("fil de contrôle arrêté")));
    if let Err(e) = pret {
        let _ = fil.join();
        return Err(e);
    }
    Ok(Serveur {
        point,
        jeton,
        arret: Some(arret_tx),
        fil: Some(fil),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::appeler;
    use serde_json::json;
    use std::sync::Mutex;

    pub(crate) fn point_de_test(dir: &std::path::Path, nom: &str) -> Point {
        Point::dans(dir, &format!("{nom}-{}", std::process::id()))
    }

    #[test]
    fn jeton_aleatoire_et_comparaison() {
        let (a, b) = (nouveau_jeton().unwrap(), nouveau_jeton().unwrap());
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
        assert!(meme_jeton(&a, &a.clone()));
        assert!(!meme_jeton(&a, &b));
        assert!(!meme_jeton(&a, "court"));
    }

    #[test]
    fn aller_retour_verifie() {
        let dir = tempfile::tempdir().unwrap();
        let point = point_de_test(dir.path(), "aller-retour");
        let vus = Arc::new(Mutex::new(Vec::new()));
        let v = vus.clone();
        let serveur = demarrer(
            point.clone(),
            Arc::new(move |c, params, origine| {
                v.lock().unwrap().push((c.nom, origine));
                match c.nom {
                    "status" => Ok(json!({"etat": "veille"})),
                    "meetings.get" => Ok(Value::Object(params)),
                    _ => Err(Erreur::new(Code::Conflict, "déjà fait")),
                }
            }),
        )
        .unwrap();
        assert!(std::fs::read_to_string(&point.jeton).unwrap().len() == 64);

        let r = appeler(&point, "status", Value::Null, Origine::Cli).unwrap();
        assert_eq!(r["etat"], "veille");
        let r = appeler(
            &point,
            "meetings.get",
            json!({"id": "last"}),
            Origine::Agent,
        )
        .unwrap();
        assert_eq!(r, json!({"id": "last"}));
        let e = appeler(&point, "meeting.start", Value::Null, Origine::Mcp).unwrap_err();
        assert_eq!((e.code, e.message.as_str()), (Code::Conflict, "déjà fait"));
        // Vérifié avant le gestionnaire.
        let e = appeler(&point, "rm.rf", Value::Null, Origine::Cli).unwrap_err();
        assert_eq!(e.code, Code::UnknownCommand);
        let e = appeler(
            &point,
            "meetings.get",
            json!({"format": "pdf"}),
            Origine::Cli,
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidParams);
        assert_eq!(
            *vus.lock().unwrap(),
            [
                ("status", Origine::Cli),
                ("meetings.get", Origine::Agent),
                ("meeting.start", Origine::Mcp)
            ]
        );

        // Mauvais jeton : refusé sans atteindre le gestionnaire.
        let vrai = std::fs::read_to_string(&point.jeton).unwrap();
        std::fs::write(&point.jeton, "0".repeat(64)).unwrap();
        let e = appeler(&point, "status", Value::Null, Origine::Cli).unwrap_err();
        assert_eq!(e.code, Code::Unauthorized);
        assert_eq!(e.code.sortie(), 4);
        std::fs::write(&point.jeton, &vrai).unwrap();

        // Une seconde instance ne peut pas prendre le point d'accès.
        assert!(demarrer(point.clone(), Arc::new(|_, _, _| Ok(Value::Null))).is_err());
        assert_eq!(std::fs::read_to_string(&point.jeton).unwrap(), vrai);

        drop(serveur);
        assert!(!point.jeton.exists(), "jeton effacé à l'arrêt");
        let e = appeler(&point, "status", Value::Null, Origine::Cli).unwrap_err();
        assert_eq!(e.code, Code::NotRunning);
        assert_eq!(e.code.sortie(), 3);
    }

    #[test]
    fn requetes_simultanees() {
        let dir = tempfile::tempdir().unwrap();
        let point = point_de_test(dir.path(), "simultanees");
        let _serveur = demarrer(
            point.clone(),
            Arc::new(|_, _, _| {
                std::thread::sleep(std::time::Duration::from_millis(50));
                Ok(json!("ok"))
            }),
        )
        .unwrap();
        let fils: Vec<_> = (0..8)
            .map(|_| {
                let p = point.clone();
                std::thread::spawn(move || appeler(&p, "status", Value::Null, Origine::Cli))
            })
            .collect();
        for f in fils {
            assert_eq!(f.join().unwrap().unwrap(), json!("ok"));
        }
    }
}
