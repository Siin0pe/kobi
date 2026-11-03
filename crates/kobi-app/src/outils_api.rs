//! Outils du backend clé API (KOB-102) : ce que Claude Code fournit de
//! lui-même, refait pour [`kobi_brain::ApiBrain`].
//!
//! * **Lecture des dossiers** déclarés (codebases, mémoire des réunions) :
//!   `Read`, `Grep` et `Glob`, mêmes noms et mêmes entrées que chez Claude
//!   Code, pour que la traduction en [`kobi_brain::Action`] et le prompt
//!   restent communs. Rien ne sort des dossiers : chaque chemin est résolu
//!   (liens compris) puis comparé aux dossiers permis.
//! * **Serveurs MCP** du workspace, par le client `rmcp` de Kobi : outils
//!   nommés `mcp__<serveur>__<outil>` comme chez Claude Code, avec les
//!   mêmes règles (KOB-320) : nature et politique lues dans le catalogue des
//!   réglages ; la lecture passe, l'écriture demande une carte (sauf
//!   serveur « faire sans demander »), un outil inconnu aussi. En lecture
//!   seule (question différée), seuls les outils connus en lecture sont
//!   proposés et rien ne demande de carte.

use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use kobi_brain::Autorisation;
use kobi_perms::mcp::{Catalogue, Nature, Verdict};
use rmcp::service::RunningService;
use rmcp::RoleClient;
use serde_json::{json, Value};

/// Lignes lues par défaut (`Read`).
const LIGNES_LUES: usize = 2000;
/// Taille maximale d'un résultat rendu à Claude.
const TAILLE_MAX: usize = 100_000;
/// Octets parcourus au plus dans un fichier (`Read`).
const LECTURE_MAX: u64 = 50_000_000;
/// Octets gardés au plus d'une ligne (`Read`, `Grep`).
const LIGNE_MAX: usize = 2000;
/// Fichiers plus gros : ignorés par `Grep`.
const FICHIER_MAX: u64 = 2_000_000;
/// Lignes trouvées au plus (`Grep`) et chemins au plus (`Glob`).
const RESULTATS_MAX: usize = 300;
/// Dossiers jamais parcourus.
const DOSSIERS_IGNORES: &[&str] = &[".git", "node_modules", "target", ".venv", "__pycache__"];

/// Bornes d'un parcours (`Grep`, `Glob`), KOB-335 : un arbre immense ne
/// retient ni la mémoire ni le tour.
#[derive(Debug, Clone, Copy)]
struct Bornes {
    /// Fichiers lus au plus par `Grep`.
    fichiers: usize,
    /// Entrées parcourues au plus par `Glob`.
    entrees: usize,
    /// Durée maximale d'un parcours.
    duree: std::time::Duration,
}

impl Default for Bornes {
    fn default() -> Self {
        Self {
            fichiers: 20_000,
            entrees: 200_000,
            duree: std::time::Duration::from_secs(10),
        }
    }
}

/// Lecture seule dans des dossiers permis.
#[derive(Debug, Clone)]
pub struct Lecteur {
    /// Dossiers permis, résolus.
    dossiers: Vec<PathBuf>,
    /// Base des chemins relatifs : le premier dossier déclaré, même s'il
    /// est contenu dans un autre.
    base: Option<PathBuf>,
    bornes: Bornes,
}

impl Lecteur {
    /// Les dossiers absents sont ignorés, et un dossier contenu dans un
    /// autre aussi : rien n'est parcouru ni compté deux fois.
    pub fn new(dossiers: &[PathBuf]) -> Self {
        let resolus: Vec<PathBuf> = dossiers
            .iter()
            .filter_map(|d| d.canonicalize().ok())
            .filter(|d| d.is_dir())
            .collect();
        let mut garde: Vec<PathBuf> = Vec::new();
        for d in &resolus {
            // Écarté seulement si le parcours d'un autre dossier déclaré y
            // passe : `/proj/target`, déclaré avec `/proj`, reste un départ
            // à part, puisque le parcours de `/proj` saute `target` (KOB-357).
            let dedans = resolus
                .iter()
                .any(|autre| autre != d && atteint_par_le_parcours(autre, d));
            if !dedans && !garde.contains(d) {
                garde.push(d.clone());
            }
        }
        Self {
            dossiers: garde,
            base: resolus.first().cloned(),
            bornes: Bornes::default(),
        }
    }

    fn dans_les_dossiers(&self, chemin: &Path) -> bool {
        self.dossiers.iter().any(|d| chemin.starts_with(d))
    }

    fn hors(&self, chemin: &Path) -> String {
        format!(
            "{} est hors des dossiers permis ({})",
            chemin.display(),
            self.liste()
        )
    }

