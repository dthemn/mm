//! The dashboard is computed on the client, because the server cannot read the data.

use crate::date::{Date, Month};
use crate::model::*;
use crate::categorize::fold;
use crate::money::{Qty, allocate};
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
    /// Categories left out, as when the user taps one off in the chart.
    pub exclude: Vec<Uuid>,
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

/// The part of one operation that went to one category, with the receipt lines in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub category: Uuid,
    pub amount_pln: i64,
    /// Indexes into `items`. Empty for the share of the total no line covers.
    pub items: Vec<usize>,
}

/// A receipt sorted by category, largest first. The groups add up to exactly `amount_pln`.
pub fn groups(t: &Transaction, cats: &HashMap<Uuid, Category>) -> Vec<Group> {
    let mut out: Vec<Group> = vec![];
    for p in parts(t, cats) {
        let g = match out.iter_mut().position(|g| g.category == p.category) {
            Some(i) => &mut out[i],
            None => {
                out.push(Group { category: p.category, amount_pln: 0, items: vec![] });
                out.last_mut().expect("just pushed")
            }
        };
        g.amount_pln += p.amount_pln;
        g.items.extend(p.item);
    }
    out.sort_by_key(|g| (std::cmp::Reverse(g.amount_pln), g.category));
    out
}

/// Whether an operation counts in totals. Transfers and duplicates of receipts do not.
pub fn counts(t: &Transaction) -> bool {
    t.kind != TxKind::Transfer && t.duplicate_of.is_none()
}

fn in_scope(t: &Transaction, f: &Filter) -> bool {
    f.from.is_none_or(|d| t.date >= d) && f.to.is_none_or(|d| t.date <= d) && f.account.is_none_or(|a| t.account_id == a)
}

