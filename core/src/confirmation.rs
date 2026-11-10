//! Transfer confirmations ("Potwierdzenie wykonania operacji") as PDFs: one operation per file, written as
//! label and value pairs. Checked on Bank Millennium's; the labels are the usual Polish ones, so other banks
//! with the same layout may read too, but count as supported only with a test on a real file.

use crate::categorize::fold;
use crate::date::Date;
use crate::import::{Row, parse_date, parse_signed};
use crate::pdf::Line;

/// A Polish transfer title is up to 4 lines of 35 characters, and the bank prints them as it stores them:
/// a full line breaks wherever the 35th character falls, even inside a word or a number.
const TITLE_LINE: usize = 35;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Confirmation {
    /// The bank, when the document names one this module knows.
    pub bank: Option<&'static str>,
    pub kind: String,
    pub date: Date,
    /// Negative for money going out, in `currency`.
    pub amount: i64,
    pub currency: String,
    /// What was booked in złoty, when the operation was in another currency.
    pub pln: Option<i64>,
    /// The recipient for money going out, the sender for money coming in.
    pub counterparty: String,
    pub title: String,
}

#[derive(Default)]
struct Fields {
    kind: Option<String>,
    booked: Option<String>,
    value_date: Option<String>,
    posted: Option<String>,
    amount: Option<String>,
    sender: Option<String>,
    recipient: Option<String>,
    title: Vec<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum Field {
    Kind,
    Booked,
    ValueDate,
    Posted,
    Amount,
    Sender,
    Recipient,
    Title,
    Other,
}

fn field(label: &str) -> Option<Field> {
    let l = fold(label.trim_end_matches(':').trim());
    Some(match l.as_str() {
        "typ operacji" | "rodzaj operacji" | "typ transakcji" => Field::Kind,
        "data ksiegowania" | "data wykonania" | "data realizacji" => Field::Booked,
        "data waluty" | "data operacji" | "data transakcji" => Field::ValueDate,
        "kwota zaksiegowana" | "kwota obciazenia" | "kwota uznania" => Field::Posted,
        "kwota transakcji" | "kwota" | "kwota operacji" | "kwota przelewu" => Field::Amount,
        "zleceniodawca" | "nadawca" => Field::Sender,
        "odbiorca" | "beneficjent" => Field::Recipient,
        "tytul" | "tytulem" | "tytul operacji" | "tytul przelewu" => Field::Title,
        "z rachunku" | "na rachunek" | "rachunek nadawcy" | "rachunek odbiorcy" | "sci status" | "status" => Field::Other,
        _ => return None,
    })
}

/// The confirmation on these pages, if they hold one.
pub fn read(pages: &[Vec<Line>]) -> Option<Confirmation> {
    let lines: Vec<&Line> = pages.iter().flatten().collect();
    let all = fold(&lines.iter().map(|l| l.text()).collect::<Vec<_>>().join(" "));
    if !all.contains("potwierdzenie") {
        return None;
    }
    let bank = all.contains("bank millennium").then_some("Bank Millennium");
    let mut f = Fields::default();
    // The value column: continuation lines start there and have no label.
    let mut value_x: Option<f32> = None;
    let mut last: Option<Field> = None;
    for l in &lines {
        let first = &l.cells[0];
        match (field(&first.text), l.cells.get(1)) {
            (Some(which), Some(v)) => {
                value_x = Some(v.x);
                let value = l.cells[1..].iter().map(|c| c.text.as_str()).collect::<Vec<_>>().join(" ");
                let slot = match which {
                    Field::Kind => &mut f.kind,
                    Field::Booked => &mut f.booked,
                    Field::ValueDate => &mut f.value_date,
                    Field::Posted => &mut f.posted,
                    Field::Amount => &mut f.amount,
                    Field::Sender => &mut f.sender,
                    Field::Recipient => &mut f.recipient,
                    Field::Title => {
                        f.title = vec![value];
                        last = Some(which);
                        continue;
                    }
                    Field::Other => {
                        last = Some(which);
                        continue;
                    }
                };
                slot.get_or_insert(value);
                last = Some(which);
            }
            _ if value_x.is_some_and(|x| (first.x - x).abs() < 2.0) => {
                // A wrapped value. Only the title goes on: the sender's further lines are an address.
                if last == Some(Field::Title) && f.title.len() < 4 {
                    f.title.push(l.text());
                }
            }
            _ => last = None,
        }
    }
    let date = f.booked.as_deref().and_then(parse_date).or_else(|| f.value_date.as_deref().and_then(parse_date))?;
    let kind = f.kind.unwrap_or_default();
    let k = fold(&kind);
    let sign_of_kind = if k.contains("wychodz") || k.contains("obciaz") {
        Some(-1)
    } else if k.contains("przychodz") || k.contains("uznani") || k.contains("wplyw") {
        Some(1)
    } else {
        None
    };
    let posted = f.posted.as_deref().and_then(|s| Some((parse_signed(s)?, currency(s))));
    let original = f.amount.as_deref().and_then(|s| Some((parse_signed(s)?, currency(s))));
    let sign = posted.as_ref().filter(|p| p.0 != 0).map(|p| p.0.signum()).or(sign_of_kind)?;
    let (amount, cur, pln) = match (original, posted) {
        (Some((a, c)), Some((p, pc))) if c != "PLN" && pc == "PLN" => (a.abs(), c, Some(p.abs())),
        (_, Some((p, pc))) => (p.abs(), pc, None),
        (Some((a, c)), None) => (a.abs(), c, None),
        (None, None) => return None,
    };
    if amount == 0 {
        return None;
    }
    let counterparty = if sign < 0 { f.recipient } else { f.sender }.unwrap_or_default();
    Some(Confirmation { bank, kind, date, amount: sign * amount, currency: cur, pln, counterparty, title: join_title(&f.title) })
}

fn currency(s: &str) -> String {
    s.split_whitespace().last()
        .map(|w| w.to_uppercase())
        .filter(|w| w.len() == 3 && w.chars().all(|c| c.is_ascii_uppercase()))
        .unwrap_or_else(|| "PLN".into())
}

/// Lines of a title back into one: a full 35-character line continues the next one without a space.
pub fn join_title(lines: &[String]) -> String {
    let mut out = String::new();
    for (i, l) in lines.iter().enumerate() {
        if i > 0 && lines[i - 1].chars().count() < TITLE_LINE {
            out.push(' ');
        }
        out.push_str(l);
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl Confirmation {
    pub fn row(&self, line: usize) -> Row {
        Row {
            line, date: self.date, amount: self.amount, currency: self.currency.clone(),
            description: self.title.clone(), counterparty: self.counterparty.clone(), ordinal: 0, pln: self.pln,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::{read as read_pdf, tests::make_pdf};

    /// The layout of a Bank Millennium confirmation, with made-up people and accounts.
    fn millennium(kind: &str, amount: &str, posted: &str, title: &[&str]) -> Vec<u8> {
        millennium_locked(kind, amount, posted, title, None)
    }

    fn millennium_locked(kind: &str, amount: &str, posted: &str, title: &[&str], password: Option<&str>) -> Vec<u8> {
        let mut cells: Vec<(f32, f32, String)> = vec![
            (59.5, 118.0, "ul. Testowa 1".into()),
            (59.5, 156.0, "Potwierdzenie wykonania operacji".into()),
        ];
        let mut y = 192.0;
        let mut row = |cells: &mut Vec<(f32, f32, String)>, label: &str, value: &str| {
            if !label.is_empty() {
                cells.push((62.1, y, label.into()));
            }
            cells.push((310.2, y, value.into()));
            y += 18.8;
        };
        row(&mut cells, "Typ operacji", kind);
        row(&mut cells, "Data księgowania", "2026-11-10");
        row(&mut cells, "Data waluty", "2026-11-09");
        row(&mut cells, "Z rachunku", "00116022020000000000000001");
        row(&mut cells, "Zleceniodawca", "JAN TESTOWY");
        row(&mut cells, "", "UL. PRZYKŁADOWA 1");
        row(&mut cells, "", "00-001 WARSZAWA");
        row(&mut cells, "Na rachunek", "00116022020000000000000002");
        row(&mut cells, "Odbiorca", "ANNA PRZYKŁAD");
        row(&mut cells, "Kwota transakcji", amount);
        row(&mut cells, "Kwota zaksięgowana", posted);
        for (i, t) in title.iter().enumerate() {
            row(&mut cells, if i == 0 { "Tytuł" } else { "" }, t);
        }
        row(&mut cells, "SCI status", "OK");
        cells.push((59.5, 745.0, "Data wystawienia dokumentu :".into()));
        cells.push((283.9, 745.0, "2026-11-10".into()));
        cells.push((59.5, 780.0, "Bank Millennium S.A. z siedzibą w Warszawie".into()));
        let refs: Vec<(f32, f32, &str)> = cells.iter().map(|(x, y, t)| (*x, *y, t.as_str())).collect();
        crate::pdf::tests::make_pdf_locked(&refs, password)
    }

    fn sample() -> Vec<u8> {
        millennium("PRZELEW WEWNĘTRZNY WYCHODZĄCY", "1 400,00 PLN", "-1 400,00 PLN", &["ADAMEK - za listopad", "mieszkanie 1400 PLN"])
    }

    #[test]
    fn a_millennium_transfer_out() {
        let pdf = millennium("PRZELEW WEWNĘTRZNY WYCHODZĄCY", "692,60 PLN", "-692,60 PLN", &["ADAMEK - ZA listopad - czynsz 692,6", "0 pln"]);
        let c = read(&read_pdf(&pdf, None).unwrap()).expect("a confirmation");
        assert_eq!(c.bank, Some("Bank Millennium"));
        assert_eq!(c.date, Date::new(2026, 11, 10).unwrap(), "the booking date, not the value date");
        assert_eq!((c.amount, c.currency.as_str(), c.pln), (-69260, "PLN", None));
        assert_eq!(c.counterparty, "ANNA PRZYKŁAD", "the recipient, not the sender or an address line");
        assert_eq!(c.title, "ADAMEK - ZA listopad - czynsz 692,60 pln", "a full 35-character line continues without a space");
        let r = c.row(1);
        assert_eq!((r.amount, r.description.as_str()), (-69260, c.title.as_str()));
    }

    #[test]
    fn a_short_title_line_is_followed_by_a_space() {
        let c = read(&read_pdf(&sample(), None).unwrap()).unwrap();
        assert_eq!(c.title, "ADAMEK - za listopad mieszkanie 1400 PLN");
        assert_eq!(join_title(&["a".into(), "b".into()]), "a b");
    }

    #[test]
    fn money_coming_in_and_a_foreign_currency() {
        let pdf = millennium("PRZELEW PRZYCHODZĄCY", "100,00 EUR", "431,20 PLN", &["Zwrot"]);
        let c = read(&read_pdf(&pdf, None).unwrap()).unwrap();
        assert_eq!((c.amount, c.currency.as_str(), c.pln), (10000, "EUR", Some(43120)));
        assert_eq!(c.counterparty, "JAN TESTOWY", "the sender for money coming in");
    }

    #[test]
    fn other_documents_are_not_confirmations() {
        let pdf = make_pdf(&[(40.0, 40.0, "Faktura VAT"), (40.0, 60.0, "Kwota"), (200.0, 60.0, "10,00 PLN")]);
        assert_eq!(read(&read_pdf(&pdf, None).unwrap()), None);
        let pdf = make_pdf(&[(40.0, 40.0, "Potwierdzenie"), (40.0, 60.0, "Kwota"), (200.0, 60.0, "10,00 PLN")]);
        assert_eq!(read(&read_pdf(&pdf, None).unwrap()), None, "no date");
    }

    /// The made-up confirmations the browser tests upload. `UPDATE_FIXTURES=1` writes them again.
    #[test]
    fn e2e_fixtures() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../e2e/fixtures");
        let files = [
            ("confirmation-rent.pdf", millennium("PRZELEW WEWNĘTRZNY WYCHODZĄCY", "692,60 PLN", "-692,60 PLN", &["ADAMEK - ZA listopad - czynsz 692,6", "0 pln"])),
            ("confirmation-locked.pdf", millennium_locked("PRZELEW WEWNĘTRZNY WYCHODZĄCY", "91,24 PLN", "-91,24 PLN", &["ADAMEK - za listopad - GAZ 91,24 pl", "n"], Some("tajne"))),
        ];
        for (name, bytes) in files {
            let path = dir.join(name);
            if std::env::var("UPDATE_FIXTURES").is_ok() {
                std::fs::write(&path, &bytes).unwrap();
            }
            let saved = std::fs::read(&path).unwrap_or_else(|_| panic!("run with UPDATE_FIXTURES=1 to write {name}"));
            let read_one = |b: &[u8]| read(&read_pdf(b, Some("tajne")).or_else(|_| read_pdf(b, None)).unwrap());
            assert_eq!(read_one(&saved), read_one(&bytes), "{name} is out of date: run with UPDATE_FIXTURES=1");
        }
    }

    /// The owner's real confirmations, when they are on this machine (samples/ is never committed).
    #[test]
    fn real_millennium_confirmations() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../samples/statements/millennium");
        let Ok(files) = std::fs::read_dir(&dir) else { return };
        let mut n = 0;
        for f in files.flatten().filter(|f| f.path().extension().is_some_and(|e| e == "pdf")) {
            let pages = read_pdf(&std::fs::read(f.path()).unwrap(), None).unwrap();
            let c = read(&pages).unwrap_or_else(|| panic!("{:?} is not read", f.path()));
            assert_eq!(c.bank, Some("Bank Millennium"));
            assert!(c.amount < 0 && !c.counterparty.is_empty() && !c.title.is_empty() && !c.title.contains('\u{fffd}'));
            n += 1;
        }
        eprintln!("{n} real confirmations read");
    }
}
