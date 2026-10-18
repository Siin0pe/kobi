//! Démarrage avec le système (KOB-143) : Kobi se lance tout seul à
//! l'ouverture de session, en veille et sans fenêtre de bienvenue.
//!
//! L'état vit là où le système le lit, pas dans `config.toml` : la valeur
//! `Kobi` de la clé `Run` du registre (HKCU) sous Windows, `kobi.desktop`
//! dans `~/.config/autostart` sous Linux, un LaunchAgent sous macOS.
//! L'installeur Windows pose la même valeur (`installeur.nsh`) : la case de
//! l'installeur et celle des réglages ne font qu'un.

use std::io;
use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager, Runtime};

/// Argument ajouté à la ligne de commande lancée par le système.
pub const ARG: &str = "--demarrage";

/// Kobi a été lancé par le système à l'ouverture de session.
pub fn lance_par_le_systeme() -> bool {
    std::env::args().any(|a| a == ARG)
}

/// Ligne de commande enregistrée : l'exécutable entre guillemets, puis [`ARG`].
pub fn commande(exe: &std::path::Path) -> String {
    format!("\"{}\" {ARG}", exe.display())
}

/// Démarrage avec le système pour un exécutable donné.
#[derive(Debug, Clone)]
pub struct Demarrage {
    exe: PathBuf,
    /// Fichier posé pour le système (Linux, macOS) ; `None` sous Windows.
    #[cfg_attr(windows, allow(dead_code))]
    fichier: Option<PathBuf>,
}

impl Demarrage {
    /// Pour l'exécutable en cours.
    pub fn courant() -> io::Result<Self> {
        Ok(Self::new(std::env::current_exe()?, fichier_par_defaut()))
    }

    pub fn new(exe: PathBuf, fichier: Option<PathBuf>) -> Self {
        Self { exe, fichier }
    }

    pub fn est_actif(&self) -> io::Result<bool> {
        systeme::est_actif(self)
    }

    pub fn regler(&self, actif: bool) -> io::Result<()> {
        if actif {
            systeme::activer(self)
        } else {
            systeme::desactiver(self)
        }
    }
}

/// Pose l'état partagé ; à appeler au démarrage de l'app.
pub fn installer<R: Runtime>(app: &AppHandle<R>) {
    match Demarrage::courant() {
        Ok(d) => {
            // Essai de bout en bout sans interface (sous Xvfb) : `oui` ou `non`.
            if let Some(v) = std::env::var_os("KOBI_ESSAI_DEMARRAGE") {
                let actif = v == "oui";
                match d.regler(actif).and_then(|_| d.est_actif()) {
                    Ok(etat) => tracing::info!(actif = etat, "essai : démarrage avec le système"),
                    Err(e) => tracing::warn!(erreur = %e, "essai : démarrage avec le système"),
                }
            }
            app.manage(d);
        }
        Err(e) => tracing::warn!(erreur = %e, "démarrage avec le système indisponible"),
    }
    if lance_par_le_systeme() {
        tracing::info!("lancé à l'ouverture de session : en veille, sans bienvenue");
    }
}

fn courant<R: Runtime>(app: &AppHandle<R>) -> Result<tauri::State<'_, Demarrage>, String> {
    app.try_state::<Demarrage>()
        .ok_or_else(|| "démarrage avec le système indisponible".to_owned())
}

/// Kobi démarre-t-il avec le système ?
#[tauri::command]
pub fn demarrage_systeme<R: Runtime>(app: AppHandle<R>) -> Result<bool, String> {
    courant(&app)?.est_actif().map_err(|e| e.to_string())
}

/// Fichier de `~/Kobi` posé dès que l'utilisateur a choisi (installeur,
/// premier lancement ou réglages) : le démarrage n'est plus jamais activé
/// d'office.
pub const CHOIX: &str = "demarrage.choisi";

/// Dossier où noter le choix, si ce n'est pas `~/Kobi` (tests).
pub struct DossierDuChoix(pub PathBuf);

fn dossier_du_choix<R: Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    app.try_state::<DossierDuChoix>()
        .map(|d| d.0.clone())
        .or_else(kobi_session::config::kobi_dir)
}

fn choix_fait(kobi_dir: &Path) -> bool {
    kobi_dir.join(CHOIX).exists()
}

fn noter_choix(kobi_dir: &Path) {
    let ecrit =
        std::fs::create_dir_all(kobi_dir).and_then(|_| std::fs::write(kobi_dir.join(CHOIX), b""));
    if let Err(e) = ecrit {
        tracing::warn!(erreur = %e, "choix du démarrage non mémorisé");
    }
}

