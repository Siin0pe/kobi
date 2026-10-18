//! Commandes en lecture seule (KOB-154) : Kobi décide lui-même, avant toute
//! exécution, si une ligne de commande ne fait que lire. Indépendant du
//! harness : on reçoit la ligne et l'interpréteur, pas un outil de Claude.
//!
//! * Lecture : chaque commande de la ligne est sur la liste blanche, sans
//!   option qui écrit, sans redirection vers un fichier, sans substitution,
//!   et ne vise que les dossiers de code déclarés.
//! * Écriture (suppression à part) : tout le reste ; elle attend une
//!   validation (KOB-156), et d'ici là elle est refusée avec la commande à
//!   copier.
//! * Refus : la ligne vise un chemin hors des dossiers déclarés.
//!
//! On préfère refuser à tort que laisser passer : ce qu'on ne sait pas
//! analyser (variables, sous-shells, blocs PowerShell) est une écriture.

use std::path::{Component, Path, PathBuf};

/// Interpréteur de la ligne.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interprete {
    Bash,
    PowerShell,
}

/// Ce que fait une ligne de commande.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Lecture,
    /// Modifie quelque chose ; `suppression` si elle efface des fichiers.
    Ecriture {
        suppression: bool,
        raison: String,
    },
    /// Hors des dossiers déclarés.
    Refus(String),
}

fn ecriture(raison: impl Into<String>) -> Verdict {
    Verdict::Ecriture {
        suppression: false,
        raison: raison.into(),
    }
}

fn suppression(raison: impl Into<String>) -> Verdict {
    Verdict::Ecriture {
        suppression: true,
        raison: raison.into(),
    }
}

/// Un morceau de la ligne après découpage.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Jeton {
    Mot(String),
    /// `|`, `&&`, `||`, `;` : enchaîne deux commandes.
    Enchainement,
}

/// Découpe une ligne Bash en mots et enchaînements. Refuse (Err) ce qui
/// sort de la lecture simple : redirections, substitutions, variables,
/// arrière-plan, sous-shells.
fn decouper_bash(ligne: &str) -> Result<Vec<Jeton>, String> {
    let mut jetons = Vec::new();
    let mut mot = String::new();
    let mut dans_mot = false;
    let mut c = ligne.chars().peekable();
    let fin_mot = |jetons: &mut Vec<Jeton>, mot: &mut String, dans_mot: &mut bool| {
        if *dans_mot {
            jetons.push(Jeton::Mot(std::mem::take(mot)));
            *dans_mot = false;
        }
    };
    while let Some(ch) = c.next() {
        match ch {
            '\'' => {
                dans_mot = true;
                loop {
                    match c.next() {
                        Some('\'') => break,
                        Some(x) => mot.push(x),
                        None => return Err("guillemet non fermé".into()),
                    }
                }
            }
            '"' => {
                dans_mot = true;
                loop {
                    match c.next() {
                        Some('"') => break,
                        Some('\\') => match c.next() {
                            Some(x) => mot.push(x),
                            None => return Err("guillemet non fermé".into()),
                        },
                        Some('$') | Some('`') => {
                            return Err("substitution ou variable entre guillemets".into())
                        }
                        Some(x) => mot.push(x),
                        None => return Err("guillemet non fermé".into()),
                    }
                }
            }
            '\\' => {
                dans_mot = true;
                match c.next() {
                    Some('\n') => {}
                    Some(x) => mot.push(x),
                    None => return Err("barre oblique inverse finale".into()),
                }
            }
            '$' => return Err("variable ou substitution `$`".into()),
            '`' => return Err("substitution entre accents graves".into()),
            '(' | ')' => return Err("sous-shell ou substitution de processus".into()),
            '{' | '}' if !dans_mot => return Err("bloc entre accolades".into()),
            '<' | '>' => {
                // `2>/dev/null` et `2>&1` sont permis : ils n'écrivent rien.
                let reste: String = c.clone().collect();
                if ch == '>' && mot == "2" && dans_mot {
                    if let Some(apres) = reste.strip_prefix("/dev/null") {
                        if apres.is_empty()
                            || apres.starts_with(char::is_whitespace)
                            || apres.starts_with(['|', ';', '&'])
                        {
                            mot.clear();
                            dans_mot = false;
                            for _ in 0.."/dev/null".len() {
                                c.next();
                            }
                            continue;
                        }
                    }
                    if reste.starts_with("&1") {
                        mot.clear();
                        dans_mot = false;
                        c.next();
                        c.next();
                        continue;
                    }
                }
                return Err(format!("redirection `{ch}`"));
            }
            '|' => {
                fin_mot(&mut jetons, &mut mot, &mut dans_mot);
                if c.peek() == Some(&'|') {
                    c.next();
                } else if c.peek() == Some(&'&') {
                    return Err("redirection `|&`".into());
                }
                jetons.push(Jeton::Enchainement);
            }
            '&' => {
                fin_mot(&mut jetons, &mut mot, &mut dans_mot);
                if c.peek() == Some(&'&') {
                    c.next();
                    jetons.push(Jeton::Enchainement);
                } else {
                    return Err("commande en arrière-plan `&`".into());
                }
            }
            ';' | '\n' => {
                fin_mot(&mut jetons, &mut mot, &mut dans_mot);
                jetons.push(Jeton::Enchainement);
            }
            ch if ch.is_whitespace() => fin_mot(&mut jetons, &mut mot, &mut dans_mot),
            ch => {
                dans_mot = true;
                mot.push(ch);
            }
        }
    }
    fin_mot(&mut jetons, &mut mot, &mut dans_mot);
    Ok(jetons)
}

