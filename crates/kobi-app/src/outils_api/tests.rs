use std::fs;
use std::sync::{Arc, Mutex};

use kobi_brain::Outillage;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ServerCapabilities, ServerConfig};
use rmcp::{schemars, tool, tool_handler, tool_router, ServerHandler, ServiceExt};
use serde::Deserialize;

use super::*;

/// Codebase de test : du code, un binaire, un dossier ignoré.
fn codebase() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let racine = dir.path();
    fs::create_dir_all(racine.join("src/ui")).unwrap();
    fs::create_dir_all(racine.join("target/debug")).unwrap();
    fs::write(
        racine.join("README.md"),
        "# Kobi\nAssistant de réunion.\nTODO : la démo\n",
    )
    .unwrap();
    fs::write(
        racine.join("src/main.rs"),
        "fn main() {\n    // todo: lancer\n}\n",
    )
    .unwrap();
    fs::write(racine.join("src/ui/vue.ts"), "export const todo = 1;\n").unwrap();
    fs::write(racine.join("target/debug/genere.rs"), "// TODO ignoré\n").unwrap();
    fs::write(racine.join("image.bin"), b"TODO\0\x01\x02").unwrap();
    dir
}

fn lecteur(dir: &tempfile::TempDir) -> Lecteur {
    Lecteur::new(&[dir.path().to_path_buf()])
}

#[test]
fn read_numerote_et_decoupe() {
    let dir = codebase();
    let l = lecteur(&dir);
    let chemin = dir.path().join("README.md");
    assert_eq!(
        l.lire(&json!({ "file_path": chemin })).unwrap(),
        "1\t# Kobi\n2\tAssistant de réunion.\n3\tTODO : la démo\n"
    );
    assert_eq!(
        l.lire(&json!({ "file_path": chemin, "offset": 2, "limit": 1 }))
            .unwrap(),
        "2\tAssistant de réunion.\n"
    );
    // Relatif : depuis le premier dossier.
    assert!(l
        .lire(&json!({ "file_path": "src/main.rs" }))
        .unwrap()
        .contains("fn main"));
    assert!(l
        .lire(&json!({ "file_path": dir.path().join("image.bin") }))
        .unwrap_err()
        .contains("pas un fichier texte"));
    assert!(l.lire(&json!({})).unwrap_err().contains("file_path"));
}

/// Rien ne se lit hors des dossiers permis : chemin absolu, `..`, lien
/// symbolique qui sort.
#[test]
fn rien_hors_des_dossiers_permis() {
    let dir = codebase();
    let dehors = tempfile::tempdir().unwrap();
    let secret = dehors.path().join("secret.txt");
    fs::write(&secret, "mot de passe").unwrap();
    let l = lecteur(&dir);
    let relatif = format!(
        "../{}/secret.txt",
        dehors.path().file_name().unwrap().to_string_lossy()
    );
    for chemin in [secret.display().to_string(), relatif] {
        let e = l.lire(&json!({ "file_path": chemin })).unwrap_err();
        assert!(e.contains("hors des dossiers permis"), "{e}");
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&secret, dir.path().join("lien.txt")).unwrap();
        let e = l
            .lire(&json!({ "file_path": dir.path().join("lien.txt") }))
            .unwrap_err();
        assert!(e.contains("hors des dossiers permis"), "{e}");
        // Grep ne suit pas les liens.
        assert_eq!(
            l.chercher(&json!({ "pattern": "passe" })).unwrap(),
            "Aucun résultat."
        );
    }
    let e = l
        .chercher(&json!({ "pattern": "x", "path": dehors.path() }))
        .unwrap_err();
    assert!(e.contains("hors des dossiers permis"), "{e}");
    // Sans dossier, rien à lire.
    assert!(Lecteur::new(&[])
        .chercher(&json!({ "pattern": "x" }))
        .is_err());
}

#[test]
fn grep_cherche_sans_les_dossiers_ignores_ni_les_binaires() {
    let dir = codebase();
    let l = lecteur(&dir);
    let r = l.chercher(&json!({ "pattern": "TODO" })).unwrap();
    assert_eq!(
        r,
        format!(
            "{}:3:TODO : la démo",
            dir.path()
                .canonicalize()
                .unwrap()
                .join("README.md")
                .display()
        )
    );
    let r = l
        .chercher(&json!({ "pattern": "todo", "-i": true }))
        .unwrap();
    assert_eq!(r.lines().count(), 3, "{r}");
    let r = l
        .chercher(&json!({ "pattern": "todo", "-i": true, "glob": "*.rs" }))
        .unwrap();
    assert!(r.contains("main.rs:2:") && r.lines().count() == 1, "{r}");
    assert!(l
        .chercher(&json!({ "pattern": "(" }))
        .unwrap_err()
        .contains("expression invalide"));
}

