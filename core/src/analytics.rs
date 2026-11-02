//! The dashboard is computed on the client, because the server cannot read the data.

use crate::date::{Date, Month};
use crate::model::*;
use crate::money::allocate;
use std::collections::{BTreeMap, HashMap};
use uuid::Uuid;

pub const TREND_MONTHS: i32 = 6;
const TOP_MERCHANTS: usize = 5;
const RECENT: usize = 5;

pub type Tx = (Uuid, Transaction);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filter {
    pub from: Option<Date>,
    pub to: Option<Date>,
    pub account: Option<Uuid>,
    pub category: Option<Uuid>,
}

/// Resolves merged categories. A cycle from conflicting merges stops at the start.
pub fn canonical(cats: &HashMap<Uuid, Category>, id: Uuid) -> Uuid {
    let mut cur = id;
    for _ in 0..32 {
        match cats.get(&cur).and_then(|c| c.merged_into) {
            Some(next) if next != id => cur = next,
            _ => return cur,
        }
    }
    id
}

/// One part of a transaction. Parts always add up to exactly `amount_pln`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Part {
    pub category: Uuid,
    pub amount_pln: i64,
    pub item: Option<usize>,
}

pub fn parts(t: &Transaction, cats: &HashMap<Uuid, Category>) -> Vec<Part> {
    let own = canonical(cats, t.category_id);
    if t.items.is_empty() {
        return vec![Part { category: own, amount_pln: t.amount_pln, item: None }];
    }
    let mut weights: Vec<i64> = t.items.iter().map(|i| i.total.max(0)).collect();
    let covered: i64 = weights.iter().fold(0i64, |a, w| a.saturating_add(*w));
    let uncovered = t.amount.saturating_sub(covered);
    if uncovered > 0 {
        weights.push(uncovered);
    }
    allocate(t.amount_pln, &weights)
        .into_iter()
        .enumerate()
        .map(|(i, amount_pln)| match t.items.get(i) {
            Some(it) => Part { category: canonical(cats, it.category_id), amount_pln, item: Some(i) },
            None => Part { category: own, amount_pln, item: None },
        })
        .collect()
}

fn in_scope(t: &Transaction, f: &Filter) -> bool {
    f.from.is_none_or(|d| t.date >= d) && f.to.is_none_or(|d| t.date <= d) && f.account.is_none_or(|a| t.account_id == a)
}

fn filtered_parts(t: &Transaction, f: &Filter, cats: &HashMap<Uuid, Category>) -> Vec<Part> {
    let want = f.category.map(|c| canonical(cats, c));
    let mut p = parts(t, cats);
    p.retain(|x| want.is_none_or(|c| x.category == c));
    p
}

pub fn filter_txs<'a>(txs: &'a [Tx], f: &Filter, cats: &HashMap<Uuid, Category>) -> Vec<&'a Tx> {
    let mut v: Vec<&Tx> = txs.iter().filter(|(_, t)| in_scope(t, f) && !filtered_parts(t, f, cats).is_empty()).collect();
    v.sort_by(|a, b| (b.1.date, &b.1.time, b.0).cmp(&(a.1.date, &a.1.time, a.0)));
    v
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MonthTotal {
    pub month: Option<Month>,
    pub income: i64,
    pub expense: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Dashboard {
    pub income: i64,
    pub expense: i64,
    pub balance: i64,
    /// Largest first. Merged categories are already resolved.
    pub by_category: Vec<(Uuid, i64)>,
    /// Oldest first, always `TREND_MONTHS` entries ending at the chosen month.
    pub trend: Vec<MonthTotal>,
    pub top_merchants: Vec<(Option<Uuid>, i64)>,
    pub recent: Vec<Uuid>,
}

/// `f.from` and `f.to` select the summary period. The trend ignores them
/// and always ends at `month`, but it honours the account and category filters.
pub fn dashboard(txs: &[Tx], cats: &HashMap<Uuid, Category>, f: &Filter, month: Month) -> Dashboard {
    let mut d = Dashboard::default();
    let mut by_cat: BTreeMap<Uuid, i64> = BTreeMap::new();
    let mut by_merchant: HashMap<Option<Uuid>, i64> = HashMap::new();
    let open = Filter { from: None, to: None, ..f.clone() };
    let months: Vec<Month> = (0..TREND_MONTHS).rev().map(|i| month.offset(-i)).collect();
    d.trend = months.iter().map(|m| MonthTotal { month: Some(*m), ..Default::default() }).collect();

    for (_, t) in txs {
        let open_sum: i64 = if in_scope(t, &open) {
            filtered_parts(t, &open, cats).iter().map(|p| p.amount_pln).sum()
        } else {
            0
        };
        if let Some(pos) = months.iter().position(|m| *m == t.date.month_of()) {
            match t.kind {
                TxKind::Income => d.trend[pos].income += open_sum,
                TxKind::Expense => d.trend[pos].expense += open_sum,
            }
        }
        if !in_scope(t, f) {
            continue;
        }
        for p in filtered_parts(t, f, cats) {
            match t.kind {
                TxKind::Income => d.income += p.amount_pln,
                TxKind::Expense => {
                    d.expense += p.amount_pln;
                    *by_cat.entry(p.category).or_default() += p.amount_pln;
                    *by_merchant.entry(t.merchant_id).or_default() += p.amount_pln;
                }
            }
        }
    }
    d.balance = d.income - d.expense;
    d.by_category = by_cat.into_iter().collect();
    d.by_category.sort_by_key(|c| (std::cmp::Reverse(c.1), c.0));
    d.top_merchants = by_merchant.into_iter().filter(|(m, _)| m.is_some()).collect();
    d.top_merchants.sort_by_key(|m| (std::cmp::Reverse(m.1), m.0));
    d.top_merchants.truncate(TOP_MERCHANTS);
    d.recent = filter_txs(txs, f, cats).iter().take(RECENT).map(|t| t.0).collect();
    d
}

/// A row in the list opened from a category.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CategoryLine {
    pub tx: Uuid,
    pub date: Date,
    pub name: Option<String>,
    pub amount_pln: i64,
}

