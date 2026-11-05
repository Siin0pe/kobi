//! KOB-506 : un mot qui commence par un caractère de plusieurs octets ne fait jamais
//! paniquer le classeur (`cat été/a.txt` coupait le `é` en deux).
use std::path::PathBuf;

use kobi_perms::{commande, Classe, Contexte, Shell};

fn contexte() -> (tempfile::TempDir, Contexte) {
    let d = tempfile::tempdir().unwrap();
    let mut ctx = Contexte::new(vec![PathBuf::from(d.path())]);
    ctx.cwd = Some(PathBuf::from(d.path()));
    (d, ctx)
}

#[test]
fn un_chemin_accentue_est_classe_comme_le_meme_sans_accent() {
    let (_d, ctx) = contexte();
    for shell in [Shell::Bash, Shell::PowerShell] {
        for (accent, simple) in [
            ("cat été/a.txt", "cat ete/a.txt"),
            ("rg x été/a.txt", "rg x ete/a.txt"),
            ("cat ü/a", "cat u/a"),
            ("cat é~x/a", "cat e~x/a"),
            ("cat 日本語/a", "cat abc/a"),
            ("cat 😀/a", "cat a/a"),
        ] {
            let a = commande::analyser(accent, shell, &ctx).classe;
            let s = commande::analyser(simple, shell, &ctx).classe;
            assert_eq!(a, s, "{accent}");
        }
    }
    let lecture = commande::analyser("gc été/a.txt", Shell::PowerShell, &ctx).classe;
    assert_eq!(lecture, Classe::Lecture);
}

/// Toutes les formes d'argument, sur des commandes aux règles différentes.
#[test]
fn aucune_panique_sur_les_mots_non_ascii() {
    let (_d, ctx) = contexte();
    let commandes = [
        "cat",
        "rg",
        "rg -e",
        "grep",
        "grep -e",
        "sed",
        "sed -n",
        "sed -e",
        "awk",
        "git log",
        "git grep",
        "git -c",
        "git diff",
        "find",
        "find . -name",
        "fd",
        "gci",
        "gc",
        "sls",
        "Get-Content",
        "Get-ChildItem -Path",
        "tr",
        "sort",
        "uniq",
        "xxd",
        "jq",
        "jq -f",
        "head",
        "tail",
        "wc",
        "ls",
        "cd",
        "pwd",
        "where.exe",
        "where.exe /R",
        "env",
        "xargs",
        "tee",
        "rm",
        "truncate",
        "echo",
        "curl",
        "cargo test",
        "npm run",
        "sort.exe",
        "./é",
        "é",
        "é.exe",
    ];
    let formes = [
        "@", "@/a", "-@", "--@", "--@=@", "-@=@", "/@", "\\@", "@~x/y", "@^x/y", "@:", "@:x",
        "C:@", ".@", "./@", "../@", "~@", "@*", "*@", "[@]", "[@-@]", "@-", "@?", "$@", "%@", "=@",
        "#@", "{@,@}", "@..@", "\"@\"", "'@'", "@>@", "2>@", "<@", "a,@", "-x:@", "@;@", "@|@",
        "@&&@",
    ];
    for c in ['é', 'ü', '日', '😀', '–', '\u{a0}', '\u{2019}'] {
        let c = c.to_string();
        for shell in [Shell::Bash, Shell::PowerShell] {
            for commande_ in commandes {
                for forme in formes {
                    let ligne = format!("{commande_} {}", forme.replace('@', &c));
                    commande::analyser(&ligne, shell, &ctx);
                }
            }
        }
    }
}