#[test]
fn glob_liste_les_chemins() {
    let dir = codebase();
    let l = lecteur(&dir);
    let racine = dir.path().canonicalize().unwrap();
    assert_eq!(
        l.lister(&json!({ "pattern": "**/*.rs" })).unwrap(),
        racine.join("src/main.rs").display().to_string()
    );
    assert_eq!(
        l.lister(&json!({ "pattern": "src/*" })).unwrap(),
        racine.join("src/main.rs").display().to_string()
    );
    assert_eq!(
        l.lister(&json!({ "pattern": "*.ts", "path": racine.join("src/ui") }))
            .unwrap(),
        racine.join("src/ui/vue.ts").display().to_string()
    );
    assert_eq!(
        l.lister(&json!({ "pattern": "*.py" })).unwrap(),
        "Aucun fichier."
    );
}

// ----- Faux serveur MCP -----

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct Note {
    /// Texte de la note.
    texte: String,
}

#[derive(Clone)]
struct Notes {
    ecrites: Arc<Mutex<Vec<String>>>,
    #[allow(dead_code, reason = "lu par la macro tool_handler")]
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl Notes {
    #[tool(description = "Lit les notes.", annotations(read_only_hint = true))]
    fn lire_notes(&self) -> String {
        "- Décision : livrer vendredi.".into()
    }

    #[tool(
        description = "Ajoute une note.",
        annotations(read_only_hint = false, destructive_hint = false)
    )]
    fn ecrire_note(&self, Parameters(note): Parameters<Note>) -> String {
        self.ecrites.lock().unwrap().push(note.texte);
        "ok".into()
    }
}

#[tool_handler]
impl ServerHandler for Notes {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }
}

/// Faux serveur MCP dans le process, relié par un tube.
async fn serveur_notes() -> (
    RunningService<RoleClient, ()>,
    Vec<Value>,
    Arc<Mutex<Vec<String>>>,
) {
    let (cote_client, cote_serveur) = tokio::io::duplex(64 * 1024);
    let ecrites = Arc::new(Mutex::new(Vec::new()));
    let notes = Notes {
        ecrites: ecrites.clone(),
        tool_router: Notes::tool_router(),
    };
    tokio::spawn(async move {
        let serveur = notes.serve(tokio::io::split(cote_serveur)).await.unwrap();
        let _ = serveur.waiting().await;
    });
    let client = ().serve(tokio::io::split(cote_client)).await.unwrap();
    let outils = client
        .list_all_tools()
        .await
        .unwrap()
        .iter()
        .map(|o| serde_json::to_value(o).unwrap())
        .collect();
    (client, outils, ecrites)
}

fn catalogue(politique: kobi_perms::mcp::Politique) -> Catalogue {
    use kobi_perms::mcp::Serveur;
    let mut c = Catalogue::default();
    c.serveurs.insert(
        "notes".into(),
        Serveur {
            politique,
            outils: [
                ("lire_notes".to_owned(), Nature::Lecture),
                ("ecrire_note".to_owned(), Nature::Ecriture),
                ("effacer".to_owned(), Nature::Suppression),
            ]
            .into(),
        },
    );
    c
}

fn noms(o: &OutillageKobi) -> Vec<String> {
    o.definitions()
        .iter()
        .map(|d| d["name"].as_str().unwrap().to_owned())
        .collect()
}

