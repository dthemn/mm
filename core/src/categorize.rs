//! Local categorization. Later steps override earlier ones:
//! a dictionary of Polish product words, shop defaults, then personal corrections.

use crate::model::*;
use uuid::Uuid;

/// Lowercase without diacritics. OCR confuses Polish letters, so comparison ignores them.
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.to_lowercase().chars() {
        out.push(match c {
            'ą' => 'a', 'ć' => 'c', 'ę' => 'e', 'ł' => 'l', 'ń' => 'n',
            'ó' => 'o', 'ś' => 's', 'ź' | 'ż' => 'z',
            c if c.is_alphanumeric() => c,
            _ => ' ',
        });
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

const WORDS: &[(Uuid, &[&str])] = &[
    (CAT_FOOD, &[
        "chleb", "bulka", "bagietk", "rogal", "pieczyw", "mleko", "mleczn", "maslo", "smietan", "jogurt", "kefir", "ser ",
        "serek", "twarog", "jaja", "jajka", "szynk", "kielbas", "parowk", "boczek", "kurczak", "indyk", "wieprz", "wolow",
        "mieso", "miesn", "filet", "losos", "ryba", "sledz", "tunczyk", "jablk", "banan", "pomarancz", "cytryn", "gruszk",
        "truskawk", "winogron", "pomidor", "ogorek", "ziemniak", "marchew", "cebula", "czosnek", "salata", "kapust", "papryk",
        "warzyw", "owoc", "makaron", "ryz", "kasza", "maka", "cukier", "sol ", "pieprz", "olej", "oliwa", "ocet", "ketchup",
        "musztard", "majonez", "czekolad", "baton", "cukierk", "ciastk", "tiramisu", "lody", "wafl", "chips", "orzech", "kawa",
        "herbat", "sok ", "woda", "napoj", "cola", "piwo", "wino ", "pierogi", "pizza", "zupa", "zurek", "dzem", "miod", "platki",
    ]),
    (CAT_HOUSEHOLD, &[
        "szampon", "mydlo", "zel pod", "zel do", "zel odsw", "odsw", "dezodorant", "pasta do zebow", "szczoteczk", "krem", "balsam", "proszek",
        "plyn do", "kapsulk", "domestos", "ludwik", "papier toalet", "recznik", "chusteczk", "worki", "gabk", "sciereczk",
        "drogeryj", "wybielacz", "plyn", "tekstylia domowe", "bateri", "zarowk",
    ]),
    (CAT_HEALTH, &[
        "apteka", "lek ", "tabletk", "kapsulki", "syrop", "witamin", "bandaz", "plaster", "recept", "sasz", "aer ", "masc",
        "krople", "paracetamol", "ibuprofen",
    ]),
    (CAT_CLOTHING, &["skarpet", "koszul", "spodni", "bluz", "kurtk", "bielizn", "czapk", "buty", "rajstop", "majtk", "sukienk"]),
    (CAT_TRANSPORT, &["paliwo", "benzyn", "diesel", "pb95", "pb98", "lpg", "bilet", "parking", "autostrad", "myjnia"]),
    (CAT_CAFES, &["kawiarni", "restauracj", "lunch", "obiad", "latte", "cappuccino", "kebab", "burger"]),
    (CAT_ENTERTAINMENT, &["kino", "teatr", "koncert", "ksiazk", "zabawk", "gra "]),
];

/// Shops where almost everything belongs to one category.
const SHOPS: &[(Uuid, &[&str])] = &[
    (CAT_HEALTH, &["apteka", "doz ", "gemini", "ziko", "super pharm"]),
    (CAT_TRANSPORT, &["orlen", "shell", "circle k", "bp ", "moya", "amic", "pkp", "koleje", "mpk", "ztm", "uber", "bolt"]),
    (CAT_CAFES, &["mcdonald", "kfc", "starbucks", "costa", "burger king", "pizzeria", "restauracja", "kawiarnia", "bar "]),
    (CAT_HOUSEHOLD, &["rossmann", "hebe", "castorama", "leroy", "obi "]),
    (CAT_CLOTHING, &["reserved", "h m", "zara", "pepco", "ccc ", "deichmann", "sinsay"]),
    (CAT_ENTERTAINMENT, &["multikino", "cinema city", "helios", "empik"]),
];

fn hit(list: &[(Uuid, &[&str])], text: &str) -> Option<Uuid> {
    let padded = format!("{text} ");
    list.iter().find(|(_, words)| words.iter().any(|w| padded.contains(w))).map(|x| x.0)
}

/// The dictionary and shop part, without personal rules.
fn suggest(merchant: &str, item: &str) -> Uuid {
    let (m, i) = (fold(merchant), fold(item));
    let pharmacy = hit(&SHOPS[..1], &m).is_some();
    if pharmacy {
        return CAT_HEALTH;
    }
    // OCR sometimes splits a word ("Ludwi k900g"), so the name is also tried without spaces.
    hit(WORDS, &i).or_else(|| hit(WORDS, &i.replace(' ', ""))).or_else(|| hit(SHOPS, &m)).unwrap_or(CAT_OTHER)
}

/// `rules` are personal corrections. A rule for this shop wins over a rule for any shop.
/// The category of money coming in, from the words of a statement line.
pub fn income_category(text: &str) -> Uuid {
    let t = fold(text);
    let has = |words: &[&str]| words.iter().any(|w| t.contains(w));
    if has(&["wynagrodz", "pensj", "salary", "payroll"]) {
        CAT_SALARY
    } else if has(&["premia", "bonus", "nagroda"]) {
        CAT_BONUS
    } else if has(&["cashback", "zwrot gotowki"]) {
        CAT_CASHBACK
    } else if has(&["zwrot", "refund", "reklamac"]) {
        CAT_REFUND
    } else if has(&["prezent", "gift", "urodzin"]) {
        CAT_GIFT
    } else {
        CAT_INCOME_OTHER
    }
}

pub fn categorize(rules: &[CategoryRule], merchant_id: Option<Uuid>, merchant: &str, item: &str) -> Uuid {
    let pattern = fold(item);
    let find = |m: Option<Uuid>| rules.iter().find(|r| r.merchant_id == m && r.pattern == pattern).map(|r| r.category_id);
    merchant_id.and_then(|m| find(Some(m))).or_else(|| find(None)).unwrap_or_else(|| suggest(merchant, item))
}

const PRETTY: &[&str] = &[
    "Chleb", "Bułka", "Mleko", "Masło", "Śmietana", "Jabłka", "Ziemniaki", "Mąka", "Kiełbasa", "Szynka", "Łosoś", "Żurek",
    "Świeży", "Płyn", "Mydło", "Ręczniki", "Pomarańcze", "Kapusta", "Woda", "Ogórek", "Herbata", "Czekolada", "Łagodzący",
];

/// Restores Polish letters in known words, since recognition often loses them.
/// The original text is always kept as `raw_name`.
pub fn tidy_name(raw: &str) -> String {
    raw.split_whitespace().map(|w| {
        let f = fold(w);
        match PRETTY.iter().find(|p| fold(p) == f) {
            Some(p) if w.chars().all(|c| c.is_uppercase() || !c.is_alphabetic()) => p.to_uppercase(),
            Some(p) if w.chars().next().is_some_and(char::is_lowercase) => p.to_lowercase(),
            Some(p) => (*p).to_string(),
            None => w.to_string(),
        }
    }).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mixed_receipt_gets_a_category_per_line() {
        assert_eq!(categorize(&[], None, "Biedronka", "LodyMarlAlmond120ml"), CAT_FOOD);
        assert_eq!(categorize(&[], None, "Biedronka", "LodyPatMarlKokos90ml"), CAT_FOOD);
        assert_eq!(categorize(&[], None, "Biedronka", "Ludwik900g"), CAT_HOUSEHOLD);
        assert_eq!(categorize(&[], None, "Biedronka", "Ludwi k900g"), CAT_HOUSEHOLD, "a word split by OCR");
    }

    #[test]
    fn income_lines_get_income_categories() {
        assert_eq!(income_category("WYNAGRODZENIE ZA 10/2026"), CAT_SALARY);
        assert_eq!(income_category("Premia kwartalna"), CAT_BONUS);
        assert_eq!(income_category("Zwrot gotówki za zakupy"), CAT_CASHBACK);
        assert_eq!(income_category("Zwrot za zamówienie 123"), CAT_REFUND);
        assert_eq!(income_category("Na urodziny"), CAT_GIFT);
        assert_eq!(income_category("Przelew od Jana"), CAT_INCOME_OTHER);
    }

    #[test]
    fn folding_ignores_case_diacritics_and_punctuation() {
        assert_eq!(fold("ŻUREK  w Butelce!"), "zurek w butelce");
        assert_eq!(fold("Świeże jabłka 1,5kg"), "swieze jablka 1 5kg");
    }

    #[test]
    fn one_receipt_gets_food_and_household() {
        for (name, want) in [
            ("Chleb żytni", CAT_FOOD), ("MLEKO 3,2% UHT", CAT_FOOD), ("Jabika luz", CAT_OTHER), ("Jabłka luz", CAT_FOOD),
            ("Szampon lagodzacy", CAT_HOUSEHOLD), ("Proszek do prania", CAT_HOUSEHOLD), ("Przytulne skarpety", CAT_CLOTHING),
            ("MINI TIRAMISU BALCONI 300G", CAT_FOOD), ("ZEL ODSW POW FELCE", CAT_HOUSEHOLD), ("Coś dziwnego", CAT_OTHER),
        ] {
            assert_eq!(categorize(&[], None, "Market", name), want, "{name}");
        }
    }

    #[test]
    fn a_shop_fills_in_when_the_name_says_nothing() {
        assert_eq!(categorize(&[], None, "Orlen S.A.", "Pozycja 12"), CAT_TRANSPORT);
        assert_eq!(categorize(&[], None, "Rossmann", "Nivea 200"), CAT_HOUSEHOLD);
    }

    #[test]
    fn a_pharmacy_outranks_product_words() {
        assert_eq!(categorize(&[], None, "Apteka Pod Lwem", "Woda utleniona"), CAT_HEALTH);
    }

    #[test]
    fn personal_rules_override_everything_and_shop_rules_win() {
        let m = Uuid::from_u128(5);
        let rules = vec![
            CategoryRule { merchant_id: None, pattern: fold("Mleko"), category_id: CAT_OTHER },
            CategoryRule { merchant_id: Some(m), pattern: fold("Mleko"), category_id: CAT_HEALTH },
        ];
        assert_eq!(categorize(&rules, Some(m), "x", "Mleko świeże"), CAT_FOOD, "a different item name");
        assert_eq!(categorize(&rules, Some(m), "x", "MŁEKO"), CAT_HEALTH, "OCR losing a Polish letter still matches");
        assert_eq!(categorize(&rules, Some(m), "x", "MLEKO"), CAT_HEALTH);
        assert_eq!(categorize(&rules, None, "x", "mleko"), CAT_OTHER);
        assert_eq!(categorize(&rules, Some(Uuid::from_u128(6)), "x", "Mleko"), CAT_OTHER);
    }

    #[test]
    fn tidy_restores_known_words_and_keeps_the_rest() {
        assert_eq!(tidy_name("Jablka luz"), "Jabłka luz");
        assert_eq!(tidy_name("LOSOS wedzony"), "ŁOSOŚ wedzony");
        assert_eq!(tidy_name("plyn do naczyn"), "płyn do naczyn");
    }
}
