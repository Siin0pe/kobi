//! Lecture du flux SSE de l'API Messages (`"stream": true`).
//!
//! Le flux est découpé en événements (`event: …` / `data: {…}` séparés par
//! une ligne vide) ; [`Reponse`] rebâtit le message complet à partir des
//! événements `message_start`, `content_block_start`, `content_block_delta`,
//! `content_block_stop` et `message_delta`. Les blocs sont gardés tels que
//! l'API les donne (réflexion et signature comprises), pour être renvoyés
//! sans changement au tour suivant.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

/// Découpe un flux d'octets en charges `data:` d'événements SSE.
#[derive(Debug, Default)]
pub struct Decoupeur {
    tampon: Vec<u8>,
}

impl Decoupeur {
    /// Ajoute un morceau reçu ; rend les données des événements complets.
    pub fn pousser(&mut self, morceau: &[u8]) -> Vec<String> {
        self.tampon.extend_from_slice(morceau);
        let mut donnees = Vec::new();
        while let Some((fin, saut)) = fin_d_evenement(&self.tampon) {
            let brut: Vec<u8> = self.tampon.drain(..fin + saut).collect();
            let texte = String::from_utf8_lossy(&brut[..fin]);
            let data: Vec<&str> = texte
                .lines()
                .filter_map(|l| l.strip_prefix("data:"))
                .map(|d| d.strip_prefix(' ').unwrap_or(d))
                .collect();
            if !data.is_empty() {
                donnees.push(data.join("\n"));
            }
        }
        donnees
    }
}

/// Position de la première ligne vide (fin d'événement) et longueur du
/// séparateur.
fn fin_d_evenement(t: &[u8]) -> Option<(usize, usize)> {
    (0..t.len()).find_map(|i| {
        if t[i..].starts_with(b"\r\n\r\n") {
            Some((i, 4))
        } else if t[i..].starts_with(b"\n\n") {
            Some((i, 2))
        } else {
            None
        }
    })
}

/// Ce qu'un événement apporte à l'interface.
#[derive(Debug, Clone, PartialEq)]
pub enum Apport {
    /// Morceau de texte de la réponse.
    Texte(String),
    /// Réflexion ou autre signe de vie.
    Vie,
    /// Rien à montrer.
    Rien,
}

/// Message de l'assistant en cours de réception.
#[derive(Debug, Default)]
pub struct Reponse {
    blocs: BTreeMap<usize, Bloc>,
    /// Entrées d'outils illisibles, par identifiant d'appel : le texte brut
    /// n'est rendu à Claude que dans le résultat en erreur ; l'historique
    /// garde `input: {}` (l'API exige un objet, KOB-318).
    pub illisibles: BTreeMap<String, String>,
    pub stop_reason: Option<String>,
    pub usage: Map<String, Value>,
    pub fini: bool,
}

#[derive(Debug)]
struct Bloc {
    contenu: Value,
    /// Morceaux `input_json_delta` d'un appel d'outil.
    json: String,
}

/// Erreur envoyée dans le flux (`{"type":"error",…}`) ou flux illisible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErreurFlux {
    pub genre: String,
    pub message: String,
}

