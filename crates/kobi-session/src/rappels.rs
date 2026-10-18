//! Rappels et tâches à faire dits à Kobi (KOB-145), compris sur place, sans
//! Claude : « rappelle-moi de … dans 20 min / à 15 h / demain matin »,
//! « ajoute … à ma liste », « qu'est-ce que j'ai à faire ? ».
//!
//! [`comprendre`] lit la phrase ; le rangement se fait ensuite dans
//! `rappels.json` par [`crate::stockage::Stockage::ajouter_rappel`]. Une
//! tâche est un rappel sans échéance.

use jiff::civil::Date;
use jiff::{Span, Zoned};

use crate::stockage::Rappel;

/// Ce que l'utilisateur demande.
#[derive(Debug, Clone, PartialEq)]
pub enum Demande {
    /// Rappel à annoncer à l'heure dite.
    Rappel { texte: String, echeance: Zoned },
    /// Tâche à faire, sans heure.
    Tache { texte: String },
    /// « Qu'est-ce que j'ai à faire ? »
    Liste,
}

impl Demande {
    /// Échéance en ms depuis l'époque Unix, pour [`Rappel::echeance_ms`].
    pub fn echeance_ms(&self) -> Option<u64> {
        match self {
            Demande::Rappel { echeance, .. } => {
                u64::try_from(echeance.timestamp().as_millisecond()).ok()
            }
            _ => None,
        }
    }
}

/// Mot de la phrase : sa forme normalisée (minuscules, sans accents) et sa
/// place dans le texte d'origine, pour en recopier un morceau tel quel.
#[derive(Debug, Clone)]
struct Mot {
    norm: String,
    debut: usize,
    fin: usize,
}

fn sans_accent(c: char) -> char {
    match c {
        'à' | 'â' | 'ä' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'î' | 'ï' => 'i',
        'ô' | 'ö' => 'o',
        'ù' | 'û' | 'ü' => 'u',
        'ç' => 'c',
        c => c,
    }
}

/// Découpe en mots : lettres et chiffres, le reste sépare (« l'eau » donne
/// « l » et « eau », « 15h30 » donne « 15 », « h », « 30 »).
fn mots(texte: &str) -> Vec<Mot> {
    let mut out: Vec<Mot> = Vec::new();
    let mut courant: Option<Mot> = None;
    let mut chiffre_avant = false;
    for (i, c) in texte.char_indices() {
        let c2 = c.to_lowercase().next().map(sans_accent).unwrap_or(c);
        if c2.is_alphanumeric() {
            let chiffre = c2.is_ascii_digit();
            // Frontière lettre/chiffre : « 15h30 », « 20min ».
            if let Some(m) = &courant {
                if chiffre != chiffre_avant {
                    out.push(m.clone());
                    courant = None;
                }
            }
            let m = courant.get_or_insert(Mot {
                norm: String::new(),
                debut: i,
                fin: i,
            });
            m.norm.push(c2);
            m.fin = i + c.len_utf8();
            chiffre_avant = chiffre;
        } else if let Some(m) = courant.take() {
            out.push(m);
        }
    }
    out.extend(courant);
    out
}

fn est(mots: &[Mot], i: usize, attendus: &[&str]) -> bool {
    mots.get(i)
        .is_some_and(|m| attendus.contains(&m.norm.as_str()))
}

/// La suite de mots `motif` (séparés par des espaces) commence en `i`.
fn suite(mots: &[Mot], i: usize, motif: &str) -> bool {
    motif
        .split(' ')
        .enumerate()
        .all(|(k, w)| mots.get(i + k).is_some_and(|m| m.norm == w))
}

fn cherche(mots: &[Mot], motifs: &[&str]) -> Option<(usize, usize)> {
    (0..mots.len()).find_map(|i| {
        motifs
            .iter()
            .find(|m| suite(mots, i, m))
            .map(|m| (i, i + m.split(' ').count()))
    })
}