/// Découpe une ligne PowerShell. Tout bloc, sous-expression, variable,
/// redirection ou appel indirect est refusé (Err).
fn decouper_powershell(ligne: &str) -> Result<Vec<Jeton>, String> {
    let mut jetons = Vec::new();
    let mut mot = String::new();
    let mut dans_mot = false;
    let mut c = ligne.chars().peekable();
    while let Some(ch) = c.next() {
        match ch {
            '\'' => {
                dans_mot = true;
                loop {
                    match c.next() {
                        Some('\'') if c.peek() == Some(&'\'') => {
                            c.next();
                            mot.push('\'');
                        }
                        Some('\'') => break,
                        Some(x) => mot.push(x),
                        None => return Err("guillemet non fermé".into()),
                    }
                }
            }
            '"' => {
                dans_mot = true;
                loop {
                    match c.next() {
                        Some('"') => break,
                        Some('$') | Some('`') => {
                            return Err("variable ou échappement entre guillemets".into())
                        }
                        Some(x) => mot.push(x),
                        None => return Err("guillemet non fermé".into()),
                    }
                }
            }
            '$' => return Err("variable ou sous-expression `$`".into()),
            '`' => return Err("caractère d'échappement PowerShell".into()),
            '{' | '}' => return Err("bloc de script entre accolades".into()),
            '(' | ')' | '@' => return Err("sous-expression ou tableau".into()),
            '<' | '>' => return Err(format!("redirection `{ch}`")),
            '&' => return Err("opérateur d'appel ou `&&`".into()),
            '|' | ';' | '\n' => {
                if dans_mot {
                    jetons.push(Jeton::Mot(std::mem::take(&mut mot)));
                    dans_mot = false;
                }
                if ch == '|' && c.peek() == Some(&'|') {
                    return Err("opérateur `||`".into());
                }
                jetons.push(Jeton::Enchainement);
            }
            ch if ch.is_whitespace() => {
                if dans_mot {
                    jetons.push(Jeton::Mot(std::mem::take(&mut mot)));
                    dans_mot = false;
                }
            }
            ch => {
                dans_mot = true;
                mot.push(ch);
            }
        }
    }
    if dans_mot {
        jetons.push(Jeton::Mot(mot));
    }
    Ok(jetons)
}

/// Les commandes simples de la ligne (mots de chaque commande).
fn commandes(jetons: Vec<Jeton>) -> Result<Vec<Vec<String>>, String> {
    let mut liste = vec![Vec::new()];
    for j in jetons {
        match j {
            Jeton::Mot(m) => liste.last_mut().expect("jamais vide").push(m),
            Jeton::Enchainement => {
                if liste.last().is_some_and(Vec::is_empty) {
                    return Err("enchaînement sans commande".into());
                }
                liste.push(Vec::new());
            }
        }
    }
    if liste.last().is_some_and(Vec::is_empty) {
        liste.pop();
    }
    if liste.is_empty() {
        return Err("ligne vide".into());
    }
    Ok(liste)
}

/// Résout `.` et `..` sans toucher au disque.
fn normaliser(chemin: &Path) -> PathBuf {
    let mut sortie = PathBuf::new();
    for c in chemin.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                sortie.pop();
            }
            autre => sortie.push(autre.as_os_str()),
        }
    }
    sortie
}

/// Contexte d'une ligne : les dossiers permis et le dossier courant.
struct Lieux<'a> {
    dossiers: &'a [PathBuf],
    courant: PathBuf,
}