    /// Ouvre un fichier déjà validé par [`Lecteur::permis`], sans laisser de
    /// fenêtre entre la vérification et l'ouverture (KOB-335).
    ///
    /// Sous Linux :
    /// * un premier descripteur `O_PATH | O_NOFOLLOW` désigne le fichier sans
    ///   l'ouvrir : aucun effet de bord, même sur un périphérique ; un lien
    ///   (dernier composant) n'est pas suivi et se voit refusé ;
    /// * le type (fichier ordinaire seulement) et le vrai chemin
    ///   (`/proc/self/fd`, dans les dossiers permis) sont vérifiés sur ce
    ///   descripteur : un dossier remplacé par un lien après la vérification
    ///   est refusé ;
    /// * le fichier est ensuite ouvert par `/proc/self/fd/N`, donc le même
    ///   inode, en `O_NONBLOCK | O_NOCTTY`.
    ///
    /// Ailleurs (Unix) : `O_NOFOLLOW | O_NONBLOCK | O_NOCTTY`, type vérifié
    /// sur le descripteur, puis chemin résolu une seconde fois : plus
    /// faible, à essayer sous macOS ; sous Windows, à essayer aussi.
    ///
    /// Limites connues : un lien physique vers un fichier extérieur, ou un
    /// montage lié, posés dans un dossier permis, ne se détectent pas par le
    /// chemin (il faut pouvoir écrire dans la codebase pour les créer).
    fn ouvrir(&self, chemin: &Path) -> Result<(std::fs::File, u64), String> {
        let erreur = |e: std::io::Error| format!("{} : {e}", chemin.display());
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd;
            use std::os::unix::fs::OpenOptionsExt;
            let repere = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_PATH | libc::O_NOFOLLOW)
                .open(chemin)
                .map_err(erreur)?;
            let meta = repere.metadata().map_err(erreur)?;
            if !meta.is_file() {
                return Err(format!(
                    "{} n'est pas un fichier ordinaire",
                    chemin.display()
                ));
            }
            let lien = format!("/proc/self/fd/{}", repere.as_raw_fd());
            let reel = std::fs::read_link(&lien).map_err(|e| {
                format!(
                    "{} : vérification impossible (/proc indisponible : {e})",
                    chemin.display()
                )
            })?;
            if !self.dans_les_dossiers(&reel) {
                return Err(self.hors(chemin));
            }
            let fichier = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
                .open(&lien)
                .map_err(erreur)?;
            Ok((fichier, meta.len()))
        }
        #[cfg(not(target_os = "linux"))]
        {
            let mut options = std::fs::OpenOptions::new();
            options.read(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_NOCTTY);
            }
            let fichier = options.open(chemin).map_err(erreur)?;
            let meta = fichier.metadata().map_err(erreur)?;
            if !meta.is_file() {
                return Err(format!(
                    "{} n'est pas un fichier ordinaire",
                    chemin.display()
                ));
            }
            let reel = chemin.canonicalize().map_err(erreur)?;
            if !self.dans_les_dossiers(&reel) {
                return Err(self.hors(chemin));
            }
            Ok((fichier, meta.len()))
        }
    }

    pub fn definitions() -> Vec<Value> {
        vec![
            json!({
                "name": "Read",
                "description": "Lit un fichier texte d'une codebase ou des réunions passées. Rend les lignes numérotées.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "file_path": { "type": "string", "description": "Chemin absolu du fichier." },
                        "offset": { "type": "integer", "description": "Première ligne lue (à partir de 1)." },
                        "limit": { "type": "integer", "description": "Nombre de lignes lues (2000 par défaut)." },
                    },
                    "required": ["file_path"],
                },
            }),
            json!({
                "name": "Grep",
                "description": "Cherche une expression régulière dans les fichiers des codebases et des réunions passées. Rend les lignes trouvées, avec fichier et numéro de ligne.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string", "description": "Expression régulière (syntaxe Rust)." },
                        "path": { "type": "string", "description": "Dossier ou fichier où chercher ; tous les dossiers permis par défaut." },
                        "glob": { "type": "string", "description": "Filtre sur le nom des fichiers, par exemple « *.md »." },
                        "-i": { "type": "boolean", "description": "Sans tenir compte de la casse." },
                    },
                    "required": ["pattern"],
                },
            }),
            json!({
                "name": "Glob",
                "description": "Liste les fichiers dont le chemin correspond à un motif (« **/*.rs », « src/*.ts »).",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string", "description": "Motif, relatif au dossier." },
                        "path": { "type": "string", "description": "Dossier de départ ; tous les dossiers permis par défaut." },
                    },
                    "required": ["pattern"],
                },
            }),
        ]
    }

    /// Chemin résolu, s'il est dans un dossier permis.
    fn permis(&self, chemin: &str) -> Result<PathBuf, String> {
        // Sous une base verbatim (`\\?\C:\…`), seul `\` sépare : `a/../x`
        // y serait un seul nom (KOB-352).
        #[cfg(windows)]
        let chemin = &chemin.replace('/', "\\");
        let brut = Path::new(chemin);
        let brut = match (brut.is_absolute(), self.base.as_ref()) {
            (false, Some(base)) => base.join(brut),
            _ => brut.to_path_buf(),
        };
        // Avant toute résolution : résoudre un chemin réseau ouvrirait déjà
        // la connexion (KOB-344). Sans dossier permis, rien ne se résout.
        if self.dossiers.is_empty() {
            return Err(format!("{chemin} : aucun dossier permis"));
        }
        let refus = || {
            format!(
                "{chemin} : chemin réseau ou de périphérique refusé (seuls les dossiers permis se lisent : {})",
                self.liste()
            )
        };
        if chemin_reseau(chemin) || chemin_reseau(&brut.to_string_lossy()) {
            return Err(refus());
        }
        // Sous Windows, un `..` du chemin demandé se réduit sur le texte avant
        // tout lien suivi : le contrôle et `canonicalize` partent du même
        // chemin réduit, verbatim compris (`absolute` laisse `\\?\` intact),
        // sans toucher au disque (KOB-352). Ailleurs, `..` est physique des
        // deux côtés.
        #[cfg(windows)]
        let brut = reduire_sur_le_texte(&brut);
        // Sous Windows, le parcours refuse aussi un nom de périphérique DOS,
        // du chemin comme des cibles de liens (KOB-353).
        if !prefixe_local(&brut) || lien_vers_le_reseau(&brut, cfg!(windows)) {
            return Err(refus());
        }
        let resolu = brut.canonicalize().map_err(|e| format!("{chemin} : {e}"))?;
        if self.dans_les_dossiers(&resolu) {
            Ok(resolu)
        } else {
            Err(format!(
                "{chemin} est hors des dossiers permis ({})",
                self.liste()
            ))
        }
    }

    fn liste(&self) -> String {
        self.dossiers
            .iter()
            .map(|d| d.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Point de départ d'une recherche : `path` ou tous les dossiers.
    fn departs(&self, entree: &Value) -> Result<Vec<PathBuf>, String> {
        match entree["path"].as_str().filter(|p| !p.is_empty()) {
            Some(p) => Ok(vec![self.permis(p)?]),
            None if self.dossiers.is_empty() => Err("aucun dossier à lire".into()),
            None => Ok(self.dossiers.clone()),
        }
    }

    /// Parcourt `depart` (sans les dossiers ignorés, sans suivre les liens)
    /// et appelle `fichier` pour chaque fichier ordinaire, jusqu'à ce qu'il
    /// rende `false`. Chaque entrée parcourue compte, dossiers et liens
    /// compris, et la durée est vérifiée à chaque entrée (KOB-335) : un arbre
    /// immense ou très profond arrête le parcours, avec `parcours.arrete`.
    fn parcourir(
        &self,
        depart: &Path,
        parcours: &mut Parcours,
        mut fichier: impl FnMut(PathBuf) -> bool,
    ) {
        let entrees = walkdir::WalkDir::new(depart)
            .follow_links(false)
            .into_iter()
            // Le départ lui-même est toujours parcouru : demandé (« cherche
            // dans node_modules ») ou déclaré (une codebase nommée `target`),
            // il ne s'ignore pas (KOB-346).
            .filter_entry(|e| {
                e.depth() == 0
                    || !(e.file_type().is_dir()
                        && DOSSIERS_IGNORES.contains(&e.file_name().to_string_lossy().as_ref()))
            });
        for entree in entrees {
            parcours.vues += 1;
            if parcours.vues > self.bornes.entrees || parcours.debut.elapsed() >= self.bornes.duree
            {
                parcours.arrete = true;
                return;
            }
            let Ok(entree) = entree else { continue };
            if entree.file_type().is_file() && !fichier(entree.into_path()) {
                return;
            }
        }
    }

    /// Message d'un parcours arrêté par ses bornes.
    fn arret(&self, parcours: &Parcours) -> String {
        format!(
            "… parcours arrêté après {} entrées ({} s au plus) : précise le dossier, le motif ou le filtre",
            parcours.vues - 1,
            self.bornes.duree.as_secs()
        )
    }

    /// Lit `limit` lignes à partir de `offset`, sans jamais charger le
    /// fichier entier (KOB-321) : lecture ligne à ligne, chaque ligne
    /// coupée à [`LIGNE_MAX`], le tout borné à [`TAILLE_MAX`] et à
    /// [`LECTURE_MAX`] octets parcourus.
    pub fn lire(&self, entree: &Value) -> Result<String, String> {
        use std::io::{BufRead, BufReader, Read};
        let chemin = self.permis(texte(entree, "file_path")?)?;
        let erreur = |e: std::io::Error| format!("{} : {e}", chemin.display());
        // Ni dossier, ni tube nommé, ni périphérique, ni chemin changé depuis
        // la vérification (KOB-335).
        let (fichier, taille) = self.ouvrir(&chemin)?;
        let mut lecteur = BufReader::new(fichier.take(LECTURE_MAX));
        let pas_texte = || format!("{} n'est pas un fichier texte", chemin.display());
        if lecteur.fill_buf().map_err(erreur)?.contains(&0) {
            return Err(pas_texte());
        }
        let debut = entree["offset"].as_u64().unwrap_or(1).max(1);
        let nombre = entree["limit"].as_u64().unwrap_or(LIGNES_LUES as u64);
        let mut sortie = String::new();
        let mut ligne = Vec::new();
        let mut numero = 0u64;
        let mut coupe = false;
        while numero < (debut - 1).saturating_add(nombre) {
            ligne.clear();
            let lue = lire_ligne(&mut lecteur, &mut ligne).map_err(erreur)?;
            if lue.octets == 0 {
                break;
            }
            // Un octet NUL plus loin que le premier tampon : binaire aussi
            // (KOB-347).
            if lue.nul {
                return Err(pas_texte());
            }
            numero += 1;
            if numero < debut {
                continue;
            }
            let texte = if lue.coupee {
                format!(
                    "{} … (ligne coupée)",
                    String::from_utf8_lossy(sans_caractere_entame(&ligne))
                )
            } else {
                String::from_utf8_lossy(&ligne)
                    .trim_end_matches(['\n', '\r'])
                    .to_owned()
            };
            sortie.push_str(&format!("{numero}\t{texte}\n"));
            if sortie.len() > TAILLE_MAX {
                coupe = true;
                break;
            }
        }
        if sortie.is_empty() {
            sortie = "(fichier vide ou fin de fichier dépassée)".into();
        }
        if taille > LECTURE_MAX && !coupe {
            sortie.push_str(&format!(
                "(fichier de {taille} octets : seuls les {LECTURE_MAX} premiers sont lus)\n"
            ));
        }
        Ok(couper(sortie))
    }

    pub fn chercher(&self, entree: &Value) -> Result<String, String> {
        use std::io::Read;
        let motif = regex::RegexBuilder::new(texte(entree, "pattern")?)
            .case_insensitive(entree["-i"].as_bool().unwrap_or(false))
            .build()
            .map_err(|e| format!("expression invalide : {e}"))?;
        let filtre = filtre(entree["glob"].as_str())?;
        let mut trouve = Vec::new();
        let mut parcours = Parcours::new();
        let mut lus = 0;
        let mut trop_de_fichiers = false;
        let mut plein = false;
        // Taille déjà trouvée : au-delà de `TAILLE_MAX`, la suite serait
        // coupée de toute façon (KOB-345).
        let mut deja_trouve = 0;
        for depart in self.departs(entree)? {
            self.parcourir(&depart, &mut parcours, |fichier| {
                let nom = fichier.file_name().unwrap_or_default().to_string_lossy();
                if filtre.as_ref().is_some_and(|f| !f.matches(&nom)) {
                    return true;
                }
                if lus >= self.bornes.fichiers {
                    trop_de_fichiers = true;
                    return false;
                }
                // Même ouverture que Read : rien hors des dossiers, aucun tube.
                let Ok((ouvert, taille)) = self.ouvrir(&fichier) else {
                    return true;
                };
                if taille > FICHIER_MAX {
                    return true;
                }
                lus += 1;
                let mut octets = Vec::new();
                if ouvert.take(FICHIER_MAX).read_to_end(&mut octets).is_err() || octets.contains(&0)
                {
                    return true;
                }
                for (i, ligne) in String::from_utf8_lossy(&octets).lines().enumerate() {
                    if motif.is_match(ligne) {
                        let trouvee =
                            format!("{}:{}:{}", fichier.display(), i + 1, ligne_bornee(ligne));
                        // Arrêté avant de dépasser : le marqueur d'arrêt reste
                        // dans ce qui est rendu.
                        if deja_trouve + trouvee.len() + 1 > TAILLE_MAX - 200 {
                            plein = true;
                            return false;
                        }
                        deja_trouve += trouvee.len() + 1;
                        trouve.push(trouvee);
                        if trouve.len() >= RESULTATS_MAX {
                            plein = true;
                            return false;
                        }
                    }
                }
                true
            });
            if plein || trop_de_fichiers || parcours.arrete {
                break;
            }
        }
        if plein && trouve.len() < RESULTATS_MAX {
            trouve.push("… arrêté : résultat trop long, précise le motif ou le dossier".into());
        } else if plein {
            trouve.push(format!("… arrêté à {RESULTATS_MAX} lignes"));
        } else if trop_de_fichiers {
            trouve.push(format!(
                "… recherche arrêtée après {lus} fichiers lus : précise le dossier ou le filtre"
            ));
        } else if parcours.arrete {
            trouve.push(self.arret(&parcours));
        }
        Ok(if trouve.is_empty() {
            "Aucun résultat.".into()
        } else {
            couper(trouve.join("\n"))
        })
    }

    pub fn lister(&self, entree: &Value) -> Result<String, String> {
        let motif = glob::Pattern::new(texte(entree, "pattern")?)
            .map_err(|e| format!("motif invalide : {e}"))?;
        let options = glob::MatchOptions {
            require_literal_separator: true,
            ..Default::default()
        };
        // Seuls les RESULTATS_MAX premiers chemins (dans l'ordre) restent en
        // mémoire (KOB-335).
        let mut gardes = std::collections::BTreeSet::new();
        let mut total = 0usize;
        let mut parcours = Parcours::new();
        for depart in self.departs(entree)? {
            self.parcourir(&depart, &mut parcours, |fichier| {
                let relatif = fichier.strip_prefix(&depart).unwrap_or(&fichier);
                if !motif.matches_path_with(relatif, options) {
                    return true;
                }
                // Le parcours peut avoir suivi un dossier remplacé par un lien
                // pendant la descente : le vrai dossier du nom doit être
                // dans les dossiers permis.
                let dans = fichier
                    .parent()
                    .and_then(|d| d.canonicalize().ok())
                    .is_some_and(|d| self.dans_les_dossiers(&d));
                if dans {
                    total += 1;
                    gardes.insert(fichier.display().to_string());
                    if gardes.len() > RESULTATS_MAX {
                        gardes.pop_last();
                    }
                }
                true
            });
            if parcours.arrete {
                break;
            }
        }
        let mut trouve: Vec<String> = gardes.into_iter().collect();
        if total > RESULTATS_MAX {
            trouve.push(format!("… {total} fichiers, {RESULTATS_MAX} montrés"));
        }
        if parcours.arrete {
            trouve.push(self.arret(&parcours));
        }
        Ok(if trouve.is_empty() {
            "Aucun fichier.".into()
        } else {
            couper(trouve.join("\n"))
        })
    }
}

/// Une ligne coupée à [`LIGNE_MAX`] octets, sur une frontière de caractère,
/// avec un marqueur : une ligne minifiée ou un JSON d'une ligne ne remplit
/// pas la mémoire ni le contexte (KOB-345).
fn ligne_bornee(ligne: &str) -> std::borrow::Cow<'_, str> {
    if ligne.len() <= LIGNE_MAX {
        return ligne.into();
    }
    let mut fin = LIGNE_MAX;
    while !ligne.is_char_boundary(fin) {
        fin -= 1;
    }
    format!("{} … (ligne coupée)", &ligne[..fin]).into()
}