/// Premier lancement : sans choix antérieur, le démarrage avec le système
/// est activé (case cochée par défaut, décision du propriétaire) et le choix
/// est noté ; ensuite, l'état est laissé tel quel.
pub fn par_defaut(d: &Demarrage, kobi_dir: &Path) -> io::Result<bool> {
    if !choix_fait(kobi_dir) {
        d.regler(true)?;
        noter_choix(kobi_dir);
        tracing::info!("démarrage avec le système activé par défaut");
    }
    d.est_actif()
}

/// Active ou coupe le démarrage avec le système ; renvoie l'état relu.
#[tauri::command]
pub fn regler_demarrage_systeme<R: Runtime>(
    app: AppHandle<R>,
    actif: bool,
) -> Result<bool, String> {
    let d = courant(&app)?;
    d.regler(actif).map_err(|e| e.to_string())?;
    if let Some(dir) = dossier_du_choix(&app) {
        noter_choix(&dir);
    }
    tracing::info!(actif, "démarrage avec le système réglé");
    d.est_actif().map_err(|e| e.to_string())
}

/// État du démarrage pour le premier lancement : coché par défaut tant que
/// l'utilisateur n'a pas choisi (voir [`par_defaut`]).
#[tauri::command]
pub fn demarrage_par_defaut<R: Runtime>(app: AppHandle<R>) -> Result<bool, String> {
    let d = courant(&app)?;
    let dir = dossier_du_choix(&app).ok_or("dossier personnel introuvable")?;
    par_defaut(&d, &dir).map_err(|e| e.to_string())
}

#[cfg(windows)]
fn fichier_par_defaut() -> Option<PathBuf> {
    None
}

#[cfg(target_os = "macos")]
fn fichier_par_defaut() -> Option<PathBuf> {
    std::env::home_dir().map(|h| h.join("Library/LaunchAgents/dev.kobi.app.plist"))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn fichier_par_defaut() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|h| h.join(".config")))?;
    Some(config.join("autostart").join("kobi.desktop"))
}

/// Contenu du fichier de démarrage (Linux : entrée XDG ; macOS : LaunchAgent).
#[cfg_attr(windows, allow(dead_code))]
fn contenu(exe: &std::path::Path) -> String {
    if cfg!(target_os = "macos") {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>dev.kobi.app</string>
  <key>ProgramArguments</key>
  <array><string>{}</string><string>{ARG}</string></array>
  <key>RunAtLoad</key><true/>
</dict>
</plist>
"#,
            exe.display()
        )
    } else {
        format!(
            "[Desktop Entry]\nType=Application\nName=Kobi\nComment=Assistant vocal local\nExec={}\nX-GNOME-Autostart-enabled=true\nNoDisplay=true\n",
            commande(exe)
        )
    }
}

#[cfg(not(windows))]
mod systeme {
    use super::{contenu, Demarrage};
    use std::fs;
    use std::io;

