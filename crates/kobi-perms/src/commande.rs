//! Commandes en lecture seule (KOB-154) : Kobi décide lui-même, avant
//! l'exécution, si une ligne de commande ne fait que lire.
//!
//! La ligne est découpée comme le ferait le shell (guillemets, échappements,
//! opérateurs) ; chaque commande d'un enchaînement (`;`, `&&`, `||`, `|`)
//! doit être sur la liste blanche, avec des options qui n'écrivent rien, et
//! tous ses chemins doivent rester dans les dossiers déclarés. Tout ce qu'on
//! ne sait pas analyser (substitution `$( )`, variable, sous-shell,
//! redirection vers un fichier, shell imbriqué) n'est pas une lecture.
//!
//! Le résultat suit le contrat commun des permissions : [`Classe::Lecture`],
//! [`Classe::Ecriture`] (commande qui modifie, suppression) ou
//! [`Classe::Refus`] (hors des dossiers déclarés), avec une raison lisible
//! que Claude reçoit telle quelle.

use std::path::{Path, PathBuf};

use crate::action::Shell;
use crate::classement::{Classe, Contexte, Genre};

/// Verdict sur une ligne de commande.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Avis {
    pub classe: Classe,
    /// Pourquoi ce n'est pas une lecture (vide pour une lecture).
    pub raison: String,
}

impl Avis {
    fn lecture() -> Self {
        Self {
            classe: Classe::Lecture,
            raison: String::new(),
        }
    }

    fn ecriture(genre: Genre, raison: impl Into<String>) -> Self {
        Self {
            classe: Classe::Ecriture(genre),
            raison: raison.into(),
        }
    }

    fn refus(raison: impl Into<String>) -> Self {
        let raison = raison.into();
        Self {
            classe: Classe::Refus(raison.clone()),
            raison,
        }
    }

    /// Le plus grave des deux : refus, puis suppression, puis écriture.
    fn pire(self, autre: Self) -> Self {
        let rang = |a: &Avis| match a.classe {
            Classe::Lecture => 0,
            Classe::Ecriture(Genre::Suppression) => 2,
            Classe::Ecriture(_) => 1,
            Classe::Refus(_) => 3,
        };
        if rang(&autre) > rang(&self) {
            autre
        } else {
            self
        }
    }

    pub fn est_lecture(&self) -> bool {
        self.classe == Classe::Lecture
    }
}

/// Classe une ligne de commande.
pub fn analyser(ligne: &str, shell: Shell, ctx: &Contexte) -> Avis {
    let Some(cwd) = ctx.dossier_courant() else {
        return Avis::refus("aucun dossier de code n'est déclaré");
    };
    if !ctx.contient(&cwd, None) {
        return Avis::refus(format!(
            "{} est hors des dossiers de code déclarés",
            cwd.display()
        ));
    }
    let jetons = match decouper(ligne, shell) {
        Ok(j) => j,
        Err(raison) => return Avis::ecriture(Genre::Commande, raison),
    };
    let mut avis = Avis::lecture();
    // Dossiers courants possibles (KOB-227) : un `cd` qui peut échouer
    // (`cd x; cat ../../y`) laisse aussi l'ancien. Chaque commande est vérifiée
    // dans tous.
    let mut cwds = vec![cwd];
    let mut mots: Vec<String> = Vec::new();
    let mut i = 0;
    while i <= jetons.len() {
        let jeton = jetons.get(i);
        match jeton {
            None | Some(Jeton::Op(_)) => {
                let op = match jeton {
                    Some(Jeton::Op(o)) => o.as_str(),
                    _ => "",
                };
                if op == "&" {
                    return avis.pire(Avis::ecriture(
                        Genre::Commande,
                        "commande lancée en arrière-plan (`&`)",
                    ));
                }
                if op == "(" || op == ")" {
                    return avis.pire(Avis::ecriture(
                        Genre::Commande,
                        "sous-shell `( )` non analysable",
                    ));
                }
                if !mots.is_empty() {
                    let mut suivants = Vec::new();
                    for c in &cwds {
                        let mut nouveau = c.clone();
                        avis = avis.pire(commande(&mots, shell, ctx, &mut nouveau));
                        // Seul `&&` garantit que le `cd` a réussi ; `|` le lance
                        // dans un sous-shell.
                        if op != "&&" && !suivants.contains(c) {
                            suivants.push(c.clone());
                        }
                        if op != "|" && !suivants.contains(&nouveau) {
                            suivants.push(nouveau);
                        }
                    }
                    if suivants.len() > DOSSIERS_POSSIBLES {
                        return avis.pire(Avis::ecriture(
                            Genre::Commande,
                            "trop de `cd` enchaînés pour suivre le dossier courant",
                        ));
                    }
                    cwds = suivants;
                    mots.clear();
                } else if matches!(op, "|" | "&&" | "||") {
                    // `| ls`, `ls && && x` : rien avant l'opérateur.
                    return avis.pire(Avis::ecriture(Genre::Commande, "enchaînement incomplet"));
                }
                if jeton.is_none()
                    && matches!(jetons.last(), Some(Jeton::Op(o)) if matches!(o.as_str(), "|" | "&&" | "||"))
                {
                    // `ls |` : la commande qui suit manque.
                    return avis.pire(Avis::ecriture(Genre::Commande, "enchaînement incomplet"));
                }
                i += 1;
            }
            Some(Jeton::Redir(r)) => {
                let cible = match jetons.get(i + 1) {
                    Some(Jeton::Mot(m)) if !r.contains('&') || r.ends_with('>') => Some(m.clone()),
                    _ => None,
                };
                let mut saute = 0;
                for c in &cwds {
                    match redirection(r, cible.as_deref(), ctx, c) {
                        Ok(n) => saute = n,
                        Err(a) => return avis.pire(a),
                    }
                }
                i += 1 + saute;
            }
            Some(Jeton::Mot(m)) => {
                mots.push(m.clone());
                i += 1;
            }
        }
    }
    avis
}

/// Dossiers courants possibles suivis au plus (`cd` incertains enchaînés).
const DOSSIERS_POSSIBLES: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Jeton {
    Mot(String),
    /// `;`, `&&`, `||`, `|`, `&`, saut de ligne, `(`, `)`.
    Op(String),
    /// `>`, `>>`, `2>`, `2>&1`, `<`, `<<`, `*>`…
    Redir(String),
}