/// `dossier` est sous `depart` et le parcours de `depart` y arrive : aucun
/// dossier entre les deux (ni `dossier` lui-même) n'est ignoré.
fn atteint_par_le_parcours(depart: &Path, dossier: &Path) -> bool {
    dossier.strip_prefix(depart).is_ok_and(|reste| {
        !reste
            .components()
            .any(|c| DOSSIERS_IGNORES.contains(&c.as_os_str().to_string_lossy().as_ref()))
    })
}

/// État d'un parcours borné (`Grep`, `Glob`).
struct Parcours {
    debut: std::time::Instant,
    /// Entrées parcourues, tous types confondus.
    vues: usize,
    arrete: bool,
}

impl Parcours {
    fn new() -> Self {
        Self {
            debut: std::time::Instant::now(),
            vues: 0,
            arrete: false,
        }
    }
}

/// Lit une ligne dans `ligne`, en n'en gardant que [`LIGNE_MAX`] octets :
/// une ligne géante (fichier minifié, sans retour à la ligne) ne passe pas
/// en mémoire.
fn lire_ligne(lecteur: &mut impl std::io::BufRead, ligne: &mut Vec<u8>) -> std::io::Result<Lue> {
    let mut lue = Lue {
        octets: 0,
        nul: false,
        coupee: false,
    };
    loop {
        let tampon = lecteur.fill_buf()?;
        if tampon.is_empty() {
            return Ok(lue);
        }
        let (fin, trouve) = match tampon.iter().position(|&b| b == b'\n') {
            Some(i) => (i + 1, true),
            None => (tampon.len(), false),
        };
        let place = LIGNE_MAX.saturating_sub(ligne.len());
        // Toute la ligne, même la partie qui n'est pas gardée.
        lue.nul |= tampon[..fin].contains(&0);
        let garde = fin.min(place);
        // Seul le retour à la ligne peut tomber sans que la ligne soit coupée.
        lue.coupee |= tampon[garde..fin].iter().any(|b| !b"\r\n".contains(b));
        ligne.extend_from_slice(&tampon[..garde]);
        lecteur.consume(fin);
        lue.octets += fin;
        // Binaire : inutile de parcourir la suite.
        if trouve || lue.nul {
            return Ok(lue);
        }
    }
}

