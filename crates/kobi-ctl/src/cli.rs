//! CLI : `kobi <commande> [arguments] [--json] [--out FICHIER]`.
//!
//! Sortie lisible par défaut ; `--json` écrit le résultat brut. Codes de
//! retour : 0 succès, 1 commande refusée ou en échec, 2 usage, 3 Kobi ne
//! tourne pas, 4 jeton refusé.

use std::io::Write;

use serde_json::{Map, Value};

use crate::commandes::{self, Commande, Genre, Param, COMMANDES};
use crate::mcp::{self, Mode};
use crate::protocole::{Code, Erreur, Origine};
use crate::transport::Point;

/// Code de retour d'une erreur d'usage.
pub const USAGE: i32 = 2;

/// Ce que demande la ligne de commande.
#[derive(Debug, Clone, PartialEq)]
pub enum Demande {
    Aide(Option<&'static Commande>),
    Version,
    Mcp(Mode),
    Commande {
        commande: &'static Commande,
        params: Value,
        json: bool,
        sortie: Option<String>,
    },
}

/// Mots qui lancent la CLI au lieu de l'app (`kobi status`). Les options de
/// l'app (`--carnet`, `--reglages`…) ne sont pas des commandes.
pub fn est_une_commande(premier: &str) -> bool {
    matches!(premier, "help" | "--help" | "-h" | "--version" | "mcp")
        || COMMANDES
            .iter()
            .any(|c| c.nom.split('.').next() == Some(premier))
}

fn usage(message: impl Into<String>) -> Erreur {
    Erreur::new(Code::InvalidParams, message)
}

fn valeur(param: &Param, texte: &str) -> Result<Value, Erreur> {
    match param.genre {
        Genre::Texte => Ok(Value::String(texte.to_owned())),
        Genre::Booleen => match texte.to_lowercase().as_str() {
            "true" | "on" | "oui" | "yes" | "1" => Ok(Value::Bool(true)),
            "false" | "off" | "non" | "no" | "0" => Ok(Value::Bool(false)),
            _ => Err(usage(format!(
                "--{} : « {texte} » n'est pas true/false (on/off)",
                param.nom
            ))),
        },
        Genre::Entier => texte
            .parse::<u64>()
            .map(Value::from)
            .map_err(|_| usage(format!("--{} : « {texte} » n'est pas un entier", param.nom))),
        // `true`, `3`, `["C:/code"]` sont lus en JSON ; un mot seul reste un texte.
        Genre::Json => {
            Ok(serde_json::from_str(texte).unwrap_or_else(|_| Value::String(texte.to_owned())))
        }
    }
}

/// Lit la ligne de commande (sans le nom du programme).
pub fn analyser(args: &[String]) -> Result<Demande, Erreur> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        [] | ["help" | "--help" | "-h"] => return Ok(Demande::Aide(None)),
        ["--version"] => return Ok(Demande::Version),
        ["help", mots @ ..] => {
            let c = trouver(mots).map(|(c, _)| c);
            return c
                .map(|c| Demande::Aide(Some(c)))
                .ok_or_else(|| usage(format!("commande inconnue : {}", mots.join(" "))));
        }
        ["mcp", reste @ ..] => {
            return match reste {
                [] => Ok(Demande::Mcp(Mode::Externe)),
                ["--agent"] => Ok(Demande::Mcp(Mode::Agent)),
                _ => Err(usage("usage : kobi mcp [--agent]")),
            }
        }
        _ => {}
    }
    let Some((commande, reste)) = trouver(&args) else {
        let groupe: Vec<String> = COMMANDES
            .iter()
            .filter(|c| c.nom.split('.').next() == Some(args[0]))
            .map(|c| format!("kobi {}", c.mots()))
            .collect();
        return Err(usage(if groupe.is_empty() {
            format!("commande inconnue : {} (voir « kobi help »)", args[0])
        } else {
            format!(
                "commande incomplète ; possibles :\n  {}",
                groupe.join("\n  ")
            )
        }));
    };
    let mut params = Map::new();
    let mut positionnels = Vec::new();
    let (mut json, mut sortie) = (false, None);
    let mut i = 0;
    while i < reste.len() {
        let arg = reste[i];
        i += 1;
        match arg {
            "--help" | "-h" => return Ok(Demande::Aide(Some(commande))),
            "--json" => json = true,
            "--out" | "-o" => {
                sortie = Some(
                    reste
                        .get(i)
                        .ok_or_else(|| usage("--out : fichier manquant"))?
                        .to_string(),
                );
                i += 1;
            }
            "--" => {
                positionnels.extend(reste[i..].iter().copied());
                break;
            }
            option if option.starts_with("--") => {
                let (nom, texte) = match option[2..].split_once('=') {
                    Some((n, t)) => (n, Some(t)),
                    None => (&option[2..], None),
                };
                let param = commande
                    .params
                    .iter()
                    .find(|p| p.nom == nom)
                    .ok_or_else(|| {
                        usage(format!("{} : option inconnue --{nom}", commande.mots()))
                    })?;
                let texte = match texte {
                    Some(t) => t,
                    // `--all` seul vaut `--all true`.
                    None if param.genre == Genre::Booleen
                        && reste.get(i).is_none_or(|s| s.starts_with("--")) =>
                    {
                        "true"
                    }
                    None => {
                        let t = reste
                            .get(i)
                            .ok_or_else(|| usage(format!("--{nom} : valeur manquante")))?;
                        i += 1;
                        t
                    }
                };
                params.insert(nom.into(), valeur(param, texte)?);
            }
            mot => positionnels.push(mot),
        }
    }
    let attendus: Vec<&Param> = commande
        .params
        .iter()
        .filter(|p| p.positionnel && !params.contains_key(p.nom))
        .collect();
    let mut mots = positionnels.into_iter();
    for (n, param) in attendus.iter().enumerate() {
        let dernier = n + 1 == attendus.len();
        // Le dernier texte prend la fin de la ligne : `kobi search point budget`.
        let texte = if dernier && param.genre == Genre::Texte {
            let fin: Vec<&str> = mots.by_ref().collect();
            (!fin.is_empty()).then(|| fin.join(" "))
        } else {
            mots.next().map(str::to_owned)
        };
        if let Some(t) = texte {
            params.insert(param.nom.into(), valeur(param, &t)?);
        }
    }
    if let Some(trop) = mots.next() {
        return Err(usage(format!(
            "{} : argument en trop « {trop} »",
            commande.mots()
        )));
    }
    let params = Value::Object(params);
    commande.verifier(&params)?;
    Ok(Demande::Commande {
        commande,
        params,
        json,
        sortie,
    })
}

