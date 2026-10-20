//! Appels d'outils normalisés, indépendants du harness (KOB-157).
//!
//! L'adaptateur d'un harness (Claude Code : `kobi_brain::claude_code::outils`)
//! traduit ses noms d'outils et leurs arguments en [`Action`] ; les règles de
//! Kobi (classement [`crate::classer`], validation, fil d'activité de KOB-152)
//! ne voient que ce type. Voir « Kobi — contrat commun des permissions »,
//! section 0.

use std::path::{Component, Path, PathBuf};

use kobi_i18n::tr;
use serde::Serialize;
use serde_json::Value;

/// Interpréteur d'une [`Action::Commande`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Bash,
    PowerShell,
}

/// Ce qu'un outil s'apprête à faire.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    LireFichier {
        chemin: PathBuf,
    },
    Chercher {
        motif: String,
        dossier: Option<PathBuf>,
    },
    Lister {
        dossier: PathBuf,
    },
    Commande {
        ligne: String,
        dossier: Option<PathBuf>,
        shell: Shell,
    },
    EcrireFichier {
        chemin: PathBuf,
        avant: Option<String>,
        apres: String,
    },
    SupprimerFichier {
        chemin: PathBuf,
    },
    Mcp {
        serveur: String,
        outil: String,
        params: Value,
    },
    Reglage {
        cle: String,
        valeur: Value,
    },
    Autre {
        nom: String,
        entree: Value,
    },
}

/// Appel d'outil d'un tour.
#[derive(Debug, Clone, PartialEq)]
pub struct DemandeOutil {
    pub id: String,
    pub tour: u64,
    pub action: Action,
}

/// Famille d'une étape, pour l'icône et l'animation de Kobi.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Famille {
    Lecture,
    Recherche,
    Commande,
    Mcp,
    Ecriture,
    Reglage,
}

/// Une ligne du fil d'activité : ce que fait Kobi, en une phrase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Etape {
    #[serde(rename = "type")]
    pub famille: Famille,
    pub resume: String,
    /// Chemin montré, relatif au dossier de code qui le contient.
    pub chemin_relatif: Option<String>,
}

/// Longueur maximale d'une commande ou d'un motif dans le fil.
const COURT: usize = 60;

impl Action {
    pub fn famille(&self) -> Famille {
        match self {
            Action::LireFichier { .. } => Famille::Lecture,
            Action::Chercher { .. } | Action::Lister { .. } => Famille::Recherche,
            Action::Commande { .. } => Famille::Commande,
            Action::Mcp { .. } => Famille::Mcp,
            Action::EcrireFichier { .. } | Action::SupprimerFichier { .. } => Famille::Ecriture,
            Action::Reglage { .. } => Famille::Reglage,
            Action::Autre { .. } => Famille::Commande,
        }
    }

    /// Résumé d'une ligne ; les chemins sont raccourcis relativement au
    /// dossier de code (`dossiers`) qui les contient, les secrets masqués.
    pub fn etape(&self, dossiers: &[PathBuf]) -> Etape {
        let rel = |p: &Path| relatif(p, dossiers);
        let (resume, chemin) = match self {
            Action::LireFichier { chemin } => {
                let r = rel(chemin);
                (tr("perms.action.lis", &[("chemin", &r)]), Some(r))
            }
            Action::Chercher { motif, dossier } => {
                let motif = court(&masquer(motif), COURT);
                match dossier {
                    Some(d) => {
                        let r = rel(d);
                        let texte = tr(
                            "perms.action.chercheDans",
                            &[("motif", &motif), ("dossier", &r)],
                        );
                        (texte, Some(r))
                    }
                    None => (tr("perms.action.cherche", &[("motif", &motif)]), None),
                }
            }
            Action::Lister { dossier } => {
                let r = rel(dossier);
                (tr("perms.action.parcours", &[("dossier", &r)]), Some(r))
            }
            Action::Commande { ligne, dossier, .. } => {
                let ligne = court(&masquer(ligne.lines().next().unwrap_or_default()), COURT);
                (
                    tr("perms.action.lance", &[("commande", &ligne)]),
                    dossier.as_deref().map(&rel),
                )
            }
            Action::EcrireFichier { chemin, avant, .. } => {
                let r = rel(chemin);
                let texte = if avant.is_some() {
                    tr("perms.action.modifie", &[("chemin", &r)])
                } else {
                    tr("perms.action.ecris", &[("chemin", &r)])
                };
                (texte, Some(r))
            }
            Action::SupprimerFichier { chemin } => {
                let r = rel(chemin);
                (tr("perms.action.supprime", &[("chemin", &r)]), Some(r))
            }
            // Les outils de réunion de Kobi disent ce qu'ils font (KOB-245) ;
            // la refonte des autres libellés MCP est celle de KOB-242.
            Action::Mcp { serveur, outil, .. }
                if serveur == "kobi" && reunion_kobi(outil).is_some() =>
            {
                (reunion_kobi(outil).unwrap_or_default(), None)
            }
            Action::Mcp {
                serveur,
                outil,
                params,
            } => (resume_mcp(serveur, outil, params), None),
            Action::Reglage { cle, .. } => {
                (tr("perms.action.changeReglage", &[("cle", cle)]), None)
            }
            Action::Autre { nom, .. } => (tr("perms.action.utilise", &[("nom", nom)]), None),
        };
        Etape {
            famille: self.famille(),
            resume,
            chemin_relatif: chemin,
        }
    }
}