impl Lieux<'_> {
    fn contient(&self, chemin: &Path) -> bool {
        let absolu = normaliser(&if chemin.is_absolute() {
            chemin.to_path_buf()
        } else {
            self.courant.join(chemin)
        });
        self.dossiers.iter().any(|d| {
            let d = normaliser(d);
            if cfg!(windows) {
                let bas = |p: &Path| PathBuf::from(p.to_string_lossy().to_lowercase());
                bas(&absolu).starts_with(bas(&d))
            } else {
                absolu.starts_with(&d)
            }
        })
    }

    /// Vérifie un argument qui ressemble à un chemin.
    fn verifier(&self, arg: &str) -> Result<(), String> {
        // L'option `--x=chemin` : on regarde la valeur.
        let valeur = arg.split_once('=').map_or(arg, |(_, v)| v);
        let semble_chemin = valeur.starts_with('/')
            || valeur.starts_with('~')
            || valeur.starts_with('\\')
            || valeur.contains("..")
            || valeur.as_bytes().get(1) == Some(&b':')
                && valeur.as_bytes()[0].is_ascii_alphabetic();
        if !semble_chemin {
            return Ok(());
        }
        if valeur.starts_with('~') {
            return Err(format!(
                "{valeur} : dossier personnel, hors des dossiers de code"
            ));
        }
        if self.contient(Path::new(valeur)) {
            Ok(())
        } else {
            Err(format!("{valeur} est hors des dossiers de code déclarés"))
        }
    }
}

/// Commandes de lecture (Bash), sans sous-commande.
const LECTURE_BASH: &[&str] = &[
    "ls",
    "cat",
    "head",
    "tail",
    "wc",
    "grep",
    "egrep",
    "fgrep",
    "rg",
    "find",
    "tree",
    "pwd",
    "echo",
    "stat",
    "file",
    "du",
    "df",
    "cut",
    "tr",
    "nl",
    "basename",
    "dirname",
    "realpath",
    "readlink",
    "which",
    "type",
    "diff",
    "cmp",
    "md5sum",
    "sha1sum",
    "sha256sum",
    "jq",
    "true",
    "false",
    "date",
    "whoami",
    "uname",
    "hostname",
    "od",
    "hexdump",
    "column",
    "comm",
    "join",
    "paste",
    "fold",
    "rev",
    "tac",
    "expand",
    "unexpand",
    "strings",
    "less",
    "more",
];

/// Commandes qui suppriment.
const SUPPRESSION_BASH: &[&str] = &["rm", "rmdir", "unlink", "shred", "del", "rd"];

/// Options interdites par commande (elles écrivent, exécutent, ou ne
/// rendent jamais la main).
fn options_interdites(cmd: &str) -> &'static [&'static str] {
    match cmd {
        "find" => &[
            "-delete", "-exec", "-execdir", "-ok", "-okdir", "-fprint", "-fprint0", "-fprintf",
            "-fls",
        ],
        "rg" => &["--pre", "--pre-glob", "--search-zip", "-z"],
        "tree" => &["-o"],
        "tail" => &["-f", "-F", "--follow"],
        "sort" => &["-o", "--output"],
        "less" | "more" => &[],
        _ => &[],
    }
}

