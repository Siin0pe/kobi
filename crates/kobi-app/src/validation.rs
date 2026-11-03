//! Carte de validation (KOB-62) : une seule carte pour tout ce qui écrit,
//! quel que soit le genre (fichier, commande, MCP, réglage, suppression).
//!
//! Contrat commun des permissions, section 4 : la coque publie l'événement
//! `carte` avec `type: "validation"` ; l'interface (`ui/src/validation.ts`)
//! affiche le résumé, le détail dépliable et le compte à rebours, et répond
//! par `decider_carte { id, accepte }`. Sans réponse, le serveur de
//! permission (`kobi-perms`) refuse seul à l'échéance.
//!
//! Sobriété (règles du 18/10, 19:05) : à côté de Kobi, la carte reste courte
//! (résumé, cible sur une ligne, compte à rebours, Valider / Refuser) ; le
//! détail complet (diff, commande, paramètres) se lit dans l'app Kobi,
//! section Activité, qui liste les cartes en attente ([`cartes_validation`]).
//!
//! Côté cerveau (KOB-156) : [`publier`] à l'ouverture de la demande,
//! [`conclure`] quand elle est tranchée ailleurs (voix, échéance) ; un clic
//! passe par `decider_carte`, qui appelle [`retirer`].
//!
//! Actions de Kobi lui-même (KOB-156 : son MCP, et le hook de ses commandes
//! et fichiers) : [`demander`] publie la carte, la dit à voix haute (« Je le
//! fais ? ») et attend la réponse, au clic ou à la voix ([`decider`]) ; sans
//! réponse dans le délai, c'est non. L'appelant n'agit qu'après un oui.
//!
//! Le genre vient du classement de `kobi-perms` (KOB-154) ;
//! [`Genre::depuis_outil`] n'est qu'un repli pour les appels pas encore
//! classés. Ce module ne fait que la présentation : résumé en une phrase et
//! détail (diff, commande, paramètres MCP, réglage avant / après).

#![cfg_attr(
    not(test),
    allow(dead_code, reason = "cartes publiées par le cerveau avec KOB-156")
)]

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use kobi_brain::Decision;
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, Runtime};

/// Nom de l'événement Tauri (le même que les cartes de confirmation).
pub const EVENEMENT: &str = "carte";

/// Lignes de diff au plus sur la carte : le reste est résumé.
const LIGNES_DIFF: usize = 80;

/// Ce que l'action modifie.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Genre {
    Fichier,
    Commande,
    Mcp,
    Reglage,
    Suppression,
}

impl Genre {
    /// Repli pour un appel que le classement n'a pas encore vu : d'après le
    /// nom de l'outil de Claude Code et ses arguments.
    pub fn depuis_outil(outil: &str, entree: &Value) -> Self {
        match outil {
            "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => Genre::Fichier,
            "Bash" | "PowerShell" => {
                let ligne = texte(entree, "command");
                if supprime(ligne) {
                    Genre::Suppression
                } else {
                    Genre::Commande
                }
            }
            o if o.starts_with("mcp__kobi__") => Genre::Reglage,
            _ => Genre::Mcp,
        }
    }
}

/// Une commande qui efface : `rm`, `del`, `Remove-Item`, `find -delete`…
fn supprime(ligne: &str) -> bool {
    ligne
        .split(['|', ';', '&', '\n'])
        .map(str::split_whitespace)
        .any(|mut mots| {
            let premier = mots.next().unwrap_or("");
            let reste: Vec<&str> = mots.collect();
            matches!(
                premier.to_ascii_lowercase().as_str(),
                "rm" | "rmdir" | "del" | "erase" | "rd" | "remove-item" | "ri" | "unlink" | "shred"
            ) || (premier == "find" && reste.contains(&"-delete"))
                || (premier == "git" && reste.first() == Some(&"rm"))
        })
}

/// Détail affiché quand on déplie la carte.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Detail {
    Diff {
        diff: String,
    },
    Commande {
        commande: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        dossier: Option<String>,
    },
    Mcp {
        serveur: String,
        outil: String,
        params: Value,
    },
    Reglage {
        cle: String,
        avant: Value,
        apres: Value,
    },
}

