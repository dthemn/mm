//! Exact money and quantity arithmetic. There are no floats anywhere in this module.
//! Amounts are integer minor units (grosze for PLN), quantities are integer thousandths.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MoneyError {
    #[error("not a valid number")]
    Invalid,
    #[error("too many decimal places")]
    TooPrecise,
    #[error("number is out of range")]
    Overflow,
}

/// Splits "1 234,56" into digits, accepting a comma or a dot and spaces between thousands.
fn parse_fixed(s: &str, places: u32) -> Result<i64, MoneyError> {
    let clean: String = s.trim().chars().filter(|c| !matches!(c, ' ' | '\u{a0}' | '\u{202f}')).collect();
    let (whole, frac) = match clean.split_once([',', '.']) {
        Some((w, f)) => (w, f),
        None => (clean.as_str(), ""),
    };
    if whole.is_empty() && frac.is_empty() {
        return Err(MoneyError::Invalid);
    }
    if !whole.chars().all(|c| c.is_ascii_digit()) || !frac.chars().all(|c| c.is_ascii_digit()) {
        return Err(MoneyError::Invalid);
    }
    if frac.len() > places as usize {
        return Err(MoneyError::TooPrecise);
    }
    let scale = 10i64.pow(places);
    let w: i64 = if whole.is_empty() { 0 } else { whole.parse().map_err(|_| MoneyError::Overflow)? };
    let f: i64 = if frac.is_empty() { 0 } else { frac.parse().map_err(|_| MoneyError::Overflow)? };
    let f = f * 10i64.pow(places - frac.len() as u32);
    w.checked_mul(scale).and_then(|v| v.checked_add(f)).ok_or(MoneyError::Overflow)
}

/// Parses a user-entered non-negative amount into minor units.
pub fn parse_amount(s: &str) -> Result<i64, MoneyError> {
    parse_fixed(s, 2)
}

/// "1234,56" with no thousands separator, for editing in an input field.
pub fn amount_input(minor: i64) -> String {
    let sign = if minor < 0 { "-" } else { "" };
    let a = minor.unsigned_abs();
    format!("{sign}{},{:02}", a / 100, a % 100)
}

/// "1 234,56", with a non-breaking thousands separator.
pub fn format_amount(minor: i64) -> String {
    let sign = if minor < 0 { "-" } else { "" };
    let a = minor.unsigned_abs();
    let whole = (a / 100).to_string();
    let mut grouped = String::new();
    for (i, c) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i).is_multiple_of(3) {
            grouped.push('\u{a0}');
        }
        grouped.push(c);
    }
    format!("{sign}{grouped},{:02}", a % 100)
}

/// "1 234,56 zł". Other currencies keep their ISO code.
pub fn format_money(minor: i64, currency: &str) -> String {
    let unit = if currency == "PLN" { "zł" } else { currency };
    format!("{}\u{a0}{unit}", format_amount(minor))
}

/// A quantity with three decimal places, such as 0.532 kg.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Qty(i64);

impl Qty {
    pub const ONE: Qty = Qty(1000);

    pub fn parse(s: &str) -> Result<Self, MoneyError> {
        parse_fixed(s, 3).map(Self)
    }

    pub fn thousandths(self) -> i64 {
        self.0
    }

    pub fn from_thousandths(n: i64) -> Self {
        Self(n)
    }

    /// The canonical form uses a dot and drops trailing zeros.
    fn canonical(self, sep: char) -> String {
        let (w, f) = (self.0 / 1000, self.0 % 1000);
        if f == 0 {
            return w.to_string();
        }
        let frac = format!("{f:03}");
        format!("{w}{sep}{}", frac.trim_end_matches('0'))
    }

    /// Polish form with a comma: "0,532".
    pub fn display(self) -> String {
        self.canonical(',')
    }
}

impl Serialize for Qty {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.canonical('.'))
    }
}