/// Une commande simple : son verdict.
fn juger_bash(mots: &[String], lieux: &mut Lieux) -> Verdict {
    let Some((cmd, args)) = mots.split_first() else {
        return ecriture("commande vide");
    };
    // `VAR=valeur commande` : la variable peut changer ce qui s'exécute
    // (pager, éditeur…).
    if cmd.contains('=') {
        return ecriture(format!(
            "variable d'environnement `{cmd}` avant la commande"
        ));
    }
    // `/usr/bin/ls` ou `./script` : on ne juge que des noms connus.
    let nom = cmd.as_str();
    if nom.contains('/') || nom.contains('\\') {
        return ecriture(format!("`{nom}` : programme désigné par un chemin"));
    }
    if SUPPRESSION_BASH.contains(&nom) {
        return suppression(format!("`{nom}` supprime des fichiers"));
    }
    if nom == "cd" {
        return match args {
            [] => Verdict::Refus("`cd` seul mène au dossier personnel".into()),
            [d] => match lieux.verifier(d) {
                Err(e) => Verdict::Refus(e),
                Ok(()) if !lieux.contient(Path::new(d)) => {
                    Verdict::Refus(format!("{d} est hors des dossiers de code déclarés"))
                }
                Ok(()) => {
                    lieux.courant = normaliser(&lieux.courant.join(d));
                    Verdict::Lecture
                }
            },
            _ => ecriture("`cd` avec plusieurs arguments"),
        };
    }
    if nom == "git" {
        return juger_git(args, lieux);
    }
    match nom {
        "sed" => return juger_sed(args, lieux),
        "sort" | "uniq"
            if nom == "uniq" && args.iter().filter(|a| !a.starts_with('-')).count() > 1 =>
        {
            return ecriture("`uniq` avec un fichier de sortie")
        }
        "cargo" => {
            return match args.first().map(String::as_str) {
                Some("metadata" | "tree" | "--version" | "-V" | "version") => {
                    verifier_args(&args[1..], lieux)
                }
                _ => ecriture(format!(
                    "`cargo {}` n'est pas une commande de lecture",
                    args.join(" ")
                )),
            }
        }
        "npm" | "pnpm" | "yarn" => {
            return match args.first().map(String::as_str) {
                Some("ls" | "list" | "view" | "info" | "--version" | "-v" | "outdated") => {
                    verifier_args(&args[1..], lieux)
                }
                _ => ecriture(format!(
                    "`{nom} {}` n'est pas une commande de lecture",
                    args.join(" ")
                )),
            }
        }
        "gh" => {
            return match (
                args.first().map(String::as_str),
                args.get(1).map(String::as_str),
            ) {
                (Some("pr"), Some("view" | "list" | "diff" | "checks" | "status"))
                | (Some("issue"), Some("view" | "list" | "status"))
                | (Some("repo"), Some("view"))
                | (Some("run"), Some("list" | "view"))
                | (Some("release"), Some("list" | "view"))
                | (Some("--version"), _) => verifier_args(&args[2.min(args.len())..], lieux),
                _ => ecriture(format!(
                    "`gh {}` n'est pas une commande de lecture",
                    args.join(" ")
                )),
            };
        }
        "rustc" | "node" | "python" | "python3" | "go" | "java" => {
            return match args {
                [v] if v == "--version" || v == "-V" || v == "version" => Verdict::Lecture,
                _ => ecriture(format!("`{nom}` exécute du code")),
            };
        }
        _ => {}
    }
    if nom == "sort" {
        if let Some(o) = args
            .iter()
            .find(|a| *a == "-o" || a.starts_with("--output") || a.starts_with("-o"))
        {
            return ecriture(format!("`sort {o}` écrit un fichier"));
        }
        return verifier_args(args, lieux);
    }
    if !LECTURE_BASH.contains(&nom) {
        return ecriture(format!(
            "`{nom}` n'est pas sur la liste des commandes de lecture"
        ));
    }
    if let Some(o) = args.iter().find(|a| {
        options_interdites(nom)
            .iter()
            .any(|i| *a == i || a.starts_with(&format!("{i}=")))
    }) {
        return if nom == "find" && o == "-delete" {
            suppression("`find -delete` supprime des fichiers")
        } else {
            ecriture(format!("`{nom} {o}` écrit, exécute ou ne rend pas la main"))
        };
    }
    // Le motif de grep et rg n'est pas un chemin.
    let a_verifier = match nom {
        "grep" | "egrep" | "fgrep" | "rg"
            if !args.iter().any(|a| {
                a == "-e" || a == "-f" || a.starts_with("--regexp") || a.starts_with("--file")
            }) =>
        {
            let premier = args.iter().position(|a| !a.starts_with('-'));
            args.iter()
                .enumerate()
                .filter(|(i, _)| Some(*i) != premier)
                .map(|(_, a)| a.clone())
                .collect::<Vec<_>>()
        }
        _ => args.to_vec(),
    };
    verifier_args(&a_verifier, lieux)
}

fn verifier_args(args: &[String], lieux: &Lieux) -> Verdict {
    for a in args {
        if let Err(e) = lieux.verifier(a) {
            return Verdict::Refus(e);
        }
    }
    Verdict::Lecture
}