/// Carte publiée sur l'événement `carte`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CarteValidation {
    pub id: String,
    #[serde(rename = "type")]
    pub type_: &'static str,
    pub genre: Genre,
    /// Une phrase : ce que Kobi va faire.
    pub resume: String,
    pub detail: Detail,
    /// Délai avant refus, pour le compte à rebours.
    pub echeance_ms: u64,
    /// Absent tant que la carte attend ; posé quand elle est tranchée.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub statut: Option<Statut>,
}

/// Issue d'une carte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Statut {
    Acceptee,
    Refusee,
    Expiree,
}

impl CarteValidation {
    /// Carte d'un appel d'outil de Claude Code (`Write`, `Bash`,
    /// `mcp__linear__save_issue`…) en attente de décision.
    pub fn depuis_appel(
        id: u64,
        genre: Genre,
        outil: &str,
        entree: &Value,
        echeance: Duration,
    ) -> Self {
        let (resume, detail) = presenter(genre, outil, entree);
        Self::nouvelle(id.to_string(), genre, resume, detail, echeance)
    }

    /// Carte d'un réglage de Kobi : l'appelant connaît la valeur actuelle.
    pub fn reglage(id: u64, cle: &str, avant: Value, apres: Value, echeance: Duration) -> Self {
        let resume = kobi_i18n::tr(
            "coque.validation.changerReglage",
            &[
                ("cle", cle),
                ("avant", &court(&avant)),
                ("apres", &court(&apres)),
            ],
        );
        let detail = Detail::Reglage {
            cle: cle.to_owned(),
            avant,
            apres,
        };
        Self::nouvelle(id.to_string(), Genre::Reglage, resume, detail, echeance)
    }

    fn nouvelle(
        id: String,
        genre: Genre,
        resume: String,
        detail: Detail,
        echeance: Duration,
    ) -> Self {
        Self {
            id,
            type_: "validation",
            genre,
            resume,
            detail,
            echeance_ms: echeance.as_millis() as u64,
            statut: None,
        }
    }
}

/// Cartes en attente, avec leur heure d'ouverture (l'échéance en découle).
#[derive(Default)]
pub struct EnAttente(Mutex<BTreeMap<String, (CarteValidation, Instant)>>);

fn en_attente<R: Runtime>(app: &AppHandle<R>) -> tauri::State<'_, EnAttente> {
    if app.try_state::<EnAttente>().is_none() {
        app.manage(EnAttente::default());
    }
    app.state::<EnAttente>()
}

/// Ouvre une carte : gardée pour l'app Kobi, publiée à toutes les fenêtres.
pub fn publier<R: Runtime>(app: &AppHandle<R>, carte: CarteValidation) {
    tracing::info!(id = %carte.id, genre = ?carte.genre, "carte de validation");
    let _ = app.emit(EVENEMENT, &carte);
    en_attente(app)
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(carte.id.clone(), (carte, Instant::now()));
}

/// Carte tranchée ailleurs (voix, échéance du serveur de permission) : elle
/// quitte la liste et toutes les fenêtres l'apprennent.
pub fn conclure<R: Runtime>(app: &AppHandle<R>, id: &str, statut: Statut) {
    let carte = en_attente(app)
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(id);
    if let Some((mut carte, _)) = carte {
        tracing::info!(%id, ?statut, "carte de validation tranchée");
        carte.statut = Some(statut);
        let _ = app.emit(EVENEMENT, &carte);
    }
}

/// Clic sur Valider / Refuser (`decider_carte`) : même effet que [`conclure`].
pub fn retirer<R: Runtime>(app: &AppHandle<R>, id: &str, accepte: bool) {
    let statut = if accepte {
        Statut::Acceptee
    } else {
        Statut::Refusee
    };
    conclure(app, id, statut);
}

