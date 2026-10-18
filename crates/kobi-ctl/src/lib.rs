//! kobi-ctl : interface de contrôle de Kobi (KOB-161).
//!
//! Une seule table de commandes ([`commandes::COMMANDES`]) sert à la CLI
//! (`kobi <commande>`), au serveur MCP (`kobi mcp`) et à l'agent de Kobi.
//! Toutes parlent à l'app en cours par un point d'accès local
//! ([`transport::Point`] : canal nommé sous Windows, socket Unix ailleurs),
//! protégé par un jeton, jamais exposé sur le réseau. L'exécution des
//! commandes vit dans l'app (`kobi-app/src/controle.rs`) ; ce crate ne
//! dépend ni de Tauri ni du harness (KOB-157).
//!
//! Contrat : commentaire « Contrat de l'interface de contrôle » de KOB-161.

pub mod cli;
pub mod client;
pub mod commandes;
pub mod mcp;
pub mod protocole;
pub mod serveur;
pub mod transport;

pub use commandes::{Commande, Effet, COMMANDES};
pub use protocole::{Code, Erreur, Origine, Reponse, Requete};
pub use transport::Point;