fn nombre(mot: &str) -> Option<i64> {
    if let Ok(n) = mot.parse() {
        return Some(n);
    }
    Some(match mot {
        "un" | "une" => 1,
        "deux" => 2,
        "trois" => 3,
        "quatre" => 4,
        "cinq" => 5,
        "six" => 6,
        "sept" => 7,
        "huit" => 8,
        "neuf" => 9,
        "dix" => 10,
        "onze" => 11,
        "douze" => 12,
        "quinze" => 15,
        "vingt" => 20,
        "trente" => 30,
        "quarante" => 40,
        "cinquante" => 50,
        "soixante" => 60,
        _ => return None,
    })
}

const MINUTES: &[&str] = &["min", "mn", "minute", "minutes"];
const HEURES: &[&str] = &["h", "heure", "heures"];
const SECONDES: &[&str] = &["s", "sec", "seconde", "secondes"];

/// Échéance trouvée : mots `debut..fin` et instant.
struct Moment {
    debut: usize,
    fin: usize,
    quand: Zoned,
}

/// Le jour `date` à `h:m`, dans le fuseau de `maintenant`.
fn a(maintenant: &Zoned, date: Date, h: i64, m: i64) -> Option<Zoned> {
    if !(0..24).contains(&h) || !(0..60).contains(&m) {
        return None;
    }
    date.at(h as i8, m as i8, 0, 0)
        .to_zoned(maintenant.time_zone().clone())
        .ok()
}

/// « dans 20 min », « dans une heure », « dans une demi-heure », « dans 1 h 30 ».
fn dans(mots: &[Mot], i: usize, maintenant: &Zoned) -> Option<Moment> {
    if !est(mots, i, &["dans"]) {
        return None;
    }
    let mut j = i + 1;
    let span = if suite(mots, j, "une demi heure") {
        j += 3;
        Span::new().minutes(30)
    } else if suite(mots, j, "un quart d heure") {
        j += 4;
        Span::new().minutes(15)
    } else {
        let n = nombre(&mots.get(j)?.norm)?;
        j += 1;
        if est(mots, j, MINUTES) {
            j += 1;
            Span::new().minutes(n)
        } else if est(mots, j, &["jour", "jours"]) {
            j += 1;
            Span::new().days(n)
        } else if est(mots, j, SECONDES) {
            j += 1;
            Span::new().seconds(n)
        } else if est(mots, j, HEURES) {
            j += 1;
            let mut s = Span::new().hours(n);
            // « dans 1 h 30 », « dans une heure et demie ».
            if let Some(m) = mots.get(j).and_then(|m| m.norm.parse::<i64>().ok()) {
                j += 1;
                if est(mots, j, MINUTES) {
                    j += 1;
                }
                s = s.minutes(m);
            } else if suite(mots, j, "et demie") {
                j += 2;
                s = s.minutes(30);
            }
            s
        } else {
            return None;
        }
    };
    Some(Moment {
        debut: i,
        fin: j,
        quand: maintenant.checked_add(span).ok()?,
    })
}

/// « 15 h », « 15 h 30 », « 9 heures », « midi », « minuit » à partir de `i`.
/// Renvoie (heure, minute, mots lus).
fn heure(mots: &[Mot], i: usize) -> Option<(i64, i64, usize)> {
    if est(mots, i, &["midi"]) {
        return Some((12, 0, 1));
    }
    if est(mots, i, &["minuit"]) {
        return Some((0, 0, 1));
    }
    let h = mots.get(i)?.norm.parse::<i64>().ok()?;
    if !est(mots, i + 1, HEURES) {
        // « à 15:30 » : deux nombres de suite.
        let m = mots.get(i + 1)?.norm.parse::<i64>().ok()?;
        return (mots[i + 1].norm.len() == 2).then_some((h, m, 2));
    }
    match mots.get(i + 2).and_then(|m| m.norm.parse::<i64>().ok()) {
        Some(m) => Some((h, m, 3)),
        None if suite(mots, i + 2, "et demie") => Some((h, 30, 4)),
        None if suite(mots, i + 2, "et quart") => Some((h, 15, 4)),
        None => Some((h, 0, 2)),
    }
}

