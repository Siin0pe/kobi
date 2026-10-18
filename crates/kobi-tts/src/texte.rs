//! Ce que Kobi dit d'une réponse écrite : le Markdown de la bulle devient du
//! texte à prononcer, découpé en phrases au fil du flux pour que la voix
//! commence dès la première phrase complète.

/// Texte à prononcer : sans Markdown, sans blocs de code ni adresses.
pub fn pour_la_voix(texte: &str) -> String {
    convertir(texte, false)
}

/// `partielle` : la dernière ligne est encore en cours d'écriture (flux) ;
/// elle n'est pas close par une pause.
fn convertir(texte: &str, partielle: bool) -> String {
    let mut sortie = String::with_capacity(texte.len());
    let mut dans_code = false;
    let lignes: Vec<&str> = texte.split('\n').collect();
    for (k, ligne) in lignes.iter().enumerate() {
        let l = ligne.trim();
        if l.starts_with("```") {
            if !dans_code {
                sortie.push_str(" (code affiché dans la bulle). ");
            }
            dans_code = !dans_code;
            continue;
        }
        if dans_code {
            continue;
        }
        // Titres, citations et puces : seul le texte reste.
        let l = l
            .trim_start_matches('#')
            .trim_start_matches('>')
            .trim_start();
        let l = l
            .strip_prefix("- ")
            .or_else(|| l.strip_prefix("* "))
            .or_else(|| l.strip_prefix("• "))
            .unwrap_or(l);
        let l = numero_de_liste(l);
        if l.is_empty() {
            continue;
        }
        sortie.push_str(&en_ligne(l));
        if partielle && k + 1 == lignes.len() {
            break;
        }
        // Une ligne sans ponctuation finale (puce, titre) reste une pause.
        if !l.ends_with(['.', '!', '?', ':', ';', ',', '…']) {
            sortie.push('.');
        }
        sortie.push(' ');
    }
    espaces(&sortie, !partielle)
}

/// « 1. texte » → « texte ».
fn numero_de_liste(l: &str) -> &str {
    let chiffres = l.chars().take_while(|c| c.is_ascii_digit()).count();
    if chiffres > 0 && chiffres <= 3 {
        if let Some(reste) = l[chiffres..].strip_prefix(". ") {
            return reste;
        }
    }
    l
}

