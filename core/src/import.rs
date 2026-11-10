//! Import of bank operations from CSV, with column mapping. Everything runs on the device.
//! Bank-specific profiles come later, each tested on a real statement.

use crate::categorize::fold;
use crate::date::Date;
use crate::money::parse_amount;
use uuid::Uuid;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Table {
    pub header: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

/// Polish banks still export Windows-1250 next to UTF-8. Invalid UTF-8 is read as Windows-1250.
pub fn decode(bytes: &[u8]) -> String {
    let body = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    match std::str::from_utf8(body) {
        Ok(s) => s.to_string(),
        Err(_) => encoding_rs::WINDOWS_1250.decode(body).0.into_owned(),
    }
}

fn split_line(line: &str, delim: char) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                chars.next();
                out.last_mut().expect("never empty").push('"');
            }
            '"' => quoted = !quoted,
            c if c == delim && !quoted => out.push(String::new()),
            c => out.last_mut().expect("never empty").push(c),
        }
    }
    out.into_iter().map(|s| s.trim().to_string()).collect()
}

/// Joins physical lines that sit inside a quoted field.
fn records(text: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut quotes = 0usize;
    for line in text.lines() {
        if !cur.is_empty() {
            cur.push('\n');
        }
        cur.push_str(line);
        quotes += line.matches('"').count();
        if quotes.is_multiple_of(2) {
            out.push(std::mem::take(&mut cur));
            quotes = 0;
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Reads a CSV with `;`, `,` or tab. Banks put account details above the table, so the header is
/// taken as the first row with the most columns.
pub fn parse_csv(text: &str) -> Table {
    let lines: Vec<String> = records(text).into_iter().filter(|l| !l.trim().is_empty()).collect();
    let sample = &lines[..lines.len().min(30)];
    let delim = [';', ',', '\t'].into_iter()
        .max_by_key(|d| sample.iter().map(|l| split_line(l, *d).len()).max().unwrap_or(0))
        .unwrap_or(';');
    let split: Vec<Vec<String>> = lines.iter().map(|l| split_line(l, delim)).collect();
    let width = split.iter().map(Vec::len).max().unwrap_or(0);
    let Some(start) = split.iter().position(|r| r.len() == width && r.iter().filter(|c| !c.is_empty()).count() >= 2) else {
        return Table::default();
    };
    let header = split[start].clone();
    let rows = split[start + 1..].iter().filter(|r| r.len() >= 2 && r.iter().any(|c| !c.is_empty())).cloned().collect();
    Table { header, rows }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mapping {
    pub date: Option<usize>,
    /// One signed amount column, or separate debit and credit columns.
    pub amount: Option<usize>,
    pub debit: Option<usize>,
    pub credit: Option<usize>,
    pub currency: Option<usize>,
    pub description: Option<usize>,
    pub counterparty: Option<usize>,
}

fn find(header: &[String], words: &[&str]) -> Option<usize> {
    let h: Vec<String> = header.iter().map(|x| fold(x)).collect();
    words.iter().find_map(|w| h.iter().position(|x| x.contains(w)))
}

/// Guesses the mapping from Polish and English column names. The user can change every choice.
pub fn guess(header: &[String]) -> Mapping {
    Mapping {
        date: find(header, &["data operacji", "data transakcji", "data ksiegowania", "data waluty", "started date", "completed date", "date", "data"]),
        amount: find(header, &["kwota operacji", "kwota transakcji", "kwota", "amount", "wartosc"]),
        debit: find(header, &["obciazenia", "wydatki", "debit", "winien"]),
        credit: find(header, &["uznania", "wplywy", "credit", "ma "]),
        currency: find(header, &["waluta", "currency"]),
        description: find(header, &["tytul", "opis operacji", "opis", "description", "szczegoly", "title"]),
        counterparty: find(header, &["nadawca odbiorca", "odbiorca", "nadawca", "kontrahent", "counterparty", "payee", "merchant"]),
    }
}

/// Accepts the date formats banks use, with or without a time.
pub fn parse_date(s: &str) -> Option<Date> {
    let d = s.trim().split([' ', 'T']).next()?;
    let p: Vec<&str> = d.split(['-', '.', '/']).collect();
    if p.len() != 3 {
        return None;
    }
    let n = |x: &str| x.parse::<u32>().ok();
    if p[0].len() == 4 {
        Date::new(p[0].parse().ok()?, n(p[1])?, n(p[2])?)
    } else if p[2].len() == 4 {
        Date::new(p[2].parse().ok()?, n(p[1])?, n(p[0])?)
    } else {
        None
    }
}

/// A signed amount: "-1 234,56", "1,234.56", "−12,30 PLN", "(12.30)".
pub fn parse_signed(s: &str) -> Option<i64> {
    let mut t: String = s.trim().chars().filter(|c| !matches!(c, ' ' | '\u{a0}' | '\u{202f}' | '\'')).collect();
    // Only a three-letter currency code may follow the number.
    let letters = t.chars().rev().take_while(|c| c.is_ascii_alphabetic()).count();
    if letters == 3 {
        t.truncate(t.len() - 3);
    } else if letters > 0 {
        return None;
    }
    let mut neg = false;
    if let Some(x) = t.strip_prefix(['-', '−']) {
        neg = true;
        t = x.to_string();
    } else if let Some(x) = t.strip_prefix('+') {
        t = x.to_string();
    } else if t.starts_with('(') && t.ends_with(')') {
        neg = true;
        t = t[1..t.len() - 1].to_string();
    }
    // The last separator is the decimal one when two digits or fewer follow it.
    let last = t.rfind([',', '.']);
    let (whole, frac) = match last {
        Some(i) if t.len() - i - 1 <= 2 => (&t[..i], Some(&t[i + 1..])),
        _ => (t.as_str(), None),
    };
    // Thousands groups after the first must have exactly three digits, so "1,2,3" is not a number.
    let groups: Vec<&str> = whole.split([',', '.']).collect();
    if groups.len() > 1 && (groups[0].is_empty() || groups[1..].iter().any(|g| g.len() != 3)) {
        return None;
    }
    let clean = match frac {
        Some(f) => format!("{},{f}", groups.concat()),
        None => groups.concat(),
    };
    let v = parse_amount(&clean).ok()?;
    Some(if neg { -v } else { v })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub line: usize,
    pub date: Date,
    /// Negative for money going out.
    pub amount: i64,
    pub currency: String,
    pub description: String,
    pub counterparty: String,
    /// How many identical operations came earlier in this file, so two equal coffees on one day stay two.
    pub ordinal: usize,
    /// What the bank booked in złoty, when `currency` is another one. Without it the NBP rate is used.
    pub pln: Option<i64>,
}

fn cell(r: &[String], i: Option<usize>) -> &str {
    i.and_then(|i| r.get(i)).map(String::as_str).unwrap_or("")
}

/// Rows the mapping can read, and the line numbers of rows it cannot, with a reason.
pub fn rows(t: &Table, m: &Mapping) -> (Vec<Row>, Vec<(usize, String)>) {
    let (mut ok, mut bad) = (Vec::new(), Vec::new());
    for (i, r) in t.rows.iter().enumerate() {
        let line = i + 1;
        let Some(date) = parse_date(cell(r, m.date)) else {
            bad.push((line, "no valid date".to_string()));
            continue;
        };
        let amount = if m.amount.is_some() {
            parse_signed(cell(r, m.amount))
        } else {
            let (d, c) = (parse_signed(cell(r, m.debit)), parse_signed(cell(r, m.credit)));
            match (d, c) {
                (Some(d), _) if d != 0 => Some(-d.abs()),
                (_, Some(c)) if c != 0 => Some(c.abs()),
                (Some(0), _) | (_, Some(0)) => Some(0),
                _ => None,
            }
        };
        let Some(amount) = amount else {
            bad.push((line, "no valid amount".to_string()));
            continue;
        };
        if amount == 0 {
            bad.push((line, "zero amount".to_string()));
            continue;
        }
        let currency = cell(r, m.currency).trim().to_uppercase();
        let currency = if currency.len() == 3 && currency.chars().all(|c| c.is_ascii_uppercase()) { currency } else { "PLN".into() };
        let description = cell(r, m.description).to_string();
        let counterparty = cell(r, m.counterparty).to_string();
        let ordinal = ok.iter().filter(|x: &&Row| x.date == date && x.amount == amount && fold(&x.description) == fold(&description)).count();
        ok.push(Row { line, date, amount, currency, description, counterparty, ordinal, pln: None });
    }
    (ok, bad)
}

/// What makes an operation the same across imports: re-importing a statement, or an overlapping one,
/// gives the same key, and so the same record id.
pub fn import_key(account: Uuid, r: &Row) -> String {
    format!("{account}|{}|{}|{}|{}|{}", r.date.iso(), r.amount, r.currency, fold(&r.description), r.ordinal)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MBANK_LIKE: &str = "#Klient;\n#Jan Testowy;\n\n#Data operacji;#Opis operacji;#Rachunek;#Kategoria;#Kwota;\n\
2026-10-30;\"ŻABKA Z1234, Warszawa\";Konto;Żywność;-12,49 PLN;\n\
2026-10-30;\"ŻABKA Z1234, Warszawa\";Konto;Żywność;-12,49 PLN;\n\
2026-10-29;\"PRZELEW \"\"WYNAGRODZENIE\"\"\";Konto;Wpływy;5 000,00 PLN;\n";

    #[test]
    fn reads_a_csv_with_a_preamble_quotes_and_signs() {
        let t = parse_csv(MBANK_LIKE);
        assert_eq!(t.header[0], "#Data operacji");
        assert_eq!(t.rows.len(), 3);
        let m = Mapping { date: Some(0), description: Some(1), amount: Some(4), ..Default::default() };
        let (r, bad) = rows(&t, &m);
        assert!(bad.is_empty(), "{bad:?}");
        assert_eq!(r[0].amount, -1249);
        assert_eq!(r[2].amount, 500000);
        assert_eq!(r[2].description, "PRZELEW \"WYNAGRODZENIE\"");
        assert_eq!((r[0].ordinal, r[1].ordinal), (0, 1), "two equal purchases stay two");
    }

    #[test]
    fn guesses_polish_and_english_columns() {
        let h = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let m = guess(&h(&["#Data operacji", "#Opis operacji", "#Rachunek", "#Kategoria", "#Kwota"]));
        assert_eq!((m.date, m.description, m.amount), (Some(0), Some(1), Some(4)));
        let m = guess(&h(&["Type", "Product", "Started Date", "Completed Date", "Description", "Amount", "Fee", "Currency"]));
        assert_eq!((m.date, m.description, m.amount, m.currency), (Some(2), Some(4), Some(5), Some(7)));
        let m = guess(&h(&["Data", "Tytuł", "Obciążenia", "Uznania"]));
        assert_eq!((m.debit, m.credit, m.amount), (Some(2), Some(3), None));
    }

    #[test]
    fn debit_and_credit_columns_give_the_sign() {
        let t = parse_csv("Data;Tytuł;Obciążenia;Uznania\n01.10.2026;Kawa;12,50;\n02.10.2026;Zwrot;;30,00\n03.10.2026;Nic;;\n");
        let (r, bad) = rows(&t, &guess(&t.header));
        assert_eq!(r.iter().map(|x| x.amount).collect::<Vec<_>>(), vec![-1250, 3000]);
        assert_eq!(bad, vec![(3, "no valid amount".to_string())]);
    }

    #[test]
    fn windows_1250_is_decoded() {
        let bytes: Vec<u8> = b"Kwota;Opis\n-1,00;\xafabka\n".to_vec();
        assert!(decode(&bytes).contains("Żabka"));
        assert_eq!(decode("\u{feff}Żabka".as_bytes()), "Żabka");
    }

    #[test]
    fn amount_formats() {
        for (s, v) in [("-1 234,56", -123456), ("1,234.56", 123456), ("−12,30 PLN", -1230), ("(12.30)", -1230), ("+5", 500), ("1.234,5", 123450), ("12'345.00", 1234500)] {
            assert_eq!(parse_signed(s), Some(v), "{s}");
        }
        for s in ["", "abc", "1,2,3,4x", "1,2,3", "12x", "1.23.456,00"] {
            assert_eq!(parse_signed(s), None, "{s}");
        }
    }

    #[test]
    fn date_formats() {
        for (s, want) in [("2026-10-31", (2026, 10, 31)), ("31.10.2026", (2026, 10, 31)), ("31-10-2026 14:02:11", (2026, 10, 31)), ("2026/10/31", (2026, 10, 31)), ("2026-10-31T09:00:00Z", (2026, 10, 31))] {
            assert_eq!(parse_date(s), Date::new(want.0, want.1, want.2), "{s}");
        }
        assert_eq!(parse_date("31/02/2026"), None);
    }

    #[test]
    fn the_same_statement_gives_the_same_keys() {
        let acct = Uuid::from_u128(7);
        let t = parse_csv(MBANK_LIKE);
        let m = Mapping { date: Some(0), description: Some(1), amount: Some(4), ..Default::default() };
        let keys = |t: &Table| rows(t, &m).0.iter().map(|r| import_key(acct, r)).collect::<Vec<_>>();
        let a = keys(&t);
        assert_eq!(a, keys(&parse_csv(MBANK_LIKE)));
        assert_eq!(a.len(), 3);
        assert_ne!(a[0], a[1], "two equal purchases on one day are two operations");
        assert_ne!(import_key(Uuid::from_u128(8), &rows(&t, &m).0[0]), a[0], "another account is another operation");
    }

    #[test]
    fn garbage_does_not_panic() {
        for s in ["", "\n\n", "a", "\"unclosed;quote\nnext", ";;;\n;;;"] {
            let t = parse_csv(s);
            let _ = rows(&t, &guess(&t.header));
        }
    }
}