/// Libellé d'un outil de réunion du MCP de Kobi.
fn reunion_kobi(outil: &str) -> Option<String> {
    Some(match outil {
        "meeting_start" => tr("perms.action.demarreReunion", &[]),
        "meeting_stop" => tr("perms.action.termineReunion", &[]),
        "meeting_pause" => tr("perms.action.pauseReunion", &[]),
        "meeting_resume" => tr("perms.action.reprendReunion", &[]),
        _ => return None,
    })
}

/// Chemin relatif au dossier de code le plus précis qui le contient
/// (séparateurs `/`). Un dossier de code lui-même est montré par son nom ;
/// un chemin hors des dossiers, par ses deux derniers éléments.
pub fn relatif(chemin: &Path, dossiers: &[PathBuf]) -> String {
    let normal = |p: &Path| -> Vec<String> {
        p.components()
            .filter(|c| !matches!(c, Component::CurDir))
            .map(|c| {
                c.as_os_str()
                    .to_string_lossy()
                    .to_lowercase()
                    .replace('\\', "/")
            })
            .collect()
    };
    let morceaux: Vec<String> = chemin
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let bas = normal(chemin);
    let racine = dossiers
        .iter()
        .map(|d| (d, normal(d)))
        .filter(|(_, d)| !d.is_empty() && bas.starts_with(d))
        .max_by_key(|(_, d)| d.len());
    if let Some((dossier, d)) = racine {
        if d.len() == bas.len() {
            return dossier
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| dossier.display().to_string());
        }
        return morceaux[d.len()..].join("/");
    }
    if chemin.is_relative() || morceaux.len() <= 2 {
        return morceaux.join("/").trim_start_matches('/').to_owned();
    }
    format!("…/{}", morceaux[morceaux.len() - 2..].join("/"))
}

/// Ce que fait un outil MCP, d'après son nom (KOB-242). Séparé du
/// classement lecture / écriture de la carte de validation : un outil
/// classé écriture par prudence n'est pas décrit comme une création.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VerbeMcp {
    Chercher,
    Lire,
    Creer,
    Modifier,
    /// `save`, `upsert` : modifier si un identifiant est donné, sinon créer.
    Enregistrer,
    Supprimer,
    Appeler,
}

/// Mots des noms d'outils et leur verbe. Une seule table, à traduire
/// (KOB-207).
const VERBES_MCP: &[(&str, VerbeMcp)] = &[
    ("search", VerbeMcp::Chercher),
    ("find", VerbeMcp::Chercher),
    ("list", VerbeMcp::Chercher),
    ("query", VerbeMcp::Chercher),
    ("lookup", VerbeMcp::Chercher),
    ("get", VerbeMcp::Lire),
    ("read", VerbeMcp::Lire),
    ("fetch", VerbeMcp::Lire),
    ("view", VerbeMcp::Lire),
    ("show", VerbeMcp::Lire),
    ("describe", VerbeMcp::Lire),
    ("retrieve", VerbeMcp::Lire),
    ("create", VerbeMcp::Creer),
    ("add", VerbeMcp::Creer),
    ("write", VerbeMcp::Creer),
    ("insert", VerbeMcp::Creer),
    ("new", VerbeMcp::Creer),
    ("post", VerbeMcp::Creer),
    ("update", VerbeMcp::Modifier),
    ("edit", VerbeMcp::Modifier),
    ("patch", VerbeMcp::Modifier),
    ("modify", VerbeMcp::Modifier),
    ("rename", VerbeMcp::Modifier),
    ("set", VerbeMcp::Modifier),
    ("save", VerbeMcp::Enregistrer),
    ("upsert", VerbeMcp::Enregistrer),
    ("delete", VerbeMcp::Supprimer),
    ("remove", VerbeMcp::Supprimer),
    ("archive", VerbeMcp::Supprimer),
    ("destroy", VerbeMcp::Supprimer),
    ("call", VerbeMcp::Appeler),
    ("run", VerbeMcp::Appeler),
    ("execute", VerbeMcp::Appeler),
    ("invoke", VerbeMcp::Appeler),
];