/// Cartes encore en attente, les plus anciennes d'abord (section Activité de
/// l'app Kobi) ; celles dont l'échéance est passée sont retirées.
#[tauri::command]
pub fn cartes_validation<R: Runtime>(app: AppHandle<R>) -> Vec<CarteValidation> {
    let maintenant = Instant::now();
    let mut expirees = Vec::new();
    let mut cartes: Vec<(CarteValidation, Instant)> = {
        let liste = en_attente(&app);
        let liste = liste.0.lock().unwrap_or_else(|e| e.into_inner());
        liste
            .values()
            .filter(|(c, debut)| {
                let vivante =
                    maintenant.duration_since(*debut) < Duration::from_millis(c.echeance_ms);
                if !vivante {
                    expirees.push(c.id.clone());
                }
                vivante
            })
            .cloned()
            .collect()
    };
    for id in expirees {
        conclure(&app, &id, Statut::Expiree);
    }
    cartes.sort_by_key(|(_, debut)| *debut);
    cartes.into_iter().map(|(c, _)| c).collect()
}

/// Essai réel sous Xvfb (`KOBI_ESSAI_VALIDATION=1`) : trois cartes d'exemple
/// publiées 8 s après le lancement (fichier, commande, MCP), comme le fera le
/// cerveau avec KOB-156. Sans la variable, rien.
pub fn essai<R: Runtime>(app: &AppHandle<R>) {
    if std::env::var_os("KOBI_ESSAI_VALIDATION").is_none() {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(8));
        let appels = [
            (
                "Edit",
                serde_json::json!({
                    "file_path": "crates/kobi-app/src/chef.rs",
                    "old_string": "intervalle: 30",
                    "new_string": "intervalle: 20"
                }),
            ),
            (
                "Bash",
                serde_json::json!({ "command": "git push -u origin feat/kob-62", "description": "Pousser la branche sur GitHub" }),
            ),
            (
                "mcp__linear__save_issue",
                serde_json::json!({ "title": "Devis client", "team": "KOB" }),
            ),
        ];
        for (i, (outil, entree)) in appels.into_iter().enumerate() {
            let genre = Genre::depuis_outil(outil, &entree);
            let carte = CarteValidation::depuis_appel(
                900 + i as u64,
                genre,
                outil,
                &entree,
                Duration::from_secs(60),
            );
            publier(&app, carte);
        }
    });
}

/// Réponse de `decider_carte` pour le serveur de permission.
pub fn decision(accepte: bool) -> Decision {
    if accepte {
        Decision::Allow
    } else {
        Decision::Deny("Refusé par l'utilisateur.".into())
    }
}

fn texte<'a>(entree: &'a Value, cle: &str) -> &'a str {
    entree.get(cle).and_then(Value::as_str).unwrap_or("")
}

/// Nom de fichier seul, pour le résumé (le chemin complet est dans le diff).
fn nom(chemin: &str) -> &str {
    chemin.rsplit(['/', '\\']).next().unwrap_or(chemin)
}

/// Valeur JSON en une courte chaîne.
fn court(v: &Value) -> String {
    let s = v.as_str().map_or_else(|| v.to_string(), String::from);
    if s.chars().count() > 40 {
        format!("{}…", s.chars().take(39).collect::<String>())
    } else {
        s
    }
}