fn filtered_parts(t: &Transaction, f: &Filter, cats: &HashMap<Uuid, Category>) -> Vec<Part> {
    let want = f.category.map(|c| canonical(cats, c));
    let mut p = parts(t, cats);
    p.retain(|x| want.is_none_or(|c| x.category == c) && !f.exclude.iter().any(|e| canonical(cats, *e) == x.category));
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
    /// Spending by category, largest first. Merged categories are already resolved.
    pub by_category: Vec<(Uuid, i64)>,
    /// Income by category, largest first.
    pub income_by_category: Vec<(Uuid, i64)>,
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
    let mut in_cat: BTreeMap<Uuid, i64> = BTreeMap::new();
    let mut by_merchant: HashMap<Option<Uuid>, i64> = HashMap::new();
    let open = Filter { from: None, to: None, ..f.clone() };
    let months: Vec<Month> = (0..TREND_MONTHS).rev().map(|i| month.offset(-i)).collect();
    d.trend = months.iter().map(|m| MonthTotal { month: Some(*m), ..Default::default() }).collect();

    for (_, t) in txs.iter().filter(|t| counts(&t.1)) {
        let open_sum: i64 = if in_scope(t, &open) {
            filtered_parts(t, &open, cats).iter().map(|p| p.amount_pln).sum()
        } else {
            0
        };
        if let Some(pos) = months.iter().position(|m| *m == t.date.month_of()) {
            match t.kind {
                TxKind::Income => d.trend[pos].income += open_sum,
                TxKind::Expense => d.trend[pos].expense += open_sum,
                TxKind::Transfer => {}
            }
        }
        if !in_scope(t, f) {
            continue;
        }
        for p in filtered_parts(t, f, cats) {
            match t.kind {
                TxKind::Transfer => {}
                TxKind::Income => {
                    d.income += p.amount_pln;
                    *in_cat.entry(p.category).or_default() += p.amount_pln;
                }
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
    d.income_by_category = in_cat.into_iter().collect();
    d.income_by_category.sort_by_key(|c| (std::cmp::Reverse(c.1), c.0));
    d.top_merchants = by_merchant.into_iter().filter(|(m, _)| m.is_some()).collect();
    d.top_merchants.sort_by_key(|m| (std::cmp::Reverse(m.1), m.0));
    d.top_merchants.truncate(TOP_MERCHANTS);
    d.recent = filter_txs(txs, f, cats).iter().filter(|t| t.1.duplicate_of.is_none()).take(RECENT).map(|t| t.0).collect();
    d
}

/// One line found by a search: a receipt line, or a whole operation without lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub tx: Uuid,
    pub date: Date,
    pub kind: TxKind,
    pub merchant: Option<String>,
    /// The receipt line, or the note of an operation without lines.
    pub name: String,
    /// Quantity and its unit, for receipt lines.
    pub qty: Option<(Qty, String)>,
    pub amount_pln: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Found {
    /// Newest first.
    pub hits: Vec<Hit>,
    pub spent: i64,
    pub received: i64,
    /// Operations with at least one hit.
    pub purchases: usize,
    /// Quantities added up per unit, the most common unit first.
    pub quantities: Vec<(String, Qty)>,
}

/// "szt", "pcs" and nothing are one unit; others keep their name.
fn unit_of(u: &str) -> String {
    match u.trim().to_lowercase().as_str() {
        "" | "szt" | "szt." | "pcs" | "op" | "op." => "pcs".into(),
        other => other.trim_end_matches('.').to_string(),
    }
}

/// Lines whose item name, category, merchant or note contains the words of `query`, within the filter's period and
/// account. Case, Polish letters and punctuation do not matter.
pub fn search(txs: &[Tx], cats: &HashMap<Uuid, Category>, merchant: impl Fn(Option<Uuid>) -> Option<String>, f: &Filter, query: &str) -> Found {
    let words: Vec<String> = fold(query).split(' ').filter(|w| !w.is_empty()).map(String::from).collect();
    let mut out = Found::default();
    if words.is_empty() {
        return out;
    }
    let all = |text: &str| { let t = fold(text); words.iter().all(|w| t.contains(w.as_str())) };
    let cat_name = |id: Uuid| cats.get(&id).map(|c| c.name.clone()).unwrap_or_default();
    let mut units: Vec<(String, i64, usize)> = vec![];
    for (id, t) in filter_txs(txs, &Filter { category: None, exclude: vec![], ..f.clone() }, cats) {
        if !counts(t) {
            continue;
        }
        let shop = merchant(t.merchant_id);
        let whole = shop.as_deref().is_some_and(all) || all(&t.note);
        let mut found = false;
        for p in parts(t, cats) {
            let item = p.item.and_then(|i| t.items.get(i));
            let hit = whole || all(&cat_name(p.category)) || item.is_some_and(|i| all(&i.name) || all(&i.raw_name));
            if !hit {
                continue;
            }
            found = true;
            match t.kind {
                TxKind::Income => out.received += p.amount_pln,
                _ => out.spent += p.amount_pln,
            }
            let qty = item.map(|i| (i.qty, unit_of(&i.unit)));
            if let Some((q, u)) = &qty {
                match units.iter_mut().find(|x| x.0 == *u) {
                    Some(x) => { x.1 += q.thousandths(); x.2 += 1; }
                    None => units.push((u.clone(), q.thousandths(), 1)),
                }
            }
            out.hits.push(Hit {
                tx: *id, date: t.date, kind: t.kind, merchant: shop.clone(),
                name: item.map(|i| i.name.clone()).unwrap_or_else(|| if t.note.is_empty() { cat_name(p.category) } else { t.note.clone() }),
                qty, amount_pln: p.amount_pln,
            });
        }
        out.purchases += usize::from(found);
    }
    units.sort_by_key(|u| std::cmp::Reverse(u.2));
    out.quantities = units.into_iter().map(|(u, n, _)| (u, Qty::from_thousandths(n))).collect();
    out
}

/// Words to suggest while searching: item names, categories and merchants from the user's own data,
/// the most frequent first.
pub fn suggestions(txs: &[Tx], cats: &HashMap<Uuid, Category>, merchant: impl Fn(Option<Uuid>) -> Option<String>) -> Vec<String> {
    let mut seen: HashMap<String, (String, usize)> = HashMap::new();
    let mut add = |s: &str| {
        let s = s.trim();
        if s.chars().count() < 2 {
            return;
        }
        seen.entry(fold(s)).or_insert_with(|| (s.to_string(), 0)).1 += 1;
    };
    for (_, t) in txs {
        for i in &t.items {
            add(&i.name);
        }
        if let Some(m) = merchant(t.merchant_id) {
            add(&m);
        }
    }
    for c in cats.values().filter(|c| c.merged_into.is_none()) {
        add(&c.name);
    }
    let mut v: Vec<(String, usize)> = seen.into_values().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v.into_iter().map(|x| x.0).collect()
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
        if t.kind != TxKind::Expense || !counts(t) {
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
            review: Review::Reviewed, note: String::new(), items: vec![], mismatch_confirmed: false, duplicate_of: None, unique_confirmed: false,
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
        cats.insert(A, Category { name: "a".into(), merged_into: Some(B), income: false });
        cats.insert(B, Category { name: "b".into(), merged_into: Some(C), income: false });
        let t = vec![tx(TxKind::Expense, d(10, 1), 100, A), tx(TxKind::Expense, d(10, 2), 50, C)];
        let r = dashboard(&t, &cats, &of(month(2026, 10)), month(2026, 10));
        assert_eq!(r.by_category, vec![(C, 150)]);
    }

    #[test]
    fn a_merge_cycle_terminates() {
        let mut cats = HashMap::new();
        cats.insert(A, Category { name: "a".into(), merged_into: Some(B), income: false });
        cats.insert(B, Category { name: "b".into(), merged_into: Some(A), income: false });
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
    fn excluded_categories_leave_totals_trend_and_receipt_shares() {
        let mut t = tx(TxKind::Expense, d(10, 5), 1000, C);
        t.1.items = vec![item("a", 700, A), item("b", 300, B)];
        let txs = [t, tx(TxKind::Expense, d(10, 6), 50, B), tx(TxKind::Income, d(10, 7), 900, C)];
        let f = Filter { exclude: vec![B], ..of(month(2026, 10)) };
        let r = dashboard(&txs, &none(), &f, month(2026, 10));
        assert_eq!(r.expense, 700);
        assert_eq!(r.by_category, vec![(A, 700)]);
        assert_eq!(r.trend.last().unwrap().expense, 700);
        assert_eq!(r.income, 900);
        assert_eq!(filter_txs(&txs, &f, &none()).len(), 2, "the line only in B is gone");
    }

    #[test]
    fn income_is_split_by_its_categories() {
        let txs = [tx(TxKind::Income, d(10, 1), 5000, A), tx(TxKind::Income, d(10, 2), 700, B), tx(TxKind::Income, d(10, 3), 300, B)];
        let r = dashboard(&txs, &none(), &of(month(2026, 10)), month(2026, 10));
        assert_eq!(r.income_by_category, vec![(A, 5000), (B, 1000)]);
        assert!(r.by_category.is_empty());
    }

    #[test]
    fn a_receipt_is_grouped_by_category_and_the_groups_add_up() {
        let mut t = tx(TxKind::Expense, d(10, 5), 2176, C);
        t.1.items = vec![item("lody", 349, A), item("lody", 349, A), item("lody kokos", 599, A), item("ludwik", 879, B)];
        let g = groups(&t.1, &none());
        assert_eq!(g, vec![
            Group { category: A, amount_pln: 1297, items: vec![0, 1, 2] },
            Group { category: B, amount_pln: 879, items: vec![3] },
        ]);
        // A total above the lines leaves a share in the operation's own category.
        t.1.amount = 2276;
        t.1.amount_pln = 2276;
        let g = groups(&t.1, &none());
        assert_eq!(g.iter().map(|x| x.amount_pln).sum::<i64>(), 2276);
        assert_eq!(g.last().unwrap(), &Group { category: C, amount_pln: 100, items: vec![] });
    }

    #[test]
    fn search_adds_up_one_product_across_receipts() {
        let mut a = tx(TxKind::Expense, d(10, 5), 1000, C);
        let mut milk = item("Mleko 3,2% 1L", 450, A);
        milk.qty = Qty::parse("2").unwrap();
        milk.unit = "szt".into();
        a.1.items = vec![milk, item("Chleb", 550, A)];
        let mut b = tx(TxKind::Expense, d(10, 9), 349, C);
        b.1.items = vec![item("MLEKO UHT", 349, A)];
        let c = tx(TxKind::Expense, d(10, 10), 2000, C);
        let txs = [a, b, c];
        let f = of(month(2026, 10));
        let r = search(&txs, &none(), |_| None, &f, "mleko");
        assert_eq!(r.spent, 799);
        assert_eq!(r.purchases, 2);
        assert_eq!(r.hits.len(), 2);
        assert_eq!(r.hits[0].date, d(10, 9), "newest first");
        assert_eq!(r.quantities, vec![("pcs".to_string(), Qty::parse("3").unwrap())]);
        assert!(search(&txs, &none(), |_| None, &f, "  ").hits.is_empty());
        assert_eq!(search(&txs, &none(), |_| None, &f, "mleko uht").spent, 349, "every word must match");
        let outside = Filter { from: Some(d(10, 6)), ..f.clone() };
        assert_eq!(search(&txs, &none(), |_| None, &outside, "mleko").spent, 349, "the period applies");
    }

    #[test]
    fn search_finds_by_merchant_and_category() {
        let mut cats = HashMap::new();
        cats.insert(A, Category { name: "Food".into(), merged_into: None, income: false });
        let shop = Uuid::from_u128(99);
        let mut a = tx(TxKind::Expense, d(10, 5), 1000, B);
        a.1.merchant_id = Some(shop);
        a.1.items = vec![item("Chleb", 600, A), item("Mydło", 400, B)];
        let txs = [a];
        let name = |m: Option<Uuid>| (m == Some(shop)).then(|| "Żabka".to_string());
        let f = of(month(2026, 10));
        assert_eq!(search(&txs, &cats, name, &f, "zabka").spent, 1000, "a merchant finds the whole receipt");
        assert_eq!(search(&txs, &cats, name, &f, "food").spent, 600, "a category finds its lines");
        let s = suggestions(&txs, &cats, name);
        assert!(s.contains(&"Chleb".to_string()) && s.contains(&"Żabka".to_string()) && s.contains(&"Food".to_string()), "{s:?}");
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
    fn transfers_and_receipt_duplicates_are_not_counted() {
        let mut transfer = tx(TxKind::Transfer, d(10, 2), 50000, A);
        transfer.1.kind = TxKind::Transfer;
        let receipt = tx(TxKind::Expense, d(10, 3), 1234, A);
        let mut bank = tx(TxKind::Expense, d(10, 4), 1234, A);
        bank.1.duplicate_of = Some(receipt.0);
        let r = dashboard(&[transfer, receipt, bank], &none(), &of(month(2026, 10)), month(2026, 10));
        assert_eq!((r.income, r.expense), (0, 1234));
        assert_eq!(r.trend[5].expense, 1234);
        assert_eq!(r.recent.len(), 2, "the duplicate is hidden from recent operations");
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