/// Commande désignée par les premiers mots (le plus long nom gagne).
fn trouver<'a>(mots: &'a [&'a str]) -> Option<(&'static Commande, &'a [&'a str])> {
    (1..=mots.len().min(3)).rev().find_map(|n| {
        if mots[..n].iter().any(|m| m.starts_with('-')) {
            return None;
        }
        commandes::trouver(&mots[..n].join(".")).map(|c| (c, &mots[n..]))
    })
}

pub fn aide(commande: Option<&Commande>) -> String {
    let Some(c) = commande else {
        let mut t = String::from(
            "Kobi : pilote l'app Kobi en cours depuis le terminal.\n\n\
             Usage : kobi <commande> [arguments] [--json] [--out FICHIER]\n\
             \x20       kobi mcp            serveur MCP (stdio) pour Claude Code, Codex…\n\
             \x20       kobi help <commande>\n\nCommandes :\n",
        );
        for c in COMMANDES {
            t.push_str(&format!(
                "  {:<22} {:<11} {}\n",
                c.mots(),
                c.effet.etiquette(),
                c.description
            ));
        }
        t.push_str(
            "\nCodes de retour : 0 succès, 1 commande refusée, 2 usage, 3 Kobi ne tourne pas, 4 jeton refusé.\n",
        );
        return t;
    };
    let mut ligne = format!("kobi {}", c.mots());
    for p in c.params {
        let nom = if p.positionnel {
            format!("<{}>", p.nom)
        } else {
            format!("--{} <{}>", p.nom, p.nom)
        };
        ligne.push(' ');
        ligne.push_str(&if p.requis { nom } else { format!("[{nom}]") });
    }
    let mut t = format!(
        "{ligne} [--json] [--out FICHIER]\n\n{} {}\n",
        c.effet.etiquette(),
        c.description
    );
    for p in c.params {
        t.push_str(&format!("  {:<10} {}", p.nom, p.description));
        if !p.choix.is_empty() {
            t.push_str(&format!(" ({})", p.choix.join(", ")));
        }
        t.push('\n');
    }
    t
}