/// `sed` : seulement l'affichage (`sed -n '1,20p'`), jamais `-i`.
fn juger_sed(args: &[String], lieux: &Lieux) -> Verdict {
    if args
        .iter()
        .any(|a| a == "-i" || a.starts_with("-i") || a.starts_with("--in-place"))
    {
        return ecriture("`sed -i` modifie le fichier");
    }
    let mut script = None;
    let mut fichiers = Vec::new();
    for a in args {
        if a == "-n" || a == "-E" || a == "-r" {
            continue;
        }
        if a.starts_with('-') {
            return ecriture(format!("`sed {a}` : option non prise en charge"));
        }
        if script.is_none() {
            script = Some(a.clone());
        } else {
            fichiers.push(a.clone());
        }
    }
    let Some(script) = script else {
        return ecriture("`sed` sans script");
    };
    // Plages de lignes à afficher : `12p`, `1,20p`, `$p`, `10,$p`.
    let ok = script.split(';').all(|s| {
        let s = s.trim();
        let Some(plage) = s.strip_suffix('p') else {
            return false;
        };
        !plage.is_empty()
            && plage
                .split(',')
                .all(|b| b == "$" || (!b.is_empty() && b.chars().all(|c| c.is_ascii_digit())))
            && plage.split(',').count() <= 2
    });
    if !ok {
        return ecriture(format!(
            "`sed '{script}'` : seul l'affichage de lignes est permis"
        ));
    }
    verifier_args(&fichiers, lieux)
}

/// Sous-commandes de git qui ne font que lire.
const GIT_LECTURE: &[&str] = &[
    "status",
    "log",
    "show",
    "diff",
    "blame",
    "grep",
    "ls-files",
    "ls-tree",
    "rev-parse",
    "describe",
    "shortlog",
    "cat-file",
    "rev-list",
    "name-rev",
    "count-objects",
    "cherry",
    "whatchanged",
    "show-ref",
    "for-each-ref",
    "merge-base",
    "annotate",
    "version",
];

fn juger_git(args: &[String], lieux: &mut Lieux) -> Verdict {
    let mut i = 0;
    // Options globales.
    while let Some(a) = args.get(i) {
        match a.as_str() {
            "--no-pager" | "--no-optional-locks" | "-P" => i += 1,
            "-C" => {
                let Some(d) = args.get(i + 1) else {
                    return ecriture("`git -C` sans dossier");
                };
                if !lieux.contient(Path::new(d)) {
                    return Verdict::Refus(format!("{d} est hors des dossiers de code déclarés"));
                }
                i += 2;
            }
            "-c" => return ecriture("`git -c` change la configuration (peut lancer un programme)"),
            a if a.starts_with("--git-dir")
                || a.starts_with("--work-tree")
                || a.starts_with("--exec-path") =>
            {
                return ecriture(format!("`git {a}`"));
            }
            "--version" => return Verdict::Lecture,
            _ => break,
        }
    }
    let Some(sous) = args.get(i).map(String::as_str) else {
        return ecriture("`git` sans sous-commande");
    };
    let reste = &args[i + 1..];
    if matches!(sous, "rm" | "clean") {
        return suppression(format!("`git {sous}` supprime des fichiers"));
    }
    // Options qui écrivent ou exécutent, quelle que soit la sous-commande.
    if let Some(o) = reste.iter().find(|a| {
        a.starts_with("--output")
            || *a == "--ext-diff"
            || *a == "-O"
            || a.starts_with("--open-files-in-pager")
            || a.starts_with("--exec")
            || a.starts_with("--upload-pack")
    }) {
        return ecriture(format!("`git {sous} {o}` écrit ou exécute"));
    }
    let lecture = match sous {
        s if GIT_LECTURE.contains(&s) => true,
        // Lister seulement : un nom de branche crée la branche.
        "branch" => {
            reste.iter().all(|a| {
                matches!(
                    a.as_str(),
                    "-a" | "--all"
                        | "-r"
                        | "--remotes"
                        | "-v"
                        | "-vv"
                        | "--verbose"
                        | "-l"
                        | "--list"
                        | "--show-current"
                        | "--merged"
                        | "--no-merged"
                        | "--contains"
                        | "--no-contains"
                        | "--points-at"
                        | "--sort=-committerdate"
                        | "--no-color"
                        | "--color"
                ) || a.starts_with("--sort=")
                    || a.starts_with("--format=")
            }) || reste.iter().any(|a| a == "-l" || a == "--list")
                && !reste.iter().any(|a| {
                    matches!(
                        a.as_str(),
                        "-d" | "-D"
                            | "-m"
                            | "-M"
                            | "-c"
                            | "-C"
                            | "--delete"
                            | "--move"
                            | "--copy"
                            | "-f"
                            | "--force"
                            | "-u"
                            | "--set-upstream-to"
                            | "--unset-upstream"
                            | "--edit-description"
                    )
                })
        }
        "tag" => reste.is_empty() || reste.iter().any(|a| a == "-l" || a == "--list"),
        "remote" => {
            reste.is_empty()
                || matches!(
                    reste.first().map(String::as_str),
                    Some("-v" | "--verbose" | "show" | "get-url")
                )
        }
        "config" => reste.first().is_some_and(|a| {
            matches!(
                a.as_str(),
                "--get" | "--get-all" | "--get-regexp" | "--list" | "-l"
            )
        }),
        "stash" => matches!(reste.first().map(String::as_str), Some("list" | "show")),
        "worktree" => matches!(reste.first().map(String::as_str), Some("list")),
        "reflog" => matches!(reste.first().map(String::as_str), None | Some("show")),
        "submodule" => matches!(reste.first().map(String::as_str), Some("status")),
        _ => false,
    };
    if !lecture {
        return ecriture(format!("`git {sous}` peut modifier le dépôt"));
    }
    // Chemins après `--`.
    if let Some(p) = reste.iter().position(|a| a == "--") {
        return verifier_args(&reste[p + 1..], lieux);
    }
    // Les autres arguments sont des révisions ou des motifs ; seuls les
    // chemins absolus ou remontants sont vérifiés.
    verifier_args(reste, lieux)
}

