//! Parsing of Polish fiscal receipts from recognized lines, with the arithmetic checks
//! the review screen relies on. Nothing here guesses silently: anything that does not
//! add up becomes a `Doubt`.

use crate::categorize::fold;
use crate::date::Date;
use crate::model::{Payment, valid_nip};
use crate::money::{Qty, line_value, parse_amount};
use crate::ocr::Line;
use regex_lite::Regex;
use std::sync::OnceLock;

/// Lines read with less confidence than this are marked for review.
const LOW_CONFIDENCE: f32 = 0.8;
const HEADER_LINES: usize = 10;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedItem {
    pub raw_name: String,
    pub qty: Qty,
    pub unit: String,
    pub unit_price: i64,
    /// Value printed on the receipt, before discounts.
    pub value: i64,
    pub discount: i64,
    pub ptu: Option<char>,
    pub doubtful: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VatLine {
    pub letter: char,
    pub rate_bp: Option<u32>,
    pub gross: Option<i64>,
    pub tax: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Doubt {
    Merchant,
    Nip,
    Date,
    Total,
    Item(usize),
    /// Items after discounts add up to `items`, while the receipt says `total`.
    ItemsSum { items: i64, total: i64 },
    Vat,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    pub merchant: Option<String>,
    pub nip: Option<String>,
    pub date: Option<Date>,
    pub time: Option<String>,
    pub receipt_no: Option<String>,
    pub fiscal: bool,
    pub total: Option<i64>,
    pub payment: Option<Payment>,
    pub items: Vec<ParsedItem>,
    pub vat: Vec<VatLine>,
    pub vat_total: Option<i64>,
    pub doubts: Vec<Doubt>,
}

impl Parsed {
    pub fn items_total(&self) -> i64 {
        self.items.iter().map(|i| i.value - i.discount).sum()
    }
}

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("valid pattern"))
}

static AMOUNT: OnceLock<Regex> = OnceLock::new();
static ITEM: OnceLock<Regex> = OnceLock::new();
static PLAIN_ITEM: OnceLock<Regex> = OnceLock::new();
static DATE_ISO: OnceLock<Regex> = OnceLock::new();
static DATE_PL: OnceLock<Regex> = OnceLock::new();
static TIME: OnceLock<Regex> = OnceLock::new();
static RATE: OnceLock<Regex> = OnceLock::new();
static LETTER: OnceLock<Regex> = OnceLock::new();
static RECEIPT_NO: OnceLock<Regex> = OnceLock::new();
static NAME_TAIL: OnceLock<Regex> = OnceLock::new();

/// OCR quirks fixed before parsing: "3, 97" and "3 ,97" become "3,97", "7.00" stays a valid amount.
fn clean(s: &str) -> String {
    let spaced: OnceLock<Regex> = OnceLock::new();
    let r = spaced.get_or_init(|| Regex::new(r"(\d)\s*([,.])\s+(\d{2})\b").expect("valid pattern"));
    r.replace_all(s.trim(), "$1$2$3").replace('×', "x")
}

fn amounts(s: &str) -> Vec<i64> {
    re(&AMOUNT, r"(-?)(\d{1,6}[,.]\d{2})\b")
        .captures_iter(s)
        .filter_map(|c| {
            let v = parse_amount(&c[2]).ok()?;
            Some(if &c[1] == "-" { -v } else { v })
        })
        .collect()
}

fn unit_of(s: &str) -> String {
    match s.trim_end_matches('.').to_lowercase().as_str() {
        "kg" => "kg".into(),
        "g" => "g".into(),
        "l" => "l".into(),
        "ml" => "ml".into(),
        _ => "pcs".into(),
    }
}

fn tidy_raw(s: &str) -> String {
    let r = re(&NAME_TAIL, r"(\s+[A-G])?\s*[.:]?\s*\d{4,}\s*[A-G]?\W*$|\s+[A-G]\W*$");
    r.replace(s.trim(), "").trim().trim_end_matches(['.', ',', ':']).trim().to_string()
}

