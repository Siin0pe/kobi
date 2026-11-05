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
        // Deux écritures : la classe la plus grave, et les deux raisons, la
        // plus grave en tête, pour que la carte ne montre pas seulement la
        // plus anodine (`touch x; bash -c …`, `rm a; bash -c …`, relecture
        // de KOB-483). Un refus garde sa raison seule.
        let ecritures = matches!(self.classe, Classe::Ecriture(_))
            && matches!(autre.classe, Classe::Ecriture(_));
        let (grave, autre) = if rang(&autre) > rang(&self) {
            (autre, self)
        } else {
            (self, autre)
        };
        if !ecritures || autre.raison.is_empty() || grave.raison.contains(&autre.raison) {
            return grave;
        }
        Self {
            raison: if grave.raison.is_empty() {
                autre.raison
            } else {
                format!("{} ; {}", grave.raison, autre.raison)
            },
            ..grave
        }
    }

    pub fn est_lecture(&self) -> bool {
        self.classe == Classe::Lecture
    }
}

/// Classe une ligne de commande.
pub fn analyser(ligne: &str, shell: Shell, ctx: &Contexte) -> Avis {
    let avis = analyser_sans_joker(ligne, shell, ctx);
    if avis.classe != Classe::Lecture {
        return avis;
    }
    // Un joker en tête de mot peut devenir une option : un fichier nommé
    // `--pre=x` fait de `rg a *` un `rg a --pre=x` (KOB-489). Jugé après le reste,
    // pour ne jamais affaiblir un refus, et sans lire le disque : la même ligne peut
    // créer le fichier (`touch -- --pre=x; rg a *`).
    let joker = match decouper_en_notant(ligne, shell) {
        Ok((_, Some(mot))) => Some(mot),
        Ok((jetons, None)) if shell == Shell::PowerShell && !cfg!(windows) => {
            joker_pour_un_natif_pwsh(&jetons)
        }
        _ => None,
    };
    match joker {
        Some(mot) => Avis::ecriture(
            Genre::Commande,
            format!("le joker `{mot}` peut devenir une option selon les noms de fichiers"),
        ),
        None => avis,
    }
}

fn analyser_sans_joker(ligne: &str, shell: Shell, ctx: &Contexte) -> Avis {
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
    // Mots dont `decouper` a mis le `./` de tête (valeur PowerShell, KOB-447).
    let mut cites: Vec<bool> = Vec::new();
    // La commande en cours reçoit-elle l'entrée d'un `|` ?
    let mut apres_pipe = false;
    // Nom de la commande précédente, celle dont la sortie arrive par le `|`.
    let mut amont = String::new();
    // Dans le pipeline en cours (PowerShell) : un amont qui donne des objets
    // aux propriétés libres (`Path`, `PSPath`…), et les listes de fichiers
    // (`gci src`) avec leurs dossiers courants (KOB-438).
    let mut source_libre = false;
    let mut listes: Vec<(String, Vec<String>, Vec<PathBuf>)> = Vec::new();
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
                    // Sous Windows, un parcours récursif qui reçoit ses
                    // chemins d'un `|` (`gci ..\autre | gci -Recurse`) part
                    // de dossiers qu'on ne voit pas ici : sur la carte
                    // (KOB-434).
                    let nom = mots[0].to_lowercase();
                    let nom = nom.strip_suffix(".exe").unwrap_or(&nom);
                    if cfg!(windows) && apres_pipe && parcours_recursif(nom, &mots[1..], shell) {
                        avis = avis.pire(Avis::ecriture(
                            Genre::Commande,
                            "parcours récursif sur des chemins venus d'un `|` : sur la carte",
                        ));
                    }
                    // PowerShell : une cmdlet qui change en fichier une chaîne
                    // reçue par le `|` (`echo C:\x | Get-Item | Get-Content`)
                    // lit un chemin qu'on n'a pas jugé, sauf si l'amont ne
                    // donne que des chemins déjà jugés (KOB-438).
                    if shell == Shell::PowerShell
                        && apres_pipe
                        && CHEMIN_PAR_LE_PIPE.contains(&nom)
                        && !AMONT_SUR.contains(&amont.as_str())
                    {
                        avis = avis.pire(Avis::ecriture(
                            Genre::Commande,
                            format!("`{nom}` prend ses chemins d'un `|` : sur la carte"),
                        ));
                    }
                    // Une cmdlet qui lit un chemin prend aussi `Path`,
                    // `LiteralPath` ou `PSPath` des objets reçus (ByPropertyName) :
                    // venus de `ConvertFrom-Json`, ils sont quelconques ; venus de
                    // `gci src`, ce sont ses fichiers, liens compris, qu'elle
                    // suivrait (`gci src | gc`) : l'arbre est vérifié (KOB-438).
                    if !apres_pipe {
                        source_libre = false;
                        listes.clear();
                    }
                    if shell == Shell::PowerShell && apres_pipe && LIT_PAR_LE_PIPE.contains(&nom) {
                        if source_libre {
                            avis = avis.pire(Avis::ecriture(
                                Genre::Commande,
                                format!("`{nom}` lit les chemins d'objets venus d'un `|` : sur la carte"),
                            ));
                        }
                        // Un seul niveau suffit pour un `gci` sans `-Recurse`
                        // lu par une cmdlet qui ne liste pas ; tout l'arbre si
                        // une autre liste descend dedans (`gci src | gci | gc`).
                        let enchaine = listes.len() > 1 || AMONT_SUR.contains(&nom);
                        for (liste, args, dossiers) in &listes {
                            let profond = enchaine || parcours_recursif(liste, args, shell);
                            for c in dossiers {
                                let verdict =
                                    racines_du_parcours(liste, args, ctx, c).and_then(|r| {
                                        if profond {
                                            arbre_sans_lien_qui_sort(ctx, &r, c)
                                        } else {
                                            liste_sans_lien_qui_sort(ctx, args, &r, c)
                                        }
                                    });
                                if let Err(a) = verdict {
                                    avis = avis.pire(a);
                                }
                            }
                        }
                    }
                    // `Get-Command` livre le texte d'un script (`ScriptContents`,
                    // sonde #411), même trouvé dans le PATH par un nom nu
                    // (`gcm npm`) ou un joker. Seul en bout de chaîne, il
                    // n'affiche que le type, le nom et la source : lu par un
                    // `|`, ou recevant des noms par un `|`, sur la carte
                    // (KOB-459).
                    if shell == Shell::PowerShell
                        && matches!(nom, "get-command" | "gcm")
                        && (apres_pipe || op == "|")
                    {
                        avis = avis.pire(Avis::ecriture(
                            Genre::Commande,
                            format!("`{nom}` dans un `|` peut livrer le texte d'un script : sur la carte"),
                        ));
                    }
                    if shell == Shell::PowerShell {
                        source_libre |= SOURCE_LIBRE.contains(&nom);
                        if AMONT_SUR.contains(&nom) {
                            listes.push((nom.to_owned(), mots[1..].to_vec(), cwds.clone()));
                        }
                    }
                    amont = nom.to_owned();
                    let mut suivants = Vec::new();
                    for c in &cwds {
                        let mut nouveau = c.clone();
                        avis = avis.pire(commande(&mots, &cites, shell, ctx, &mut nouveau));
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
                    cites.clear();
                    apres_pipe = op == "|";
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
                    Some(Jeton::Mot(m) | Jeton::Cite(m))
                        if !r.contains('&') || r.ends_with('>') =>
                    {
                        Some(m.clone())
                    }
                    _ => None,
                };
                let mut saute = 0;
                for c in &cwds {
                    match redirection(r, cible.as_deref(), shell, ctx, c) {
                        Ok(n) => saute = n,
                        Err(a) => return avis.pire(a),
                    }
                }
                i += 1 + saute;
            }
            Some(Jeton::Mot(m)) => {
                mots.push(m.clone());
                cites.push(false);
                i += 1;
            }
            Some(Jeton::Cite(m)) => {
                mots.push(m.clone());
                cites.push(true);
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
    /// Valeur PowerShell entre guillemets ou élément de tableau qui commence
    /// par un tiret : `decouper` y a mis un `./` de tête (KOB-437), qu'un
    /// exécutable ne reçoit pas (KOB-447).
    Cite(String),
    /// `;`, `&&`, `||`, `|`, `&`, saut de ligne, `(`, `)`.
    Op(String),
    /// `>`, `>>`, `2>`, `2>&1`, `<`, `<<`, `*>`…
    Redir(String),
}

/// Le mot commence-t-il par un tiret de paramètre PowerShell (`-`, `–`,
/// `—`, `―`) ?
fn commence_par_un_tiret(mot: &str) -> bool {
    mot.starts_with(['-', '\u{2013}', '\u{2014}', '\u{2015}'])
}

/// Paramètre dont le nom n'est pas fermé par `:` ou `=` (`-Recurse`, et non
/// `--format=%h` ni `-Path:a.txt`) : des guillemets qui le suivent rendraient
/// son sens ambigu (KOB-437).
fn parametre_ouvert(mot: &str) -> bool {
    commence_par_un_tiret(mot) && !mot.contains([':', '='])
}

/// PowerShell coupe un mot qui commence par un guillemet juste après le
/// guillemet fermant (`'x'/R` donne `x` puis `/R`, sonde #402) : le
/// caractère qui suit le guillemet fermé en `i` en fait-il un second
/// argument, au lieu de finir le mot (KOB-441) ?
fn suite_collee(c: &[char], i: usize) -> bool {
    c.get(i + 1).is_some_and(|x| !" \t\n\r,;|&<>)".contains(*x))
}

/// Découpe la ligne en jetons. Refuse ce qui demanderait d'exécuter du code
/// pour connaître la commande : substitutions, variables, blocs.
fn decouper(ligne: &str, shell: Shell) -> Result<Vec<Jeton>, String> {
    decouper_en_notant(ligne, shell).map(|(jetons, _)| jetons)
}