/// Moment de la journée : (heure, mots lus).
fn partie_du_jour(mots: &[Mot], i: usize) -> Option<(i64, usize)> {
    if est(mots, i, &["matin"]) {
        Some((9, 1))
    } else if est(mots, i, &["midi"]) {
        Some((12, 1))
    } else if suite(mots, i, "apres midi") {
        Some((14, 2))
    } else if est(mots, i, &["soir"]) {
        Some((19, 1))
    } else {
        None
    }
}

/// « à 15 h » (aujourd'hui, ou demain si l'heure est passée), « demain »,
/// « demain matin », « demain à 10 h », « ce soir », « cet après-midi ».
fn jour_et_heure(mots: &[Mot], i: usize, maintenant: &Zoned) -> Option<Moment> {
    let aujourdhui = maintenant.date();
    let fini = |fin: usize, date: Date, h: i64, m: i64| {
        a(maintenant, date, h, m).map(|quand| Moment {
            debut: i,
            fin,
            quand,
        })
    };
    if est(mots, i, &["demain"]) {
        let demain = aujourdhui.tomorrow().ok()?;
        let j = i + 1;
        if est(mots, j, &["a", "vers"]) {
            if let Some((h, m, n)) = heure(mots, j + 1) {
                return fini(j + 1 + n, demain, h, m);
            }
        }
        if let Some((h, n)) = partie_du_jour(mots, j) {
            return fini(j + n, demain, h, 0);
        }
        return fini(j, demain, 9, 0);
    }
    if est(mots, i, &["ce", "cet", "cette"]) {
        let (h, n) = partie_du_jour(mots, i + 1)?;
        return fini(i + 1 + n, aujourdhui, h, 0);
    }
    if est(mots, i, &["a", "vers"]) {
        let (h, m, n) = heure(mots, i + 1)?;
        let mut quand = a(maintenant, aujourdhui, h, m)?;
        if quand <= *maintenant {
            quand = a(maintenant, aujourdhui.tomorrow().ok()?, h, m)?;
        }
        return Some(Moment {
            debut: i,
            fin: i + 1 + n,
            quand,
        });
    }
    None
}

fn moment(mots: &[Mot], debut: usize, maintenant: &Zoned) -> Option<Moment> {
    (debut..mots.len())
        .find_map(|i| dans(mots, i, maintenant).or_else(|| jour_et_heure(mots, i, maintenant)))
}

/// Recopie les mots `debut..fin` du texte d'origine, sans les mots-outils
/// qui les encadrent (« de », « d' », « que », « à ma liste »…).
fn texte(phrase: &str, mots: &[Mot], mut debut: usize, mut fin: usize) -> Option<String> {
    while debut < fin
        && est(
            mots,
            debut,
            &["de", "d", "que", "qu", "a", "pour", "il", "faut"],
        )
    {
        debut += 1;
    }
    while fin > debut && est(mots, fin - 1, &["de", "d", "a", "pour", "et"]) {
        fin -= 1;
    }
    (debut < fin).then(|| {
        phrase[mots[debut].debut..mots[fin - 1].fin]
            .trim()
            .to_owned()
    })
}

const RAPPELLE: &[&str] = &[
    "rappelle moi",
    "rappelle nous",
    "rappelles moi",
    "tu peux me rappeler",
    "peux tu me rappeler",
    "fais moi penser a",
    "fais moi penser",
    "remind me to",
    "remind me",
];
const AJOUTE: &[&str] = &["ajoute", "rajoute", "note", "mets", "add"];
const LISTE: &[&str] = &[
    "a ma liste de taches",
    "a ma liste",
    "a la liste",
    "dans ma liste",
    "a mes taches",
    "aux taches",
    "to my list",
    "to my todo list",
];
const QUOI_FAIRE: &[&str] = &[
    "qu est ce que j ai a faire",
    "qu est ce que je dois faire",
    "j ai quoi a faire",
    "quoi faire aujourd hui",
    "ma liste de taches",
    "mes taches",
    "mes rappels",
    "lis ma liste",
    "montre ma liste",
    "what do i have to do",
    "my todo list",
];

