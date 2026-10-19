//! Essai contre le vrai Claude Code (KOB-165) : en réunion, Claude arrête
//! l'enregistrement avec `meeting_stop` pour toute demande d'arrêt, même mal
//! transcrite, et jamais sur une simple mention. Mesure aussi la latence de
//! l'arrêt (envoi de la question → appel de l'outil).
//!
//! Ignoré par défaut :
//! `cargo test -p kobi-app -- --ignored pilotage_reel --nocapture`.
//! Le MCP de Kobi est servi par `kobi-ctl mcp --agent` (à côté de
//! l'exécutable de test, ou `KOBI_CTL_EXE`), sur un point d'accès qui
//! n'existe pas : l'appel échoue sans toucher au Kobi qui tourne, et seul
//! compte le fait que Claude l'ait fait.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use kobi_brain::claude_code::Event;
use kobi_brain::{ClaudeCode, ClaudeOptions, Detection, SYSTEM_PROMPT};

use crate::lecture::{arguments, outils_appeles};
use crate::mcp_config::{assembler_avec, serveur_kobi};

/// Transcription de la réunion du 18/10 avant les demandes d'arrêt.
const AVANT: &str = "\
[00:05:10 Moi] Donc pour la démo de jeudi on garde le scénario court.
[00:05:31 Eux] Ok, et je m'occupe des captures d'écran.
[00:05:48 Moi] Parfait, je crois qu'on a fait le tour.";

/// (transcription récente, question telle qu'elle part à Claude, arrêt attendu)
const CAS: &[(&str, &str, bool)] = &[
    // Phrases réelles du 18/10 (KOB-165).
    (AVANT, "Pour ma merci, euh, fin de réunion, est-ce que je peux fin à l'enregistrement, s'il te plaît", true),
    (
        "[00:06:02 Moi] Pour ma merci, euh, fin de réunion, est-ce que je peux fin à l'enregistrement, s'il te plaît, Kobe",
        "Mais faire l'enregistrement",
        true,
    ),
    (AVANT, "Mais faut que tu mettes fin à l'enregistrement, on a fini la réunion", true),
    (AVANT, "faut que tu mettes fin à la réunion, on a fini l'enregistrement", true),
    // Autres façons d'arrêter.
    (AVANT, "c'est bon on a terminé tu peux couper", true),
    (AVANT, "arrête d'enregistrer s'il te plaît", true),
    (AVANT, "la réunion est finie merci", true),
    (AVANT, "on s'arrête là pour aujourd'hui, tu peux arrêter la prise de notes", true),
    (AVANT, "stoppe la réunion", true),
    (AVANT, "euh termine l'enregistrement", true),
    // Mentions, questions et demandes qui ne doivent pas arrêter.
    (AVANT, "on en reparle à la fin de la réunion", false),
    (AVANT, "à quelle heure finit la réunion ?", false),
    (AVANT, "résume ce qu'on a dit depuis le début de la réunion", false),
    (AVANT, "est-ce que tu enregistres bien ?", false),
    (AVANT, "combien de temps dure l'enregistrement pour l'instant ?", false),
    (AVANT, "note que la réunion de vendredi est annulée", false),
    (AVANT, "qu'est-ce qu'on a décidé pour la démo de jeudi ?", false),
    (AVANT, "tu peux rappeler qui s'occupe des captures ?", false),
    (AVANT, "on fera le point budget à la fin de la réunion, note-le", false),
    (AVANT, "l'enregistrement du client est arrivé hier, tu peux le noter ?", false),
    (AVANT, "c'est quoi la prochaine étape après la réunion ?", false),
    (AVANT, "comment je fais pour arrêter l'enregistrement tout seul ?", false),
    (AVANT, "rappelle-moi d'envoyer le compte rendu à Paul demain", false),
    (AVANT, "quels sont les points qu'on n'a pas encore traités ?", false),
    (AVANT, "on a fini le point sur le budget, passe au suivant dans les notes", false),
    (AVANT, "qui a parlé de la fin du projet ?", false),
    (AVANT, "la réunion de lundi s'est terminée à quelle heure ?", false),
    (AVANT, "ajoute une action : Thomas finit la maquette", false),
    (AVANT, "tu peux me dire ce que Paul a dit sur l'enregistrement des démos ?", false),
    (AVANT, "est-ce que la réunion est enregistrée depuis le début ?", false),
    (AVANT, "on termine par les questions ouvertes, liste-les", false),
    (AVANT, "à la fin de l'enregistrement tu feras le compte rendu ?", false),
    (AVANT, "quelle est la durée de la réunion jusqu'ici ?", false),
    (AVANT, "c'est fini pour le point démo, on passe au budget", false),
    (AVANT, "explique-moi comment marche le compte rendu", false),
    (AVANT, "qu'est-ce que tu as noté sur les captures d'écran ?", false),
    (AVANT, "arrête de répéter la même chose dans les notes", false),
    (AVANT, "coupe la phrase sur le budget dans les notes, elle est fausse", false),
    (AVANT, "stop, attends, je reformule", false),
    (AVANT, "le stop de la ligne 4 est cassé, tu peux regarder le code ?", false),
    (AVANT, "dis-moi quand il reste cinq minutes de réunion", false),
    (AVANT, "la fin de réunion approche, prépare un résumé", false),
    (AVANT, "qu'est-ce que tu feras quand la réunion sera terminée ?", false),
    (AVANT, "fais une liste des tâches à finir avant la fin du mois", false),
    (AVANT, "on termine bientôt, il reste un point sur le recrutement", false),
    (AVANT, "les enregistrements de la semaine dernière sont où ?", false),
    (AVANT, "j'ai fini ma partie, Paul, à toi", false),
    (AVANT, "le client veut arrêter le projet, note-le comme risque", false),
    (AVANT, "pourquoi tu as arrêté l'enregistrement la dernière fois ?", false),
    (AVANT, "termine ta phrase sur la démo", false),
];

