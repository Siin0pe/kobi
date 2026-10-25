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

    #[tool(
        description = "Efface les notes.",
        annotations(destructive_hint = true)
    )]
    fn effacer(&self) -> String {
        self.ecrites.lock().unwrap().clear();
        "effacé".into()
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
/// lecture connue passe, l'écriture demande une carte, un nom non proposé
/// est refusé sans carte. Les outils internes passent par le même point
/// d'entrée.
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
            "mcp__notes__effacer",
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
        // Jamais proposés : refus sans carte.
        ("mcp__notes__inconnu", Autorisation::Refus),
        ("mcp__autre__lire_notes", Autorisation::Refus),
        ("mcp__notes__LIRE_NOTES", Autorisation::Refus),
        ("mcp__notes__lire_notes__x", Autorisation::Refus),
        ("mcp_notes__lire_notes", Autorisation::Refus),
        ("Bash", Autorisation::Refus),
        ("read", Autorisation::Refus),
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
/// direct. Un outil annoncé `readOnlyHint` mais absent du catalogue de son
/// serveur (ajouté depuis le dernier test), ou d'un serveur jamais testé,
/// demande une carte, comme chez Claude Code.
#[tokio::test]
async fn annotation_seule_ne_suffit_pas() {
    use kobi_perms::mcp::{Politique, Serveur};
    let (client, outils, _) = serveur_notes().await;
    assert!(outils
        .iter()
        .any(|o| o["name"] == "lire_notes" && o["annotations"]["readOnlyHint"] == true));
    // Serveur connu, `lire_notes` absent de son catalogue.
    let mut connu = Catalogue::default();
    connu.serveurs.insert(
        "notes".into(),
        Serveur {
            politique: Politique::Demander,
            outils: [("ecrire_note".to_owned(), Nature::Ecriture)].into(),
        },
    );
    let o = OutillageKobi::new(
        &[],
        vec![("notes".into(), client, outils)],
        connu,
        Acces::Complet,
    );
    assert_eq!(
        o.autorisation("mcp__notes__lire_notes"),
        Autorisation::Carte
    );
    o.fermer().await;
    // Serveur jamais testé.
    let (client, outils, _) = serveur_notes().await;
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

/// Question différée (KOB-307) : aucun serveur de l'utilisateur, comme
/// chez Claude Code ; du MCP de Kobi, seuls les outils connus en lecture
/// sont proposés, le reste est refusé sans carte, même « sans demander ».
#[tokio::test]
async fn lecture_seule_sans_carte() {
    use kobi_perms::mcp::Politique;
    let kobi = crate::mcp_config::SERVEUR_KOBI;
    let (utilisateur, outils_u, ecrites_u) = serveur_notes().await;
    let (propre, outils_k, ecrites_k) = serveur_notes().await;
    let mut cat = catalogue(Politique::SansDemander);
    let notes = cat.serveurs["notes"].clone();
    cat.serveurs.insert(kobi.into(), notes);
    let o = OutillageKobi::new(
        &[],
        vec![
            ("notes".into(), utilisateur, outils_u),
            (kobi.into(), propre, outils_k),
        ],
        cat,
        Acces::LectureSeule,
    );
    assert_eq!(
        noms(&o),
        ["Read", "Grep", "Glob", &format!("mcp__{kobi}__lire_notes")]
    );
    assert_eq!(
        o.autorisation(&format!("mcp__{kobi}__lire_notes")),
        Autorisation::Libre
    );
    for refuse in [
        "mcp__notes__lire_notes".to_owned(),
        format!("mcp__{kobi}__ecrire_note"),
        format!("mcp__{kobi}__effacer"),
        "Bash".to_owned(),
    ] {
        assert_eq!(o.autorisation(&refuse), Autorisation::Refus, "{refuse}");
    }
    for nom in [
        "mcp__notes__lire_notes".to_owned(),
        format!("mcp__{kobi}__ecrire_note"),
    ] {
        assert!(
            o.appeler(&nom, &json!({ "texte": "x" })).await.is_err(),
            "{nom}"
        );
    }
    assert!(ecrites_u.lock().unwrap().is_empty() && ecrites_k.lock().unwrap().is_empty());
    o.fermer().await;
}

// ----- Noms d'outils hors du format de l'API (KOB-317) -----

fn conforme(nom: &str) -> bool {
    !nom.is_empty()
        && nom.len() <= NOM_MAX
        && nom
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

#[test]
fn noms_normalises_pour_l_api() {
    let mut pris = BTreeMap::new();
    let mut ajouter = |serveur: &str, outil: &str| {
        let nom = nom_pour_l_api(serveur, outil, &pris);
        assert!(conforme(&nom), "{nom}");
        pris.insert(nom.clone(), (serveur.to_owned(), outil.to_owned()));
        nom
    };
    // Déjà conforme : inchangé, comme chez Claude Code.
    assert_eq!(ajouter("linear", "list_issues"), "mcp__linear__list_issues");
    assert_eq!(ajouter("fs", "files.read"), "mcp__fs__files_read");
    // Doublon après normalisation : numéroté.
    assert_eq!(ajouter("fs", "files_read"), "mcp__fs__files_read_2");
    assert_eq!(ajouter("fs", "files read"), "mcp__fs__files_read_3");
    // Trop long : coupé, avec une empreinte stable du vrai nom.
    let long = "un_outil_au_nom_vraiment_beaucoup_trop_long_pour_l_api_messages_d_anthropic";
    let a = ajouter("serveur", long);
    assert_eq!(a.len(), NOM_MAX);
    assert_eq!(a, nom_pour_l_api("serveur", long, &BTreeMap::new()));
    let b = ajouter("serveur", &format!("{long}_bis"));
    assert_ne!(a, b);
    // Caractères non ASCII.
    assert_eq!(ajouter("fs", "lire/fichier.é"), "mcp__fs__lire_fichier__");
}

#[derive(Clone)]
struct Pointe {
    lus: Arc<Mutex<Vec<String>>>,
    #[allow(dead_code, reason = "lu par la macro tool_handler")]
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl Pointe {
    #[tool(
        name = "files.read",
        description = "Lit un fichier.",
        annotations(read_only_hint = true)
    )]
    fn files_read(&self) -> String {
        self.lus.lock().unwrap().push("files.read".into());
        "contenu".into()
    }

    #[tool(
        name = "un_outil_au_nom_vraiment_beaucoup_trop_long_pour_l_api_messages_d_anthropic",
        description = "Outil au nom trop long.",
        annotations(read_only_hint = true)
    )]
    fn long(&self) -> String {
        self.lus.lock().unwrap().push("long".into());
        "long".into()
    }
}

