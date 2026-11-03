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

/// KOB-344 : un chemin réseau ou de périphérique Windows est refusé avant
/// toute résolution (qui ouvrirait la connexion SMB et enverrait le hash
/// NTLM) ; jugé sur le texte, il se teste sous Linux. Le chemin verbatim d'un
/// disque local, que rend `canonicalize` sous Windows, passe.
#[test]
fn chemins_reseau_refuses_avant_toute_resolution() {
    for reseau in [
        r"\\attaquant.example\partage\x",
        "//attaquant.example/partage/x",
        r"\\?\UNC\attaquant\partage\x",
        r"\\.\pipe\nom",
        r"\\?\GLOBALROOT\Device\x",
        r"\\?\Volume{0000}\x",
        r"\\localhost\c$\Windows",
        r"/\attaquant\partage",
        r"\??\UNC\attaquant\partage\x",
        "/??/pipe/nom",
        r"\??\GLOBALROOT\Device\Mup\x",
    ] {
        assert!(chemin_reseau(reseau), "{reseau}");
    }
    for local in [
        r"\\?\C:\code\kobi\src\main.rs",
        r"C:\code\kobi",
        "/code/kobi/src",
        "src/main.rs",
        "../secret",
    ] {
        assert!(!chemin_reseau(local), "{local}");
    }
    let dir = codebase();
    let l = lecteur(&dir);
    let refuse = |r: Result<String, String>| {
        let e = r.unwrap_err();
        assert!(e.contains("chemin réseau"), "{e}");
    };
    for chemin in [
        r"\\attaquant\partage\x",
        "//attaquant/partage/x",
        r"\??\UNC\attaquant\partage\x",
    ] {
        refuse(l.lire(&json!({ "file_path": chemin })));
        refuse(l.chercher(&json!({ "pattern": "x", "path": chemin })));
        refuse(l.lister(&json!({ "pattern": "*", "path": chemin })));
    }
    // Sans dossier permis : rien ne se résout, pas même un chemin qui
    // échapperait au contrôle du texte.
    let vide = Lecteur::new(&[]);
    for chemin in [r"\??\UNC\attaquant\p\x", "COM1", "/etc/passwd"] {
        let e = vide.lire(&json!({ "file_path": chemin })).unwrap_err();
        assert!(e.contains("aucun dossier permis"), "{e}");
    }
    // Liste blanche des préfixes (seuls les chemins Windows en ont).
    assert!(prefixe_local(Path::new("/code/kobi")));
    assert!(prefixe_local(Path::new("src/main.rs")));
    #[cfg(windows)]
    {
        for reseau in [r"\\srv\p\x", r"\\?\UNC\srv\p", r"\\.\pipe\x"] {
            assert!(!prefixe_local(Path::new(reseau)), "{reseau}");
        }
        for local in [r"C:\code\kobi", r"\\?\C:\code\kobi"] {
            assert!(prefixe_local(Path::new(local)), "{local}");
        }
    }
}

/// KOB-352 : un lien symbolique local qui vise un chemin réseau, même par
/// un autre lien ou au milieu du chemin, est refusé sans être suivi ; un lien
/// local qui reste dans la codebase se lit encore.
#[cfg(unix)]
#[test]
fn lien_vers_un_chemin_reseau_refuse_sans_le_suivre() {
    use std::os::unix::fs::symlink;
    let dir = codebase();
    let racine = dir.path();
    symlink("//attaquant/partage/x", racine.join("piege")).unwrap();
    symlink("piege", racine.join("relais")).unwrap();
    symlink("//attaquant/partage", racine.join("dossier")).unwrap();
    symlink("README.md", racine.join("lisez-moi")).unwrap();
    let l = lecteur(&dir);
    for chemin in ["piege", "relais", "dossier/x"] {
        let e = l.lire(&json!({ "file_path": chemin })).unwrap_err();
        assert!(e.contains("chemin réseau"), "{chemin} : {e}");
        let e = l
            .chercher(&json!({ "pattern": "x", "path": chemin }))
            .unwrap_err();
        assert!(e.contains("chemin réseau"), "{chemin} : {e}");
    }
    assert!(l
        .lire(&json!({ "file_path": "lisez-moi" }))
        .unwrap()
        .contains("Kobi"));
    // `..` dans une cible s'applique au chemin résolu, pas au nom du lien :
    // `a/x` mène à `sub/evil`, qui vise le réseau.
    fs::create_dir_all(racine.join("sub/deep")).unwrap();
    symlink("sub/deep", racine.join("a")).unwrap();
    symlink("../evil", racine.join("sub/deep/x")).unwrap();
    symlink("//attaquant/p", racine.join("sub/evil")).unwrap();
    let e = l.lire(&json!({ "file_path": "a/x" })).unwrap_err();
    assert!(e.contains("chemin réseau"), "{e}");
    // Un lien absolu local, dans la codebase : se lit.
    symlink(racine.join("README.md"), racine.join("absolu")).unwrap();
    assert!(l
        .lire(&json!({ "file_path": "absolu" }))
        .unwrap()
        .contains("Kobi"));
    // Une boucle de liens : refusée par le compte des liens, sans tourner
    // sans fin.
    symlink("boucle-b", racine.join("boucle-a")).unwrap();
    symlink("boucle-a", racine.join("boucle-b")).unwrap();
    let e = l.lire(&json!({ "file_path": "boucle-a" })).unwrap_err();
    assert!(e.contains("chemin réseau"), "{e}");
    // `a -> .` répété : un seul passage, borné par le compte des liens.
    symlink(".", racine.join("ici")).unwrap();
    let long = vec!["ici"; 30].join("/") + "/README.md";
    assert!(l
        .lire(&json!({ "file_path": long }))
        .unwrap()
        .contains("Kobi"));
}

