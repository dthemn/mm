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

/// Product words, folded. Each entry matches at the start of a word ("mlek" finds "mleko" and "mleczny" no,
/// "mlecz" does); a trailing space asks for the whole word ("ser " is cheese, never "serwetki"). Entries may span
/// words. When several match, the longest wins, so "proszek do piecz" (food) beats "proszek" (household)
/// and "woda toalet" (household) beats "woda" (food).
const WORDS: &[(Uuid, &[&str])] = &[
    (CAT_FOOD, &[
        // Bread and bakery.
        "chleb", "chlebek", "bulk", "bulecz", "kajzer", "bagiet", "rogal", "croissant", "pieczyw", "tortill", "grahamk",
        "drozdzowk", "paczek", "paczki", "babk", "sernik", "szarlotk", "ciast", "kremowk", "muffin", "biszkopt", "wafl",
        "wafel", "precel", "tost ", "tosty", "pita ", "deser",
        // Dairy and eggs.
        "mlek", "mlecz", "jog ", "jogurt", "kefir", "maslank", "maslo", "smiet", "ser ", "sery ", "sera ", "serek", "serki",
        "twarog", "twar ", "gouda", "edam", "mozzarell", "feta", "parmez", "camembert", "brie ", "skyr", "danio", "actimel",
        "monte ", "budyn", "kisiel", "galaret", "jaja", "jajka", "jaj ", "margaryn", "kasia ", "rama ",
        // Meat and fish.
        "szynk", "kielb", "parowk", "parow ", "boczek", "kurcz", "indyk", "wieprz", "wolow", "mieso", "miesn", "mielon",
        "filet", "piers", "udko", "udka", "skrzyd", "schab", "karkow", "poledwic", "golonk", "watrob", "salami", "kabanos",
        "wedlin", "pasztet", "kaszank", "losos", "ryba", "ryby", "sledz", "tunczyk", "dorsz", "makrel", "pstrag", "mintaj",
        "krewet", "paluszki krab", "konserw",
        // Fruit and vegetables.
        "jabl", "banan", "pomarancz", "mandaryn", "cytryn", "limonk", "grejpfrut", "gruszk", "sliwk", "brzoskw", "morel",
        "truskaw", "malin", "borowk", "jagod", "winogron", "arbuz", "melon", "ananas", "kiwi", "mango", "awokado", "granat",
        "pomid", "ogor", "ziemn", "marchew", "marchw", "cebul", "czosn", "salat", "kapust", "papryk", "pieczark", "brokul",
        "kalafior", "cukini", "baklazan", "szpinak", "rzodk", "burak", "seler", "por ", "pietrusz", "koper", "fasol", "groch",
        "soczewic", "kukurydz", "oliwk", "dyni", "rukol", "warzyw", "owoc", "natka", "szczypior",
        // Dry goods, sauces and spices.
        "makaron", "spaghetti", "penne", "fusilli", "ryz", "kasz", "maka", "mak ", "cukier", "sol ", "pieprz", "przypraw",
        "ketchup", "majonez", "musztard", "sos ", "sosy", "koncentrat", "pesto", "passata", "olej", "oliwa", "ocet", "rosol",
        "bulion", "zupa", "zupk", "zurek", "barszcz", "platki", "musli", "granola", "owsian", "dzem", "miod", "powidl",
        "konfitur", "nutella", "krem czekol", "proszek do piecz", "drozdze", "cukier waniliow", "budyn", "knorr", "winiary",
        // Sweets and snacks.
        "czekol", "baton", "cukierk", "lizak", "zelk", "haribo", "tiramisu", "lody", "lod ", "lodow", "chips", "chrupk",
        "paluszk", "popcorn", "orzech", "orzesz", "migdal", "pestk", "rodzyn", "slonecz", "delicje", "grzesk", "prince polo",
        "princess", "snickers", "twix", "kitkat", "milka", "wedel", "michalk", "krakers", "ciasteczk", "ciastk",
        // Drinks.
        "kawa", "kawy", "herbat", "kakao", "sok ", "soki", "soczek", "nektar", "woda", "napoj", "nap ", "cola", "pepsi",
        "fanta", "sprite", "lemoniad", "kompot", "energet", "red bull", "tymbark", "kubus", "piwo", "piwa", "wino ", "wina ",
        "wodka", "whisky", "cydr",
        // Ready meals.
        "pierog", "pizza", "frytk", "nalesn", "kopytk", "uszka", "krokiet", "hummus", "tofu", "sushi", "kanapk", "salatk",
        "hot dog", "zapiekank", "gotowe danie",
        // Pets eat too.
        "karma",
    ]),
    (CAT_HOUSEHOLD, &[
        "szampon", "odzywk", "mydl", "zel pod", "zel do", "zel odsw", "odsw", "dezodor", "antyperspir", "pasta do zeb",
        "szczoteczk", "nic dentyst", "plyn do plukania ust", "krem", "balsam", "olejek", "mascar", "tusz do rzes", "pomadk",
        "lakier", "farba do wlos", "perfum", "woda toalet", "woda perf", "woda micel", "plyn micel", "kosmet", "wacik",
        "patyczk", "podpask", "tampon", "wkladk", "pieluch", "chusteczk", "maszynk", "pianka do gol", "nivea",
        "proszek", "plyn", "kapsulki do pran", "tabletki do zmyw", "do naczyn", "do pran", "plukank", "domestos", "ludwik",
        "persil", "ariel", "lenor", "vanish", "perwoll", "fairy", "finish", "somat", "cif ", "ajax", "pronto", "clin ",
        "wybielacz", "odkamieniacz", "papier toalet", "recznik", "serwet", "worki", "worek", "folia", "gabk", "zmywak",
        "sciereczk", "scierk", "mop", "drogery", "drogeri", "tekstyl", "bateri", "zarowk", "swieczk", "znicz", "torba",
        "reklamowk", "zwirek",
        // Home bills, as transfer titles name them.
        "czynsz", "najem", "wynajem", "mieszkani", "gaz ", "prad ", "ogrzewani", "wywoz smieci",
    ]),
    (CAT_HEALTH, &[
        "apteka", "lek ", "leki ", "tabletk", "kapsulki", "syrop", "witamin", "suplement", "probiot", "bandaz", "plaster",
        "recept", "sasz", "aer ", "masc", "krople", "paracetamol", "ibuprofen", "ibuprom", "apap", "nurofen", "strepsils",
        "woda utlen", "termometr", "test ciaz", "prezerwatyw", "opatrun",
    ]),
    (CAT_CLOTHING, &[
        "skarpet", "koszul", "spodni", "bluz", "kurtk", "bielizn", "czapk", "buty", "rajstop", "majtk", "sukienk", "t shirt",
        "szalik", "rekawicz", "pizam", "kapcie",
    ]),
    (CAT_TRANSPORT, &["paliwo", "benzyn", "diesel", "pb95", "pb 95", "pb98", "pb 98", "on ", "lpg", "bilet", "parking", "autostrad", "myjnia", "adblue"]),
    (CAT_CAFES, &["kawiarni", "restauracj", "lunch", "obiad", "latte", "cappuccino", "espresso", "americano", "kebab", "burger"]),
    (CAT_ENTERTAINMENT, &["kino ", "teatr", "koncert", "ksiazk", "zabawk", "gra ", "puzzle", "klocki"]),
];