#[tool_handler]
impl ServerHandler for Pointe {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }
}

/// KOB-317 : un outil `files.read` (ou au nom trop long) est proposé sous
/// un nom valide pour l'API, et appelé sous son vrai nom ; les règles et
/// l'action portent sur le vrai nom.
#[tokio::test]
async fn outil_au_nom_hors_format() {
    use kobi_perms::mcp::{Politique, Serveur};
    let (cote_client, cote_serveur) = tokio::io::duplex(64 * 1024);
    let lus = Arc::new(Mutex::new(Vec::new()));
    let pointe = Pointe {
        lus: lus.clone(),
        tool_router: Pointe::tool_router(),
    };
    tokio::spawn(async move {
        let serveur = pointe.serve(tokio::io::split(cote_serveur)).await.unwrap();
        let _ = serveur.waiting().await;
    });
    let client = ().serve(tokio::io::split(cote_client)).await.unwrap();
    let outils: Vec<Value> = client
        .list_all_tools()
        .await
        .unwrap()
        .iter()
        .map(|o| serde_json::to_value(o).unwrap())
        .collect();
    let mut cat = Catalogue::default();
    cat.serveurs.insert(
        "fs".into(),
        Serveur {
            politique: Politique::Demander,
            outils: [("files.read".to_owned(), Nature::Lecture)].into(),
        },
    );
    let o = OutillageKobi::new(
        &[],
        vec![("fs".into(), client, outils)],
        cat,
        Acces::Complet,
    );
    let mcp: Vec<String> = noms(&o)
        .into_iter()
        .filter(|n| n.starts_with("mcp__"))
        .collect();
    assert_eq!(mcp.len(), 2, "{mcp:?}");
    assert!(mcp.iter().all(|n| conforme(n)), "{mcp:?}");
    assert!(mcp.contains(&"mcp__fs__files_read".to_owned()));
    let long = mcp
        .iter()
        .find(|n| *n != "mcp__fs__files_read")
        .unwrap()
        .clone();

    // Règles sur le vrai nom : `files.read` est une lecture connue, l'outil
    // au nom long est hors catalogue.
    assert_eq!(o.autorisation("mcp__fs__files_read"), Autorisation::Libre);
    assert_eq!(o.autorisation(&long), Autorisation::Carte);
    // Le nom brut n'a jamais été proposé.
    assert_eq!(o.autorisation("mcp__fs__files.read"), Autorisation::Refus);
    assert!(matches!(
        o.action("mcp__fs__files_read", &json!({})),
        kobi_brain::Action::Mcp { serveur, outil, .. } if serveur == "fs" && outil == "files.read"
    ));
    assert_eq!(
        o.appeler("mcp__fs__files_read", &json!({})).await.unwrap(),
        "contenu"
    );
    assert_eq!(o.appeler(&long, &json!({})).await.unwrap(), "long");
    assert!(o.appeler("mcp__fs__files.read", &json!({})).await.is_err());
    assert_eq!(*lus.lock().unwrap(), ["files.read", "long"]);
    o.fermer().await;
}

