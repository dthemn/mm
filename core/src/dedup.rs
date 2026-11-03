//! Finding the same money twice: a receipt and its bank operation, both sides of a transfer
//! between own accounts, and payments that repeat. Nothing is merged without a clear rule;
//! uncertain cases go to the user.

use crate::analytics::Tx;
use crate::categorize::fold;
use crate::model::*;
use uuid::Uuid;

/// Banks book card payments up to a few days after the purchase.
const RECEIPT_DAYS: i64 = 4;
const TRANSFER_DAYS: i64 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confidence {
    /// One candidate, same amount, at most a day apart: linked without asking.
    Sure,
    /// Shown to the user, who decides.
    Maybe,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiptMatch {
    pub bank: Uuid,
    pub receipt: Uuid,
    pub confidence: Confidence,
}

fn open(t: &Transaction) -> bool {
    t.duplicate_of.is_none() && !t.unique_confirmed && t.kind == TxKind::Expense
}

fn similar(a: &str, b: &str) -> bool {
    let (a, b) = (fold(a), fold(b));
    let words = |s: &str| s.split(' ').filter(|w| w.len() >= 4).map(str::to_string).collect::<Vec<_>>();
    let wb = words(&b);
    words(&a).iter().any(|w| wb.iter().any(|x| x.starts_with(w.as_str()) || w.starts_with(x.as_str())))
}

/// Pairs of a bank operation and a receipt for the same purchase. `name` gives a transaction's
/// merchant or description, for comparing names.
pub fn receipt_matches(txs: &[Tx], name: impl Fn(&Transaction) -> String) -> Vec<ReceiptMatch> {
    let linked: std::collections::HashSet<Uuid> = txs.iter().filter_map(|t| t.1.duplicate_of).collect();
    let receipts: Vec<&Tx> = txs.iter().filter(|t| t.1.source == Source::Receipt && open(&t.1) && !linked.contains(&t.0)).collect();
    let mut taken = std::collections::HashSet::new();
    let mut out = vec![];
    let mut banks: Vec<&Tx> = txs.iter().filter(|t| t.1.source == Source::Statement && open(&t.1)).collect();
    banks.sort_by_key(|t| (t.1.date, t.0));
    for (bid, b) in banks {
        let cands: Vec<&&Tx> = receipts.iter()
            .filter(|(rid, r)| !taken.contains(rid) && r.amount_pln == b.amount_pln && r.date.days_between(b.date) <= RECEIPT_DAYS)
            .collect();
        let Some((rid, r)) = cands.iter().min_by_key(|(id, r)| (r.date.days_between(b.date), *id)).map(|c| **c) else { continue };
        let sure = cands.len() == 1 && r.date.days_between(b.date) <= 1
            && (similar(&name(r), &name(b)) || name(b).is_empty());
        taken.insert(*rid);
        out.push(ReceiptMatch { bank: *bid, receipt: *rid, confidence: if sure { Confidence::Sure } else { Confidence::Maybe } });
    }
    out
}

/// An outgoing and an incoming operation of the same amount between two of the user's accounts.
pub fn transfer_pairs(txs: &[Tx]) -> Vec<(Uuid, Uuid)> {
    let mut used = std::collections::HashSet::new();
    let mut out = vec![];
    let mut outs: Vec<&Tx> = txs.iter().filter(|t| t.1.kind == TxKind::Expense && t.1.duplicate_of.is_none() && t.1.source != Source::Receipt).collect();
    outs.sort_by_key(|t| (t.1.date, t.0));
    for (oid, o) in outs {
        let best = txs.iter()
            .filter(|(iid, i)| i.kind == TxKind::Income && i.account_id != o.account_id && !used.contains(iid)
                && i.amount_pln == o.amount_pln && i.date.days_between(o.date) <= TRANSFER_DAYS)
            .min_by_key(|(iid, i)| (i.date.days_between(o.date), *iid));
        if let Some((iid, _)) = best {
            used.insert(*iid);
            out.push((*oid, *iid));
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recurring {
    pub name: String,
    pub merchant_id: Option<Uuid>,
    /// The latest amount, which is what the next payment will most likely be.
    pub amount_pln: i64,
    pub period_days: i64,
    pub count: usize,
    pub last: crate::date::Date,
}

/// Expenses that repeat weekly, monthly or yearly with a steady amount, from at least three payments.
pub fn recurring(txs: &[Tx], name: impl Fn(&Transaction) -> String) -> Vec<Recurring> {
    let mut groups: std::collections::BTreeMap<String, Vec<&Transaction>> = Default::default();
    for (_, t) in txs.iter().filter(|t| crate::analytics::counts(&t.1) && t.1.kind == TxKind::Expense) {
        let key = match t.merchant_id {
            Some(m) => m.to_string(),
            None => fold(&name(t)).split(' ').take(3).collect::<Vec<_>>().join(" "),
        };
        if !key.is_empty() {
            groups.entry(key).or_default().push(t);
        }
    }
    let mut out = vec![];
    for (_, mut g) in groups {
        if g.len() < 3 {
            continue;
        }
        g.sort_by_key(|t| t.date);
        let gaps: Vec<i64> = g.windows(2).map(|w| w[1].date.days() - w[0].date.days()).collect();
        let mut sorted = gaps.clone();
        sorted.sort();
        let period = sorted[sorted.len() / 2];
        let steady = match period {
            6..=8 => gaps.iter().all(|x| (5..=9).contains(x)),
            27..=33 => gaps.iter().all(|x| (25..=36).contains(x)),
            360..=371 => gaps.iter().all(|x| (355..=376).contains(x)),
            _ => false,
        };
        let mut amounts: Vec<i64> = g.iter().map(|t| t.amount_pln).collect();
        amounts.sort();
        let median = amounts[amounts.len() / 2];
        let stable = amounts.iter().all(|a| (a - median).abs() * 10 <= median);
        if steady && stable {
            let last = g[g.len() - 1];
            out.push(Recurring {
                name: name(last), merchant_id: last.merchant_id, amount_pln: last.amount_pln,
                period_days: period, count: g.len(), last: last.date,
            });
        }
    }
    out.sort_by_key(|r| std::cmp::Reverse(r.amount_pln));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::date::Date;

    fn t(kind: TxKind, source: Source, date: (u32, u32), amount: i64, note: &str) -> Tx {
        (Uuid::new_v4(), Transaction {
            kind, date: Date::new(2026, date.0, date.1).unwrap(), time: None, merchant_id: None, nip: None, amount,
            currency: "PLN".into(), amount_pln: amount, payment: Payment::Card, account_id: ACCOUNT_MAIN, category_id: CAT_OTHER,
            source, review: Review::Reviewed, note: note.into(), items: vec![], mismatch_confirmed: false,
            duplicate_of: None, unique_confirmed: false,
        })
    }

    fn note(t: &Transaction) -> String {
        t.note.clone()
    }

    #[test]
    fn a_receipt_and_its_card_payment_are_one_purchase() {
        let r = t(TxKind::Expense, Source::Receipt, (10, 30), 3200, "TEDi");
        let b = t(TxKind::Expense, Source::Statement, (10, 31), 3200, "TEDI SKLEP 7251 BIELAWA");
        let m = receipt_matches(&[r.clone(), b.clone()], note);
        assert_eq!(m, vec![ReceiptMatch { bank: b.0, receipt: r.0, confidence: Confidence::Sure }]);
    }

    #[test]
    fn different_amounts_or_far_dates_never_match() {
        let r = t(TxKind::Expense, Source::Receipt, (10, 1), 3200, "TEDi");
        assert!(receipt_matches(&[r.clone(), t(TxKind::Expense, Source::Statement, (10, 2), 3201, "TEDI")], note).is_empty());
        assert!(receipt_matches(&[r, t(TxKind::Expense, Source::Statement, (10, 9), 3200, "TEDI")], note).is_empty());
    }

    #[test]
    fn uncertain_matches_are_left_to_the_user() {
        // Two receipts for the same amount: the app must not pick one silently.
        let r1 = t(TxKind::Expense, Source::Receipt, (10, 1), 999, "Żabka");
        let r2 = t(TxKind::Expense, Source::Receipt, (10, 2), 999, "Lidl");
        let b = t(TxKind::Expense, Source::Statement, (10, 2), 999, "KARTA 1234");
        let m = receipt_matches(&[r1, r2, b], note);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].confidence, Confidence::Maybe);
        // Booked three days later with an unrelated name is also only a suggestion.
        let r = t(TxKind::Expense, Source::Receipt, (10, 1), 500, "Apteka");
        let b = t(TxKind::Expense, Source::Statement, (10, 4), 500, "ORLEN STACJA");
        assert_eq!(receipt_matches(&[r, b], note)[0].confidence, Confidence::Maybe);
    }

    #[test]
    fn linked_or_dismissed_operations_are_not_offered_again() {
        let r = t(TxKind::Expense, Source::Receipt, (10, 1), 500, "A");
        let mut b = t(TxKind::Expense, Source::Statement, (10, 1), 500, "A");
        b.1.duplicate_of = Some(r.0);
        assert!(receipt_matches(&[r.clone(), b.clone()], note).is_empty());
        b.1.duplicate_of = None;
        b.1.unique_confirmed = true;
        assert!(receipt_matches(&[r, b], note).is_empty());
    }

    #[test]
    fn one_receipt_matches_one_bank_operation() {
        let r = t(TxKind::Expense, Source::Receipt, (10, 1), 500, "Kawa");
        let b1 = t(TxKind::Expense, Source::Statement, (10, 1), 500, "KAWA");
        let b2 = t(TxKind::Expense, Source::Statement, (10, 2), 500, "KAWA");
        assert_eq!(receipt_matches(&[r, b1, b2], note).len(), 1);
    }

    #[test]
    fn a_transfer_between_own_accounts_is_found() {
        let out = t(TxKind::Expense, Source::Statement, (10, 1), 100000, "Przelew własny");
        let mut inc = t(TxKind::Income, Source::Statement, (10, 2), 100000, "Przelew własny");
        inc.1.account_id = Uuid::from_u128(77);
        assert_eq!(transfer_pairs(&[out.clone(), inc.clone()]), vec![(out.0, inc.0)]);
        inc.1.account_id = ACCOUNT_MAIN;
        assert!(transfer_pairs(&[out, inc]).is_empty(), "the same account is not a transfer");
    }

    #[test]
    fn monthly_payments_are_recognized() {
        let txs: Vec<Tx> = [(7, 5), (8, 5), (9, 6), (10, 5)].iter().map(|d| t(TxKind::Expense, Source::Statement, *d, 4999, "NETFLIX.COM 866")).collect();
        let r = recurring(&txs, note);
        assert_eq!(r.len(), 1);
        assert_eq!((r[0].period_days, r[0].count, r[0].amount_pln), (31, 4, 4999));
    }

    #[test]
    fn irregular_or_varying_payments_are_not_subscriptions() {
        let shop: Vec<Tx> = [(7, 1), (7, 3), (8, 20), (10, 2)].iter().map(|d| t(TxKind::Expense, Source::Statement, *d, 4999, "BIEDRONKA")).collect();
        assert!(recurring(&shop, note).is_empty());
        let varying: Vec<Tx> = [(7, 5, 4999), (8, 5, 8999), (9, 5, 1999)].iter().map(|d| t(TxKind::Expense, Source::Statement, (d.0, d.1), d.2, "SKLEP")).collect();
        assert!(recurring(&varying, note).is_empty());
        let two: Vec<Tx> = [(7, 5), (8, 5)].iter().map(|d| t(TxKind::Expense, Source::Statement, *d, 4999, "SPOTIFY")).collect();
        assert!(recurring(&two, note).is_empty());
    }
}