/// Cmdlets PowerShell (et alias) de lecture.
const LECTURE_POWERSHELL: &[&str] = &[
    "get-childitem",
    "gci",
    "ls",
    "dir",
    "get-content",
    "gc",
    "cat",
    "type",
    "select-string",
    "sls",
    "get-location",
    "pwd",
    "gl",
    "test-path",
    "get-item",
    "gi",
    "resolve-path",
    "rvpa",
    "measure-object",
    "measure",
    "select-object",
    "select",
    "sort-object",
    "sort",
    "format-table",
    "ft",
    "format-list",
    "fl",
    "get-filehash",
    "get-date",
    "get-itemproperty",
    "gp",
    "out-string",
    "split-path",
    "join-path",
    "convertfrom-json",
    "convertto-json",
    "group-object",
    "group",
    "get-command",
    "gcm",
    "write-output",
    "echo",
];

const SUPPRESSION_POWERSHELL: &[&str] = &[
    "remove-item",
    "rm",
    "del",
    "erase",
    "rd",
    "rmdir",
    "ri",
    "clear-content",
    "clc",
];

fn juger_powershell(mots: &[String], lieux: &mut Lieux) -> Verdict {
    let Some((cmd, args)) = mots.split_first() else {
        return ecriture("commande vide");
    };
    let nom = cmd.to_lowercase();
    if SUPPRESSION_POWERSHELL.contains(&nom.as_str()) {
        return suppression(format!("`{cmd}` supprime des fichiers"));
    }
    if nom == "git" {
        return juger_git(args, lieux);
    }
    if nom == "set-location" || nom == "cd" || nom == "sl" {
        return match args {
            [d] if lieux.contient(Path::new(d)) => {
                lieux.courant = normaliser(&lieux.courant.join(d));
                Verdict::Lecture
            }
            [d] => Verdict::Refus(format!("{d} est hors des dossiers de code déclarés")),
            _ => ecriture("`Set-Location` sans dossier unique"),
        };
    }
    if !LECTURE_POWERSHELL.contains(&nom.as_str()) {
        return ecriture(format!(
            "`{cmd}` n'est pas sur la liste des commandes de lecture"
        ));
    }
    if let Some(o) = args.iter().find(|a| {
        let a = a.to_lowercase();
        a.starts_with("-encodedcommand") || a == "-wait" || a.starts_with("-outfile")
    }) {
        return ecriture(format!("`{cmd} {o}`"));
    }
    verifier_args(args, lieux)
}

/// Juge une ligne de commande. `dossiers` : dossiers de code déclarés ;
/// `courant` : dossier où la commande s'exécute (le premier dossier si
/// rien n'est dit).
pub fn juger(
    ligne: &str,
    interprete: Interprete,
    dossiers: &[PathBuf],
    courant: Option<&Path>,
) -> Verdict {
    let Some(premier) = dossiers.first() else {
        return Verdict::Refus("aucun dossier de code n'est déclaré".into());
    };
    let courant = courant.map_or_else(|| premier.clone(), Path::to_path_buf);
    let mut lieux = Lieux { dossiers, courant };
    if !lieux.contient(&lieux.courant.clone()) {
        return Verdict::Refus(format!(
            "{} est hors des dossiers de code déclarés",
            lieux.courant.display()
        ));
    }
    let decoupe = match interprete {
        Interprete::Bash => decouper_bash(ligne),
        Interprete::PowerShell => decouper_powershell(ligne),
    };
    let liste = match decoupe.and_then(commandes) {
        Ok(l) => l,
        Err(raison) => return ecriture(raison),
    };
    let mut verdict = Verdict::Lecture;
    for mots in &liste {
        let v = match interprete {
            Interprete::Bash => juger_bash(mots, &mut lieux),
            Interprete::PowerShell => juger_powershell(mots, &mut lieux),
        };
        match v {
            Verdict::Lecture => {}
            // Un refus l'emporte ; puis une suppression ; puis une écriture.
            Verdict::Refus(_) => return v,
            Verdict::Ecriture {
                suppression: true, ..
            } => verdict = v,
            Verdict::Ecriture { .. } if verdict == Verdict::Lecture => verdict = v,
            Verdict::Ecriture { .. } => {}
        }
    }
    verdict
}