/// Découpe la ligne en jetons. Refuse ce qui demanderait d'exécuter du code
/// pour connaître la commande : substitutions, variables, blocs.
fn decouper(ligne: &str, shell: Shell) -> Result<Vec<Jeton>, String> {
    let ps = shell == Shell::PowerShell;
    let echap = if ps { '`' } else { '\\' };
    let mut jetons = Vec::new();
    let mut mot = String::new();
    let mut dans_mot = false;
    let c: Vec<char> = ligne.chars().collect();
    let mut i = 0;
    let fin_mot = |jetons: &mut Vec<Jeton>, mot: &mut String, dans_mot: &mut bool| {
        if *dans_mot {
            jetons.push(Jeton::Mot(std::mem::take(mot)));
            *dans_mot = false;
        }
    };
    while i < c.len() {
        let ch = c[i];
        match ch {
            ' ' | '\t' | '\r' => fin_mot(&mut jetons, &mut mot, &mut dans_mot),
            '\'' => {
                dans_mot = true;
                i += 1;
                loop {
                    match c.get(i) {
                        None => return Err("guillemet simple non fermé".into()),
                        // PowerShell : `''` dans une chaîne simple vaut `'`.
                        Some('\'') if ps && c.get(i + 1) == Some(&'\'') => {
                            mot.push('\'');
                            i += 1;
                        }
                        Some('\'') => break,
                        Some(&x) => mot.push(x),
                    }
                    i += 1;
                }
            }
            '"' => {
                dans_mot = true;
                i += 1;
                loop {
                    match c.get(i) {
                        None => return Err("guillemet double non fermé".into()),
                        Some('"') => break,
                        Some('$') => {
                            return Err("variable ou substitution `$` entre guillemets".into())
                        }
                        Some('`') if !ps => return Err("substitution entre accents graves".into()),
                        Some(&x) if x == echap => {
                            i += 1;
                            match c.get(i) {
                                Some(&y) => mot.push(y),
                                None => return Err("échappement en fin de ligne".into()),
                            }
                        }
                        Some(&x) => mot.push(x),
                    }
                    i += 1;
                }
            }
            '$' => return Err("variable ou substitution `$` non analysable".into()),
            '`' if !ps => return Err("substitution entre accents graves".into()),
            '{' | '}' if ps => return Err("bloc de script PowerShell `{ }`".into()),
            // `cat {x,../../.ssh/id_rsa}`, `{1..3}` : bash les développe en
            // plusieurs chemins (KOB-227). `{}` seul reste tel quel.
            '{' if accolades_developpees(&c[i + 1..]) => {
                return Err("expansion d'accolades `{…}` non analysable".into())
            }
            '@' if ps && matches!(c.get(i + 1), Some('(' | '{' | '"' | '\'')) => {
                return Err("expression PowerShell `@( )` non analysable".into())
            }
            x if x == echap => {
                dans_mot = true;
                i += 1;
                match c.get(i) {
                    Some('\n') => {}
                    Some(&y) => mot.push(y),
                    None => return Err("échappement en fin de ligne".into()),
                }
            }
            '#' if !dans_mot => break,
            ';' | '\n' | '(' | ')' => {
                fin_mot(&mut jetons, &mut mot, &mut dans_mot);
                jetons.push(Jeton::Op(ch.to_string()));
            }
            '|' => {
                fin_mot(&mut jetons, &mut mot, &mut dans_mot);
                if c.get(i + 1) == Some(&'|') {
                    i += 1;
                    jetons.push(Jeton::Op("||".into()));
                } else {
                    jetons.push(Jeton::Op("|".into()));
                }
            }
            '&' => {
                if c.get(i + 1) == Some(&'&') {
                    fin_mot(&mut jetons, &mut mot, &mut dans_mot);
                    i += 1;
                    jetons.push(Jeton::Op("&&".into()));
                } else if c.get(i + 1) == Some(&'>') {
                    fin_mot(&mut jetons, &mut mot, &mut dans_mot);
                    i += 1;
                    let mut r = String::from("&>");
                    if c.get(i + 1) == Some(&'>') {
                        i += 1;
                        r.push('>');
                    }
                    jetons.push(Jeton::Redir(r));
                } else {
                    fin_mot(&mut jetons, &mut mot, &mut dans_mot);
                    jetons.push(Jeton::Op("&".into()));
                }
            }
            '>' | '<' => {
                // `2>`, `1>>`, `*>` : le numéro collé fait partie de la redirection.
                let mut r = String::new();
                if dans_mot && (mot.chars().all(|d| d.is_ascii_digit()) || (ps && mot == "*")) {
                    r = std::mem::take(&mut mot);
                    dans_mot = false;
                } else {
                    fin_mot(&mut jetons, &mut mot, &mut dans_mot);
                }
                r.push(ch);
                while let Some(&n) = c.get(i + 1) {
                    if n == ch || (n == '&' && ch == '>') {
                        r.push(n);
                        i += 1;
                        if n == '&' {
                            // `2>&1` : la destination est un descripteur.
                            while let Some(&d) = c.get(i + 1) {
                                if d.is_ascii_digit() || d == '-' {
                                    r.push(d);
                                    i += 1;
                                } else {
                                    break;
                                }
                            }
                            break;
                        }
                    } else {
                        break;
                    }
                }
                jetons.push(Jeton::Redir(r));
            }
            x => {
                dans_mot = true;
                mot.push(x);
            }
        }
        i += 1;
    }
    fin_mot(&mut jetons, &mut mot, &mut dans_mot);
    Ok(jetons)
}

/// Ce qui suit un `{` hors guillemets : bash peut-il le développer
/// (`{a,b}`, `{1..3}`) ? Règle prudente, sans chercher à refaire l'analyse de
/// bash : une virgule ou `..` n'importe où après l'accolade, suivie plus loin
/// d'un `}`, suffit pour refuser. Guillemets, échappements (KOB-258, KOB-272)
/// et accolades imbriquées (`{{},-delete}` donne `{} -delete`, KOB-286) n'y
/// changent rien ; `{}` seul (`find -exec cat {} +`) reste tel quel.
fn accolades_developpees(suite: &[char]) -> bool {
    suite
        .iter()
        .enumerate()
        .find(|&(i, &x)| x == ',' || (x == '.' && suite.get(i + 1) == Some(&'.')))
        .is_some_and(|(i, _)| suite[i..].contains(&'}'))
}

/// Redirection : seules celles qui n'écrivent dans aucun fichier passent.
/// Renvoie le nombre de jetons de cible consommés.
fn redirection(r: &str, cible: Option<&str>, ctx: &Contexte, cwd: &Path) -> Result<usize, Avis> {
    // `2>&1`, `>&2` : d'un descripteur vers un autre, rien n'est écrit.
    if r.contains(">&") && r.chars().last().is_some_and(|c| c.is_ascii_digit()) {
        return Ok(0);
    }
    if r.starts_with("<<") {
        return Err(Avis::ecriture(
            Genre::Commande,
            "document en ligne `<<` non analysable",
        ));
    }
    let Some(cible) = cible else {
        return Err(Avis::ecriture(Genre::Commande, "redirection sans cible"));
    };
    if r.contains('<') {
        // Lecture d'un fichier sur l'entrée : permise dans les dossiers.
        return if ctx.contient(Path::new(cible), Some(cwd)) {
            Ok(1)
        } else {
            Err(Avis::refus(format!(
                "{cible} est hors des dossiers de code déclarés"
            )))
        };
    }
    let poubelle = matches!(
        cible.to_ascii_lowercase().as_str(),
        "/dev/null" | "nul" | "$null" | "null"
    );
    if poubelle {
        Ok(1)
    } else {
        Err(Avis::ecriture(
            Genre::Fichier,
            format!("redirection vers un fichier (`{r} {cible}`)"),
        ))
    }
}

/// Commandes qui suppriment.
const SUPPRESSION: &[&str] = &[
    "rm",
    "rmdir",
    "unlink",
    "shred",
    "del",
    "erase",
    "rd",
    "remove-item",
    "ri",
    "remove-itemproperty",
    "clear-content",
    "clc",
];

/// Shells et interpréteurs : ils lancent n'importe quoi.
const SHELLS: &[&str] = &[
    "bash",
    "sh",
    "zsh",
    "dash",
    "fish",
    "cmd",
    "powershell",
    "pwsh",
    "python",
    "python3",
    "py",
    "node",
    "deno",
    "perl",
    "ruby",
    "php",
    "lua",
    "wsl",
    "busybox",
    "invoke-expression",
    "iex",
    "invoke-command",
    "icm",
    "start-process",
    "saps",
    "start",
    "env",
    "xargs",
    "eval",
    "exec",
    "source",
    ".",
    "sudo",
    "runas",
    "nohup",
    "timeout",
    "watch",
    "time",
    "nice",
    "awk",
    "gawk",
    "mawk",
    "osascript",
    "ssh",
];

/// Commandes de lecture sans option dangereuse connue.
const LECTURE_SIMPLE: &[&str] = &[
    "ls",
    "dir",
    "cat",
    "head",
    "tail",
    "wc",
    "tree",
    "pwd",
    "echo",
    "file",
    "stat",
    "du",
    "which",
    "where",
    "type",
    "basename",
    "dirname",
    "realpath",
    "readlink",
    "cut",
    "tr",
    "nl",
    "column",
    "diff",
    "cmp",
    "comm",
    "md5sum",
    "sha1sum",
    "sha256sum",
    "sha512sum",
    "b2sum",
    "cksum",
    "true",
    "false",
    "whoami",
    "uname",
    "jq",
    "grep",
    "egrep",
    "fgrep",
    "rg",
    "fd",
    "sort",
    "uniq",
    "find",
    "sed",
    "date",
    "hostname",
    "printf",
    "tac",
    "rev",
    "fold",
    "fmt",
    "expand",
    "od",
    "xxd",
    "hexdump",
    "strings",
    "less",
    "more",
    "git",
    "cargo",
    "npm",
    "pnpm",
    "yarn",
    "gh",
    "rustc",
    "rustup",
    "go",
    "dotnet",
    "java",
    "tsc",
];