/// KOB-353 : les noms de périphériques DOS sont reconnus avec extension,
/// flux, casse, exposant ou espaces de fin ; les noms voisins passent.
#[test]
fn noms_de_peripheriques_dos() {
    use std::ffi::OsStr;
    for nom in [
        "COM1",
        "com0",
        "nul",
        "CON.txt",
        "aux .log",
        "lpt9.tar.gz",
        "COM¹",
        "LPT²",
        "com³",
        "NUL.",
        "prn:flux",
        "CON::$DATA",
        "CONIN$",
        "CONOUT$",
    ] {
        assert!(nom_de_peripherique(OsStr::new(nom)), "{nom}");
    }
    for nom in [
        "COM10",
        "COM",
        "COM1 x.txt",
        "console.rs",
        "nul_part.md",
        ".con",
        "lpt",
        "COMA",
        "main.rs",
    ] {
        assert!(!nom_de_peripherique(OsStr::new(nom)), "{nom}");
    }
    assert!(chemin_verbatim(r"\\?\C:\code"));
    assert!(!chemin_verbatim("//?/C:/code"));
    assert!(!chemin_verbatim(r"C:\code"));
    assert!(!chemin_verbatim("/code"));
}

/// KOB-353 : le parcours (actif sous Windows) refuse un nom de périphérique
/// dans le chemin, au milieu comme à la fin, et dans la cible relative d'un
/// lien ; sans le jugement des périphériques (ailleurs), rien ne change.
#[cfg(unix)]
#[test]
fn parcours_refuse_les_noms_de_peripheriques() {
    let dir = codebase();
    let racine = dir.path();
    fs::create_dir_all(racine.join("outils")).unwrap();
    std::os::unix::fs::symlink("COM1", racine.join("outils/port")).unwrap();
    std::os::unix::fs::symlink("../sous/AUX/x", racine.join("outils/relais")).unwrap();
    // KOB-367 : après un lien à cible absolue (une jonction sous Windows,
    // rendue en `\\?\…`), la suite du chemin est encore jugée.
    fs::create_dir_all(racine.join("vendor")).unwrap();
    std::os::unix::fs::symlink(racine.join("vendor"), racine.join("lib")).unwrap();
    for chemin in [
        "outils/port",
        "outils/relais",
        "NUL.txt",
        "code/nul/x.rs",
        "lib/COM1",
        "lib/CON.txt",
    ] {
        let chemin = racine.join(chemin);
        assert!(lien_vers_le_reseau(&chemin, true), "{}", chemin.display());
        assert!(!lien_vers_le_reseau(&chemin, false), "{}", chemin.display());
    }
    assert!(!lien_vers_le_reseau(&racine.join("src/main.rs"), true));
}

/// KOB-352 : `.` et `..` se retirent sur le texte, sans remonter au-dessus
/// de la racine, verbatim compris (utilisé sous Windows).
#[test]
fn reduction_sur_le_texte() {
    assert_eq!(
        reduire_sur_le_texte(Path::new("/base/a/../evil")),
        Path::new("/base/evil")
    );
    assert_eq!(
        reduire_sur_le_texte(Path::new("/base/./a/./b")),
        Path::new("/base/a/b")
    );
    assert_eq!(reduire_sur_le_texte(Path::new("/../../x")), Path::new("/x"));
}