/// Shops where almost everything belongs to one category. Grocery shops make Food the default for a line
/// whose name says nothing.
const SHOPS: &[(Uuid, &[&str])] = &[
    (CAT_HEALTH, &["apteka", "doz ", "gemini", "ziko", "super pharm"]),
    (CAT_TRANSPORT, &["orlen", "shell", "circle k", "bp ", "moya", "amic", "pkp", "koleje", "mpk", "ztm", "uber", "bolt"]),
    (CAT_CAFES, &["mcdonald", "kfc", "starbucks", "costa", "burger king", "pizzeria", "restauracja", "kawiarnia", "bar "]),
    (CAT_HOUSEHOLD, &[
        "rossmann", "hebe", "castorama", "leroy", "obi ", "tedi", "dealz", "action", "kik ", "flying tiger", "jysk",
        "ikea", "agata", "home you", "duka", "mrowka", "bricomarche", "psb ",
    ]),
    (CAT_CLOTHING, &["reserved", "h m", "zara", "pepco", "ccc ", "deichmann", "sinsay"]),
    (CAT_ENTERTAINMENT, &["multikino", "cinema city", "helios", "empik"]),
    (CAT_FOOD, &[
        "biedronka", "jeronimo", "lidl", "zabka", "kaufland", "auchan", "carrefour", "netto", "dino ", "stokrotka", "aldi",
        "lewiatan", "polomarket", "intermarche", "spar ", "eurospar", "delikatesy", "spolem", "piotr i pawel", "frisco",
        "freshmarket", "groszek", "chata polska", "topaz", "makro", "selgros", "piekarni", "cukierni", "spozywcz",
        "supermarket", "hipermarket", "market",
    ]),
];