/// Cmdlets PowerShell de lecture (et leurs alias).
const LECTURE_PS: &[&str] = &[
    "get-childitem",
    "gci",
    "get-content",
    "gc",
    "select-string",
    "sls",
    "get-item",
    "gi",
    "get-itemproperty",
    "gp",
    "test-path",
    "resolve-path",
    "rvpa",
    "get-location",
    "gl",
    "measure-object",
    "measure",
    "select-object",
    "select",
    "sort-object",
    "format-table",
    "ft",
    "format-list",
    "fl",
    "format-wide",
    "fw",
    "get-filehash",
    "get-command",
    "gcm",
    "out-string",
    "write-output",
    "write",
    "write-host",
    "get-date",
    "split-path",
    "join-path",
    "group-object",
    "group",
    "get-unique",
    "gu",
    "compare-object",
    "compare",
    "convertto-json",
    "convertfrom-json",
    "get-acl",
    "get-psdrive",
    "gdr",
    "get-help",
    "get-member",
    "gm",
    "where-object",
    "where",
    "?",
];

/// Cmdlets de pipeline : leurs arguments sont des noms de propriétés, pas des chemins.
const SANS_CHEMIN_PS: &[&str] = &[
    "measure-object",
    "measure",
    "select-object",
    "select",
    "sort-object",
    "format-table",
    "ft",
    "format-list",
    "fl",
    "format-wide",
    "fw",
    "out-string",
    "write-output",
    "write",
    "write-host",
    "get-date",
    "group-object",
    "group",
    "get-unique",
    "gu",
    "compare-object",
    "compare",
    "convertto-json",
    "convertfrom-json",
    "get-command",
    "gcm",
    "get-help",
    "get-member",
    "gm",
    "where-object",
    "where",
    "?",
    "echo",
    "printf",
    "date",
    "hostname",
    "whoami",
    "uname",
    "true",
    "false",
    "tr",
    "rustc",
    "rustup",
    "go",
    "dotnet",
    "java",
    "tsc",
    "which",
];

/// Commandes dont le premier argument positionnel est un motif (ou un
/// script `sed`, un filtre `jq`), pas un chemin.
const MOTIF_D_ABORD: &[&str] = &[
    "grep",
    "egrep",
    "fgrep",
    "rg",
    "select-string",
    "sls",
    "fd",
    "sed",
    "jq",
];

/// Options suivies d'une valeur qui n'est pas un chemin (motif, nombre…),
/// par commande. Une option absente de la liste n'emporte pas de valeur :
/// l'argument suivant est alors vérifié comme un chemin (au pire, un refus de
/// trop, jamais un chemin qui échappe à la vérification).
fn options_a_valeur(nom: &str) -> &'static [&'static str] {
    match nom {
        "grep" | "egrep" | "fgrep" => &[
            "-e",
            "--regexp",
            "-m",
            "--max-count",
            "-A",
            "-B",
            "-C",
            "--context",
            "--after-context",
            "--before-context",
            "--color",
            "--label",
        ],
        "rg" => &[
            "-e",
            "--regexp",
            "-g",
            "--glob",
            "--iglob",
            "-t",
            "--type",
            "-T",
            "--type-not",
            "-m",
            "--max-count",
            "-A",
            "-B",
            "-C",
            "--context",
            "-M",
            "--max-columns",
            "-j",
            "--threads",
            "--sort",
            "--sortr",
            "-E",
            "--encoding",
            "--max-depth",
            "-d",
        ],
        "fd" => &[
            "-e",
            "--extension",
            "-t",
            "--type",
            "-d",
            "--max-depth",
            "--min-depth",
            "-E",
            "--exclude",
            "-j",
        ],
        "head" | "tail" => &["-n", "-c", "--lines", "--bytes"],
        "tree" => &["-L", "-I", "-P", "--charset"],
        "find" => &[
            "-name",
            "-iname",
            "-path",
            "-ipath",
            "-type",
            "-maxdepth",
            "-mindepth",
            "-size",
            "-mtime",
            "-mmin",
            "-atime",
            "-ctime",
            "-regex",
            "-iregex",
            "-user",
            "-group",
            "-perm",
            "-wholename",
        ],
        "du" => &["-d", "--max-depth"],
        "cut" => &["-d", "-f", "-c", "-b", "--delimiter", "--fields"],
        "sort" => &["-k", "-t", "--key", "--field-separator"],
        "uniq" => &["-f", "-s", "-w"],
        "git" => &[
            "--format",
            "--pretty",
            "--since",
            "--until",
            "--after",
            "--before",
            "--author",
            "--committer",
            "--grep",
            "-L",
            "-S",
            "-G",
            "-n",
            "--max-count",
            "-U",
            "--abbrev",
            "--date",
            "--sort",
            "--points-at",
            "--contains",
            "--merged",
            "--no-merged",
            "--depth",
        ],
        "gh" => &[
            "--json",
            "--jq",
            "-q",
            "--state",
            "-s",
            "--search",
            "-S",
            "--label",
            "-l",
            "--repo",
            "-R",
            "--limit",
            "-L",
            "--author",
            "-A",
            "--base",
            "-B",
            "--head",
            "-H",
            "--assignee",
            "-a",
            "--template",
            "-t",
            "--branch",
            "-b",
            "--workflow",
            "-w",
            "--status",
        ],
        "npm" | "pnpm" | "yarn" => &["--depth"],
        "cargo" => &[
            "-p",
            "--package",
            "--format-version",
            "--depth",
            "--prefix",
            "-e",
            "--edges",
            "-i",
            "--invert",
            "--target",
            "--charset",
        ],
        "get-childitem" | "gci" | "ls" | "dir" => {
            &["-filter", "-include", "-exclude", "-depth", "-attributes"]
        }
        "get-content" | "gc" | "cat" | "type" => &[
            "-totalcount",
            "-head",
            "-first",
            "-tail",
            "-last",
            "-encoding",
            "-delimiter",
            "-readcount",
        ],
        "select-string" | "sls" => &["-pattern", "-include", "-exclude", "-context", "-encoding"],
        "get-filehash" => &["-algorithm"],
        _ => &[],
    }
}

/// Option qui donne le motif (le premier positionnel devient alors un chemin).
/// Renvoie `Some(true)` si sa valeur suit et est un motif, `Some(false)` si sa
/// valeur est un fichier de motifs (vérifié comme un chemin).
fn option_de_motif(nom: &str, x: &str) -> Option<bool> {
    let bas = x.to_lowercase();
    match bas.as_str() {
        "-e" | "--regexp" | "-pattern" | "-simplematch" => return Some(true),
        "-f" | "--file" if matches!(nom, "grep" | "egrep" | "fgrep" | "rg" | "sed" | "jq") => {
            return Some(false)
        }
        _ => {}
    }
    // Options courtes groupées (`-rne motif`, `-rf motifs.txt`).
    if matches!(nom, "grep" | "egrep" | "fgrep" | "rg")
        && x.len() > 2
        && x.starts_with('-')
        && !x.starts_with("--")
    {
        if x.ends_with('e') {
            return Some(true);
        }
        if x.ends_with('f') {
            return Some(false);
        }
    }
    None
}

/// Une commande simple : nom et arguments.
fn commande(mots: &[String], shell: Shell, ctx: &Contexte, cwd: &mut PathBuf) -> Avis {
    let premier = &mots[0];
    if !premier.starts_with('-') && premier.contains('=') && shell == Shell::Bash {
        return Avis::ecriture(
            Genre::Commande,
            format!("variable d'environnement posée avant la commande (`{premier}`)"),
        );
    }
    if premier.contains('/') || premier.contains('\\') {
        return Avis::ecriture(
            Genre::Commande,
            format!("commande appelée par un chemin (`{premier}`)"),
        );
    }
    let nom = premier.to_lowercase();
    let nom = nom.strip_suffix(".exe").unwrap_or(&nom).to_owned();
    let args = &mots[1..];
    if nom == "cd" || nom == "set-location" || nom == "sl" || nom == "chdir" || nom == "pushd" {
        let Some(cible) = args.iter().find(|a| !a.starts_with('-')) else {
            return Avis::refus("`cd` sans dossier mène hors des dossiers de code");
        };
        if !ctx.contient(Path::new(cible), Some(cwd)) {
            return Avis::refus(format!("{cible} est hors des dossiers de code déclarés"));
        }
        if let Some(nouveau) = crate::classement::absolu(Path::new(cible), Some(cwd)) {
            *cwd = nouveau;
        }
        return Avis::lecture();
    }
    if SUPPRESSION.contains(&nom.as_str()) {
        return Avis::ecriture(
            Genre::Suppression,
            format!("`{premier}` supprime des fichiers"),
        );
    }
    if SHELLS.contains(&nom.as_str()) {
        return Avis::ecriture(
            Genre::Commande,
            format!("`{premier}` peut lancer n'importe quelle commande"),
        );
    }
    let connue = LECTURE_SIMPLE.contains(&nom.as_str())
        || (shell == Shell::PowerShell && LECTURE_PS.contains(&nom.as_str()));
    if !connue {
        return Avis::ecriture(
            Genre::Commande,
            format!("`{premier}` n'est pas une commande de lecture connue de Kobi"),
        );
    }
    if let Some(a) = options_interdites(&nom, args) {
        return a;
    }
    chemins(&nom, args, shell, ctx, cwd)
}

