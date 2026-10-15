//! Normalisation du texte avant le calcul du WER : casse, ponctuation,
//! nombres écrits en lettres (français ou anglais).

use serde::{Deserialize, Serialize};

/// Langue utilisée pour écrire les nombres en lettres.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    Fr,
    En,
}

const FR_HINTS: &[&str] = &[
    "le", "la", "les", "un", "une", "des", "de", "du", "et", "est", "je", "tu", "il", "elle",
    "nous", "vous", "ils", "on", "que", "qui", "pas", "pour", "dans", "sur", "avec", "ce", "ça",
    "c", "j", "l", "d", "qu", "n", "au", "aux", "mais", "ou", "oui", "non", "bonjour", "merci",
];
const EN_HINTS: &[&str] = &[
    "the", "a", "an", "and", "is", "are", "i", "you", "he", "she", "we", "they", "it", "that",
    "this", "of", "to", "in", "for", "with", "not", "do", "does", "what", "can", "yes", "no",
    "hello", "thanks", "s", "t", "m", "re", "ll", "be", "have", "has",
];

impl Lang {
    /// Devine la langue d'un texte d'après ses mots-outils ; français en cas
    /// d'égalité.
    pub fn detect(text: &str) -> Lang {
        let (mut fr, mut en) = (0, 0);
        for token in tokenize(text) {
            if let Token::Word(w) = token {
                fr += FR_HINTS.contains(&w.as_str()) as usize;
                en += EN_HINTS.contains(&w.as_str()) as usize;
            }
        }
        if en > fr {
            Lang::En
        } else {
            Lang::Fr
        }
    }
}

/// Normalise `text` en une suite de mots : minuscules, sans ponctuation,
/// apostrophes et traits d'union comme séparateurs, nombres en lettres dans
/// la langue `lang` (détectée si `None`).
pub fn normalize(text: &str, lang: Option<Lang>) -> Vec<String> {
    let lang = lang.unwrap_or_else(|| Lang::detect(text));
    let mut out = Vec::new();
    for token in tokenize(text) {
        match token {
            Token::Word(w) => out.push(w),
            Token::Number { int, frac } => {
                let words = match frac {
                    None => number_to_words(int, lang),
                    Some(frac) => decimal_to_words(int, &frac, lang),
                };
                out.extend(split_words(&words));
            }
            Token::Percent => out.extend(split_words(match lang {
                Lang::Fr => "pour cent",
                Lang::En => "percent",
            })),
        }
    }
    out
}

fn split_words(s: &str) -> impl Iterator<Item = String> + '_ {
    s.split([' ', '-'])
        .filter(|w| !w.is_empty())
        .map(str::to_string)
}

#[derive(Debug, PartialEq)]
enum Token {
    Word(String),
    Number { int: u64, frac: Option<String> },
    Percent,
}

fn is_group_space(c: char) -> bool {
    matches!(c, '\u{a0}' | '\u{202f}' | '\u{2009}')
}

fn tokenize(text: &str) -> Vec<Token> {
    let chars: Vec<char> = text.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_ascii_digit() {
            let (token, next) = read_number(&chars, i);
            tokens.push(token);
            i = next;
        } else if c.is_alphanumeric() {
            let start = i;
            while i < chars.len() && chars[i].is_alphanumeric() && !chars[i].is_ascii_digit() {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect::<String>().to_lowercase();
            tokens.push(Token::Word(word));
        } else {
            if c == '%' {
                tokens.push(Token::Percent);
            }
            i += 1;
        }
    }
    tokens
}

/// Lit un nombre à partir de `chars[i]` : groupes de milliers séparés par
/// une espace insécable, une virgule (anglais) ou un point (français), et
/// partie décimale après `,` ou `.`.
fn read_number(chars: &[char], mut i: usize) -> (Token, usize) {
    let digits_at = |j: usize| {
        let mut k = j;
        while k < chars.len() && chars[k].is_ascii_digit() {
            k += 1;
        }
        k - j
    };

    let mut int = String::new();
    let n = digits_at(i);
    int.extend(&chars[i..i + n]);
    i += n;

    let mut frac = None;
    loop {
        if i + 1 >= chars.len() {
            break;
        }
        let sep = chars[i];
        let n = digits_at(i + 1);
        if n == 0 {
            break;
        }
        let group_follows = n == 3 && !(i + 4 < chars.len() && chars[i + 4].is_alphanumeric());
        if is_group_space(sep) && n == 3 {
            int.extend(&chars[i + 1..i + 4]);
            i += 4;
        } else if (sep == ',' || sep == '.') && group_follows && frac.is_none() && int.len() <= 3 {
            // « 1,000 » ou « 1.000 » : séparateur de milliers ambigu, mais
            // un groupe de trois chiffres suivi d'un autre groupe ou de la
            // fin désigne des milliers dans les transcriptions.
            let mut k = i;
            let mut groups = String::new();
            while k + 1 < chars.len() && chars[k] == sep && digits_at(k + 1) == 3 {
                groups.extend(&chars[k + 1..k + 4]);
                k += 4;
            }
            if k < chars.len() && chars[k].is_ascii_digit() {
                break;
            }
            int.push_str(&groups);
            i = k;
        } else if (sep == ',' || sep == '.') && frac.is_none() {
            frac = Some(chars[i + 1..i + 1 + n].iter().collect::<String>());
            i += 1 + n;
            break;
        } else {
            break;
        }
    }

    let value = int.parse::<u64>().unwrap_or(u64::MAX);
    (Token::Number { int: value, frac }, i)
}