/// KOB-102 et KOB-320 : tous les outils MCP sont proposés et appelés ; la
/// lecture connue passe, l'écriture demande une carte. Les outils internes
/// passent par le même point d'entrée.
#[tokio::test]
async fn outils_mcp_et_autorisations() {
    use kobi_perms::mcp::Politique;
    let dir = codebase();
    let (client, outils, ecrites) = serveur_notes().await;
    let o = OutillageKobi::new(
        &[dir.path().to_path_buf()],
        vec![("notes".into(), client, outils)],
        catalogue(Politique::Demander),
        Acces::Complet,
    );
    assert_eq!(
        noms(&o),
        [
            "Read",
            "Grep",
            "Glob",
            "mcp__notes__ecrire_note",
            "mcp__notes__lire_notes"
        ]
    );
    for d in o.definitions() {
        assert_eq!(d["input_schema"]["type"], "object", "{d}");
        assert!(!d["description"].as_str().unwrap().is_empty(), "{d}");
    }
    for (nom, attendu) in [
        ("Read", Autorisation::Libre),
        ("Grep", Autorisation::Libre),
        ("Glob", Autorisation::Libre),
        ("mcp__notes__lire_notes", Autorisation::Libre),
        ("mcp__notes__ecrire_note", Autorisation::Carte),
        ("mcp__notes__effacer", Autorisation::Carte),
        ("mcp__autre__lire_notes", Autorisation::Carte),
        ("Bash", Autorisation::Carte),
    ] {
        assert_eq!(o.autorisation(nom), attendu, "{nom}");
    }
    assert_eq!(
        o.appeler("mcp__notes__lire_notes", &json!({}))
            .await
            .unwrap(),
        "- Décision : livrer vendredi."
    );
    assert!(matches!(
        o.action("mcp__notes__ecrire_note", &json!({ "texte": "x" })),
        kobi_brain::Action::Mcp { serveur, outil, .. } if serveur == "notes" && outil == "ecrire_note"
    ));
    // L'appel, une fois la carte acceptée par le cerveau, arrive au serveur.
    assert_eq!(
        o.appeler("mcp__notes__ecrire_note", &json!({ "texte": "x" }))
            .await
            .unwrap(),
        "ok"
    );
    assert_eq!(*ecrites.lock().unwrap(), ["x"]);
    let e = o
        .appeler("mcp__notes__inconnu", &json!({}))
        .await
        .unwrap_err();
    assert!(e.contains("outil inconnu"), "{e}");
    assert!(o
        .appeler("Grep", &json!({ "pattern": "Assistant" }))
        .await
        .unwrap()
        .contains("README.md:2:"));
    assert!(o
        .appeler("Bash", &json!({ "command": "ls" }))
        .await
        .is_err());
    o.fermer().await;
}

/// KOB-320 : la nature vient du catalogue, pas des annotations reçues en
/// direct. Un outil annoncé en lecture mais absent du catalogue (jamais
/// testé) demande une carte, comme chez Claude Code.
#[tokio::test]
async fn annotation_seule_ne_suffit_pas() {
    let (client, outils, _) = serveur_notes().await;
    let lecture_annoncee = outils
        .iter()
        .any(|o| o["name"] == "lire_notes" && o["annotations"]["readOnlyHint"] == true);
    assert!(lecture_annoncee);
    let o = OutillageKobi::new(
        &[],
        vec![("notes".into(), client, outils)],
        Catalogue::default(),
        Acces::Complet,
    );
    assert_eq!(
        o.autorisation("mcp__notes__lire_notes"),
        Autorisation::Carte
    );
    o.fermer().await;
}

/// Serveur réglé « faire sans demander » : l'écriture passe sans carte,
/// une suppression en demande toujours une.
#[tokio::test]
async fn serveur_sans_demander() {
    use kobi_perms::mcp::Politique;
    let (client, outils, _) = serveur_notes().await;
    let o = OutillageKobi::new(
        &[],
        vec![("notes".into(), client, outils)],
        catalogue(Politique::SansDemander),
        Acces::Complet,
    );
    assert_eq!(
        o.autorisation("mcp__notes__ecrire_note"),
        Autorisation::Libre
    );
    assert_eq!(o.autorisation("mcp__notes__effacer"), Autorisation::Carte);
    o.fermer().await;
}

/// Question différée (KOB-307) : seuls les outils connus en lecture sont
/// proposés ; une écriture est refusée sans carte, même sur un serveur
/// « faire sans demander ».
#[tokio::test]
async fn lecture_seule_sans_carte() {
    use kobi_perms::mcp::Politique;
    let (client, outils, ecrites) = serveur_notes().await;
    let o = OutillageKobi::new(
        &[],
        vec![("notes".into(), client, outils)],
        catalogue(Politique::SansDemander),
        Acces::LectureSeule,
    );
    assert_eq!(noms(&o), ["Read", "Grep", "Glob", "mcp__notes__lire_notes"]);
    assert_eq!(
        o.autorisation("mcp__notes__lire_notes"),
        Autorisation::Libre
    );
    for refuse in [
        "mcp__notes__ecrire_note",
        "mcp__notes__effacer",
        "mcp__autre__x",
        "Bash",
    ] {
        assert_eq!(o.autorisation(refuse), Autorisation::Refus, "{refuse}");
    }
    assert!(o
        .appeler("mcp__notes__ecrire_note", &json!({ "texte": "x" }))
        .await
        .is_err());
    assert!(ecrites.lock().unwrap().is_empty());
    o.fermer().await;
}