fn presenter(genre: Genre, outil: &str, entree: &Value) -> (String, Detail) {
    match outil {
        "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => {
            let chemin = [texte(entree, "file_path"), texte(entree, "notebook_path")]
                .into_iter()
                .find(|c| !c.is_empty())
                .unwrap_or("");
            let resume = match outil {
                "Write" => kobi_i18n::tr(
                    "coque.validation.ecrireFichier",
                    &[("fichier", nom(chemin))],
                ),
                _ => kobi_i18n::tr(
                    "coque.validation.modifierFichier",
                    &[("fichier", nom(chemin))],
                ),
            };
            (
                resume,
                Detail::Diff {
                    diff: diff(outil, chemin, entree),
                },
            )
        }
        "Bash" | "PowerShell" => {
            let commande = texte(entree, "command").to_owned();
            let description = texte(entree, "description");
            let resume = match (genre, description.is_empty()) {
                (Genre::Suppression, _) => kobi_i18n::tr(
                    "coque.validation.supprimer",
                    &[("commande", &court(&Value::from(commande.as_str())))],
                ),
                (_, false) => description.to_owned(),
                (_, true) => kobi_i18n::tr(
                    "coque.validation.lancer",
                    &[("commande", &court(&Value::from(commande.as_str())))],
                ),
            };
            let dossier = entree.get("cwd").and_then(Value::as_str).map(String::from);
            (resume, Detail::Commande { commande, dossier })
        }
        _ => {
            let mut morceaux = outil.trim_start_matches("mcp__").splitn(2, "__");
            let serveur = morceaux.next().unwrap_or(outil).to_owned();
            let action = morceaux.next().unwrap_or("").to_owned();
            let titre = ["title", "titre", "name", "summary", "subject"]
                .iter()
                .find_map(|cle| entree.get(cle).and_then(Value::as_str));
            let mut resume = format!("{serveur} · {}", action.replace('_', " "));
            if let Some(t) = titre {
                resume.push_str(&kobi_i18n::tr(
                    "coque.validation.titreMcp",
                    &[("titre", &court(&Value::from(t)))],
                ));
            }
            let detail = Detail::Mcp {
                serveur,
                outil: action,
                params: entree.clone(),
            };
            (resume, detail)
        }
    }
}

/// Diff lisible : `-` pour ce qui part, `+` pour ce qui arrive.
fn diff(outil: &str, chemin: &str, entree: &Value) -> String {
    let mut lignes = vec![format!("--- {chemin}"), format!("+++ {chemin}")];
    let mut remplacement = |avant: &str, apres: &str| {
        lignes.push("@@".into());
        lignes.extend(avant.lines().map(|l| format!("-{l}")));
        lignes.extend(apres.lines().map(|l| format!("+{l}")));
    };
    match outil {
        "Write" => remplacement("", texte(entree, "content")),
        "Edit" => remplacement(texte(entree, "old_string"), texte(entree, "new_string")),
        "MultiEdit" => {
            for e in entree
                .get("edits")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                remplacement(texte(e, "old_string"), texte(e, "new_string"));
            }
        }
        _ => remplacement("", texte(entree, "new_source")),
    }
    let total = lignes.len();
    if total > LIGNES_DIFF {
        lignes.truncate(LIGNES_DIFF);
        lignes.push(kobi_i18n::tr(
            "coque.validation.lignesDePlus",
            &[("n", &(total - LIGNES_DIFF).to_string())],
        ));
    }
    lignes.join("\n")
}

/// Délai de réponse à une carte de Kobi avant refus (KOB-156).
pub const ECHEANCE: Duration = Duration::from_secs(60);

/// Réponse à une carte de Kobi.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reponse {
    Accepte,
    Refuse,
    /// Pas de réponse dans le délai : refusé.
    Delai,
}

impl Reponse {
    pub fn acceptee(self) -> bool {
        self == Reponse::Accepte
    }

    /// Pour le fil d'activité (`decision`).
    pub fn decision(self) -> &'static str {
        match self {
            Reponse::Accepte => "accepte",
            Reponse::Refuse => "refuse",
            Reponse::Delai => "delai",
        }
    }

    fn statut(self) -> Statut {
        match self {
            Reponse::Accepte => Statut::Acceptee,
            Reponse::Refuse => Statut::Refusee,
            Reponse::Delai => Statut::Expiree,
        }
    }
}

/// Cartes de Kobi ouvertes : chacune attend sa réponse.
pub struct Validations {
    attente: Mutex<HashMap<String, Sender<bool>>>,
    compteur: AtomicU64,
    echeance: Duration,
}

impl Validations {
    pub fn new(echeance: Duration) -> Self {
        Self {
            attente: Mutex::default(),
            compteur: AtomicU64::new(1),
            echeance,
        }
    }
}

impl Default for Validations {
    fn default() -> Self {
        Self::new(ECHEANCE)
    }
}