/// Une ligne lue par [`lire_ligne`].
struct Lue {
    /// Octets parcourus (0 à la fin).
    octets: usize,
    /// Un octet NUL quelque part dans la ligne.
    nul: bool,
    /// Plus de [`LIGNE_MAX`] octets avant le retour à la ligne.
    coupee: bool,
}

/// Sans le caractère UTF-8 qu'une coupe aurait laissé incomplet à la fin.
fn sans_caractere_entame(octets: &[u8]) -> &[u8] {
    let debut = octets.len().saturating_sub(3);
    for i in (debut..octets.len()).rev() {
        // Premier octet d'un caractère.
        if octets[i] & 0xC0 != 0x80 {
            let attendu = match octets[i] {
                0xF0.. => 4,
                0xE0.. => 3,
                0xC0.. => 2,
                _ => 1,
            };
            return if octets.len() - i < attendu {
                &octets[..i]
            } else {
                octets
            };
        }
    }
    octets
}

fn texte<'a>(entree: &'a Value, cle: &str) -> Result<&'a str, String> {
    entree[cle]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("paramètre « {cle} » manquant"))
}

fn filtre(glob: Option<&str>) -> Result<Option<glob::Pattern>, String> {
    glob.filter(|g| !g.is_empty())
        .map(|g| glob::Pattern::new(g).map_err(|e| format!("filtre invalide : {e}")))
        .transpose()
}