/// `decouper`, plus le premier mot Bash dont un joker non cité peut donner une
/// option (`joker_peut_donner_une_option`). Seul ce découpage voit les guillemets.
fn decouper_en_notant(ligne: &str, shell: Shell) -> Result<(Vec<Jeton>, Option<String>), String> {
    let ps = shell == Shell::PowerShell;
    let echap = if ps { '`' } else { '\\' };
    let mut joker = None;
    let mut jetons = Vec::new();
    let mut mot = String::new();
    let mut dans_mot = false;
    let brut: Vec<char> = ligne.chars().collect();
    // PowerShell prend aussi `‘ ’ ‚ ‛` pour `'` et `“ ” „` pour `"` : le
    // découpage suit ces délimiteurs, le mot garde le caractère tapé (le
    // vrai nom du fichier) (KOB-441).
    let c: Vec<char> = brut
        .iter()
        .map(|&x| match x {
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' if ps => '\'',
            '\u{201C}' | '\u{201D}' | '\u{201E}' if ps => '"',
            x => x,
        })
        .collect();
    let mut i = 0;
    // Mot entre guillemets, ou voisin d'une virgule (élément d'un tableau
    // PowerShell) : jamais un paramètre (KOB-437).
    let mut cite = false;
    let mut apres_virgule = false;
    let fin_mot = |jetons: &mut Vec<Jeton>,
                   mot: &mut String,
                   dans_mot: &mut bool,
                   cite: &mut bool,
                   apres_virgule: &mut bool,
                   avant_virgule: bool| {
        if *dans_mot {
            let litteral = std::mem::take(cite) || std::mem::take(apres_virgule) || avant_virgule;
            let mut m = std::mem::take(mot);
            // PowerShell prend aussi `–`, `—` et `―` pour le tiret d'un
            // paramètre (`gci –Recurse`, `gc –LiteralPath:…`) : ramenés à
            // `-`, sinon le paramètre passait pour un chemin (KOB-434). Pas
            // entre guillemets ni dans un tableau : c'est une valeur, reçue
            // telle quelle (`rg . '–e' x` : un fichier `–e`, KOB-447).
            if ps && !litteral && m.starts_with(['\u{2013}', '\u{2014}', '\u{2015}']) {
                let premier = m.chars().next().map_or(0, char::len_utf8);
                m.replace_range(..premier, "-");
            }
            // Un élément de tableau ou une chaîne entre guillemets qui commence
            // par un tiret est une valeur pour PowerShell, pas un paramètre :
            // `./` le garde chemin relatif, jugé comme tel, sans qu'il prenne
            // pour valeur l'élément suivant (`gc a,'-Encoding',..\x`).
            if ps && litteral && m.starts_with('-') {
                m.insert_str(0, "./");
                jetons.push(Jeton::Cite(m));
            } else {
                jetons.push(Jeton::Mot(m));
            }
            *dans_mot = false;
        }
    };
    while i < c.len() {
        let ch = c[i];
        match ch {
            ' ' | '\t' => fin_mot(
                &mut jetons,
                &mut mot,
                &mut dans_mot,
                &mut cite,
                &mut apres_virgule,
                false,
            ),
            // PowerShell : `a,b` est un tableau ; chaque élément est un mot,
            // jugé à part (KOB-437). Entre guillemets, la virgule reste dans
            // le nom (boucles des guillemets ci-dessous).
            ',' if ps => {
                // `-Recurse,src` : PowerShell finit-il le paramètre à la
                // virgule ? Un mot à tiret hors guillemets qui touche une
                // virgule sans en suivre une est ambigu : sur la carte plutôt
                // que deviné (un vrai `-Recurse` y serait caché).
                // Une valeur collée (`--format=%h,%s`, `-Path:a,b`) ferme le
                // nom : le mot reste un paramètre, la suite est sa valeur.
                let parametre = dans_mot && !cite && !apres_virgule && commence_par_un_tiret(&mot);
                if parametre && (parametre_ouvert(&mot) || mot.ends_with([':', '='])) {
                    return Err(format!("`{mot},` : paramètre ou valeur ? À valider"));
                }
                fin_mot(
                    &mut jetons,
                    &mut mot,
                    &mut dans_mot,
                    &mut cite,
                    &mut apres_virgule,
                    !parametre,
                );
                apres_virgule = true;
            }
            '\'' => {
                // Un mot à tiret hors guillemets qui se poursuit entre
                // guillemets (`-Recurse''`) : paramètre ou chaîne ? Ambigu.
                if ps && dans_mot && !cite && !apres_virgule && parametre_ouvert(&mot) {
                    return Err(format!(
                        "`{mot}` suivi de guillemets : paramètre ou valeur ? À valider"
                    ));
                }
                // Après une valeur collée (`--format='%h %s'`), le mot reste
                // un paramètre.
                // Après `-Param:` aussi, PowerShell lit la valeur comme un
                // argument à part.
                let debut = !dans_mot
                    || (!cite && commence_par_un_tiret(&mot) && mot.ends_with([':', '=']));
                cite |= !(dans_mot && commence_par_un_tiret(&mot));
                dans_mot = true;
                i += 1;
                loop {
                    match c.get(i) {
                        None => return Err("guillemet simple non fermé".into()),
                        // PowerShell : `''` dans une chaîne simple vaut `'`.
                        Some('\'') if ps && c.get(i + 1) == Some(&'\'') => {
                            mot.push(brut[i]);
                            i += 1;
                        }
                        Some('\'') => break,
                        Some(_) => mot.push(brut[i]),
                    }
                    i += 1;
                }
                if ps && debut && suite_collee(&c, i) {
                    return Err(format!(
                        "`'{mot}'` suivi d'autres caractères : un ou deux arguments ? À valider"
                    ));
                }
            }
            '"' => {
                // Un mot à tiret hors guillemets qui se poursuit entre
                // guillemets (`-Recurse''`) : paramètre ou chaîne ? Ambigu.
                if ps && dans_mot && !cite && !apres_virgule && parametre_ouvert(&mot) {
                    return Err(format!(
                        "`{mot}` suivi de guillemets : paramètre ou valeur ? À valider"
                    ));
                }
                // Après `-Param:` aussi, PowerShell lit la valeur comme un
                // argument à part.
                let debut = !dans_mot
                    || (!cite && commence_par_un_tiret(&mot) && mot.ends_with([':', '=']));
                cite |= !(dans_mot && commence_par_un_tiret(&mot));
                dans_mot = true;
                i += 1;
                loop {
                    match c.get(i) {
                        None => return Err("guillemet double non fermé".into()),
                        Some('"') => break,
                        // Bash : un `$` juste avant le guillemet fermant est
                        // littéral (`grep "foo$"`, KOB-482). Non prouvé sous
                        // PowerShell : il y reste refusé.
                        Some('$') if !ps && c.get(i + 1) == Some(&'"') => mot.push('$'),
                        Some('$') => {
                            return Err("variable ou substitution `$` entre guillemets".into())
                        }
                        Some('`') if !ps => return Err("substitution entre accents graves".into()),
                        // PowerShell : un accent grave entre guillemets doubles
                        // change le caractère qui suit (`` `u{2E} `` vaut `.` sous
                        // pwsh 7, `` `0 ``, `` `n ``…) : le mot reçu n'est pas celui
                        // lu ici (KOB-477). Comme `$` : non analysable.
                        Some('`') => return Err(
                            "accent grave entre guillemets doubles (PowerShell) : non analysable"
                                .into(),
                        ),
                        Some(&x) if x == echap => {
                            i += 1;
                            match c.get(i) {
                                // Bash retire `\` et saut de ligne : le mot que
                                // la commande reçoit n'est pas celui lu ici
                                // (`"\⏎--output=x"` est une option, KOB-474).
                                Some('\n' | '\r') => {
                                    return Err(
                                        "continuation de ligne entre guillemets doubles".into()
                                    )
                                }
                                Some(_) => mot.push(brut[i]),
                                None => return Err("échappement en fin de ligne".into()),
                            }
                        }
                        Some(_) => mot.push(brut[i]),
                    }
                    i += 1;
                }
                if ps && debut && suite_collee(&c, i) {
                    return Err(format!(
                        "`\"{mot}\"` suivi d'autres caractères : un ou deux arguments ? À valider"
                    ));
                }
            }
            // `> $null` jette le flux sous PowerShell (KOB-480) : la cible devient
            // `/dev/null`, seule cible nulle de `redirection`. Tout autre `$`, et
            // `'$null'` cité, qui écrit un fichier, restent à part.
            '$' if ps && !dans_mot && dollar_null_apres_redirection(&c, i, jetons.last()) => {
                jetons.push(Jeton::Mot("/dev/null".into()));
                i += "null".len();
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
            // `@x`, `@env:X` : splatting, la valeur de la variable devient des
            // arguments (`echo @env:CLE` l'affiche, KOB-470). En tête de mot,
            // d'élément de tableau, après une continuation (accent grave et
            // saut de ligne : le mot est vide) ou après `-Param:`, que
            // PowerShell coupe. `%an@%ae` reste un mot.
            '@' if ps
                && (!dans_mot
                    || mot.is_empty()
                    || c.get(i.wrapping_sub(1)) == Some(&',')
                    || (commence_par_un_tiret(&mot) && mot.ends_with(':')))
                && c.get(i + 1)
                    .is_some_and(|n| n.is_alphanumeric() || matches!(n, '_' | '?' | ':' | '^')) =>
            {
                return Err("splatting PowerShell `@variable` non analysable".into())
            }
            x if x == echap => {
                i += 1;
                match c.get(i) {
                    // Une continuation disparaît et ne commence pas de mot : le
                    // `#` qui la suit ouvre un commentaire (KOB-474). Au milieu
                    // d'un mot (`src`⏎--pre=./p`), PowerShell pourrait couper
                    // là où on recolle : seulement entre deux mots (KOB-479).
                    Some('\n')
                        if dans_mot
                            && !mot.is_empty()
                            && !c.get(i + 1).is_none_or(|n| n.is_whitespace()) =>
                    {
                        return Err("continuation de ligne au milieu d'un mot".into())
                    }
                    Some('\n') => {}
                    // PowerShell finit aussi une ligne sur `\r` : `-x` serait
                    // collé à la suite sans qu'on le voie (KOB-474).
                    Some('\r') => return Err("continuation de ligne sur un retour chariot".into()),
                    // PowerShell, hors guillemets : `` `-Pattern `` est une valeur
                    // (et non le paramètre), `` `t `` une tabulation… Le mot reçu
                    // n'est pas celui lu ici (KOB-479, même règle que KOB-477).
                    Some(_) if ps => {
                        return Err(
                            "accent grave hors guillemets (PowerShell) : non analysable".into()
                        )
                    }
                    Some(_) => {
                        dans_mot = true;
                        mot.push(brut[i]);
                    }
                    None => return Err("échappement en fin de ligne".into()),
                }
            }
            // Commentaire. Règle prudente (KOB-298) : suivi d'un saut de
            // ligne (`\n`, `\r\n`, `\r`), il n'est jamais une lecture, car
            // la ligne suivante est une autre commande. Seul en fin de ligne,
            // il clôt la commande. PowerShell : `<# … #>` est refusé.
            '#' if !dans_mot => {
                if ps && i > 0 && c[i - 1] == '<' {
                    return Err("commentaire de bloc PowerShell `<# #>` non analysable".into());
                }
                if c[i..].iter().any(|&x| matches!(x, '\n' | '\r')) {
                    return Err("commentaire suivi d'une autre ligne".into());
                }
                break;
            }
            // Sous bash, `\r` hors guillemets est un caractère ordinaire : lu
            // comme un séparateur, il cachait des options (`rg -g x\r# --pre=p`,
            // KOB-475). Sur la carte.
            '\r' if !ps => return Err("retour chariot hors guillemets".into()),
            // `\r` seul finit aussi une ligne (PowerShell) : un séparateur,
            // comme `\n` (KOB-298).
            ';' | '\n' | '\r' | '(' | ')' => {
                fin_mot(
                    &mut jetons,
                    &mut mot,
                    &mut dans_mot,
                    &mut cite,
                    &mut apres_virgule,
                    false,
                );
                jetons.push(Jeton::Op(ch.to_string()));
            }
            '|' => {
                fin_mot(
                    &mut jetons,
                    &mut mot,
                    &mut dans_mot,
                    &mut cite,
                    &mut apres_virgule,
                    false,
                );
                if c.get(i + 1) == Some(&'|') {
                    i += 1;
                    jetons.push(Jeton::Op("||".into()));
                } else {
                    jetons.push(Jeton::Op("|".into()));
                }
            }
            '&' => {
                if c.get(i + 1) == Some(&'&') {
                    fin_mot(
                        &mut jetons,
                        &mut mot,
                        &mut dans_mot,
                        &mut cite,
                        &mut apres_virgule,
                        false,
                    );
                    i += 1;
                    jetons.push(Jeton::Op("&&".into()));
                } else if c.get(i + 1) == Some(&'>') {
                    fin_mot(
                        &mut jetons,
                        &mut mot,
                        &mut dans_mot,
                        &mut cite,
                        &mut apres_virgule,
                        false,
                    );
                    i += 1;
                    let mut r = String::from("&>");
                    if c.get(i + 1) == Some(&'>') {
                        i += 1;
                        r.push('>');
                    }
                    jetons.push(Jeton::Redir(r));
                } else {
                    fin_mot(
                        &mut jetons,
                        &mut mot,
                        &mut dans_mot,
                        &mut cite,
                        &mut apres_virgule,
                        false,
                    );
                    jetons.push(Jeton::Op("&".into()));
                }
            }
            '>' | '<' => {
                // `2>`, `1>>`, `*>` : le numéro collé fait partie de la redirection.
                let mut r = String::new();
                // PowerShell ne redirige que les flux 1 à 6 : `7>` y est un mot
                // entier, et ce qui suit un argument (sonde #446, KOB-480).
                let numero = !mot.is_empty() && mot.chars().all(|d| d.is_ascii_digit());
                if ps
                    && dans_mot
                    && numero
                    && !matches!(mot.as_str(), "1" | "2" | "3" | "4" | "5" | "6")
                {
                    return Err(format!(
                        "`{mot}{ch}` n'est pas une redirection de PowerShell"
                    ));
                }
                if dans_mot && (numero_ecrit_tel_quel(&c, i, &mot, echap) || (ps && mot == "*")) {
                    r = std::mem::take(&mut mot);
                    dans_mot = false;
                    cite = false;
                    apres_virgule = false;
                } else {
                    fin_mot(
                        &mut jetons,
                        &mut mot,
                        &mut dans_mot,
                        &mut cite,
                        &mut apres_virgule,
                        false,
                    );
                }
                r.push(ch);
                while let Some(&n) = c.get(i + 1) {
                    if n == ch || (n == '&' && ch == '>') {
                        r.push(n);
                        i += 1;
                        if n == '&' {
                            // `2>&1` : la destination est un descripteur.
                            while let Some(&d) = c.get(i + 1) {
                                if d.is_ascii_digit() {
                                    r.push(d);
                                    i += 1;
                                } else {
                                    break;
                                }
                            }
                            if c.get(i + 1) == Some(&'-') {
                                r.push('-');
                                i += 1;
                            }
                            // `>&1M` n'est pas `>&1` suivi d'un mot : bash écrit
                            // le fichier `1M` (KOB-475).
                            if c.get(i + 1).is_some_and(|&d| {
                                !matches!(d, ' ' | '\t' | '\n' | ';' | '|' | '&' | '(' | ')')
                                    && !matches!(d, '<' | '>')
                                    && !(ps && d == '\r')
                            }) {
                                return Err(format!("`{r}` collé à la suite : à valider"));
                            }
                            break;
                        }
                    } else {
                        break;
                    }
                }
                jetons.push(Jeton::Redir(r));
            }
            // Espace insécable et autres blancs : PowerShell y coupe-t-il le
            // mot ? Sur la carte plutôt que deviné (KOB-441).
            x if ps && x.is_whitespace() => {
                return Err(format!("blanc inhabituel U+{:04X} : à valider", x as u32))
            }
            // Bash étend ce joker non cité : noté, puis jugé par `analyser` (KOB-489).
            x if !ps && matches!(x, '*' | '?' | '[') && joker_peut_donner_une_option(&mot) => {
                joker.get_or_insert_with(|| format!("{mot}{x}"));
                dans_mot = true;
                mot.push(x);
            }
            x => {
                dans_mot = true;
                mot.push(x);
            }
        }
        i += 1;
    }
    fin_mot(
        &mut jetons,
        &mut mot,
        &mut dans_mot,
        &mut cite,
        &mut apres_virgule,
        false,
    );
    // `--%` (arrêt de l'analyse) : PowerShell passe le reste tel quel à un
    // programme natif, après avoir remplacé `%VAR%` par sa valeur
    // (`which --% %CLE%` l'affiche, KOB-471). Comme `$` : non analysable.
    // C'est la valeur de l'argument qui compte : `'--%'` entre guillemets
    // (`./--%` ici) et `-x:--%` aussi.
    let arret = |m: &str| {
        let m = m.replace(['\u{2013}', '\u{2014}', '\u{2015}'], "-");
        let valeur = m.strip_prefix("./").unwrap_or(&m);
        let apres_param = (valeur.starts_with('-') && !valeur.starts_with("--%"))
            .then(|| valeur.split_once(':').map(|(_, v)| v))
            .flatten()
            .unwrap_or(valeur);
        valeur.starts_with("--%") || apres_param.trim_start_matches("./").starts_with("--%")
    };
    if ps
        && jetons
            .iter()
            .any(|j| matches!(j, Jeton::Mot(m) | Jeton::Cite(m) if arret(m)))
    {
        return Err("arrêt de l'analyse PowerShell `--%` non analysable".into());
    }
    Ok((jetons, joker))
}

/// `pwsh` hors Windows étend les jokers pour un exécutable natif, comme bash. `ls`,
/// `cat` ou `sort` y sont des exécutables ; seules les cmdlets et leurs alias
/// portables reçoivent le motif sans expansion (`where.exe` n'existe que sous
/// Windows). Les guillemets ne se voient plus dans les jetons : un motif cité
/// compte aussi.
fn joker_pour_un_natif_pwsh(jetons: &[Jeton]) -> Option<String> {
    let mut en_tete = true;
    let mut natif = false;
    for j in jetons {
        match j {
            Jeton::Op(_) => en_tete = true,
            Jeton::Redir(_) => {}
            Jeton::Mot(m) | Jeton::Cite(m) if en_tete => {
                let nom = m.to_lowercase();
                natif = !nom.contains('-')
                    && !matches!(
                        nom.as_str(),
                        "gci" | "gc" | "dir" | "type" | "sls" | "where.exe"
                    );
                en_tete = false;
            }
            Jeton::Mot(m) | Jeton::Cite(m) => {
                let x = m.strip_prefix("./").unwrap_or(m);
                let dangereux = x
                    .find(['*', '?', '['])
                    .is_some_and(|i| joker_peut_donner_une_option(&x[..i]));
                if natif && dangereux {
                    return Some(x.to_owned());
                }
            }
        }
    }
    None
}

/// Ce qui suit un `{` hors guillemets : bash peut-il le développer
/// (`{a,b}`, `{1..3}`) ? Règle prudente, sans chercher à refaire l'analyse de
/// bash : une virgule ou `..` après l'accolade, suivie plus loin d'un `}`,
/// suffit pour refuser. Guillemets doubles, échappements (KOB-258, KOB-272)
/// et accolades imbriquées (`{{},-delete}` donne `{} -delete`, KOB-286) n'y
/// changent rien ; `{}` seul (`find -exec cat {} +`) reste tel quel.
///
/// Seule exception, sûre : entre guillemets simples, bash ne développe rien
/// et n'échappe rien. Le texte y est sauté jusqu'au `'` suivant
/// (`… | jq '{name, id}'` reste une lecture, KOB-300). Un `'` échappé (`\'`)
/// ou entre guillemets doubles (`"it's"`) est un caractère ordinaire.
fn accolades_developpees(suite: &[char]) -> bool {
    let mut doubles = false;
    let mut i = 0;
    while let Some(&x) = suite.get(i) {
        match x {
            // Bash retire `\` et saut de ligne : `{o.\⏎.o}` est `{o..o}`. Au
            // doute, une expansion (KOB-474).
            '\\' if matches!(suite.get(i + 1), Some('\n' | '\r')) => return true,
            '\\' => i += 1,
            '"' => doubles = !doubles,
            '\'' if !doubles => {
                // Guillemet simple sans fermant : refusé ailleurs, et ici
                // tout le reste est sauté.
                match suite[i + 1..].iter().position(|&y| y == '\'') {
                    Some(n) => i += n + 1,
                    None => return false,
                }
            }
            ',' => return suite[i..].contains(&'}'),
            '.' if suite.get(i + 1) == Some(&'.') => return suite[i..].contains(&'}'),
            _ => {}
        }
        i += 1;
    }
    false
}

/// Un joker qui suit `debut`, le début du mot, peut-il donner une option ? Oui
/// si le mot est encore vide (`*`, `''*`), ou s'il commence par un tiret sans
/// `=` avant le joker (`-*`, `--pr[e]=x`). `src/*` et `--glob=*` commencent par
/// un texte fixe : ni le mot ni le nom de l'option ne peuvent changer.
fn joker_peut_donner_une_option(debut: &str) -> bool {
    debut.is_empty() || (debut.starts_with('-') && !debut.contains('='))
}

/// `c[i]` ouvre-t-il exactement `$null` (sans casse), juste après une
/// redirection de sortie, et suivi d'un séparateur ? `$nullx`, `$null.x`,
/// `$null:x`, `$null[0]`, `< $null` et `2>&1 $null` ne le sont pas.
fn dollar_null_apres_redirection(c: &[char], i: usize, avant: Option<&Jeton>) -> bool {
    let sortie = matches!(avant, Some(Jeton::Redir(r)) if r.contains('>') && !r.contains('&'));
    let nom = c
        .get(i + 1..i + 5)
        .is_some_and(|n| n.iter().collect::<String>().eq_ignore_ascii_case("null"));
    let fin = c
        .get(i + 5)
        .is_none_or(|d| matches!(d, ' ' | '\t' | '\n' | '\r' | ';' | '|' | '&' | ')'));
    sortie && nom && fin
}

/// Le mot juste avant `c[i]` est-il un numéro de descripteur, écrit sans
/// guillemet ni échappement, et qui tient sur un entier ? `'2'>x`, `\2>x` et
/// `2147483648>x` donnent un mot à la commande (KOB-475).
fn numero_ecrit_tel_quel(c: &[char], i: usize, mot: &str, echap: char) -> bool {
    let n = mot.chars().count();
    if n == 0
        || i < n
        || !mot.chars().all(|d| d.is_ascii_digit())
        || mot.parse::<i32>().is_err()
        || !c[i - n..i].iter().copied().eq(mot.chars())
    {
        return false;
    }
    // Le numéro commence un mot : `''2>x` donne aussi le mot `2`. Après une
    // continuation (échappement et saut de ligne), un mot a pu commencer avant.
    match i.checked_sub(n + 1).map(|p| (p, c[p])) {
        None => true,
        Some((p, '\n')) => p == 0 || c[p - 1] != echap,
        Some((_, avant)) => {
            matches!(
                avant,
                ' ' | '\t' | '\r' | ';' | '|' | '&' | '(' | ')' | '<' | '>'
            )
        }
    }
}

/// Redirection : seules celles qui n'écrivent dans aucun fichier passent.
/// Renvoie le nombre de jetons de cible consommés.
fn redirection(
    r: &str,
    cible: Option<&str>,
    shell: Shell,
    ctx: &Contexte,
    cwd: &Path,
) -> Result<usize, Avis> {
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
        return match juger_chemin(ctx, cible, cwd) {
            None => Ok(1),
            Some(avis) => Err(avis),
        };
    }
    // Seul `/dev/null` jette la sortie, plus `nul` pour PowerShell sous Windows.
    // Partout ailleurs, `null`, `nul` ou `'$null'` cité sont des fichiers (KOB-475).
    // `decouper` ramène ici un `$null` non cité de PowerShell à `/dev/null` (KOB-480).
    let poubelle = cible == "/dev/null"
        || (shell == Shell::PowerShell && cfg!(windows) && cible.eq_ignore_ascii_case("nul"));
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

/// Cmdlets PowerShell qui créent, changent ou retirent un alias (KOB-466).
const ALIAS_MODIFIES: &[&str] = &[
    "set-alias",
    "sal",
    "new-alias",
    "nal",
    "remove-alias",
    "import-alias",
    "ipal",
];

/// Mot qui désigne le lecteur `alias:` (`alias:where`, `Alias:\where`,
/// `-Path:alias:x`, `Microsoft.PowerShell.Core\Alias::where`).
fn touche_un_alias(x: &str) -> bool {
    let x = x.to_ascii_lowercase();
    let valeur = if x.starts_with('-') {
        x.split_once(':').map_or("", |(_, v)| v)
    } else {
        &x
    };
    valeur.starts_with("alias:") || x.contains("\\alias::") || x.contains("/alias::")
}

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

/// Options de `hostname` qui ne font qu'afficher.
const OPTIONS_HOSTNAME: &[&str] = &[
    "-a",
    "--alias",
    "-A",
    "--all-fqdns",
    "-d",
    "--domain",
    "-f",
    "--fqdn",
    "--long",
    "-i",
    "--ip-address",
    "-I",
    "--all-ip-addresses",
    "-s",
    "--short",
    "-y",
    "--yp",
    "--nis",
    "-h",
    "--help",
    "-V",
    "--version",
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
    "gh",
    "go",
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

/// Changements de dossier courant (cmdlets ou leurs alias sous PowerShell).
const CD: &[&str] = &["cd", "set-location", "sl", "chdir", "pushd"];

/// Noms de [`LECTURE_SIMPLE`] qui sont, sous PowerShell (Windows), des alias de
/// cmdlets : `'-x'` y reste une valeur, comme pour la cmdlet (KOB-447).
const ALIAS_PS: &[&str] = &[
    "ls", "dir", "cat", "echo", "type", "pwd", "sort", "diff", "where", "more",
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
    "go",
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
        // Seules les options dont git prend le mot suivant pour valeur :
        // `--format`, `--pretty`, `-U`, `--abbrev`, `--contains`… le laissent
        // à la commande (révision ou chemin), jugé alors (KOB-452).
        "git" => &[
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
            "--date",
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
        // Le `-e` de jq est le code de sortie, pas un motif (KOB-487).
        "-e" if nom == "jq" => {}
        "-e" | "--regexp" | "-pattern" | "-simplematch" => return Some(true),
        "-f" | "--file" if matches!(nom, "grep" | "egrep" | "fgrep" | "rg" | "sed" | "jq") => {
            return Some(false)
        }
        // `--from-file` est le `-f` de jq (KOB-487). Ni l'un ni l'autre ne
        // prend de valeur : le programme est le premier positionnel. Répondre
        // ici fait juger tous les positionnels comme des chemins.
        "--from-file" if nom == "jq" => return Some(false),
        _ => {}
    }
    // jq groupe ses options courtes (`-nf`, `-fn`, `-nfr`) : un `f` n'importe
    // où dans le groupe vaut `-f`.
    if nom == "jq" && x.len() > 2 && x.starts_with('-') && !x.starts_with("--") && x.contains('f') {
        return Some(false);
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
fn commande(
    mots: &[String],
    cites: &[bool],
    shell: Shell,
    ctx: &Contexte,
    cwd: &mut PathBuf,
) -> Avis {
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
    let exe = nom.ends_with(".exe");
    let nom = nom.strip_suffix(".exe").unwrap_or(&nom).to_owned();
    // Sous PowerShell, un exécutable reçoit `'--x'` tel quel : une option,
    // pas une valeur. Le `./` que `decouper` met devant (pour les cmdlets,
    // KOB-437) est retiré, sinon les contrôles d'options ne le voyaient plus
    // (`rg '--pre=calc.exe'`, KOB-447).
    // Seul le `./` mis par `decouper` est retiré : un `./-e` écrit tel quel
    // est un fichier pour l'exécutable aussi. `cd` et ses alias sont des
    // cmdlets.
    let natif = shell == Shell::PowerShell
        && (exe
            || !(LECTURE_PS.contains(&nom.as_str())
                || ALIAS_PS.contains(&nom.as_str())
                || CD.contains(&nom.as_str())));
    let recus: Vec<String>;
    let args: &[String] = if natif {
        recus = mots[1..]
            .iter()
            .zip(cites.iter().skip(1).chain(std::iter::repeat(&false)))
            .map(|(x, &cite)| {
                if cite {
                    arg_d_executable(x).to_owned()
                } else {
                    x.clone()
                }
            })
            .collect();
        &recus
    } else {
        &mots[1..]
    };
    // ripgrep lit `---pre=x` comme `--pre=x` et lance le programme : les filtres
    // d'options, écrits pour `--`, ne le voient pas (KOB-476). On ne connaît pas
    // le parseur de chaque commande : trois tirets ou plus, sur la carte. Sauf
    // `echo` et `printf`, qui les affichent (`echo "---"` sert de séparateur).
    let affiche = matches!(nom.as_str(), "echo" | "printf");
    if let Some(x) = args.iter().find(|x| !affiche && x.starts_with("---")) {
        return Avis::ecriture(
            Genre::Commande,
            format!(
                "`{x}` : certains programmes lisent trois tirets comme une option à deux tirets"
            ),
        );
    }
    if CD.contains(&nom.as_str()) {
        // PowerShell : un paramètre qui prend une valeur (`-ea 0`,
        // `-StackName .`) ferait prendre cette valeur pour le dossier visé ;
        // seuls `-Path`, `-LiteralPath` et `-PassThru` sont suivis, et un seul
        // dossier (KOB-458).
        if shell == Shell::PowerShell {
            let autre = args.iter().any(|a| {
                a.starts_with('-')
                    && !["-path", "-literalpath", "-passthru"]
                        .iter()
                        .any(|o| abrege_ps(a, o))
                    && !a.eq_ignore_ascii_case("-lp")
            });
            let dossiers = args.iter().filter(|a| !a.starts_with('-')).count();
            if autre || dossiers > 1 {
                return Avis::ecriture(
                    Genre::Commande,
                    "`cd` avec un paramètre ou plusieurs dossiers : vérifié sur la carte",
                );
            }
        }
        // Bash : `cd -P` suit les liens physiquement (`dedans/..` mène au
        // parent réel du lien), alors que le dossier courant est suivi ici sur
        // le texte : sur la carte (KOB-453).
        if shell == Shell::Bash
            && args
                .iter()
                .any(|a| a.starts_with('-') && !a.starts_with("--") && a[1..].contains('P'))
        {
            return Avis::ecriture(
                Genre::Commande,
                "`cd -P` suit les liens : vérifié sur la carte",
            );
        }
        // Bash et zsh : le dossier courant suivi sur le texte diverge aussi
        // quand `pushd -n` ne bouge pas, quand `pushd ±N` tourne la pile, et
        // quand zsh lit `cd ancien nouveau` comme un remplacement (KOB-462).
        if shell == Shell::Bash {
            // `pushd` : toute option (`-n`, `+1`, `'+1 '`, qui vaut `+1` pour
            // bash) ; `cd ±N` : zsh y lit une entrée de la pile.
            let numero = |a: &str| {
                let a = a.trim_end();
                a.len() > 1
                    && a.starts_with(['+', '-'])
                    && a[1..].bytes().all(|b| b.is_ascii_digit())
            };
            let pile = if nom == "pushd" {
                args.iter().any(|a| a.starts_with(['+', '-']) && a != "--")
            } else {
                args.iter().any(|a| numero(a))
            };
            // zsh arrête les options au premier opérande : tout mot qui suit
            // compte (`cd ancien nouveau`, `cd u/code -L`).
            let premier = args.iter().position(|a| !a.starts_with('-'));
            let plusieurs = premier.is_some_and(|i| i + 1 < args.len());
            if pile || plusieurs {
                return Avis::ecriture(
                    Genre::Commande,
                    "`cd`/`pushd` qui ne suit pas un seul dossier : vérifié sur la carte",
                );
            }
        }
        let Some(cible) = args.iter().find(|a| !a.starts_with('-')) else {
            return Avis::refus("`cd` sans dossier mène hors des dossiers de code");
        };
        // PowerShell : `Env:`, `Variable:`, `HKCU:\…` ne sont pas des
        // dossiers ; les suivre comme un sous-dossier ferait passer
        // `gc ANTHROPIC_API_KEY` pour un fichier du code (KOB-468).
        if shell == Shell::PowerShell && (lecteur_ps(cible) || fournisseur_ps(cible)) {
            return Avis::refus(format!("{cible} est hors des dossiers de code déclarés"));
        }
        // PowerShell 6.2 et plus : `cd -` et `cd +` parcourent l'historique
        // des dossiers (`'-'` arrive ici en `./-`, mis par `decouper`).
        if shell == Shell::PowerShell && matches!(arg_d_executable(cible), "-" | "+" | "./+") {
            return Avis::ecriture(
                Genre::Commande,
                "`cd -` ou `cd +` (historique des dossiers) : vérifié sur la carte",
            );
        }
        if let Some(avis) = juger_chemin(ctx, cible, cwd) {
            return avis;
        }
        if a_des_jokers(cible) {
            return Avis::ecriture(Genre::Commande, "`cd` vers un motif : vérifié sur la carte");
        }
        if let Some(nouveau) = crate::classement::absolu(Path::new(cible), Some(cwd)) {
            // Bash : un `cd` logique vers un dossier absent sur le texte, mais
            // présent physiquement, est refait physiquement (`dedans/../z` mène
            // au parent réel du lien) : le texte ne vaut plus (KOB-462).
            let physique = std::fs::canonicalize(cwd.join(cible)).is_ok_and(|p| p.is_dir());
            if shell == Shell::Bash && !nouveau.is_dir() && physique {
                return Avis::ecriture(
                    Genre::Commande,
                    format!("`cd {cible}` : dossier absent sur le texte, vérifié sur la carte"),
                );
            }
            *cwd = nouveau;
        }
        return Avis::lecture();
    }
    // Un alias retiré ou redéfini change le sens des commandes qui suivent
    // (`Remove-Item alias:where -Force; where C:\x:*` lance where.exe) : la
    // carte le dit (KOB-466). Les commandes de lecture connues gardent leurs
    // contrôles (`cat alias:x C:\secret` reste refusé).
    let alias = shell == Shell::PowerShell
        && (ALIAS_MODIFIES.contains(&nom.as_str()) || args.iter().any(|x| touche_un_alias(x)));
    const ALIAS: &str =
        "modifie un alias PowerShell : les commandes suivantes peuvent changer de sens";
    if SUPPRESSION.contains(&nom.as_str()) {
        return Avis::ecriture(
            Genre::Suppression,
            if alias {
                format!("`{premier}` supprime des fichiers ou {ALIAS}")
            } else {
                format!("`{premier}` supprime des fichiers")
            },
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
            if alias {
                format!("`{premier}` {ALIAS}")
            } else {
                format!("`{premier}` n'est pas une commande de lecture connue de Kobi")
            },
        );
    }
    if let Some(a) = options_interdites(&nom, args, shell) {
        return a;
    }
    // `where` est le programme `where.exe` sauf sous PowerShell, où seul le
    // nom avec `.exe` l'appelle (sans, c'est l'alias `Where-Object`).
    if nom == "where" && (shell != Shell::PowerShell || premier.to_lowercase().ends_with(".exe")) {
        if let Some(a) = where_exe(args, ctx, cwd) {
            return a;
        }
    } else if nom == "where" && args.iter().any(|x| est_r_de_where(x)) {
        // L'alias `Where-Object`, sauf s'il a été retiré (`Remove-Item
        // alias:where`, profil) : `/R` serait alors l'option de where.exe
        // (KOB-455). Ni lecture, ni refus.
        return Avis::ecriture(
            Genre::Commande,
            "`where … /R` : `Where-Object` ou where.exe ? Vérifié sur la carte",
        );
    }
    let avis = chemins(&nom, args, shell, ctx, cwd);
    // jq : après le jugement des chemins, pour qu'un chemin hors des
    // dossiers reste un refus (`jq -f prog ../../secret`, KOB-463).
    if nom == "jq" && avis.classe == Classe::Lecture && jq_lit_hors(args) {
        return Avis::ecriture(
            Genre::Commande,
            "`jq` lit des fichiers ou l'environnement (import, env, -f, -L) : sur la carte",
        );
    }
    // Garde-fou en attendant KOB-469 (KOB-473) : une lecture git peut lancer
    // un programme configuré dans le dépôt (`core.fsmonitor` sur status et
    // diff, même d'un sous-dépôt caché dans l'index ; `diff.external` et
    // textconv sur diff, show, log -p, blame, grep ; `gpg.program` sur log
    // --show-signature et verify-commit ; les filtres…). Ce qui reste une
    // lecture après le jugement des chemins passe donc par la carte, sans
    // examen de la config ni de l'index. Seul `git version` ne lit pas le
    // dépôt (`git -C version status` : `version` y est un dossier).
    if nom == "git" && avis.classe == Classe::Lecture {
        let mots: Vec<&String> = args.iter().filter(|x| !x.starts_with('-')).collect();
        // Arguments entiers : `git version --help` lance `man.<outil>.cmd`, et une
        // valeur de `-C` à tiret (`git -C -*`) n'est pas un mot (KOB-488).
        let avec_mot = !mots.is_empty() || args.iter().any(|x| x == "-C");
        if avec_mot && !matches!(args, [seul] if seul == "version") {
            return Avis::ecriture(
                Genre::Commande,
                "git peut lancer des programmes configurés dans ce dépôt (fsmonitor, diff externe, textconv, gpg) : vérifié sur la carte",
            );
        }
    }
    // Sous Windows, les parcours récursifs entrent dans les jonctions et les
    // liens de dossier, même ceux qui sortent (sonde #392, KOB-434) : le
    // départ est jugé, puis tout l'arbre, sans rien suivre.
    if cfg!(windows) && avis.classe == Classe::Lecture && parcours_recursif(&nom, args, shell) {
        let racines = match racines_du_parcours(&nom, args, ctx, cwd) {
            Ok(r) => r,
            Err(a) => return a,
        };
        if let Err(a) = arbre_sans_lien_qui_sort(ctx, &racines, cwd) {
            return a;
        }
    }
    avis
}

/// Commande qui parcourt des dossiers récursivement : `Get-ChildItem`,
/// `gci`, `ls` ou `dir` avec `-Recurse` ou `-Depth` (qui l'implique,
/// PowerShell 5 et plus), `dir /S`, `tree`, `where /R` (cmd).
fn parcours_recursif(nom: &str, args: &[String], shell: Shell) -> bool {
    // `-s` est un alias de `-Recurse` ; `-d` est `-Directory`, pas `-Depth`.
    let recurse = || {
        args.iter().any(|x| {
            abrege_ps(x, "-recurse")
                || est_depth(x)
                || x.split([':', '='])
                    .next()
                    .is_some_and(|o| o.eq_ignore_ascii_case("-s"))
        })
    };
    let interrupteur = |c: &str| args.iter().any(|x| x.eq_ignore_ascii_case(c));
    match nom {
        "get-childitem" | "gci" => recurse(),
        "ls" | "dir" if shell == Shell::PowerShell => recurse(),
        "dir" => interrupteur("/s"),
        "tree" => true,
        "where" => args.iter().any(|x| est_r_de_where(x)),
        _ => false,
    }
}

/// `-Depth` ou son abréviation, au moins `-de` (`-d` est `-Directory`).
fn est_depth(x: &str) -> bool {
    x.split([':', '=']).next().is_some_and(|o| o.len() >= 3) && abrege_ps(x, "-depth")
}

/// Paramètre PowerShell ou son abréviation (`-r`, `-rec` pour `-Recurse`),
/// valeur collée comprise (`-Recurse:$true`).
fn abrege_ps(x: &str, nom: &str) -> bool {
    let x = x.split([':', '=']).next().unwrap_or(x).to_lowercase();
    x.len() >= 2 && x.starts_with('-') && !x.starts_with("--") && nom.starts_with(&x)
}

/// Dossiers où commence le parcours, tels que la commande les prendra : les
/// positionnels et les valeurs de `-Path`/`-LiteralPath` (`-Path:x`
/// compris), jamais les valeurs des autres options (`-Filter`, `-Exclude`,
/// `-Depth`…) ; pour `where`, le dossier de `/R`. Chacun est jugé par
/// `contient` avant tout accès au disque, coupé avant son premier joker,
/// puis résolu sans rien suivre (une racine qui est une jonction est
/// remplacée par sa cible). Sans positionnel : le dossier de la commande.
fn racines_du_parcours(
    nom: &str,
    args: &[String],
    ctx: &Contexte,
    cwd: &Path,
) -> Result<Vec<PathBuf>, Avis> {
    let mut textes: Vec<&str> = Vec::new();
    // Un paramètre que le parcours ne connaît pas peut prendre une valeur
    // (`-ea 0`, `-Exclude a,b`) : cette valeur passerait pour une racine et
    // cacherait le dossier courant, d'où partirait vraiment le parcours. Il
    // est alors parcouru aussi (KOB-457).
    let mut doute = false;
    if nom == "where" {
        if let Some(i) = args.iter().position(|x| est_r_de_where(x)) {
            textes.extend(args.get(i + 1).map(String::as_str));
        }
    } else {
        let mut i = 0;
        while i < args.len() {
            let x = args[i].as_str();
            // Un nom de paramètre n'a ni `/` ni `\` : `–x\..\..` est un chemin
            // pour PowerShell, comme dans `chemins()` (KOB-434), donc une
            // racine du parcours (KOB-457).
            let chemin_a_tiret = x
                .split([':', '='])
                .next()
                .is_some_and(|o| o.contains(['/', '\\']));
            if x.starts_with('-') && x.len() > 1 && !chemin_a_tiret {
                let (option, valeur) = match x.split_once([':', '=']) {
                    Some((o, v)) => (o, Some(v)),
                    None => (x, None),
                };
                let chemin = abrege_ps(option, "-path")
                    || abrege_ps(option, "-literalpath")
                    || option.eq_ignore_ascii_case("-lp")
                    || option.eq_ignore_ascii_case("-pspath");
                let a_valeur = chemin
                    || est_depth(option)
                    || options_a_valeur(nom)
                        .iter()
                        .any(|o| *o != "-depth" && abrege_ps(option, o));
                let sur = chemin
                    || est_depth(option)
                    || [
                        "-recurse",
                        "-filter",
                        "-force",
                        "-file",
                        "-directory",
                        "-name",
                        "-hidden",
                        "-readonly",
                        "-system",
                    ]
                    .iter()
                    .any(|o| abrege_ps(option, o));
                doute |= !sur;
                match (chemin, valeur) {
                    (true, Some(v)) => textes.push(v),
                    (true, None) => {
                        textes.extend(args.get(i + 1).map(String::as_str));
                        i += 1;
                    }
                    (false, None) if a_valeur => i += 1,
                    _ => {}
                }
                i += 1;
                continue;
            }
            // Interrupteurs de cmd (`/S`, `/F`, `/B`).
            if !(x.starts_with('/') && x.len() == 2) {
                textes.push(x);
            }
            i += 1;
        }
    }
    let mut racines = Vec::new();
    for t in textes {
        if !ctx.contient(Path::new(t), Some(cwd)) {
            return Err(Avis::refus(format!(
                "{t} est hors des dossiers de code déclarés"
            )));
        }
        let Some(absolu) = crate::classement::absolu(Path::new(t), Some(cwd)) else {
            return Err(Avis::ecriture(
                Genre::Commande,
                format!("parcours depuis `{t}` : vérifié sur la carte"),
            ));
        };
        // Avant le premier joker : `..\*.rs` part de `..`.
        let fixe: PathBuf = absolu
            .components()
            .take_while(|c| !a_des_jokers(&c.as_os_str().to_string_lossy()))
            .collect();
        match crate::classement::resolu(&fixe) {
            Some(r) => racines.push(r),
            None => {
                return Err(Avis::ecriture(
                    Genre::Commande,
                    format!("parcours depuis `{t}` : vérifié sur la carte"),
                ))
            }
        }
    }
    if racines.is_empty() || doute {
        if let Some(r) = crate::classement::resolu(cwd) {
            if !racines.contains(&r) {
                racines.push(r);
            }
        }
    }
    Ok(racines)
}

/// L'arbre sous `racines` ne contient-il aucun lien qui sort des dossiers ?
/// Parcouru comme le ferait la commande, mais sans rien ouvrir hors des
/// dossiers : `read_dir` sur les vrais dossiers, chaque lien (jonction
/// comprise) jugé par `contient` ; un lien qui reste dedans est suivi par sa
/// cible résolue, car la commande le suivrait aussi (une jonction dedans peut
/// mener à une autre qui sort). Un dossier déjà vu ne l'est pas deux fois.
/// À un lien qui sort, à une erreur de lecture (autre qu'« absent »), ou
/// au-delà de [`ENTREES_MAX`] entrées : validé sur la carte.
fn arbre_sans_lien_qui_sort(ctx: &Contexte, racines: &[PathBuf], cwd: &Path) -> Result<(), Avis> {
    sans_lien_qui_sort(ctx, racines, cwd, None)
}

/// Comme [`arbre_sans_lien_qui_sort`], mais sur ce que liste un `gci` sans
/// `-Recurse` : les enfants de ses racines, et un niveau sous chaque joker
/// (`gci src/*` liste aussi le contenu des dossiers trouvés), lus ensuite
/// par une cmdlet qui ne liste pas (`gci *.rs | sls motif`, KOB-438).
fn liste_sans_lien_qui_sort(
    ctx: &Contexte,
    args: &[String],
    racines: &[PathBuf],
    cwd: &Path,
) -> Result<(), Avis> {
    let niveaux = args
        .iter()
        .map(|x| {
            let v = match x.strip_prefix('-') {
                Some(o) => o.split_once([':', '=']).map_or("", |(_, v)| v),
                None => x,
            };
            let elements: Vec<String> = Path::new(v)
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            elements
                .iter()
                .position(|e| a_des_jokers(e))
                .map_or(0, |i| elements.len() - i)
        })
        .max()
        .unwrap_or(0);
    sans_lien_qui_sort(ctx, racines, cwd, Some(niveaux + 1))
}

/// Parcours commun, limité à `niveaux` sous les racines s'il est donné.
fn sans_lien_qui_sort(
    ctx: &Contexte,
    racines: &[PathBuf],
    cwd: &Path,
    niveaux: Option<usize>,
) -> Result<(), Avis> {
    const ENTREES_MAX: usize = 10_000;
    let carte = |raison: String| Err(Avis::ecriture(Genre::Commande, raison));
    let mut a_voir: Vec<(PathBuf, usize)> = racines.iter().map(|r| (r.clone(), 0)).collect();
    // Profondeur la plus faible où chaque dossier a été vu : revu plus haut,
    // il est parcouru plus loin.
    let mut vus = std::collections::HashMap::new();
    let mut entrees_vues = 0usize;
    while let Some((dossier, profondeur)) = a_voir.pop() {
        if vus.get(&dossier).is_some_and(|&p| p <= profondeur) {
            continue;
        }
        vus.insert(dossier.clone(), profondeur);
        // Une racine fichier (`gci Cargo.toml`) : jugée, rien dessous.
        if profondeur == 0 && dossier.is_file() {
            continue;
        }
        let descendre = niveaux.is_none_or(|n| profondeur + 1 < n);
        let entrees = match std::fs::read_dir(&dossier) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                return carte(format!(
                    "parcours récursif : {} illisible ({e}), sur la carte",
                    dossier.display()
                ))
            }
        };
        for e in entrees {
            let Ok(e) = e else {
                return carte("parcours récursif : entrée illisible, sur la carte".into());
            };
            entrees_vues += 1;
            if entrees_vues > ENTREES_MAX {
                return carte(
                    "parcours récursif trop grand pour être vérifié : sur la carte".into(),
                );
            }
            let chemin = e.path();
            let Ok(genre) = e.file_type() else {
                return carte(format!(
                    "parcours récursif : {} illisible, sur la carte",
                    chemin.display()
                ));
            };
            if genre.is_symlink() {
                if !ctx.contient(&chemin, Some(cwd)) {
                    return carte(format!(
                        "le parcours récursif suivrait {}, qui sort des dossiers : sur la carte",
                        chemin.display()
                    ));
                }
                if !descendre {
                    continue;
                }
                if let Some(cible) = crate::classement::resolu(&chemin) {
                    if cible.is_dir() {
                        a_voir.push((cible, profondeur + 1));
                    }
                }
            } else if genre.is_dir() && descendre {
                a_voir.push((chemin, profondeur + 1));
            }
        }
    }
    Ok(())
}

/// Cmdlets dont `-Path` accepte une chaîne venue du `|` (ByValue) : elles
/// changent du texte en fichiers, que la suite du pipeline lit (KOB-438).
const CHEMIN_PAR_LE_PIPE: &[&str] = &[
    "get-item",
    "gi",
    "get-childitem",
    "gci",
    "ls",
    "dir",
    "resolve-path",
    "rvpa",
    "test-path",
    "convert-path",
    "cvpa",
    "get-itemproperty",
    "gp",
    "get-filehash",
    "get-acl",
];

/// Amont dont la sortie n'est faite que des chemins de leurs arguments, déjà
/// jugés, et de ce qu'ils contiennent, vérifié quand une cmdlet en aval le
/// lit (KOB-438).
const AMONT_SUR: &[&str] = &["get-childitem", "gci", "ls", "dir", "get-item", "gi"];

/// Cmdlets qui lisent un chemin pris aux objets du `|` (`Path`,
/// `LiteralPath`, `PSPath`, ByPropertyName), en plus de celles qui le
/// prennent d'une chaîne (KOB-438).
const LIT_PAR_LE_PIPE: &[&str] = &[
    "get-item",
    "gi",
    "get-childitem",
    "gci",
    "ls",
    "dir",
    "resolve-path",
    "rvpa",
    "test-path",
    "convert-path",
    "cvpa",
    "get-itemproperty",
    "gp",
    "get-filehash",
    "get-acl",
    "get-content",
    "gc",
    "cat",
    "type",
    "select-string",
    "sls",
];

/// Cmdlets de lecture dont la sortie porte des chemins qu'on n'a pas jugés :
/// propriétés quelconques (`ConvertFrom-Json`), exécutables trouvés dans le
/// PATH (`Get-Command`), racines des lecteurs (KOB-438).
const SOURCE_LIBRE: &[&str] = &[
    "convertfrom-json",
    "get-command",
    "gcm",
    "get-psdrive",
    "gdr",
];

/// Options ou sous-commandes qui écrivent, par commande.
fn options_interdites(nom: &str, args: &[String], shell: Shell) -> Option<Avis> {
    let a = |s: &str| args.iter().any(|x| x == s);
    let commence = |p: &str| args.iter().any(|x| x.starts_with(p));
    let ecrit = |raison: String| Some(Avis::ecriture(Genre::Commande, raison));
    // Suivre les liens en parcourant un dossier : un lien qui sort y serait
    // lu sans être jugé (KOB-432). Validé sur la carte.
    let court = |c: char| {
        args.iter()
            .any(|x| x.starts_with('-') && !x.starts_with("--") && x[1..].contains(c))
    };
    let longue = |o: &str| args.iter().any(|x| abrege(x, o));
    // PowerShell accepte un paramètre abrégé (`-Follow`, `-Fol`) : pwsh hors
    // de Windows suit alors les liens (KOB-456). `ls` et `dir` y sont
    // `Get-ChildItem`.
    let suit_ps = shell == Shell::PowerShell
        && matches!(nom, "gci" | "get-childitem" | "ls" | "dir")
        && args.iter().any(|x| abrege_ps(x, "-followsymlink"));
    let suit_les_liens = match nom {
        // `-S` : le grep BSD de macOS suit tous les liens (KOB-454).
        "grep" | "egrep" | "fgrep" => court('R') || court('S') || longue("--dereference-recursive"),
        "rg" | "fd" => court('L') || longue("--follow"),
        "find" => a("-L") || a("-follow"),
        "tree" => court('l'),
        "ls" | "du" => court('L') || longue("--dereference"),
        // GNU `dir` est `ls -C -b` (KOB-472) ; sous PowerShell, c'est
        // `Get-ChildItem`, où `-L` abrège `-LiteralPath`.
        "dir" if shell != Shell::PowerShell => {
            court('L')
                || longue("--dereference")
                || args
                    .iter()
                    .any(|x| x.eq_ignore_ascii_case("-followsymlink"))
        }
        // `diff -r` compare les cibles des liens qu'il trouve.
        "diff" => court('r') || longue("--recursive"),
        _ => args
            .iter()
            .any(|x| x.eq_ignore_ascii_case("-followsymlink")),
    };
    let suit_les_liens = suit_les_liens || suit_ps;
    if suit_les_liens {
        return ecrit(format!("`{nom}` suit les liens en parcourant les dossiers"));
    }
    // `--files0-from=F` : la liste des fichiers lus est dans `F`, jamais
    // jugée ici (KOB-442).
    // De même pour `strings @F` (arguments pris dans `F`), `file -f F` et
    // `sha256sum -c F` (fichiers listés dans `F`, ouverts pour vérification).
    let liste_dans_un_fichier = match nom {
        "sort" | "wc" | "du" => longue("--files0-from"),
        "find" => a("-files0-from"),
        "strings" => commence("@"),
        "file" => court('f') || longue("--files-from"),
        "md5sum" | "sha1sum" | "sha256sum" | "sha512sum" | "b2sum" | "cksum" => {
            court('c') || longue("--check")
        }
        _ => false,
    };
    // `gcm -ShowCommandInfo` affiche `Definition` : le corps des fonctions
    // chargées (modules, profil) (relecture de KOB-459).
    if matches!(nom, "get-command" | "gcm") && args.iter().any(|x| abrege_ps(x, "-showcommandinfo"))
    {
        return ecrit(format!(
            "`{nom} -ShowCommandInfo` affiche le code des fonctions"
        ));
    }
    if liste_dans_un_fichier {
        return ecrit(format!(
            "`{nom}` lit des chemins pris dans un fichier (`files0-from`)"
        ));
    }
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
            // Seules les options sans valeur connues : une option à valeur
            // (`-l N`, `--line-length N`) prendrait le mot suivant, et le vrai
            // script passerait pour un fichier (`sed -n -l 1p '1e …'`,
            // KOB-464). Le script est le premier opérande, `--` compris.
            let mut options = true;
            let mut script = None;
            let mut muet = false;
            for x in args {
                if options && x == "--" {
                    options = false;
                } else if options && x.starts_with('-') && x != "-" {
                    muet |= matches!(x.as_str(), "--quiet" | "--silent")
                        || (!x.starts_with("--") && x.contains('n'));
                    let sure = match x.strip_prefix("--") {
                        Some(longue) => matches!(
                            longue,
                            "quiet"
                                | "silent"
                                | "regexp-extended"
                                | "separate"
                                | "unbuffered"
                                | "null-data"
                                | "posix"
                                | "sandbox"
                        ),
                        None => x[1..].chars().all(|c| "nErsuz".contains(c)),
                    };
                    if !sure {
                        return ecrit(format!("`sed {x}` : option non vérifiée"));
                    }
                } else {
                    script = Some(x);
                    break;
                }
            }
            let sur = script.is_some_and(|s| script_sed_sur(s)) && muet;
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
        "uniq" if positionnels_uniq(args) > 1 => Some(Avis::ecriture(
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
        // Liste blanche (KOB-478) : `-Ff`, `--file=f`, `--fil=f`, `-b` et un nom en
        // argument changent le nom de la machine.
        "hostname" if args.iter().any(|x| !OPTIONS_HOSTNAME.contains(&x.as_str())) => ecrit(
            "`hostname` avec autre chose qu'une option d'affichage peut changer le nom".into(),
        ),
        "less" | "more"
            if args
                .iter()
                .any(|x| x.starts_with('+') || x == "-o" || commence("--log-file")) =>
        {
            ecrit("option de `less` qui écrit ou exécute".into())
        }
        "git" => git(args),
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
        // npm, yarn, pnpm, cargo, rustc, rustup et dotnet (`global.json`
        // peut désigner un SDK du dossier) ne sont pas des lectures :
        // ils exécutent du code venu du dossier (scripts homonymes,
        // `.yarnrc` yarn-path, `.npmrc` prefix sous Windows, `rustc-wrapper`,
        // `rust-toolchain` path, KOB-467).
        "go" | "java" | "tsc"
            if !matches!(
                args,
                [x] if matches!(x.as_str(), "--version" | "-V" | "version" | "-v")
            ) =>
        {
            ecrit(format!("seul `{nom} --version` est en lecture"))
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

/// Opérandes de `uniq` (entrée, puis sortie) : après `--`, tout mot compte
/// (`uniq -- in -sortie` écrit `-sortie`, KOB-461) ; les valeurs de `-f`,
/// `-s`, `-w` (séparées) ne comptent pas ; `-` seul est un opérande ; après
/// le premier opérande, tout mot compte (BSD uniq).
fn positionnels_uniq(args: &[String]) -> usize {
    let mut n = 0;
    let mut options = true;
    let mut i = 0;
    while let Some(x) = args.get(i) {
        if options && x == "--" {
            options = false;
        } else if options && x.starts_with('-') && x != "-" {
            let a_valeur = matches!(x.as_str(), "-f" | "-s" | "-w")
                || ["--skip-fields", "--skip-chars", "--check-chars"]
                    .iter()
                    .any(|o| x == o);
            if a_valeur {
                i += 1;
            }
        } else {
            n += 1;
            // BSD uniq (macOS) arrête les options au premier opérande, comme
            // GNU sous `POSIXLY_CORRECT` : `uniq in -sortie` écrit `-sortie`.
            options = false;
        }
        i += 1;
    }
    n
}

/// `mot` apparaît-il dans `texte` comme un mot entier (ni lettre, ni
/// chiffre, ni `_` autour), hors d'un champ jq (`.env`, `.include`, jamais
/// exécutés) ? `env.X` oui, `.environment` et `.spec.env` non.
fn mot_entier(texte: &str, mot: &str) -> bool {
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    texte.match_indices(mot).any(|(i, _)| {
        !texte[..i]
            .chars()
            .next_back()
            .is_some_and(|c| ident(c) || c == '.')
            && !texte[i + mot.len()..].chars().next().is_some_and(ident)
    })
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
    // Un `\` échappe le `/` d'une adresse : `/^\|x\/,/e cmd #/p` cache une
    // commande `e` après une adresse qui paraît fermée (relecture de KOB-464).
    if s.contains('\\') {
        return false;
    }
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

/// jq lit hors des arguments jugés (KOB-463) : par son filtre (`import`,
/// `include`, `modulemeta`), dans l'environnement (`env`, `$ENV`), par un
/// filtre ou des tests chargés d'un fichier (`-f`, `-nf`, `--from-file`,
/// `--run-tests`) ou par des modules (`-L`, groupé ou non, `--library-path`).
/// Tous les mots comptent : un filtre peut commencer par un tiret
/// (`jq -n '-1|env.X'`). Un mot précédé d'un `.` est un champ (`.env`).
fn jq_lit_hors(args: &[String]) -> bool {
    let mot_cle = args.iter().any(|x| {
        ["import", "include", "modulemeta", "env", "ENV"]
            .iter()
            .any(|m| mot_entier(x, m))
    });
    let fichier = args.iter().any(|x| {
        let court = x.starts_with('-') && !x.starts_with("--");
        x == "--from-file"
            || x == "--run-tests"
            || abrege(x, "--library-path")
            || (court && (x.contains('f') || x.contains('L')))
    });
    mot_cle || fichier
}

/// `git` : options globales et sous-commandes en lecture.
fn git(args: &[String]) -> Option<Avis> {
    let ecrit = |raison: String| Some(Avis::ecriture(Genre::Commande, raison));
    // `git --version`, seul, n'exécute rien du dépôt, comme `git version`
    // (essayé sur un dépôt avec fsmonitor, pager, alias et hook, KOB-481).
    if matches!(args, [seul] if seul == "--version") {
        return None;
    }
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

/// `where.exe` : ses dossiers sont des chemins, jugés comme tels (KOB-439).
/// Celui de `/R` ou `-R` (les deux formes marchent), et la partie chemin
/// d'un motif `chemin:motif`, qui liste aussi ce dossier (`\\hôte\partage:x`
/// ouvrirait une connexion réseau). Une variable (`$PATH:motif`), une option
/// inconnue ou un `C:motif` (lecteur, ou dossier `C` ?) passent par la carte.
fn where_exe(args: &[String], ctx: &Contexte, cwd: &Path) -> Option<Avis> {
    let carte = |x: &str| {
        Some(Avis::ecriture(
            Genre::Commande,
            format!("`where.exe {x}` : vérifié sur la carte"),
        ))
    };
    // Un seul dossier de parcours : le contrôle des jonctions (KOB-434) ne
    // juge que le premier.
    if args.iter().filter(|x| est_r_de_where(x)).count() > 1 {
        return carte("/R … /R");
    }
    let mut i = 0;
    while i < args.len() {
        let x = arg_de_where(&args[i]);
        let bas = x.to_ascii_lowercase();
        if bas == "/r" || bas == "-r" {
            let Some(d) = args.get(i + 1) else {
                return Some(Avis::refus("`where /R` sans dossier"));
            };
            if let Some(a) = juger_chemin(ctx, d, cwd) {
                return Some(a);
            }
            i += 2;
            continue;
        }
        if x.len() == 2 && x.starts_with(['/', '-']) {
            if !matches!(&bas[1..], "q" | "f" | "t" | "?") {
                return carte(x);
            }
        } else if x.starts_with(['-', '$'])
            || (x.starts_with('/') && x.len() > 2 && !x[1..].contains('/'))
        {
            // Option longue, variable, ou option collée à sa valeur
            // (`/RC:\x`) : un chemin Bash (`/c/x`) a un second `/`.
            return carte(x);
        } else if let Some((chemin, _)) = x.rsplit_once(':') {
            // `C:motif` : lecteur, ou dossier `C` ? Un `;` : where.exe peut
            // le lire comme une liste de dossiers, comme `PATH`.
            if chemin.len() <= 1 || chemin.contains(';') {
                return carte(x);
            }
            if let Some(a) = juger_chemin(ctx, chemin, cwd) {
                return Some(a);
            }
        }
        i += 1;
    }
    None
}

/// Argument tel qu'un exécutable le reçoit : un `-…` entre guillemets ou dans
/// un tableau PowerShell porte un `./` de tête ajouté par `decouper` (KOB-437),
/// que l'exécutable ne voit pas (`'-R'` est bien l'option `-R`).
fn arg_d_executable(x: &str) -> &str {
    x.strip_prefix("./")
        .filter(|r| r.starts_with('-'))
        .unwrap_or(x)
}

/// Argument tel que where.exe le reçoit (voir [`arg_d_executable`]).
fn arg_de_where(x: &str) -> &str {
    let x = arg_d_executable(x);
    // Git Bash (MSYS) passe `//R` à where.exe comme `/R` : c'est même la
    // forme naturelle, `/R` seul y étant pris pour un chemin (KOB-449). Comme
    // MSYS, seulement sans autre `/` : `//hôte/partage` est un chemin réseau,
    // transmis tel quel.
    match x.strip_prefix('/') {
        Some(r) if r.starts_with('/') && !r[1..].contains('/') => r,
        _ => x,
    }
}

/// `/R` ou `-R` de where.exe, sous toutes ses formes.
fn est_r_de_where(x: &str) -> bool {
    let x = arg_de_where(x);
    x.eq_ignore_ascii_case("/r") || x.eq_ignore_ascii_case("-r")
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
        if let Some(x) = args.iter().find(|x| lecteur_ps(x) || fournisseur_ps(x)) {
            return Avis::refus(format!("{x} est hors des dossiers de code déclarés"));
        }
    }
    // Motif donné par une option (`grep /etc/passwd -e root`) : tous les
    // positionnels sont des chemins (KOB-227).
    let mut motif_vu = !MOTIF_D_ABORD.contains(&nom) || motif_par_option(nom, args);
    // `git -C d` : git lit ses chemins depuis `d` (`--git-dir=lien`) ; ils
    // sont jugés depuis chaque dossier possible (KOB-444).
    let mut bases = vec![cwd.to_path_buf()];
    if nom == "git" {
        let mut j = 0;
        while j < args.len() {
            if args[j] == "-C" {
                if let Some(d) = args.get(j + 1) {
                    let suivante = bases[bases.len() - 1].join(d);
                    bases.push(suivante);
                }
                j += 1;
            }
            j += 1;
        }
    }
    let juge = |v: &str| bases.iter().find_map(|b| juger_chemin(ctx, v, b));
    // Après `--`, plus d'options : chaque mot est un opérande, jugé même
    // s'il commence par un tiret (`cat -- -lien/x`, KOB-452).
    let mut fin_des_options = false;
    let mut i = 0;
    while i < args.len() {
        let x = &args[i];
        let bas = x.to_lowercase();
        if x == "--" && !fin_des_options {
            fin_des_options = true;
            i += 1;
            continue;
        }
        // Sous PowerShell, un nom de paramètre n'a ni `/` ni `\` : `'-x\..\y'`
        // (entre guillemets, ou au tiret Unicode ramené à `-`) est un
        // chemin, jugé comme tel (KOB-434).
        if ps
            && x.starts_with('-')
            && x.split([':', '='])
                .next()
                .is_some_and(|o| o.contains(['/', '\\']))
        {
            if let Some(avis) = juger_chemin(ctx, x, cwd) {
                return avis;
            }
            i += 1;
            continue;
        }
        if x.starts_with('-') && x.len() > 1 {
            // `--git-dir=/x`, `-Path=…`, PowerShell `-Path:/x` : la valeur est
            // vérifiée si c'est un chemin, ou un nom qui existe dans le
            // dossier (`--from-file=lien`, `-Path:lien`, KOB-444), lu sans
            // rien suivre.
            let valeur = x
                .split_once('=')
                .or_else(|| if ps { x.split_once(':') } else { None });
            // Sous PowerShell, un joker (`-Path:l*`) est développé par la
            // cmdlet : jugé aussi. Sous Bash, `--include=*.rs` est un filtre
            // de noms, pas un chemin.
            if let Some((_, v)) = valeur {
                let existe = !v.is_empty()
                    && bases
                        .iter()
                        .any(|b| std::fs::symlink_metadata(b.join(v)).is_ok());
                if ressemble_a_un_chemin(v) || existe || (ps && a_des_jokers(v)) {
                    if let Some(avis) = juge(v) {
                        return avis;
                    }
                }
            }
            // `grep -flien`, `rg -iflien` : le fichier de motifs collé à `-f`
            // dans des options courtes groupées (KOB-444).
            if let Some(f) = fichier_colle_a_f(nom, x) {
                if let Some(avis) = juge(f) {
                    return avis;
                }
            }
            // Après `--`, le mot est aussi un opérande : motif ou chemin. Les
            // contrôles ci-dessus restent, car PowerShell retire le premier
            // `--` avant un exécutable (le mot y redevient une option).
            if fin_des_options {
                if motif_vu {
                    if let Some(avis) = juge(x) {
                        return avis;
                    }
                } else {
                    motif_vu = true;
                }
                i += 1;
                continue;
            }
            // jq : `--arg NOM VALEUR`, `--argjson NOM JSON` prennent deux mots
            // qui ne sont pas des chemins ; `--rawfile NOM FICHIER`,
            // `--slurpfile` et `--argfile` lisent le second, jugé même à
            // tiret (`--rawfile a -lien/x`, KOB-460). Le filtre reste le
            // premier opérande.
            if nom == "jq" {
                match x.as_str() {
                    "--arg" | "--argjson" => {
                        i += 3;
                        continue;
                    }
                    "--rawfile" | "--slurpfile" | "--argfile" => {
                        if let Some(f) = args.get(i + 2) {
                            if let Some(avis) = juge(f) {
                                return avis;
                            }
                        }
                        i += 3;
                        continue;
                    }
                    _ => {}
                }
            }
            if nom == "git" && x == "-C" {
                if let Some(d) = args.get(i + 1) {
                    if let Some(avis) = juge(d) {
                        return avis;
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
        if let Some(avis) = juge(x) {
            return avis;
        }
        i += 1;
    }
    Avis::lecture()
}

/// grep, rg : dans des options courtes groupées, la première qui prend une
/// valeur prend la suite du mot ; si c'est `-f` (fichier de motifs) et que
/// la suite n'est pas vide, c'est ce fichier (`-iflien` → `lien`). Seules
/// comptent les lettres qui prennent une valeur dans toutes les variantes
/// (GNU grep, ugrep, ripgrep) : `-Tflien` est `-T` puis `-f lien`.
fn fichier_colle_a_f<'a>(nom: &str, x: &'a str) -> Option<&'a str> {
    let a_valeur = match nom {
        "grep" | "egrep" | "fgrep" => "efmABCdD",
        "rg" => "efmABCdDgtTjMEr",
        _ => return None,
    };
    if x.starts_with("--") {
        return None;
    }
    let reste = x.strip_prefix('-')?;
    let (i, c) = reste.char_indices().find(|(_, c)| a_valeur.contains(*c))?;
    let valeur = &reste[i + c.len_utf8()..];
    (c == 'f' && !valeur.is_empty()).then_some(valeur)
}

/// Un chemin donné à une commande est-il permis ? `None` s'il l'est. Un
/// motif (`*`, `?`, `[…]`) est développé plus largement que tout shell, sans
/// rien suivre, et chaque chemin possible est jugé : sinon `cat
/// sorti?/secret`, absent tel quel, passait pour un chemin du dossier avant
/// que le shell le change en `sortie/secret` (KOB-432). Ce que le shell
/// développe autrement (accolades, `**`, motifs étendus) ou un motif trop
/// large se fait valider sur la carte.
fn juger_chemin(ctx: &Contexte, x: &str, cwd: &Path) -> Option<Avis> {
    let hors = |p: &str| {
        Some(Avis::refus(format!(
            "{p} est hors des dossiers de code déclarés"
        )))
    };
    if !ctx.contient(Path::new(x), Some(cwd)) {
        return hors(x);
    }
    // Accolades (`{a,b}`, `{1..3}`, n'importe où), `**` (zsh, `globstar`),
    // motifs étendus (`extglob` : `@(…)`, `+(…)`…), ou un motif à
    // backtracking coûteux : validés sur la carte plutôt que devinés.
    let accolades = x.contains('{') && x.contains('}') && (x.contains(',') || x.contains(".."));
    // zsh `EXTENDED_GLOB` (`^x/…`, `(#i)x`, `a~b/…`) quand l'argument
    // désigne un chemin : pas une révision git (`HEAD~1`, `HEAD^`). Un `#`
    // collé à un nom reste un nom (KOB-298) ; un `~` en tête est le dossier
    // personnel, refusé plus haut.
    let zsh = (x.contains('/') && (x.contains('^') || x[1..].contains('~')))
        || x.starts_with('^')
        || x.starts_with("(#");
    let etendu = ["@(", "+(", "!(", "*(", "?("].iter().any(|o| x.contains(o)) || zsh;
    if accolades || etendu || x.contains("**") || x.matches('*').count() > 6 {
        return Some(Avis::ecriture(
            Genre::Commande,
            format!("motif `{x}` : vérifié sur la carte"),
        ));
    }
    if !a_des_jokers(x) {
        return None;
    }
    match developper(ctx, x, cwd) {
        Ok(chemins) => {
            // Ce que le motif désigne et qui sort : refusé. Ce que notre
            // lecture du motif écarte, mais qu'un shell pourrait prendre
            // (classes POSIX, casse sous PowerShell…), et qui sort : validé
            // sur la carte, jamais lu sans elle.
            if let Some((p, _)) = chemins
                .iter()
                .find(|(p, pris)| *pris && !ctx.contient(p, Some(cwd)))
            {
                return hors(&p.display().to_string());
            }
            chemins
                .iter()
                .find(|(p, _)| !ctx.contient(p, Some(cwd)))
                .map(|(p, _)| {
                    Avis::ecriture(
                        Genre::Commande,
                        format!(
                            "motif `{x}` : {} sort des dossiers, vérifié sur la carte",
                            p.display()
                        ),
                    )
                })
        }
        Err(avis) => Some(avis),
    }
}

/// `*`, `?` ou `[` : un motif que le shell développe.
fn a_des_jokers(x: &str) -> bool {
    x.contains(['*', '?', '['])
}

/// Chemins qu'un motif peut désigner, composant par composant depuis sa
/// partie fixe, chacun avec « pris par notre lecture du motif ». Pour un
/// composant à jokers, **toutes** les entrées du dossier sont gardées, plus
/// `.` et `..` si le composant commence par un point (bash avant 5.2) : plus
/// large que n'importe quel shell, jamais moins. Chaque dossier est jugé
/// avant d'être listé : `read_dir` suivrait un lien qui sort (sous Windows,
/// vers un partage réseau).
fn developper(ctx: &Contexte, motif: &str, cwd: &Path) -> Result<Vec<(PathBuf, bool)>, Avis> {
    const MAX: usize = 256;
    let texte = motif.replace('\\', "/");
    let (depart, reste) = if let Some(r) = texte.strip_prefix('/') {
        (PathBuf::from("/"), r.to_owned())
    } else if texte.len() >= 2 && texte.as_bytes()[1] == b':' {
        let (lecteur, r) = texte.split_at(2);
        (
            PathBuf::from(format!("{lecteur}/")),
            r.trim_start_matches('/').to_owned(),
        )
    } else {
        (cwd.to_path_buf(), texte)
    };
    let mut courants = vec![(depart, true)];
    for morceau in reste.split('/').filter(|m| !m.is_empty()) {
        if !a_des_jokers(morceau) {
            courants = courants
                .into_iter()
                .map(|(c, pris)| (c.join(morceau), pris))
                .collect();
            continue;
        }
        let mut suivants = Vec::new();
        for (dossier, pris) in &courants {
            if !ctx.contient(dossier, Some(cwd)) {
                return Err(Avis::refus(format!(
                    "{} est hors des dossiers de code déclarés",
                    dossier.display()
                )));
            }
            let Ok(entrees) = std::fs::read_dir(dossier) else {
                continue;
            };
            let points = morceau
                .starts_with('.')
                .then(|| [".", ".."].map(std::ffi::OsString::from))
                .into_iter()
                .flatten();
            for nom in entrees.flatten().map(|e| e.file_name()).chain(points) {
                let garde = *pris && correspond(morceau, &nom.to_string_lossy());
                suivants.push((dossier.join(&nom), garde));
                if suivants.len() > MAX {
                    return Err(Avis::ecriture(
                        Genre::Commande,
                        format!("motif `{motif}` trop large : vérifié sur la carte"),
                    ));
                }
            }
        }
        courants = suivants;
    }
    Ok(courants)
}

/// Un nom correspond-il au motif d'un composant (`*`, `?`, `[a-z]`, `[!x]`) ?
fn correspond(motif: &str, nom: &str) -> bool {
    fn suite(m: &[char], n: &[char]) -> bool {
        match m.first() {
            None => n.is_empty(),
            Some('*') => (0..=n.len()).any(|i| suite(&m[1..], &n[i..])),
            Some('?') => !n.is_empty() && suite(&m[1..], &n[1..]),
            Some('[') => {
                let Some(fin) = m.iter().skip(2).position(|c| *c == ']').map(|p| p + 2) else {
                    return n.first() == Some(&'[') && suite(&m[1..], &n[1..]);
                };
                let Some(c) = n.first() else { return false };
                let (negatif, classe) = match m[1] {
                    '!' | '^' => (true, &m[2..fin]),
                    _ => (false, &m[1..fin]),
                };
                let mut dedans = false;
                let mut i = 0;
                while i < classe.len() {
                    if i + 2 < classe.len() && classe[i + 1] == '-' {
                        dedans |= classe[i] <= *c && *c <= classe[i + 2];
                        i += 3;
                    } else {
                        dedans |= classe[i] == *c;
                        i += 1;
                    }
                }
                dedans != negatif && suite(&m[fin + 1..], &n[1..])
            }
            Some(x) => n.first() == Some(x) && suite(&m[1..], &n[1..]),
        }
    }
    let m: Vec<char> = motif.chars().collect();
    let n: Vec<char> = nom.chars().collect();
    suite(&m, &n)
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

/// Chemin qualifié par un fournisseur PowerShell
/// (`Microsoft.PowerShell.Core\Environment::X`, `FileSystem::C:\x`) :
/// `lecteur_ps` ne le voit pas, et le reste passerait pour un fichier relatif
/// (KOB-468). Un chemin Rust (`crate::x`) n'est pas un fournisseur.
fn fournisseur_ps(x: &str) -> bool {
    let x = x
        .split_once(':')
        .map(|(_, v)| v)
        .filter(|_| x.starts_with('-'))
        .unwrap_or(x);
    x.split_once("::").is_some_and(|(avant, _)| {
        let nom = avant.rsplit(['\\', '/']).next().unwrap_or(avant);
        [
            "alias",
            "environment",
            "filesystem",
            "function",
            "registry",
            "variable",
            "certificate",
            "wsman",
        ]
        .iter()
        .any(|f| nom.eq_ignore_ascii_case(f))
    })
}

/// Valeur d'option qui désigne un fichier (`/x`, `C:\x`, `../x`, `~/x`).
fn ressemble_a_un_chemin(v: &str) -> bool {
    v.contains('/') || v.contains('\\') || v.starts_with('~') || v.starts_with('.')
}

#[cfg(test)]
mod tests {
    use super::*;

    /// KOB-441 : sous PowerShell, `'x'/R` donne deux arguments (sonde
    /// #402 : `where.exe 'x'/R ..\dehors secret.txt` a listé le fichier) ;
    /// Kobi n'y voyait qu'un mot. Ce qui suit un mot entre guillemets, collé,
    /// passe par la carte ; un guillemet au milieu d'un mot reste du mot.
    #[test]
    fn mot_apres_guillemet_powershell() {
        let x = ctx();
        for ligne in [
            r"where.exe 'x'/R ..\dehors secret.txt",
            r"where 'x'/R ..\dehors secret.txt",
            r#"where.exe "x"/R ..\dehors secret.txt"#,
            r"Get-Content 'x'..\..\..\..\Windows\win.ini",
            "gc ''C:/Windows/win.ini",
            "gc 'Cargo.toml'\"C:/Windows/win.ini\"",
            "gc Cargo.toml,'x'C:/Windows/win.ini",
            // Guillemets typographiques, que PowerShell prend pour des
            // guillemets (relecture sécurité de KOB-441).
            "where.exe \u{2018}x\u{2019}/R ..\\dehors secret.txt",
            "where.exe 'x\u{2019} /R ..\\dehors secret.txt \u{2018}'",
            "where.exe \"x\u{201D} /R ..\\dehors secret.txt \u{201C}\"",
            // Valeur collée à un paramètre, puis autre chose.
            "where.exe -Param:'x'/R ..\\dehors secret.txt",
            // Blancs Unicode : coupure incertaine.
            "where.exe 'x'\u{A0}/R ..\\dehors secret.txt",
            "where.exe x\u{A0}/R ..\\dehors secret.txt",
        ] {
            assert_ne!(
                analyser(ligne, Shell::PowerShell, &x).classe,
                Classe::Lecture,
                "{ligne}"
            );
        }
        for ligne in [
            "gc 'Cargo.toml'",
            "gc \"Cargo.toml\"",
            "gc x'y'z",
            "gc 'Cargo.toml','README.md'",
            "gc 'Cargo.toml' | Select-String motif",
            "gc 'Cargo.toml';gc README.md",
            "git log --format='%h %s'",
            // Apostrophe typographique dans une chaîne double : un caractère.
            "git log --grep \"l\u{2019}appli\"",
        ] {
            assert!(lu(&analyser(ligne, Shell::PowerShell, &x)), "{ligne}");
        }
        // Bash colle les morceaux : un seul mot, comme le voit Kobi.
        assert_eq!(
            analyser("cat 'src'/main.rs", Shell::Bash, &x).classe,
            Classe::Lecture
        );
    }

    /// KOB-437 : sous PowerShell, `a,b` est un tableau de deux chemins
    /// (sonde #396 : `gc a.txt,..\dehors\secret.txt` lit les deux), chacun
    /// jugé ; entre guillemets, la virgule reste dans le nom.
    #[test]
    fn tableaux_powershell_a_virgule() {
        let x = ctx();
        for ligne in [
            "Get-Content Cargo.toml,C:/Windows/win.ini",
            r"Get-Content Cargo.toml,..\..\..\..\Windows\win.ini",
            "gc README.md,C:/Users/moi/.ssh/id_rsa",
            r"gci -Recurse src,..\..\autre",
            "Select-String motif -Path Cargo.toml,C:/Windows/win.ini",
            // Un élément qui ressemble à une option ne cache pas le suivant
            // (relecture sécurité de KOB-437).
            r"Get-Content Cargo.toml,'-Encoding',..\..\..\..\Windows\win.ini",
            r"Get-Content Cargo.toml,-Encoding,C:\Windows\win.ini",
            "gc Cargo.toml,'\u{2013}Tail',C:\\Windows\\win.ini",
            // Un paramètre collé à une virgule ou à des guillemets : ambigu,
            // sur la carte (un vrai `-Recurse` n'y est jamais caché).
            "gci -Recurse,src",
            "gci -Recurse, src",
            "gci -Recurse'' src",
            "gci -Recurse:,src",
            "gc -Path:Cargo.toml,C:/Windows/win.ini",
            r"gci -Recurse src,'-Filter',C:\Windows",
            // Espaces autour de la virgule, virgule unaire, tableau mixte.
            r"gc Cargo.toml , C:\Windows\win.ini",
            r"gc Cargo.toml, C:\Windows\win.ini",
            r"gc ,C:\Windows\win.ini",
            r"gc 'Cargo.toml',C:\Windows\win.ini",
        ] {
            assert_ne!(
                analyser(ligne, Shell::PowerShell, &x).classe,
                Classe::Lecture,
                "{ligne}"
            );
        }
        for ligne in [
            "Get-Content Cargo.toml,README.md",
            "Get-Content 'a,b.txt'",
            "gc \"a,b.txt\"",
            // Propriétés de Select-Object.
            "gci src | Select-Object Name,Length",
            // Un vrai paramètre reste un paramètre.
            "Get-Content -Encoding utf8 Cargo.toml",
            // Une valeur collée ferme le nom du paramètre : pas d'ambiguïté.
            "git log -5 --format='%h %s'",
            "gc -Path:Cargo.toml,README.md",
        ] {
            assert!(lu(&analyser(ligne, Shell::PowerShell, &x)), "{ligne}");
        }
        // Virgule échappée : tout accent grave hors guillemets passe par la
        // carte sous PowerShell (KOB-479).
        assert_ne!(
            analyser("gc a`,b.txt", Shell::PowerShell, &x).classe,
            Classe::Lecture
        );
        // Sous Bash, la virgule n'est qu'un caractère du nom.
        assert_eq!(
            analyser("cat a,b.txt", Shell::Bash, &x).classe,
            Classe::Lecture
        );
    }

    /// KOB-438 : sous PowerShell, une chaîne passée par le `|` à une cmdlet
    /// qui en fait un fichier (sonde #396 : `echo <dehors> | gi | gc` lit le
    /// fichier) n'est jamais une lecture sans carte ; un amont de chemins
    /// déjà jugés, ou du texte cherché par Select-String, reste une lecture.
    #[test]
    fn chemins_par_le_pipe_powershell() {
        let x = ctx();
        for ligne in [
            r"echo C:\Windows\win.ini | Get-Item | Get-Content",
            "Write-Output C:/Users/moi/.ssh/id_rsa | gi | gc",
            "echo C:/Windows | Resolve-Path",
            "echo C:/Windows/win.ini | Test-Path",
            r#"ConvertFrom-Json '{"Path":"C:/Windows/win.ini"}' | Get-Content"#,
            r#"echo '{"PSPath":"C:/Windows/win.ini"}' | ConvertFrom-Json | Select-Object PSPath | gc"#,
            "gcm notepad | gc",
            r"echo \\hote\partage | gci",
        ] {
            assert_ne!(
                analyser(ligne, Shell::PowerShell, &x).classe,
                Classe::Lecture,
                "{ligne}"
            );
        }
        for ligne in [
            "gci src | gc",
            "Get-Item Cargo.toml | Get-Content",
            "cat Cargo.toml | Select-String motif",
            "gci src | Select-Object Name",
        ] {
            assert_eq!(
                analyser(ligne, Shell::PowerShell, &x).classe,
                Classe::Lecture,
                "{ligne}"
            );
        }
    }

    /// Jonction Windows `lien` → `cible` ; la sortie de `mklink` en cas d'échec.
    #[cfg(windows)]
    fn jonction(lien: &Path, cible: &Path) {
        let sortie = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(lien)
            .arg(cible)
            .output()
            .unwrap();
        assert!(
            sortie.status.success(),
            "mklink /J {} {} : {}{}",
            lien.display(),
            cible.display(),
            String::from_utf8_lossy(&sortie.stdout),
            String::from_utf8_lossy(&sortie.stderr)
        );
    }

    /// KOB-438 : `gci src | gc` lit les fichiers que `gci` a listés (par
    /// `PSPath`), liens compris : un lien qui sort, même au fond de l'arbre
    /// (`gci src | gci | gc`), passe par la carte.
    #[test]
    fn liste_par_le_pipe_et_lien_qui_sort() {
        let code = tempfile::tempdir().unwrap();
        let dehors = tempfile::tempdir().unwrap();
        let c = code.path();
        std::fs::create_dir_all(c.join("src/sous")).unwrap();
        std::fs::write(c.join("src/a.rs"), "").unwrap();
        let mut x = Contexte::new(vec![c.to_path_buf()]);
        x.cwd = Some(c.to_path_buf());
        // Listes qui descendent dans l'arbre, et listes d'un seul niveau.
        let profondes = [
            "gci src | gci | Get-Content",
            "Get-Item src | gci | Select-String motif",
            "gci -Recurse src | gc",
            // Joker au milieu : ses chemins sont jugés un à un (`src/sous/lien`).
            "gci src/*/* | gc",
            // Un joker descend d'un niveau : par excès, un dossier `x.rs`
            // serait listé.
            "gci src/*.rs | sls motif",
            // Deux racines (KOB-437) : `src/sous` est vérifié aussi.
            "gci src,src/sous | gc",
        ];
        let plates = ["gci src | gc", "gci src | Sort-Object Name | gc"];
        for l in profondes.iter().chain(&plates) {
            assert_eq!(
                analyser(l, Shell::PowerShell, &x).classe,
                Classe::Lecture,
                "{l} sans lien qui sort"
            );
        }
        // Un joker sur des dossiers qui existent reste une lecture (sous
        // Windows, `src\*` n'est pas un nom valide : il est lu comme absent).
        assert_eq!(
            analyser("cat src/*.rs", Shell::Bash, &x).classe,
            Classe::Lecture
        );
        assert_eq!(
            analyser("Get-Content src/*.rs", Shell::PowerShell, &x).classe,
            Classe::Lecture
        );
        // Composant par composant : `mklink` refuse un chemin à `/`.
        let lien = c.join("src").join("sous").join("lien");
        #[cfg(unix)]
        std::os::unix::fs::symlink(dehors.path(), &lien).unwrap();
        #[cfg(windows)]
        jonction(&lien, dehors.path());
        for l in profondes {
            assert_ne!(
                analyser(l, Shell::PowerShell, &x).classe,
                Classe::Lecture,
                "{l} par un lien qui sort"
            );
        }
        // Un `gci` sans `-Recurse` lu par une cmdlet qui ne liste pas : un
        // seul niveau, le lien au fond de `src/sous` n'est pas lu.
        for l in plates {
            assert_eq!(
                analyser(l, Shell::PowerShell, &x).classe,
                Classe::Lecture,
                "{l} : lien hors de portée"
            );
        }
        // `gci src/*` liste aussi le contenu des dossiers trouvés : le lien
        // dans `src/sous` est lu (relecture sécurité de KOB-438).
        assert_ne!(
            analyser("gci src/* | gc", Shell::PowerShell, &x).classe,
            Classe::Lecture
        );
        let lien_haut = c.join("src").join("haut");
        #[cfg(unix)]
        std::os::unix::fs::symlink(dehors.path(), &lien_haut).unwrap();
        #[cfg(windows)]
        jonction(&lien_haut, dehors.path());
        for l in plates {
            assert_ne!(
                analyser(l, Shell::PowerShell, &x).classe,
                Classe::Lecture,
                "{l} par un lien qui sort, au premier niveau"
            );
        }
        // Une racine fichier : rien dessous, même à côté d'un lien.
        assert_eq!(
            analyser("gci src/a.rs | gc", Shell::PowerShell, &x).classe,
            Classe::Lecture
        );
        // Sans cmdlet qui lise en aval, la liste reste une lecture.
        assert_eq!(
            analyser("gci src | Select-Object Name", Shell::PowerShell, &x).classe,
            Classe::Lecture
        );
    }

    /// KOB-434 : les parcours récursifs reconnus, et l'arbre vérifié sans
    /// rien suivre (les liens sont jugés, pas traversés).
    #[test]
    fn parcours_recursifs_reconnus() {
        let v = |l: &str| l.split(' ').map(str::to_owned).collect::<Vec<_>>();
        assert!(parcours_recursif(
            "get-childitem",
            &v("-Recurse -Filter *.rs"),
            Shell::PowerShell
        ));
        assert!(parcours_recursif(
            "gci",
            &v("crates -rec"),
            Shell::PowerShell
        ));
        assert!(parcours_recursif("ls", &v("-r"), Shell::PowerShell));
        assert!(parcours_recursif("dir", &v("/S /B"), Shell::Bash));
        assert!(parcours_recursif("tree", &v("/F"), Shell::Bash));
        assert!(parcours_recursif("where", &v("/R . *.rs"), Shell::Bash));
        assert!(!parcours_recursif("gci", &v("crates"), Shell::PowerShell));
        assert!(!parcours_recursif("ls", &v("-r"), Shell::Bash));
        assert!(!parcours_recursif("dir", &v("/B"), Shell::Bash));
    }

    #[cfg(unix)]
    #[test]
    fn arbre_verifie_sans_suivre_les_liens() {
        use std::os::unix::fs::symlink;
        let code = tempfile::tempdir().unwrap();
        let dehors = tempfile::tempdir().unwrap();
        let c = code.path();
        std::fs::create_dir_all(c.join("a/b")).unwrap();
        std::fs::write(c.join("a/b/f.rs"), "").unwrap();
        symlink(c.join("a"), c.join("a/b/vers_a")).unwrap();
        let mut x = Contexte::new(vec![c.to_path_buf()]);
        x.cwd = Some(c.to_path_buf());
        // Un lien qui reste dedans est suivi par sa cible, une seule fois :
        // la boucle `vers_a` finit.
        assert!(arbre_sans_lien_qui_sort(&x, &[c.to_path_buf()], c).is_ok());
        // Un lien qui reste dedans mène à un autre qui sort : vu, sur la
        // carte (la commande les suivrait tous deux).
        std::fs::create_dir_all(c.join("autre")).unwrap();
        symlink(c.join("autre"), c.join("a/vers_autre")).unwrap();
        symlink(dehors.path(), c.join("autre/sortie")).unwrap();
        let racine_a = [c.join("a")];
        let avis = arbre_sans_lien_qui_sort(&x, &racine_a, c).unwrap_err();
        assert_eq!(avis.classe.cle(), "ecriture", "{}", avis.raison);
        // Les racines : positionnels et `-Path`, jamais les valeurs de
        // `-Exclude`, `-Filter` ou `-Depth` ; le dossier courant sinon.
        let v = |l: &str| l.split(' ').map(str::to_owned).collect::<Vec<_>>();
        let r = |l: &str| racines_du_parcours("gci", &v(l), &x, c).unwrap();
        let reel = |p: &std::path::Path| crate::classement::resolu(p).unwrap();
        assert_eq!(r("a -Filter *.rs"), vec![reel(&c.join("a"))]);
        assert_eq!(r("-Recurse -Exclude a"), vec![reel(c)]);
        assert_eq!(r("-Recurse -Excl a"), vec![reel(c)]);
        assert_eq!(r("-Depth 3"), vec![reel(c)]);
        assert_eq!(r("-Path:a -Recurse"), vec![reel(&c.join("a"))]);
        assert_eq!(r("-LiteralPath a"), vec![reel(&c.join("a"))]);
        // Un joker : le parcours part de ce qui le précède.
        assert_eq!(r("a/*.rs -Recurse"), vec![reel(&c.join("a"))]);
        // Une racine hors des dossiers, même en valeur d'option `-Path` :
        // refusée sans accès au disque.
        assert_eq!(
            racines_du_parcours("gci", &v("-Path:/etc"), &x, c)
                .unwrap_err()
                .classe
                .cle(),
            "refus"
        );
    }

    /// KOB-434 : un dossier de code déclaré sous un lien (comme `/var` →
    /// `/private/var` sous macOS) se parcourt par sa forme résolue sans
    /// paraître sortir de lui-même ; un lien qui sort reste vu.
    #[cfg(unix)]
    #[test]
    fn dossier_declare_sous_un_lien() {
        use std::os::unix::fs::symlink;
        let base = tempfile::tempdir().unwrap();
        let dehors = tempfile::tempdir().unwrap();
        let reel = base.path().join("prive");
        std::fs::create_dir_all(reel.join("code/src")).unwrap();
        symlink(&reel, base.path().join("var")).unwrap();
        let declare = base.path().join("var/code");
        symlink(declare.join("src"), declare.join("vers_src")).unwrap();
        let mut x = Contexte::new(vec![declare.clone()]);
        x.cwd = Some(declare.clone());
        let racines = racines_du_parcours("gci", &[], &x, &declare).unwrap();
        // Résolue jusqu'au bout (sous macOS, le dossier temporaire lui-même
        // est sous `/var` → `/private/var`).
        let attendu = crate::classement::resolu(&reel.join("code")).unwrap();
        assert_eq!(racines, vec![attendu], "racine résolue");
        assert!(arbre_sans_lien_qui_sort(&x, &racines, &declare).is_ok());
        symlink(dehors.path(), declare.join("src/sortie")).unwrap();
        assert!(arbre_sans_lien_qui_sort(&x, &racines, &declare).is_err());
    }

    /// KOB-434 : `-Depth` implique `-Recurse` ; un tiret Unicode est un
    /// tiret de paramètre pour PowerShell ; `where /R` juge son dossier.
    /// KOB-447 : sous PowerShell, une option d'exécutable entre guillemets ou
    /// dans un tableau est vue comme l'option elle-même (classée comme sans
    /// guillemets) ; pour une cmdlet ou son alias, `'-x'` reste une valeur.
    #[test]
    fn option_d_executable_entre_guillemets_sous_powershell() {
        let x = ctx();
        let ps = Shell::PowerShell;
        let cle = |l: &str| analyser(l, ps, &x).classe.cle();
        for (cite, nu) in [
            ("rg '--pre=calc.exe' foo", "rg --pre=calc.exe foo"),
            ("rg foo,--pre=calc", "rg foo --pre=calc"),
            ("git grep '-Ocalc.exe' foo", "git grep -Ocalc.exe foo"),
            ("git diff '--output=C:/x.bat'", "git diff --output=C:/x.bat"),
            ("rg '-L' foo", "rg -L foo"),
            ("rg.exe '--pre=calc.exe' foo", "rg.exe --pre=calc.exe foo"),
        ] {
            assert_ne!(cle(cite), "lecture", "{cite}");
            assert_eq!(cle(cite), cle(nu), "{cite} / {nu}");
        }
        // Un `./-e` écrit tel quel est un fichier pour l'exécutable : il ne
        // devient pas l'option `-e` qui ferait sauter le chemin suivant.
        for l in [
            r"rg . ./-e C:\Windows\win.ini",
            r"head.exe ./-n C:\Windows\win.ini",
            r"grep.exe x ./-f ..\..\..\secret",
            "rg . '\u{2013}e' C:/Windows/win.ini",
            "head.exe '\u{2014}n' C:/Windows/win.ini",
            "rg . foo,\u{2013}e,C:/Windows/win.ini",
        ] {
            assert_ne!(cle(l), "lecture", "{l}");
        }
        // Cmdlet ou alias, `cd` compris : la valeur entre guillemets reste un
        // chemin jugé.
        for l in [
            r"Get-Content '-x\..\..\..\Windows\win.ini'",
            r"cat '-x\..\..\..\Windows\win.ini'",
            r"type '-x\..\..\..\Windows\win.ini'",
        ] {
            assert_eq!(cle(l), "refus", "{l}");
        }
        // Avec un paramètre, `cd`/`pushd` passe par la carte (KOB-458).
        for l in [
            r"cd '-x\..\..\..\Windows' -ea 0; gc win.ini",
            r"pushd '-x\..\..\..\Windows' -StackName .; gc win.ini",
        ] {
            assert_ne!(cle(l), "lecture", "{l}");
        }
    }

    /// KOB-448 : sous PowerShell, l'option de format de git entre guillemets
    /// n'est pas prise pour un chemin (refusée à tort depuis KOB-437).
    #[test]
    fn git_log_format_entre_guillemets_sous_powershell() {
        let x = ctx();
        for l in [
            "git log '--format=%h %s'",
            r#"git log "--pretty=format:%h %s""#,
        ] {
            assert!(lu(&analyser(l, Shell::PowerShell, &x)), "{l}");
        }
    }

    #[test]
    fn parcours_depth_tirets_et_where() {
        let v = |l: &str| l.split(' ').map(str::to_owned).collect::<Vec<_>>();
        assert!(parcours_recursif("gci", &v("-Depth 20"), Shell::PowerShell));
        // `-s` est un alias de `-Recurse` ; `-d` est `-Directory`.
        assert!(parcours_recursif("gci", &v("-s"), Shell::PowerShell));
        assert!(!parcours_recursif("gci", &v("-d src"), Shell::PowerShell));
        // `gc –LiteralPath:C:/Windows/win.ini` : le tiret long ne cache plus
        // le chemin, refusé hors des dossiers.
        let x = ctx();
        assert_eq!(
            analyser(
                "Get-Content \u{2013}LiteralPath:C:/Windows/win.ini",
                Shell::PowerShell,
                &x
            )
            .classe
            .cle(),
            "refus"
        );
        assert_eq!(
            analyser("where.exe /R C:/Users/moi/.ssh *", Shell::PowerShell, &x)
                .classe
                .cle(),
            "refus"
        );
        // Un chemin qui commence par un tiret (entre guillemets) : pas un
        // paramètre pour PowerShell, jugé comme un chemin.
        assert_eq!(
            analyser(
                r"Get-Content '-x\..\..\..\Windows\win.ini'",
                Shell::PowerShell,
                &x
            )
            .classe
            .cle(),
            "refus"
        );
        assert_eq!(
            analyser("where /R C:/Windows *.ini", Shell::Bash, &x)
                .classe
                .cle(),
            "refus"
        );
    }

    /// KOB-468 : un `cd` vers un lecteur PowerShell (`Env:`, `Variable:`,
    /// `HKCU:`) ne fait pas passer la suite pour un fichier du dossier :
    /// `cd Env:; gc ANTHROPIC_API_KEY` lisait la clé sans carte.
    #[test]
    fn cd_vers_un_lecteur_powershell() {
        let x = ctx();
        for l in [
            "cd Env:; gc ANTHROPIC_API_KEY",
            r"cd Env:\; gc ANTHROPIC_API_KEY",
            "Set-Location Env:; Get-Content ANTHROPIC_API_KEY",
            "Set-Location -Path Env:; gc ANTHROPIC_API_KEY",
            "Set-Location -LiteralPath Env:; gc ANTHROPIC_API_KEY",
            "sl Variable:; gc x",
            "chdir Env:; gc ANTHROPIC_API_KEY",
            "pushd Env:; gc ANTHROPIC_API_KEY",
            r"cd HKCU:\Software; gci",
            "cd alias:; ri where -Force",
            r"cd Microsoft.PowerShell.Core\Environment::; gc ANTHROPIC_API_KEY",
            r"gc Microsoft.PowerShell.Core\Environment::ANTHROPIC_API_KEY",
            "gc Env:ANTHROPIC_API_KEY",
        ] {
            assert_eq!(
                analyser(l, Shell::PowerShell, &x).classe.cle(),
                "refus",
                "{l}"
            );
        }
        for l in [
            r"gc FileSystem::C:\Users\moi\.ssh\id_rsa",
            r"gc -Path:Environment::ANTHROPIC_API_KEY",
            r"cd Registry::HKEY_CURRENT_USER; gci",
        ] {
            assert_eq!(
                analyser(l, Shell::PowerShell, &x).classe.cle(),
                "refus",
                "{l}"
            );
        }
        assert_eq!(
            analyser("cd src; gc a.rs", Shell::PowerShell, &x)
                .classe
                .cle(),
            "lecture"
        );
        // Historique des dossiers (PowerShell 6.2+) : jamais une lecture.
        for l in ["cd '-'; gc .ssh/id_rsa", "cd +; gc x", "cd -Path '-'; gc x"] {
            assert_ne!(
                analyser(l, Shell::PowerShell, &x).classe.cle(),
                "lecture",
                "{l}"
            );
        }
        // Un chemin Rust n'est pas un fournisseur.
        assert!(!fournisseur_ps("crate::commande"));
        assert!(fournisseur_ps(r"Microsoft.PowerShell.Core\Environment::X"));
    }

    /// KOB-470 : `@variable` (splatting) éclate la valeur en arguments :
    /// `echo @env:ANTHROPIC_API_KEY` affichait la clé, un caractère par ligne
    /// (sonde #425). Traité comme `$` : jamais une lecture.
    #[test]
    fn splatting_powershell_jamais_lu() {
        let x = ctx();
        for l in [
            "echo @env:ANTHROPIC_API_KEY",
            "Write-Output @env:ANTHROPIC_API_KEY",
            "gc @env:ANTHROPIC_API_KEY",
            "ls @env:ANTHROPIC_API_KEY",
            "echo @variable:HOME",
            "echo @global:x",
            "echo @x",
            "echo @_",
            "echo a,@env:X",
            "echo @{a=1}",
            "Get-ChildItem @p",
            "echo -InputObject:@env:ANTHROPIC_API_KEY",
            "Write-Host -Object:@env:ANTHROPIC_API_KEY",
            "echo \u{2013}InputObject:@env:ANTHROPIC_API_KEY",
            "Write-Host \u{2014}Object:@env:ANTHROPIC_API_KEY",
            "echo `\n@env:ANTHROPIC_API_KEY",
            "echo @^",
        ] {
            // Comme `$` : jamais une lecture (carte).
            assert_ne!(
                analyser(l, Shell::PowerShell, &x).classe.cle(),
                "lecture",
                "{l}"
            );
        }
        assert_eq!(
            analyser("echo @env:X", Shell::PowerShell, &x).classe,
            analyser("echo $env:X", Shell::PowerShell, &x).classe
        );
        // Au milieu d'un mot, ou entre guillemets, `@` n'est pas un splatting.
        for l in [
            "git log --format=%an@%ae",
            "echo a@b",
            "echo '@env:X'",
            "echo @",
        ] {
            assert!(lu(&analyser(l, Shell::PowerShell, &x)), "{l}");
        }
        // Sous Bash, `@x` n'est qu'un mot.
        assert_eq!(analyser("echo @x", Shell::Bash, &x).classe.cle(), "lecture");
    }

    /// KOB-471 : `--%` (arrêt de l'analyse) fait remplacer `%VAR%` par sa
    /// valeur pour un programme natif : `which --% %ANTHROPIC_API_KEY%`
    /// affichait la clé (sonde #427). Traité comme `$` : jamais une lecture.
    #[test]
    fn arret_de_l_analyse_powershell() {
        let x = ctx();
        for l in [
            "which --% %ANTHROPIC_API_KEY%",
            "rg --% %ANTHROPIC_API_KEY% .",
            "echo --% %X%",
            "git log --% %X%",
            "which \u{2013}-% %X%",
            "which --%%X%",
            "Get-ChildItem | which --% %X%",
            "which '--%' %ANTHROPIC_API_KEY%",
            "which \"--%\" %ANTHROPIC_API_KEY%",
            "rg x '--%' %X%",
            "which -x:--% %X%",
            "which -x:'--%' %X%",
            "which `--% %X%",
        ] {
            assert_ne!(
                analyser(l, Shell::PowerShell, &x).classe.cle(),
                "lecture",
                "{l}"
            );
        }
        assert_eq!(
            analyser("which --% %X%", Shell::PowerShell, &x).classe,
            analyser("which $env:X", Shell::PowerShell, &x).classe
        );
        // Sous Bash, `--%` n'est qu'un mot.
        assert_eq!(
            analyser("echo --%", Shell::Bash, &x).classe.cle(),
            "lecture"
        );
    }

    /// KOB-466 : une commande qui retire ou redéfinit un alias PowerShell
    /// passe par une carte qui le dit (sonde #421 : la carte disait
    /// « `Remove-Item` supprime des fichiers »).
    #[test]
    fn carte_qui_dit_qu_un_alias_change() {
        let x = ctx();
        for l in [
            "Remove-Item alias:where -Force",
            "del alias:where -Force",
            r"ri alias:\where -Force",
            "Remove-Item -Path alias:where -Force",
            "Remove-Item -Path:Alias:where -Force",
            r"Remove-Item 'Microsoft.PowerShell.Core\Alias::where' -Force",
            "Remove-Item x.txt,alias:where -Force",
            "Remove-Alias where -Force",
            "Set-Alias where where.exe -Force",
            "sal where where.exe",
            "New-Alias w where.exe",
            "nal w where.exe",
            "Import-Alias alias.csv",
            "Set-Item alias:where where.exe -Force",
            "New-Item -Path alias: -Name where -Value where.exe",
            "Rename-Item alias:where w -Force",
            r"Remove-Item alias:where -Force; where C:\Users\moi\.ssh:*",
        ] {
            let a = analyser(l, Shell::PowerShell, &x);
            assert_ne!(a.classe.cle(), "lecture", "{l}");
            assert!(a.raison.contains("alias"), "{l} : {}", a.raison);
        }
        // Les commandes de lecture connues gardent leurs contrôles, et une
        // suppression de fichiers reste une suppression.
        assert_eq!(
            analyser(
                r"cat alias:x C:\Users\moi\.ssh\id_rsa",
                Shell::PowerShell,
                &x
            )
            .classe
            .cle(),
            "refus"
        );
        for l in [
            "git clean -fdx . alias:x",
            "Remove-Item -Recurse src,alias:x -Force",
        ] {
            let a = analyser(l, Shell::PowerShell, &x);
            assert_eq!(a.classe, Classe::Ecriture(Genre::Suppression), "{l}");
        }
        assert!(analyser("pwsh -c x alias:x", Shell::PowerShell, &x)
            .raison
            .contains("n'importe quelle commande"));
        // Sous Bash, `alias:x` n'est qu'un mot.
        assert!(!analyser("rm alias:x", Shell::Bash, &x)
            .raison
            .contains("alias PowerShell"));
    }

    /// Relecture de KOB-483 : deux écritures de même rang gardent leurs deux
    /// raisons ; la carte ne montre pas seulement la plus anodine.
    #[test]
    fn raisons_cumulees_a_rang_egal() {
        let x = ctx();
        let a = analyser("touch notes.md; bash -c x", Shell::Bash, &x);
        assert!(
            a.raison.contains("touch") && a.raison.contains("bash"),
            "{}",
            a.raison
        );
        // Une seule fois la même raison.
        let a = analyser("touch a; touch a", Shell::Bash, &x);
        assert_eq!(a.raison.matches("touch").count(), 1, "{}", a.raison);
        // Rangs différents : la suppression en tête, l'autre raison gardée.
        let a = analyser("rm a; bash -c x", Shell::Bash, &x);
        assert_eq!(a.classe, Classe::Ecriture(Genre::Suppression));
        assert!(
            a.raison.starts_with("`rm`") && a.raison.contains("bash"),
            "{}",
            a.raison
        );
        // Un refus garde sa raison seule.
        let a = analyser("touch a; cat /etc/passwd", Shell::Bash, &x);
        assert_eq!(a.classe.cle(), "refus");
    }

    /// KOB-439 : `where.exe` juge le dossier de `-R` comme celui de `/R`, et
    /// la partie chemin d'un motif `chemin:motif` ; `where git` reste une
    /// lecture, et l'alias `Where-Object` de PowerShell n'est pas touché.
    #[test]
    fn where_exe_juge_ses_dossiers() {
        let x = ctx();
        let cle = |l: &str, s| analyser(l, s, &x).classe.cle();
        for shell in [Shell::PowerShell, Shell::Bash] {
            for l in [
                "where.exe -R C:/Users/moi/.ssh *",
                "where.exe -r C:/Users/moi/.ssh *",
                "where.exe /R C:/Users/moi/.ssh *",
                "where.exe C:/Windows:*.ini",
                r"where.exe C:\Windows:*.ini",
                r"where.exe \\hote\partage:x",
                "where.exe /Q C:/Users/moi/.ssh:*",
            ] {
                assert_eq!(cle(l, shell), "refus", "{l} ({shell:?})");
            }
            for l in [
                "where.exe $PATH:*.exe",
                "where.exe C:*.ini",
                "where.exe /X git",
                "where.exe -Rx C:/Windows *",
                r"where.exe /RC:\Windows *",
                "where.exe /R src /R tests *",
                r"where.exe src;C:\Windows:*.ini",
            ] {
                assert_ne!(cle(l, shell), "lecture", "{l} ({shell:?})");
            }
            for l in [
                "where.exe git",
                "where.exe /Q git",
                "where.exe /R C:/Users/moi/Desktop/kobi '*.rs'",
                "where.exe C:/Users/moi/Desktop/kobi/src:*.rs",
            ] {
                assert_eq!(cle(l, shell), "lecture", "{l} ({shell:?})");
            }
        }
        // Non cité, le joker s'étend sous Git Bash (KOB-489), pas sous PowerShell.
        let non_cite = "where.exe /R C:/Users/moi/Desktop/kobi *.rs";
        assert_eq!(cle(non_cite, Shell::PowerShell), "lecture");
        assert_ne!(cle(non_cite, Shell::Bash), "lecture");
        // Sans `.exe`, `where` est le programme sous Bash…
        assert_eq!(cle("where -R C:/Windows *.ini", Shell::Bash), "refus");
        assert_eq!(cle("where C:/Windows:*.ini", Shell::Bash), "refus");
        assert_eq!(cle("where git", Shell::Bash), "lecture");
        // La virgule de PowerShell sépare les arguments (KOB-437) : le `/R`
        // d'un tableau est jugé comme les autres.
        assert_eq!(
            cle("where.exe x,/R,C:/Users/moi/.ssh,*", Shell::PowerShell),
            "refus"
        );
        // `'-R'` ou `x,-R` : PowerShell passe `-R` à where.exe, même si le
        // découpage le garde comme valeur (`./-R`).
        for l in [
            r"where.exe '-R' \Users\moi\.ssh *",
            r"where.exe x,-R,\Users\moi\.ssh,*",
            r"where.exe '-R' ..\..\.. *",
            r#"where.exe "-r" ..\..\.. *"#,
        ] {
            assert_ne!(cle(l, Shell::PowerShell), "lecture", "{l}");
        }
        assert!(parcours_recursif(
            "where",
            &v_mots("./-R . *.rs"),
            Shell::PowerShell
        ));
        // … et l'alias `Where-Object` sous PowerShell (pipeline).
        assert_eq!(
            cle("Get-ChildItem | where Name -like 'a:b*'", Shell::PowerShell),
            "lecture"
        );
        assert!(parcours_recursif(
            "where",
            &v_mots("-R . *.rs"),
            Shell::Bash
        ));
    }

    /// KOB-482 : sous Bash, un `$` juste avant le guillemet fermant est
    /// littéral (`printf "[%s]" "foo$"` affiche `[foo$]`) : `grep "foo$"` est
    /// une lecture. Tout autre `$` entre guillemets doubles reste refusé.
    #[test]
    fn dollar_final_entre_guillemets_bash() {
        let x = ctx();
        for l in [
            "grep \"foo$\" README.md",
            "rg \"^fn.*\\{$\" src",
            "grep -n \"TODO$\" src/main.rs",
        ] {
            assert_eq!(analyser(l, Shell::Bash, &x).classe.cle(), "lecture", "{l}");
        }
        for l in [
            "grep \"$x\" README.md",
            "grep \"${x}\" README.md",
            "grep \"$(id)\" README.md",
            "grep \"a$b\" README.md",
            "grep \"$\"x\" README.md",
            "grep \"$\\\"\" README.md",
            "grep \"$\" ../../etc",
        ] {
            assert_ne!(analyser(l, Shell::Bash, &x).classe.cle(), "lecture", "{l}");
        }
        // Sous PowerShell, non prouvé : rien ne change.
        assert_ne!(
            analyser("Select-String \"foo$\" README.md", Shell::PowerShell, &x)
                .classe
                .cle(),
            "lecture"
        );
    }

    /// KOB-482, avec les vrais shells : Kobi classe zsh comme Bash ; là où
    /// zsh (macos-latest) ou dash est installé, `"foo$"` y reste littéral.
    #[cfg(unix)]
    #[test]
    fn dollar_final_litteral_dans_les_vrais_shells() {
        for shell in ["bash", "zsh", "dash", "sh"] {
            let Ok(o) = std::process::Command::new(shell)
                .args(["-c", "printf '[%s]' \"foo$\" \"$\""])
                .output()
            else {
                continue; // shell absent
            };
            assert_eq!(String::from_utf8_lossy(&o.stdout), "[foo$][$]", "{shell}");
        }
    }

    /// KOB-477 : sous pwsh 7, `` `u{2E} `` entre guillemets doubles vaut `.` :
    /// `Get-Content "`u{2E}`u{2E}/x"` lisait hors du dossier et
    /// `jq -n "`u{65}nv.X"` affichait un secret (sonde #439). Tout accent
    /// grave entre guillemets doubles passe par la carte, comme `$`.
    #[test]
    fn accent_grave_entre_guillemets_doubles() {
        let x = ctx();
        for l in [
            "jq -n \"`u{65}nv.ANTHROPIC_API_KEY\"",
            "jq -n \"`u{24}ENV.HOME\"",
            "Get-Content \"`u{2E}`u{2E}/dehors/secret.txt\"",
            "gc \"`u{2E}`u{2E}`u{5C}dehors`u{5C}secret.txt\"",
            "cat \"`u{2E}`u{2E}/dehors/secret.txt\"",
            "Get-Content \"`0..`/x\"",
            "Get-Content \"a`\"b\"",
            "Get-Content \"`$x\"",
            "echo \"`n\"",
        ] {
            let a = analyser(l, Shell::PowerShell, &x);
            assert_ne!(a.classe.cle(), "lecture", "{l}");
            assert_eq!(
                a.classe,
                analyser("echo $x", Shell::PowerShell, &x).classe,
                "{l} : {}",
                a.raison
            );
        }
        // Entre guillemets simples, l'accent grave est littéral ; hors
        // guillemets, il échappe un caractère (déjà géré).
        assert_eq!(
            analyser("Get-Content 'a`b'", Shell::PowerShell, &x)
                .classe
                .cle(),
            "lecture"
        );
        assert_eq!(
            analyser("Get-Content \"src/a.rs\"", Shell::PowerShell, &x)
                .classe
                .cle(),
            "lecture"
        );
        // Sous Bash, rien ne change : l'accent grave y est une substitution,
        // déjà sur la carte, et `"src/a.rs"` reste une lecture.
        assert_eq!(
            analyser("cat \"src/a.rs\"", Shell::Bash, &x).classe.cle(),
            "lecture"
        );
    }

    /// KOB-477, avec le vrai pwsh : une ligne que pwsh 7 exécute en lisant
    /// hors du dossier ou en affichant une variable n'est jamais une lecture.
    #[cfg(windows)]
    #[test]
    fn accent_grave_avec_le_vrai_pwsh() {
        let racine = tempfile::tempdir().unwrap();
        let code = racine.path().join("code");
        std::fs::create_dir_all(&code).unwrap();
        std::fs::create_dir_all(racine.path().join("dehors")).unwrap();
        std::fs::write(
            racine.path().join("dehors").join("secret.txt"),
            "contenu-dehors",
        )
        .unwrap();
        let mut x = Contexte::new(vec![code.clone()]);
        x.cwd = Some(code.clone());
        for l in [
            "Get-Content \"`u{2E}`u{2E}/dehors/secret.txt\"",
            "cat \"`u{2E}`u{2E}`u{5C}dehors`u{5C}secret.txt\"",
            // Le vecteur de la sonde : `` `u{65} `` donne `e`, donc `env`.
            "jq -n \"`u{65}nv.KOBI_TEST_477\"",
        ] {
            let Ok(o) = std::process::Command::new("pwsh")
                .args(["-NoProfile", "-NonInteractive", "-Command", l])
                .current_dir(&code)
                .env("KOBI_TEST_477", "valeur-de-test")
                .output()
            else {
                return; // pwsh absent : rien à vérifier ici.
            };
            let sortie = String::from_utf8_lossy(&o.stdout);
            // jq affiche la valeur entre guillemets.
            let fuite = sortie.contains("contenu-dehors") || sortie.contains("valeur-de-test");
            if fuite {
                assert_ne!(
                    analyser(l, Shell::PowerShell, &x).classe.cle(),
                    "lecture",
                    "{l}"
                );
            }
        }
    }

    /// KOB-479 : hors guillemets, `` `-Pattern `` est une valeur pour
    /// PowerShell : `Select-String -NotMatch `-Pattern ../x` affichait un
    /// fichier hors du dossier (sonde #441). Tout accent grave hors guillemets
    /// passe par la carte, sauf la continuation de fin de ligne.
    #[test]
    fn accent_grave_hors_guillemets() {
        let x = ctx();
        for l in [
            "Select-String -NotMatch `-Pattern ../dehors/secret.txt",
            "Select-String -NotMatch `\u{2013}Pattern ../dehors/secret.txt",
            "sls -NotMatch `-Pattern ..\\dehors\\secret.txt",
            "Get-Content a`tb",
            "Get-Content `u{2E}`u{2E}/x",
            "g`ci -Recurse",
        ] {
            let a = analyser(l, Shell::PowerShell, &x);
            assert_ne!(a.classe.cle(), "lecture", "{l}");
            assert_eq!(
                a.classe,
                analyser("echo $x", Shell::PowerShell, &x).classe,
                "{l} : {}",
                a.raison
            );
        }
        // Continuation au milieu d'un mot : carte (`--pre` y serait recollé).
        assert_ne!(
            analyser("rg x src`\n--pre=./p a", Shell::PowerShell, &x)
                .classe
                .cle(),
            "lecture"
        );
        // La continuation de fin de ligne ne change pas (KOB-474).
        assert_eq!(
            analyser("Get-ChildItem `\nsrc", Shell::PowerShell, &x)
                .classe
                .cle(),
            "lecture"
        );
        // Après un mot, suivie d'un blanc : deux mots, comme pour pwsh.
        for (avec, sans) in [
            ("Get-ChildItem src`\n -Name", "Get-ChildItem src -Name"),
            (
                "Get-ChildItem src `\n -Recurse",
                "Get-ChildItem src -Recurse",
            ),
        ] {
            assert_eq!(
                analyser(avec, Shell::PowerShell, &x).classe,
                analyser(sans, Shell::PowerShell, &x).classe,
                "{avec}"
            );
        }
        // Sous Bash, `\` reste l'échappement.
        assert_eq!(
            analyser("cat src/a\\ b.rs", Shell::Bash, &x).classe.cle(),
            analyser("cat 'src/a b.rs'", Shell::Bash, &x).classe.cle()
        );
    }

    /// KOB-475 : sous bash, un retour chariot hors guillemets est un caractère
    /// ordinaire, pas un séparateur. Lu comme un séparateur, il cachait des
    /// options à la commande : `x␍#` n'ouvre pas de commentaire pour bash.
    #[test]
    fn retour_chariot_hors_guillemets_find_exec() {
        let ligne = "find . \r echo -maxdepth 0 -exec touch m \\;";
        assert_ne!(bash(ligne).classe, Classe::Lecture, "{ligne:?}");
    }

    #[test]
    fn retour_chariot_hors_guillemets_rg_pre() {
        let ligne = "rg -g x\r# --pre=./p a f";
        assert_ne!(bash(ligne).classe, Classe::Lecture, "{ligne:?}");
    }

    #[test]
    fn retour_chariot_hors_guillemets_git_log() {
        let ligne = "git log --grep=x\r# --output=m";
        assert_ne!(bash(ligne).classe, Classe::Lecture, "{ligne:?}");
    }

    #[test]
    fn retour_chariot_en_fin_de_ligne_bash() {
        assert_ne!(bash("grep a f\r").classe, Classe::Lecture);
        assert_ne!(bash("grep a f\r\n").classe, Classe::Lecture);
    }

    /// Le retour chariot reste un séparateur sous PowerShell, et un caractère
    /// ordinaire entre guillemets sous bash.
    #[test]
    fn retour_chariot_temoins() {
        for ligne in ["grep 'a\rb' Cargo.toml", "grep \"a\rb\" Cargo.toml"] {
            assert_eq!(bash(ligne).classe, Classe::Lecture, "{ligne:?}");
        }
        assert_eq!(ps("Get-ChildItem\rGet-ChildItem").classe, Classe::Lecture);
    }

    /// KOB-475 : `>&1M` n'est pas `>&1` suivi d'un mot : bash écrit le fichier
    /// `1M`.
    #[test]
    fn redirection_descripteur_colle_a_un_mot() {
        carte_redirection("sort f >&1m");
    }

    #[test]
    fn redirection_descripteur_colle_a_un_diese() {
        carte_redirection("grep a f >&1#m");
    }

    #[test]
    fn redirection_descripteur_colle_a_des_guillemets() {
        carte_redirection("grep a f >&1\"m\"");
        carte_redirection("grep a f >&1'm'");
    }

    #[test]
    fn redirection_descripteur_colle_a_un_echappement() {
        carte_redirection("grep a f >&1\\m");
        carte_redirection("grep a f >&1\\\nm");
    }

    #[test]
    fn redirection_tiret_colle_a_un_mot() {
        carte_redirection("grep a f >&-m");
        carte_redirection("grep a f 2>&1-m");
    }

    #[track_caller]
    fn carte_redirection(ligne: &str) {
        assert_ne!(bash(ligne).classe, Classe::Lecture, "{ligne:?}");
    }

    /// Seuls l'espace, la tabulation et le saut de ligne séparent pour bash :
    /// un autre blanc reste dans le nom du fichier.
    #[test]
    fn redirection_descripteur_colle_a_un_blanc_rare() {
        for blanc in ['\u{a0}', '\u{c}', '\u{b}', '\r'] {
            carte_redirection(&format!("grep a f >&1{blanc}m"));
        }
    }

    /// `>&1-` déplace un descripteur ; `>&1-2` et `>&-2` écrivent un fichier.
    #[test]
    fn redirection_tiret_suivi_de_chiffres() {
        carte_redirection("grep a f >&1-2");
        carte_redirection("grep a f >&-2");
    }

    /// Un numéro cité ou échappé est un mot, pas un descripteur : la commande
    /// le reçoit (`uniq f '2'>&1` écrit le fichier `2`).
    #[test]
    fn redirection_numero_cite_est_un_mot() {
        let attendu = vec![
            Jeton::Mot("x".into()),
            Jeton::Mot("2".into()),
            Jeton::Redir(">&1".into()),
        ];
        for ligne in [
            "x '2'>&1",
            "x \"2\">&1",
            "x \\2>&1",
            "x 2''>&1",
            "x ''2>&1",
            "x \"\"2>&1",
            "x ''\\\n2>&1",
        ] {
            assert_eq!(decouper(ligne, Shell::Bash), Ok(attendu.clone()), "{ligne}");
        }
        assert_eq!(
            decouper("x 2>&1", Shell::Bash),
            Ok(vec![Jeton::Mot("x".into()), Jeton::Redir("2>&1".into())])
        );
        // Bash ne prend pour descripteur qu'un numéro qui tient sur 31 bits.
        assert_eq!(
            decouper("x 2147483648>&1", Shell::Bash),
            Ok(vec![
                Jeton::Mot("x".into()),
                Jeton::Mot("2147483648".into()),
                Jeton::Redir(">&1".into()),
            ])
        );
        assert_eq!(
            decouper("x 2147483647>&1", Shell::Bash),
            Ok(vec![
                Jeton::Mot("x".into()),
                Jeton::Redir("2147483647>&1".into()),
            ])
        );
    }

    /// Sous bash, seul `/dev/null` jette la sortie ; `null` et `nul` sont des
    /// fichiers. Sous PowerShell, `null` et `'$null'` aussi.
    #[test]
    fn redirection_vers_un_faux_nul() {
        for ligne in [
            "grep a f >null",
            "grep a f >nul",
            "grep a f >NUL",
            "grep a f >'$null'",
            "grep a f >/DEV/NULL",
        ] {
            carte_redirection(ligne);
        }
        for ligne in ["Get-ChildItem > null", "Get-ChildItem > '$null'"] {
            assert_ne!(ps(ligne).classe, Classe::Lecture, "{ligne:?}");
        }
        assert_eq!(
            bash("grep a Cargo.toml 2>/dev/null").classe,
            Classe::Lecture
        );
        assert_eq!(bash("grep a Cargo.toml >/dev/null").classe, Classe::Lecture);
    }

    /// Les vraies copies de descripteur restent des lectures.
    #[test]
    fn redirection_descripteur_temoins() {
        for ligne in [
            "grep a Cargo.toml 2>&1",
            "grep a Cargo.toml >&2",
            "grep a Cargo.toml 2>&1 | head -1",
            "grep a Cargo.toml 2>&1;ls",
            "grep a Cargo.toml 2>&1 && ls",
            "grep a Cargo.toml 2>&1\nls",
            "grep a Cargo.toml 2>&1 # note",
        ] {
            assert_eq!(bash(ligne).classe, Classe::Lecture, "{ligne:?}");
        }
    }

    /// KOB-488 : le garde-fou de KOB-473 comparait sans les options : `git
    /// version --help` restait une lecture et lançait `man.<outil>.cmd` (vu
    /// avec le vrai git dans `tests/git_version_help.rs`).
    #[test]
    fn git_version_avec_autre_chose_sur_la_carte() {
        for ligne in [
            "git version --help",
            "git version -h",
            "git version --hel",
            "git version --he",
            "git version --build-options",
            "git version --build-options --help",
            "git version --help --build-options",
            "git version -- --help",
            "git --no-pager version --help",
            "git -P version --help",
            "git --bare version --help",
            "git --no-optional-locks version --help",
            "git --literal-pathspecs version --help",
            "git --git-dir=x version --help",
            "git --work-tree=x version --help",
            "git --no-pager version",
            "git -P version",
            "git --bare version",
            "git --git-dir=x version",
            "git -C version",
            "git \"version\" --help",
            "git version \"--help\"",
            "git version '--help'",
            "git version --\\help",
        ] {
            assert_ne!(bash(ligne).classe, Classe::Lecture, "{ligne}");
            assert_ne!(ps(ligne).classe, Classe::Lecture, "{ligne}");
        }
        assert_ne!(bash("git ver\\sion --help").classe, Classe::Lecture);
        assert_ne!(bash("git version \\--help").classe, Classe::Lecture);
        assert_ne!(bash("git version \\\n--help").classe, Classe::Lecture);
    }

    /// La valeur de `-C` qui commence par un tiret échappait au garde-fou : avec
    /// `-*`, les noms de fichiers choisissent les arguments de git.
    #[test]
    fn git_c_valeur_a_tiret_sur_la_carte() {
        for ligne in ["git -C -*", "git -C -x", "git -C --help", "git -C -- "] {
            assert_ne!(bash(ligne).classe, Classe::Lecture, "{ligne}");
        }
    }

    /// `git version` exact, cité ou non, reste une lecture.
    #[test]
    fn git_version_exact_en_lecture() {
        for ligne in ["git version", "git \"version\"", "git 'version'"] {
            assert_eq!(bash(ligne).classe, Classe::Lecture, "{ligne}");
            assert_eq!(ps(ligne).classe, Classe::Lecture, "{ligne}");
        }
    }

    /// KOB-473 : garde-fou en attendant KOB-469. Une lecture git peut lancer
    /// un programme configuré dans le dépôt (core.fsmonitor sur status et
    /// diff, diff.external d'un dépôt nu intégré sur diff et show, gpg sur
    /// log --show-signature…) : toujours sur la carte.
    #[test]
    fn lectures_git_sur_la_carte() {
        let x = ctx();
        for l in [
            "git status",
            "git diff",
            "git show",
            "git log",
            "git log -p",
            "git diff HEAD~1 HEAD",
            "cd src && git diff HEAD~1 HEAD",
            "git -C src status",
            "git --no-pager log",
            "git ls-files",
            "git grep motif",
            "git blame a.rs",
            "git verify-commit HEAD",
            "git config --get user.name",
            "git branch --list",
            "git -C version status",
            // `git status --help` devient `git help status`, qui lance le
            // `man.<outil>.cmd` de la config du dépôt (relecture de KOB-473).
            "git status --help",
            "git log --help",
        ] {
            let a = analyser(l, Shell::Bash, &x);
            assert_ne!(a.classe.cle(), "lecture", "{l}");
            assert_ne!(
                analyser(l, Shell::PowerShell, &x).classe.cle(),
                "lecture",
                "{l} (PowerShell)"
            );
        }
        let raison = analyser("git status", Shell::Bash, &x).raison;
        assert!(raison.contains("programmes configurés"), "{raison}");
        // Sans dépôt lu : comme avant.
        assert_eq!(
            analyser("git version", Shell::Bash, &x).classe.cle(),
            "lecture"
        );
        // Les suppressions gardent leur genre.
        assert_eq!(
            analyser("git branch -d x", Shell::Bash, &x).classe,
            Classe::Ecriture(Genre::Suppression)
        );
    }

    /// KOB-455 : sous PowerShell, `where` sans `.exe` est l'alias
    /// `Where-Object`, où un `/r` est une valeur : pas de refus. Il passe par
    /// la carte, car l'alias peut avoir été retiré (where.exe).
    #[test]
    fn where_object_et_slash_r() {
        let x = ctx();
        let cle = |l: &str, s| analyser(l, s, &x).classe.cle();
        for l in [
            "Get-ChildItem | where Name -eq /r",
            "Get-ChildItem | where Name -eq /R",
            "Get-ChildItem | where Name -eq -r",
            "Get-ChildItem | where Name -eq /r | select Name",
            r"Get-ChildItem | where Name -eq /r C:\Windows",
            r"where /R C:\Users\moi\.ssh *",
        ] {
            assert_eq!(cle(l, Shell::PowerShell), "ecriture", "{l}");
        }
        assert_eq!(
            cle("Get-ChildItem | where Name -eq src", Shell::PowerShell),
            "lecture"
        );
        // where.exe, lui, garde son contrôle.
        assert_eq!(cle("where.exe /r", Shell::PowerShell), "refus");
        assert_eq!(cle("where /r", Shell::Bash), "refus");
    }

    /// KOB-456 : `-FollowSymlink` abrégé (`-Follow`, `-Fol`) suit aussi les
    /// liens (pwsh hors de Windows) : sur la carte, comme la forme entière.
    #[test]
    fn followsymlink_abrege() {
        let x = ctx();
        for l in [
            "Get-ChildItem -Recurse -Follow",
            "gci -Recurse -Fol",
            "gci -r -FollowSymlink",
            "ls -Recurse -FollowSymlink",
            "dir -Recurse -follows",
            "gci -Recurse -Follow:true",
        ] {
            assert_ne!(
                analyser(l, Shell::PowerShell, &x).classe.cle(),
                "lecture",
                "{l}"
            );
        }
        // Sans lien suivi, ou sous Bash (`sort -f`), rien ne change.
        assert_eq!(
            analyser("gci -Recurse -Filter *.rs", Shell::PowerShell, &x)
                .classe
                .cle(),
            "lecture"
        );
        assert_eq!(
            analyser("gci -Force", Shell::PowerShell, &x).classe.cle(),
            "lecture"
        );
        assert_eq!(
            analyser("sort -f a.txt", Shell::Bash, &x).classe.cle(),
            "lecture"
        );
    }

    /// KOB-472 : GNU `dir` suit les liens avec `-L` comme `ls` ; sous
    /// PowerShell, `dir -L` est `Get-ChildItem -LiteralPath`.
    #[test]
    fn dir_l_suit_les_liens_sous_bash() {
        let x = ctx();
        for l in ["dir -RL .", "dir -R --dereference .", "dir -LR src"] {
            assert_ne!(analyser(l, Shell::Bash, &x).classe.cle(), "lecture", "{l}");
        }
        assert_eq!(
            analyser("dir -R src", Shell::Bash, &x).classe.cle(),
            "lecture"
        );
        assert_eq!(
            analyser("dir -L src", Shell::PowerShell, &x).classe,
            analyser("gci -L src", Shell::PowerShell, &x).classe
        );
    }

    /// KOB-449 : sous Git Bash, `//R` est le `/R` de where.exe (MSYS le
    /// ramène à `/R`) : son dossier est jugé, comme celui de `/R`.
    #[test]
    fn where_double_barre_sous_git_bash() {
        let x = ctx();
        let cle = |l: &str| analyser(l, Shell::Bash, &x).classe.cle();
        for l in [
            "where //R /c/Users/moi/.ssh '*'",
            "where //r C:/Users/moi/.ssh '*'",
            "where //R src //R /c/Users/moi/.ssh '*'",
        ] {
            assert_ne!(cle(l), "lecture", "{l}");
        }
        // Un chemin réseau (`//hôte/partage`) n'est pas ramené à un chemin
        // local : il reste hors des dossiers.
        for l in [
            "where //c/Users/moi/Desktop/kobi:'*'",
            "where //cygdrive/c/Users/moi/Desktop/kobi:'*'",
        ] {
            assert_ne!(cle(l), "lecture", "{l}");
            assert_ne!(
                analyser(
                    &l.replace("where", "where.exe").replace('\'', ""),
                    Shell::PowerShell,
                    &x
                )
                .classe
                .cle(),
                "lecture",
                "{l} (PowerShell)"
            );
        }
        // Variante à barres obliques inverses sous PowerShell : jamais une
        // lecture (where.exe y voit un chemin réseau).
        assert_ne!(
            analyser(
                r"where.exe //c\Users\moi\Desktop\kobi:*",
                Shell::PowerShell,
                &x
            )
            .classe
            .cle(),
            "lecture"
        );
        assert_eq!(cle("where //R C:/Users/moi/Desktop/kobi '*.rs'"), "lecture");
        assert_eq!(cle("where //Q git"), "lecture");
        assert_ne!(cle("where //X git"), "lecture");
    }

    /// KOB-454 : `grep -S` (grep BSD de macOS) suit tous les liens, comme
    /// `-R` : sur la carte, seul ou groupé (`-rS`).
    #[test]
    fn grep_s_suit_les_liens() {
        let x = ctx();
        for l in ["grep -rS motif .", "grep -S motif src", "egrep -SR x ."] {
            assert_ne!(analyser(l, Shell::Bash, &x).classe, Classe::Lecture, "{l}");
        }
        assert_eq!(
            analyser("grep -r motif .", Shell::Bash, &x).classe,
            Classe::Lecture
        );
    }

    /// KOB-453 : `cd -P` suit les liens physiquement ; le dossier courant
    /// suivi sur le texte ne vaut plus, la suite passe par la carte.
    #[cfg(unix)]
    #[test]
    fn cd_physique_sous_bash() {
        use std::os::unix::fs::symlink;
        let racine = tempfile::tempdir().unwrap();
        let code = racine.path().join("code");
        let dehors = racine.path().join("dehors");
        std::fs::create_dir_all(code.join("sous/profond")).unwrap();
        std::fs::create_dir_all(&dehors).unwrap();
        std::fs::write(dehors.join("secret.txt"), "x").unwrap();
        symlink(code.join("sous/profond"), code.join("dedans")).unwrap();
        symlink(dehors.join("secret.txt"), code.join("sous/y")).unwrap();
        let mut x = Contexte::new(vec![code.clone()]);
        x.cwd = Some(code.clone());
        for l in [
            "cd -P dedans/.. && cat y",
            "cd dedans && cd -P .. && cat y",
            "cd -LP dedans/.. && cat y",
        ] {
            assert_ne!(analyser(l, Shell::Bash, &x).classe, Classe::Lecture, "{l}");
        }
        // `cd` logique (par défaut) : le texte suffit.
        assert_eq!(
            analyser("cd sous && ls", Shell::Bash, &x).classe,
            Classe::Lecture
        );
    }

    /// Le dossier courant suivi sur le texte diverge aussi sans `-P` : un
    /// `cd` logique qui échoue est refait physiquement par bash, `pushd -n`
    /// ne change pas de dossier, `pushd +N` tourne la pile, et zsh lit
    /// `cd ancien nouveau` comme un remplacement dans `$PWD`.
    #[cfg(unix)]
    #[test]
    fn dossier_courant_qui_diverge_sous_bash() {
        use std::os::unix::fs::symlink;
        let racine = tempfile::tempdir().unwrap();
        let code = racine.path().join("code");
        let dehors = racine.path().join("dehors");
        std::fs::create_dir_all(code.join("sous/profond")).unwrap();
        std::fs::create_dir_all(code.join("sous/z")).unwrap();
        std::fs::create_dir_all(code.join("a/b")).unwrap();
        std::fs::create_dir_all(&dehors).unwrap();
        std::fs::write(dehors.join("secret.txt"), "x").unwrap();
        symlink(code.join("sous/profond"), code.join("dedans")).unwrap();
        symlink(dehors.join("secret.txt"), code.join("sous/z/lien")).unwrap();
        let mut x = Contexte::new(vec![code.clone()]);
        x.cwd = Some(code.clone());
        for l in [
            "cd dedans/../z && cat lien",
            "pushd -n a/b && cat ../dehors/secret.txt",
            "pushd +1 && cat ../dehors/secret.txt",
            "cd a/b a && cat ../dehors/secret.txt",
            "pushd a/b && pushd '+1 ' && cat ../../../dehors/secret.txt",
            "pushd a/b && cd +1 && cat ../../../dehors/secret.txt",
            "cd a/b -L && cat ../dehors/secret.txt",
        ] {
            assert_ne!(analyser(l, Shell::Bash, &x).classe, Classe::Lecture, "{l}");
        }
        assert_eq!(
            analyser("cd a/b && ls", Shell::Bash, &x).classe,
            Classe::Lecture
        );
        assert_eq!(
            analyser("pushd a && ls", Shell::Bash, &x).classe,
            Classe::Lecture
        );
    }

    fn v_mots(l: &str) -> Vec<String> {
        l.split(' ').map(str::to_owned).collect()
    }

    /// KOB-434, sous Windows : un parcours récursif qui passerait par une
    /// jonction ou un lien de dossier qui sort n'est jamais une lecture sans
    /// carte ; sans lien qui sort, il reste une lecture.
    #[cfg(windows)]
    #[test]
    fn parcours_recursif_et_jonction_qui_sort() {
        let code = tempfile::tempdir().unwrap();
        let dehors = tempfile::tempdir().unwrap();
        let c = code.path();
        std::fs::create_dir_all(c.join("src")).unwrap();
        std::fs::write(c.join("src/a.rs"), "").unwrap();
        let mut x = Contexte::new(vec![c.to_path_buf()]);
        x.cwd = Some(c.to_path_buf());
        // Formes réelles sous Windows (PowerShell) ; `dir /S` ou `tree /F`
        // y prendraient `/S` pour un chemin, déjà refusé hors des dossiers.
        let lignes = [
            ("Get-ChildItem -Recurse", Shell::PowerShell),
            ("gci src -rec", Shell::PowerShell),
            ("tree", Shell::PowerShell),
            ("gci -Depth 5", Shell::PowerShell),
            ("gci \u{2013}Recurse", Shell::PowerShell),
        ];
        for (l, sh) in lignes {
            assert_eq!(
                analyser(l, sh, &x).classe,
                Classe::Lecture,
                "{l} sans lien qui sort"
            );
        }
        // Un parcours récursif qui reçoit ses chemins d'un `|` : sur la carte,
        // même sans lien qui sort.
        assert_ne!(
            analyser("gci src | gci -Recurse", Shell::PowerShell, &x).classe,
            Classe::Lecture
        );
        let j = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(c.join("src").join("jonction"))
            .arg(dehors.path())
            .output()
            .unwrap();
        assert!(j.status.success(), "mklink /J");
        for (l, sh) in lignes {
            assert_ne!(
                analyser(l, sh, &x).classe,
                Classe::Lecture,
                "{l} par une jonction qui sort"
            );
        }
    }

    /// KOB-457 : la valeur d'un paramètre que le parcours ne connaît pas
    /// (`-ErrorAction SilentlyContinue`, `-ea 0`, `-Exclude a,b`) ne cache plus
    /// le dossier courant : il est parcouru aussi.
    #[test]
    fn parcours_avec_un_parametre_inconnu_garde_le_dossier_courant() {
        let code = tempfile::tempdir().unwrap();
        let c = code.path();
        let x = Contexte::new(vec![c.to_path_buf()]);
        let courant = crate::classement::resolu(c).unwrap();
        for l in [
            "-Recurse -ErrorAction SilentlyContinue",
            "-Recurse -ea 0",
            "-Recurse -Exclude target node_modules",
        ] {
            let r = racines_du_parcours("get-childitem", &v_mots(l), &x, c).unwrap();
            assert!(r.contains(&courant), "{l} : {r:?}");
        }
        // Un mot à tiret avec `/` ou `\` est un chemin pour PowerShell : depuis
        // `src`, `-x/../..` part du dossier de code, pas de `src`.
        std::fs::create_dir_all(c.join("src")).unwrap();
        let r = racines_du_parcours(
            "get-childitem",
            &v_mots("-Recurse -x/../.."),
            &x,
            &c.join("src"),
        )
        .unwrap();
        assert!(r.contains(&courant), "-x/../.. : {r:?}");
    }

    /// KOB-457, sous Windows : avec une jonction qui sort, ces parcours
    /// passent par la carte, comme `Get-ChildItem -Recurse` seul.
    #[cfg(windows)]
    #[test]
    fn parcours_avec_un_parametre_inconnu_et_jonction_qui_sort() {
        let code = tempfile::tempdir().unwrap();
        let dehors = tempfile::tempdir().unwrap();
        let c = code.path();
        let mut x = Contexte::new(vec![c.to_path_buf()]);
        x.cwd = Some(c.to_path_buf());
        let j = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(c.join("jonction"))
            .arg(dehors.path())
            .output()
            .unwrap();
        assert!(j.status.success(), "mklink /J");
        for l in [
            "Get-ChildItem -Recurse -ErrorAction SilentlyContinue",
            "Get-ChildItem -Recurse -ea 0",
            "Get-ChildItem -Recurse -Exclude target,node_modules",
        ] {
            assert_ne!(
                analyser(l, Shell::PowerShell, &x).classe,
                Classe::Lecture,
                "{l}"
            );
        }
    }

    /// KOB-458 : sous PowerShell, un paramètre qui prend une valeur ne fait
    /// pas prendre cette valeur pour le dossier visé par `cd` ou `pushd`.
    #[test]
    fn cd_avec_un_parametre_sous_powershell() {
        let racine = tempfile::tempdir().unwrap();
        let code = racine.path().join("code");
        let dehors = racine.path().join("dehors");
        std::fs::create_dir_all(code.join("src")).unwrap();
        std::fs::create_dir_all(&dehors).unwrap();
        std::fs::write(dehors.join("secret.txt"), "x").unwrap();
        std::fs::write(code.join("src/a.rs"), "").unwrap();
        let mut x = Contexte::new(vec![code.clone()]);
        x.cwd = Some(code.clone());
        let d = dehors.display().to_string();
        let ps = Shell::PowerShell;
        for l in [
            format!("cd -ea 0 {d}; gc secret.txt"),
            format!("cd -ErrorAction SilentlyContinue {d}; gc secret.txt"),
            format!("pushd -StackName . {d}; gc secret.txt"),
            "cd -ea 0 ../dehors; gc secret.txt".to_owned(),
        ] {
            assert_ne!(analyser(&l, ps, &x).classe, Classe::Lecture, "{l}");
        }
        for l in ["cd src; gc a.rs", "cd -Path src; gc a.rs"] {
            assert_eq!(analyser(l, ps, &x).classe, Classe::Lecture, "{l}");
        }
    }

    /// KOB-442 : `--files0-from` lit la liste des fichiers dans un fichier ;
    /// ces chemins ne sont pas jugés (sonde : `sort --files0-from=liste`
    /// affichait un fichier hors des dossiers).
    #[test]
    fn liste_de_fichiers_dans_un_fichier() {
        for l in [
            "sort --files0-from=liste",
            "sort --files0-from liste",
            "sort --files0=liste",
            "wc --files0-from=liste",
            "wc --f=liste",
            "du --files0-from=liste",
            "find -files0-from liste",
            // Même famille (relecture sécurité de KOB-442).
            "strings @liste",
            "strings -a @liste",
            "sha256sum -c sommes",
            "md5sum --check sommes",
            "cksum -c sommes",
            "file -f liste",
            "file --files-from liste",
        ] {
            assert_ne!(bash(l).classe, Classe::Lecture, "{l}");
        }
        for l in [
            "sort -t, -k2 Cargo.toml",
            "sort --field-separator=, Cargo.toml",
            "wc -l Cargo.toml",
            "du -sh src",
            "rg --files",
            "strings Cargo.toml",
            "sha256sum Cargo.toml",
            "file Cargo.toml",
        ] {
            assert_eq!(bash(l).classe, Classe::Lecture, "{l}");
        }
    }

    /// KOB-444 : une valeur collée qui ne ressemble pas à un chemin mais
    /// nomme un lien du dossier qui sort (`diff --from-file=lien x` affichait
    /// le fichier) est jugée comme en argument séparé.
    #[cfg(unix)]
    #[test]
    fn valeur_collee_qui_nomme_un_lien() {
        let code = tempfile::tempdir().unwrap();
        let dehors = tempfile::tempdir().unwrap();
        let c = code.path();
        std::fs::write(dehors.path().join("secret"), "s").unwrap();
        std::os::unix::fs::symlink(dehors.path().join("secret"), c.join("lien")).unwrap();
        std::fs::write(c.join("x"), "").unwrap();
        std::fs::write(c.join("motifs"), "").unwrap();
        std::fs::create_dir_all(c.join("sub")).unwrap();
        // Un nom qui n'existe que sous `sub` : jugé depuis `-C sub`.
        std::os::unix::fs::symlink(dehors.path(), c.join("sub").join("depot")).unwrap();
        let mut x = Contexte::new(vec![c.to_path_buf()]);
        x.cwd = Some(c.to_path_buf());
        for (l, sh) in [
            ("diff --from-file=lien x", Shell::Bash),
            ("diff --to-file=lien x", Shell::Bash),
            ("grep --file=lien x", Shell::Bash),
            ("Get-Content -Path:lien", Shell::PowerShell),
            ("Get-Content -LiteralPath:lien", Shell::PowerShell),
            ("gc -lp:lien", Shell::PowerShell),
            // Relecture sécurité : joker développé par `-Path`, `git -C`,
            // fichier de motifs collé à `-f`.
            ("Get-Content -Path:l*", Shell::PowerShell),
            ("Select-String x -Path:lie?", Shell::PowerShell),
            ("git -C sub --git-dir=depot log -p", Shell::Bash),
            ("grep -flien x", Shell::Bash),
            ("grep -iflien x", Shell::Bash),
            ("rg -flien x", Shell::Bash),
            ("grep -Tflien x", Shell::Bash),
            ("grep -jflien x", Shell::Bash),
            ("git --git-dir=lien show HEAD:x", Shell::Bash),
        ] {
            assert_ne!(analyser(l, sh, &x).classe, Classe::Lecture, "{l}");
        }
        for (l, sh) in [
            ("diff --from-file=x x", Shell::Bash),
            ("grep --file=motifs x", Shell::Bash),
            ("grep -r --include=*.rs motif .", Shell::Bash),
            ("git log --format=oneline", Shell::Bash),
            ("Get-Content -Path:x", Shell::PowerShell),
            ("Get-Content -Encoding:utf8 x", Shell::PowerShell),
            ("grep -fmotifs x", Shell::Bash),
            ("grep -A2 motif x", Shell::Bash),
            ("rg -g'*.rs' motif", Shell::Bash),
            ("rg -tpy motif", Shell::Bash),
            ("git -C sub log", Shell::Bash),
        ] {
            assert!(lu(&analyser(l, sh, &x)), "{l}");
        }
    }

    /// KOB-452 : après `--`, tout mot est un opérande, même s'il commence
    /// par un tiret (`cat -- -lien/secret.txt` lisait hors des dossiers) ;
    /// pour grep, le premier reste le motif.
    #[cfg(unix)]
    #[test]
    fn operandes_apres_deux_tirets() {
        let code = tempfile::tempdir().unwrap();
        let dehors = tempfile::tempdir().unwrap();
        let c = code.path();
        std::fs::write(dehors.path().join("secret.txt"), "s").unwrap();
        std::os::unix::fs::symlink(dehors.path(), c.join("-lien")).unwrap();
        std::fs::write(c.join("-fichier"), "").unwrap();
        std::fs::write(c.join("x"), "").unwrap();
        let mut x = Contexte::new(vec![c.to_path_buf()]);
        x.cwd = Some(c.to_path_buf());
        for l in [
            "cat -- -lien/secret.txt",
            "head -n 3 -- -lien/secret.txt",
            "grep -- -motif -lien/secret.txt",
            "rg -- -motif -lien",
            "ls -- -lien",
            // Une option à valeur facultative ne prend pas le mot suivant
            // (relecture sécurité de KOB-452).
            "grep --color -- secret -lien/secret.txt",
            "grep --color secret -- -lien/secret.txt",
            "git diff --no-index -- -lien/secret.txt x",
            "git diff --no-index --abbrev -- -lien/secret.txt x",
        ] {
            assert_ne!(analyser(l, Shell::Bash, &x).classe, Classe::Lecture, "{l}");
        }
        let dehors_abs = dehors.path().join("secret.txt").display().to_string();
        for l in [
            format!("grep --color secret {dehors_abs}"),
            format!("git diff --no-index --abbrev {dehors_abs} x"),
        ] {
            assert_ne!(analyser(&l, Shell::Bash, &x).classe, Classe::Lecture, "{l}");
        }
        // PowerShell retire le premier `--` devant un exécutable : la valeur
        // de l'option reste contrôlée.
        assert_ne!(
            analyser("grep -- --file=C:/Windows/win.ini x", Shell::PowerShell, &x).classe,
            Classe::Lecture
        );
        for l in [
            "cat -- -fichier",
            "cat -- x",
            "grep -- -motif x",
            "grep -e motif -- -fichier",
            "git log -- x",
            "grep --color=auto motif x",
            "git log --abbrev=8 -- x",
            "git log --format=%h x",
            "git branch --contains abc",
        ] {
            assert!(lu(&analyser(l, Shell::Bash, &x)), "{l}");
        }
    }

    /// KOB-459 : `Get-Command` sur un script en livre le contenu
    /// (`ScriptContents`, sonde #411) : son argument est un chemin, jugé ; dans
    /// un `|`, même sur un nom nu ou un joker, il passe par la carte.
    #[test]
    fn get_command_sur_un_script() {
        let x = ctx();
        for l in [
            "gcm C:/Windows/s.ps1",
            r"Get-Command C:\Users\moi\s.ps1",
            "gcm ../../s.ps1",
            "gcm s.ps1 | Select-Object -ExpandProperty ScriptContents",
            "gcm * | Select-Object -ExpandProperty ScriptContents",
            "gcm npm | Select-Object -ExpandProperty ScriptContents",
            "gcm -Name:s.ps1 | Select-Object -ExpandProperty ScriptContents",
            "gcm -CommandType ExternalScript | Format-List *",
            "gcm git | ConvertTo-Json",
            "gci outils | gcm",
            "gcm -CommandType Function -ShowCommandInfo",
            "gcm prompt -ShowC",
        ] {
            assert_ne!(
                analyser(l, Shell::PowerShell, &x).classe,
                Classe::Lecture,
                "{l}"
            );
        }
        for l in [
            "gcm git",
            "Get-Command cargo",
            "gcm python -All",
            "gcm -Name git",
            "gcm s.ps1",
            "gcm ./outils/s.ps1",
        ] {
            assert_eq!(
                analyser(l, Shell::PowerShell, &x).classe,
                Classe::Lecture,
                "{l}"
            );
        }
    }

    /// KOB-460 : `jq --rawfile NOM FICHIER` lit le fichier (sonde : `jq -n
    /// --rawfile a -lien/secret.txt -f prog` l'affichait) ; il est jugé, le
    /// nom ne passe pas pour le filtre.
    #[cfg(unix)]
    #[test]
    fn jq_fichiers_nommes() {
        let code = tempfile::tempdir().unwrap();
        let dehors = tempfile::tempdir().unwrap();
        let c = code.path();
        std::os::unix::fs::symlink(dehors.path(), c.join("-lien")).unwrap();
        std::fs::write(c.join("prog"), "$a").unwrap();
        std::fs::write(c.join("f.json"), "{}").unwrap();
        let mut x = Contexte::new(vec![c.to_path_buf()]);
        x.cwd = Some(c.to_path_buf());
        for l in [
            "jq -n --rawfile a -lien/secret.txt -f prog",
            "jq -n --slurpfile a -lien/s.json --from-file prog",
            "jq -n --argfile a -lien/s.json -f prog",
            "jq --rawfile a ../../x . f.json",
        ] {
            assert_ne!(analyser(l, Shell::Bash, &x).classe, Classe::Lecture, "{l}");
        }
        for l in [
            "jq --rawfile a f.json . f.json",
            "jq --arg x -y . f.json",
            "jq --argjson n 3 . f.json",
            "jq . f.json",
        ] {
            assert_eq!(analyser(l, Shell::Bash, &x).classe, Classe::Lecture, "{l}");
        }
    }

    /// KOB-464 : `sed -l N` prend le mot suivant pour sa valeur ; le vrai
    /// script passait pour un fichier et s'exécutait (`sed -n -l 1p '1e …'`
    /// a créé un fichier). Seules les options sans valeur connues passent.
    #[test]
    fn sed_option_a_valeur() {
        for l in [
            "sed -n -l 1p '1e touch PWNED' in",
            "sed -n -l 1p 'w out' in",
            "sed -n --line-length 1p '1e touch PWNED' in",
            "sed -n --line-length=1 '1e touch PWNED' in",
            "sed -n -b 1p in",
            "sed -n --follow-symlinks 1p in",
            "sed -n -- '1e touch PWNED' in",
            // `\/` échappé : la commande `e` se cache après l'adresse.
            r"sed -n '/^\|x\/,/e touch PWNED #/p' in",
            r"sed -nE '/^|x\/,/e touch PWNED #/p' in",
            r"sed -n '/^\|x\/,/w sortie/p' in",
        ] {
            assert_ne!(bash(l).classe, Classe::Lecture, "{l}");
        }
        for l in [
            "sed -n '1,20p' Cargo.toml",
            "sed -n -E '/a/p' Cargo.toml",
            "sed -n -s 1p a b",
            "sed --quiet 1p Cargo.toml",
            "sed -n -- 1p Cargo.toml",
            "sed -nE '/a/p' Cargo.toml",
        ] {
            assert_eq!(bash(l).classe, Classe::Lecture, "{l}");
        }
    }

    /// KOB-465 : une option globale à valeur cachait la sous-commande
    /// (`npm --prefix ls install` lançait le script `install`) ; elle doit
    /// être le premier mot.
    #[test]
    fn sous_commande_en_premier() {
        for l in [
            "npm --prefix ls install",
            "npm -C ls install",
            "pnpm --dir ls install",
            "yarn --cwd ls install",
            "npm --version install",
            "cargo -C metadata build",
            "cargo --manifest-path tree build",
            "cargo --version build",
        ] {
            assert_ne!(bash(l).classe, Classe::Lecture, "{l}");
        }
    }

    /// KOB-467 : npm, yarn, pnpm, cargo, rustc et rustup exécutent du code
    /// venu du dossier (script yarn homonyme de `ls`, `.yarnrc` yarn-path,
    /// `.npmrc` prefix sous Windows, `rustc-wrapper` lancé par `cargo tree`,
    /// `rust-toolchain` path) : jamais en lecture.
    #[test]
    fn yarn_pnpm_cargo_sur_la_carte() {
        for l in [
            "yarn ls",
            "yarn view",
            "yarn why react",
            "yarn --version",
            "pnpm why x",
            "pnpm ls",
            "cargo tree",
            "cargo metadata --format-version 1",
            "cargo --version",
            "npm ls",
            "npm view react version",
            "npm --version",
            "rustc --version",
            "rustup --version",
            "rustup show",
            "dotnet --version",
        ] {
            assert_ne!(bash(l).classe, Classe::Lecture, "{l}");
        }
        assert_eq!(bash("go version").classe, Classe::Lecture);
    }

    /// KOB-461 : `uniq ENTRÉE SORTIE` écrit dans la sortie ; après `--`,
    /// une sortie à tiret compte aussi (`uniq -- in -sortie` passait pour
    /// une lecture).
    #[test]
    fn uniq_sortie_apres_deux_tirets() {
        for l in [
            "uniq -- in -sortie",
            "uniq in sortie",
            "uniq -c -- in -sortie",
            "uniq -f 1 in sortie",
            "uniq in -sortie",
            "uniq - -x",
            "uniq -c in -x",
            // Sortie au nom numérique (relecture de KOB-475 par kobi-a).
            "uniq f 2",
            "uniq -c f 2",
        ] {
            assert_ne!(bash(l).classe, Classe::Lecture, "{l}");
        }
        for l in [
            "uniq in",
            "uniq -c in",
            "uniq -- -in",
            "uniq -f 1 in",
            "uniq -",
        ] {
            assert_eq!(bash(l).classe, Classe::Lecture, "{l}");
        }
    }

    /// KOB-463 : jq lit hors du dossier par `import`/`include`, lit
    /// l'environnement (`env`, `$ENV`), et charge des fichiers par `-f`,
    /// `--run-tests` ou `-L` (sonde sur main : tout classé lecture).
    #[cfg(unix)]
    #[test]
    fn jq_filtre_qui_lit() {
        let code = tempfile::tempdir().unwrap();
        let dehors = tempfile::tempdir().unwrap();
        let c = code.path();
        std::os::unix::fs::symlink(dehors.path(), c.join("lien")).unwrap();
        std::fs::write(c.join("f.json"), "{}").unwrap();
        let mut x = Contexte::new(vec![c.to_path_buf()]);
        x.cwd = Some(c.to_path_buf());
        for l in [
            r#"jq -n 'import "lien/secret" as $s; $s'"#,
            r#"jq -n 'include "lien/m"; x'"#,
            "jq -n env.ANTHROPIC_API_KEY",
            "jq -n env",
            "jq -n '$ENV.HOME'",
            // Filtre qui commence par un tiret (relecture sécurité).
            "jq -n '-1|env.X'",
            "jq -n '-1, $ENV.X'",
            "jq -nf lien/secret.json",
            "jq -rf lien/secret.json f.json",
            "jq -f prog f.json",
            "jq --from-file prog f.json",
            "jq --run-tests lien/t.txt",
            "jq -L lien -n x",
            "jq -Llien -n x",
            "jq -n '\"m\" | modulemeta'",
            "jq -nLlien x",
        ] {
            assert_ne!(analyser(l, Shell::Bash, &x).classe, Classe::Lecture, "{l}");
        }
        // Un chemin hors des dossiers reste un refus (relecture de #419).
        for l in [
            "jq -f prog ../../secret.json",
            "jq -n env.X ../../secret",
            "jq --from-file p ../../x",
            "jq -L lien . ../../x",
        ] {
            assert_eq!(analyser(l, Shell::Bash, &x).classe.cle(), "refus", "{l}");
        }
        for l in [
            "jq . f.json",
            "jq -r .name f.json",
            "jq '.environment' f.json",
            "jq '.envoi' f.json",
            "jq '.spec.containers[].env' f.json",
            "jq .include f.json",
            "jq -n 1",
            "jq --arg x y . f.json",
        ] {
            assert_eq!(analyser(l, Shell::Bash, &x).classe, Classe::Lecture, "{l}");
        }
    }

    /// KOB-487 : `--from-file` est le `-f` de jq : son fichier est jugé comme
    /// un chemin, et un chemin hors des dossiers est un refus, pas une carte.
    #[test]
    fn jq_from_file_juge_comme_f() {
        let x = ctx();
        for l in [
            "jq -f ../../secret.json",
            "jq --from-file ../../secret.json",
            "jq --from-file=../../secret.json",
            "jq -nf ../../secret.json",
            "jq -rf ../../secret.json f.json",
        ] {
            assert_eq!(analyser(l, Shell::Bash, &x).classe.cle(), "refus", "{l}");
        }
        // `f` ailleurs dans le groupe (relecture) : jq le lit comme `-nf`.
        for l in ["jq -fn ../../secret.json", "jq -nfr ../../secret.json"] {
            assert_eq!(analyser(l, Shell::Bash, &x).classe.cle(), "refus", "{l}");
        }
        // `-e` de jq est le code de sortie : le mot suivant n'est pas un motif
        // sauté, il est jugé (`jq -e ../../s -f` lit `../../s`).
        for l in [
            "jq -e ../../secret.json -f",
            "jq -e ../../secret.json --from-file",
        ] {
            assert_eq!(analyser(l, Shell::Bash, &x).classe.cle(), "refus", "{l}");
        }
    }

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

    /// Lecture, ou lecture git que le garde-fou de KOB-473 met sur la carte
    /// (la ligne a passé tous les autres contrôles).
    fn lu(avis: &Avis) -> bool {
        avis.est_lecture() || avis.raison.starts_with("git peut lancer des programmes")
    }

    /// KOB-474 : entre guillemets doubles, bash retire `\` et saut de ligne.
    /// Le mot devenait une option que le filtrage ne voyait plus : un mot
    /// avec une continuation passe par la carte.
    #[track_caller]
    fn carte_bash(ligne: &str) {
        assert_ne!(bash(ligne).classe, Classe::Lecture, "{ligne:?}");
    }

    #[test]
    fn continuation_entre_guillemets_sort_output() {
        carte_bash("sort \"\\\n--output=m\" f");
    }

    #[test]
    fn continuation_entre_guillemets_compress_program() {
        carte_bash("sort -S 1K \"\\\n--compress-program=./p\" f");
    }

    #[test]
    fn continuation_entre_guillemets_find_exec() {
        carte_bash("find . -maxdepth 0 \"\\\n-exec\" touch m \";\"");
    }

    #[test]
    fn continuation_entre_guillemets_rg_pre() {
        carte_bash("rg \"\\\n--pre=./p\" a f");
    }

    #[test]
    fn continuation_entre_guillemets_git_log() {
        carte_bash("git log \"\\\n--output=m\"");
    }

    #[test]
    fn continuation_au_milieu_du_nom() {
        carte_bash("sort \"--out\\\nput=m\" f");
    }

    #[test]
    fn continuations_multiples() {
        carte_bash("sort \"\\\n\\\n--output=m\" f");
    }

    #[test]
    fn continuation_apres_un_debut_hors_guillemets() {
        carte_bash("sort --\"\\\noutput=m\" f");
    }

    #[test]
    fn continuation_crlf_entre_guillemets() {
        carte_bash("sort \"\\\r\n--output=m\" f");
    }

    #[test]
    fn continuation_cr_seul_entre_guillemets() {
        carte_bash("sort \"\\\r--output=m\" f");
    }

    /// Hors guillemets, la continuation est déjà retirée : l'option se voit.
    #[test]
    fn continuation_hors_guillemets_bash() {
        carte_bash("sort --out\\\nput=m f");
        carte_bash("sort \\\n--output=m f");
        carte_bash("sort \\\r\n--output=m f");
    }

    /// PowerShell : accent grave et saut de ligne, sous toutes leurs formes.
    #[test]
    fn continuation_powershell() {
        for ligne in [
            "rg `\n--pre=./p a f",
            "rg `\r\n--pre=./p a f",
            "rg `\r--pre=./p a f",
            "rg \"`\n--pre=./p\" a f",
            "rg \"`\r\n--pre=./p\" a f",
            "rg \"`\r--pre=./p\" a f",
        ] {
            assert_ne!(ps(ligne).classe, Classe::Lecture, "{ligne:?}");
        }
    }

    /// Une continuation ne commence pas de mot : `#` qui la suit ouvre un
    /// commentaire pour bash, et la ligne d'après s'exécute.
    #[test]
    fn continuation_puis_commentaire() {
        carte_bash("grep a f \\\n#\\\ntouch m");
        carte_bash("grep a f \\\n#\\\nsort --output=m f");
    }

    /// `{o.\⏎.o}` est `{o..o}` pour bash, donc une expansion.
    #[test]
    fn continuation_dans_des_accolades() {
        carte_bash("sort --{o.\\\n.o}utput=m f");
        carte_bash("find . -maxdepth 0 -{e.\\\n.e}xec touch m \\;");
    }

    /// Ce qui ne change pas : le saut de ligne reste un caractère ordinaire
    /// entre guillemets simples et sans barre avant lui, et les autres
    /// échappements ne vont pas sur la carte.
    #[test]
    fn continuation_temoins_en_lecture() {
        for ligne in [
            "grep '\\\na' Cargo.toml",
            "grep \"a\nb\" Cargo.toml",
            "grep \"a\\\"b\" Cargo.toml",
            "grep \"a\\\\b\" Cargo.toml",
            "grep a \\\nCargo.toml",
        ] {
            assert_eq!(bash(ligne).classe, Classe::Lecture, "{ligne:?}");
        }
    }

    #[track_caller]
    fn lecture(avis: Avis, ligne: &str) {
        assert!(
            lu(&avis),
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
            "gh pr view 103",
            "gh pr list --state open",
            "gh issue view 12 --json title",
            "git log -1 2>&1",
            "ls nope 2>/dev/null",
            "echo bonjour",
            "pwd",
            "du -sh crates",
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
            // KOB-298 : commentaire seul en fin de ligne, `#` collé à un mot,
            // `#` entre guillemets (même avec un saut de ligne entre eux).
            "cat Cargo.toml # commentaire",
            "ls crates/a#b",
            "grep -n '# titre' Cargo.toml",
            "grep -n \"#\" Cargo.toml",
            "grep -n '#\nx' Cargo.toml",
            // KOB-300 : virgule ou `..` et `}` entre guillemets simples,
            // après un `{}` hors guillemets.
            "ls {} | grep -n '{x,y}'",
            "ls {}; grep -n 'a,b}' Cargo.toml",
            "ls {} | grep -n 'a..}' Cargo.toml",
        ] {
            lecture(bash(ligne), ligne);
        }
    }

    /// KOB-478 : `hostname -F f` passait par la carte, mais pas `-Ff` ni
    /// `--file=f`. Seules les options d'affichage restent des lectures.
    #[track_caller]
    fn hostname_sur_la_carte(ligne: &str) {
        assert_ne!(bash(ligne).classe, Classe::Lecture, "{ligne}");
        assert_ne!(ps(ligne).classe, Classe::Lecture, "{ligne}");
    }

    #[test]
    fn hostname_fichier_colle() {
        hostname_sur_la_carte("hostname -Ff");
    }

    #[test]
    fn hostname_fichier_groupe() {
        hostname_sur_la_carte("hostname -sFf");
        hostname_sur_la_carte("hostname -fF");
    }

    #[test]
    fn hostname_fichier_long() {
        hostname_sur_la_carte("hostname --file=f");
        hostname_sur_la_carte("hostname --file");
    }

    #[test]
    fn hostname_fichier_abrege() {
        hostname_sur_la_carte("hostname --fil=f");
        hostname_sur_la_carte("hostname --fi=f");
    }

    #[test]
    fn hostname_boot() {
        hostname_sur_la_carte("hostname -b");
        hostname_sur_la_carte("hostname --boot");
        hostname_sur_la_carte("hostname --bo");
    }

    #[test]
    fn hostname_nom_en_argument() {
        hostname_sur_la_carte("hostname x");
        hostname_sur_la_carte("hostname -F f");
        hostname_sur_la_carte("hostname -b x");
        hostname_sur_la_carte("hostname -s x");
        hostname_sur_la_carte("hostname -- x");
    }

    #[test]
    fn hostname_option_inconnue() {
        hostname_sur_la_carte("hostname -1");
        hostname_sur_la_carte("hostname --inconnue");
    }

    #[test]
    fn hostname_affichage_en_lecture() {
        for ligne in [
            "hostname",
            "hostname -a",
            "hostname -A",
            "hostname -d",
            "hostname -f",
            "hostname -i",
            "hostname -I",
            "hostname -s",
            "hostname -y",
            "hostname -V",
            "hostname -h",
            "hostname --alias",
            "hostname --all-fqdns",
            "hostname --domain",
            "hostname --fqdn",
            "hostname --long",
            "hostname --ip-address",
            "hostname --all-ip-addresses",
            "hostname --short",
            "hostname --yp",
            "hostname --nis",
            "hostname --help",
            "hostname --version",
        ] {
            assert_eq!(bash(ligne).classe, Classe::Lecture, "{ligne}");
        }
    }

    /// KOB-476 : le parseur de ripgrep accepte trois tirets ou plus : `rg
    /// ---pre=./p` lance le programme, et le filtre ne voyait que `--pre`.
    #[test]
    fn trois_tirets_rg_pre_colle() {
        assert_ne!(bash("rg ---pre=./p a f").classe, Classe::Lecture);
        assert_ne!(ps("rg ---pre=./p a f").classe, Classe::Lecture);
    }

    #[test]
    fn trois_tirets_rg_pre_separe() {
        assert_ne!(bash("rg ---pre ./p a f").classe, Classe::Lecture);
        assert_ne!(ps("rg ---pre ./p a f").classe, Classe::Lecture);
    }

    #[test]
    fn quatre_tirets_et_plus() {
        for ligne in ["rg ----pre=./p a f", "rg -------pre=./p a f"] {
            assert_ne!(bash(ligne).classe, Classe::Lecture, "{ligne}");
            assert_ne!(ps(ligne).classe, Classe::Lecture, "{ligne}");
        }
    }

    #[test]
    fn trois_tirets_cites() {
        for ligne in ["rg '---pre=./p' a f", "rg \"---pre=./p\" a f"] {
            assert_ne!(bash(ligne).classe, Classe::Lecture, "{ligne}");
            assert_ne!(ps(ligne).classe, Classe::Lecture, "{ligne}");
        }
        assert_ne!(bash("rg \\---pre=./p a f").classe, Classe::Lecture);
    }

    /// Quelle que soit la commande : on ne connaît pas le parseur de chacune.
    #[test]
    fn trois_tirets_autres_commandes() {
        for ligne in [
            "sort ---output=m f",
            "grep ---file=x a f",
            "cat ---x",
            "ls ---",
        ] {
            assert_ne!(bash(ligne).classe, Classe::Lecture, "{ligne}");
            assert_ne!(ps(ligne).classe, Classe::Lecture, "{ligne}");
        }
    }

    /// `echo` et `printf` affichent du texte : `echo "---"` sert de séparateur.
    #[test]
    fn trois_tirets_affichage_en_lecture() {
        for ligne in [
            "echo ---",
            "echo \"---\"",
            "echo \"--- fin ---\"",
            "echo a ---",
            "ls; echo ---; pwd",
        ] {
            assert_eq!(bash(ligne).classe, Classe::Lecture, "{ligne}");
        }
        assert_eq!(bash("printf '%s\\n' ---").classe, Classe::Lecture);
    }

    /// Un ou deux tirets ne changent pas.
    #[test]
    fn deux_tirets_temoins_en_lecture() {
        for ligne in [
            "rg -n a",
            "rg --files",
            "grep -n a Cargo.toml",
            "grep -- a Cargo.toml",
            "ls -la",
        ] {
            assert_eq!(bash(ligne).classe, Classe::Lecture, "{ligne}");
        }
        assert_ne!(bash("rg --pre=./p a f").classe, Classe::Lecture);
    }

    /// KOB-489 : un joker non cité en tête de mot peut donner une option.
    #[test]
    fn joker_en_tete_de_mot_sur_la_carte() {
        for ligne in [
            "rg a *",
            "sort *",
            "rg a ?-pre=x",
            "rg a [-]-pre=x",
            "cat *.md",
            "ls *",
            "rg a ''*",
            "rg a \"\"*",
            "rg a -- *",
            "touch -- --pre=x; rg a *",
        ] {
            assert_ne!(bash(ligne).classe, Classe::Lecture, "{ligne}");
        }
    }

    /// Un mot à tiret avec un joker avant le premier `=` peut changer d'option.
    #[test]
    fn joker_dans_un_nom_d_option_sur_la_carte() {
        for ligne in [
            "rg a -*",
            "rg a --*",
            "rg --pr[e]=x a f",
            "rg --pr?=x a f",
            "rg --p*=x a f",
            "rg a \\-*",
            "rg a '-'*",
            "rg a \"--\"*",
            "rg a -n*",
        ] {
            assert_ne!(bash(ligne).classe, Classe::Lecture, "{ligne}");
        }
    }

    /// Le mot étendu commence par un texte fixe, ou le joker est cité.
    #[test]
    fn joker_apres_un_debut_fixe_en_lecture() {
        for ligne in [
            "ls src/*",
            "ls src/*.rs",
            "cat ./*",
            "ls ./*.md",
            "wc -l src/*.rs",
            "grep a f*",
            "rg a --glob=*.rs",
            "grep -r a --include=*.rs",
            "rg 'a*'",
            "rg \"a?\"",
            "find . -name '*.rs'",
            "find . -name \\*.rs",
            "rg '*'",
        ] {
            assert_eq!(bash(ligne).classe, Classe::Lecture, "{ligne}");
        }
    }

    /// KOB-480 : sous PowerShell, `$null` non cité après une redirection de
    /// sortie jette le flux (vu avec le vrai PowerShell dans
    /// `tests/redirection_null_powershell.rs`).
    #[test]
    fn dollar_null_powershell_en_lecture() {
        for ligne in [
            "Get-Content Cargo.toml 2>$null",
            "Get-Content Cargo.toml 2> $null",
            "Get-Content Cargo.toml > $null",
            "Get-Content Cargo.toml >$null",
            "Get-Content Cargo.toml >> $null",
            "Get-Content Cargo.toml 1> $null",
            "Get-Content Cargo.toml *> $null",
            "Get-Content Cargo.toml 3> $null",
            "Get-Content Cargo.toml > $NULL",
            "Get-Content Cargo.toml > $null; Get-ChildItem",
            "Get-Content Cargo.toml 2>$null | Select-Object -First 1",
            "Get-ChildItem > $null",
        ] {
            assert_eq!(ps(ligne).classe, Classe::Lecture, "{ligne}");
        }
    }

    /// Tout autre `$` reste sur la carte : c'est une valeur inconnue.
    #[test]
    fn dollar_null_autres_formes_sur_la_carte() {
        for ligne in [
            "Get-Content a > '$null'",
            "Get-Content a > \"$null\"",
            "Get-Content a > $nullx",
            "Get-Content a > $null.txt",
            "Get-Content a > $null:x",
            "Get-Content a > $null[0]",
            "Get-Content a > ${null}",
            "Get-Content a > $env:X",
            "Get-Content a > $x",
            "Get-Content a > $null$x",
            "Get-Content a > $null`x",
            "Get-Content a > $null'x'",
            "Get-Content a > x$null",
            "Get-Content a < $null",
            "Get-Content a 2>&1 $null",
            "Get-Content a $null",
            "Get-Content $null",
            "$null",
            "$null = Get-Content a",
            "Get-Content a > null",
        ] {
            assert_ne!(ps(ligne).classe, Classe::Lecture, "{ligne}");
        }
    }

    /// Sous le vrai PowerShell, `7>` n'est pas une redirection mais un mot
    /// entier, et ce qui suit reste un argument (prouvé par la sonde #446 et
    /// `tests/redirection_null_powershell.rs`) : sur la carte.
    #[test]
    fn numero_hors_des_flux_powershell_sur_la_carte() {
        for ligne in [
            "x 7> $null",
            "x 7> f",
            "x 7>$null",
            "x 0> $null",
            "x 01> $null",
            "x 9> $null",
            "x 7>> f",
            "x 7>&1",
            "x 99999999999> $null",
        ] {
            assert!(decouper(ligne, Shell::PowerShell).is_err(), "{ligne}");
        }
        assert_eq!(
            decouper("x 6> $null", Shell::PowerShell),
            Ok(vec![
                Jeton::Mot("x".into()),
                Jeton::Redir("6>".into()),
                Jeton::Mot("/dev/null".into()),
            ])
        );
        // Bash garde tous ses descripteurs.
        assert_eq!(
            decouper("x 7> f", Shell::Bash),
            Ok(vec![
                Jeton::Mot("x".into()),
                Jeton::Redir("7>".into()),
                Jeton::Mot("f".into()),
            ])
        );
        assert_ne!(ps("xxd a 7> $null").classe, Classe::Lecture);
        assert_ne!(ps("Get-Content a 7> $null").classe, Classe::Lecture);
    }

    /// Sous bash, `$null` est une variable (vide : `> $null` échoue, et
    /// `> $null/x` viserait la racine) : refusée comme toute autre.
    #[test]
    fn dollar_null_bash_sur_la_carte() {
        for ligne in ["ls 2>$null", "ls > $null", "ls >> $null"] {
            assert_ne!(bash(ligne).classe, Classe::Lecture, "{ligne}");
        }
    }

    /// KOB-481 : `git --version`, seul, ne lit pas le dépôt, comme `git version`.
    #[test]
    fn git_version_en_option_seule_en_lecture() {
        for ligne in ["git --version", "git version"] {
            assert_eq!(bash(ligne).classe, Classe::Lecture, "{ligne}");
            assert_eq!(ps(ligne).classe, Classe::Lecture, "{ligne}");
        }
    }

    /// Rien d'autre ne l'accompagne : ces formes restent sur la carte.
    #[test]
    fn git_version_en_option_accompagnee_sur_la_carte() {
        for ligne in [
            "git --version status",
            "git --version --build-options",
            "git --version -c core.pager=x",
            "git -C src --version",
            "git --no-pager --version",
            "git --vers",
            "git --versionx",
            "git -c core.pager=x --version",
            "git --version x",
        ] {
            assert_ne!(bash(ligne).classe, Classe::Lecture, "{ligne}");
            assert_ne!(ps(ligne).classe, Classe::Lecture, "{ligne}");
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
            // KOB-298 : un commentaire suivi d'un saut de ligne.
            "ls # liste\nrm -rf crates",
            "cat Cargo.toml #\nfind . -delete",
            "ls # x\r\nrm -rf crates",
            "ls # x\rrm -rf crates",
            "ls #\n",
            "cat 'a#b' # c\nrm x",
            // `\r` seul sépare deux commandes.
            "ls\rrm -rf crates",
            "ls\r\nrm -rf crates",
        ] {
            pas_lecture(bash(ligne), ligne);
        }
        for ligne in [
            "Get-ChildItem # x\nRemove-Item -Recurse crates",
            "Get-ChildItem # x\r\nRemove-Item -Recurse crates",
            "Get-ChildItem # x\rRemove-Item -Recurse crates",
            "Get-ChildItem <# x\n#> ; Remove-Item crates",
            "Get-ChildItem <# x #>",
            "Get-ChildItem\rRemove-Item crates",
        ] {
            pas_lecture(ps(ligne), ligne);
        }
        // KOB-300 : l'exception des guillemets simples ne s'étend pas aux
        // guillemets doubles ni aux échappements.
        for ligne in [
            "find . {\"it's\",-delete}",
            "find . {\"a'b\",-delete}",
            "find . {'a',-delete}",
            "find . {'x'..'y'}",
            "find . {\\'x,-delete}",
            "find . {\"'\",-delete} '",
            "git log HEAD@{1}..HEAD@{0}",
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
            // KOB-298.
            "Get-ChildItem # rien",
            "Select-String -Pattern '#' -Path Cargo.toml",
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

        /// Une lecture git passe par la carte (garde-fou de KOB-473).
        #[track_caller]
        fn lecture(v: Verdict, ligne: &str) {
            if ligne.starts_with("git ") {
                assert!(matches!(v, Verdict::Ecriture { .. }), "{ligne} : {v:?}");
            } else {
                assert_eq!(v, Verdict::Lecture, "{ligne}");
            }
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
                // KOB-467 : cargo exécute du code du dossier (`rustc-wrapper`).
                "cargo metadata --format-version 1",
                "cargo tree -p kobi-app",
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
