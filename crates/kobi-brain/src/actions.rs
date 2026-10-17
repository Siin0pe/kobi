//! Journal des actions d'une réunion (`actions.json`) : chaque écriture
//! proposée par Claude, puis faite (avec son lien), refusée ou échouée.
//! Il alimente le compte rendu (actions non faites) et les reprises après
//! une erreur.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::brain::ReportRequest;

/// État d'une action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Carte affichée, pas encore de réponse.
    Proposed,
    /// Autorisée et exécutée.
    Done,
    /// Refusée par l'utilisateur, ou sans réponse dans le délai.
    Refused,
    /// Autorisée mais l'outil a échoué.
    Failed,
}

/// Une action du journal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Action {
    pub id: u64,
    /// Instant de la proposition, en secondes depuis le début de la réunion.
    pub t: f64,
    /// Outil (`mcp__linear__save_issue`).
    pub tool: String,
    /// Arguments de l'appel.
    pub input: Value,
    pub status: Status,
    /// Lien vers ce qui a été créé (ticket, page), si l'outil en renvoie un.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    /// Raison du refus ou de l'échec.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct File {
    version: u32,
    actions: Vec<Action>,
}

const VERSION: u32 = 1;

/// Journal attaché à un fichier ; chaque changement est enregistré tout
/// de suite (écriture dans un fichier temporaire puis renommage, pour ne
/// jamais laisser un `actions.json` à moitié écrit).
#[derive(Debug)]
pub struct ActionLog {
    path: PathBuf,
    actions: Vec<Action>,
}

impl ActionLog {
    /// Ouvre le journal, ou en crée un vide si le fichier n'existe pas.
    pub fn open(path: impl Into<PathBuf>) -> io::Result<Self> {
        let path = path.into();
        let actions = match std::fs::read_to_string(&path) {
            Ok(text) => {
                let file: File = serde_json::from_str(&text)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                file.actions
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e),
        };
        Ok(Self { path, actions })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn actions(&self) -> &[Action] {
        &self.actions
    }

    /// Nouvelle action proposée (carte affichée) ; renvoie son numéro.
    pub fn propose(&mut self, t: f64, tool: &str, input: Value) -> io::Result<u64> {
        let id = self.actions.last().map_or(1, |a| a.id + 1);
        self.actions.push(Action {
            id,
            t,
            tool: tool.to_owned(),
            input,
            status: Status::Proposed,
            link: None,
            reason: None,
        });
        self.save()?;
        Ok(id)
    }

    /// Action faite ; le lien est cherché dans le résultat de l'outil.
    pub fn done(&mut self, id: u64, tool_result: &str) -> io::Result<()> {
        self.update(id, Status::Done, None, first_url(tool_result))
    }

    pub fn refused(&mut self, id: u64, reason: &str) -> io::Result<()> {
        self.update(id, Status::Refused, Some(reason), None)
    }

    pub fn failed(&mut self, id: u64, reason: &str) -> io::Result<()> {
        self.update(id, Status::Failed, Some(reason), None)
    }

    /// Actions à reprendre ou à signaler dans le compte rendu : proposées
    /// sans réponse et échouées. Les refus sont un choix de l'utilisateur.
    pub fn pending(&self) -> Vec<&Action> {
        self.actions
            .iter()
            .filter(|a| matches!(a.status, Status::Proposed | Status::Failed))
            .collect()
    }

    /// Lignes pour [`ReportRequest::pending_actions`].
    pub fn pending_lines(&self) -> Vec<String> {
        self.pending()
            .into_iter()
            .map(|a| {
                let what = match a.status {
                    Status::Failed => "échouée",
                    _ => "sans réponse",
                };
                let reason = a
                    .reason
                    .as_deref()
                    .map(|r| format!(" : {r}"))
                    .unwrap_or_default();
                format!("{} {} ({what}{reason})", a.tool, a.input)
            })
            .collect()
    }

    /// Complète une demande de compte rendu avec les actions non faites.
    pub fn fill_report(&self, report: &mut ReportRequest) {
        report.pending_actions.extend(self.pending_lines());
    }

    fn update(
        &mut self,
        id: u64,
        status: Status,
        reason: Option<&str>,
        link: Option<String>,
    ) -> io::Result<()> {
        let action = self
            .actions
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, format!("action {id} inconnue"))
            })?;
        action.status = status;
        action.reason = reason.map(String::from);
        if link.is_some() {
            action.link = link;
        }
        self.save()
    }

    fn save(&self) -> io::Result<()> {
        let text = serde_json::to_string_pretty(&File {
            version: VERSION,
            actions: self.actions.clone(),
        })
        .map_err(io::Error::other)?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &self.path)
    }
}