fn couper(mut s: String) -> String {
    if s.len() > TAILLE_MAX {
        let mut fin = TAILLE_MAX;
        while !s.is_char_boundary(fin) {
            fin -= 1;
        }
        s.truncate(fin);
        s.push_str("\n… (coupé)");
    }
    s
}

/// Longueur maximale d'un nom d'outil pour l'API Messages.
const NOM_MAX: usize = 64;

/// Nom d'un outil MCP pour l'API Messages (KOB-317) : `mcp__<serveur>__<outil>`
/// comme chez Claude Code, mais seulement en `[a-zA-Z0-9_-]` et en
/// [`NOM_MAX`] caractères au plus, sans doublon avec `pris`. Un nom trop
/// long garde une empreinte du vrai nom ; un doublon reçoit un numéro.
fn nom_pour_l_api(serveur: &str, outil: &str, pris: &BTreeMap<String, (String, String)>) -> String {
    let brut = format!("mcp__{serveur}__{outil}");
    let propre: String = brut
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let reduit = |base: &str, suffixe: &str| -> String {
        let garde = NOM_MAX - suffixe.len();
        format!("{}{suffixe}", &base[..base.len().min(garde)])
    };
    let mut nom = if propre.len() > NOM_MAX {
        // FNV-1a : stable d'une version de Rust à l'autre (cache de prompt).
        let empreinte = brut.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
        });
        reduit(&propre, &format!("_{:08x}", empreinte as u32))
    } else {
        propre.clone()
    };
    let mut n = 2;
    while pris.contains_key(&nom) || matches!(nom.as_str(), "Read" | "Grep" | "Glob") {
        nom = reduit(&propre, &format!("_{n}"));
        n += 1;
    }
    nom
}