// ----- Confinement (KOB-321, demandé pour la relecture de KOB-103) -----

/// Dossier déclaré, et à côté un dossier « dehors » avec un secret.
fn deux_dossiers() -> (tempfile::TempDir, tempfile::TempDir, PathBuf) {
    let dedans = codebase();
    let dehors = tempfile::tempdir().unwrap();
    let secret = dehors.path().join("secret.txt");
    fs::write(&secret, "mot de passe\n").unwrap();
    (dedans, dehors, secret)
}

fn hors(r: Result<String, String>) {
    let e = r.expect_err("devait être refusé");
    assert!(e.contains("hors des dossiers permis"), "{e}");
}

/// `..` sous toutes ses formes : relatif, absolu qui repasse par le
/// dossier, et `path` de Grep et de Glob.
#[test]
fn confinement_points_points() {
    let (dedans, dehors, _) = deux_dossiers();
    let l = lecteur(&dedans);
    let voisin = dehors
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    for chemin in [
        format!("../{voisin}/secret.txt"),
        format!("src/../../{voisin}/secret.txt"),
        format!("{}/src/../../{voisin}/secret.txt", dedans.path().display()),
    ] {
        hors(l.lire(&json!({ "file_path": chemin })));
    }
    hors(l.chercher(&json!({ "pattern": "passe", "path": format!("../{voisin}") })));
    hors(l.lister(&json!({ "pattern": "*", "path": format!("../{voisin}") })));
    // Un motif de Glob ne remonte pas : il ne porte que sur les chemins
    // trouvés sous le dossier.
    assert_eq!(
        l.lister(&json!({ "pattern": format!("../{voisin}/*") }))
            .unwrap(),
        "Aucun fichier."
    );
}

/// Chemins absolus hors des dossiers, fichiers système compris.
#[test]
fn confinement_chemins_absolus() {
    let (dedans, _dehors, secret) = deux_dossiers();
    let l = lecteur(&dedans);
    hors(l.lire(&json!({ "file_path": secret })));
    #[cfg(unix)]
    for systeme in ["/etc/passwd", "/etc/hostname", "/"] {
        if Path::new(systeme).exists() {
            hors(l.lire(&json!({ "file_path": systeme })));
            hors(l.chercher(&json!({ "pattern": "root", "path": systeme })));
        }
    }
    // Un préfixe de nom n'est pas un dossier parent : `/x/code-bis` n'est
    // pas dans `/x/code`.
    let parent = tempfile::tempdir().unwrap();
    let (code, bis) = (parent.path().join("code"), parent.path().join("code-bis"));
    fs::create_dir_all(&code).unwrap();
    fs::create_dir_all(&bis).unwrap();
    fs::write(bis.join("a.txt"), "x").unwrap();
    hors(Lecteur::new(&[code]).lire(&json!({ "file_path": bis.join("a.txt") })));
}

/// Liens symboliques qui sortent : vers un fichier, vers un dossier (ni
/// lus, ni parcourus), et dossier déclaré qui est lui-même un lien.
#[cfg(unix)]
#[test]
fn confinement_liens_symboliques() {
    use std::os::unix::fs::symlink;
    let (dedans, dehors, secret) = deux_dossiers();
    symlink(&secret, dedans.path().join("lien.txt")).unwrap();
    symlink(dehors.path(), dedans.path().join("lien-dossier")).unwrap();
    let l = lecteur(&dedans);
    hors(l.lire(&json!({ "file_path": dedans.path().join("lien.txt") })));
    hors(l.lire(&json!({ "file_path": "lien-dossier/secret.txt" })));
    hors(l.chercher(&json!({ "pattern": "passe", "path": "lien-dossier" })));
    hors(l.lister(&json!({ "pattern": "*", "path": "lien-dossier" })));
    assert_eq!(
        l.chercher(&json!({ "pattern": "passe" })).unwrap(),
        "Aucun résultat."
    );
    let tout = l.lister(&json!({ "pattern": "**/*" })).unwrap();
    assert!(!tout.contains("secret") && !tout.contains("lien"), "{tout}");
    // Le dossier déclaré par un lien : c'est sa cible qui est permise, et
    // seulement elle.
    let parent = tempfile::tempdir().unwrap();
    symlink(dedans.path(), parent.path().join("code")).unwrap();
    let l = Lecteur::new(&[parent.path().join("code")]);
    assert!(l
        .lire(&json!({ "file_path": parent.path().join("code/README.md") }))
        .is_ok());
    hors(l.lire(&json!({ "file_path": secret })));
}