impl Reponse {
    /// Applique la charge `data:` d'un événement.
    pub fn appliquer(&mut self, donnee: &str) -> Result<Apport, ErreurFlux> {
        // Jamais la charge elle-même dans l'erreur : elle peut contenir la
        // réponse, des notes ou de la transcription, et l'erreur s'affiche
        // et part dans les journaux (KOB-322).
        let ev: Value = serde_json::from_str(donnee).map_err(|e| ErreurFlux {
            genre: "flux_illisible".into(),
            message: format!("{e} (événement de {} octets)", donnee.len()),
        })?;
        let index = ev["index"].as_u64().map(|i| i as usize);
        match ev["type"].as_str().unwrap_or_default() {
            "message_start" => {
                fusionner(&mut self.usage, &ev["message"]["usage"]);
                Ok(Apport::Vie)
            }
            "content_block_start" => {
                let contenu = ev["content_block"].clone();
                let apport = match contenu["type"].as_str() {
                    Some("text") => Apport::Rien,
                    _ => Apport::Vie,
                };
                self.blocs.insert(
                    index.unwrap_or(self.blocs.len()),
                    Bloc {
                        contenu,
                        json: String::new(),
                    },
                );
                Ok(apport)
            }
            "content_block_delta" => {
                let Some(bloc) = index.and_then(|i| self.blocs.get_mut(&i)) else {
                    return Ok(Apport::Rien);
                };
                let delta = &ev["delta"];
                let morceau = |cle: &str| delta[cle].as_str().unwrap_or_default().to_owned();
                match delta["type"].as_str().unwrap_or_default() {
                    "text_delta" => {
                        let t = morceau("text");
                        allonger(&mut bloc.contenu, "text", &t);
                        Ok(Apport::Texte(t))
                    }
                    "thinking_delta" => {
                        allonger(&mut bloc.contenu, "thinking", &morceau("thinking"));
                        Ok(Apport::Vie)
                    }
                    "signature_delta" => {
                        allonger(&mut bloc.contenu, "signature", &morceau("signature"));
                        Ok(Apport::Rien)
                    }
                    "input_json_delta" => {
                        bloc.json.push_str(&morceau("partial_json"));
                        Ok(Apport::Vie)
                    }
                    "citations_delta" => {
                        if let Some(o) = bloc.contenu.as_object_mut() {
                            let liste =
                                o.entry("citations").or_insert_with(|| Value::Array(vec![]));
                            if let Some(l) = liste.as_array_mut() {
                                l.push(delta["citation"].clone());
                            }
                        }
                        Ok(Apport::Rien)
                    }
                    _ => Ok(Apport::Rien),
                }
            }
            "content_block_stop" => {
                if let Some(bloc) = index.and_then(|i| self.blocs.get_mut(&i)) {
                    if !bloc.json.is_empty() {
                        match serde_json::from_str::<Value>(&bloc.json) {
                            Ok(entree) if entree.is_object() => bloc.contenu["input"] = entree,
                            // Entrée illisible (ou qui n'est pas un objet) :
                            // l'appel est refusé plus loin, et l'historique
                            // garde un objet vide.
                            _ => {
                                bloc.contenu["input"] = Value::Object(Map::new());
                                let id = bloc.contenu["id"].as_str().unwrap_or_default();
                                self.illisibles.insert(id.to_owned(), bloc.json.clone());
                            }
                        }
                    }
                }
                Ok(Apport::Rien)
            }
            "message_delta" => {
                if let Some(raison) = ev["delta"]["stop_reason"].as_str() {
                    self.stop_reason = Some(raison.to_owned());
                }
                fusionner(&mut self.usage, &ev["usage"]);
                Ok(Apport::Rien)
            }
            "message_stop" => {
                self.fini = true;
                Ok(Apport::Rien)
            }
            "error" => Err(ErreurFlux {
                genre: ev["error"]["type"].as_str().unwrap_or("error").to_owned(),
                message: ev["error"]["message"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            }),
            // `ping` et événements inconnus.
            _ => Ok(Apport::Rien),
        }
    }

    /// Blocs du message, dans l'ordre.
    pub fn blocs(&self) -> Vec<Value> {
        self.blocs.values().map(|b| b.contenu.clone()).collect()
    }
}

fn allonger(bloc: &mut Value, cle: &str, morceau: &str) {
    if let Some(o) = bloc.as_object_mut() {
        let actuel = o.entry(cle).or_insert_with(|| Value::String(String::new()));
        let mut s = actuel.as_str().unwrap_or_default().to_owned();
        s.push_str(morceau);
        *actuel = Value::String(s);
    }
}

/// Recopie les compteurs non nuls de `usage` (le `message_delta` final
/// donne les valeurs cumulées).
fn fusionner(usage: &mut Map<String, Value>, nouveau: &Value) {
    if let Some(o) = nouveau.as_object() {
        for (k, v) in o {
            if !v.is_null() {
                usage.insert(k.clone(), v.clone());
            }
        }
    }
}

/// Blocs à renvoyer dans l'historique. Après un repli sur un autre modèle
/// en cours de réponse (bloc `fallback`), seuls les blocs de texte d'avant
/// le dernier repli sont gardés : la réflexion et les appels d'outils du
/// modèle qui a refusé ne se renvoient pas (doc de l'API, « Echoing
/// fallback turns back »). Le marqueur lui-même est retiré.
pub fn a_renvoyer(blocs: Vec<Value>) -> Vec<Value> {
    let Some(limite) = blocs.iter().rposition(|b| b["type"] == "fallback") else {
        return blocs;
    };
    blocs
        .into_iter()
        .enumerate()
        .filter(|(i, b)| *i > limite || (*i < limite && b["type"] == "text"))
        .map(|(_, b)| b)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn evenements(texte: &str) -> Vec<String> {
        Decoupeur::default().pousser(texte.as_bytes())
    }

    #[test]
    fn decoupe_meme_coupe_au_milieu() {
        let flux = "event: ping\ndata: {\"type\":\"ping\"}\n\nevent: message_stop\r\ndata: {\"type\":\"message_stop\"}\r\n\r\n";
        let mut d = Decoupeur::default();
        let mut tout = Vec::new();
        for morceau in flux.as_bytes().chunks(7) {
            tout.extend(d.pousser(morceau));
        }
        assert_eq!(tout, ["{\"type\":\"ping\"}", "{\"type\":\"message_stop\"}"]);
        // Commentaire SSE sans `data:` : ignoré.
        assert!(evenements(": rien\n\n").is_empty());
    }

    #[test]
    fn message_rebati_avec_reflexion_texte_et_outil() {
        let mut r = Reponse::default();
        let donnees = [
            json!({"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":10,"cache_read_input_tokens":700,"cache_creation_input_tokens":0,"output_tokens":1}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"c2ln"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Je "}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"regarde."}}),
            json!({"type":"content_block_stop","index":1}),
            json!({"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_1","name":"lire","input":{}}}),
            json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"chemin\": \"a"}}),
            json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":".md\"}"}}),
            json!({"type":"content_block_stop","index":2}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":42}}),
            json!({"type":"message_stop"}),
        ];
        let apports: Vec<Apport> = donnees
            .iter()
            .map(|d| r.appliquer(&d.to_string()).unwrap())
            .collect();
        assert!(apports.contains(&Apport::Texte("regarde.".into())));
        assert!(r.fini);
        assert_eq!(r.stop_reason.as_deref(), Some("tool_use"));
        assert_eq!(r.usage["output_tokens"], 42);
        assert_eq!(r.usage["cache_read_input_tokens"], 700);
        assert_eq!(
            r.blocs(),
            [
                json!({"type":"thinking","thinking":"","signature":"c2ln"}),
                json!({"type":"text","text":"Je regarde."}),
                json!({"type":"tool_use","id":"toolu_1","name":"lire","input":{"chemin":"a.md"}}),
            ]
        );
    }

    #[test]
    fn erreur_dans_le_flux() {
        let mut r = Reponse::default();
        let e = r
            .appliquer(
                r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
            )
            .unwrap_err();
        assert_eq!(e.genre, "overloaded_error");
        // KOB-322 : rien de la charge dans le message.
        let e = r
            .appliquer(r#"{"type":"content_block_delta","delta":{"text":"compte rendu secret"#)
            .unwrap_err();
        assert_eq!(e.genre, "flux_illisible");
        assert!(!e.message.contains("secret"), "{}", e.message);
        assert!(e.message.contains("octets"), "{}", e.message);
    }

    #[test]
    fn entree_d_outil_illisible_mise_a_part() {
        let mut r = Reponse::default();
        for d in [
            json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t","name":"x","input":{}}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"a\": "}}),
            json!({"type":"content_block_stop","index":0}),
        ] {
            r.appliquer(&d.to_string()).unwrap();
        }
        // L'historique garde un objet vide, le texte brut est à part.
        assert_eq!(r.blocs()[0]["input"], json!({}));
        assert_eq!(r.illisibles["t"], "{\"a\": ");
    }

    #[test]
    fn repli_en_cours_de_reponse() {
        let blocs = vec![
            json!({"type":"thinking","thinking":"","signature":"s"}),
            json!({"type":"text","text":"Début"}),
            json!({"type":"tool_use","id":"t","name":"x","input":{}}),
            json!({"type":"fallback","from":{"model":"a"},"to":{"model":"b"}}),
            json!({"type":"thinking","thinking":"","signature":"s2"}),
            json!({"type":"text","text":"Suite"}),
        ];
        assert_eq!(
            a_renvoyer(blocs),
            [
                json!({"type":"text","text":"Début"}),
                json!({"type":"thinking","thinking":"","signature":"s2"}),
                json!({"type":"text","text":"Suite"}),
            ]
        );
        let sans = vec![json!({"type":"text","text":"a"})];
        assert_eq!(a_renvoyer(sans.clone()), sans);
    }
}