/// Serveur MCP connecté pour la réunion.
struct ServeurMcp {
    nom: String,
    client: RunningService<RoleClient, ()>,
    /// Outils proposés à Claude (nom côté serveur).
    outils: Vec<String>,
}

/// Ce que la session peut faire avec les outils MCP (comme les sessions
/// Claude Code, KOB-320).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Acces {
    /// Conversation et notes : la lecture passe, l'écriture demande une
    /// carte (sauf serveur réglé « faire sans demander »).
    Complet,
    /// Question différée (KOB-307) : aucun serveur de l'utilisateur ; du
    /// MCP de Kobi, seuls les outils connus en lecture sont proposés. Rien
    /// ne demande de carte, le reste est refusé.
    LectureSeule,
}

/// Outils de la réunion : lecture des dossiers et serveurs MCP.
pub struct OutillageKobi {
    lecteur: Lecteur,
    serveurs: Vec<ServeurMcp>,
    definitions: Vec<Value>,
    /// Nature et politique de chaque outil, tirées des réglages (dernier
    /// test de connexion), jamais des annotations reçues en direct.
    catalogue: Catalogue,
    /// Nom donné à Claude → serveur et vrai nom de l'outil (KOB-317).
    noms: BTreeMap<String, (String, String)>,
    acces: Acces,
}

impl OutillageKobi {
    /// `serveurs` : clients déjà connectés (voir [`crate::mcp::connecter`]),
    /// avec leur `tools/list`. `catalogue` : le même que pour les sessions
    /// Claude Code ([`crate::mcp::pour_session`]).
    pub fn new(
        dossiers: &[PathBuf],
        serveurs: Vec<(String, RunningService<RoleClient, ()>, Vec<Value>)>,
        catalogue: Catalogue,
        acces: Acces,
    ) -> Self {
        let mut definitions = Lecteur::definitions();
        let mut connectes = Vec::new();
        let mut noms = BTreeMap::new();
        for (nom, client, outils) in serveurs {
            // Lecture seule : aucun serveur de l'utilisateur, seulement le MCP
            // de Kobi (comme la question différée de Claude Code).
            if acces == Acces::LectureSeule && nom != crate::mcp_config::SERVEUR_KOBI {
                tracing::info!(serveur = %nom, "serveur MCP écarté : session en lecture seule");
                continue;
            }
            let mut proposes = Vec::new();
            for o in &outils {
                let Some(outil) = o["name"].as_str() else {
                    continue;
                };
                if acces == Acces::LectureSeule && catalogue.nature(&nom, outil) != Nature::Lecture
                {
                    continue;
                }
                let nom_api = nom_pour_l_api(&nom, outil, &noms);
                noms.insert(nom_api.clone(), (nom.clone(), outil.to_owned()));
                definitions.push(json!({
                    "name": nom_api,
                    "description": o["description"].as_str().unwrap_or_default(),
                    "input_schema": o.get("inputSchema").cloned()
                        .unwrap_or_else(|| json!({ "type": "object" })),
                }));
                proposes.push(outil.to_owned());
            }
            tracing::info!(serveur = %nom, outils = ?proposes, ?acces, "outils MCP du backend clé API");
            connectes.push(ServeurMcp {
                nom,
                client,
                outils: proposes,
            });
        }
        Self {
            lecteur: Lecteur::new(dossiers),
            serveurs: connectes,
            definitions,
            catalogue,
            noms,
            acces,
        }
    }

    /// Ferme les connexions MCP. En réunion, elles se ferment quand la
    /// session (l'`ApiBrain`) est lâchée ; les tests les ferment à la main.
    #[cfg(test)]
    pub async fn fermer(self) {
        for s in self.serveurs {
            let _ = s.client.cancel().await;
        }
    }

    /// Serveur et vrai nom de l'outil MCP proposé sous `nom`.
    fn outil_mcp(&self, nom: &str) -> Option<(&str, &str)> {
        self.noms.get(nom).map(|(s, o)| (s.as_str(), o.as_str()))
    }

    async fn appel_mcp(
        &self,
        serveur: &str,
        outil: &str,
        entree: &Value,
    ) -> Result<String, String> {
        let s = self
            .serveurs
            .iter()
            .find(|s| s.nom == serveur && s.outils.iter().any(|o| o == outil))
            .ok_or_else(|| format!("outil inconnu : {serveur} / {outil}"))?;
        let mut params = rmcp::model::CallToolRequestParams::new(outil.to_owned());
        params.arguments = entree.as_object().cloned();
        let resultat = s
            .client
            .call_tool(params)
            .await
            .map_err(|e| format!("{serveur} : {e}"))?;
        let texte: Vec<String> = resultat
            .content
            .iter()
            .filter_map(|c| serde_json::to_value(c).ok())
            .map(|c| match c["text"].as_str() {
                Some(t) if c["type"] == "text" => t.to_owned(),
                _ => c.to_string(),
            })
            .collect();
        let texte = couper(texte.join("\n"));
        if resultat.is_error == Some(true) {
            Err(texte)
        } else {
            Ok(texte)
        }
    }
}

impl kobi_brain::Outillage for OutillageKobi {
    fn definitions(&self) -> Vec<Value> {
        self.definitions.clone()
    }

    fn action(&self, nom: &str, entree: &Value) -> kobi_brain::Action {
        match self.outil_mcp(nom) {
            Some((serveur, outil)) => kobi_brain::Action::Mcp {
                serveur: serveur.to_owned(),
                outil: outil.to_owned(),
                params: entree.clone(),
            },
            None => kobi_brain::claude_code::outils::action(nom, entree),
        }
    }

