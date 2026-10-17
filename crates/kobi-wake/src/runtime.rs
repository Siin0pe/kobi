//! Chargement d'ONNX Runtime pour les détecteurs à base de modèles ONNX.
//!
//! Deux façons d'obtenir ONNX Runtime, au choix à la compilation :
//!
//! - feature `ort-dynamic` (défaut) : aucun lien à la compilation ; la
//!   bibliothèque partagée (`libonnxruntime.so`, `.dylib` ou
//!   `onnxruntime.dll`, version 1.27 ou plus récente) est chargée à la
//!   première session. Les binaires officiels de Microsoft pour Linux ne
//!   demandent que glibc ≥ 2.27 ; les paquets de Kobi l'embarquent.
//! - feature `ort-static` (avec `--no-default-features`) : la bibliothèque
//!   précompilée par `ort` est téléchargée au build et liée statiquement.
//!   Rien à installer, mais sous Linux elle exige glibc ≥ 2.38 et
//!   libstdc++ ≥ 13 (Ubuntu 24.04 et plus récents ; pas Debian 12 ni
//!   Ubuntu 22.04 : `undefined symbol: __isoc23_strtol`).
//!
//! En `ort-dynamic`, la bibliothèque est cherchée dans cet ordre : la variable
//! `ORT_DYLIB_PATH`, puis le dossier de l'exécutable (là où un paquet Kobi la
//! dépose), puis les chemins du chargeur du système.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::WakeError;

/// Nom de la bibliothèque partagée d'ONNX Runtime sur cette plateforme.
pub const LIBRARY: &str = if cfg!(windows) {
    "onnxruntime.dll"
} else if cfg!(target_os = "macos") {
    "libonnxruntime.dylib"
} else {
    "libonnxruntime.so"
};

/// Chemin de la bibliothèque à charger en `ort-dynamic`, d'après la valeur de
/// `ORT_DYLIB_PATH` et le dossier de l'exécutable.
pub fn library_path(env: Option<OsString>, exe_dir: Option<&Path>) -> PathBuf {
    if let Some(path) = env.filter(|p| !p.is_empty()) {
        return PathBuf::from(path);
    }
    match exe_dir.map(|dir| dir.join(LIBRARY)) {
        Some(beside) if beside.is_file() => beside,
        // Nom seul : recherche par le chargeur du système.
        _ => PathBuf::from(LIBRARY),
    }
}

/// Prépare ONNX Runtime avant la première session. Sans effet quand la
/// bibliothèque est liée statiquement ; en `ort-dynamic`, la charge une seule
/// fois pour tout le processus.
pub fn ensure() -> Result<(), WakeError> {
    #[cfg(feature = "ort-dynamic")]
    {
        use std::sync::OnceLock;

        static LOADED: OnceLock<Result<(), String>> = OnceLock::new();
        LOADED
            .get_or_init(|| {
                let exe = std::env::current_exe().ok();
                let path = library_path(
                    std::env::var_os("ORT_DYLIB_PATH"),
                    exe.as_deref().and_then(Path::parent),
                );
                match ort::init_from(&path) {
                    Ok(env) => {
                        env.commit();
                        Ok(())
                    }
                    Err(e) => Err(format!(
                        "ONNX Runtime introuvable ({}) : {e}. Définir ORT_DYLIB_PATH ou \
                         placer {LIBRARY} à côté de l'exécutable",
                        path.display()
                    )),
                }
            })
            .clone()
            .map_err(WakeError::Ort)
    }
    #[cfg(not(feature = "ort-dynamic"))]
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_path_prefers_env_then_executable_dir() {
        let dir = std::env::temp_dir().join(format!("kobi-ort-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let env = Some(OsString::from("/opt/ort/lib/libonnxruntime.so.1.28.0"));
        assert_eq!(
            library_path(env, Some(&dir)),
            PathBuf::from("/opt/ort/lib/libonnxruntime.so.1.28.0")
        );
        // Variable vide et rien à côté de l'exécutable : le chargeur cherche.
        assert_eq!(
            library_path(Some(OsString::new()), Some(&dir)),
            PathBuf::from(LIBRARY)
        );
        std::fs::write(dir.join(LIBRARY), b"").unwrap();
        assert_eq!(library_path(None, Some(&dir)), dir.join(LIBRARY));
        std::fs::remove_dir_all(&dir).ok();
    }
}