/// Options ou sous-commandes qui écrivent, par commande.
fn options_interdites(nom: &str, args: &[String]) -> Option<Avis> {
    let a = |s: &str| args.iter().any(|x| x == s);
    let commence = |p: &str| args.iter().any(|x| x.starts_with(p));
    let ecrit = |raison: String| Some(Avis::ecriture(Genre::Commande, raison));
    match nom {
        "find" => {
            if a("-delete") {
                return Some(Avis::ecriture(
                    Genre::Suppression,
                    "`find -delete` supprime des fichiers",
                ));
            }
            for o in ["-exec", "-execdir", "-ok", "-okdir"] {
                if a(o) {
                    return ecrit(format!("`find {o}` lance une commande"));
                }
            }
            for o in ["-fprint", "-fprint0", "-fprintf", "-fls"] {
                if a(o) {
                    return Some(Avis::ecriture(
                        Genre::Fichier,
                        format!("`find {o}` écrit dans un fichier"),
                    ));
                }
            }
            None
        }
        "fd" => {
            // `-x`/`-X` lancent une commande par résultat, `-l` lance `ls`.
            let lance = args.iter().any(|x| {
                (x.starts_with('-') && !x.starts_with("--") && x.contains(['x', 'X', 'l']))
                    || x.starts_with("--exec")
                    || x.starts_with("--list-details")
            });
            if lance {
                ecrit("`fd -x`/`-X`/`-l` lance une commande".into())
            } else {
                None
            }
        }
        "sed" => {
            if args.iter().any(|x| {
                x == "-i"
                    || abrege(x, "--in-place")
                    || (x.starts_with("-i") && !x.starts_with("--"))
            }) {
                return Some(Avis::ecriture(
                    Genre::Fichier,
                    "`sed -i` modifie le fichier",
                ));
            }
            // Scripts passés par `-e`/`-f` (même groupés, `-ne`) : non vérifiés.
            if args.iter().any(|x| {
                (x.starts_with('-') && !x.starts_with("--") && x.contains(['e', 'f']))
                    || abrege(x, "--expression")
                    || abrege(x, "--file")
            }) {
                return ecrit("`sed -e`/`sed -f` : script non vérifié".into());
            }
            let scripts: Vec<&String> = args
                .iter()
                .filter(|x| !x.starts_with('-'))
                .take(1)
                .collect();
            let sur = scripts.iter().all(|s| script_sed_sur(s)) && a("-n");
            if sur {
                None
            } else {
                ecrit("`sed` n'est permis qu'en affichage (`sed -n '1,20p'`)".into())
            }
        }
        // `-o` seul, collé (`-o/x`) ou groupé (`-uo`), ou `--output` abrégé ;
        // `-T` / `--temporary-directory` écrit ailleurs, et
        // `--compress-program` lance un programme (KOB-258).
        "sort"
            if args.iter().any(|x| {
                (x.starts_with('-') && !x.starts_with("--") && (x.contains('o') || x.contains('T')))
                    || abrege(x, "--output")
                    || abrege(x, "--temporary-directory")
                    || abrege(x, "--compress-program")
            }) =>
        {
            Some(Avis::ecriture(
                Genre::Fichier,
                "`sort -o`, `-T` ou `--compress-program` écrit ou lance un programme",
            ))
        }
        "uniq" if args.iter().filter(|x| !x.starts_with('-')).count() > 1 => Some(Avis::ecriture(
            Genre::Fichier,
            "`uniq` avec deux fichiers écrit dans le second",
        )),
        "rg" if commence("--pre") => ecrit("`rg --pre` lance une commande".into()),
        "rg" if commence("--hostname-bin") => {
            ecrit("`rg --hostname-bin` lance une commande".into())
        }
        // Suit le fichier sans jamais rendre la main.
        "tail" if a("-f") || a("-F") || commence("--follow") => {
            ecrit("`tail -f` ne s'arrête jamais".into())
        }
        // `-o` seul, groupé (`-ao`) ou collé ; `-R` écrit `00Tree.html` partout.
        "tree"
            if args
                .iter()
                .any(|x| x.starts_with('-') && !x.starts_with("--") && x.contains(['o', 'R'])) =>
        {
            Some(Avis::ecriture(
                Genre::Fichier,
                "`tree -o`/`tree -R` écrit dans un fichier",
            ))
        }
        // `printf -v nom` affecte une variable du shell.
        "printf" if commence("-v") => ecrit("`printf -v` modifie une variable".into()),
        // Le second positionnel de `xxd` est le fichier de sortie.
        "xxd" if positionnels_xxd(args) > 1 => Some(Avis::ecriture(
            Genre::Fichier,
            "`xxd entrée sortie` écrit dans un fichier",
        )),
        "date" if a("-s") || commence("--set") => ecrit("`date -s` change l'heure".into()),
        "hostname" if args.iter().any(|x| !x.starts_with('-')) => {
            ecrit("`hostname <nom>` change le nom".into())
        }
        "less" | "more"
            if args
                .iter()
                .any(|x| x.starts_with('+') || x == "-o" || commence("--log-file")) =>
        {
            ecrit("option de `less` qui écrit ou exécute".into())
        }
        "git" => git(args),
        "cargo" => {
            if commence("--config") || commence("-Z") {
                return ecrit(
                    "`cargo --config`/`-Z` change la configuration (et peut lancer un programme)"
                        .into(),
                );
            }
            let sous = args
                .iter()
                .find(|x| !x.starts_with('-'))
                .map(String::as_str);
            match sous {
                None if a("--version") || a("-V") || a("--list") => None,
                Some(
                    "metadata" | "tree" | "version" | "locate-project" | "pkgid" | "verify-project",
                ) => None,
                _ => ecrit(
                    "seuls `cargo metadata`, `cargo tree` et `cargo --version` sont en lecture"
                        .into(),
                ),
            }
        }
        "npm" | "pnpm" | "yarn" => {
            let sous = args
                .iter()
                .find(|x| !x.starts_with('-'))
                .map(String::as_str);
            match sous {
                None if a("--version") || a("-v") => None,
                Some(
                    "ls" | "list" | "ll" | "la" | "view" | "info" | "outdated" | "why" | "explain",
                ) => None,
                _ => ecrit(format!(
                    "seuls `{nom} ls`, `{nom} view` et `{nom} --version` sont en lecture"
                )),
            }
        }
        "gh" => {
            if a("--web") || a("-w") {
                return ecrit("`gh --web` ouvre le navigateur".into());
            }
            let pos: Vec<&str> = args
                .iter()
                .filter(|x| !x.starts_with('-'))
                .map(String::as_str)
                .collect();
            match pos.as_slice() {
                [] if a("--version") => None,
                ["pr" | "issue", "view" | "list" | "status" | "diff" | "checks", ..]
                | ["repo" | "release", "view" | "list", ..]
                | ["run" | "workflow", "list" | "view", ..]
                | ["auth", "status"]
                | ["status"] => None,
                _ => ecrit("cette commande `gh` n'est pas en lecture (seuls view, list, status, diff, checks)".into()),
            }
        }
        // Un seul argument, la version : `go run -v x.go` n'en est pas une.
        "rustc" | "go" | "dotnet" | "java" | "tsc"
            if !matches!(
                args,
                [x] if matches!(x.as_str(), "--version" | "-V" | "version" | "-v")
            ) =>
        {
            ecrit(format!("seul `{nom} --version` est en lecture"))
        }
        "rustup" => {
            let pos: Vec<&str> = args
                .iter()
                .filter(|x| !x.starts_with('-'))
                .map(String::as_str)
                .collect();
            match pos.as_slice() {
                [] if a("--version") || a("-V") => None,
                ["show", ..]
                | ["toolchain", "list"]
                | ["target", "list"]
                | ["component", "list"] => None,
                _ => ecrit("seuls `rustup show` et les listes sont en lecture".into()),
            }
        }
        _ => {
            // PowerShell : rien qui écrive ou supprime, quelle que soit la cmdlet.
            for x in args {
                let x = x.to_lowercase();
                if matches!(
                    x.as_str(),
                    "-outfile" | "-outvariable" | "-ov" | "-pipelinevariable" | "-pv" | "-online"
                ) {
                    return ecrit(format!("`{x}` écrit le résultat ailleurs"));
                }
            }
            None
        }
    }
}