/// Demande de rappel ou de tâche dans la phrase, s'il y en a une.
pub fn comprendre(phrase: &str, maintenant: &Zoned) -> Option<Demande> {
    let mots = mots(phrase);
    // Le nom de Kobi en tête (« Kobi, rappelle-moi… ») ne compte pas.
    let depart = usize::from(est(&mots, 0, &["kobi"]));
    let mots = &mots[depart..];
    if mots.len() > 30 {
        return None;
    }
    if let Some((_, fin)) = cherche(mots, RAPPELLE) {
        return Some(match moment(mots, fin, maintenant) {
            Some(m) => {
                let avant = texte(phrase, mots, fin, m.debut);
                let apres = texte(phrase, mots, m.fin, mots.len());
                let texte = match (avant, apres) {
                    (Some(a), Some(b)) => format!("{a} {b}"),
                    (a, b) => a.or(b)?,
                };
                Demande::Rappel {
                    texte,
                    echeance: m.quand,
                }
            }
            // « Rappelle-moi d'appeler Léa » sans heure : dans la liste.
            None => Demande::Tache {
                texte: texte(phrase, mots, fin, mots.len())?,
            },
        });
    }
    if est(mots, 0, AJOUTE) {
        if let Some((debut_liste, _)) = cherche(mots, LISTE) {
            return Some(Demande::Tache {
                texte: texte(phrase, mots, 1, debut_liste)?,
            });
        }
    }
    if mots.len() <= 10 && cherche(mots, QUOI_FAIRE).is_some() {
        return Some(Demande::Liste);
    }
    None
}

fn hm(z: &Zoned) -> String {
    match z.minute() {
        0 => format!("{} h", z.hour()),
        m => format!("{} h {m:02}", z.hour()),
    }
}

/// Quand, dit simplement : « dans 2 minutes », « à 15 h 30 », « demain à 9 h ».
pub fn quand(echeance: &Zoned, maintenant: &Zoned) -> String {
    let secondes = echeance.timestamp().as_second() - maintenant.timestamp().as_second();
    if (0..3600).contains(&secondes) {
        let minutes = (secondes + 30) / 60;
        return match minutes {
            0 => "dans un instant".into(),
            1 => "dans 1 minute".into(),
            n => format!("dans {n} minutes"),
        };
    }
    let jour = maintenant.date();
    if echeance.date() == jour {
        format!("à {}", hm(echeance))
    } else if jour.tomorrow().is_ok_and(|d| d == echeance.date()) {
        format!("demain à {}", hm(echeance))
    } else {
        format!(
            "le {:02}/{:02} à {}",
            echeance.day(),
            echeance.month(),
            hm(echeance)
        )
    }
}