/// Noms de services écrits comme leurs auteurs ; les autres restent tels
/// quels (« treg »).
const SERVICES: &[(&str, &str)] = &[
    ("linear", "Linear"),
    ("github", "GitHub"),
    ("gitlab", "GitLab"),
    ("notion", "Notion"),
    ("slack", "Slack"),
    ("jira", "Jira"),
    ("confluence", "Confluence"),
    ("figma", "Figma"),
    ("sentry", "Sentry"),
    ("stripe", "Stripe"),
    ("cloudflare", "Cloudflare"),
    ("deepwiki", "DeepWiki"),
];

/// Arguments qui disent sur quoi porte l'appel, du plus parlant au moins.
const ARGUMENTS_PRINCIPAUX: &[&str] = &[
    "query",
    "q",
    "search",
    "requete",
    "keyword",
    "keywords",
    "title",
    "titre",
    "name",
    "nom",
    "tool",
    "method",
    "id",
    "identifier",
    "issueId",
    "url",
];

/// Mots d'un nom d'outil : `catalog_search`, `get-issue`, `listIssues`.
fn mots(outil: &str) -> Vec<String> {
    let mut mots = Vec::new();
    let mut mot = String::new();
    let mut precedent_minuscule = false;
    for c in outil.chars() {
        if c == '_' || c == '-' || c == '.' || c == ' ' {
            if !mot.is_empty() {
                mots.push(std::mem::take(&mut mot));
            }
            precedent_minuscule = false;
            continue;
        }
        if c.is_uppercase() && precedent_minuscule && !mot.is_empty() {
            mots.push(std::mem::take(&mut mot));
        }
        precedent_minuscule = c.is_lowercase() || c.is_ascii_digit();
        mot.extend(c.to_lowercase());
    }
    if !mot.is_empty() {
        mots.push(mot);
    }
    mots
}

/// Le dernier mot reconnu l'emporte : `catalog_search` cherche.
fn verbe_mcp(outil: &str) -> Option<VerbeMcp> {
    mots(outil)
        .iter()
        .rev()
        .find_map(|m| VERBES_MCP.iter().find(|(mot, _)| mot == m).map(|(_, v)| *v))
}

/// Argument principal (requête, titre, identifiant), masqué et tronqué ;
/// à défaut, les deux premières valeurs simples.
fn complement_mcp(params: &Value) -> String {
    let principal = ARGUMENTS_PRINCIPAUX
        .iter()
        .find_map(|cle| match params.get(cle)? {
            Value::String(s) if !s.trim().is_empty() => Some(court(&masquer(s), 40)),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        });
    principal.unwrap_or_else(|| parametres(params))
}

fn a_un_identifiant(params: &Value) -> bool {
    params.as_object().is_some_and(|o| {
        o.iter().any(|(cle, v)| {
            let cle = cle.to_lowercase();
            (cle == "id" || cle.ends_with("id") || cle == "identifier")
                && (v.as_str().is_some_and(|s| !s.is_empty()) || v.is_number())
        })
    })
}