/// Option longue ou son abréviation (getopt accepte `--exp` pour `--expression`).
fn abrege(x: &str, longue: &str) -> bool {
    let nom = x.split('=').next().unwrap_or(x);
    nom.len() >= 3 && nom.starts_with("--") && (longue.starts_with(nom) || nom.starts_with(longue))
}

/// Positionnels de `xxd`, sans les valeurs d'options (`-l 64`, `-s -0x10`).
fn positionnels_xxd(args: &[String]) -> usize {
    let mut n = 0;
    let mut options = true;
    let mut i = 0;
    while let Some(x) = args.get(i) {
        if options && x == "--" {
            options = false;
        } else if options && x.starts_with('-') && x != "-" {
            let o = x.trim_start_matches('-');
            if matches!(
                o,
                "c" | "cols"
                    | "g"
                    | "groupsize"
                    | "l"
                    | "len"
                    | "n"
                    | "name"
                    | "o"
                    | "offset"
                    | "s"
                    | "seek"
                    | "R"
            ) {
                i += 1;
            }
        } else {
            n += 1;
        }
        i += 1;
    }
    n
}

/// `sed -n` : seulement `p` sur des numéros de lignes ou un motif.
fn script_sed_sur(script: &str) -> bool {
    let s = script.trim();
    let Some(adresse) = s.strip_suffix('p') else {
        return false;
    };
    adresse.split(',').all(|a| {
        let a = a.trim();
        a.is_empty()
            || a == "$"
            || a.chars().all(|c| c.is_ascii_digit())
            || (a.len() >= 2
                && a.starts_with('/')
                && a.ends_with('/')
                && !a[1..a.len() - 1].contains('/'))
    })
}

/// `git` : options globales et sous-commandes en lecture.
fn git(args: &[String]) -> Option<Avis> {
    let ecrit = |raison: String| Some(Avis::ecriture(Genre::Commande, raison));
    let mut i = 0;
    // Options globales avant la sous-commande.
    while let Some(x) = args.get(i) {
        match x.as_str() {
            "-c" | "--config-env" => {
                return ecrit(
                    "`git -c` change la configuration (et peut lancer un programme)".into(),
                )
            }
            "-C" => i += 2,
            "--no-pager" | "-P" | "--no-optional-locks" | "--literal-pathspecs" | "--bare" => {
                i += 1
            }
            x if x.starts_with("--git-dir=") || x.starts_with("--work-tree=") => i += 1,
            x if x.starts_with('-') => {
                return ecrit(format!("option de git inconnue de Kobi (`{x}`)"))
            }
            _ => break,
        }
    }
    let sous = args.get(i)?;
    let reste = &args[i + 1..];
    let a = |s: &str| reste.iter().any(|x| x == s);
    for x in reste {
        // `git grep -iO<pager>` : `-O` groupé derrière des options sans valeur.
        let o_groupe = sous == "grep"
            && x.starts_with('-')
            && !x.starts_with("--")
            && x[1..]
                .chars()
                .take_while(|c| !"efABCm".contains(*c) && !c.is_ascii_digit())
                .any(|c| c == 'O');
        // git accepte les options longues abrégées (`--outp=`, `--ext`).
        if abrege(x, "--output")
            || abrege(x, "--ext-diff")
            || x.starts_with("-O")
            || o_groupe
            || abrege(x, "--open-files-in-pager")
            || x.starts_with("--exec")
        {
            return ecrit(format!("`git {sous} {x}` écrit ou lance un programme"));
        }
    }
    let positionnels = || reste.iter().filter(|x| !x.starts_with('-')).count();
    match sous.as_str() {
        "status" | "log" | "show" | "diff" | "blame" | "annotate" | "grep" | "ls-files"
        | "ls-tree" | "rev-parse" | "describe" | "shortlog" | "cat-file" | "show-ref"
        | "rev-list" | "count-objects" | "whatchanged" | "name-rev" | "merge-base"
        | "for-each-ref" | "version" | "show-branch" | "cherry" | "range-diff" | "check-ignore"
        | "check-attr" | "var" | "diff-tree" | "diff-files" | "diff-index" | "verify-commit"
        | "verify-tag" | "fsck" => None,
        "branch" => {
            if a("-d") || a("-D") || a("--delete") {
                return Some(Avis::ecriture(
                    Genre::Suppression,
                    "`git branch -d` supprime une branche",
                ));
            }
            let lecture = [
                "-a",
                "-r",
                "-v",
                "-vv",
                "--list",
                "-l",
                "--all",
                "--remotes",
                "--show-current",
                "--contains",
                "--no-contains",
                "--merged",
                "--no-merged",
                "--points-at",
                "--sort",
                "--format",
                "--color",
                "--no-color",
                "--column",
                "--no-column",
                "-i",
                "--ignore-case",
            ];
            let options_ok = reste.iter().filter(|x| x.starts_with('-')).all(|x| {
                lecture
                    .iter()
                    .any(|o| x == o || x.starts_with(&format!("{o}=")))
            });
            let filtre = a("--list")
                || a("-l")
                || a("--contains")
                || a("--merged")
                || a("--no-merged")
                || a("--points-at")
                || a("--no-contains");
            if options_ok && (positionnels() == 0 || filtre) {
                None
            } else {
                ecrit("`git branch` crée, renomme ou déplace une branche".into())
            }
        }
        "tag" => {
            if a("-d") || a("--delete") {
                return Some(Avis::ecriture(
                    Genre::Suppression,
                    "`git tag -d` supprime une étiquette",
                ));
            }
            if positionnels() == 0 || a("-l") || a("--list") {
                None
            } else {
                ecrit("`git tag <nom>` crée une étiquette".into())
            }
        }
        "remote" => match reste
            .iter()
            .find(|x| !x.starts_with('-'))
            .map(String::as_str)
        {
            None | Some("show" | "get-url") => None,
            _ => ecrit("`git remote` modifie les dépôts distants".into()),
        },
        "stash" => match reste.first().map(String::as_str) {
            Some("list" | "show") => None,
            _ => ecrit("`git stash` modifie le dépôt (seuls list et show sont en lecture)".into()),
        },
        "worktree" => match reste.first().map(String::as_str) {
            Some("list") => None,
            _ => ecrit("`git worktree` modifie les arbres de travail".into()),
        },
        "reflog" => match reste.first().map(String::as_str) {
            None | Some("show") => None,
            Some(x) if x.starts_with('-') => None,
            _ => ecrit("`git reflog expire/delete` modifie l'historique".into()),
        },
        "config" => {
            let lecture = reste.iter().any(|x| {
                matches!(
                    x.as_str(),
                    "--get" | "--get-all" | "--get-regexp" | "--list" | "-l" | "get"
                )
            }) || (reste.first().is_some_and(|x| x == "list"));
            if lecture {
                None
            } else {
                ecrit("`git config` modifie la configuration".into())
            }
        }
        "clean" | "rm" => Some(Avis::ecriture(
            Genre::Suppression,
            format!("`git {sous}` supprime des fichiers"),
        )),
        _ => ecrit(format!("`git {sous}` modifie le dépôt")),
    }
}

