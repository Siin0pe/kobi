//! Sessions du backend clé API (KOB-101 à 103) : ce que
//! [`crate::chef::fabrique_claude`] ouvre quand `brain = "api"`, ou en
//! `auto` quand Claude Code manque et qu'une clé est enregistrée.
//!
//! Même prompt, mêmes outils et mêmes règles que les sessions Claude Code
//! du même rôle :
//! * lecture des dossiers de code et de la mémoire des réunions (`Read`,
//!   `Grep`, `Glob`) ;
//! * MCP de Kobi (`kobi mcp --agent`, ou `--lecture` pour une question
//!   différée), dont les outils d'écriture demandent eux-mêmes le « oui » ;
//!   les outils qui pilotent la réunion sont retirés des notes ;
//! * serveurs MCP de l'utilisateur (pas pour une question différée), avec
//!   la carte de confirmation de Kobi pour ce qui écrit.

use std::path::PathBuf;
use std::sync::Arc;

use kobi_brain::{ApiBrain, ApiConfig, BrainError};
use kobi_perms::mcp::Catalogue;
use kobi_session::config::Brain;
use kobi_session::mcp::{Serveur, Transport};
use serde_json::Value;

use crate::cerveau::Role;
use crate::lecture::Memoire;
use crate::mcp::{Coffre, CoffreSysteme};
use crate::mcp_config::SERVEUR_KOBI;
use crate::outils_api::Acces;

/// Réglage `brain` de `config.toml`.
pub fn reglage() -> Brain {
    kobi_session::config::default_path()
        .map(|p| kobi_session::config::load_or_create(&p).config.brain)
        .unwrap_or_default()
}

/// Backend d'une session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choix {
    ClaudeCode,
    Api,
    /// `brain = "api"` sans clé enregistrée.
    SansCle,
}

/// `auto` : Claude Code s'il est prêt, sinon la clé API s'il y en a une
/// (sinon l'erreur de Claude Code). `api` : la clé, jamais Claude Code.
pub fn choisir(reglage: Brain, claude_code_pret: bool, cle: bool) -> Choix {
    match reglage {
        Brain::ClaudeCode => Choix::ClaudeCode,
        Brain::Api if cle => Choix::Api,
        Brain::Api => Choix::SansCle,
        Brain::Auto if !claude_code_pret && cle => Choix::Api,
        Brain::Auto => Choix::ClaudeCode,
    }
}

/// Clé API enregistrée (KOB-104), s'il y en a une.
pub fn cle() -> Option<String> {
    crate::cle_api::lire(&CoffreSysteme)
        .map_err(|e| tracing::warn!(erreur = %e, "clé API illisible"))
        .ok()
        .flatten()
}

/// Ouvre une session du backend clé API pour `role`.
pub async fn ouvrir(
    role: Role,
    cle: String,
    codebases: Arc<Vec<PathBuf>>,
    memoire: Option<Memoire>,
) -> Result<ApiBrain, BrainError> {
    let cfg = kobi_session::config::default_path()
        .map(|p| kobi_session::config::load_or_create(&p).config)
        .unwrap_or_default();
    let coffre: Arc<dyn Coffre> = Arc::new(CoffreSysteme);
    let differee = role == Role::Differee;
    let exe = std::env::current_exe()
        .map_err(|e| BrainError::Unavailable(format!("exécutable de Kobi : {e}")))?;

    // Serveurs de l'utilisateur : ni pour une question différée, ni sans
    // nom valide (comme pour Claude Code).
    // Catalogue des réglages : nature et politique de chaque outil, comme
    // pour Claude Code (KOB-320).
    let (mut serveurs, mut catalogue, prompt_utilisateur) = if differee {
        (Vec::new(), Catalogue::default(), None)
    } else {
        let session = crate::mcp::pour_session(&cfg.mcp, coffre.as_ref());
        let serveurs = crate::mcp::connecter(&cfg.mcp, coffre.clone()).await;
        let noms: Vec<&str> = serveurs.iter().map(|(n, _, _)| n.as_str()).collect();
        let prompt = (!noms.is_empty()).then(|| crate::mcp::prompt(&noms));
        (serveurs, session.catalogue, prompt)
    };

    // MCP de Kobi : ses outils d'écriture demandent eux-mêmes le « oui ».
    let kobi = Serveur {
        transport: Transport::Stdio,
        commande: exe.display().to_string(),
        args: vec![
            "mcp".into(),
            if differee { "--lecture" } else { "--agent" }.into(),
        ],
        ..Default::default()
    };
    match crate::mcp::connecter_un(SERVEUR_KOBI, &kobi, coffre).await {
        Some((nom, client, outils)) => {
            let (permis, retires) = outils_kobi(role);
            let garde = |o: &str| permis.iter().any(|p| p == o) && !retires.iter().any(|r| r == o);
            let outils: Vec<Value> = outils
                .into_iter()
                .filter(|o| garde(o["name"].as_str().unwrap_or_default()))
                .collect();
            catalogue
                .serveurs
                .insert(SERVEUR_KOBI.to_owned(), catalogue_kobi(&permis, differee));
            serveurs.push((nom, client, outils));
        }
        None => tracing::warn!("MCP de Kobi injoignable : session clé API sans ses outils"),
    }

    let mut dossiers = codebases.as_ref().clone();
    if let Some(m) = &memoire {
        dossiers.extend([m.reunions.clone(), m.conversations.clone()]);
    }
    let acces = if differee {
        Acces::LectureSeule
    } else {
        Acces::Complet
    };
    let outils = crate::outils_api::OutillageKobi::new(&dossiers, serveurs, catalogue, acces);
    let system_prompt = crate::lecture::prompt_de_session(
        kobi_brain::SYSTEM_PROMPT,
        &codebases,
        memoire.as_ref(),
        role,
        prompt_utilisateur.as_deref(),
    );
    ApiBrain::new(ApiConfig {
        cle,
        system_prompt,
        outils: Arc::new(outils),
        ..Default::default()
    })
}