fn decimal_to_words(int: u64, frac: &str, lang: Lang) -> String {
    let mut s = number_to_words(int, lang);
    match lang {
        Lang::Fr => {
            s.push_str(" virgule");
            let zeros = frac.chars().take_while(|&c| c == '0').count();
            for _ in 0..zeros {
                s.push_str(" zéro");
            }
            if zeros < frac.len() {
                let rest: u64 = frac[zeros..].parse().unwrap_or(0);
                s.push(' ');
                s.push_str(&number_to_words(rest, lang));
            }
        }
        Lang::En => {
            s.push_str(" point");
            for c in frac.chars() {
                s.push(' ');
                s.push_str(EN_UNITS[c.to_digit(10).unwrap_or(0) as usize]);
            }
        }
    }
    s
}

/// Écrit un entier en lettres. Les mots composés gardent leurs traits
/// d'union, retirés ensuite par [`normalize`].
pub fn number_to_words(n: u64, lang: Lang) -> String {
    match lang {
        Lang::Fr => fr_number(n),
        Lang::En => en_number(n),
    }
}

const FR_UNITS: [&str; 17] = [
    "zéro", "un", "deux", "trois", "quatre", "cinq", "six", "sept", "huit", "neuf", "dix", "onze",
    "douze", "treize", "quatorze", "quinze", "seize",
];
const FR_TENS: [&str; 7] = [
    "",
    "dix",
    "vingt",
    "trente",
    "quarante",
    "cinquante",
    "soixante",
];

fn fr_below_100(n: u64) -> String {
    match n {
        0..=16 => FR_UNITS[n as usize].to_string(),
        17..=19 => format!("dix-{}", FR_UNITS[(n - 10) as usize]),
        20..=69 => {
            let (t, u) = (n / 10, n % 10);
            match u {
                0 => FR_TENS[t as usize].to_string(),
                1 => format!("{} et un", FR_TENS[t as usize]),
                _ => format!("{}-{}", FR_TENS[t as usize], FR_UNITS[u as usize]),
            }
        }
        71 => "soixante et onze".to_string(),
        70..=79 => format!("soixante-{}", fr_below_100(n - 60)),
        80 => "quatre-vingts".to_string(),
        _ => format!("quatre-vingt-{}", fr_below_100(n - 80)),
    }
}

fn fr_below_1000(n: u64) -> String {
    let (h, r) = (n / 100, n % 100);
    if h == 0 {
        return fr_below_100(r);
    }
    let hundreds = match (h, r) {
        (1, _) => "cent".to_string(),
        (_, 0) => format!("{} cents", FR_UNITS[h as usize]),
        _ => format!("{} cent", FR_UNITS[h as usize]),
    };
    if r == 0 {
        hundreds
    } else {
        format!("{hundreds} {}", fr_below_100(r))
    }
}

fn fr_number(n: u64) -> String {
    if n == 0 {
        return "zéro".to_string();
    }
    let mut parts = Vec::new();
    let mut rest = n;
    for (scale, singular, plural) in [
        (1_000_000_000_000u64, "billion", "billions"),
        (1_000_000_000, "milliard", "milliards"),
        (1_000_000, "million", "millions"),
    ] {
        let q = rest / scale;
        if q > 0 {
            parts.push(if q == 1 {
                format!("un {singular}")
            } else {
                format!("{} {plural}", fr_number(q))
            });
            rest %= scale;
        }
    }
    let thousands = rest / 1000;
    if thousands == 1 {
        parts.push("mille".to_string());
    } else if thousands > 1 {
        // « mille » est invariable et fait perdre le pluriel de « cents » et
        // « vingts » qui le précèdent.
        let mut t = fr_below_1000(thousands);
        if t.ends_with("cents") || t.ends_with("vingts") {
            t.pop();
        }
        parts.push(format!("{t} mille"));
    }
    if !rest.is_multiple_of(1000) {
        parts.push(fr_below_1000(rest % 1000));
    }
    parts.join(" ")
}