/// KOB-352, sous Windows : `a\..\evil` (ou `a/../evil`) se réduit sur le
/// texte en `evil`, base verbatim comprise
/// (comme le fait `canonicalize`), dont le lien vise le réseau : refusé ;
/// `a\x`, dont le `..` de la cible est physique, aussi. La CI windows-latest
/// peut créer des liens.
#[cfg(windows)]
#[test]
fn lien_vers_un_chemin_reseau_refuse_sous_windows() {
    // La base est verbatim : la réduction ne doit pas s'y arrêter.
    assert_eq!(
        reduire_sur_le_texte(Path::new(r"\\?\C:\base\a\..\.\evil")),
        Path::new(r"\\?\C:\base\evil")
    );
    assert_eq!(
        reduire_sur_le_texte(Path::new(r"\\?\C:\..\x")),
        Path::new(r"\\?\C:\x")
    );
    use std::os::windows::fs::{symlink_dir, symlink_file};
    let dir = codebase();
    let racine = dir.path();
    fs::create_dir_all(racine.join(r"sub\deep")).unwrap();
    if symlink_dir(r"sub\deep", racine.join("a")).is_err() {
        // Pas le droit de créer des liens sur ce poste : rien à vérifier,
        // sauf en CI, qui doit pouvoir le faire.
        assert!(std::env::var_os("CI").is_none(), "liens impossibles en CI");
        return;
    }
    symlink_file(r"\\attaquant\p", racine.join("evil")).unwrap();
    symlink_file(r"..\evil2", racine.join(r"sub\deep\x")).unwrap();
    symlink_file(r"\\attaquant\q", racine.join(r"sub\evil2")).unwrap();
    let l = lecteur(&dir);
    for chemin in [r"a\..\evil", "a/../evil", r"a\x", "evil"] {
        let e = l.lire(&json!({ "file_path": chemin })).unwrap_err();
        assert!(e.contains("chemin réseau"), "{chemin} : {e}");
    }
    // KOB-367 : après un lien à cible absolue, la suite du chemin donné (non
    // verbatim, que Win32 traduit) est encore jugée, quelle que soit la
    // forme rendue par `read_link`. Selon le poste, une cible donnée en
    // `C:\…` est rendue telle quelle ou en `\\?\C:\…` : les deux formes sont
    // créées ici (`lib`, puis `lib2` en verbatim explicite, le cas de
    // KOB-367). Donné verbatim, Win32 ne traduit rien : permis (KOB-398).
    fs::create_dir_all(racine.join("vendor")).unwrap();
    let vendor = racine.join("vendor");
    symlink_dir(&vendor, racine.join("lib")).unwrap();
    // Une cible de plus de 260 caractères ne peut pas être rendue sans le
    // préfixe : `lib2` reproduit KOB-367, sans nom ambigu dans la cible.
    let long = vendor.join("a".repeat(120)).join("b".repeat(120));
    let long = PathBuf::from(format!(r"\\?\{}", long.display()));
    fs::create_dir_all(&long).unwrap();
    symlink_dir(&long, racine.join("lib2")).unwrap();
    assert!(
        std::fs::read_link(racine.join("lib2"))
            .unwrap()
            .to_string_lossy()
            .starts_with(r"\\?\"),
        "cible verbatim relue sans son préfixe"
    );
    for lien in ["lib", "lib2"] {
        for nom in ["COM1", "CON.txt", "nul"] {
            let chemin = racine.join(lien).join(nom);
            assert!(lien_vers_le_reseau(&chemin, true), "{}", chemin.display());
        }
        let verbatim = PathBuf::from(format!(r"\\?\{}", racine.join(lien).join("COM1").display()));
        assert!(
            !lien_vers_le_reseau(&verbatim, true),
            "{}",
            verbatim.display()
        );
    }
}

/// KOB-398, correctif de sécurité du confinement Windows : le chemin jugé
/// est celui que NT ouvrira.
/// * A : chemin donné verbatim `\\?\…\lib\x.`, où `lib` vise `C:\…\vendor`
///   (cible rendue sans `\\?\`) et `vendor\x.` est un lien vers le réseau.
///   La sonde voyait `vendor\x` (Win32 retire le point) : lien manqué.
/// * B : chemin donné `C:\…\lib3\x.`, où `lib3` vise `\\?\…\d.` et `d.\x`
///   est un lien vers le réseau. Win32 ouvre `lib3\x`, la sonde voyait
///   `d.\x.` : lien manqué.
///
/// Variantes : espace final, flux NTFS (`::$DATA`), casse, nom court 8.3.
#[cfg(windows)]
#[test]
fn chemins_que_win32_traduit_juges_tels_que_nt_les_ouvre() {
    use std::os::windows::fs::{symlink_dir, symlink_file};
    let dir = codebase();
    let racine = dir.path().to_path_buf();
    let v = |p: &Path| PathBuf::from(format!(r"\\?\{}", p.display()));
    let vendor = racine.join("vendor");
    fs::create_dir_all(&vendor).unwrap();
    if symlink_dir(&vendor, racine.join("lib")).is_err() {
        assert!(std::env::var_os("CI").is_none(), "liens impossibles en CI");
        return;
    }
    // A
    symlink_file(r"\\attaquant\p", v(&vendor.join("x."))).unwrap();
    let a = v(&racine.join(r"lib\x."));
    assert!(lien_vers_le_reseau(&a, true), "A : {}", a.display());
    // B
    let d = PathBuf::from(format!("{}.", racine.join("d").display()));
    fs::create_dir_all(v(&d)).unwrap();
    symlink_file(r"\\attaquant\q", v(&d.join("x"))).unwrap();
    symlink_dir(v(&d), racine.join("lib3")).unwrap();
    for nom in [r"lib3\x.", r"lib3\x", r"lib3\x ", r"lib3\x. .", r"LIB3\X."] {
        let chemin = racine.join(nom);
        assert!(
            lien_vers_le_reseau(&chemin, true),
            "B : {}",
            chemin.display()
        );
    }
    // Par la lecture : B en absolu non verbatim (relatif, il est joint à la
    // base verbatim et NT ouvre `x.` tel quel, absent) ; A en relatif. Un
    // lecteur sans racine (`C:evil`) est refusé avant toute sonde.
    let l = lecteur(&dir);
    let b_absolu = |nom: &str| racine.join(nom).display().to_string();
    for chemin in [
        b_absolu(r"lib3\x."),
        b_absolu(r"lib3\x "),
        r"lib\x.".to_string(),
        "C:evil".to_string(),
        r"C:lib\x.".to_string(),
    ] {
        let e = l.lire(&json!({ "file_path": chemin })).unwrap_err();
        assert!(e.contains("chemin réseau"), "{chemin} : {e}");
    }
    // Flux NTFS : `y::$DATA` ouvre `y`, un lien vers le réseau.
    symlink_file(r"\\attaquant\r", vendor.join("y")).unwrap();
    for nom in [r"lib\y::$DATA", r"lib\y:flux"] {
        assert!(lien_vers_le_reseau(&racine.join(nom), true), "{nom}");
        let verbatim = v(&racine.join(nom));
        assert!(
            lien_vers_le_reseau(&verbatim, true),
            "{}",
            verbatim.display()
        );
    }
    // Nom court 8.3 d'un dossier qui mène au lien : même objet pour NT.
    let long = racine.join("dossier-au-nom-long");
    fs::create_dir_all(&long).unwrap();
    symlink_file(r"\\attaquant\s", long.join("z")).unwrap();
    let court = racine.join(r"DOSSIE~1\z");
    if fs::symlink_metadata(&court).is_ok() {
        assert!(lien_vers_le_reseau(&court, true), "{}", court.display());
    }
    // Un fichier ordinaire, dans les mêmes dossiers, se lit toujours.
    fs::write(vendor.join("ordinaire.txt"), "ok").unwrap();
    assert!(!lien_vers_le_reseau(
        &racine.join(r"lib\ordinaire.txt"),
        true
    ));
    assert!(!lien_vers_le_reseau(
        &v(&racine.join(r"lib\ordinaire.txt")),
        true
    ));
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

/// KOB-345 : une ligne trouvée trop longue (fichier minifié, JSON d'une
/// ligne) est coupée, sur une frontière de caractère, et la sortie reste
/// bornée.
#[test]
fn grep_coupe_les_lignes_trop_longues() {
    let dir = tempfile::tempdir().unwrap();
    let longue = format!("début {} fin", "é".repeat(400_000));
    for i in 0..5 {
        fs::write(dir.path().join(format!("min{i}.js")), &longue).unwrap();
    }
    let l = lecteur(&dir);
    let sortie = l.chercher(&json!({ "pattern": "début" })).unwrap();
    assert!(sortie.len() <= TAILLE_MAX + 200, "{}", sortie.len());
    assert_eq!(
        sortie.matches("(ligne coupée)").count(),
        5,
        "{}",
        &sortie[..300]
    );
    assert!(!sortie.contains("fin\n"));
    assert!(!sortie.contains('\u{FFFD}'));
    // 1500 lignes de 1000 octets : arrêt sur la taille, pas sur le nombre.
    let dir = tempfile::tempdir().unwrap();
    let lignes = format!("{}\n", "x".repeat(1000)).repeat(1500);
    fs::write(dir.path().join("gros.txt"), lignes).unwrap();
    let sortie = lecteur(&dir).chercher(&json!({ "pattern": "x" })).unwrap();
    assert!(sortie.contains("résultat trop long"), "{}", sortie.len());
    // Une ligne courte, ou d'exactement LIGNE_MAX octets, reste entière ;
    // un caractère de 4 octets n'est jamais coupé.
    assert_eq!(ligne_bornee("court"), "court");
    let pile = "x".repeat(LIGNE_MAX);
    assert_eq!(ligne_bornee(&pile), pile.as_str());
    let emojis = format!("x{}", "🦀".repeat(LIGNE_MAX));
    let coupee = ligne_bornee(&emojis);
    assert!(coupee.ends_with(" … (ligne coupée)"), "{coupee}");
    assert!(coupee.len() <= LIGNE_MAX + " … (ligne coupée)".len());
}

/// KOB-346 : un dossier ignoré se parcourt quand il est le départ demandé,
/// ou une codebase déclarée ; ses sous-dossiers ignorés restent ignorés.
#[test]
fn depart_nomme_comme_un_dossier_ignore_parcouru() {
    let dir = codebase();
    let l = lecteur(&dir);
    let target = dir.path().join("target");
    // Sous le départ, un dossier ignoré reste ignoré.
    fs::create_dir_all(target.join("node_modules")).unwrap();
    fs::write(target.join("node_modules/cache.rs"), "// TODO\n").unwrap();
    let r = l
        .chercher(&json!({ "pattern": "TODO", "path": target }))
        .unwrap();
    assert!(r.contains("genere.rs") && !r.contains("cache.rs"), "{r}");
    let r = l
        .lister(&json!({ "pattern": "**/*.rs", "path": target }))
        .unwrap();
    assert!(r.contains("genere.rs") && !r.contains("cache.rs"), "{r}");
    // Une codebase déclarée nommée `target`.
    let declaree = Lecteur::new(std::slice::from_ref(&target));
    let r = declaree.chercher(&json!({ "pattern": "TODO" })).unwrap();
    assert!(r.contains("genere.rs") && !r.contains("cache.rs"), "{r}");
    // Depuis la racine, `target` reste ignoré.
    let r = l.chercher(&json!({ "pattern": "TODO" })).unwrap();
    assert!(!r.contains("genere.rs"), "{r}");
}

/// KOB-357 : `/proj` et `/proj/target` déclarés tous les deux : sans
/// `path`, Grep et Glob parcourent aussi `target`, une seule fois. Un
/// dossier déclaré sous un dossier non ignoré reste absorbé.
#[test]
fn dossier_declare_sous_un_nom_ignore_parcouru() {
    let dir = codebase();
    let racine = dir.path().to_path_buf();
    let target = racine.join("target");
    let l = Lecteur::new(&[racine.clone(), target.clone(), racine.join("src")]);
    assert_eq!(
        l.dossiers,
        vec![
            racine.canonicalize().unwrap(),
            target.canonicalize().unwrap()
        ]
    );
    let r = l.chercher(&json!({ "pattern": "TODO" })).unwrap();
    assert_eq!(r.matches("genere.rs").count(), 1, "{r}");
    assert!(r.contains("README.md"), "{r}");
    let r = l.lister(&json!({ "pattern": "**/*.rs" })).unwrap();
    assert_eq!(r.matches("genere.rs").count(), 1, "{r}");
    assert_eq!(r.matches("main.rs").count(), 1, "{r}");
    // Sous `target` déclaré, un dossier non ignoré est absorbé, un
    // `node_modules` déclaré reste à part.
    let debug = target.join("debug");
    let modules = target.join("node_modules");
    fs::create_dir_all(&modules).unwrap();
    let l = Lecteur::new(&[target.clone(), debug.clone(), modules.clone()]);
    assert_eq!(
        l.dossiers,
        vec![
            target.canonicalize().unwrap(),
            modules.canonicalize().unwrap()
        ]
    );
    // Chaque composant compte, pas seulement le nom final ni le premier :
    // `target/debug` et `src/node_modules` ne sont pas atteints depuis la
    // racine.
    let profond = racine.join("src/node_modules");
    fs::create_dir_all(&profond).unwrap();
    let l = Lecteur::new(&[racine.clone(), debug.clone(), profond.clone()]);
    assert_eq!(l.dossiers.len(), 3, "{:?}", l.dossiers);
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
/// ligne à ligne et le dit ; une ligne géante de zéros, binaire, est refusée
/// sans être chargée (KOB-347) ; Grep ignore le fichier.
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
    // La ligne 10 001 fait près de 60 Mo de zéros : binaire, même passée
    // en lignes à sauter.
    for offset in [10_001, 20_000] {
        let e = l
            .lire(&json!({ "file_path": gros, "offset": offset, "limit": 1 }))
            .unwrap_err();
        assert!(e.contains("pas un fichier texte"), "{e}");
    }
    assert_eq!(
        l.chercher(&json!({ "pattern": "premiere" })).unwrap(),
        "Aucun résultat."
    );
}

/// KOB-347 : un octet NUL après le premier tampon est vu ; une ligne trop
/// longue est coupée sur une frontière de caractère, et le dit ; une ligne
/// d'exactement `LIGNE_MAX` octets reste entière.
#[test]
fn read_binaire_tardif_et_ligne_coupee() {
    let dir = codebase();
    let l = lecteur(&dir);
    let tardif = dir.path().join("journal.log");
    let mut contenu = "texte\n".repeat(1500).into_bytes();
    contenu.extend_from_slice(b"donnee\0\x01\n");
    fs::write(&tardif, contenu).unwrap();
    let e = l.lire(&json!({ "file_path": tardif })).unwrap_err();
    assert!(e.contains("pas un fichier texte"), "{e}");
    // Ce qui précède reste lisible.
    assert_eq!(
        l.lire(&json!({ "file_path": tardif, "limit": 1 })).unwrap(),
        "1\ttexte\n"
    );

    let longue = dir.path().join("config.json");
    // 5001 octets : la coupe à 2000 tombe au milieu d'un « é ».
    let ligne = format!("x{}", "é".repeat(2500));
    let pile = "y".repeat(LIGNE_MAX);
    fs::write(&longue, format!("{ligne}\n{pile}\r\nfin\n")).unwrap();
    assert_eq!(
        l.lire(&json!({ "file_path": longue })).unwrap(),
        format!(
            "1\tx{} … (ligne coupée)\n2\t{pile}\n3\tfin\n",
            "é".repeat(999)
        )
    );

    // Sur plusieurs tampons de 8 Kio : le `\n` seul au début du second ne
    // fait pas oublier la coupe, une ligne de 20 Ko non plus.
    let etalee = dir.path().join("minifie.js");
    let (a, b) = ("a".repeat(8192), "b".repeat(20_000));
    fs::write(&etalee, format!("{a}\n{b}\nfin\n")).unwrap();
    assert_eq!(
        l.lire(&json!({ "file_path": etalee })).unwrap(),
        format!(
            "1\t{} … (ligne coupée)\n2\t{} … (ligne coupée)\n3\tfin\n",
            &a[..LIGNE_MAX],
            &b[..LIGNE_MAX]
        )
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
    assert!(
        r.ends_with(&format!("arrêté à {RESULTATS_MAX} lignes")),
        "{r}"
    );
    let r = l.lister(&json!({ "pattern": "f*.txt" })).unwrap();
    assert_eq!(r.lines().count(), RESULTATS_MAX + 1);
}

// ----- Course entre vérification et ouverture, bornes (KOB-335) -----

/// Un dossier intermédiaire remplacé par un lien vers l'extérieur entre la
/// vérification du chemin et son ouverture : refusé (vrai chemin du
/// descripteur, `/proc/self/fd`).
#[cfg(target_os = "linux")]
#[test]
fn course_dossier_remplace_par_un_lien() {
    let (dedans, dehors, _) = deux_dossiers();
    fs::create_dir(dedans.path().join("sous")).unwrap();
    fs::write(dedans.path().join("sous/note.txt"), "dedans").unwrap();
    fs::write(dehors.path().join("note.txt"), "secret").unwrap();
    let l = lecteur(&dedans);
    let verifie = l.permis("sous/note.txt").unwrap();
    // L'attaquant échange le dossier contre un lien.
    fs::rename(dedans.path().join("sous"), dedans.path().join("ancien")).unwrap();
    std::os::unix::fs::symlink(dehors.path(), dedans.path().join("sous")).unwrap();
    let e = l.ouvrir(&verifie).unwrap_err();
    assert!(e.contains("hors des dossiers permis"), "{e}");
}

/// Le fichier lui-même remplacé par un lien, même vers un fichier des
/// dossiers permis : refusé (le lien n'est pas suivi, `O_NOFOLLOW`).
#[cfg(unix)]
#[test]
fn course_fichier_remplace_par_un_lien() {
    let dedans = codebase();
    let l = lecteur(&dedans);
    let verifie = l.permis("README.md").unwrap();
    fs::remove_file(&verifie).unwrap();
    std::os::unix::fs::symlink(dedans.path().join("src/main.rs"), &verifie).unwrap();
    let e = l.ouvrir(&verifie).unwrap_err();
    // Sous Linux, le lien est désigné sans être suivi (`O_PATH`) et refusé
    // pour son type ; ailleurs, `O_NOFOLLOW` fait échouer l'ouverture.
    assert!(
        e.contains("pas un fichier ordinaire") || e.contains("symbolic links"),
        "{e}"
    );
}

/// Grep borné même quand le filtre écarte tout, et le parcours compte les
/// dossiers (relecture de KOB-335).
#[test]
fn bornes_avant_le_filtre_et_sur_les_dossiers() {
    let dedans = codebase();
    for i in 0..30 {
        fs::create_dir_all(dedans.path().join(format!("vide{i:02}/a/b"))).unwrap();
    }
    let mut l = lecteur(&dedans);
    l.bornes.entrees = 20;
    let r = l
        .chercher(&json!({ "pattern": "x", "glob": "*.zzz" }))
        .unwrap();
    assert!(r.contains("parcours arrêté après 20 entrées"), "{r}");
    let r = l.lister(&json!({ "pattern": "*.zzz" })).unwrap();
    assert!(r.contains("parcours arrêté après 20 entrées"), "{r}");
    // Durée : une borne nulle arrête tout de suite.
    let mut l = lecteur(&dedans);
    l.bornes.duree = std::time::Duration::ZERO;
    let r = l.lister(&json!({ "pattern": "*" })).unwrap();
    assert!(r.contains("parcours arrêté"), "{r}");
}

/// Dossiers permis imbriqués (mémoire sous une codebase) : rien n'est
/// compté ni montré deux fois.
#[test]
fn dossiers_imbriques_comptes_une_fois() {
    let dedans = codebase();
    let l = Lecteur::new(&[dedans.path().to_path_buf(), dedans.path().join("src")]);
    assert_eq!(l.dossiers.len(), 1);
    // Le premier dossier déclaré reste la base des chemins relatifs, même
    // contenu dans un autre.
    let l2 = Lecteur::new(&[dedans.path().join("src"), dedans.path().to_path_buf()]);
    assert!(l2
        .lire(&json!({ "file_path": "main.rs" }))
        .unwrap()
        .contains("fn main"));
    let r = l.lister(&json!({ "pattern": "**/*.rs" })).unwrap();
    assert_eq!(r.lines().count(), 1, "{r}");
}

/// Le fichier remplacé par un tube nommé : refusé tout de suite, sans
/// bloquer (`O_NONBLOCK`, type vérifié sur le descripteur).
#[cfg(unix)]
#[test]
fn course_fichier_remplace_par_un_tube() {
    let dedans = codebase();
    let l = lecteur(&dedans);
    let verifie = l.permis("README.md").unwrap();
    fs::remove_file(&verifie).unwrap();
    assert!(std::process::Command::new("mkfifo")
        .arg(&verifie)
        .status()
        .unwrap()
        .success());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(l.ouvrir(&verifie).map(|_| ()));
    });
    let r = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("l'ouverture d'un tube a bloqué");
    let e = r.unwrap_err();
    assert!(e.contains("pas un fichier ordinaire"), "{e}");
}

/// Grep et Glob bornés en fichiers lus et en entrées parcourues ; Glob ne
/// garde que les premiers chemins, dans l'ordre.
#[test]
fn parcours_bornes() {
    let dedans = codebase();
    for i in 0..40 {
        fs::write(dedans.path().join(format!("f{i:02}.txt")), "cible\n").unwrap();
    }
    let mut l = lecteur(&dedans);
    l.bornes.fichiers = 5;
    let r = l.chercher(&json!({ "pattern": "cible" })).unwrap();
    assert!(r.contains("recherche arrêtée après 5 fichiers"), "{r}");
    assert!(
        r.lines().filter(|x| x.contains(":1:cible")).count() <= 5,
        "{r}"
    );
    l.bornes.entrees = 10;
    let r = l.lister(&json!({ "pattern": "*.txt" })).unwrap();
    assert!(r.contains("parcours arrêté après 10 entrées"), "{r}");
    // Sans borne atteinte : les RESULTATS_MAX premiers chemins triés.
    for i in 40..400 {
        fs::write(dedans.path().join(format!("f{i:03}.txt")), "x").unwrap();
    }
    let l = lecteur(&dedans);
    let r = l.lister(&json!({ "pattern": "f*.txt" })).unwrap();
    let lignes: Vec<&str> = r.lines().collect();
    assert_eq!(lignes.len(), RESULTATS_MAX + 1);
    let mut tous: Vec<String> = fs::read_dir(dedans.path().canonicalize().unwrap())
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with('f'))
        .map(|p| p.display().to_string())
        .collect();
    tous.sort();
    assert_eq!(&lignes[..RESULTATS_MAX], &tous[..RESULTATS_MAX]);
    assert!(
        lignes[RESULTATS_MAX].contains("400 fichiers"),
        "{}",
        lignes[RESULTATS_MAX]
    );
}