/// Un cas : arrêt appelé ? latence de l'arrêt, réponse, outils.
fn essayer(
    programme: &Path,
    args: &[String],
    cwd: &Path,
    transcript: &str,
    question: &str,
) -> (bool, Option<Duration>, String, Vec<String>) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _contexte = rt.enter();
    let mut claude = ClaudeCode::spawn(&ClaudeOptions {
        program: programme.to_path_buf(),
        cwd: Some(cwd.to_path_buf()),
        partial_messages: false,
        args: args.to_vec(),
        env: Vec::new(),
    })
    .unwrap();
    rt.block_on(async {
        // Même message que la vraie conversation (kobi-brain), avec l'état
        // de la réunion ajouté par le chef.
        let message = format!(
            "[interpellation]\n\n## Nouvelle transcription\n{transcript}\n\n## Question\n(Réunion : enregistrement en cours.)\n{question}\n"
        );
        let debut = Instant::now();
        claude.send(&message).await.unwrap();
        let mut outils = Vec::new();
        let mut arret = None;
        loop {
            let e = claude.next_event().await.expect("flux");
            for o in outils_appeles(&e) {
                if o == "mcp__kobi__meeting_stop" && arret.is_none() {
                    arret = Some(debut.elapsed());
                }
                outils.push(o);
            }
            if let Event::Result(r) = e {
                return (
                    arret.is_some(),
                    arret,
                    r.result.unwrap_or_default(),
                    outils,
                );
            }
        }
    })
}

#[test]
#[ignore = "demande Claude Code connecté"]
fn pilotage_reel() {
    // Point d'accès introuvable pour le MCP : rien n'atteint le Kobi qui tourne.
    std::env::set_var("USERNAME", "kobi-essai-pilotage");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let programme = match rt.block_on(kobi_brain::claude_code::detect::detect(
        Duration::from_secs(30),
    )) {
        Detection::Ready { install, .. } => install.program,
        autre => panic!("Claude Code indisponible : {autre:?}"),
    };
    let ctl = std::env::var_os("KOBI_CTL_EXE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let exe = std::env::current_exe().unwrap();
            exe.parent()
                .and_then(|d| d.parent())
                .unwrap()
                .join(format!("kobi-ctl{}", std::env::consts::EXE_SUFFIX))
        });
    assert!(ctl.exists(), "kobi-ctl introuvable : {}", ctl.display());
    let cwd = std::env::temp_dir().join(format!("kobi-pilotage-{}", std::process::id()));
    std::fs::create_dir_all(&cwd).unwrap();
    let mcp = assembler_avec(&cwd.join("mcp.json"), serveur_kobi(&ctl), Vec::new()).unwrap();
    let args = arguments(
        crate::interpellation::MODELE_CLAUDE,
        SYSTEM_PROMPT,
        &[],
        None,
        Some(&mcp),
        true,
    );
    let filtre = std::env::var("KOBI_ESSAI_CAS").ok();
    let cas: Vec<_> = CAS
        .iter()
        .enumerate()
        .filter(|(i, _)| {
            filtre
                .as_ref()
                .is_none_or(|f| f.split(',').any(|n| n == i.to_string()))
        })
        .collect();
    // Trois sessions à la fois.
    let resultats: Vec<_> = std::thread::scope(|s| {
        cas.chunks(cas.len().div_ceil(3).max(1))
            .map(|lot| {
                let (programme, args, cwd) = (&programme, &args, &cwd);
                s.spawn(move || {
                    lot.iter()
                        .map(|(i, (t, q, attendu))| {
                            (*i, *q, *attendu, essayer(programme, args, cwd, t, q))
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect()
    });
    let mut erreurs = Vec::new();
    let mut latences = Vec::new();
    for (i, q, attendu, (arret, latence, reponse, outils)) in &resultats {
        let ok = arret == attendu;
        println!(
            "{} #{i:02} {} « {q} »\n      outils {outils:?}{}\n      -> {}",
            if ok { "OK  " } else { "RATÉ" },
            if *attendu { "arrêt" } else { "rien " },
            latence
                .map(|l| format!(", arrêt en {:.1} s", l.as_secs_f64()))
                .unwrap_or_default(),
            reponse.replace('\n', " "),
        );
        if let Some(l) = latence {
            latences.push(l.as_secs_f64());
        }
        if !ok {
            erreurs.push(*i);
        }
    }
    latences.sort_by(f64::total_cmp);
    if !latences.is_empty() {
        println!(
            "latence de l'arrêt : médiane {:.1} s, max {:.1} s ({} arrêts)",
            latences[latences.len() / 2],
            latences[latences.len() - 1],
            latences.len()
        );
    }
    let positifs = resultats.iter().filter(|r| r.2).count();
    println!(
        "{} cas ({} arrêts attendus, {} mentions) : {} ratés {:?}",
        resultats.len(),
        positifs,
        resultats.len() - positifs,
        erreurs.len(),
        erreurs
    );
    let _ = std::fs::remove_dir_all(&cwd);
    assert!(erreurs.is_empty(), "cas ratés : {erreurs:?}");
}