/// Demande la validation d'une action de Kobi et attend la réponse
/// (bloquant : à appeler hors du fil du chef et du runtime). L'identifiant
/// et l'échéance de `carte` sont remplacés par ceux du registre.
pub fn demander<R: Runtime>(app: &AppHandle<R>, mut carte: CarteValidation) -> Reponse {
    let Some(v) = app.try_state::<Validations>() else {
        // Sans le registre (tests sans app), on ne fait rien qui écrit.
        return Reponse::Refuse;
    };
    let id = format!("v{}", v.compteur.fetch_add(1, Ordering::SeqCst));
    let (tx, rx) = mpsc::channel();
    v.attente
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id.clone(), tx);
    carte.id = id.clone();
    carte.echeance_ms = v.echeance.as_millis() as u64;
    let resume = carte.resume.clone();
    tracing::info!(%id, genre = ?carte.genre, %resume, "validation demandée");
    publier(app, carte);
    crate::chef::publier(
        app,
        crate::chef::Message::ValidationOuverte {
            id: id.clone(),
            resume,
        },
    );
    let reponse = match rx.recv_timeout(v.echeance) {
        Ok(true) => Reponse::Accepte,
        Ok(false) => Reponse::Refuse,
        Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => Reponse::Delai,
    };
    v.attente
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&id);
    tracing::info!(%id, decision = reponse.decision(), "validation tranchée");
    conclure(app, &id, reponse.statut());
    crate::chef::publier(app, crate::chef::Message::ValidationFermee { id, reponse });
    reponse
}

/// Cartes ouvertes, dans l'ordre où l'interface les montre (la plus
/// ancienne d'abord, les autres en file). La voix vise toujours la carte
/// affichée, et seule celle-ci est annoncée (KOB-218).
#[derive(Debug, Default)]
pub struct FileValidations(std::collections::VecDeque<(String, String)>);

impl FileValidations {
    /// Nouvelle carte ; son résumé si elle passe aussitôt à l'écran (à annoncer).
    pub fn ouvrir(&mut self, id: String, resume: &str) -> Option<String> {
        self.0.push_back((id, resume.to_owned()));
        (self.0.len() == 1).then(|| resume.to_owned())
    }

    /// Carte tranchée ; le résumé de la suivante si elle passe à l'écran.
    pub fn fermer(&mut self, id: &str) -> Option<String> {
        let tete = self.cible() == Some(id);
        self.0.retain(|(i, _)| i != id);
        if tete {
            self.0.front().map(|(_, r)| r.clone())
        } else {
            None
        }
    }

    /// La carte affichée, à qui « oui » / « non » répondent.
    pub fn cible(&self) -> Option<&str> {
        self.0.front().map(|(i, _)| i.as_str())
    }

    /// Résumé de la carte `id` si elle est encore celle qui est affichée.
    pub fn resume(&self, id: &str) -> Option<String> {
        self.0
            .front()
            .filter(|(i, _)| i == id)
            .map(|(_, r)| r.clone())
    }

    pub fn est_vide(&self) -> bool {
        self.0.is_empty()
    }
}

/// Une carte de Kobi attend sa réponse : le cerveau ne laisse pas tomber
/// le tour qui l'a demandée (KOB-191).
pub fn attend<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.try_state::<Validations>().is_some_and(|v| {
        !v.attente
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
    })
}

