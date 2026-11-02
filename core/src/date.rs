//! Calendar dates without a time zone. A purchase date is what the receipt says.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

pub fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(y) => 29,
        2 => 28,
        _ => 31,
    }
}

impl Date {
    pub fn new(year: i32, month: u32, day: u32) -> Option<Self> {
        ((1..=9999).contains(&year) && (1..=12).contains(&month) && (1..=days_in_month(year, month)).contains(&day))
            .then_some(Self { year, month, day })
    }

    /// "2026-10-31", as produced by `<input type="date">`.
    pub fn parse_iso(s: &str) -> Option<Self> {
        let mut p = s.split('-');
        let (y, m, d) = (p.next()?, p.next()?, p.next()?);
        if p.next().is_some() || y.len() != 4 || m.len() != 2 || d.len() != 2 {
            return None;
        }
        Self::new(y.parse().ok()?, m.parse().ok()?, d.parse().ok()?)
    }

    pub fn iso(self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    /// "31.10.2026"
    pub fn display(self) -> String {
        format!("{:02}.{:02}.{:04}", self.day, self.month, self.year)
    }

    pub fn month_of(self) -> Month {
        Month { year: self.year, month: self.month }
    }
}

impl Serialize for Date {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.iso())
    }
}

impl<'de> Deserialize<'de> for Date {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Date::parse_iso(&s).ok_or_else(|| serde::de::Error::custom("invalid date"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Month {
    pub year: i32,
    pub month: u32,
}

impl Month {
    pub fn offset(self, n: i32) -> Self {
        let total = self.year * 12 + self.month as i32 - 1 + n;
        Self { year: total.div_euclid(12), month: total.rem_euclid(12) as u32 + 1 }
    }

    pub fn first(self) -> Date {
        Date { year: self.year, month: self.month, day: 1 }
    }

    pub fn last(self) -> Date {
        Date { year: self.year, month: self.month, day: days_in_month(self.year, self.month) }
    }

    /// "10.2026"
    pub fn display(self) -> String {
        format!("{:02}.{:04}", self.month, self.year)
    }

    pub fn short_name(self) -> &'static str {
        ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"][self.month as usize - 1]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_calendar_dates() {
        assert!(Date::new(2024, 2, 29).is_some());
        assert!(Date::new(2026, 2, 29).is_none());
        assert!(Date::new(1900, 2, 29).is_none());
        assert!(Date::new(2000, 2, 29).is_some());
        assert!(Date::new(2026, 13, 1).is_none());
        assert!(Date::new(2026, 4, 31).is_none());
        assert!(Date::new(2026, 1, 0).is_none());
    }

    #[test]
    fn iso_round_trip_and_display() {
        let d = Date::parse_iso("2026-10-31").unwrap();
        assert_eq!(d.display(), "31.10.2026");
        assert_eq!(d.iso(), "2026-10-31");
        for bad in ["", "2026-1-1", "2026-13-01", "31.10.2026", "2026-10-31-1", "abcd-ef-gh"] {
            assert!(Date::parse_iso(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn dates_sort_chronologically() {
        assert!(Date::new(2025, 12, 31) < Date::new(2026, 1, 1));
    }

    #[test]
    fn month_arithmetic_crosses_years() {
        let m = Month { year: 2026, month: 1 };
        assert_eq!(m.offset(-1), Month { year: 2025, month: 12 });
        assert_eq!(m.offset(-13), Month { year: 2024, month: 12 });
        assert_eq!(m.offset(12), Month { year: 2027, month: 1 });
        assert_eq!(Month { year: 2024, month: 2 }.last().day, 29);
    }
}