/// Liens, code en ligne, emphase et adresses dans une ligne.
fn en_ligne(l: &str) -> String {
    let mut s = String::with_capacity(l.len());
    let mut chars = l.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            // [texte](adresse) → texte
            '[' => {
                if let Some(fin) = l[i..].find("](") {
                    let fin_lien = l[i + fin..].find(')');
                    if let Some(fin_lien) = fin_lien {
                        s.push_str(&l[i + 1..i + fin]);
                        let saut = i + fin + fin_lien;
                        while chars.peek().is_some_and(|&(j, _)| j <= saut) {
                            chars.next();
                        }
                        continue;
                    }
                }
                s.push(c);
            }
            '*' | '_' | '`' | '~' => {
                // Un `_` au milieu d'un mot (nom de fonction) se dit comme une espace.
                if c == '_' && s.ends_with(|p: char| p.is_alphanumeric()) {
                    s.push(' ');
                }
            }
            _ => s.push(c),
        }
    }
    s.split_whitespace()
        .map(|mot| {
            if mot.starts_with("http://") || mot.starts_with("https://") {
                "un lien"
            } else {
                mot
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Espaces simples ; `fermer` retire l'espace finale (texte complet). Sinon
/// elle reste : la fin de la dernière ligne complète est une fin de phrase.
fn espaces(s: &str, fermer: bool) -> String {
    let fin = s.ends_with(char::is_whitespace);
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s = s.replace(" .", ".").replace("..", ".");
    if fin && !fermer && !s.is_empty() {
        s + " "
    } else {
        s
    }
}

/// Fin de phrase : ponctuation forte suivie d'une espace (ou fin du texte).
fn fins_de_phrase(s: &str) -> Vec<usize> {
    let octets: Vec<(usize, char)> = s.char_indices().collect();
    let mut fins = Vec::new();
    for (k, &(i, c)) in octets.iter().enumerate() {
        if !matches!(c, '.' | '!' | '?' | '…' | ':') {
            continue;
        }
        let suivant = octets.get(k + 1).map(|&(_, n)| n);
        if !suivant.is_some_and(char::is_whitespace) {
            continue;
        }
        // « 3.5 », « M. » et les initiales ne coupent pas.
        let avant = s[..i].split_whitespace().last().unwrap_or("");
        if c == '.' && (avant.len() <= 1 || avant == "M" || avant == "Mme" || avant == "cf") {
            continue;
        }
        fins.push(i + c.len_utf8());
    }
    fins
}

/// Découpe une réponse qui arrive en flux en phrases prêtes à dire.
///
/// On lui donne le texte accumulé de la bulle à chaque morceau ; elle rend
/// les phrases complètes pas encore rendues, puis le reste à la fin.
#[derive(Debug, Default)]
pub struct Diction {
    /// Texte à dire déjà rendu (en octets de `pour_la_voix`).
    rendu: usize,
}

/// Une phrase trop courte attend la suivante (« Oui. » seul sonne haché).
const PHRASE_MIN: usize = 12;

impl Diction {
    pub fn new() -> Self {
        Self::default()
    }

    /// Phrases complètes apparues depuis le dernier appel. Le dernier bloc de
    /// code ouvert n'est pas encore dit.
    pub fn suite(&mut self, accumule: &str) -> Vec<String> {
        // Un bloc de code non fermé est mis de côté jusqu'à sa fin.
        let stable = match accumule.matches("```").count() % 2 {
            0 => accumule,
            _ => &accumule[..accumule.rfind("```").unwrap_or(accumule.len())],
        };
        // La dernière ligne peut être incomplète (puce en cours) : on attend
        // une fin de phrase ou la fin du flux.
        let dit = convertir(stable, true);
        self.phrases(&dit, false)
    }

    /// Fin du flux : tout ce qui reste.
    pub fn fin(&mut self, accumule: &str) -> Vec<String> {
        let dit = pour_la_voix(accumule);
        self.phrases(&dit, true)
    }

    fn phrases(&mut self, dit: &str, tout: bool) -> Vec<String> {
        if self.rendu > dit.len() || !dit.is_char_boundary(self.rendu) {
            // Le texte a été réécrit (ne devrait pas arriver en flux) : on repart de la fin.
            self.rendu = dit.len();
            return Vec::new();
        }
        let reste = &dit[self.rendu..];
        let mut phrases = Vec::new();
        let mut debut = 0;
        for fin in fins_de_phrase(reste) {
            let phrase = reste[debut..fin].trim();
            if phrase.chars().count() < PHRASE_MIN {
                continue;
            }
            phrases.push(phrase.to_string());
            debut = fin;
        }
        if tout {
            let phrase = reste[debut..].trim();
            if !phrase.is_empty() {
                phrases.push(phrase.to_string());
            }
            debut = reste.len();
        }
        self.rendu += debut;
        phrases
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_markdown_ne_se_prononce_pas() {
        let texte = "## Réponse\n\nLe refresh token est géré dans **`auth/token.rs`**, \
                     voir [la doc](https://exemple.fr/doc).\n\n- première piste\n- seconde piste\n\n\
                     ```rust\nfn secret() {}\n```\nVoilà https://exemple.fr !";
        assert_eq!(
            pour_la_voix(texte),
            "Réponse. Le refresh token est géré dans auth/token.rs, voir la doc. \
             première piste. seconde piste. (code affiché dans la bulle). Voilà un lien !"
        );
    }

    #[test]
    fn listes_numerotees_et_noms_de_fonction() {
        assert_eq!(
            pour_la_voix("1. appeler refresh_token\n2. relancer"),
            "appeler refresh token. relancer."
        );
    }

    #[test]
    fn les_phrases_sortent_au_fil_du_flux() {
        let mut d = Diction::new();
        let mut dit = Vec::new();
        let morceaux = [
            "Le refresh",
            " token est géré dans auth.rs. Il expire",
            " au bout de 3.5 heures, puis il est",
            " renouvelé. Oui.",
            " C'est tout.",
        ];
        let mut accumule = String::new();
        let mut par_morceau = Vec::new();
        for m in morceaux {
            accumule.push_str(m);
            let p = d.suite(&accumule);
            par_morceau.push(p.len());
            dit.extend(p);
        }
        dit.extend(d.fin(&accumule));
        assert_eq!(
            dit,
            [
                "Le refresh token est géré dans auth.rs.",
                "Il expire au bout de 3.5 heures, puis il est renouvelé.",
                // Trop courte seule, « Oui. » part avec la phrase suivante.
                "Oui. C'est tout."
            ]
        );
        // La première phrase part dès qu'elle est complète.
        assert_eq!(par_morceau[1], 1);
    }

    #[test]
    fn un_bloc_de_code_ouvert_attend_sa_fin() {
        let mut d = Diction::new();
        let p = d.suite("Voici la fonction demandée :\n```rust\nfn a() {\n");
        assert_eq!(p, ["Voici la fonction demandée :"]);
        let p = d.fin("Voici la fonction demandée :\n```rust\nfn a() {}\n```\nElle renvoie un.");
        assert_eq!(p, ["(code affiché dans la bulle).", "Elle renvoie un."]);
    }

    #[test]
    fn rien_a_dire() {
        let mut d = Diction::new();
        assert!(d.suite("").is_empty());
        assert!(d.fin("   ").is_empty());
    }
}