/// Réponse de l'utilisateur à la carte `id` de Kobi ; `false` si ce n'en
/// est pas une qui attend (carte du serveur de permission, ou déjà fermée).
pub fn decider<R: Runtime>(app: &AppHandle<R>, id: &str, accepte: bool) -> bool {
    let Some(v) = app.try_state::<Validations>() else {
        return false;
    };
    let tx = v
        .attente
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(id);
    match tx {
        Some(tx) => {
            let _ = tx.send(accepte);
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use kobi_perms::{Approver, Cards, PermissionRequest, DEFAULT_TIMEOUT};
    use rmcp::handler::server::wrapper::Parameters;
    use serde_json::json;
    use tokio::sync::mpsc;

    use super::*;

    fn appel(outil: &str, entree: Value) -> PermissionRequest {
        PermissionRequest {
            tool_name: outil.into(),
            input: entree,
            tool_use_id: Some("toolu_1".into()),
        }
    }

    /// Ce que fait la coque à chaque demande : carte ouverte, carte publiée.
    async fn demande(
        outil: &str,
        entree: Value,
    ) -> (Cards, CarteValidation, tokio::task::JoinHandle<String>) {
        let (tx, mut rx) = mpsc::channel(4);
        let approver = Approver::new(tx, DEFAULT_TIMEOUT);
        let requete = appel(outil, entree);
        let appel = tokio::spawn(async move { approver.approve(Parameters(requete)).await });
        let attente = rx.recv().await.expect("demande reçue");
        let cards = Cards::new(DEFAULT_TIMEOUT);
        let card = cards.open(attente);
        let genre = Genre::depuis_outil(&card.tool_name, &card.input);
        let carte = CarteValidation::depuis_appel(
            card.id,
            genre,
            &card.tool_name,
            &card.input,
            Duration::from_millis(card.timeout_ms),
        );
        (cards, carte, appel)
    }

    /// Cartes de Kobi (KOB-156) : oui, non, ou rien avant l'échéance.
    #[test]
    fn kobi_demande_et_attend_la_reponse() {
        use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};
        use tauri::{App, Listener};
        fn app(echeance: Duration) -> App<MockRuntime> {
            let app = crate::session::installer(mock_builder())
                .build(mock_context(noop_assets()))
                .unwrap();
            app.manage(Validations::new(echeance));
            app
        }
        /// Répond à la première carte en attente, depuis un autre fil.
        fn repondre(
            app: &App<MockRuntime>,
            accepte: Option<bool>,
        ) -> std::sync::mpsc::Receiver<Value> {
            let (tx, rx) = std::sync::mpsc::channel();
            let h = app.handle().clone();
            app.listen(EVENEMENT, move |e| {
                let carte: Value = serde_json::from_str(e.payload()).unwrap();
                let _ = tx.send(carte.clone());
                if let (None, Some(a)) = (carte.get("statut"), accepte) {
                    let id = carte["id"].as_str().unwrap().to_owned();
                    let h = h.clone();
                    std::thread::spawn(move || assert!(decider(&h, &id, a)));
                }
            });
            rx
        }
        let carte =
            || CarteValidation::reglage(0, "voice.assistant", json!(false), json!(true), ECHEANCE);
        let a = app(Duration::from_secs(5));
        let cartes = repondre(&a, Some(true));
        assert_eq!(demander(a.handle(), carte()), Reponse::Accepte);
        let ouverte = cartes.recv().unwrap();
        assert_eq!(ouverte["type"], "validation");
        assert_eq!(ouverte["genre"], "reglage");
        assert_eq!(ouverte["id"], "v1");
        assert_eq!(ouverte["echeance_ms"], 5000);
        assert_eq!(ouverte["detail"]["cle"], "voice.assistant");
        assert_eq!(cartes.recv().unwrap()["statut"], "acceptee");

        let b = app(Duration::from_secs(5));
        let cartes = repondre(&b, Some(false));
        assert_eq!(demander(b.handle(), carte()), Reponse::Refuse);
        cartes.recv().unwrap();
        assert_eq!(cartes.recv().unwrap()["statut"], "refusee");

        // Personne ne répond : refus à l'échéance, et la carte ne se décide plus.
        let c = app(Duration::from_millis(200));
        let cartes = repondre(&c, None);
        assert_eq!(demander(c.handle(), carte()), Reponse::Delai);
        cartes.recv().unwrap();
        assert_eq!(cartes.recv().unwrap()["statut"], "expiree");
        assert!(!decider(c.handle(), "v1", true));
    }

    fn reponse(texte: &str) -> Value {
        serde_json::from_str(texte).unwrap()
    }

    #[tokio::test(start_paused = true)]
    async fn clic_sur_valider_autorise() {
        let entree = json!({ "file_path": "/code/kobi/notes.md", "old_string": "a\nb", "new_string": "a\nc" });
        let (cards, carte, appel) = demande("Edit", entree.clone()).await;
        assert_eq!(
            serde_json::to_value(&carte).unwrap(),
            json!({
                "id": "1", "type": "validation", "genre": "fichier",
                "resume": "Modifier le fichier notes.md",
                "detail": { "diff": "--- /code/kobi/notes.md\n+++ /code/kobi/notes.md\n@@\n-a\n-b\n+a\n+c" },
                "echeance_ms": 60000
            })
        );
        // `decider_carte { id: "1", accepte: true }`.
        let id: u64 = carte.id.parse().unwrap();
        assert!(cards.answer(id, decision(true)));
        assert_eq!(
            reponse(&appel.await.unwrap()),
            json!({ "behavior": "allow", "updatedInput": entree })
        );
        assert!(cards.pending().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn clic_sur_refuser_refuse() {
        let (cards, carte, appel) = demande(
            "Bash",
            json!({ "command": "git push", "description": "Pousser la branche" }),
        )
        .await;
        assert_eq!(carte.genre, Genre::Commande);
        assert_eq!(carte.resume, "Pousser la branche");
        assert!(cards.answer(carte.id.parse().unwrap(), decision(false)));
        assert_eq!(
            reponse(&appel.await.unwrap()),
            json!({ "behavior": "deny", "message": "Refusé par l'utilisateur." })
        );
    }

    #[tokio::test(start_paused = true)]
    async fn ignoree_refusee_a_60_s() {
        let (cards, carte, mut appel) = demande(
            "mcp__linear__save_issue",
            json!({ "title": "Devis", "team": "KOB" }),
        )
        .await;
        assert_eq!(carte.genre, Genre::Mcp);
        assert_eq!(carte.echeance_ms, 60_000);
        // À 59 s, l'appel attend toujours.
        tokio::time::advance(Duration::from_secs(59)).await;
        assert!(tokio::time::timeout(Duration::ZERO, &mut appel)
            .await
            .is_err());
        assert_eq!(cards.pending().len(), 1);
        // À 60 s : refus, la carte disparaît, un clic tardif n'a plus d'effet.
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(
            reponse(&appel.await.unwrap()),
            json!({ "behavior": "deny", "message": "Pas de confirmation en 60 s : refusé." })
        );
        assert!(cards.pending().is_empty());
        assert!(!cards.answer(carte.id.parse().unwrap(), decision(true)));
    }

    /// Publiée, listée pour l'app, puis tranchée par un clic : elle quitte la
    /// liste et les fenêtres reçoivent son statut ; une carte dépassée expire.
    #[test]
    fn cartes_en_attente_pour_l_app() {
        use std::sync::{Arc, Mutex};

        use tauri::test::{mock_builder, mock_context, noop_assets};
        use tauri::Listener;

        let app = crate::session::installer(mock_builder())
            .build(mock_context(noop_assets()))
            .unwrap();
        let h = app.handle().clone();
        let recus = Arc::new(Mutex::new(Vec::<Value>::new()));
        let r = recus.clone();
        app.listen_any(EVENEMENT, move |e| {
            r.lock()
                .unwrap()
                .push(serde_json::from_str(e.payload()).unwrap());
        });
        let fichier = CarteValidation::depuis_appel(
            11,
            Genre::Fichier,
            "Write",
            &json!({ "file_path": "a.txt", "content": "x" }),
            DEFAULT_TIMEOUT,
        );
        publier(&h, fichier);
        publier(
            &h,
            CarteValidation::reglage(
                12,
                "voice.meeting",
                json!(false),
                json!(true),
                Duration::ZERO,
            ),
        );
        // La carte à échéance nulle est déjà expirée : seule la première reste.
        let ids: Vec<_> = cartes_validation(h.clone())
            .into_iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(ids, ["11"]);
        // Clic sur Valider : `decider_carte` la retire.
        retirer(&h, "11", true);
        assert!(cartes_validation(h.clone()).is_empty());
        let statuts: Vec<_> = recus
            .lock()
            .unwrap()
            .iter()
            .map(|c| {
                (
                    c["id"].as_str().unwrap().to_owned(),
                    c.get("statut").cloned(),
                )
            })
            .collect();
        assert_eq!(
            statuts,
            [
                ("11".to_owned(), None),
                ("12".to_owned(), None),
                ("12".to_owned(), Some(json!("expiree"))),
                ("11".to_owned(), Some(json!("acceptee"))),
            ]
        );
        // Une décision tardive ne republie rien.
        retirer(&h, "11", false);
        assert_eq!(recus.lock().unwrap().len(), 4);
    }

    #[test]
    fn genres() {
        let g = |outil: &str, cmd: &str| Genre::depuis_outil(outil, &json!({ "command": cmd }));
        assert_eq!(g("Write", ""), Genre::Fichier);
        assert_eq!(g("Bash", "cargo test"), Genre::Commande);
        assert_eq!(g("Bash", "cd /tmp && rm -rf build"), Genre::Suppression);
        assert_eq!(g("PowerShell", "Remove-Item x.txt"), Genre::Suppression);
        assert_eq!(g("Bash", "find . -name '*.o' -delete"), Genre::Suppression);
        assert_eq!(g("Bash", "git rm a.txt"), Genre::Suppression);
        assert_eq!(g("Bash", "echo rm"), Genre::Commande);
        assert_eq!(g("mcp__kobi__reglages_ecrire", ""), Genre::Reglage);
        assert_eq!(g("mcp__github__create_issue", ""), Genre::Mcp);
    }

    #[test]
    fn reglage_avant_apres() {
        let carte = CarteValidation::reglage(
            7,
            "voice.meeting",
            json!(false),
            json!(true),
            DEFAULT_TIMEOUT,
        );
        assert_eq!(
            serde_json::to_value(&carte).unwrap(),
            json!({
                "id": "7", "type": "validation", "genre": "reglage",
                "resume": "Changer le réglage « voice.meeting » : false → true",
                "detail": { "cle": "voice.meeting", "avant": false, "apres": true },
                "echeance_ms": 60000
            })
        );
    }

    #[test]
    fn diff_long_tronque() {
        let contenu = (0..200)
            .map(|i| format!("ligne {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let carte = CarteValidation::depuis_appel(
            2,
            Genre::Fichier,
            "Write",
            &json!({ "file_path": "C:\\code\\a.txt", "content": contenu }),
            DEFAULT_TIMEOUT,
        );
        assert_eq!(carte.resume, "Écrire le fichier a.txt");
        let Detail::Diff { diff } = &carte.detail else {
            panic!()
        };
        assert_eq!(diff.lines().count(), LIGNES_DIFF + 1);
        assert!(diff.ends_with("… 123 lignes de plus"));
    }

    /// KOB-218 : deux cartes, la voix vise celle affichée (la plus ancienne),
    /// puis la suivante quand elle passe à l'écran.
    #[test]
    fn la_voix_vise_la_carte_affichee() {
        let mut f = FileValidations::default();
        assert_eq!(
            f.ouvrir("a".into(), "Écrire notes.md"),
            Some("Écrire notes.md".into())
        );
        // B attend son tour : ni annonce ni cible.
        assert_eq!(f.ouvrir("b".into(), "Lancer : cargo test"), None);
        assert_eq!(f.cible(), Some("a"));
        // B tranchée ailleurs (échéance) pendant qu'elle attend : rien ne change.
        assert_eq!(f.ouvrir("c".into(), "Pousser la branche"), None);
        assert_eq!(f.fermer("b"), None);
        assert_eq!(f.cible(), Some("a"));
        // A tranchée : C passe à l'écran, annoncée, et devient la cible.
        assert_eq!(f.fermer("a"), Some("Pousser la branche".into()));
        assert_eq!(f.cible(), Some("c"));
        assert_eq!(f.fermer("c"), None);
        assert!(f.est_vide());
        assert_eq!(f.cible(), None);
    }
}
