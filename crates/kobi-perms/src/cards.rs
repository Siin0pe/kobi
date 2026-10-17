//! Côté interface : chaque demande de permission devient une carte
//! « Créer / Pas maintenant » identifiée par un numéro. L'interface affiche
//! les cartes ([`Card`], sérialisable pour les événements Tauri) et répond
//! par numéro ; l'appel de Claude Code reste bloqué jusque-là.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use crate::{ApprovalRequest, Decision};

/// Carte envoyée à l'interface.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Card {
    pub id: u64,
    /// Nom complet de l'outil (`mcp__linear__save_issue`, `Write`…).
    pub tool_name: String,
    /// Serveur MCP (`linear`), absent pour un outil interne de Claude Code.
    pub server: Option<String>,
    /// Outil sans le préfixe MCP (`save_issue`).
    pub tool: String,
    /// Arguments de l'appel, à résumer sur la carte.
    pub input: Value,
    /// Délai avant refus automatique, pour le compte à rebours.
    pub timeout_ms: u64,
}

/// Cartes en attente de réponse.
pub struct Cards {
    next_id: AtomicU64,
    timeout: Duration,
    pending: Mutex<BTreeMap<u64, ApprovalRequest>>,
}

impl Cards {
    /// `timeout` : celui du serveur de permission, affiché sur les cartes.
    pub fn new(timeout: Duration) -> Self {
        Self {
            next_id: AtomicU64::new(1),
            timeout,
            pending: Mutex::new(BTreeMap::new()),
        }
    }

    /// Garde la demande en attente et renvoie la carte à afficher.
    pub fn open(&self, request: ApprovalRequest) -> Card {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let card = self.card(id, &request);
        self.lock().insert(id, request);
        card
    }

    /// Réponse de l'utilisateur. `false` si la carte n'existe plus (déjà
    /// répondue, ou expirée côté serveur : Claude a alors reçu un refus).
    pub fn answer(&self, id: u64, decision: Decision) -> bool {
        let Some(request) = self.lock().remove(&id) else {
            return false;
        };
        if request.is_abandoned() {
            return false;
        }
        request.decide(decision);
        true
    }

    /// Cartes encore en attente ; celles qui ont expiré sont retirées.
    pub fn pending(&self) -> Vec<Card> {
        let mut pending = self.lock();
        pending.retain(|_, request| !request.is_abandoned());
        pending.iter().map(|(&id, r)| self.card(id, r)).collect()
    }

    /// Refuse toutes les cartes (fin de réunion, fermeture de Kobi).
    pub fn deny_all(&self, message: &str) {
        for (_, request) in std::mem::take(&mut *self.lock()) {
            request.deny(message);
        }
    }

    fn card(&self, id: u64, request: &ApprovalRequest) -> Card {
        let name = &request.request.tool_name;
        let (server, tool) = match name
            .strip_prefix("mcp__")
            .and_then(|rest| rest.split_once("__"))
        {
            Some((server, tool)) => (Some(server.to_owned()), tool.to_owned()),
            None => (None, name.clone()),
        };
        Card {
            id,
            tool_name: name.clone(),
            server,
            tool,
            input: request.request.input.clone(),
            timeout_ms: self.timeout.as_millis() as u64,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<u64, ApprovalRequest>> {
        self.pending.lock().expect("verrou des cartes")
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::PermissionRequest;

    fn request(tool: &str) -> PermissionRequest {
        PermissionRequest {
            tool_name: tool.into(),
            input: json!({ "title": "Devis" }),
            tool_use_id: None,
        }
    }

    #[tokio::test]
    async fn carte_puis_reponse_par_numero() {
        let cards = Cards::new(Duration::from_secs(60));
        let (pending, answer) = ApprovalRequest::new(request("mcp__linear__save_issue"));
        let card = cards.open(pending);
        assert_eq!(
            serde_json::to_value(&card).unwrap(),
            json!({ "id": 1, "tool_name": "mcp__linear__save_issue", "server": "linear",
                    "tool": "save_issue", "input": { "title": "Devis" }, "timeout_ms": 60000 })
        );
        assert_eq!(cards.pending(), std::slice::from_ref(&card));
        assert!(cards.answer(card.id, Decision::Allow));
        assert_eq!(answer.await.unwrap(), Decision::Allow);
        assert!(cards.pending().is_empty());
        assert!(!cards.answer(card.id, Decision::Allow), "déjà répondue");
    }

    #[test]
    fn outil_interne_sans_serveur() {
        let cards = Cards::new(Duration::from_secs(60));
        let (pending, _answer) = ApprovalRequest::new(request("Write"));
        let card = cards.open(pending);
        assert_eq!((card.server, card.tool.as_str()), (None, "Write"));
    }

    #[test]
    fn carte_expiree_retiree() {
        let cards = Cards::new(Duration::from_secs(60));
        let (pending, answer) = ApprovalRequest::new(request("Write"));
        let card = cards.open(pending);
        // Le serveur a abandonné l'appel (délai dépassé).
        drop(answer);
        assert!(cards.pending().is_empty());
        assert!(!cards.answer(card.id, Decision::Allow));
    }

    #[tokio::test]
    async fn tout_refuser_en_fin_de_reunion() {
        let cards = Cards::new(Duration::from_secs(60));
        let (a, answer_a) = ApprovalRequest::new(request("Write"));
        let (b, answer_b) = ApprovalRequest::new(request("Edit"));
        cards.open(a);
        cards.open(b);
        cards.deny_all("Réunion terminée.");
        for answer in [answer_a, answer_b] {
            assert_eq!(
                answer.await.unwrap(),
                Decision::Deny("Réunion terminée.".into())
            );
        }
        assert!(cards.pending().is_empty());
    }
}
