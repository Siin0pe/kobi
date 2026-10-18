//! Messages échangés avec l'app : une ligne JSON par requête, une ligne JSON
//! par réponse, une requête par connexion.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Version du protocole.
pub const VERSION: u32 = 1;

/// Taille maximale d'une ligne (requête ou réponse).
pub const LIGNE_MAX: usize = 16 * 1024 * 1024;

/// Qui appelle : décide de la validation (KOB-156).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Origine {
    /// L'utilisateur, dans son terminal.
    Cli,
    /// `kobi mcp`, branché par l'utilisateur dans un autre agent.
    Mcp,
    /// L'agent de Kobi lui-même (`kobi mcp --agent`).
    Agent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Requete {
    pub v: u32,
    pub token: String,
    pub command: String,
    #[serde(default)]
    pub params: Value,
    pub origin: Origine,
}

/// Code d'erreur stable (contrat KOB-161).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    InvalidParams,
    InvalidValue,
    NotFound,
    Conflict,
    UnknownCommand,
    Unavailable,
    NeedsValidation,
    Internal,
    /// Côté client : Kobi ne tourne pas, ou le point d'accès est injoignable.
    NotRunning,
    Unauthorized,
}

impl Code {
    /// Code de retour de la CLI.
    pub fn sortie(self) -> i32 {
        match self {
            Code::NotRunning => 3,
            Code::Unauthorized => 4,
            _ => 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Erreur {
    pub code: Code,
    pub message: String,
}

impl Erreur {
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for Erreur {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Erreur {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reponse {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<Erreur>,
}

impl Reponse {
    pub fn de(resultat: Result<Value, Erreur>) -> Self {
        match resultat {
            Ok(v) => Self {
                ok: true,
                result: Some(v),
                error: None,
            },
            Err(e) => Self {
                ok: false,
                result: None,
                error: Some(e),
            },
        }
    }

    pub fn resultat(self) -> Result<Value, Erreur> {
        match (self.ok, self.error) {
            (true, _) => Ok(self.result.unwrap_or(Value::Null)),
            (false, Some(e)) => Err(e),
            (false, None) => Err(Erreur::new(
                Code::Internal,
                "réponse sans erreur ni résultat",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn format_du_contrat() {
        let r: Requete = serde_json::from_value(json!({
            "v": 1, "token": "ab", "command": "meeting.start", "origin": "agent"
        }))
        .unwrap();
        assert_eq!(r.origin, Origine::Agent);
        assert_eq!(r.params, Value::Null);
        let ok = serde_json::to_value(Reponse::de(Ok(json!({"a": 1})))).unwrap();
        assert_eq!(ok, json!({"ok": true, "result": {"a": 1}}));
        let ko = serde_json::to_value(Reponse::de(Err(Erreur::new(Code::NeedsValidation, "non"))))
            .unwrap();
        assert_eq!(
            ko,
            json!({"ok": false, "error": {"code": "needs_validation", "message": "non"}})
        );
    }

    #[test]
    fn codes_de_retour() {
        assert_eq!(Code::NotFound.sortie(), 1);
        assert_eq!(Code::NotRunning.sortie(), 3);
        assert_eq!(Code::Unauthorized.sortie(), 4);
    }
}