/// Splits names that receipts glue together ("LodyMarlAlmond120ml", "SerGouda") into words, then folds them.
fn words(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    let mut prev: Option<char> = None;
    for c in s.chars() {
        if let Some(p) = prev {
            let camel = p.is_lowercase() && c.is_uppercase();
            let digits = p.is_alphabetic() != c.is_alphabetic() && (p.is_ascii_digit() || c.is_ascii_digit());
            if camel || digits {
                out.push(' ');
            }
        }
        out.push(c);
        prev = Some(c);
    }
    fold(&out)
}

/// The longest entry found at the start of a word, with its length.
fn best(list: &[(Uuid, &[&str])], text: &str) -> Option<(Uuid, usize)> {
    let padded = format!(" {text} ");
    list.iter()
        .flat_map(|(cat, ws)| ws.iter().filter(|w| padded.contains(&format!(" {w}"))).map(|w| (*cat, w.trim_end().len())))
        .max_by_key(|x| x.1)
}

/// OCR sometimes splits a word ("Ludwi k900g"): the name is also tried with spaces removed, for entries long
/// enough not to turn up by chance inside other words.
fn joined(list: &[(Uuid, &[&str])], text: &str) -> Option<Uuid> {
    let t = text.replace(' ', "");
    list.iter()
        .flat_map(|(cat, ws)| ws.iter().map(|w| w.trim_end()).filter(|w| w.len() >= 5 && !w.contains(' ') && t.contains(w)).map(|w| (*cat, w.len())))
        .max_by_key(|x| x.1)
        .map(|x| x.0)
}

/// The dictionary and shop part, without personal rules; `fallback` when neither says anything.
fn suggest(merchant: &str, item: &str, fallback: Uuid) -> Uuid {
    let m = fold(merchant);
    if best(&SHOPS[..1], &m).is_some() {
        return CAT_HEALTH;
    }
    let i = words(item);
    best(WORDS, &i).map(|x| x.0)
        .or_else(|| joined(WORDS, &i))
        .or_else(|| best(SHOPS, &m).map(|x| x.0))
        .unwrap_or(fallback)
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
    by_rules(rules, merchant_id, item).unwrap_or_else(|| suggest(merchant, item, CAT_OTHER))
}

/// A line of a shop receipt. Most of what a family buys on receipts is food, so a line that neither its name
/// nor its shop explains is Food rather than Other.
pub fn categorize_line(rules: &[CategoryRule], merchant_id: Option<Uuid>, merchant: &str, item: &str) -> Uuid {
    by_rules(rules, merchant_id, item).unwrap_or_else(|| suggest(merchant, item, CAT_FOOD))
}

fn by_rules(rules: &[CategoryRule], merchant_id: Option<Uuid>, item: &str) -> Option<Uuid> {
    let pattern = fold(item);
    let find = |m: Option<Uuid>| rules.iter().find(|r| r.merchant_id == m && r.pattern == pattern).map(|r| r.category_id);
    merchant_id.and_then(|m| find(Some(m))).or_else(|| find(None))
}

/// The name of a line whose name could not be read.
pub const UNKNOWN_ITEM: &str = "Unknown item";