    fn fichier(d: &Demarrage) -> io::Result<&std::path::Path> {
        d.fichier
            .as_deref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "dossier personnel introuvable"))
    }

    pub fn est_actif(d: &Demarrage) -> io::Result<bool> {
        match fs::read_to_string(fichier(d)?) {
            // Désactivé à la main dans le bureau (GNOME, KDE) : compte comme absent.
            Ok(texte) => Ok(!texte.lines().any(|l| {
                l.trim() == "X-GNOME-Autostart-enabled=false" || l.trim() == "Hidden=true"
            })),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }

    pub fn activer(d: &Demarrage) -> io::Result<()> {
        let f = fichier(d)?;
        if let Some(dossier) = f.parent() {
            fs::create_dir_all(dossier)?;
        }
        let tmp = f.with_extension("tmp");
        fs::write(&tmp, contenu(&d.exe))?;
        fs::rename(tmp, f)
    }

    pub fn desactiver(d: &Demarrage) -> io::Result<()> {
        match fs::remove_file(fichier(d)?) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

#[cfg(windows)]
mod systeme {
    use super::{commande, Demarrage};
    use std::io;
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    const CLE: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALEUR: &str = "Kobi";

    pub fn est_actif(_: &Demarrage) -> io::Result<bool> {
        let run = RegKey::predef(HKEY_CURRENT_USER).open_subkey(CLE)?;
        match run.get_value::<String, _>(VALEUR) {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }

    pub fn activer(d: &Demarrage) -> io::Result<()> {
        let (run, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(CLE)?;
        run.set_value(VALEUR, &commande(&d.exe))
    }

    pub fn desactiver(_: &Demarrage) -> io::Result<()> {
        let run = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(CLE, winreg::enums::KEY_SET_VALUE)?;
        match run.delete_value(VALEUR) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;

    #[test]
    fn activer_puis_desactiver() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("autostart/kobi.desktop");
        let d = Demarrage::new(PathBuf::from("/opt/Kobi/kobi"), Some(f.clone()));
        assert!(!d.est_actif().unwrap());
        d.regler(true).unwrap();
        assert!(d.est_actif().unwrap());
        let texte = std::fs::read_to_string(&f).unwrap();
        assert!(
            texte.contains("\"/opt/Kobi/kobi\" --demarrage")
                || texte.contains("<string>--demarrage</string>"),
            "{texte}"
        );
        d.regler(true).unwrap();
        d.regler(false).unwrap();
        assert!(!d.est_actif().unwrap());
        d.regler(false).unwrap();
    }

    /// Premier lancement : activé par défaut une seule fois ; un choix
    /// (décoché) n'est jamais repris, même en rejouant le parcours.
    #[test]
    fn active_par_defaut_tant_que_rien_n_est_choisi() {
        let dir = tempfile::tempdir().unwrap();
        let kobi = dir.path().join("Kobi");
        let d = Demarrage::new(
            PathBuf::from("/opt/Kobi/kobi"),
            Some(dir.path().join("kobi.desktop")),
        );
        assert!(par_defaut(&d, &kobi).unwrap(), "profil neuf : coché");
        assert!(kobi.join(CHOIX).exists());
        // L'utilisateur décoche, puis rejoue le parcours.
        d.regler(false).unwrap();
        assert!(!par_defaut(&d, &kobi).unwrap(), "décoché : le reste");
        // Choix fait ailleurs (installeur : « Non ») : rien n'est activé.
        let autre = dir.path().join("Autre");
        noter_choix(&autre);
        let d2 = Demarrage::new(
            PathBuf::from("kobi"),
            Some(dir.path().join("autre.desktop")),
        );
        assert!(!par_defaut(&d2, &autre).unwrap());
    }

    #[test]
    fn desactive_par_le_bureau() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("kobi.desktop");
        std::fs::write(&f, "[Desktop Entry]\nExec=kobi\nHidden=true\n").unwrap();
        let d = Demarrage::new(PathBuf::from("kobi"), Some(f));
        assert!(!d.est_actif().unwrap());
    }

    /// La case des réglages passe par l'IPC de Tauri et pose le fichier.
    #[test]
    fn regler_depuis_l_interface() {
        use tauri::ipc::{CallbackFn, InvokeBody};
        use tauri::test::{get_ipc_response, mock_builder, mock_context, noop_assets, INVOKE_KEY};
        use tauri::webview::InvokeRequest;

        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("autostart/kobi.desktop");
        let app = crate::session::installer(mock_builder())
            .build(mock_context(noop_assets()))
            .unwrap();
        app.manage(Demarrage::new(
            PathBuf::from("/opt/Kobi/kobi"),
            Some(f.clone()),
        ));
        let kobi = dir.path().join("Kobi");
        app.manage(DossierDuChoix(kobi.clone()));
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let appel = |cmd: &str, corps: serde_json::Value| {
            let requete = InvokeRequest {
                cmd: cmd.into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: "tauri://localhost".parse().unwrap(),
                body: InvokeBody::Json(corps),
                headers: Default::default(),
                invoke_key: INVOKE_KEY.to_string(),
            };
            get_ipc_response(&webview, requete)
                .map(|b| b.deserialize::<bool>().unwrap())
                .unwrap()
        };
        assert!(!appel("demarrage_systeme", serde_json::json!({})));
        assert!(appel(
            "regler_demarrage_systeme",
            serde_json::json!({ "actif": true })
        ));
        assert!(f.exists());
        assert!(appel("demarrage_systeme", serde_json::json!({})));
        assert!(!appel(
            "regler_demarrage_systeme",
            serde_json::json!({ "actif": false })
        ));
        assert!(!f.exists());
        // Le choix est noté : le premier lancement ne réactive rien.
        assert!(kobi.join(CHOIX).exists());
        assert!(!appel("demarrage_par_defaut", serde_json::json!({})));
        assert!(!f.exists());
    }

    #[test]
    fn sans_dossier_personnel() {
        let d = Demarrage::new(PathBuf::from("kobi"), None);
        assert!(d.est_actif().is_err());
        assert!(d.regler(true).is_err());
    }
}