/// Fichiers spéciaux : un tube nommé ne bloque pas (ni Read, ni Grep, ni
/// Glob), un dossier ou un périphérique n'est pas lu.
#[cfg(unix)]
#[test]
fn confinement_fichiers_speciaux() {
    let dedans = codebase();
    let tube = dedans.path().join("tube");
    assert!(std::process::Command::new("mkfifo")
        .arg(&tube)
        .status()
        .unwrap()
        .success());
    let l = lecteur(&dedans);
    let e = l.lire(&json!({ "file_path": tube })).unwrap_err();
    assert!(e.contains("pas un fichier ordinaire"), "{e}");
    let e = l.lire(&json!({ "file_path": "src" })).unwrap_err();
    assert!(e.contains("pas un fichier ordinaire"), "{e}");
    assert!(!l
        .chercher(&json!({ "pattern": "x" }))
        .unwrap()
        .contains("tube"));
    assert!(!l
        .chercher(&json!({ "pattern": "x", "path": "tube" }))
        .unwrap()
        .contains("tube"));
    assert!(!l
        .lister(&json!({ "pattern": "*" }))
        .unwrap()
        .contains("tube"));
    // Un lien vers un périphérique sort des dossiers.
    std::os::unix::fs::symlink("/dev/zero", dedans.path().join("zero")).unwrap();
    hors(l.lire(&json!({ "file_path": "zero" })));
}

/// KOB-321 : un très gros fichier n'est jamais chargé en entier. Read lit
/// ligne à ligne et le dit ; une ligne géante est coupée ; les résultats
/// restent bornés ; Grep ignore le fichier.
#[test]
fn gros_fichier_lu_par_morceaux() {
    use std::io::Write;
    let dedans = codebase();
    let gros = dedans.path().join("gros.log");
    // Fichier creux de 60 Mo : 10 000 lignes de texte (100 ko), puis une
    // « ligne » de zéros sans fin de ligne.
    let mut f = fs::File::create(&gros).unwrap();
    f.write_all(b"premiere TODO\nseconde\n").unwrap();
    for i in 3..=10_000 {
        writeln!(f, "ligne {i:05}").unwrap();
    }
    f.set_len(60_000_000).unwrap();
    drop(f);
    let l = lecteur(&dedans);
    assert_eq!(
        l.lire(&json!({ "file_path": gros, "limit": 2 })).unwrap(),
        "1\tpremiere TODO\n2\tseconde\n(fichier de 60000000 octets : seuls les 50000000 premiers sont lus)\n"
    );
    assert!(l
        .lire(&json!({ "file_path": gros, "offset": 9_999, "limit": 2 }))
        .unwrap()
        .starts_with("9999\tligne 09999\n10000\tligne 10000\n"));
    // La ligne 10 001 fait près de 60 Mo : coupée à LIGNE_MAX.
    let r = l
        .lire(&json!({ "file_path": gros, "offset": 10_001, "limit": 1 }))
        .unwrap();
    assert!(r.starts_with("10001\t"), "{}", &r[..10]);
    assert!(r.len() < LIGNE_MAX * 4 + 200, "{}", r.len());
    // Au-delà de ce qui est parcouru : rien, mais pas d'erreur.
    let r = l
        .lire(&json!({ "file_path": gros, "offset": 20_000 }))
        .unwrap();
    assert!(
        r.starts_with("(fichier vide ou fin de fichier dépassée)"),
        "{r}"
    );
    assert_eq!(
        l.chercher(&json!({ "pattern": "premiere" })).unwrap(),
        "Aucun résultat."
    );
}

/// Beaucoup de lignes ou de correspondances : résultats bornés.
#[test]
fn resultats_bornes() {
    let dedans = codebase();
    let long = "x".repeat(1000) + "\n";
    fs::write(dedans.path().join("long.txt"), long.repeat(500)).unwrap();
    let l = lecteur(&dedans);
    let r = l
        .lire(&json!({ "file_path": dedans.path().join("long.txt") }))
        .unwrap();
    assert!(
        r.len() <= TAILLE_MAX + 20 && r.ends_with("… (coupé)"),
        "{}",
        r.len()
    );
    for i in 0..400 {
        fs::write(dedans.path().join(format!("f{i}.txt")), "cible\n").unwrap();
    }
    let r = l.chercher(&json!({ "pattern": "cible" })).unwrap();
    assert_eq!(r.lines().count(), RESULTATS_MAX + 1);
    let r = l.lister(&json!({ "pattern": "f*.txt" })).unwrap();
    assert_eq!(r.lines().count(), RESULTATS_MAX + 1);
}