const EN_UNITS: [&str; 20] = [
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
];
const EN_TENS: [&str; 10] = [
    "", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];

fn en_below_1000(n: u64) -> String {
    let (h, r) = (n / 100, n % 100);
    let below_100 = match r {
        0..=19 => EN_UNITS[r as usize].to_string(),
        _ if r % 10 == 0 => EN_TENS[(r / 10) as usize].to_string(),
        _ => format!(
            "{}-{}",
            EN_TENS[(r / 10) as usize],
            EN_UNITS[(r % 10) as usize]
        ),
    };
    match (h, r) {
        (0, _) => below_100,
        (_, 0) => format!("{} hundred", EN_UNITS[h as usize]),
        _ => format!("{} hundred {below_100}", EN_UNITS[h as usize]),
    }
}

fn en_number(n: u64) -> String {
    if n == 0 {
        return "zero".to_string();
    }
    let mut parts = Vec::new();
    let mut rest = n;
    for (scale, name) in [
        (1_000_000_000_000u64, "trillion"),
        (1_000_000_000, "billion"),
        (1_000_000, "million"),
        (1_000, "thousand"),
    ] {
        let q = rest / scale;
        if q > 0 {
            parts.push(format!("{} {name}", en_number(q)));
            rest %= scale;
        }
    }
    if rest > 0 {
        parts.push(en_below_1000(rest));
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn norm(text: &str, lang: Lang) -> String {
        normalize(text, Some(lang)).join(" ")
    }

    #[test]
    fn case_punctuation_apostrophes_and_hyphens() {
        assert_eq!(
            norm("Bonjour, Kobi ! L'équipe est-elle prête ?", Lang::Fr),
            "bonjour kobi l équipe est elle prête"
        );
        assert_eq!(
            norm("Hey Kobi, what's the status?", Lang::En),
            "hey kobi what s the status"
        );
        assert_eq!(norm("C’est ça…", Lang::Fr), "c est ça");
    }

    #[test]
    fn french_numbers() {
        let cases = [
            (0, "zéro"),
            (17, "dix sept"),
            (21, "vingt et un"),
            (71, "soixante et onze"),
            (77, "soixante dix sept"),
            (80, "quatre vingts"),
            (81, "quatre vingt un"),
            (91, "quatre vingt onze"),
            (99, "quatre vingt dix neuf"),
            (100, "cent"),
            (200, "deux cents"),
            (201, "deux cent un"),
            (1000, "mille"),
            (1981, "mille neuf cent quatre vingt un"),
            (80_000, "quatre vingt mille"),
            (200_000, "deux cent mille"),
            (2_000_000, "deux millions"),
            (1_000_001, "un million un"),
        ];
        for (n, expected) in cases {
            assert_eq!(
                split_words(&fr_number(n)).collect::<Vec<_>>().join(" "),
                expected,
                "{n}"
            );
        }
    }

    #[test]
    fn english_numbers() {
        let cases = [
            (0, "zero"),
            (13, "thirteen"),
            (42, "forty two"),
            (100, "one hundred"),
            (305, "three hundred five"),
            (1999, "one thousand nine hundred ninety nine"),
            (2_500_000, "two million five hundred thousand"),
        ];
        for (n, expected) in cases {
            assert_eq!(
                split_words(&en_number(n)).collect::<Vec<_>>().join(" "),
                expected,
                "{n}"
            );
        }
    }

    #[test]
    fn numbers_inside_text() {
        assert_eq!(
            norm("On a 3 tickets et 25 % de retard.", Lang::Fr),
            "on a trois tickets et vingt cinq pour cent de retard"
        );
        assert_eq!(
            norm("Il reste 3,5 jours", Lang::Fr),
            "il reste trois virgule cinq jours"
        );
        assert_eq!(norm("0,05", Lang::Fr), "zéro virgule zéro cinq");
        assert_eq!(
            norm("version 2.75", Lang::En),
            "version two point seven five"
        );
        assert_eq!(norm("1,000 users", Lang::En), "one thousand users");
        assert_eq!(norm("10\u{202f}000 euros", Lang::Fr), "dix mille euros");
        assert_eq!(norm("3, 4 et 5", Lang::Fr), "trois quatre et cinq");
    }

    #[test]
    fn language_detection() {
        assert_eq!(
            Lang::detect("Kobi, est-ce que le ticket est fait ?"),
            Lang::Fr
        );
        assert_eq!(Lang::detect("Kobi, is the ticket done?"), Lang::En);
    }

    #[test]
    fn detected_language_drives_numbers() {
        assert_eq!(
            normalize("We have 21 tickets", None).join(" "),
            "we have twenty one tickets"
        );
        assert_eq!(
            normalize("On a 21 tickets", None).join(" "),
            "on a vingt et un tickets"
        );
    }
}
