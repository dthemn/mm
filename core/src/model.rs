//! Records of the finance model. All of them live inside the ciphertext.

use crate::date::Date;
use crate::money::{Qty, line_value};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const KIND_TRANSACTION: &str = "transaction";
pub const KIND_CATEGORY: &str = "category";
pub const KIND_ACCOUNT: &str = "account";
pub const KIND_MERCHANT: &str = "merchant";
pub const KIND_RULE: &str = "rule";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TxKind {
    #[default]
    Expense,
    Income,
    /// Between the user's own accounts: neither income nor expense.
    Transfer,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Payment {
    #[default]
    Card,
    Cash,
    Transfer,
    Other,
}

impl Payment {
    pub const ALL: [Payment; 4] = [Self::Card, Self::Cash, Self::Transfer, Self::Other];

    pub fn label(self) -> &'static str {
        match self {
            Self::Card => "Card",
            Self::Cash => "Cash",
            Self::Transfer => "Transfer",
            Self::Other => "Other",
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::Card => "card",
            Self::Cash => "cash",
            Self::Transfer => "transfer",
            Self::Other => "other",
        }
    }

    pub fn from_code(s: &str) -> Self {
        Self::ALL.into_iter().find(|p| p.code() == s).unwrap_or_default()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Receipt,
    Statement,
    Invoice,
    #[default]
    Manual,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Review {
    #[default]
    Reviewed,
    NeedsReview,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineItem {
    /// As printed on the receipt.
    pub raw_name: String,
    pub name: String,
    pub qty: Qty,
    pub unit: String,
    pub unit_price: i64,
    #[serde(default)]
    pub discount: i64,
    pub total: i64,
    /// Basis points: 2300 means 23%.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vat_rate: Option<u32>,
    pub category_id: Uuid,
}

impl LineItem {
    /// Value after the discount, or None if it does not fit or goes below zero.
    pub fn computed_total(&self) -> Option<i64> {
        let v = line_value(self.qty, self.unit_price)?.checked_sub(self.discount)?;
        (v >= 0 && self.discount >= 0).then_some(v)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transaction {
    pub kind: TxKind,
    pub date: Date,
    /// "HH:MM"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merchant_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nip: Option<String>,
    /// Minor units of `currency`.
    pub amount: i64,
    pub currency: String,
    pub amount_pln: i64,
    pub payment: Payment,
    pub account_id: Uuid,
    /// Used when there are no line items, and for any part they do not cover.
    pub category_id: Uuid,
    pub source: Source,
    pub review: Review,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<LineItem>,
    /// The user saw that the items do not add up to the amount and kept it anyway.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mismatch_confirmed: bool,
    /// Set on a bank operation that a receipt already records. It is kept for the record
    /// but not counted, and the receipt's items stand in for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duplicate_of: Option<Uuid>,
    /// The user said this operation has no duplicate, so it is not suggested again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unique_confirmed: bool,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TxError {
    #[error("Amount must be greater than zero")]
    Amount,
    #[error("Amount in PLN must be greater than zero")]
    AmountPln,
    #[error("Currency must be a three-letter code")]
    Currency,
    #[error("PLN amounts must match")]
    PlnMismatch,
    #[error("Time must look like 14:30")]
    Time,
    #[error("Item \"{0}\" has a wrong total")]
    ItemTotal(String),
    #[error("Items must have names")]
    ItemName,
    #[error("Items add up to {items}, not {amount}")]
    ItemsSum { items: String, amount: String },
}

/// NIP as digits only. Receipts print it as "123-456-32-18", "123 456 32 18" or "PL1234563218".
pub fn normalize_nip(s: &str) -> String {
    let t = s.trim();
    let t = t.strip_prefix("PL").or_else(|| t.strip_prefix("pl")).unwrap_or(t);
    t.chars().filter(|c| !matches!(c, ' ' | '-' | '.' | '\u{a0}')).collect()
}

/// A NIP is never required. This only says whether a given one looks right.
pub fn valid_nip(s: &str) -> bool {
    const W: [u32; 9] = [6, 5, 7, 2, 3, 4, 5, 6, 7];
    let d: Vec<u32> = s.chars().filter_map(|c| c.to_digit(10)).collect();
    d.len() == 10
        && s.chars().all(|c| c.is_ascii_digit())
        && d[..9].iter().zip(W).map(|(a, b)| a * b).sum::<u32>() % 11 == d[9]
}

fn valid_time(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 5 && b[2] == b':' && s[..2].parse::<u32>().is_ok_and(|h| h < 24) && s[3..].parse::<u32>().is_ok_and(|m| m < 60)
}

impl Transaction {
    pub fn items_total(&self) -> Option<i64> {
        self.items.iter().try_fold(0i64, |a, i| a.checked_add(i.total))
    }

    pub fn validate(&self) -> Result<(), TxError> {
        if self.amount <= 0 {
            return Err(TxError::Amount);
        }
        if self.amount_pln <= 0 {
            return Err(TxError::AmountPln);
        }
        if self.currency.len() != 3 || !self.currency.chars().all(|c| c.is_ascii_uppercase()) {
            return Err(TxError::Currency);
        }
        if self.currency == "PLN" && self.amount != self.amount_pln {
            return Err(TxError::PlnMismatch);
        }
        if self.time.as_deref().is_some_and(|t| !valid_time(t)) {
            return Err(TxError::Time);
        }
        for i in &self.items {
            if i.name.trim().is_empty() {
                return Err(TxError::ItemName);
            }
            if i.computed_total() != Some(i.total) {
                return Err(TxError::ItemTotal(i.name.clone()));
            }
        }
        if !self.items.is_empty() && !self.mismatch_confirmed && self.items_total() != Some(self.amount) {
            return Err(TxError::ItemsSum {
                items: crate::money::format_amount(self.items_total().unwrap_or(0)),
                amount: crate::money::format_amount(self.amount),
            });
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Category {
    pub name: String,
    /// Set when merged into another category. Old records keep working: a device
    /// that was offline during the merge can still send the previous id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged_into: Option<Uuid>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub name: String,
    pub currency: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Merchant {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nip: Option<String>,
}

/// One import of a file. Operations from it keep their own record ids, derived from their contents.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportBatch {
    pub name: String,
    pub imported: Date,
    pub account_id: Uuid,
    pub rows: usize,
    pub added: usize,
}

pub const KIND_IMPORT: &str = "import";
pub const KIND_PREFS: &str = "prefs";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    Dark,
    Light,
}

impl Theme {
    pub fn code(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }
}

/// The user's interface settings. One record per account, encrypted like the rest, so every device follows it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prefs {
    #[serde(default)]
    pub theme: Theme,
}

/// A personal correction. A rule without a merchant applies to every shop.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CategoryRule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merchant_id: Option<Uuid>,
    /// The item name in folded form, see `categorize::fold`.
    pub pattern: String,
    pub category_id: Uuid,
}

const fn builtin(n: u128) -> Uuid {
    Uuid::from_u128(0x6d6d0000_0000_4000_8000_000000000000 + n)
}

pub const CAT_FOOD: Uuid = builtin(0);
pub const CAT_HOUSEHOLD: Uuid = builtin(1);
pub const CAT_SUBSCRIPTIONS: Uuid = builtin(2);
pub const CAT_TRANSPORT: Uuid = builtin(3);
pub const CAT_HEALTH: Uuid = builtin(4);
pub const CAT_CLOTHING: Uuid = builtin(5);
pub const CAT_ENTERTAINMENT: Uuid = builtin(6);
pub const CAT_CAFES: Uuid = builtin(7);
pub const CAT_OTHER: Uuid = builtin(8);
pub const ACCOUNT_MAIN: Uuid = builtin(100);

/// Built-in ids are constant, so two devices never create duplicates.
/// Editing one writes a record with the same id, which takes priority.
pub fn default_categories() -> Vec<(Uuid, Category)> {
    ["Food", "Household", "Subscriptions", "Transport", "Health", "Clothing", "Entertainment", "Cafes & restaurants", "Other"]
        .iter()
        .enumerate()
        .map(|(i, n)| (builtin(i as u128), Category { name: (*n).into(), merged_into: None }))
        .collect()
}

pub fn default_accounts() -> Vec<(Uuid, Account)> {
    vec![(ACCOUNT_MAIN, Account { name: "Main".into(), currency: "PLN".into() })]
}

/// "  ŻABKA " and "żabka" are one merchant, so both devices get the same id.
pub fn normalize_name(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::parse_amount;

    pub fn tx(amount: i64) -> Transaction {
        Transaction {
            kind: TxKind::Expense,
            date: Date::new(2026, 10, 31).unwrap(),
            time: None,
            merchant_id: None,
            nip: None,
            amount,
            currency: "PLN".into(),
            amount_pln: amount,
            payment: Payment::Card,
            account_id: ACCOUNT_MAIN,
            category_id: CAT_OTHER,
            source: Source::Manual,
            review: Review::Reviewed,
            note: String::new(),
            items: vec![],
            mismatch_confirmed: false,
            duplicate_of: None,
            unique_confirmed: false,
        }
    }

    fn item(name: &str, qty: &str, price: i64, discount: i64) -> LineItem {
        let qty = Qty::parse(qty).unwrap();
        let total = line_value(qty, price).unwrap() - discount;
        LineItem { raw_name: name.into(), name: name.into(), qty, unit: "pcs".into(), unit_price: price, discount, total, vat_rate: None, category_id: CAT_OTHER }
    }

    #[test]
    fn a_plain_transaction_is_valid() {
        tx(1234).validate().unwrap();
    }

    #[test]
    fn rejects_bad_amounts() {
        assert_eq!(tx(0).validate(), Err(TxError::Amount));
        assert_eq!(tx(-1).validate(), Err(TxError::Amount));
        let t = Transaction { amount_pln: 5, ..tx(10) };
        assert_eq!(t.validate(), Err(TxError::PlnMismatch));
    }

    #[test]
    fn foreign_currency_keeps_both_amounts() {
        let t = Transaction { currency: "EUR".into(), amount: 1000, amount_pln: 4312, ..tx(1) };
        t.validate().unwrap();
        assert_eq!(Transaction { amount_pln: 0, ..t.clone() }.validate(), Err(TxError::AmountPln));
        assert_eq!(Transaction { currency: "eur".into(), ..t }.validate(), Err(TxError::Currency));
    }

    #[test]
    fn items_must_add_up_with_discounts() {
        let t = Transaction { items: vec![item("A", "3", 800, 400), item("B", "1", 700, 0)], ..tx(2700) };
        t.validate().unwrap();
        let bad = Transaction { amount: 2800, amount_pln: 2800, ..t };
        assert!(matches!(bad.validate(), Err(TxError::ItemsSum { .. })));
        Transaction { mismatch_confirmed: true, ..bad }.validate().unwrap();
    }

    #[test]
    fn item_total_must_match_quantity_and_price() {
        let mut i = item("A", "1,614", 2183, 0);
        assert_eq!(i.total, 3523);
        i.total = 3524;
        let t = Transaction { items: vec![i], ..tx(3524) };
        assert_eq!(t.validate(), Err(TxError::ItemTotal("A".into())));
    }

    #[test]
    fn discount_cannot_exceed_value() {
        let i = LineItem { discount: 900, total: -100, ..item("A", "1", 800, 0) };
        assert_eq!(i.computed_total(), None);
    }

    #[test]
    fn nip_uses_the_check_digit() {
        assert!(valid_nip("1234563218"));
        assert!(!valid_nip("1234563217"));
        assert!(!valid_nip("123456321"));
        assert!(!valid_nip("123-456-32-18"));
        assert!(valid_nip(&normalize_nip("123-456-32-18")));
        assert!(valid_nip(&normalize_nip(" PL 123 456 32 18 ")));
        assert_eq!(normalize_nip(""), "");
    }

    #[test]
    fn nip_is_optional_and_never_blocks_saving() {
        tx(1).validate().unwrap();
        // A typo is shown as a warning in the form, but the operation still saves.
        Transaction { nip: Some("1234563217".into()), ..tx(1) }.validate().unwrap();
    }

    #[test]
    fn time_format_is_checked() {
        for (t, ok) in [("14:30", true), ("00:00", true), ("23:59", true), ("24:00", false), ("12:60", false), ("1:30", false), ("ab:cd", false)] {
            let x = Transaction { time: Some(t.into()), ..tx(1) };
            assert_eq!(x.validate().is_ok(), ok, "{t}");
        }
    }

    #[test]
    fn json_keeps_amounts_as_integers() {
        let t = Transaction { items: vec![item("Jabłka", "0,532", 599, 0)], ..tx(parse_amount("3,19").unwrap()) };
        let j = serde_json::to_value(&t).unwrap();
        assert_eq!(j["amount"], 319);
        assert_eq!(j["items"][0]["qty"], "0.532");
        assert_eq!(serde_json::from_value::<Transaction>(j).unwrap(), t);
    }

    #[test]
    fn builtin_ids_are_stable_and_distinct() {
        let c = default_categories();
        assert_eq!(c.len(), 9);
        assert_eq!(c[8].0, CAT_OTHER);
        assert_eq!(c[8].1.name, "Other");
        let mut ids: Vec<_> = c.iter().map(|x| x.0).collect();
        ids.push(ACCOUNT_MAIN);
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 10);
    }
}
