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
use kobi_i18n::tr;

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
            _ => Err(usage(tr(
                "ctl.cli.pasBooleen",
                &[("nom", param.nom), ("texte", texte)],
            ))),
        },
        Genre::Entier => texte.parse::<u64>().map(Value::from).map_err(|_| {
            usage(tr(
                "ctl.cli.pasEntier",
                &[("nom", param.nom), ("texte", texte)],
            ))
        }),
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
            return c.map(|c| Demande::Aide(Some(c))).ok_or_else(|| {
                usage(tr(
                    "ctl.cli.commandeInconnue",
                    &[("commande", &mots.join(" "))],
                ))
            });
        }
        ["mcp", reste @ ..] => {
            return match reste {
                [] => Ok(Demande::Mcp(Mode::Externe)),
                ["--agent"] => Ok(Demande::Mcp(Mode::Agent)),
                _ => Err(usage(tr("ctl.cli.usageMcp", &[]))),
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
            tr("ctl.cli.commandeInconnueAide", &[("commande", args[0])])
        } else {
            tr(
                "ctl.cli.commandeIncomplete",
                &[("possibles", &groupe.join("\n  "))],
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
                        .ok_or_else(|| usage(tr("ctl.cli.fichierManquant", &[])))?
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
                        usage(tr(
                            "ctl.cli.optionInconnue",
                            &[("commande", &commande.mots()), ("nom", nom)],
                        ))
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
                            .ok_or_else(|| usage(tr("ctl.cli.valeurManquante", &[("nom", nom)])))?;
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
        return Err(usage(tr(
            "ctl.cli.argumentEnTrop",
            &[("commande", &commande.mots()), ("trop", trop)],
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
        let mut t = tr("ctl.cli.aide", &[]);
        for c in COMMANDES {
            t.push_str(&format!(
                "  {:<22} {:<11} {}\n",
                c.mots(),
                c.effet.etiquette_affichee(),
                c.description_affichee()
            ));
        }
        t.push_str(&tr("ctl.cli.aideCodes", &[]));
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
        "{ligne} {}\n\n{} {}\n",
        tr("ctl.cli.aideOptions", &[]),
        c.effet.etiquette_affichee(),
        c.description_affichee()
    );
    for p in c.params {
        t.push_str(&format!("  {:<10} {}", p.nom, c.description_du_param(p)));
        if !p.choix.is_empty() {
            t.push_str(&format!(" ({})", p.choix.join(", ")));
        }
        t.push('\n');
    }
    t
}

/// Libellé traduit d'un identifiant interne (état, activité) pour le rendu
/// lisible (KOB-270) ; l'identifiant lui-même s'il n'a pas de traduction.
/// `--json` garde l'identifiant brut.
fn libelle(prefixe: &str, id: &str) -> String {
    let cle = format!("{prefixe}.{id}");
    match tr(&cle, &[]) {
        texte if texte == cle => id.to_owned(),
        texte => texte,
    }
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
            Some(l) if l.is_empty() => tr("ctl.cli.aucun", &[]),
            Some(l) => l.iter().map(f).collect::<Vec<_>>().join("\n"),
            None => serde_json::to_string_pretty(liste).unwrap_or_default(),
        }
    };
    match commande.nom {
        "status" => {
            let reunion = &r["reunion"];
            let reunion = if reunion["actif"] == true {
                let etat = if reunion["pause"] == true {
                    tr("ctl.cli.enPause", &[])
                } else {
                    tr("ctl.cli.enregistree", &[])
                };
                tr(
                    "ctl.cli.reunionDepuis",
                    &[
                        ("etat", &etat),
                        (
                            "duree",
                            &duree(r["reunion"]["duree_s"].as_f64().unwrap_or(0.0)),
                        ),
                        ("n", &s(&reunion["notes"])),
                    ],
                )
            } else {
                tr("ctl.cli.aucune", &[])
            };
            let activite = r["activite"]
                .as_str()
                .map(|a| format!(", {}", libelle("ctl.activite", a)))
                .unwrap_or_default();
            let ecoute = if r["ecoute"]["pause"] == true {
                tr("ctl.cli.enPause", &[])
            } else {
                tr("ctl.cli.ecouteActive", &[])
            };
            let fenetre = if r["fenetre"]["visible"] == false {
                tr("ctl.cli.fenetreCachee", &[])
            } else {
                tr("ctl.cli.fenetreVisible", &[])
            };
            let discret = if r["fenetre"]["discret"] == true {
                tr("ctl.cli.modeDiscret", &[])
            } else {
                String::new()
            };
            tr(
                "ctl.cli.statut",
                &[
                    ("etat", &libelle("ctl.etat", &s(&r["etat"]))),
                    ("activite", &activite),
                    ("mascotte", &s(&r["mascotte"])),
                    ("reunion", &reunion),
                    ("ecoute", &ecoute),
                    ("fenetre", &fenetre),
                    ("discret", &discret),
                ],
            )
        }
        "meeting.start" | "meeting.stop" | "meeting.pause" | "meeting.resume" => s(&r["message"]),
        "meeting.status" => {
            if r["actif"] == true {
                let etat = if r["pause"] == true {
                    tr("ctl.cli.enPause", &[])
                } else {
                    tr("ctl.cli.enCours", &[])
                };
                let derniere = r["derniere_note"]
                    .as_str()
                    .map(|n| tr("ctl.cli.derniereNote", &[("note", n)]))
                    .unwrap_or_default();
                tr(
                    "ctl.cli.reunionEnCours",
                    &[
                        ("etat", &etat),
                        ("duree", &duree(r["duree_s"].as_f64().unwrap_or(0.0))),
                        ("n", &s(&r["notes"])),
                        ("derniere", &derniere),
                    ],
                )
            } else {
                tr("ctl.cli.aucuneReunion", &[])
            }
        }
        "meetings.list" => lignes(r, &|m| {
            format!(
                "{}  {}  {}{}",
                s(&m["nom"]),
                duree(m["duree_s"].as_f64().unwrap_or(0.0)),
                m["titre"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| tr("ctl.cli.sansTitre", &[])),
                if m["en_cours"] == true {
                    tr("ctl.cli.marqueEnCours", &[])
                } else {
                    String::new()
                }
            )
        }),
        "conversations.list" => lignes(r, &|c| {
            tr(
                "ctl.cli.conversation",
                &[
                    ("id", &s(&c["id"])),
                    ("n", &s(&c["tours"])),
                    ("apercu", &s(&c["apercu"])),
                ],
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
        "reminders.add" => tr(
            "ctl.cli.rappelAjoute",
            &[("texte", &s(&r["texte"])), ("id", &s(&r["id"]))],
        ),
        "search" => lignes(r, &|t| {
            let ou = match t["type"].as_str() {
                Some("reunion") => format!("{} ({})", s(&t["nom"]), s(&t["partie"])),
                _ => tr("ctl.cli.resultatConversation", &[("id", &s(&t["id"]))]),
            };
            tr(
                "ctl.cli.resultat",
                &[("ou", &ou), ("extrait", &s(&t["extrait"]))],
            )
        }),
        "config.get" if !r.is_object() => s(r),
        "config.list" => lignes(r, &|e| {
            format!(
                "{} = {}   [{}] {}",
                s(&e["key"]),
                e["value"],
                if e["apply"] == "hot" {
                    tr("ctl.cli.aChaud", &[])
                } else {
                    tr("ctl.cli.auRedemarrage", &[])
                },
                s(&e["description"])
            )
        }),
        "config.set" | "config.undo" => tr(
            "ctl.cli.reglageChange",
            &[
                ("cle", &s(&r["key"])),
                ("avant", &r["before"].to_string()),
                ("apres", &r["after"].to_string()),
                (
                    "application",
                    &match r["apply"].as_str() {
                        Some("hot") => tr("ctl.cli.appliqueTout", &[]),
                        Some(_) => tr("ctl.cli.appliqueRedemarrage", &[]),
                        None => String::new(),
                    },
                ),
            ],
        ),
        "meetings.export" => s(&r["markdown"]),
        "mascot.discreet" => {
            if r["discret"] == true {
                tr("ctl.cli.discretOui", &[])
            } else {
                tr("ctl.cli.discretNon", &[])
            }
        }
        "mascot.show" | "mascot.hide" => {
            if r["visible"] == true {
                tr("ctl.cli.mascotteVisible", &[])
            } else {
                tr("ctl.cli.mascotteCachee", &[])
            }
        }
        "app.open" => tr("ctl.cli.appOuverte", &[("section", &s(&r["section"]))]),
        "view.open" | "view.close" | "view.toggle" => match r["vue"].as_str() {
            Some("ouvrir") => tr("ctl.cli.vueOuverte", &[]),
            Some("fermer") => tr("ctl.cli.vueRangee", &[]),
            _ => tr("ctl.cli.vueBasculee", &[]),
        },
        _ if r.as_object().is_some_and(Map::is_empty) || r.is_null() => "OK".into(),
        _ => serde_json::to_string_pretty(r).unwrap_or_default(),
    }
}

/// Lance la CLI ; renvoie le code de retour.
pub fn main_avec(args: &[String]) -> i32 {
    // La langue de l'interface (`[ui] language`), sinon celle du système.
    kobi_i18n::regler(
        crate::transport::dossier_kobi()
            .and_then(|d| kobi_i18n::reglee_dans(&d.join("config.toml"))),
    );
    let demande = match analyser(args) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("kobi : {}", e.message);
            return USAGE;
        }
    };
    let Some(point) = Point::defaut() else {
        eprintln!("kobi : {}", tr("ctl.cli.dossierIntrouvable", &[]));
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
                        eprintln!("{}", tr("ctl.cli.ecrit", &[("fichier", fichier)]));
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

    /// KOB-270 : chaque état et chaque activité de la session a son libellé
    /// anglais (sinon `kobi status` montrerait l'identifiant français).
    #[test]
    fn etats_traduits_en_anglais() {
        use kobi_i18n::{t, Langue};
        let en = |cle: &str| t(Langue::En, cle, &[]);
        for etat in ["veille", "assistant", "reunion", "pause", "fin"] {
            let cle = format!("ctl.etat.{etat}");
            assert_ne!(en(&cle), cle);
        }
        for activite in [
            "calme",
            "ecoute",
            "reflechit",
            "note",
            "repond",
            "propose",
            "fait",
            "oups",
            "occupe",
        ] {
            let cle = format!("ctl.activite.{activite}");
            assert_ne!(en(&cle), cle);
        }
        assert_eq!(en("ctl.etat.veille"), "standby");
    }

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
        // KOB-256 : `null` remet le réglage par défaut.
        assert_eq!(
            params("config set ui.language null"),
            ("config.set", json!({"key": "ui.language", "value": null}))
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

    /// KOB-210 : la CLI parle anglais quand l'interface est en anglais.
    #[test]
    fn textes_anglais() {
        use kobi_i18n::{t, Langue};
        assert_eq!(
            t(
                Langue::En,
                "ctl.cli.commandeInconnueAide",
                &[("commande", "cuisine")]
            ),
            "unknown command: cuisine (see “kobi help”)"
        );
        assert_eq!(
            t(Langue::En, "ctl.cli.ecrit", &[("fichier", "cr.md")]),
            "written: cr.md"
        );
        assert!(t(Langue::En, "ctl.cli.aide", &[]).starts_with("Kobi: control"));
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
        // KOB-270 : l'état et l'activité sont traduits, pas l'identifiant.
        assert!(st.starts_with("Kobi      réunion, calme"), "{st}");
        assert_eq!(libelle("ctl.etat", "inconnu"), "inconnu");
        assert_eq!(lisible(c("config.get"), &json!(true)), "true");
        assert_eq!(lisible(c("app.quit"), &json!({})), "OK");
        assert_eq!(
            lisible(c("mascot.discreet"), &json!({"discret": true})),
            "Mode discret : Kobi est une pastille."
        );
        assert_eq!(lisible(c("reminders.list"), &json!([])), "(aucun)");
        assert_eq!(
            lisible(c("view.open"), &json!({"vue": "ouvrir"})),
            "Conversation ouverte en grand."
        );
        assert_eq!(params("view open"), ("view.open", json!({})));
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