#[cfg(test)]
mod tests {
    use super::*;

    fn racine() -> PathBuf {
        PathBuf::from(if cfg!(windows) {
            "C:/code/kobi"
        } else {
            "/code/kobi"
        })
    }

    fn abs(p: &str) -> String {
        if cfg!(windows) {
            format!("C:{p}")
        } else {
            p.to_string()
        }
    }

    fn bash(ligne: &str) -> Verdict {
        juger(ligne, Interprete::Bash, &[racine()], None)
    }

    fn ps(ligne: &str) -> Verdict {
        juger(ligne, Interprete::PowerShell, &[racine()], None)
    }

    #[track_caller]
    fn lecture(v: Verdict, ligne: &str) {
        assert_eq!(v, Verdict::Lecture, "{ligne}");
    }

    #[track_caller]
    fn ecrit(v: Verdict, ligne: &str) {
        assert!(
            matches!(
                v,
                Verdict::Ecriture {
                    suppression: false,
                    ..
                }
            ),
            "{ligne} : {v:?}"
        );
    }

    #[track_caller]
    fn supprime(v: Verdict, ligne: &str) {
        assert!(
            matches!(
                v,
                Verdict::Ecriture {
                    suppression: true,
                    ..
                }
            ),
            "{ligne} : {v:?}"
        );
    }

    #[track_caller]
    fn refuse(v: Verdict, ligne: &str) {
        assert!(matches!(v, Verdict::Refus(_)), "{ligne} : {v:?}");
    }

    #[test]
    fn commandes_de_lecture_acceptees() {
        for ligne in [
            "ls",
            "ls -la crates",
            "cat Cargo.toml",
            "head -n 20 crates/kobi-app/src/chef.rs",
            "tail -n 50 README.md",
            "wc -l crates/kobi-app/src/*.rs",
            "rg fabrique_claude",
            "rg -n 'fn fabrique' crates",
            "grep -rn \"Kobi\" ui/src",
            "find . -name '*.rs' -type f",
            "tree -L 2",
            "git status",
            "git log --oneline -5",
            "git log -5 --format='%h %s'",
            "git show HEAD~1 --stat",
            "git diff main...HEAD -- crates",
            "git blame -L 10,20 crates/kobi-app/src/chef.rs",
            "git branch",
            "git branch -a --sort=-committerdate",
            "git grep -n fabrique",
            "git ls-files crates",
            "git -C crates/kobi-app log -3",
            "git --no-pager log -1",
            "git tag",
            "git remote -v",
            "git config --get remote.origin.url",
            "git stash list",
            "git worktree list",
            "cargo metadata --format-version 1",
            "cargo tree -p kobi-app",
            "npm ls --depth 0",
            "gh pr view 108",
            "gh pr list --state open",
            "gh issue list",
            "sed -n '1,20p' Cargo.toml",
            "git log --oneline | head -5",
            "rg TODO | wc -l",
            "cd crates && ls",
            "git status && git log -1",
            "ls 2>/dev/null",
            "rg foo 2>&1 | head",
            "pwd; ls",
            "echo bonjour",
            "rustc --version",
            "diff Cargo.toml Cargo.lock",
        ] {
            lecture(bash(ligne), ligne);
        }
    }