fn duree(secondes: f64) -> String {
    let s = secondes.max(0.0) as u64;
    if s >= 3600 {
        format!("{} h {:02} min", s / 3600, (s % 3600) / 60)
    } else {
        format!("{} min {:02} s", s / 60, s % 60)
    }
}

fn texte_de(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        autre => autre.to_string(),
    }
}

/// Rendu lisible d'un résultat.
pub fn lisible(commande: &Commande, r: &Value) -> String {
    if let Value::String(s) = r {
        return s.clone();
    }
    let s = |v: &Value| texte_de(v);
    let lignes = |liste: &Value, f: &dyn Fn(&Value) -> String| -> String {
        match liste.as_array() {
            Some(l) if l.is_empty() => "(aucun)".into(),
            Some(l) => l.iter().map(f).collect::<Vec<_>>().join("\n"),
            None => serde_json::to_string_pretty(liste).unwrap_or_default(),
        }
    };
    match commande.nom {
        "status" => {
            let reunion = &r["reunion"];
            let reunion = if reunion["actif"] == true {
                format!(
                    "{} depuis {}, {} notes",
                    if reunion["pause"] == true {
                        "en pause"
                    } else {
                        "enregistrée"
                    },
                    duree(r["reunion"]["duree_s"].as_f64().unwrap_or(0.0)),
                    s(&reunion["notes"])
                )
            } else {
                "aucune".into()
            };
            format!(
                "Kobi      {}{} (mascotte : {})\nRéunion   {reunion}\nÉcoute    {}\nFenêtre   {}{}",
                s(&r["etat"]),
                r["activite"]
                    .as_str()
                    .map(|a| format!(", {a}"))
                    .unwrap_or_default(),
                s(&r["mascotte"]),
                if r["ecoute"]["pause"] == true {
                    "en pause"
                } else {
                    "active"
                },
                if r["fenetre"]["visible"] == false {
                    "cachée"
                } else {
                    "visible"
                },
                if r["fenetre"]["discret"] == true {
                    ", mode discret"
                } else {
                    ""
                },
            )
        }
        "meeting.start" | "meeting.stop" | "meeting.pause" | "meeting.resume" => s(&r["message"]),
        "meeting.status" => {
            if r["actif"] == true {
                format!(
                    "Réunion {} depuis {}, {} notes{}",
                    if r["pause"] == true {
                        "en pause"
                    } else {
                        "en cours"
                    },
                    duree(r["duree_s"].as_f64().unwrap_or(0.0)),
                    s(&r["notes"]),
                    r["derniere_note"]
                        .as_str()
                        .map(|n| format!("\nDernière note : {n}"))
                        .unwrap_or_default()
                )
            } else {
                "Aucune réunion en cours.".into()
            }
        }
        "meetings.list" => lignes(r, &|m| {
            format!(
                "{}  {}  {}{}",
                s(&m["nom"]),
                duree(m["duree_s"].as_f64().unwrap_or(0.0)),
                m["titre"].as_str().unwrap_or("(sans titre)"),
                if m["en_cours"] == true {
                    "  [en cours]"
                } else {
                    ""
                }
            )
        }),
        "conversations.list" => lignes(r, &|c| {
            format!(
                "{}  {} tours  {}",
                s(&c["id"]),
                s(&c["tours"]),
                s(&c["apercu"])
            )
        }),
        "reminders.list" => lignes(r, &|x| {
            format!(
                "{} {}  {}",
                if x["fait"] == true { "[x]" } else { "[ ]" },
                s(&x["id"]),
                s(&x["texte"])
            )
        }),
        "reminders.add" => format!("Rappel ajouté : {} ({})", s(&r["texte"]), s(&r["id"])),
        "search" => lignes(r, &|t| {
            let ou = match t["type"].as_str() {
                Some("reunion") => format!("{} ({})", s(&t["nom"]), s(&t["partie"])),
                _ => format!("conversation {}", s(&t["id"])),
            };
            format!("{ou} : {}", s(&t["extrait"]))
        }),
        "config.get" if !r.is_object() => s(r),
        "config.list" => lignes(r, &|e| {
            format!(
                "{} = {}   [{}] {}",
                s(&e["key"]),
                e["value"],
                if e["apply"] == "hot" {
                    "à chaud"
                } else {
                    "au redémarrage"
                },
                s(&e["description"])
            )
        }),
        "config.set" | "config.undo" => format!(
            "{} : {} → {}{}",
            s(&r["key"]),
            r["before"],
            r["after"],
            match r["apply"].as_str() {
                Some("hot") => " (appliqué tout de suite)",
                Some(_) => " (appliqué au prochain redémarrage de Kobi)",
                None => "",
            }
        ),
        "meetings.export" => s(&r["markdown"]),
        "app.open" => format!(
            "Ouvert : {}{}",
            s(&r["section"]),
            if r["provisoire"] == true {
                format!(
                    " (dans la fenêtre {}, en attendant l'app Kobi)",
                    s(&r["fenetre"])
                )
            } else {
                String::new()
            }
        ),
        _ if r.as_object().is_some_and(Map::is_empty) || r.is_null() => "OK".into(),
        _ => serde_json::to_string_pretty(r).unwrap_or_default(),
    }
}