/// « Je cherche dans treg : mot clé », « J'appelle treg : export » ; sans
/// verbe sûr, « J'utilise treg · catalog_search ».
fn resume_mcp(serveur: &str, outil: &str, params: &Value) -> String {
    let service = SERVICES
        .iter()
        .find(|(nom, _)| nom.eq_ignore_ascii_case(serveur))
        .map_or(serveur, |(_, beau)| beau);
    let Some(verbe) = verbe_mcp(outil) else {
        return tr(
            "perms.action.utiliseOutil",
            &[("service", service), ("outil", outil)],
        );
    };
    let verbe = match verbe {
        VerbeMcp::Enregistrer if a_un_identifiant(params) => VerbeMcp::Modifier,
        VerbeMcp::Enregistrer => VerbeMcp::Creer,
        v => v,
    };
    let service = [("service", service)];
    let debut = match verbe {
        VerbeMcp::Chercher => tr("perms.action.chercheDansService", &service),
        VerbeMcp::Lire => tr("perms.action.lisDansService", &service),
        VerbeMcp::Creer | VerbeMcp::Enregistrer => tr("perms.action.creeDansService", &service),
        VerbeMcp::Modifier => tr("perms.action.modifieDansService", &service),
        VerbeMcp::Supprimer => tr("perms.action.supprimeDansService", &service),
        VerbeMcp::Appeler => tr("perms.action.appelleService", &service),
    };
    let complement = complement_mcp(params);
    if complement.is_empty() {
        debut
    } else {
        tr(
            "perms.action.avecComplement",
            &[("debut", &debut), ("complement", &complement)],
        )
    }
}

/// Coupe au-delà de `max` caractères.
fn court(texte: &str, max: usize) -> String {
    let texte = texte.trim();
    if texte.chars().count() <= max {
        return texte.to_owned();
    }
    let mut s: String = texte.chars().take(max - 1).collect();
    s.push('…');
    s
}

/// Deux premières valeurs simples des paramètres d'un outil MCP.
fn parametres(params: &Value) -> String {
    let Some(objet) = params.as_object() else {
        return String::new();
    };
    objet
        .iter()
        .filter(|(cle, _)| !secret_de_nom(cle))
        .filter_map(|(cle, v)| {
            let v = match v {
                Value::String(s) => court(&masquer(s), 30),
                Value::Number(n) => n.to_string(),
                Value::Bool(b) => b.to_string(),
                _ => return None,
            };
            Some(format!("{cle} {v}"))
        })
        .take(2)
        .collect::<Vec<_>>()
        .join(", ")
}

fn secret_de_nom(nom: &str) -> bool {
    let nom = nom.to_lowercase();
    [
        "token",
        "secret",
        "password",
        "passwd",
        "api_key",
        "apikey",
        "authorization",
        "cle",
        "key",
    ]
    .iter()
    .any(|m| nom.contains(m))
}