/// SONDE de revue de sécurité (kobi-a, PR brouillon, jamais mergée) : rend
/// ce que Kobi juge et ce que Windows fait, pour chaque variante.
#[cfg(windows)]
#[test]
fn sonde_revue_confinement() {
    use std::fmt::Write;
    use std::os::windows::fs::{symlink_dir, symlink_file};
    let dir = codebase();
    let racine = dir.path().to_path_buf();
    let l = lecteur(&dir);
    let mut r = String::new();
    let essai = |r: &mut String, nom: &str, chemin: &Path| {
        let t = std::time::Instant::now();
        let m = std::fs::metadata(chemin);
        let duree = t.elapsed();
        let os = m.as_ref().err().map(|e| (e.raw_os_error(), e.to_string()));
        let lu = l
            .lire(&json!({ "file_path": chemin.display().to_string() }))
            .map(|_| ())
            .map_err(|e| e.chars().take(90).collect::<String>());
        let _ = writeln!(
            r,
            "{nom} | kobi_reseau={} | lire={lu:?} | metadata={os:?} en {duree:?}",
            lien_vers_le_reseau(chemin, true)
        );
    };
    let profond = racine.join(r"a\b\c");
    fs::create_dir_all(&profond).unwrap();
    let monte = "..\\".repeat(30);
    // 1. Lien relatif qui monte au-dessus du lecteur vers GLOBALROOT.
    let c = format!(r"{monte}GLOBALROOT\Device\Mup\127.0.0.1\partage\f");
    let _ = writeln!(
        r,
        "creation g : {:?}",
        symlink_file(&c, profond.join("g")).err()
    );
    essai(&mut r, "1 g", &profond.join("g"));
    // 2. Même chose vers ??\UNC.
    let c = format!(r"{monte}??\UNC\127.0.0.1\partage\f");
    let _ = writeln!(
        r,
        "creation u : {:?}",
        symlink_file(&c, profond.join("u")).err()
    );
    essai(&mut r, "2 u", &profond.join("u"));
    // 3. Lien de dossier qui monte, puis la suite du chemin.
    let _ = writeln!(
        r,
        "creation haut : {:?}",
        symlink_dir(&monte, profond.join("haut")).err()
    );
    essai(
        &mut r,
        "3 haut",
        &profond.join(r"haut\GLOBALROOT\Device\Mup\127.0.0.1\partage\f"),
    );
    essai(&mut r, "3b haut seul", &profond.join("haut"));
    // 4. Nom court 8.3 d'un lien vers le réseau.
    let _ = writeln!(
        r,
        "creation long : {:?}",
        symlink_file(r"\\127.0.0.1\partage\f", racine.join("longnomdelien.txt")).err()
    );
    let liste = std::process::Command::new("cmd")
        .args(["/c", "dir", "/x"])
        .current_dir(&racine)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let court = liste
        .lines()
        .find(|ligne| ligne.contains("longnomdelien"))
        .and_then(|ligne| {
            ligne
                .split_whitespace()
                .find(|m| m.contains('~'))
                .map(str::to_string)
        });
    let _ = writeln!(r, "nom court : {court:?}");
    if let Some(court) = court {
        essai(&mut r, "4 court", &racine.join(court));
    }
    essai(&mut r, "4b long", &racine.join("longnomdelien.txt"));
    // 5. Jonction vers UNC (mklink /J).
    let o = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(racine.join("jonction"))
        .arg(r"\\127.0.0.1\partage")
        .output();
    let _ = writeln!(
        r,
        "mklink /J unc : {:?}",
        o.map(|o| (
            o.status.code(),
            String::from_utf8_lossy(&o.stdout).trim().to_string(),
            String::from_utf8_lossy(&o.stderr).trim().to_string()
        ))
    );
    essai(&mut r, "5 jonction", &racine.join(r"jonction\f"));
    let _ = writeln!(
        r,
        "read_link jonction : {:?}",
        fs::read_link(racine.join("jonction"))
    );
    // 6. Jonction vers un dossier local hors de la codebase.
    let dehors = tempfile::tempdir().unwrap();
    fs::write(dehors.path().join("secret.txt"), "secret").unwrap();
    let o = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(racine.join("jdehors"))
        .arg(dehors.path())
        .output();
    let _ = writeln!(r, "mklink /J dehors : {:?}", o.map(|o| o.status.code()));
    essai(&mut r, "6 jdehors", &racine.join(r"jdehors\secret.txt"));
    // 7. Verbatim qui monte au-dessus du lecteur.
    let v = format!(
        r"\\?\{}\{monte}GLOBALROOT\Device\Mup\127.0.0.1\partage\f",
        racine.display()
    );
    essai(&mut r, "7 verbatim monte", Path::new(&v));
    panic!("SONDE\n{r}");
}