    /// Lecture des dossiers : libre. Outil MCP proposé : verdict du
    /// catalogue, comme pour Claude Code (hors catalogue = carte) ; en
    /// lecture seule, ce qui demanderait une carte est refusé. Outil non
    /// proposé : refusé sans carte.
    fn autorisation(&self, nom: &str) -> Autorisation {
        if matches!(nom, "Read" | "Grep" | "Glob") {
            return Autorisation::Libre;
        }
        // Un nom que Kobi n'a pas proposé n'existe pas : refusé sans carte.
        let Some((serveur, outil)) = self.outil_mcp(nom) else {
            return Autorisation::Refus;
        };
        match self.acces {
            Acces::Complet if self.catalogue.verdict(serveur, outil) == Verdict::Libre => {
                Autorisation::Libre
            }
            Acces::Complet => Autorisation::Carte,
            Acces::LectureSeule if self.catalogue.nature(serveur, outil) == Nature::Lecture => {
                Autorisation::Libre
            }
            Acces::LectureSeule => Autorisation::Refus,
        }
    }

    fn appeler<'a>(
        &'a self,
        nom: &'a str,
        entree: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(async move {
            if let Some((serveur, outil)) = self.outil_mcp(nom) {
                return self.appel_mcp(serveur, outil, entree).await;
            }
            if nom.starts_with("mcp__") {
                return Err(format!("outil inconnu : {nom}"));
            }
            let lecteur = self.lecteur.clone();
            let (nom, entree) = (nom.to_owned(), entree.clone());
            // Parcours de disque : hors du runtime.
            tokio::task::spawn_blocking(move || match nom.as_str() {
                "Read" => lecteur.lire(&entree),
                "Grep" => lecteur.chercher(&entree),
                "Glob" => lecteur.lister(&entree),
                _ => Err(format!("outil inconnu : {nom}")),
            })
            .await
            .unwrap_or_else(|e| Err(e.to_string()))
        })
    }
}

/// Chemin réseau ou de périphérique Windows : `\\serveur\partage`,
/// `\\?\UNC\…`, `\\.\pipe\…`, `\\?\GLOBALROOT\…`, et leurs formes en `/`.
/// Le résoudre (`canonicalize`) ouvre déjà un handle : Windows se connecte au
/// serveur en SMB et lui envoie le hash NTLM de l'utilisateur, ou se branche
/// sur un tube (KOB-344). Seul le préfixe verbatim d'un disque local
/// (`\\?\C:\`, que rend `canonicalize` sous Windows) passe. Jugé sur le texte,
/// le même sous tous les systèmes.
fn chemin_reseau(chemin: &str) -> bool {
    let c = chemin.replace('/', "\\");
    // `\??\UNC\…`, `\??\pipe\…` : passés tels quels à NT.
    if c.starts_with("\\??\\") {
        return true;
    }
    if !c.starts_with("\\\\") {
        return false;
    }
    let disque_local = c.strip_prefix("\\\\?\\").is_some_and(|reste| {
        let b = reste.as_bytes();
        b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\'
    });
    !disque_local
}

/// Un lien du chemin vise-t-il, directement ou par d'autres liens, un chemin
/// réseau ou de périphérique (KOB-352) ? Sous Windows, `canonicalize` le
/// suivrait et ouvrirait la connexion SMB. Le chemin est résolu comme le
/// ferait le système, sans rien suivre : chaque lien est lu (`read_link`) et
/// **remplacé** par sa cible, et `..` s'applique au chemin déjà résolu
/// (physique), pas au nom du lien. Plus de [`LIENS_MAX`] liens en tout
/// (boucle, chaîne sans fin) : refusé.
///
/// Avec `peripheriques` (Windows), un composant qui est un nom de
/// périphérique DOS sous un chemin non verbatim est refusé avant d'être
/// ouvert : Win32 changerait `C:\…\COM1` en `\\.\COM1` (KOB-353). Même au
/// milieu du chemin, puisque chaque préfixe est ouvert ici. « Verbatim » se
/// juge sur le chemin donné, celui que Win32 traduit : la cible absolue d'un
/// lien, rendue en `\\?\…`, ne le rend pas verbatim (KOB-367).
fn lien_vers_le_reseau(chemin: &Path, peripheriques: bool) -> bool {
    use std::collections::VecDeque;
    use std::path::Component;
    let mut a_lire: VecDeque<PathBuf> = chemin
        .components()
        .map(|c| PathBuf::from(c.as_os_str()))
        .collect();
    let mut resolu = PathBuf::new();
    let mut liens = 0;
    let verbatim = chemin_verbatim(&chemin.to_string_lossy());
    while let Some(morceau) = a_lire.pop_front() {
        match morceau.components().next() {
            Some(Component::CurDir) | None => continue,
            Some(Component::ParentDir) => {
                resolu.pop();
                continue;
            }
            Some(Component::Prefix(_) | Component::RootDir) => {
                resolu.push(&morceau);
                continue;
            }
            Some(Component::Normal(_)) => {}
        }
        // Un nom que Win32 changerait (point ou espace final, KOB-398) est
        // refusé comme un nom de périphérique : le chemin ouvert ne serait
        // pas celui qui est jugé ici. Un flux NTFS (`x::$DATA`) l'est même
        // en verbatim : NT l'interprète aussi.
        if peripheriques
            && ((!verbatim
                && (nom_de_peripherique(morceau.as_os_str()) || fin_ambigue(morceau.as_os_str())))
                || morceau.to_string_lossy().contains(':'))
        {
            return true;
        }
        let candidat = resolu.join(&morceau);
        // Sondé tel que NT l'ouvrira, sans traduction Win32 (KOB-398).
        let sonde = pour_nt(&candidat);
        let est_un_lien =
            std::fs::symlink_metadata(&sonde).is_ok_and(|m| m.file_type().is_symlink());
        if !est_un_lien {
            resolu = candidat;
            continue;
        }
        liens += 1;
        if liens > LIENS_MAX {
            return true;
        }
        let Ok(cible) = std::fs::read_link(&sonde) else {
            return true;
        };
        if chemin_reseau(&cible.to_string_lossy())
            || !prefixe_local(&cible)
            || racine_sans_disque(&cible)
        {
            return true;
        }
        // La cible prend la place du lien : absolue, elle repart de sa
        // racine ; relative, du dossier où est le lien.
        if cible.is_absolute() {
            resolu = PathBuf::new();
        }
        for composant in cible.components().rev() {
            a_lire.push_front(PathBuf::from(composant.as_os_str()));
        }
    }
    false
}

