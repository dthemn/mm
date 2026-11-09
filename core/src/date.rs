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

    /// 0 is Monday. 1970-01-01, day 0, was a Thursday.
    pub fn weekday(self) -> u32 {
        (self.days() + 3).rem_euclid(7) as u32
    }

    /// "Wed, 4 November" for a day list. The year is added when it is not `this_year`.
    pub fn day_label(self, this_year: i32) -> String {
        let wd = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"][self.weekday() as usize];
        let month = self.month_of().name();
        if self.year == this_year {
            format!("{wd}, {} {month}", self.day)
        } else {
            format!("{wd}, {} {month} {}", self.day, self.year)
        }
    }

    pub fn month_of(self) -> Month {
        Month { year: self.year, month: self.month }
    }

    /// Days since 1970-01-01, for date differences.
    pub fn days(self) -> i64 {
        let (y, m, d) = (self.year as i64 - (self.month <= 2) as i64, self.month as i64, self.day as i64);
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146097 + doe - 719468
    }

    pub fn days_between(self, other: Date) -> i64 {
        (other.days() - self.days()).abs()
    }

    /// The day `n` days later (earlier for a negative `n`). Stays within years 1 to 9999.
    pub fn add_days(self, n: i64) -> Date {
        let z = (self.days() + n).clamp(Date { year: 1, month: 1, day: 1 }.days(), Date { year: 9999, month: 12, day: 31 }.days()) + 719468;
        let era = z.div_euclid(146097);
        let doe = z - era * 146097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        let year = (yoe + era * 400 + (month <= 2) as i64) as i32;
        Date { year, month, day }
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

    /// "November 2026"
    pub fn long(self) -> String {
        format!("{} {}", self.name(), self.year)
    }

    pub fn name(self) -> &'static str {
        ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November",
            "December"][self.month as usize - 1]
    }

    pub fn short_name(self) -> &'static str {
        ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"][self.month as usize - 1]
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn add_days_crosses_months_years_and_leap_days() {
        let d = |y, m, n| Date::new(y, m, n).unwrap();
        assert_eq!(d(2026, 11, 9).add_days(1), d(2026, 11, 10));
        assert_eq!(d(2026, 11, 30).add_days(1), d(2026, 12, 1));
        assert_eq!(d(2026, 12, 31).add_days(1), d(2027, 1, 1));
        assert_eq!(d(2027, 1, 1).add_days(-1), d(2026, 12, 31));
        assert_eq!(d(2028, 2, 28).add_days(1), d(2028, 2, 29));
        assert_eq!(d(2028, 3, 1).add_days(-1), d(2028, 2, 29));
        assert_eq!(d(2026, 3, 1).add_days(-1), d(2026, 2, 28));
        assert_eq!(d(2026, 11, 9).add_days(0), d(2026, 11, 9));
        // Every day of four years, one step at a time, matches the day count.
        let mut x = d(2024, 1, 1);
        for _ in 0..1461 {
            let next = x.add_days(1);
            assert_eq!(next.days(), x.days() + 1);
            assert!(Date::new(next.year, next.month, next.day).is_some());
            x = next;
        }
        assert_eq!(d(1, 1, 1).add_days(-5), d(1, 1, 1), "clamped at the first day");
    }

    use super::*;

    #[test]
    fn weekdays_and_day_labels() {
        let d = |y, m, n| Date::new(y, m, n).unwrap();
        assert_eq!(d(1970, 1, 1).weekday(), 3);
        assert_eq!(d(2026, 11, 4).day_label(2026), "Wed, 4 November");
        assert_eq!(d(2024, 2, 29).day_label(2026), "Thu, 29 February 2024");
        assert_eq!(d(1969, 12, 29).weekday(), 0);
        assert_eq!(d(2026, 11, 4).month_of().long(), "November 2026");
    }

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
    fn day_counts_cross_months_years_and_leap_days() {
        let d = |y, m, day| Date::new(y, m, day).unwrap();
        assert_eq!(d(1970, 1, 1).days(), 0);
        assert_eq!(d(2026, 3, 1).days() - d(2026, 2, 28).days(), 1);
        assert_eq!(d(2024, 3, 1).days() - d(2024, 2, 28).days(), 2);
        assert_eq!(d(2026, 1, 2).days_between(d(2025, 12, 31)), 2);
        assert_eq!(d(2027, 1, 1).days() - d(2026, 1, 1).days(), 365);
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