/// Lance la CLI ; renvoie le code de retour.
pub fn main_avec(args: &[String]) -> i32 {
    let demande = match analyser(args) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("kobi : {}", e.message);
            return USAGE;
        }
    };
    let Some(point) = Point::defaut() else {
        eprintln!("kobi : dossier personnel introuvable");
        return 3;
    };
    match demande {
        Demande::Aide(c) => {
            print!("{}", aide(c));
            0
        }
        Demande::Version => {
            println!("kobi {}", env!("CARGO_PKG_VERSION"));
            0
        }
        Demande::Mcp(mode) => match mcp::servir(point, mode) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("kobi mcp : {e}");
                1
            }
        },
        Demande::Commande {
            commande,
            params,
            json,
            sortie,
        } => executer(&point, commande, params, json, sortie.as_deref()),
    }
}

fn executer(
    point: &Point,
    commande: &Commande,
    params: Value,
    json: bool,
    sortie: Option<&str>,
) -> i32 {
    match crate::client::appeler(point, commande.nom, params, Origine::Cli) {
        Ok(r) => {
            let texte = if json {
                serde_json::to_string_pretty(&r).unwrap_or_default()
            } else {
                lisible(commande, &r)
            };
            match sortie {
                Some(fichier) => match std::fs::write(fichier, format!("{texte}\n")) {
                    Ok(()) => {
                        eprintln!("écrit : {fichier}");
                        0
                    }
                    Err(e) => {
                        eprintln!("kobi : {fichier} : {e}");
                        1
                    }
                },
                None => {
                    let mut out = std::io::stdout().lock();
                    let _ = writeln!(out, "{texte}");
                    0
                }
            }
        }
        Err(e) => {
            if json {
                eprintln!("{}", serde_json::to_string(&e).unwrap_or_default());
            } else {
                eprintln!("kobi : {}", e.message);
            }
            e.code.sortie()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn lire(ligne: &str) -> Result<Demande, Erreur> {
        let args: Vec<String> = ligne.split_whitespace().map(String::from).collect();
        analyser(&args)
    }

    fn params(ligne: &str) -> (&'static str, Value) {
        match lire(ligne).unwrap() {
            Demande::Commande {
                commande, params, ..
            } => (commande.nom, params),
            autre => panic!("{ligne} → {autre:?}"),
        }
    }

    #[test]
    fn exemples_du_ticket() {
        assert_eq!(
            params("app open --section notes"),
            ("app.open", json!({"section": "notes"}))
        );
        assert_eq!(
            params("app open notes"),
            ("app.open", json!({"section": "notes"}))
        );
        assert_eq!(
            params("config set voice.assistant true"),
            (
                "config.set",
                json!({"key": "voice.assistant", "value": true})
            )
        );
        assert_eq!(params("meeting start"), ("meeting.start", json!({})));
        assert_eq!(
            params("notes last --format md"),
            ("notes.last", json!({"format": "md"}))
        );
        assert_eq!(params("status"), ("status", json!({})));
    }

    #[test]
    fn arguments() {
        assert_eq!(
            params("search point budget"),
            ("search", json!({"query": "point budget"}))
        );
        assert_eq!(
            params("reminders add appeler Sophie --due 2026-10-19T09:00:00+02:00"),
            (
                "reminders.add",
                json!({"text": "appeler Sophie", "due": "2026-10-19T09:00:00+02:00"})
            )
        );
        assert_eq!(
            params("reminders list --all"),
            ("reminders.list", json!({"all": true}))
        );
        assert_eq!(
            params("mascot discreet on"),
            ("mascot.discreet", json!({"on": true}))
        );
        assert_eq!(
            params("meetings list --limit=5"),
            ("meetings.list", json!({"limit": 5}))
        );
        assert_eq!(
            params("config set codebases [\"C:/code\"]"),
            (
                "config.set",
                json!({"key": "codebases", "value": ["C:/code"]})
            )
        );
        assert_eq!(
            params("config set hotkey Ctrl+Alt+K"),
            (
                "config.set",
                json!({"key": "hotkey", "value": "Ctrl+Alt+K"})
            )
        );
        match lire("meetings export last --out cr.md --json").unwrap() {
            Demande::Commande {
                sortie,
                json,
                params,
                ..
            } => {
                assert_eq!(sortie.as_deref(), Some("cr.md"));
                assert!(json);
                assert_eq!(params, json!({"id": "last"}));
            }
            autre => panic!("{autre:?}"),
        }
    }

    #[test]
    fn erreurs_d_usage() {
        for ligne in [
            "meeting",
            "nimporte quoi",
            "app open",
            "app open cuisine",
            "meetings list --limit trois",
            "mascot discreet peut-etre",
            "status en-trop",
            "meeting start --force",
            "mcp --root",
        ] {
            assert!(lire(ligne).is_err(), "{ligne}");
        }
        let e = lire("meeting").unwrap_err();
        assert!(e.message.contains("kobi meeting start"), "{}", e.message);
    }

    #[test]
    fn aide_et_mcp() {
        assert_eq!(lire("").unwrap(), Demande::Aide(None));
        assert_eq!(lire("help").unwrap(), Demande::Aide(None));
        assert!(
            matches!(lire("help meeting start").unwrap(), Demande::Aide(Some(c)) if c.nom == "meeting.start")
        );
        assert!(
            matches!(lire("notes last --help").unwrap(), Demande::Aide(Some(c)) if c.nom == "notes.last")
        );
        assert_eq!(lire("mcp").unwrap(), Demande::Mcp(Mode::Externe));
        assert_eq!(lire("mcp --agent").unwrap(), Demande::Mcp(Mode::Agent));
        let generale = aide(None);
        for c in COMMANDES {
            assert!(generale.contains(&c.mots()), "{}", c.nom);
        }
        assert!(aide(commandes::trouver("config.set")).contains("kobi config set <key> <value>"));
    }

    #[test]
    fn commandes_de_la_cli_et_options_de_l_app() {
        for mot in [
            "status", "app", "meeting", "notes", "mcp", "help", "search", "config",
        ] {
            assert!(est_une_commande(mot), "{mot}");
        }
        for mot in ["--carnet", "--reglages", "--bienvenue"] {
            assert!(!est_une_commande(mot), "{mot}");
        }
    }

    #[test]
    fn rendus_lisibles() {
        let c = |n| commandes::trouver(n).unwrap();
        assert_eq!(lisible(c("notes.last"), &json!("# Notes")), "# Notes");
        assert_eq!(
            lisible(
                c("meeting.start"),
                &json!({"message": "C'est parti", "reunion": {}})
            ),
            "C'est parti"
        );
        let st = lisible(
            c("status"),
            &json!({"etat": "reunion", "activite": "calme", "mascotte": "idle",
                    "reunion": {"actif": true, "pause": false, "notes": 4, "duree_s": 185.0},
                    "ecoute": {"pause": false}, "fenetre": {"visible": true, "discret": false}}),
        );
        assert!(
            st.contains("enregistrée depuis 3 min 05 s, 4 notes"),
            "{st}"
        );
        assert_eq!(lisible(c("config.get"), &json!(true)), "true");
        assert_eq!(lisible(c("mascot.show"), &json!({})), "OK");
        assert_eq!(lisible(c("reminders.list"), &json!([])), "(aucun)");
        let set = lisible(
            c("config.set"),
            &json!({"key": "voice.assistant", "before": true, "after": false, "apply": "hot"}),
        );
        assert_eq!(
            set,
            "voice.assistant : true → false (appliqué tout de suite)"
        );
    }
}