const CHAINS: &[(&str, &str)] = &[
    ("biedronka", "Biedronka"), ("lidl", "Lidl"), ("zabka", "Żabka"), ("dealz", "Dealz"), ("rossmann", "Rossmann"),
    ("tedi", "TEDi"), ("kaufland", "Kaufland"), ("auchan", "Auchan"), ("carrefour", "Carrefour"), ("netto", "Netto"),
    ("stokrotka", "Stokrotka"), ("aldi", "Aldi"), ("pepco", "Pepco"), ("hebe", "Hebe"), ("orlen", "Orlen"),
    ("dino", "Dino"), ("lewiatan", "Lewiatan"), ("intermarche", "Intermarché"), ("polomarket", "POLOmarket"),
    ("apteka gemini", "Apteka Gemini"), ("action", "Action"), ("castorama", "Castorama"), ("leroy merlin", "Leroy Merlin"),
    ("mcdonald", "McDonald's"), ("kfc", "KFC"), ("starbucks", "Starbucks"), ("circle k", "Circle K"), ("shell", "Shell"),
];

fn chain(line: &str) -> Option<&'static str> {
    let f = format!(" {} ", fold(line));
    CHAINS.iter().find(|(k, _)| f.contains(&format!(" {k}")) || f.contains(&format!(" {k} "))).map(|c| c.1)
}

fn starts(f: &str, words: &[&str]) -> bool {
    words.iter().any(|w| f.starts_with(w))
}

fn is_footer(f: &str, raw: &str) -> bool {
    starts(f, &["sprzed", "sp op", "suma", "kwota ptu", "ptu ", "razem", "rozliczenie", "do zaplaty", "zaplacono"])
        || raw.trim().chars().filter(|c| *c == '-' || *c == '=' || *c == '_').count() > 8
}

fn is_noise(f: &str) -> bool {
    starts(f, &["bez recepty", "na recepte", "rabat", "opust", "upust", "obnizka", "promocja"]) || f.is_empty()
}

fn find_nip(lines: &[&str]) -> (Option<String>, bool) {
    for l in lines {
        let f = fold(l);
        let Some(at) = f.find("nip") else { continue };
        let digits: String = f[at + 3..].chars()
            .map(|c| match c { 'o' => '0', 'i' | 'l' => '1', c => c })
            .filter(|c| c.is_ascii_digit())
            .take(10)
            .collect();
        return if valid_nip(&digits) { (Some(digits), true) } else { (Some(digits).filter(|d| !d.is_empty()), false) };
    }
    (None, false)
}

fn find_date(lines: &[&str]) -> (Option<Date>, Option<String>) {
    let mut date = None;
    let mut time = None;
    for l in lines {
        if date.is_none() {
            if let Some(c) = re(&DATE_ISO, r"\b(20\d\d)[-./](\d\d)[-./](\d\d)\b").captures(l) {
                date = Date::new(c[1].parse().unwrap_or(0), c[2].parse().unwrap_or(0), c[3].parse().unwrap_or(0));
            } else if let Some(c) = re(&DATE_PL, r"\b(\d\d)[-./](\d\d)[-./](20\d\d)\b").captures(l) {
                date = Date::new(c[3].parse().unwrap_or(0), c[2].parse().unwrap_or(0), c[1].parse().unwrap_or(0));
            }
        }
        if time.is_none() {
            time = re(&TIME, r"\b([01]\d|2[0-3]):([0-5]\d)\b").captures(l).map(|c| format!("{}:{}", &c[1], &c[2]));
        }
    }
    (date, time)
}

fn parse_item(text: &str) -> Option<(String, ParsedItem)> {
    let r = re(&ITEM, r"^(.*?)\s*(\d{1,4}(?:[.,]\d{1,3})?)\s*(szt\.?|op\.?|pcs|kg|g|l|ml)?\s*[xX*+]\s*(\d{1,6}[.,]\d{2})\s*=?\s*(\d{1,6}[.,]\d{2})\s*([A-Ga-g8])?\W*$");
    if let Some(c) = r.captures(text) {
        let qty = Qty::parse(&c[2]).ok()?;
        let unit_price = parse_amount(&c[4]).ok()?;
        let value = parse_amount(&c[5]).ok()?;
        let ptu = c.get(6).and_then(|m| m.as_str().chars().next()).map(|p| if p == '8' { 'B' } else { p.to_ascii_uppercase() });
        let unit = c.get(3).map(|m| unit_of(m.as_str())).unwrap_or_else(|| if c[2].contains([',', '.']) { "kg".into() } else { "pcs".into() });
        return Some((c[1].to_string(), ParsedItem { raw_name: String::new(), qty, unit, unit_price, value, discount: 0, ptu, doubtful: false }));
    }
    None
}