impl<'de> Deserialize<'de> for Qty {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Qty::parse(&String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

/// quantity × unit price, rounded half up to a whole grosz.
pub fn line_value(qty: Qty, unit_price: i64) -> Option<i64> {
    if unit_price < 0 {
        return None;
    }
    let exact = qty.0 as i128 * unit_price as i128;
    i64::try_from((exact + 500) / 1000).ok()
}

/// "4,3012"
pub fn format_rate(e4: i64) -> String {
    format!("{},{:04}", e4 / 10000, e4 % 10000)
}

/// An exchange rate such as "4.3012" as an integer number of ten-thousandths.
pub fn parse_rate(s: &str) -> Option<i64> {
    parse_fixed(s, 4).ok().filter(|r| *r > 0)
}

/// Converts `minor` units at an exchange rate given in ten-thousandths (NBP publishes four decimals),
/// rounding half up to a whole grosz.
pub fn convert(minor: i64, rate_e4: i64) -> Option<i64> {
    if minor < 0 || rate_e4 <= 0 {
        return None;
    }
    i64::try_from((minor as i128 * rate_e4 as i128 + 5000) / 10000).ok()
}

/// Splits `total` in proportion to `weights` so the parts add up to exactly `total`.
/// Leftover grosze go to the largest remainders, and ties go to the earliest part.
pub fn allocate(total: i64, weights: &[i64]) -> Vec<i64> {
    let sum: i128 = weights.iter().map(|w| *w as i128).sum();
    if weights.is_empty() {
        return vec![];
    }
    if sum <= 0 {
        let mut out = vec![0; weights.len()];
        out[0] = total;
        return out;
    }
    let mut out = Vec::with_capacity(weights.len());
    let mut rest: Vec<(i128, usize)> = Vec::with_capacity(weights.len());
    for (i, w) in weights.iter().enumerate() {
        let exact = total as i128 * *w as i128;
        out.push((exact / sum) as i64);
        rest.push((exact % sum, i));
    }
    let mut left = total - out.iter().sum::<i64>();
    rest.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    for (_, i) in rest {
        if left == 0 {
            break;
        }
        out[i] += 1;
        left -= 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_amounts_exactly() {
        for (s, want) in [
            ("12,34", 1234), ("12.34", 1234), ("12", 1200), ("12,3", 1230), ("0,01", 1),
            (",5", 50), ("1 234,56", 123456), ("1\u{a0}234,56", 123456), (" 7 ", 700),
            // These values turn into 0.07 * 100 = 7.000000000000001 and the like in floats.
            ("0,07", 7), ("1,10", 110), ("19,99", 1999), ("4,35", 435), ("1,15", 115),
        ] {
            assert_eq!(parse_amount(s), Ok(want), "{s}");
        }
    }

    #[test]
    fn rejects_bad_amounts() {
        for s in ["", " ", ",", "abc", "-5", "1,2,3", "1,234,56", "+1", "1e3", "١٢"] {
            assert!(parse_amount(s).is_err(), "{s}");
        }
        assert_eq!(parse_amount("1,234"), Err(MoneyError::TooPrecise));
        assert_eq!(parse_amount("99999999999999999999"), Err(MoneyError::Overflow));
        assert_eq!(parse_amount("92233720368547758,08"), Err(MoneyError::Overflow));
    }

    #[test]
    fn formats_like_poland() {
        assert_eq!(format_money(123456, "PLN"), "1\u{a0}234,56\u{a0}zł");
        assert_eq!(format_amount(5), "0,05");
        assert_eq!(format_amount(100), "1,00");
        assert_eq!(format_amount(100000), "1\u{a0}000,00");
        assert_eq!(format_amount(123456789), "1\u{a0}234\u{a0}567,89");
        assert_eq!(format_amount(-123456), "-1\u{a0}234,56");
        assert_eq!(format_money(1050, "EUR"), "10,50\u{a0}EUR");
    }

    #[test]
    fn format_handles_the_extremes_without_panic() {
        assert!(format_amount(i64::MAX).ends_with(",07"));
        assert!(format_amount(i64::MIN).starts_with('-'));
    }

    #[test]
    fn amount_input_round_trips() {
        for v in [0, 1, 99, 100, 123456] {
            assert_eq!(parse_amount(&amount_input(v)), Ok(v));
        }
    }

    #[test]
    fn quantities_are_decimal() {
        assert_eq!(Qty::parse("0,532").unwrap().thousandths(), 532);
        assert_eq!(Qty::parse("1.5").unwrap().display(), "1,5");
        assert_eq!(Qty::parse("3").unwrap().display(), "3");
        assert_eq!(Qty::parse("0,050").unwrap().display(), "0,05");
        assert!(Qty::parse("0,5321").is_err());
    }

    #[test]
    fn quantity_survives_json_as_a_decimal_string() {
        let q = Qty::parse("0,532").unwrap();
        let j = serde_json::to_string(&q).unwrap();
        assert_eq!(j, "\"0.532\"");
        assert_eq!(serde_json::from_str::<Qty>(&j).unwrap(), q);
    }

    #[test]
    fn line_value_rounds_half_up() {
        let q = |s| Qty::parse(s).unwrap();
        assert_eq!(line_value(q("1,614"), 2183), Some(3523)); // 3523.362
        assert_eq!(line_value(q("2,202"), 390), Some(859)); // 858.78
        assert_eq!(line_value(q("0,5"), 1), Some(1)); // 0.5 rounds up
        assert_eq!(line_value(q("0,499"), 1), Some(0));
        assert_eq!(line_value(q("3"), 800), Some(2400));
        assert_eq!(line_value(q("1"), -1), None);
        assert_eq!(line_value(Qty(i64::MAX), i64::MAX), None);
    }

    #[test]
    fn rates_parse_without_floats() {
        assert_eq!(parse_rate("4.3012"), Some(43012));
        assert_eq!(parse_rate("0.0371"), Some(371));
        assert_eq!(parse_rate("4.30125"), None);
        assert_eq!(parse_rate("0"), None);
    }

    #[test]
    fn conversion_is_exact_and_rounds_half_up() {
        assert_eq!(convert(1000, 43012), Some(4301)); // 10,00 EUR at 4.3012 = 43,012 PLN
        assert_eq!(convert(1050, 43050), Some(4520)); // 45,2025
        assert_eq!(convert(1, 5000), Some(1)); // 0,5 grosz rounds up
        assert_eq!(convert(1, 4999), Some(0));
        assert_eq!(convert(-1, 40000), None);
        assert_eq!(convert(i64::MAX, i64::MAX), None);
    }

    #[test]
    fn allocation_always_adds_up() {
        for (total, w) in [
            (100i64, vec![1i64, 1, 1]),
            (1, vec![1, 1, 1]),
            (0, vec![5, 5]),
            (99_999, vec![1, 2, 3, 4, 5, 6, 7]),
            (10, vec![0, 1]),
            (i64::MAX / 4, vec![i64::MAX / 4, 3, 1]),
        ] {
            let got = allocate(total, &w);
            assert_eq!(got.len(), w.len());
            assert_eq!(got.iter().map(|x| *x as i128).sum::<i128>(), total as i128, "{w:?}");
            assert!(got.iter().all(|x| *x >= 0));
        }
    }

    #[test]
    fn allocation_gives_leftovers_to_the_largest_remainder() {
        assert_eq!(allocate(100, &[1, 1, 1]), vec![34, 33, 33]);
        assert_eq!(allocate(10, &[1, 3]), vec![3, 7]); // 2.5 and 7.5, a tie goes to the first
    }

    #[test]
    fn allocation_with_zero_weights_keeps_the_money() {
        assert_eq!(allocate(50, &[0, 0]), vec![50, 0]);
        assert!(allocate(50, &[]).is_empty());
    }
}