pub fn category_lines(txs: &[Tx], cats: &HashMap<Uuid, Category>, f: &Filter, category: Uuid) -> Vec<CategoryLine> {
    let f = Filter { category: Some(category), ..f.clone() };
    let mut out = vec![];
    for (id, t) in filter_txs(txs, &f, cats) {
        if t.kind != TxKind::Expense {
            continue;
        }
        for p in filtered_parts(t, &f, cats) {
            let name = p.item.and_then(|i| t.items.get(i)).map(|i| i.name.clone());
            out.push(CategoryLine { tx: *id, date: t.date, name, amount_pln: p.amount_pln });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Qty;

    const A: Uuid = Uuid::from_u128(11);
    const B: Uuid = Uuid::from_u128(12);
    const C: Uuid = Uuid::from_u128(13);

    fn d(m: u32, day: u32) -> Date {
        Date::new(2026, m, day).unwrap()
    }

    fn tx(kind: TxKind, date: Date, amount: i64, cat: Uuid) -> Tx {
        (Uuid::new_v4(), Transaction {
            kind, date, time: None, merchant_id: None, nip: None, amount, currency: "PLN".into(), amount_pln: amount,
            payment: Payment::Card, account_id: ACCOUNT_MAIN, category_id: cat, source: Source::Manual,
            review: Review::Reviewed, note: String::new(), items: vec![],
        })
    }

    fn item(name: &str, total: i64, cat: Uuid) -> LineItem {
        LineItem { raw_name: name.into(), name: name.into(), qty: Qty::ONE, unit: "pcs".into(), unit_price: total, discount: 0, total, vat_rate: None, category_id: cat }
    }

    fn none() -> HashMap<Uuid, Category> {
        HashMap::new()
    }

    fn month(y: i32, m: u32) -> Month {
        Month { year: y, month: m }
    }

    fn of(m: Month) -> Filter {
        Filter { from: Some(m.first()), to: Some(m.last()), ..Default::default() }
    }

    #[test]
    fn month_summary_counts_income_and_expense() {
        let t = vec![
            tx(TxKind::Income, d(10, 1), 500000, A),
            tx(TxKind::Expense, d(10, 5), 12345, A),
            tx(TxKind::Expense, d(10, 31), 1, B),
            tx(TxKind::Expense, d(9, 30), 99999, A),
            tx(TxKind::Expense, d(11, 1), 99999, A),
        ];
        let r = dashboard(&t, &none(), &of(month(2026, 10)), month(2026, 10));
        assert_eq!((r.income, r.expense, r.balance), (500000, 12346, 487654));
    }

    #[test]
    fn categories_are_counted_per_item() {
        let mut t = tx(TxKind::Expense, d(10, 5), 1000, C);
        t.1.items = vec![item("bread", 300, A), item("milk", 300, A), item("shampoo", 400, B)];
        let r = dashboard(&[t], &none(), &of(month(2026, 10)), month(2026, 10));
        assert_eq!(r.by_category, vec![(A, 600), (B, 400)]);
    }

    #[test]
    fn income_never_appears_in_expense_categories() {
        let r = dashboard(&[tx(TxKind::Income, d(10, 1), 100, A)], &none(), &of(month(2026, 10)), month(2026, 10));
        assert!(r.by_category.is_empty());
    }

    #[test]
    fn parts_always_add_up_to_the_pln_amount() {
        let mut t = tx(TxKind::Expense, d(10, 5), 1000, C).1;
        t.currency = "EUR".into();
        t.amount_pln = 4333;
        t.items = vec![item("a", 333, A), item("b", 333, B), item("c", 334, A)];
        let p = parts(&t, &none());
        assert_eq!(p.iter().map(|x| x.amount_pln).sum::<i64>(), 4333);
        assert_eq!(p.len(), 3);
    }

    #[test]
    fn the_part_items_do_not_cover_goes_to_the_transaction_category() {
        let mut t = tx(TxKind::Expense, d(10, 5), 1000, C).1;
        t.items = vec![item("a", 600, A)];
        let p = parts(&t, &none());
        assert_eq!(p[1], Part { category: C, amount_pln: 400, item: None });
    }

    #[test]
    fn items_above_the_amount_cannot_invent_money() {
        let mut t = tx(TxKind::Expense, d(10, 5), 1000, C).1;
        t.items = vec![item("a", 900, A), item("b", 900, B)];
        assert_eq!(parts(&t, &none()).iter().map(|x| x.amount_pln).sum::<i64>(), 1000);
    }

    #[test]
    fn merged_categories_resolve_even_for_late_records() {
        let mut cats = HashMap::new();
        cats.insert(A, Category { name: "a".into(), merged_into: Some(B) });
        cats.insert(B, Category { name: "b".into(), merged_into: Some(C) });
        let t = vec![tx(TxKind::Expense, d(10, 1), 100, A), tx(TxKind::Expense, d(10, 2), 50, C)];
        let r = dashboard(&t, &cats, &of(month(2026, 10)), month(2026, 10));
        assert_eq!(r.by_category, vec![(C, 150)]);
    }

    #[test]
    fn a_merge_cycle_terminates() {
        let mut cats = HashMap::new();
        cats.insert(A, Category { name: "a".into(), merged_into: Some(B) });
        cats.insert(B, Category { name: "b".into(), merged_into: Some(A) });
        let _ = canonical(&cats, A);
        let _ = canonical(&cats, B);
    }

    #[test]
    fn trend_has_six_months_ending_at_the_chosen_one_across_a_year() {
        let t = vec![
            (Uuid::new_v4(), Transaction { date: Date::new(2025, 9, 15).unwrap(), ..tx(TxKind::Expense, d(1, 1), 100, A).1 }),
            (Uuid::new_v4(), Transaction { date: Date::new(2025, 8, 31).unwrap(), ..tx(TxKind::Expense, d(1, 1), 999, A).1 }),
            tx(TxKind::Income, d(2, 1), 700, A),
        ];
        let r = dashboard(&t, &none(), &of(month(2026, 2)), month(2026, 2));
        assert_eq!(r.trend.len(), 6);
        assert_eq!(r.trend[0].month, Some(month(2025, 9)));
        assert_eq!(r.trend[0].expense, 100);
        assert_eq!(r.trend[5].month, Some(month(2026, 2)));
        assert_eq!(r.trend[5].income, 700);
    }

    #[test]
    fn account_filter_applies_to_summary_and_trend() {
        let other = Uuid::from_u128(99);
        let mut b = tx(TxKind::Expense, d(10, 1), 50, A);
        b.1.account_id = other;
        let t = vec![tx(TxKind::Expense, d(10, 1), 100, A), b];
        let f = Filter { account: Some(other), ..of(month(2026, 10)) };
        let r = dashboard(&t, &none(), &f, month(2026, 10));
        assert_eq!(r.expense, 50);
        assert_eq!(r.trend[5].expense, 50);
    }

    #[test]
    fn category_filter_counts_only_that_share_of_a_receipt() {
        let mut t = tx(TxKind::Expense, d(10, 5), 1000, C);
        t.1.items = vec![item("a", 700, A), item("b", 300, B)];
        let txs = [t];
        let f = Filter { category: Some(B), ..of(month(2026, 10)) };
        assert_eq!(dashboard(&txs, &none(), &f, month(2026, 10)).expense, 300);
        assert_eq!(filter_txs(&txs, &f, &none()).len(), 1);
        let miss = Filter { category: Some(C), ..of(month(2026, 10)) };
        assert!(filter_txs(&txs, &miss, &none()).is_empty());
    }

    #[test]
    fn top_merchants_sorted_and_unknown_ignored() {
        let (m, n) = (Uuid::from_u128(1), Uuid::from_u128(2));
        let with = |x, a| { let mut t = tx(TxKind::Expense, d(10, 1), a, A); t.1.merchant_id = x; t };
        let t = vec![with(Some(m), 100), with(Some(n), 300), with(Some(m), 150), with(None, 9999)];
        let r = dashboard(&t, &none(), &of(month(2026, 10)), month(2026, 10));
        assert_eq!(r.top_merchants, vec![(Some(n), 300), (Some(m), 250)]);
    }

    #[test]
    fn recent_lists_newest_first() {
        let a = tx(TxKind::Expense, d(10, 1), 1, A);
        let b = tx(TxKind::Expense, d(10, 9), 1, A);
        let ids = (a.0, b.0);
        let r = dashboard(&[a, b], &none(), &of(month(2026, 10)), month(2026, 10));
        assert_eq!(r.recent, vec![ids.1, ids.0]);
    }

    #[test]
    fn category_lines_list_items_and_plain_transactions() {
        let mut r = tx(TxKind::Expense, d(10, 5), 1000, B);
        r.1.items = vec![item("bread", 400, A), item("soap", 600, B)];
        let plain = tx(TxKind::Expense, d(10, 6), 250, A);
        let l = category_lines(&[r, plain], &none(), &of(month(2026, 10)), A);
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].amount_pln, 250);
        assert_eq!(l[1].name.as_deref(), Some("bread"));
    }
}