fn parse_plain_item(text: &str) -> Option<ParsedItem> {
    let r = re(&PLAIN_ITEM, r"^([^\d-].*?)\s+(\d{1,6}[.,]\d{2})\s*([A-G])\W*$");
    let c = r.captures(text)?;
    let value = parse_amount(&c[2]).ok()?;
    Some(ParsedItem {
        raw_name: tidy_raw(&c[1]), qty: Qty::ONE, unit: "pcs".into(), unit_price: value, value, discount: 0,
        ptu: c[3].chars().next(), doubtful: false,
    })
}

pub fn parse(input: &[Line]) -> Parsed {
    let texts: Vec<String> = input.iter().map(|l| clean(&l.text)).collect();
    let folded: Vec<String> = texts.iter().map(|t| fold(t)).collect();
    let all: Vec<&str> = texts.iter().map(String::as_str).collect();
    let mut p = Parsed::default();

    let fiscal_at = folded.iter().position(|f| f.contains("paragon fisk") || f.contains("fiskaln"));
    p.fiscal = fiscal_at.is_some();
    let header_end = fiscal_at.unwrap_or(HEADER_LINES.min(texts.len()));
    let header = &all[..header_end];

    p.merchant = header.iter().find_map(|l| chain(l)).map(str::to_string).or_else(|| {
        header.iter()
            .find(|l| { let f = fold(l); f.contains("sp z o o") || f.contains(" s a") || f.ends_with(" sa") })
            .or_else(|| header.iter().find(|l| l.chars().filter(|c| c.is_alphabetic()).count() >= 3))
            .map(|l| l.trim().to_string())
    });
    if p.merchant.is_none() {
        p.doubts.push(Doubt::Merchant);
    }

    let (nip, nip_ok) = find_nip(&all);
    p.nip = nip;
    if !nip_ok {
        p.doubts.push(Doubt::Nip);
    }
    (p.date, p.time) = find_date(&all);
    if p.date.is_none() {
        p.doubts.push(Doubt::Date);
    }
    p.receipt_no = all.iter().find_map(|l| {
        re(&RECEIPT_NO, r"(?i)nr\s*(?:wydr|dok|paragonu)\.?\s*:?\s*([0-9][0-9/]*)").captures(l).map(|c| c[1].to_string())
    });

    // Items: between the fiscal header and the first line of the summary.
    let start = fiscal_at.map(|i| i + 1).unwrap_or(0);
    let end = (start..texts.len()).find(|i| is_footer(&folded[*i], &texts[*i])).unwrap_or(texts.len());
    let mut last_name_line: Option<String> = None;
    for i in start..end {
        let (t, f) = (&texts[i], &folded[i]);
        let low = input[i].confidence < LOW_CONFIDENCE;
        if starts(f, &["rabat", "opust", "upust", "obnizka"]) {
            if let (Some(a), Some(item)) = (amounts(t).last(), p.items.last_mut()) {
                item.discount += a.abs();
                item.doubtful |= low;
            }
            continue;
        }
        if let Some((name, mut item)) = parse_item(t) {
            let own = tidy_raw(&name);
            let has_name = own.chars().filter(|c| c.is_alphabetic()).count() >= 2;
            item.raw_name = if has_name { own } else { last_name_line.take().unwrap_or_default() };
            item.doubtful = low || item.raw_name.is_empty();
            p.items.push(item);
            last_name_line = None;
            continue;
        }
        if is_noise(f) {
            continue;
        }
        // A line with a value but no multiplication is a whole item on some registers.
        if let Some(mut item) = parse_plain_item(t) {
            if let Some(n) = last_name_line.take()
                && item.raw_name.chars().filter(|c| c.is_alphabetic()).count() < 2 {
                    item.raw_name = n;
                }
            item.doubtful = true;
            p.items.push(item);
            continue;
        }
        // A product code on its own line keeps the name from the line above.
        if t.chars().filter(|c| c.is_alphabetic()).count() >= 2 {
            last_name_line = Some(tidy_raw(t));
        }
    }

    // Summary.
    for i in end..texts.len() {
        let (t, f) = (&texts[i], &folded[i]);
        if p.total.is_none() && f.contains("suma") && f.contains("pln") && !f.contains("ptu") {
            p.total = amounts(t).last().copied().or_else(|| texts.get(i + 1).and_then(|n| amounts(n).first().copied()));
        }
        if f.starts_with("suma ptu") || f.starts_with("razem ptu") {
            p.vat_total = amounts(t).last().copied().or_else(|| texts.get(i + 1).and_then(|n| amounts(n).first().copied()));
            continue;
        }
        let letter = re(&LETTER, r"(?i)(?:ptu|op\.?)\s*([A-G])\b").captures(t).and_then(|c| c[1].chars().next()).map(|c| c.to_ascii_uppercase());
        if let Some(letter) = letter {
            let entry = match p.vat.iter_mut().position(|v| v.letter == letter) {
                Some(i) => &mut p.vat[i],
                None => {
                    p.vat.push(VatLine { letter, ..Default::default() });
                    p.vat.last_mut().expect("just pushed")
                }
            };
            let rate = re(&RATE, r"(\d{1,2})[,.](\d{2})\s*%").captures(t).map(|c| c[1].parse::<u32>().unwrap_or(0) * 100 + c[2].parse::<u32>().unwrap_or(0));
            let last = amounts(t).last().copied();
            if f.starts_with("sprzed") || f.starts_with("sp op") {
                entry.gross = last.or_else(|| texts.get(i + 1).and_then(|n| amounts(n).first().copied()));
            } else if let Some(r) = rate {
                entry.rate_bp = Some(r);
                let after_rate: Vec<i64> = t.split('%').nth(1).map(amounts).unwrap_or_default();
                entry.tax = after_rate.last().copied().or_else(|| texts.get(i + 1).and_then(|n| amounts(n).first().copied()));
            }
        }
        if p.payment.is_none() {
            p.payment = if f.contains("karta") || f.contains("karten") || f.contains("card") || f.contains("visa") || f.contains("mastercard") {
                Some(Payment::Card)
            } else if f.contains("gotowka") || f.contains("gotowk") {
                Some(Payment::Cash)
            } else if f.contains("blik") || f.contains("przelew") {
                Some(Payment::Transfer)
            } else {
                None
            };
        }
    }
    if p.total.is_none() {
        p.total = texts[end.min(texts.len())..].iter().zip(&folded[end.min(texts.len())..])
            .find(|(_, f)| f.contains("do zaplaty"))
            .and_then(|(t, _)| amounts(t).last().copied());
    }

    // Checks.
    for (i, it) in p.items.iter_mut().enumerate() {
        let ok = line_value(it.qty, it.unit_price) == Some(it.value) && it.discount <= it.value;
        if !ok {
            it.doubtful = true;
        }
        if it.doubtful {
            p.doubts.push(Doubt::Item(i));
        }
    }
    match p.total {
        None => p.doubts.push(Doubt::Total),
        Some(total) => {
            let items = p.items_total();
            if items != total {
                p.doubts.push(Doubt::ItemsSum { items, total });
            }
        }
    }
    let vat_ok = p.vat.iter().all(|v| match (v.rate_bp, v.gross, v.tax) {
        (Some(r), Some(g), Some(t)) => {
            let expect = (g as i128 * r as i128 * 2 + (10000 + r) as i128) / (2 * (10000 + r) as i128);
            (expect as i64 - t).abs() <= 1
        }
        _ => true,
    });
    let gross_sum: Option<i64> = p.vat.iter().map(|v| v.gross).sum();
    let tax_sum: Option<i64> = p.vat.iter().map(|v| v.tax).sum();
    let gross_ok = match (gross_sum, p.total) {
        (Some(g), Some(t)) if !p.vat.is_empty() => g == t,
        _ => true,
    };
    let tax_ok = match (tax_sum, p.vat_total) {
        (Some(s), Some(t)) if !p.vat.is_empty() => s == t,
        _ => true,
    };
    if !(vat_ok && gross_ok && tax_ok) {
        p.doubts.push(Doubt::Vat);
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<Line> {
        s.lines().map(|t| Line { text: t.to_string(), confidence: 0.95 }).collect()
    }

    /// Names on their own line, price line below with "op" and "=", as in pharmacies.
    const PHARMACY: &str = "Apteka Testowa Sp. z o.o.
00-001 Warszawa, ul. Prosta 1
NIP: 123-456-32-18
nr dok.000123
PARAGON FISKALNY
Testolek aer 1mg/g 50ml.41731 B
1 op * 18,49 = 18,49 B
Bez recepty 18,49
Probiotyk x 60 sasz. .19865 B
1 op * 104,99 = 104,99 B
Bez recepty 104,99
Sp.op.B 123,48
PTU B= 8,00% 9,15
SUMA PTU 9,15
SUMA PLN 123,48
DO ZAPŁATY PLN 123,48
ZAPŁACONO GOTÓWKA PLN 123,48
00122/1118 #UZ/09 26.10.2026 19:34";

    /// Name on one line, product code and price on the next.
    const CODES: &str = "Sklep Testowy Sp. z o.o.
NIP 1234563218
PARAGON FISKALNY nr wydr. 410987/1227
Produkty drogeryjne
355110015610000 1pcs * 5,00 5,00 A
Tekstylia domowe ogólne
695810018510000 1pcs * 17,00 17,00 A
Sprzed. opod. PTU A 22,00
Kwota PTU A 23,00% 4,11
SUMA PTU 4,11
SUMA PLN 22,00
Płatność andere Karten 22,00
2026-10-30 18:18";

    /// Everything on one line, with a discount line.
    const INLINE: &str = "Market Prosty
NIP 123-456-32-18
PARAGON FISKALNY
MINI TIRAMISU 300G 1 * 7,00 7,00 C
ZEL ODSW 140G 3 * 8,00 24,00 A
OPUST ZEL ODSW 140 -4,00 A
Suma Opustów -4,00
Sprzed. opod. PTU A 20,00
Sprzed. opod. PTU C 7,00
Kwota PTU A 23,00% 3,74
Kwota PTU C 05,00% 0,33
SUMA PTU 4,07
SUMA PLN 27,00
Karta 27,00
2026-10-31 10:44";

    #[test]
    fn pharmacy_format() {
        let p = parse(&lines(PHARMACY));
        assert_eq!(p.merchant.as_deref(), Some("Apteka Testowa Sp. z o.o."));
        assert_eq!(p.nip.as_deref(), Some("1234563218"));
        assert_eq!(p.date, Date::new(2026, 10, 26));
        assert_eq!(p.time.as_deref(), Some("19:34"));
        assert_eq!(p.total, Some(12348));
        assert_eq!(p.payment, Some(Payment::Cash));
        assert_eq!(p.receipt_no.as_deref(), Some("000123"));
        assert_eq!(p.items.len(), 2);
        assert_eq!(p.items[0].raw_name, "Testolek aer 1mg/g 50ml");
        assert_eq!(p.items[1].raw_name, "Probiotyk x 60 sasz");
        assert_eq!((p.items[1].unit_price, p.items[1].value, p.items[1].ptu), (10499, 10499, Some('B')));
        assert_eq!(p.vat, vec![VatLine { letter: 'B', rate_bp: Some(800), gross: Some(12348), tax: Some(915) }]);
        assert!(p.doubts.is_empty(), "{:?}", p.doubts);
    }

    #[test]
    fn product_codes_between_name_and_price() {
        let p = parse(&lines(CODES));
        assert_eq!(p.items.iter().map(|i| i.raw_name.as_str()).collect::<Vec<_>>(), ["Produkty drogeryjne", "Tekstylia domowe ogólne"]);
        assert_eq!(p.total, Some(2200));
        assert_eq!(p.payment, Some(Payment::Card));
        assert_eq!(p.receipt_no.as_deref(), Some("410987/1227"));
        assert!(p.doubts.is_empty(), "{:?}", p.doubts);
    }

    #[test]
    fn inline_items_with_a_discount() {
        let p = parse(&lines(INLINE));
        assert_eq!(p.merchant.as_deref(), Some("Market Prosty"));
        assert_eq!(p.items.len(), 2);
        assert_eq!((p.items[1].qty, p.items[1].value, p.items[1].discount), (Qty::parse("3").unwrap(), 2400, 400));
        assert_eq!(p.items_total(), 2700);
        assert_eq!(p.vat_total, Some(407));
        assert!(p.doubts.is_empty(), "{:?}", p.doubts);
    }

    #[test]
    fn ocr_noise_in_amounts_is_tolerated() {
        let mut l = lines(INLINE);
        l[3].text = "MINI TIRAMISU 300G 1 * 7.00 7, 00 C".into();
        l[12].text = "SUMA PLN 27.00".into();
        let p = parse(&l);
        assert_eq!(p.items[0].value, 700);
        assert_eq!(p.total, Some(2700));
        assert!(p.doubts.is_empty(), "{:?}", p.doubts);
    }

    #[test]
    fn weighed_items_keep_three_decimals() {
        let p = parse(&lines("Sklep\nPARAGON FISKALNY\nJabłka luz\n1,614 x21,83 35,23 C\nSUMA PLN 35,23"));
        assert_eq!(p.items[0].qty, Qty::parse("1,614").unwrap());
        assert_eq!(p.items[0].unit, "kg");
        assert_eq!(p.items[0].raw_name, "Jabłka luz");
        assert!(!p.doubts.iter().any(|d| matches!(d, Doubt::Item(_) | Doubt::ItemsSum { .. })));
    }

    #[test]
    fn a_wrong_line_value_is_flagged() {
        let p = parse(&lines("Sklep\nPARAGON FISKALNY\nMleko 2 x3,29 6,60 C\nSUMA PLN 6,60"));
        assert!(p.doubts.contains(&Doubt::Item(0)));
    }

    #[test]
    fn a_wrong_total_is_flagged() {
        let p = parse(&lines("Sklep\nPARAGON FISKALNY\nMleko 2 x3,29 6,58 C\nSUMA PLN 6,85"));
        assert!(p.doubts.contains(&Doubt::ItemsSum { items: 658, total: 685 }));
    }

    #[test]
    fn missing_total_and_date_are_flagged() {
        let p = parse(&lines("Sklep\nPARAGON FISKALNY\nMleko 1 x3,29 3,29 C"));
        assert!(p.doubts.contains(&Doubt::Total) && p.doubts.contains(&Doubt::Date));
    }

    #[test]
    fn a_bad_nip_is_flagged_and_letters_read_as_digits_are_fixed() {
        assert!(parse(&lines("Sklep\nNIP 1234563217")).doubts.contains(&Doubt::Nip));
        assert_eq!(parse(&lines("Sklep\nNIP 12345632l8")).nip.as_deref(), Some("1234563218"));
    }

    #[test]
    fn vat_that_does_not_match_is_flagged() {
        let mut l = lines(INLINE);
        l[9].text = "Kwota PTU A 23,00% 3,99".into();
        assert!(parse(&l).doubts.contains(&Doubt::Vat));
    }

    #[test]
    fn known_chains_are_named_even_below_the_logo() {
        let p = parse(&lines("Dobre ceny zawsze\nTest Poland Sp. z o.o.\nsklep \"BIEDRONKA\"\nPARAGON FISKALNY"));
        assert_eq!(p.merchant.as_deref(), Some("Biedronka"));
    }

    #[test]
    fn low_confidence_items_are_doubtful() {
        let mut l = lines(INLINE);
        l[3].confidence = 0.5;
        assert!(parse(&l).doubts.contains(&Doubt::Item(0)));
    }

    #[test]
    fn garbage_does_not_panic() {
        for s in ["", "\n\n", "x * 9,99 =", "SUMA PLN", "PARAGON FISKALNY\nRABAT -1,00", "1 x 1,00 1,00\n2026-02-30"] {
            let _ = parse(&lines(s));
        }
    }
}