/// Outils du MCP de Kobi proposés selon le rôle (noms côté serveur), et
/// ceux retirés : une question différée n'a que la lecture ; les notes ne
/// pilotent pas la réunion. Mêmes listes que les sessions Claude Code.
fn outils_kobi(role: Role) -> (Vec<String>, Vec<String>) {
    let court = |noms: Vec<String>| -> Vec<String> {
        let prefixe = format!("mcp__{SERVEUR_KOBI}__");
        noms.into_iter()
            .filter_map(|n| n.strip_prefix(&prefixe).map(str::to_owned))
            .collect()
    };
    match role {
        Role::Differee => (
            court(kobi_ctl::mcp::outils_en_lecture_seule(SERVEUR_KOBI)),
            Vec::new(),
        ),
        Role::Notes => (
            court(kobi_ctl::mcp::outils_de_l_agent(SERVEUR_KOBI)),
            court(kobi_ctl::mcp::outils_de_reunion(SERVEUR_KOBI)),
        ),
        Role::Conversation => (
            court(kobi_ctl::mcp::outils_de_l_agent(SERVEUR_KOBI)),
            Vec::new(),
        ),
    }
}

/// Entrée du catalogue pour le MCP de Kobi. Ses outils qui écrivent
/// demandent eux-mêmes le « oui » dans Kobi : ils passent sans carte du
/// backend, comme `--allowedTools` côté Claude Code. En lecture seule, seuls
/// les outils de `outils_en_lecture_seule` sont connus, en lecture.
fn catalogue_kobi(permis: &[String], lecture_seule: bool) -> kobi_perms::mcp::Serveur {
    use kobi_perms::mcp::{Nature, Politique};
    kobi_perms::mcp::Serveur {
        politique: Politique::SansDemander,
        outils: permis
            .iter()
            .map(|o| {
                let nature = if lecture_seule {
                    Nature::Lecture
                } else {
                    Nature::Ecriture
                };
                (o.clone(), nature)
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `auto` : Claude Code s'il est prêt, sinon la clé ; `api` : la clé
    /// seulement.
    #[test]
    fn choix_du_backend() {
        use Choix::*;
        for (reglage, pret, cle, attendu) in [
            (Brain::Auto, true, true, ClaudeCode),
            (Brain::Auto, true, false, ClaudeCode),
            (Brain::Auto, false, true, Api),
            (Brain::Auto, false, false, ClaudeCode),
            (Brain::ClaudeCode, false, true, ClaudeCode),
            (Brain::Api, true, true, Api),
            (Brain::Api, true, false, SansCle),
        ] {
            assert_eq!(
                choisir(reglage, pret, cle),
                attendu,
                "{reglage:?} {pret} {cle}"
            );
        }
    }

    fn courts(noms: Vec<String>) -> Vec<String> {
        let prefixe = format!("mcp__{SERVEUR_KOBI}__");
        noms.iter()
            .map(|n| n.strip_prefix(&prefixe).unwrap().to_owned())
            .collect()
    }

    /// Mêmes droits que les sessions Claude Code : la question différée
    /// lit seulement, les notes ne pilotent pas la réunion.
    #[test]
    fn outils_de_kobi_selon_le_role() {
        let (conv, retires) = outils_kobi(Role::Conversation);
        assert!(retires.is_empty());
        let reunion = courts(kobi_ctl::mcp::outils_de_reunion(SERVEUR_KOBI));
        assert!(!reunion.is_empty());
        assert!(reunion.iter().all(|o| conv.contains(o)));
        let (_, retires) = outils_kobi(Role::Notes);
        assert_eq!(retires, reunion);
        let (lecture, _) = outils_kobi(Role::Differee);
        assert_eq!(
            lecture,
            courts(kobi_ctl::mcp::outils_en_lecture_seule(SERVEUR_KOBI))
        );
        assert!(!lecture.is_empty());
        assert!(lecture.iter().all(|o| !reunion.contains(o)));
    }

    /// Catalogue du MCP de Kobi : tout passe sans carte du backend en accès
    /// complet (Kobi demande lui-même le « oui ») ; en lecture seule, seules
    /// les lectures sont connues.
    #[test]
    fn catalogue_du_mcp_de_kobi() {
        use kobi_perms::mcp::{Nature, Politique, Verdict};
        let (agent, _) = outils_kobi(Role::Conversation);
        let mut c = Catalogue::default();
        c.serveurs
            .insert(SERVEUR_KOBI.into(), catalogue_kobi(&agent, false));
        assert!(agent
            .iter()
            .all(|o| c.verdict(SERVEUR_KOBI, o) == Verdict::Libre));
        let (lecture, _) = outils_kobi(Role::Differee);
        let k = catalogue_kobi(&lecture, true);
        assert_eq!(k.politique, Politique::SansDemander);
        assert!(k.outils.values().all(|n| *n == Nature::Lecture));
        assert_eq!(k.outils.len(), lecture.len());
    }
}