/// Tous les chemins des arguments doivent rester dans les dossiers déclarés.
fn chemins(nom: &str, args: &[String], shell: Shell, ctx: &Contexte, cwd: &Path) -> Avis {
    if SANS_CHEMIN_PS.contains(&nom) {
        return Avis::lecture();
    }
    let ps = shell == Shell::PowerShell;
    // Lecteurs PowerShell (`Env:`, `Variable:`, `HKLM:`…) : pas des dossiers
    // (KOB-227, `Get-Content Env:ANTHROPIC_API_KEY`).
    if ps {
        if let Some(x) = args.iter().find(|x| lecteur_ps(x)) {
            return Avis::refus(format!("{x} est hors des dossiers de code déclarés"));
        }
    }
    // Motif donné par une option (`grep /etc/passwd -e root`) : tous les
    // positionnels sont des chemins (KOB-227).
    let mut motif_vu = !MOTIF_D_ABORD.contains(&nom) || motif_par_option(nom, args);
    let mut i = 0;
    while i < args.len() {
        let x = &args[i];
        let bas = x.to_lowercase();
        if x.starts_with('-') && x.len() > 1 {
            // `--git-dir=/x`, `-Path=…`, PowerShell `-Path:/x` : la valeur est
            // vérifiée si c'est un chemin.
            let valeur = x
                .split_once('=')
                .or_else(|| if ps { x.split_once(':') } else { None });
            if let Some((_, v)) = valeur {
                if ressemble_a_un_chemin(v) && !ctx.contient(Path::new(v), Some(cwd)) {
                    return Avis::refus(format!("{v} est hors des dossiers de code déclarés"));
                }
            }
            if nom == "git" && x == "-C" {
                if let Some(d) = args.get(i + 1) {
                    if !ctx.contient(Path::new(d), Some(cwd)) {
                        return Avis::refus(format!("{d} est hors des dossiers de code déclarés"));
                    }
                }
                i += 2;
                continue;
            }
            match option_de_motif(nom, x) {
                Some(true) => {
                    // La valeur est le motif : on la saute.
                    motif_vu = true;
                    i += 2;
                    continue;
                }
                Some(false) => {
                    // Fichier de motifs : vérifié comme les autres chemins.
                    motif_vu = true;
                    i += 1;
                    continue;
                }
                None => {}
            }
            if options_a_valeur(nom)
                .iter()
                .any(|o| o.eq_ignore_ascii_case(&bas))
                && !x.contains('=')
            {
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        if !motif_vu {
            motif_vu = true;
            i += 1;
            continue;
        }
        if !ctx.contient(Path::new(x), Some(cwd)) {
            return Avis::refus(format!("{x} est hors des dossiers de code déclarés"));
        }
        i += 1;
    }
    Avis::lecture()
}

/// Le motif est-il donné par une option (`-e`, `-ne`, `--regexp=`, `-f`) ?
fn motif_par_option(nom: &str, args: &[String]) -> bool {
    let grep = matches!(nom, "grep" | "egrep" | "fgrep" | "rg");
    args.iter().take_while(|x| *x != "--").any(|x| {
        let bas = x.to_lowercase();
        option_de_motif(nom, x).is_some()
            || bas.starts_with("--regexp=")
            || (grep && bas.starts_with("--file="))
            || (grep
                && x.len() > 1
                && x.starts_with('-')
                && !x.starts_with("--")
                && x[1..].contains(['e', 'f']))
    })
}

/// Lecteur PowerShell qui n'est pas un disque (`Env:x`, `HKLM:\x`,
/// `Variable:`) : un nom de deux lettres ou plus suivi de `:`.
fn lecteur_ps(x: &str) -> bool {
    let x = x
        .split_once(':')
        .map(|(_, v)| v)
        .filter(|_| x.starts_with('-'))
        .unwrap_or(x);
    x.split_once(':')
        .is_some_and(|(nom, _)| nom.len() >= 2 && nom.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// Valeur d'option qui désigne un fichier (`/x`, `C:\x`, `../x`, `~/x`).
fn ressemble_a_un_chemin(v: &str) -> bool {
    v.contains('/') || v.contains('\\') || v.starts_with('~') || v.starts_with('.')
}

#[cfg(test)]
mod tests {
    use super::*;

    const KOBI: &str = "C:/Users/moi/Desktop/kobi";

    fn ctx() -> Contexte {
        let mut c = Contexte::new(vec![
            PathBuf::from(KOBI),
            PathBuf::from("C:/Users/moi/Desktop/site"),
        ]);
        c.cwd = Some(PathBuf::from(KOBI));
        c
    }

    fn bash(ligne: &str) -> Avis {
        analyser(ligne, Shell::Bash, &ctx())
    }

    fn ps(ligne: &str) -> Avis {
        analyser(ligne, Shell::PowerShell, &ctx())
    }

    #[track_caller]
    fn lecture(avis: Avis, ligne: &str) {
        assert!(
            avis.est_lecture(),
            "« {ligne} » devrait être une lecture : {}",
            avis.raison
        );
    }

    #[track_caller]
    fn pas_lecture(avis: Avis, ligne: &str) {
        assert!(
            !avis.est_lecture(),
            "« {ligne} » ne devrait pas être une lecture"
        );
        assert!(!avis.raison.is_empty(), "« {ligne} » : raison vide");
    }

    #[test]
    fn lectures_bash_acceptees() {
        for ligne in [
            "git log --oneline -5",
            "git log -5 --format='%h %s'",
            "git status",
            "git status --short",
            "git show HEAD~1 --stat",
            "git diff main...HEAD -- crates/kobi-app",
            "git blame -L 10,20 crates/kobi-app/src/chef.rs",
            "git branch",
            "git branch -a -v",
            "git branch --list 'feat/*'",
            "git branch --show-current",
            "git grep -n fabrique_claude",
            "git ls-files crates",
            "git -C ../site log -3",
            "git --no-pager log -1",
            "git tag",
            "git remote -v",
            "git stash list",
            "git worktree list",
            "git config --get user.name",
            "ls -la",
            "ls crates/kobi-app/src",
            "cat Cargo.toml",
            "head -n 40 crates/kobi-app/src/main.rs",
            "tail -20 README.md",
            "wc -l crates/kobi-app/src/*.rs",
            "rg -n 'fn fabrique_claude' crates",
            "rg fabrique_claude",
            "grep -rn \"TODO\" crates --include=*.rs",
            "find . -name '*.rs' -type f",
            "find crates -maxdepth 2 -name Cargo.toml",
            "tree -L 2",
            "sed -n '1,80p' crates/kobi-app/src/chef.rs",
            "sort Cargo.toml | uniq -c",
            "git log --oneline | head -5",
            "cd crates && ls",
            "cd /c/Users/moi/Desktop/site && git status",
            "ls; git status",
            "git status && git log -1 || echo rien",
            "cargo metadata --format-version 1 --no-deps",
            "cargo tree -p kobi-app",
            "npm ls --depth 0",
            "gh pr view 103",
            "gh pr list --state open",
            "gh issue view 12 --json title",
            "git log -1 2>&1",
            "ls nope 2>/dev/null",
            "echo bonjour",
            "pwd",
            "du -sh crates",
            "rustc --version",
            "go version",
            "xxd -l 64 Cargo.toml",
            "fd -e rs motif crates",
            "cat < Cargo.toml",
            // KOB-227 : ce qui reste permis.
            "cd crates && cat ../Cargo.toml",
            "cd crates; ls",
            "ls {}",
            "grep -e motif Cargo.toml",
            "git show HEAD:Cargo.toml",
            "git log -3 # les trois derniers",
            "ls \"crates/kobi-app\"",
            "ls crates\\ avec\\ espace",
        ] {
            lecture(bash(ligne), ligne);
        }
    }

    /// KOB-227 : chaque commande du tableau du ticket, telle quelle, donne
    /// une carte ou un refus, jamais une lecture.
    #[test]
    fn tableau_de_kob_227_jamais_en_lecture() {
        for ligne in [
            "fd -x rm -rf {}",
            "fd -X rm",
            "fd --exec rm",
            "sed -n 1p -e '1e touch x' f",
            "sed -n 1p --expression='1e touch x' f",
            "sed -n 1p -f script f",
            "git grep -O'sh -c id' motif",
            "go run . -v",
            "java -jar x.jar -v",
            "dotnet run -v q",
            "printf -v 'a[$(id>x)]' 1",
            "xxd -r -p - .git/config",
            "sort -o/chemin f",
            "sort -uo f g",
            "tree -ao out.txt",
            "cargo --config build.rustc-wrapper=./x tree",
            "cargo -Zunstable-options tree",
            "rg --hostname-bin ./x motif",
            // KOB-258 : guillemets dans une expansion d'accolades, `sort` qui
            // lance un programme ou écrit ailleurs.
            "find . {\"-delete\",}",
            // KOB-272 : un guillemet échappé n'ouvre ni ne ferme rien.
            "find . {\"\\\"\",-delete}",
            "find . {\\\",-delete}",
            "sed -n 1p {\"\\\"\",-i,Cargo.toml}",
            "rg {\"\\\"\",--pre=sh} . crates",
            "git log {\\\",--output=x.txt}",
            "find . {\\',-delete}",
            "find . {\"\\'\",-delete}",
            "fd {\\',-x,rm} motif",
            "sed -n 1p {\"-i\",Cargo.toml}",
            "git log {\"--output=x.txt\",}",
            "fd {'-x',rm} motif",
            "rg {\"--pre=sh\",} . crates",
            "sort -S 1 --compress-program=sh f",
            "sort --comp=./outil f",
            "sort -T /tmp f",
            "sort --temporary-directory=/tmp f",
            // KOB-286 : accolades imbriquées, que bash apparie par niveau.
            "find . {{},-delete}",
            "find . {a{b},-delete}",
            "find . {{a,b},-delete}",
            "sed -n 1p {{},-i,Cargo.toml}",
            "fd {{},-x,rm} motif",
            "rg {{},--pre=sh} . crates",
            "git log {{},--output=x.txt}",
            "find . {{\"\\\"\"},-delete}",
            "cat {{a..c},../../.ssh/id_rsa}",
        ] {
            pas_lecture(bash(ligne), ligne);
        }
    }

    #[test]
    fn ecritures_bash_refusees() {
        for (ligne, genre) in [
            ("rm -rf target", Genre::Suppression),
            ("rm target/x", Genre::Suppression),
            ("rmdir vide", Genre::Suppression),
            ("find . -name '*.tmp' -delete", Genre::Suppression),
            ("git clean -fdx", Genre::Suppression),
            ("git branch -D vieille", Genre::Suppression),
            ("git rm Cargo.lock", Genre::Suppression),
            ("git checkout main", Genre::Commande),
            ("git reset --hard HEAD~1", Genre::Commande),
            ("git commit -m x", Genre::Commande),
            ("git push --force", Genre::Commande),
            ("git pull", Genre::Commande),
            ("git branch nouvelle", Genre::Commande),
            ("git tag v2", Genre::Commande),
            ("git config user.name moi", Genre::Commande),
            ("git stash", Genre::Commande),
            ("git -c core.pager=calc log", Genre::Commande),
            ("git log --output=log.txt", Genre::Commande),
            ("git diff --ext-diff", Genre::Commande),
            ("find . -exec rm {} ;", Genre::Commande),
            ("cargo build", Genre::Commande),
            ("cargo test", Genre::Commande),
            ("npm install", Genre::Commande),
            ("gh pr merge 103", Genre::Commande),
            ("gh api repos/x/y -X DELETE", Genre::Commande),
            ("gh pr view 103 --web", Genre::Commande),
            ("touch x", Genre::Commande),
            ("mkdir x", Genre::Commande),
            ("mv a b", Genre::Commande),
            ("cp a b", Genre::Commande),
            ("chmod +x a", Genre::Commande),
            ("curl https://exemple.org", Genre::Commande),
            ("sed -i 's/a/b/' Cargo.toml", Genre::Fichier),
            ("sed 's/a/b/w out' Cargo.toml", Genre::Commande),
            ("sort -o trie.txt Cargo.toml", Genre::Fichier),
            ("rg --pre ./script fabrique", Genre::Commande),
            ("awk 'BEGIN{system(\"rm x\")}'", Genre::Commande),
            ("xargs rm < liste", Genre::Commande),
            ("fd -x rm", Genre::Commande),
            ("fd -HX rm", Genre::Commande),
            ("fd --exec=rm motif", Genre::Commande),
            ("fd --exec-batch rm", Genre::Commande),
            ("fd -l", Genre::Commande),
            ("sed -n 1p -e 'w sortie' Cargo.toml", Genre::Commande),
            ("sed -ne 'w sortie' Cargo.toml", Genre::Commande),
            (
                "sed -n 1p --expression='w sortie' Cargo.toml",
                Genre::Commande,
            ),
            ("sed -n 1p --exp='w sortie' Cargo.toml", Genre::Commande),
            ("sed -n 1p -f script.sed Cargo.toml", Genre::Commande),
            ("sed -n 1p --file=script.sed Cargo.toml", Genre::Commande),
            ("git grep -Ocalc motif", Genre::Commande),
            ("git grep -iOcalc motif", Genre::Commande),
            ("git grep --open motif", Genre::Commande),
            ("go run -v main.go", Genre::Commande),
            ("rustc -V main.rs", Genre::Commande),
            ("printf -v PATH x", Genre::Commande),
            ("xxd Cargo.toml sortie.hex", Genre::Fichier),
            ("xxd -l 64 Cargo.toml sortie.hex", Genre::Fichier),
            ("sort -osortie.txt Cargo.toml", Genre::Fichier),
            ("sort -uo sortie.txt Cargo.toml", Genre::Fichier),
            ("sort --out=sortie.txt Cargo.toml", Genre::Fichier),
            ("tree -ao arbre.txt", Genre::Fichier),
            ("tree -oarbre.txt", Genre::Fichier),
            ("cargo --config alias.tree=build tree", Genre::Commande),
            ("cargo --config=alias.tree=build tree", Genre::Commande),
            ("cargo tree -Zunstable-options", Genre::Commande),
            ("rg --hostname-bin=./script motif", Genre::Commande),
            ("sed --in-pl=.bak 's/a/b/' Cargo.toml", Genre::Fichier),
            ("git log --outp=log.txt", Genre::Commande),
            ("git diff --ext", Genre::Commande),
            ("tree -R -H . -L 1", Genre::Fichier),
        ] {
            let avis = bash(ligne);
            pas_lecture(avis.clone(), ligne);
            assert_eq!(
                avis.classe,
                Classe::Ecriture(genre),
                "« {ligne} » : {}",
                avis.raison
            );
        }
    }

    #[test]
    fn contournements_bash_refuses() {
        for ligne in [
            // Enchaînements avec une commande qui n'est pas une lecture.
            "ls; rm -rf target",
            "ls && rm -rf target",
            "ls || rm -rf target",
            "git status | xargs rm",
            "cat Cargo.toml | tee copie.txt",
            "ls\nrm -rf target",
            "ls & rm -rf target",
            // Substitutions, variables, sous-shells.
            "ls $(rm -rf target)",
            "ls `rm -rf target`",
            "echo \"$(rm -rf target)\"",
            "cat $HOME/.ssh/id_rsa",
            "echo \"$PATH\"",
            "(rm -rf target)",
            "ls <(rm x)",
            // Redirections vers un fichier.
            "echo x > Cargo.toml",
            "git log >> log.txt",
            "ls 2> erreurs.txt",
            "ls &> tout.txt",
            "cat <<EOF",
            // Guillemets qui cachent le nom de la commande.
            "'rm' -rf target",
            "\"rm\" -rf target",
            "r\\m -rf target",
            // Chemins absolus et sorties des dossiers.
            "/bin/rm -rf target",
            "./script.sh",
            "C:/Windows/System32/cmd.exe /c del x",
            "cat /etc/passwd",
            "cat C:/Users/moi/.claude/.credentials.json",
            "cat ../../.ssh/id_rsa",
            "cat ~/.ssh/id_rsa",
            "ls C:\\Windows",
            "cd .. && ls",
            "cd / && ls",
            "cd",
            "git -C C:/Users/moi/autre log",
            "git --git-dir=C:/autre/.git log",
            "rg secret C:/Users/moi/Documents",
            "grep -r motdepasse /c/Users/moi",
            // KOB-227 : `cd` qui peut échouer, accolades, motif par option.
            "cd crates/kobi-app; cat ../../.ssh/id_rsa",
            "cd crates/kobi-app || cat ../../.ssh/id_rsa",
            "cd crates/kobi-app | cat ../../.ssh/id_rsa",
            "cat {Cargo.toml,../../.ssh/id_rsa}",
            "cat crates/{a..c}",
            "grep /etc/passwd -e root",
            "grep /etc/passwd -eroot",
            "grep /etc/passwd --regexp=root",
            "grep -ie root /etc/passwd",
            // Shells imbriqués.
            "bash -c 'rm -rf target'",
            "sh -c ls",
            "cmd /c del x",
            "powershell -Command Remove-Item x",
            "python -c 'import os'",
            "env rm x",
            "FOO=1 ls",
            "sudo ls",
            // Lignes illisibles.
            "ls 'non fermé",
            "ls \"non fermé",
            "ls |",
        ] {
            pas_lecture(bash(ligne), ligne);
        }
    }

    #[test]
    fn hors_des_dossiers_est_un_refus() {
        for ligne in [
            "cat /etc/passwd",
            "ls C:/Windows",
            "cd .. && ls",
            "git -C C:/autre log",
        ] {
            assert!(matches!(bash(ligne).classe, Classe::Refus(_)), "{ligne}");
        }
        let mut dehors = ctx();
        dehors.cwd = Some(PathBuf::from("C:/Windows"));
        assert!(matches!(
            analyser("ls", Shell::Bash, &dehors).classe,
            Classe::Refus(_)
        ));
        let vide = Contexte::default();
        assert!(matches!(
            analyser("ls", Shell::Bash, &vide).classe,
            Classe::Refus(_)
        ));
    }

    #[test]
    fn powershell_lecture_acceptee() {
        for ligne in [
            "Get-ChildItem crates -Recurse -Filter *.rs",
            "Get-ChildItem",
            "gci -Name",
            "Get-Content Cargo.toml -TotalCount 20",
            "Get-Content -Path crates/kobi-app/src/chef.rs -Tail 30",
            "Select-String -Pattern fabrique_claude -Path crates/kobi-app/src/*.rs",
            "Select-String fabrique_claude crates/kobi-app/src/chef.rs",
            "Get-ChildItem crates | Select-Object Name, Length",
            "Get-ChildItem -Recurse -Include *.rs | Measure-Object",
            "Get-ChildItem | Sort-Object Length -Descending | Select-Object -First 5",
            "Test-Path Cargo.toml",
            "Get-Location",
            "Resolve-Path crates",
            "Get-FileHash Cargo.lock",
            "git log --oneline -5",
            "git status; git log -1",
            "Get-Content Cargo.toml | Out-String",
            "Get-Item 'crates/kobi-app'",
            "Get-ChildItem 2>&1",
        ] {
            lecture(ps(ligne), ligne);
        }
    }

    #[test]
    fn powershell_ecriture_et_contournements_refuses() {
        for ligne in [
            "Remove-Item -Recurse -Force target",
            "rm -r target",
            "del Cargo.lock",
            "ri target",
            "Set-Content Cargo.toml x",
            "Add-Content README.md x",
            "Out-File x.txt",
            "New-Item -ItemType File x",
            "Copy-Item a b",
            "Move-Item a b",
            "Rename-Item a b",
            "Clear-Content Cargo.toml",
            "Start-Process notepad",
            "Invoke-Expression 'rm x'",
            "iex 'rm x'",
            "Invoke-WebRequest https://exemple.org -OutFile x",
            "Get-ChildItem | ForEach-Object { Remove-Item $_ }",
            "Get-ChildItem | Where-Object { $_.Length -gt 0 }",
            "Get-Content $env:USERPROFILE\\.ssh\\id_rsa",
            "Get-Content (Get-Item x)",
            "Get-ChildItem @(1)",
            "& rm x",
            "Get-ChildItem > liste.txt",
            "Get-ChildItem *> tout.txt",
            "Get-Content C:\\Windows\\win.ini",
            "Get-ChildItem C:/Users/moi/.ssh",
            "Get-Content ..\\..\\secret.txt",
            "Set-Location C:\\Windows; Get-ChildItem",
            "Get-ChildItem; Remove-Item x",
            "Get-ChildItem | Tee-Object x.txt",
            "cmd /c dir",
            "powershell -c ls",
            "Get-ChildItem -OutVariable x",
            "Stop-Process -Name kobi",
        ] {
            pas_lecture(ps(ligne), ligne);
        }
        assert_eq!(
            ps("Remove-Item target").classe,
            Classe::Ecriture(Genre::Suppression)
        );
    }

    #[test]
    fn decoupage() {
        assert_eq!(
            decouper("git log -5 --format='%h %s' && ls", Shell::Bash).unwrap(),
            vec![
                Jeton::Mot("git".into()),
                Jeton::Mot("log".into()),
                Jeton::Mot("-5".into()),
                Jeton::Mot("--format=%h %s".into()),
                Jeton::Op("&&".into()),
                Jeton::Mot("ls".into()),
            ]
        );
        assert_eq!(
            decouper("ls 2>&1 >/dev/null", Shell::Bash).unwrap(),
            vec![
                Jeton::Mot("ls".into()),
                Jeton::Redir("2>&1".into()),
                Jeton::Redir(">".into()),
                Jeton::Mot("/dev/null".into()),
            ]
        );
        assert_eq!(
            decouper("Get-Content 'l''été'", Shell::PowerShell).unwrap(),
            vec![Jeton::Mot("Get-Content".into()), Jeton::Mot("l'été".into())]
        );
        assert!(decouper("ls $(id)", Shell::Bash).is_err());
    }

    #[test]
    fn options_qui_ne_cachent_pas_un_chemin() {
        // `-n` ne prend pas de valeur pour grep/rg : le chemin reste vérifié.
        for ligne in [
            "rg -n motif /etc",
            "grep -n motif /etc/passwd",
            "grep -rn motif C:/Users/moi/Documents",
            "grep -f /etc/motifs crates",
            "grep -rf /etc/motifs crates",
            "rg -e motif C:/Windows",
            "type C:/Users/moi/.ssh/id_rsa",
            "head -n 5 /etc/passwd",
            "sed -n 1p /etc/passwd",
            "jq . C:/Users/moi/secrets.json",
        ] {
            assert!(
                matches!(bash(ligne).classe, Classe::Refus(_)),
                "« {ligne} » : {:?}",
                bash(ligne)
            );
        }
        let ligne = "Select-String -Pattern x -Path C:/Windows/win.ini";
        assert!(
            matches!(ps(ligne).classe, Classe::Refus(_)),
            "« {ligne} » : {:?}",
            ps(ligne)
        );
        lecture(bash("grep -e motif crates"), "grep -e motif crates");
        lecture(bash("rg -n -e motif crates"), "rg -n -e motif crates");
        lecture(bash("jq .name package.json"), "jq .name package.json");
        lecture(bash("cut -d , -f 1 Cargo.toml"), "cut -d , -f 1 Cargo.toml");
        for ligne in ["ls |", "| ls", "ls && && ls"] {
            pas_lecture(bash(ligne), ligne);
        }
    }

    /// Cas de test écrits par kobi-voix pour son propre validateur (commit
    /// 089d69c, KOB-154), repris tels quels : même sémantique (lecture,
    /// écriture, suppression, refus), avec une cale vers [`analyser`].
    mod cas_kobi_voix {
        use super::super::*;
        use std::path::PathBuf;

        #[derive(Debug, PartialEq)]
        enum Verdict {
            Lecture,
            Ecriture { suppression: bool },
            Refus,
        }

        #[derive(Clone, Copy)]
        enum Interprete {
            Bash,
            PowerShell,
        }

        fn juger(
            ligne: &str,
            i: Interprete,
            dossiers: &[PathBuf],
            _courant: Option<&Path>,
        ) -> Verdict {
            let shell = match i {
                Interprete::Bash => Shell::Bash,
                Interprete::PowerShell => Shell::PowerShell,
            };
            match analyser(ligne, shell, &Contexte::new(dossiers.to_vec())).classe {
                Classe::Lecture => Verdict::Lecture,
                Classe::Ecriture(Genre::Suppression) => Verdict::Ecriture { suppression: true },
                Classe::Ecriture(_) => Verdict::Ecriture { suppression: false },
                Classe::Refus(_) => Verdict::Refus,
            }
        }

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

        /// Écrit (ou supprime : la suppression est une écriture plus grave).
        #[track_caller]
        fn ecrit(v: Verdict, ligne: &str) {
            assert!(matches!(v, Verdict::Ecriture { .. }), "{ligne} : {v:?}");
        }

        #[track_caller]
        fn supprime(v: Verdict, ligne: &str) {
            assert!(
                matches!(v, Verdict::Ecriture { suppression: true }),
                "{ligne} : {v:?}"
            );
        }

        #[track_caller]
        fn refuse(v: Verdict, ligne: &str) {
            assert!(matches!(v, Verdict::Refus), "{ligne} : {v:?}");
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
                // KOB-227 : lecteurs PowerShell hors des dossiers.
                "Get-Content Env:ANTHROPIC_API_KEY",
                "Get-ChildItem Env:",
                "Get-Content -Path:Env:ANTHROPIC_API_KEY",
                "Get-ChildItem HKLM:\\Software",
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
            // `-Path:valeur` : la valeur est vérifiée comme un chemin (KOB-227).
            refuse(
                ps(&format!("Get-Content -Path:{}", abs("/Windows/win.ini"))),
                "Get-Content -Path: hors dossiers",
            );
        }

        #[test]
        fn sans_dossier_tout_est_refuse() {
            assert!(matches!(
                juger("ls", Interprete::Bash, &[], None),
                Verdict::Refus
            ));
        }
    }
}