/// Masque ce qui ressemble à un secret : jeton après `Bearer`, valeur d'une
/// option ou d'une variable dont le nom parle de clé, de jeton ou de mot de
/// passe, clés au format connu (`sk-…`, `ghp_…`, `lin_api_…`…).
pub fn masquer(texte: &str) -> String {
    const PREFIXES: [&str; 7] = [
        "sk-",
        "ghp_",
        "gho_",
        "github_pat_",
        "lin_api_",
        "xoxb-",
        "AKIA",
    ];
    let mots: Vec<&str> = texte.split(' ').collect();
    let mut sortie = Vec::with_capacity(mots.len());
    let mut suivant_secret = false;
    for mot in mots {
        let nu = mot.trim_matches(|c| c == '"' || c == '\'');
        if suivant_secret && !mot.is_empty() && !nu.eq_ignore_ascii_case("bearer") {
            // Le guillemet fermant reste, pour que la commande se lise.
            let guillemet = &mot[mot.trim_end_matches(['"', '\'']).len()..];
            sortie.push(format!("•••{guillemet}"));
            suivant_secret = false;
            continue;
        }
        if nu.eq_ignore_ascii_case("bearer") || nu.eq_ignore_ascii_case("authorization:") {
            suivant_secret = true;
            sortie.push(mot.to_owned());
            continue;
        }
        if let Some((nom, _)) = mot.split_once('=') {
            if secret_de_nom(nom.trim_start_matches('-')) {
                sortie.push(format!("{nom}=•••"));
                continue;
            }
        }
        if (mot.starts_with("--") || mot.starts_with('-')) && secret_de_nom(mot) {
            suivant_secret = true;
            sortie.push(mot.to_owned());
            continue;
        }
        if PREFIXES
            .iter()
            .any(|p| nu.starts_with(p) && nu.len() > p.len() + 8)
        {
            sortie.push("•••".to_owned());
            continue;
        }
        sortie.push(mot.to_owned());
    }
    sortie.join(" ")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn code() -> Vec<PathBuf> {
        vec![
            PathBuf::from("C:/Users/moi/Desktop/kobi"),
            PathBuf::from("C:/Users/moi/Desktop"),
        ]
    }

    #[test]
    fn chemins_relatifs_au_dossier_le_plus_precis() {
        let d = code();
        assert_eq!(
            relatif(
                Path::new("C:/Users/moi/Desktop/kobi/crates/kobi-app/src/chef.rs"),
                &d
            ),
            "crates/kobi-app/src/chef.rs"
        );
        // Les antislashs ne séparent les éléments d'un chemin que sous Windows.
        if cfg!(windows) {
            assert_eq!(
                relatif(Path::new(r"C:\Users\moi\Desktop\kobi\README.md"), &d),
                "README.md"
            );
        }
        assert_eq!(
            relatif(Path::new("c:/users/moi/desktop/site/index.html"), &d),
            "site/index.html"
        );
        assert_eq!(relatif(Path::new("C:/Users/moi/Desktop/kobi"), &d), "kobi");
        // Un chemin absolu hors des dossiers de code (« D:/… » n'est absolu que sous Windows).
        let ailleurs = if cfg!(windows) {
            "D:/autre/projet/src/main.rs"
        } else {
            "/autre/projet/src/main.rs"
        };
        assert_eq!(relatif(Path::new(ailleurs), &d), "…/src/main.rs");
        assert_eq!(relatif(Path::new("src/lib.rs"), &d), "src/lib.rs");
    }

    #[test]
    fn resumes_lisibles() {
        let d = code();
        let lire = Action::LireFichier {
            chemin: "C:/Users/moi/Desktop/kobi/crates/kobi-app/src/chef.rs".into(),
        };
        let e = lire.etape(&d);
        assert_eq!(e.resume, "Je lis crates/kobi-app/src/chef.rs");
        assert_eq!(e.famille, Famille::Lecture);
        let chercher = Action::Chercher {
            motif: "fabrique_claude".into(),
            dossier: Some("C:/Users/moi/Desktop/kobi".into()),
        };
        assert_eq!(
            chercher.etape(&d).resume,
            "Je cherche « fabrique_claude » dans kobi"
        );
        let git = Action::Commande {
            ligne: "git log -5\ngit status".into(),
            dossier: None,
            shell: Shell::Bash,
        };
        assert_eq!(git.etape(&d).resume, "Je lance git log -5");
        let mcp = Action::Mcp {
            serveur: "linear".into(),
            outil: "list_issues".into(),
            params: json!({"state": "In Progress", "limit": 5, "filtre": {"a": 1}}),
        };
        let e = mcp.etape(&d);
        assert_eq!(
            e.resume,
            "Je cherche dans Linear : limit 5, state In Progress"
        );
        assert_eq!(e.famille, Famille::Mcp);
        // KOB-245 : les outils de réunion de Kobi disent ce qu'ils font.
        let fin = Action::Mcp {
            serveur: "kobi".into(),
            outil: "meeting_stop".into(),
            params: json!({}),
        };
        assert_eq!(fin.etape(&d).resume, "Je termine la réunion");
        let pause = Action::Mcp {
            serveur: "kobi".into(),
            outil: "meeting_pause".into(),
            params: json!({}),
        };
        assert_eq!(pause.etape(&d).resume, "Je mets la réunion en pause");
        let long = Action::Commande {
            ligne: "x".repeat(200),
            dossier: None,
            shell: Shell::PowerShell,
        };
        assert!(long.etape(&d).resume.chars().count() <= "Je lance ".len() + COURT);
    }

    #[test]
    fn secrets_masques() {
        assert_eq!(
            masquer("curl -H \"Authorization: Bearer abcdef123456\" https://x"),
            "curl -H \"Authorization: Bearer •••\" https://x"
        );
        assert_eq!(masquer("API_KEY=sk-123 cargo run"), "API_KEY=••• cargo run");
        assert_eq!(
            masquer("gh auth --token ghp_aaaaaaaaaaaa"),
            "gh auth --token •••"
        );
        assert_eq!(masquer("echo sk-ant-api03-zzzzzzzzzzzz"), "echo •••");
        assert_eq!(masquer("git log -5"), "git log -5");
        let mcp = Action::Mcp {
            serveur: "x".into(),
            outil: "appel".into(),
            params: json!({"api_key": "secret", "nom": "ok"}),
        };
        assert_eq!(mcp.etape(&[]).resume, "J'utilise x · appel");
        let mcp = Action::Mcp {
            serveur: "x".into(),
            outil: "search".into(),
            params: json!({"api_key": "secret", "filtre": "ok"}),
        };
        assert_eq!(mcp.etape(&[]).resume, "Je cherche dans x : filtre ok");
    }

    fn resume(serveur: &str, outil: &str, params: Value) -> String {
        Action::Mcp {
            serveur: serveur.into(),
            outil: outil.into(),
            params,
        }
        .etape(&[])
        .resume
    }

    /// KOB-210 : les libellés des cartes et du fil d'activité existent en
    /// anglais (la langue courante n'est jamais réglée dans un test).
    #[test]
    fn libelles_anglais() {
        use kobi_i18n::{t, Langue};
        assert_eq!(
            t(Langue::En, "perms.action.lis", &[("chemin", "src/main.rs")]),
            "Reading src/main.rs"
        );
        let debut = t(
            Langue::En,
            "perms.action.chercheDansService",
            &[("service", "Linear")],
        );
        assert_eq!(
            t(
                Langue::En,
                "perms.action.avecComplement",
                &[("debut", &debut), ("complement", "rafale")]
            ),
            "Searching Linear: rafale"
        );
        assert_eq!(
            t(Langue::En, "perms.action.termineReunion", &[]),
            "Ending the meeting"
        );
    }

    /// KOB-242 : les quatre appels du journal du 20/10, et des outils réels.
    /// Le verbe vient du nom de l'outil, pas du classement prudent.
    #[test]
    fn libelles_des_outils_mcp() {
        // Journal du propriétaire (treg).
        assert_eq!(
            resume("treg", "catalog_search", json!({"query": "facture client"})),
            "Je cherche dans treg : facture client"
        );
        assert_eq!(
            resume("treg", "catalog_get", json!({"id": "cat_42"})),
            "Je lis dans treg : cat_42"
        );
        assert_eq!(
            resume(
                "treg",
                "call",
                json!({"tool": "export_pdf", "args": {"n": 1}})
            ),
            "J'appelle treg : export_pdf"
        );
        // Linear.
        assert_eq!(
            resume(
                "linear",
                "list_issues",
                json!({"query": "rafale", "limit": 5})
            ),
            "Je cherche dans Linear : rafale"
        );
        assert_eq!(
            resume("linear", "get_issue", json!({"id": "KOB-238"})),
            "Je lis dans Linear : KOB-238"
        );
        assert_eq!(
            resume(
                "linear",
                "save_issue",
                json!({"title": "Nouveau ticket", "team": "Kobi"})
            ),
            "Je crée dans Linear : Nouveau ticket"
        );
        assert_eq!(
            resume(
                "linear",
                "save_issue",
                json!({"id": "KOB-12", "title": "Renommé"})
            ),
            "Je modifie dans Linear : Renommé"
        );
        assert_eq!(
            resume("linear", "delete_comment", json!({"id": "c1"})),
            "Je supprime dans Linear : c1"
        );
        // Notion, GitHub (camelCase, tirets).
        assert_eq!(
            resume("notion", "notion-search", json!({"query": "roadmap"})),
            "Je cherche dans Notion : roadmap"
        );
        assert_eq!(
            resume(
                "notion",
                "notion-create-pages",
                json!({"pages": [{"title": "x"}]})
            ),
            "Je crée dans Notion"
        );
        assert_eq!(
            resume(
                "github",
                "create_issue",
                json!({"owner": "o", "repo": "r", "title": "Bug"})
            ),
            "Je crée dans GitHub : Bug"
        );
        assert_eq!(
            resume("github", "searchCode", json!({"q": "fn main"})),
            "Je cherche dans GitHub : fn main"
        );
        // Rien de sûr : le nom tel quel.
        assert_eq!(
            resume("treg", "catalog", json!({})),
            "J'utilise treg · catalog"
        );
        // Jamais un secret, et un complément tronqué.
        assert_eq!(
            resume(
                "treg",
                "query_db",
                json!({"token": "lin_api_123", "q": "x".repeat(80)})
            ),
            format!("Je cherche dans treg : {}…", "x".repeat(39))
        );
    }
}