/// Réponse à « qu'est-ce que j'ai à faire ? » : les rappels pas faits.
pub fn resume(rappels: &[Rappel], maintenant: &Zoned) -> String {
    let a_faire: Vec<String> = rappels
        .iter()
        .filter(|r| !r.fait)
        .map(|r| {
            let echeance = r
                .echeance_ms
                .and_then(|ms| jiff::Timestamp::from_millisecond(ms as i64).ok())
                .map(|t| t.to_zoned(maintenant.time_zone().clone()));
            match echeance {
                Some(e) => format!("{} ({})", r.texte, quand(&e, maintenant)),
                None => r.texte.clone(),
            }
        })
        .collect();
    match a_faire.len() {
        0 => "Rien à faire pour l'instant : ta liste est vide.".into(),
        1 => format!("Une seule chose : {}.", a_faire[0]),
        n => format!("{n} choses à faire : {}.", a_faire.join(" ; ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::tz::{Offset, TimeZone};

    /// Samedi 18 octobre 2026, 10 h 00, heure de Paris (UTC+2).
    fn maintenant() -> Zoned {
        jiff::civil::date(2026, 10, 18)
            .at(10, 0, 0, 0)
            .to_zoned(TimeZone::fixed(Offset::constant(2)))
            .unwrap()
    }

    fn rappel(phrase: &str) -> (String, String) {
        match comprendre(phrase, &maintenant()) {
            Some(Demande::Rappel { texte, echeance }) => (texte, quand(&echeance, &maintenant())),
            autre => panic!("{phrase} : {autre:?}"),
        }
    }

    #[test]
    fn rappels_avec_heure() {
        for (phrase, texte, q) in [
            (
                "Kobi, rappelle-moi dans une minute de boire de l'eau",
                "boire de l'eau",
                "dans 1 minute",
            ),
            (
                "rappelle-moi de boire de l'eau dans une minute",
                "boire de l'eau",
                "dans 1 minute",
            ),
            ("Rappelle-moi d'appeler Léa à 15 h", "appeler Léa", "à 15 h"),
            (
                "rappelle moi à 15h30 d'envoyer le devis",
                "envoyer le devis",
                "à 15 h 30",
            ),
            (
                "rappelle-moi dans 20 min de sortir le linge",
                "sortir le linge",
                "dans 20 minutes",
            ),
            (
                "rappelle-moi demain matin de relire la PR",
                "relire la PR",
                "demain à 9 h",
            ),
            (
                "rappelle-moi demain à 10 h de rappeler Sophie",
                "rappeler Sophie",
                "demain à 10 h",
            ),
            (
                "rappelle-moi ce soir de sortir les poubelles",
                "sortir les poubelles",
                "à 19 h",
            ),
            // 8 h est passé : demain.
            ("rappelle-moi à 8 h de courir", "courir", "demain à 8 h"),
            (
                "fais-moi penser à appeler maman dans une demi-heure",
                "appeler maman",
                "dans 30 minutes",
            ),
            ("rappelle-moi dans 2 heures de partir", "partir", "à 12 h"),
            (
                "rappelle-moi dans 3 jours de payer",
                "payer",
                "le 21/10 à 10 h",
            ),
            (
                "rappelle-moi d'appeler Léa demain à 10 h pour le devis",
                "appeler Léa pour le devis",
                "demain à 10 h",
            ),
            (
                "remind me to call Bob dans 5 minutes",
                "call Bob",
                "dans 5 minutes",
            ),
        ] {
            assert_eq!(rappel(phrase), (texte.to_owned(), q.to_owned()), "{phrase}");
        }
    }

    #[test]
    fn taches_et_liste() {
        let m = maintenant();
        assert_eq!(
            comprendre("ajoute acheter du pain à ma liste", &m),
            Some(Demande::Tache {
                texte: "acheter du pain".into()
            })
        );
        assert_eq!(
            comprendre("Kobi, rappelle-moi d'appeler le plombier", &m),
            Some(Demande::Tache {
                texte: "appeler le plombier".into()
            })
        );
        for phrase in [
            "qu'est-ce que j'ai à faire ?",
            "Kobi, qu'est-ce que je dois faire aujourd'hui ?",
            "lis ma liste",
            "mes rappels",
        ] {
            assert_eq!(comprendre(phrase, &m), Some(Demande::Liste), "{phrase}");
        }
    }

    #[test]
    fn pas_une_demande() {
        let m = maintenant();
        for phrase in [
            "quelle heure est-il ?",
            "où est géré le refresh token ?",
            "ajoute un test pour la fonction de parsing",
            "rappelle-moi",
            "enregistre la réunion",
            "",
        ] {
            assert_eq!(comprendre(phrase, &m), None, "{phrase}");
        }
    }

    #[test]
    fn echeance_en_ms() {
        let d = comprendre("rappelle-moi dans une minute de boire", &maintenant()).unwrap();
        assert_eq!(
            d.echeance_ms(),
            Some(maintenant().timestamp().as_millisecond() as u64 + 60_000)
        );
    }

    #[test]
    fn resume_de_la_liste() {
        let m = maintenant();
        let ms = m.timestamp().as_millisecond() as u64;
        let r = |texte: &str, echeance_ms, fait| Rappel {
            id: texte.into(),
            texte: texte.into(),
            echeance_ms,
            cree_ms: 0,
            fait,
            source: None,
        };
        assert_eq!(
            resume(&[], &m),
            "Rien à faire pour l'instant : ta liste est vide."
        );
        assert_eq!(
            resume(&[r("courir", None, true), r("pain", None, false)], &m),
            "Une seule chose : pain."
        );
        assert_eq!(
            resume(
                &[
                    r("appeler Léa", Some(ms + 5 * 3_600_000), false),
                    r("pain", None, false)
                ],
                &m
            ),
            "2 choses à faire : appeler Léa (à 15 h) ; pain."
        );
    }
}