    #[test]
    fn options_et_commandes_qui_ecrivent() {
        for ligne in [
            "touch x.txt",
            "mkdir build",
            "mv a b",
            "cp a b",
            "sed -i 's/a/b/' x.rs",
            "sed 's/a/b/w sortie' x.rs",
            "sed -e 's/a/b/' x.rs",
            "find . -exec cat {} +",
            "find . -name '*.rs' -execdir ls ;",
            "find . -fprint liste.txt",
            "git checkout main",
            "git reset --hard HEAD~1",
            "git commit -m test",
            "git push",
            "git branch nouvelle",
            "git branch -D vieille",
            "git branch -m nouveau-nom",
            "git tag v1.0",
            "git config user.name Kobi",
            "git stash",
            "git log --output=journal.txt",
            "git diff --ext-diff",
            "git grep -O fabrique",
            "git -c core.pager=sh log",
            "cargo build",
            "cargo test",
            "npm install",
            "gh pr merge 108",
            "gh issue create",
            "tail -f kobi.log",
            "rg --pre cat fabrique",
            "sort -o sortie.txt entree.txt",
            "uniq entree.txt sortie.txt",
            "tree -o arbre.txt",
            "python script.py",
            "node -e 'require(\"fs\")'",
            "awk '{print}' x",
            "chmod +x script.sh",
            "curl https://example.com",
            "/usr/bin/ls",
            "./build.sh",
            "env",
            "PAGER=sh git log",
        ] {
            ecrit(bash(ligne), ligne);
        }
    }

    #[test]
    fn contournements_classiques() {
        for ligne in [
            "ls; touch x",
            "ls && rm -f x",
            "ls || mkdir y",
            "cat x | tee y",
            "ls > liste.txt",
            "ls >> liste.txt",
            "cat < /etc/passwd",
            "echo $(touch x)",
            "echo `touch x`",
            "echo \"$(touch x)\"",
            "echo $HOME",
            "ls &",
            "(cd .. && touch x)",
            "cat <(ls)",
            "ls |& tee x",
            "ls\ntouch x",
            "echo 'non fermé",
            "{ ls; }",
        ] {
            let v = bash(ligne);
            assert!(
                !matches!(v, Verdict::Lecture),
                "contournement accepté : {ligne}"
            );
        }
    }

    #[test]
    fn suppressions() {
        for ligne in [
            "rm -rf target",
            "rm Cargo.lock",
            "rmdir vide",
            "find . -name '*.tmp' -delete",
            "git rm fichier",
            "git clean -fdx",
            "ls && rm -rf target",
        ] {
            supprime(bash(ligne), ligne);
        }
    }

    #[test]
    fn hors_des_dossiers_de_code() {
        for ligne in [
            format!("cat {}", abs("/etc/passwd")),
            "cat ../../etc/passwd".to_string(),
            "ls ~".to_string(),
            "ls ~/Documents".to_string(),
            format!("cd {}", abs("/tmp")),
            "cd ..".to_string(),
            "cd".to_string(),
            format!("git -C {} log", abs("/etc")),
            format!("rg secret {}", abs("/home")),
            format!("find {} -name x", abs("/")),
            format!("head {}", abs("/code/kobi-bis/x")),
        ] {
            refuse(bash(&ligne), &ligne);
        }
        // Le motif de rg n'est pas un chemin : seul le dossier compte.
        lecture(bash("rg '/api/v1' crates"), "rg '/api/v1' crates");
        // Un chemin absolu dans les dossiers est permis.
        let dedans = format!("cat {}", abs("/code/kobi/Cargo.toml"));
        lecture(bash(&dedans), &dedans);
    }

    #[test]
    fn powershell() {
        for ligne in [
            "Get-ChildItem",
            "Get-ChildItem -Recurse -Filter *.rs crates",
            "Get-Content Cargo.toml",
            "gc README.md | Select-Object -First 10",
            "Select-String -Pattern fabrique -Path crates",
            "git log --oneline -5",
            "Get-ChildItem | Measure-Object",
        ] {
            lecture(ps(ligne), ligne);
        }
        for ligne in [
            "Set-Content x.txt bonjour",
            "New-Item x.txt",
            "Out-File x.txt",
            "Get-ChildItem | Where-Object { $_.Length -gt 0 }",
            "Get-Content x; New-Item y",
            "Get-ChildItem > liste.txt",
            "Invoke-Expression 'rm x'",
            "iex (gc script.ps1)",
            "Start-Process notepad",
            "powershell -EncodedCommand ZQBjAGgAbwA=",
            "Get-Content $env:USERPROFILE\\secret",
            "& calc.exe",
            "Copy-Item a b",
        ] {
            assert!(!matches!(ps(ligne), Verdict::Lecture), "{ligne}");
        }
        supprime(ps("Remove-Item -Recurse target"), "Remove-Item");
        supprime(ps("rm target"), "rm");
        refuse(
            ps(&format!("Get-Content {}", abs("/Windows/win.ini"))),
            "Get-Content hors dossiers",
        );
    }

    #[test]
    fn sans_dossier_tout_est_refuse() {
        assert!(matches!(
            juger("ls", Interprete::Bash, &[], None),
            Verdict::Refus(_)
        ));
    }
}