/// `.` et `..` retirés sur le texte, sans remonter au-dessus de la racine ni
/// du préfixe, sans rien lire sur le disque (KOB-352, Windows).
#[cfg_attr(not(windows), allow(dead_code))]
fn reduire_sur_le_texte(chemin: &Path) -> PathBuf {
    use std::path::Component;
    let mut reduit = PathBuf::new();
    for composant in chemin.components() {
        match composant {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(reduit.components().next_back(), Some(Component::Normal(_))) {
                    reduit.pop();
                }
            }
            autre => reduit.push(autre.as_os_str()),
        }
    }
    reduit
}

/// Liens lus au plus pour un chemin, comme Linux (KOB-352).
const LIENS_MAX: usize = 40;

/// Sous Windows, une cible de lien qui a une racine sans disque
/// (`\Device\Mup\…`, `\??\…` déjà vu) : NT la suit telle quelle, vers le
/// réseau ou un périphérique (KOB-352). Ailleurs, une racine est locale.
fn racine_sans_disque(cible: &Path) -> bool {
    cfg!(windows)
        && cible.has_root()
        && !matches!(
            cible.components().next(),
            Some(std::path::Component::Prefix(_))
        )
}

/// Chemin sondé sur le disque tel que NT l'ouvrira : sous Windows, un
/// chemin à disque (`C:\…`) l'est en `\\?\C:\…`, que Win32 ne traduit pas.
/// Une étape du parcours (cible de lien rendue en `C:\…`, suite du chemin)
/// n'est ainsi jamais vue sous un autre nom que celui que NT suivra : un
/// `x.` resterait sinon `x` à la sonde, et son lien passerait (KOB-398).
fn pour_nt(chemin: &Path) -> PathBuf {
    #[cfg(windows)]
    if let Some(std::path::Component::Prefix(p)) = chemin.components().next() {
        if matches!(p.kind(), std::path::Prefix::Disk(_)) {
            return PathBuf::from(format!(r"\\?\{}", chemin.display()));
        }
    }
    chemin.to_path_buf()
}

/// Un nom à point ou espace final (`x.`, `d `) : Win32 les retire d'un
/// chemin non verbatim, qui n'ouvre donc pas le nom jugé (KOB-398).
fn fin_ambigue(nom: &std::ffi::OsStr) -> bool {
    let nom = nom.to_string_lossy();
    nom.ends_with('.') || nom.ends_with(' ')
}

/// `\\?\…` : Win32 n'y change rien, ni `..` ni nom de périphérique.
fn chemin_verbatim(chemin: &str) -> bool {
    // `//?/` n'est pas verbatim : Win32 le normalise.
    chemin.starts_with("\\\\?\\")
}

/// Un nom de périphérique DOS réservé (`CON`, `NUL`, `COM1`, `lpt¹.txt`…),
/// sans égard à la casse, à l'extension, au flux ni aux espaces de fin
/// (KOB-353, Windows).
fn nom_de_peripherique(nom: &std::ffi::OsStr) -> bool {
    let nom = nom.to_string_lossy();
    let base = nom
        .split(['.', ':'])
        .next()
        .unwrap_or("")
        .trim_end_matches(' ');
    let base = base.to_ascii_uppercase();
    if ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].contains(&base.as_str()) {
        return true;
    }
    let Some(numero) = base
        .strip_prefix("COM")
        .or_else(|| base.strip_prefix("LPT"))
    else {
        return false;
    };
    let mut chiffres = numero.chars();
    matches!(
        (chiffres.next(), chiffres.next()),
        (Some('0'..='9' | '¹' | '²' | '³'), None)
    )
}

/// Sous Windows, liste blanche : un chemin absolu n'a pour préfixe qu'un
/// disque local (`C:`, `\\?\C:`) ; tout autre préfixe (UNC, périphérique,
/// verbatim) est refusé avant résolution (KOB-344). Ailleurs, rien à juger.
fn prefixe_local(chemin: &Path) -> bool {
    use std::path::{Component, Prefix};
    match chemin.components().next() {
        Some(Component::Prefix(p)) => {
            matches!(p.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_))
        }
        _ => true,
    }
}

#[cfg(test)]
mod tests;
