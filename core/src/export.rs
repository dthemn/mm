//! Full export of a user's data. The output is plaintext by design: the user asked for it.

use crate::analytics::Tx;
use crate::date::Date;
use crate::model::*;
use crate::money::format_amount;
use serde_json::{Value, json};
use std::collections::HashMap;
use uuid::Uuid;

pub struct Lookup<'a> {
    pub categories: &'a HashMap<Uuid, Category>,
    pub accounts: &'a HashMap<Uuid, Account>,
    pub merchants: &'a HashMap<Uuid, Merchant>,
}

/// Plain dot-decimal, so spreadsheets in any locale read it the same way.
fn plain(minor: i64) -> String {
    format_amount(minor).replace(['\u{a0}', ' '], "").replace(',', ".")
}

/// Quotes a field, and defuses text a spreadsheet would run as a formula.
fn cell(s: &str) -> String {
    let safe = if s.starts_with(['=', '+', '-', '@', '\t', '\r']) { format!("'{s}") } else { s.to_string() };
    if safe.contains([',', '"', '\n', '\r', ';']) {
        format!("\"{}\"", safe.replace('"', "\"\""))
    } else {
        safe
    }
}

const HEADER: &str = "type,date,time,merchant,nip,amount,currency,amount_pln,payment,account,category,source,status,note,\
item,quantity,unit,unit_price,discount,item_total,vat_percent,item_category";

/// One row per line item. A transaction without items takes one row.
pub fn to_csv(txs: &[Tx], l: &Lookup) -> String {
    let cat = |id: &Uuid| l.categories.get(id).map(|c| c.name.as_str()).unwrap_or("");
    let mut out = String::from(HEADER);
    out.push_str("\r\n");
    let mut sorted: Vec<&Tx> = txs.iter().collect();
    sorted.sort_by_key(|t| (t.1.date, t.0));
    for (_, t) in sorted {
        let merchant = t.merchant_id.and_then(|m| l.merchants.get(&m)).map(|m| m.name.as_str()).unwrap_or("");
        let head = [
            if t.kind == TxKind::Income { "income" } else { "expense" }.to_string(),
            t.date.iso(),
            t.time.clone().unwrap_or_default(),
            merchant.to_string(),
            t.nip.clone().unwrap_or_default(),
            plain(t.amount),
            t.currency.clone(),
            plain(t.amount_pln),
            t.payment.code().to_string(),
            l.accounts.get(&t.account_id).map(|a| a.name.clone()).unwrap_or_default(),
            cat(&t.category_id).to_string(),
            format!("{:?}", t.source).to_lowercase(),
            if t.review == Review::NeedsReview { "needs_review" } else { "reviewed" }.to_string(),
            t.note.clone(),
        ];
        let rows: Vec<Vec<String>> = if t.items.is_empty() {
            vec![vec![String::new(); 8]]
        } else {
            t.items.iter().map(|i| vec![
                i.name.clone(),
                i.qty.display().replace(',', "."),
                i.unit.clone(),
                plain(i.unit_price),
                plain(i.discount),
                plain(i.total),
                i.vat_rate.map(|r| plain(r as i64)).unwrap_or_default(),
                cat(&i.category_id).to_string(),
            ]).collect()
        };
        for r in rows {
            let line: Vec<String> = head.iter().chain(r.iter()).map(|c| cell(c)).collect();
            out.push_str(&line.join(","));
            out.push_str("\r\n");
        }
    }
    out
}

pub fn to_json(txs: &[Tx], l: &Lookup, exported: Date) -> Value {
    fn map<T: serde::Serialize>(m: &HashMap<Uuid, T>) -> Vec<Value> {
        let mut v: Vec<(&Uuid, &T)> = m.iter().collect();
        v.sort_by_key(|x| *x.0);
        v.into_iter().map(|(id, x)| json!({ "id": id, "data": x })).collect()
    }
    let t: Vec<Value> = txs.iter().map(|(id, t)| json!({ "id": id, "data": t })).collect();
    json!({
        "format": "mm-export",
        "version": 1,
        "exported": exported.iso(),
        "transactions": t,
        "categories": map(l.categories),
        "accounts": map(l.accounts),
        "merchants": map(l.merchants),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Qty;

    fn tx(note: &str) -> Transaction {
        Transaction {
            kind: TxKind::Expense, date: Date::new(2026, 10, 31).unwrap(), time: Some("10:44".into()), merchant_id: None,
            nip: None, amount: 123456, currency: "PLN".into(), amount_pln: 123456, payment: Payment::Card,
            account_id: ACCOUNT_MAIN, category_id: CAT_OTHER, source: Source::Manual, review: Review::Reviewed,
            note: note.into(), items: vec![], mismatch_confirmed: false,
        }
    }

    fn lookup<'a>(c: &'a HashMap<Uuid, Category>, a: &'a HashMap<Uuid, Account>, m: &'a HashMap<Uuid, Merchant>) -> Lookup<'a> {
        Lookup { categories: c, accounts: a, merchants: m }
    }

    #[test]
    fn csv_has_exact_decimal_amounts() {
        let (c, a, m): (HashMap<_, _>, HashMap<_, _>, _) = (default_categories().into_iter().collect(), default_accounts().into_iter().collect(), HashMap::new());
        let s = to_csv(&[(Uuid::new_v4(), tx("x"))], &lookup(&c, &a, &m));
        let row = s.lines().nth(1).unwrap();
        assert!(row.starts_with("expense,2026-10-31,10:44,,,1234.56,PLN,1234.56,card,Main,Other,manual,reviewed,x,"), "{row}");
        assert_eq!(row.split(',').count(), HEADER.split(',').count());
    }

    #[test]
    fn csv_quotes_special_characters() {
        assert_eq!(cell("a,b"), "\"a,b\"");
        assert_eq!(cell("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(cell("two\nlines"), "\"two\nlines\"");
    }

    #[test]
    fn csv_defuses_spreadsheet_formulas() {
        for s in ["=1+1", "+1", "-1", "@SUM(A1)"] {
            assert!(cell(s).starts_with('\''), "{s}");
        }
        assert_eq!(cell("bread"), "bread");
    }

    #[test]
    fn each_item_takes_a_row() {
        let mut t = tx("");
        t.items = vec![LineItem {
            raw_name: "Mleko".into(), name: "Mleko".into(), qty: Qty::parse("0,532").unwrap(), unit: "kg".into(),
            unit_price: 599, discount: 0, total: 319, vat_rate: Some(500), category_id: CAT_OTHER,
        }; 2];
        let (c, a, m) = (HashMap::new(), HashMap::new(), HashMap::new());
        let s = to_csv(&[(Uuid::new_v4(), t)], &lookup(&c, &a, &m));
        assert_eq!(s.lines().count(), 3);
        assert!(s.lines().nth(1).unwrap().contains(",Mleko,0.532,kg,5.99,0.00,3.19,5.00,"));
    }

    #[test]
    fn json_export_round_trips_transactions() {
        let (c, a, m) = (HashMap::new(), HashMap::new(), HashMap::new());
        let id = Uuid::new_v4();
        let t = tx("note");
        let v = to_json(&[(id, t.clone())], &lookup(&c, &a, &m), t.date);
        assert_eq!(v["transactions"][0]["id"], id.to_string());
        assert_eq!(serde_json::from_value::<Transaction>(v["transactions"][0]["data"].clone()).unwrap(), t);
    }
}