/// Whether a recognized name carries anything readable: at least two letters in a row.
pub fn readable(name: &str) -> bool {
    let mut run = 0;
    for c in name.chars() {
        run = if c.is_alphabetic() { run + 1 } else { 0 };
        if run >= 2 {
            return true;
        }
    }
    false
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
            assert_eq!(categorize(&[], None, "Kiosk", name), want, "{name}");
        }
    }

    #[test]
    fn glued_and_shortened_receipt_names_are_read() {
        for (name, want) in [
            ("SerGoudaPlastry150g", CAT_FOOD), ("JogNatur400g", CAT_FOOD), ("MlekoLaciate2%1L", CAT_FOOD),
            ("KajzerkaDuza", CAT_FOOD), ("PiersZKurczaka", CAT_FOOD), ("BananLuz", CAT_FOOD), ("PomidorMalinowy", CAT_FOOD),
            ("Woda Cisowianka 1,5L", CAT_FOOD), ("Ziemniaki 2kg", CAT_FOOD), ("Kawa Ziarnista 1kg", CAT_FOOD),
            ("Proszek do pieczenia", CAT_FOOD), ("Krem czekoladowy", CAT_FOOD), ("Kremówka", CAT_FOOD),
            ("Woda toaletowa 50ml", CAT_HOUSEHOLD), ("Serwetki 100szt", CAT_HOUSEHOLD), ("Olejek do ciała", CAT_HOUSEHOLD),
            ("Tabletki do zmywarki", CAT_HOUSEHOLD), ("Mascara czarna", CAT_HOUSEHOLD), ("Torba foliowa", CAT_HOUSEHOLD),
            ("Ibuprom 10 tabl", CAT_HEALTH),
        ] {
            assert_eq!(categorize_line(&[], None, "Kiosk", name), want, "{name}");
        }
    }

    #[test]
    fn a_stem_inside_another_word_does_not_count() {
        // "ryz" inside "fryzjer", "kino" inside "kina" is fine, "ser" inside "laser" is not cheese.
        assert_eq!(categorize(&[], None, "Kiosk", "Fryzjer"), CAT_OTHER);
        assert_eq!(categorize(&[], None, "Kiosk", "Bilet do kina"), CAT_TRANSPORT);
        assert_eq!(categorize(&[], None, "Kiosk", "Laser"), CAT_OTHER);
    }

    #[test]
    fn a_receipt_line_nothing_explains_is_food_unless_the_shop_says_otherwise() {
        assert_eq!(categorize_line(&[], None, "Biedronka", "Marl Almond 120"), CAT_FOOD);
        assert_eq!(categorize_line(&[], None, "", "XKZ 300"), CAT_FOOD);
        assert_eq!(categorize_line(&[], None, "Rossmann", "Isana 200"), CAT_HOUSEHOLD);
        assert_eq!(categorize_line(&[], None, "Orlen", "Pozycja 12"), CAT_TRANSPORT);
        // OCR misreads from a real TEDi receipt.
        assert_eq!(categorize_line(&[], None, "TEDi", "Produkty drogeryine"), CAT_HOUSEHOLD);
        assert_eq!(categorize_line(&[], None, "TEDi", "Produkty drogery"), CAT_HOUSEHOLD);
        assert_eq!(categorize_line(&[], None, "TEDi", "Tekstylia donove ogólne"), CAT_HOUSEHOLD);
        assert_eq!(categorize_line(&[], None, "Dealz", "Kubek 300"), CAT_HOUSEHOLD);
        assert_eq!(categorize(&[], None, "", "XKZ 300"), CAT_OTHER, "a bank line keeps Other");
        assert_eq!(categorize(&[], None, "Lidl sp. z o.o.", "Zakup"), CAT_FOOD);
    }

    #[test]
    fn names_without_letters_are_not_readable() {
        assert!(readable("Mleko"));
        assert!(readable("Ser 2"));
        for bad in ["", "  ", "12,99", "*#4", "a 1 b", "—"] {
            assert!(!readable(bad), "{bad:?}");
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