/// Premier lien `http(s)://` d'un texte, sans la ponctuation qui le suit.
fn first_url(text: &str) -> Option<String> {
    let start = text.find("https://").or_else(|| text.find("http://"))?;
    let url: String = text[start..]
        .chars()
        .take_while(|c| !c.is_whitespace() && !matches!(c, '"' | '\'' | '<' | '>' | ')' | ']'))
        .collect();
    Some(url.trim_end_matches(['.', ',', ';', ':']).to_owned())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kobi-actions-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("actions.json")
    }

    #[test]
    fn ecriture_puis_relecture() {
        let path = temp("relecture");
        let mut log = ActionLog::open(&path).unwrap();
        assert!(log.actions().is_empty());
        let a = log
            .propose(
                61.5,
                "mcp__linear__save_issue",
                json!({ "title": "Corriger la connexion" }),
            )
            .unwrap();
        let b = log
            .propose(90.0, "mcp__notes__write_note", json!({ "text": "devis" }))
            .unwrap();
        let c = log
            .propose(120.0, "mcp__colanode__create_page", json!({}))
            .unwrap();
        let d = log
            .propose(130.0, "mcp__linear__save_issue", json!({ "title": "Doc" }))
            .unwrap();
        log.done(
            a,
            "Ticket créé : https://linear.app/siin0pe/issue/KOB-200/corriger.",
        )
        .unwrap();
        log.refused(b, "Pas maintenant.").unwrap();
        log.failed(c, "MCP colanode : délai dépassé").unwrap();

        let reread = ActionLog::open(&path).unwrap();
        assert_eq!(reread.actions(), log.actions());
        let by_id = |id| reread.actions().iter().find(|x| x.id == id).unwrap();
        assert_eq!(by_id(a).status, Status::Done);
        assert_eq!(
            by_id(a).link.as_deref(),
            Some("https://linear.app/siin0pe/issue/KOB-200/corriger")
        );
        assert_eq!(by_id(b).status, Status::Refused);
        assert_eq!(by_id(b).reason.as_deref(), Some("Pas maintenant."));
        assert_eq!(by_id(c).status, Status::Failed);
        assert_eq!(by_id(d).status, Status::Proposed);
        let pending: Vec<_> = reread.pending().iter().map(|x| x.id).collect();
        assert_eq!(pending, [c, d]);
        // Numérotation reprise après relecture.
        let mut reread = reread;
        assert_eq!(reread.propose(140.0, "x", json!({})).unwrap(), d + 1);
    }

    #[test]
    fn format_du_fichier() {
        let path = temp("format");
        let mut log = ActionLog::open(&path).unwrap();
        let id = log
            .propose(1.0, "mcp__linear__save_issue", json!({ "title": "A" }))
            .unwrap();
        log.done(id, "pas de lien").unwrap();
        let file: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            file,
            json!({ "version": 1, "actions": [{ "id": 1, "t": 1.0,
                "tool": "mcp__linear__save_issue", "input": { "title": "A" }, "status": "done" }] })
        );
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn actions_non_faites_dans_le_compte_rendu() {
        let path = temp("compte-rendu");
        let mut log = ActionLog::open(&path).unwrap();
        let a = log
            .propose(1.0, "mcp__linear__save_issue", json!({ "title": "A" }))
            .unwrap();
        log.failed(a, "403").unwrap();
        let b = log
            .propose(2.0, "mcp__notes__write_note", json!({ "text": "B" }))
            .unwrap();
        log.refused(b, "Pas maintenant.").unwrap();
        let mut report = ReportRequest::default();
        log.fill_report(&mut report);
        assert_eq!(
            report.pending_actions,
            [r#"mcp__linear__save_issue {"title":"A"} (échouée : 403)"#]
        );
    }

    #[test]
    fn fichier_illisible_et_action_inconnue() {
        let path = temp("illisible");
        std::fs::write(&path, "{ pas du json").unwrap();
        assert_eq!(
            ActionLog::open(&path).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        let mut log = ActionLog::open(temp("inconnue")).unwrap();
        assert_eq!(
            log.done(42, "").unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn lien_dans_un_resultat() {
        assert_eq!(
            first_url("voir (https://a.b/c)."),
            Some("https://a.b/c".into())
        );
        assert_eq!(
            first_url(r#"{"url":"http://x.y/1"}"#),
            Some("http://x.y/1".into())
        );
        assert_eq!(first_url("rien"), None);
    }
}
