//! Bank statement import with a preview, and the review of possible duplicates.
//! Files are read on this device; only the resulting encrypted operations are synced.

use crate::app::{Ctx, Screen, busy_task, reload, run_sync, today};
use crate::data::{self, Data};
use crate::flows::Session;
use js_sys::Uint8Array;
use leptos::prelude::*;
use leptos::task::spawn_local;
use mm_core::analytics::Tx;
use mm_core::api::RatesResp;
use mm_core::categorize::categorize;
use mm_core::crypto::named_id;
use mm_core::date::Date;
use mm_core::dedup::{self, Confidence};
use mm_core::import::{self, Mapping, Row, Table};
use mm_core::model::*;
use mm_core::money::{convert, format_money};
use std::collections::HashMap;
use uuid::Uuid;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

fn session(c: Ctx) -> Result<Session, String> {
    c.session.get_untracked().ok_or_else(|| "The app is locked".to_string())
}

/// The record id of an imported row. Keyed with the DEK, so the server cannot learn the contents,
/// and stable, so importing the same statement again adds nothing.
pub fn row_id(s: &Session, account: Uuid, r: &Row) -> Uuid {
    named_id(&s.dek, b"import", &import::import_key(account, r))
}

/// The NBP rate for a date: the last one published before it, as Polish rules use the previous business day.
/// Rates already fetched, by currency, year and quarter.
pub type RateCache = HashMap<(String, i32, u32), Vec<(Date, i64)>>;

pub async fn nbp_rate(currency: &str, date: Date, cache: &mut RateCache) -> Result<Option<(Date, i64)>, String> {
    let q = |d: Date| (d.year, (d.month - 1) / 3 + 1);
    let (y, n) = q(date);
    // A date early in a quarter may need the last rates of the previous one.
    let prev = if n == 1 { (y - 1, 4) } else { (y, n - 1) };
    let mut all: Vec<(Date, i64)> = vec![];
    for (year, quarter) in [prev, (y, n)] {
        let key = (currency.to_string(), year, quarter);
        if !cache.contains_key(&key) {
            let r: RatesResp = crate::api::get(&format!("/api/rates/{}/{year}/{quarter}", currency.to_lowercase())).await
                .map_err(|e| format!("Could not get the NBP rate: {e}"))?;
            let rates = r.rates.iter().filter_map(|x| Some((Date::parse_iso(&x.date)?, x.rate_e4))).collect();
            cache.insert(key.clone(), rates);
        }
        all.extend(cache[&key].iter().copied());
    }
    Ok(all.into_iter().filter(|(d, _)| *d < date).max_by_key(|x| x.0))
}

#[derive(Clone, PartialEq)]
enum Status {
    New,
    Known,
}

fn read_file(input: web_sys::HtmlInputElement, done: impl FnOnce(Result<(String, Vec<u8>), String>) + 'static) {
    spawn_local(async move {
        let res = async {
            let file = input.files().and_then(|f| f.get(0)).ok_or("No file was chosen")?;
            let buf = JsFuture::from(file.array_buffer()).await.map_err(|_| "The file could not be read")?;
            Ok::<_, String>((file.name(), Uint8Array::new(&buf).to_vec()))
        }.await;
        input.set_value("");
        done(res);
    });
}

fn column_select(header: Vec<String>, sig: RwSignal<Option<usize>>, label: &'static str, test: &'static str) -> impl IntoView {
    view! {
        <label>{label}
            <select data-test=test on:change=move |e| sig.set(event_target_value(&e).parse().ok())>
                <option value="" selected=move || sig.get().is_none()>"Not in this file"</option>
                {header.into_iter().enumerate().map(|(i, h)| view! {
                    <option value=i.to_string() selected=move || sig.get() == Some(i)>{if h.is_empty() { format!("Column {}", i + 1) } else { h }}</option>
                }).collect_view()}
            </select>
        </label>
    }
}

pub fn import(c: Ctx) -> impl IntoView {
    let table: RwSignal<Option<Table>> = RwSignal::new(None);
    let name = RwSignal::new(String::new());
    let cols: [RwSignal<Option<usize>>; 7] = std::array::from_fn(|_| RwSignal::new(None));
    let [date, amount, debit, credit, currency, description, counterparty] = cols;
    let account = RwSignal::new(c.data.with_untracked(|d| d.account_list().first().map(|a| a.0)).unwrap_or(ACCOUNT_MAIN));
    let summary = RwSignal::new(String::new());

    let mapping = move || Mapping {
        date: date.get(), amount: amount.get(), debit: debit.get(), credit: credit.get(),
        currency: currency.get(), description: description.get(), counterparty: counterparty.get(),
    };
    let pick = move |ev: leptos::ev::Event| {
        let Some(input) = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) else { return };
        read_file(input, move |res| match res {
            Err(e) => c.error.set(e),
            Ok((file, bytes)) => {
                let t = import::parse_csv(&import::decode(&bytes));
                if t.rows.is_empty() {
                    c.error.set("No table was found in this file. Choose a CSV export from your bank".into());
                    return;
                }
                let g = import::guess(&t.header);
                for (sig, v) in cols.iter().zip([g.date, g.amount, g.debit, g.credit, g.currency, g.description, g.counterparty]) {
                    sig.set(v);
                }
                c.error.set(String::new());
                summary.set(String::new());
                name.set(file);
                table.set(Some(t));
            }
        });
    };
    let preview = move || {
        let t = table.get()?;
        let s = c.session.get()?;
        let (rows, bad) = import::rows(&t, &mapping());
        let known: std::collections::HashSet<Uuid> = c.data.with(|d| d.txs.iter().map(|t| t.0).collect());
        let acc = account.get();
        let marked: Vec<(Row, Status)> = rows.into_iter().map(|r| {
            let st = if known.contains(&row_id(&s, acc, &r)) { Status::Known } else { Status::New };
            (r, st)
        }).collect();
        let fresh = marked.iter().filter(|x| x.1 == Status::New).count();
        let list = marked.iter().take(50).map(|(r, st)| { let incoming = r.amount > 0; view! {
            <li class="txrow" data-test="import-row">
                <span class="grow">
                    <span class="name">{if r.counterparty.is_empty() { r.description.clone() } else { r.counterparty.clone() }}</span>
                    <span class="hint">{r.date.display()}{if *st == Status::Known { " · already imported" } else { "" }}</span>
                </span>
                <span class="amount" class:income=incoming><span>{format_money(r.amount, &r.currency)}</span></span>
            </li>
        }}).collect_view();
        Some(view! {
            <p data-test="import-counts">{format!("{} operations, {} new, {} already imported", marked.len(), fresh, marked.len() - fresh)}</p>
            {(!bad.is_empty()).then(|| view! {
                <p class="warn" data-test="import-skipped">{format!("{} rows skipped: {}", bad.len(),
                    bad.iter().take(3).map(|(l, why)| format!("row {l}, {why}")).collect::<Vec<_>>().join("; "))}</p>
            })}
            <ul class="txs">{list}</ul>
        })
    };
    let run = move |_| busy_task(c, async move {
        let s = session(c)?;
        let t = table.get_untracked().ok_or("Choose a file first")?;
        let (rows, _) = import::rows(&t, &mapping());
        let acc = account.get_untracked();
        let d = c.data.get_untracked();
        let rules = d.rule_list();
        let known: std::collections::HashSet<Uuid> = d.txs.iter().map(|t| t.0).collect();
        let mut rates = HashMap::new();
        let mut added = 0;
        for r in &rows {
            let id = row_id(&s, acc, r);
            if known.contains(&id) {
                continue;
            }
            let amount = r.amount.abs();
            let (amount_pln, review) = if r.currency == "PLN" {
                (amount, Review::Reviewed)
            } else {
                match nbp_rate(&r.currency, r.date, &mut rates).await? {
                    Some((_, rate)) => (convert(amount, rate).ok_or("amount too large")?, Review::Reviewed),
                    None => return Err(format!("No NBP rate was found for {} before {}", r.currency, r.date.display())),
                }
            };
            let text = [r.counterparty.as_str(), r.description.as_str()].iter().filter(|x| !x.is_empty()).copied().collect::<Vec<_>>().join(" · ");
            let tx = Transaction {
                kind: if r.amount < 0 { TxKind::Expense } else { TxKind::Income },
                date: r.date, time: None, merchant_id: None, nip: None, amount, currency: r.currency.clone(), amount_pln,
                payment: Payment::Card, account_id: acc,
                category_id: categorize(&rules, None, &r.counterparty, &r.description),
                source: Source::Statement, review, note: text, items: vec![], mismatch_confirmed: false,
                duplicate_of: None, unique_confirmed: false,
            };
            data::write(&s, id, KIND_TRANSACTION, &tx).await?;
            added += 1;
        }
        let batch = ImportBatch { name: name.get_untracked(), imported: today(), account_id: acc, rows: rows.len(), added };
        data::write(&s, Uuid::new_v4(), KIND_IMPORT, &batch).await?;
        reload(c, &s).await;
        let linked = link_sure(c, &s).await?;
        summary.set(format!("Imported {added} new operations. {linked} matched receipts you scanned."));
        table.set(None);
        reload(c, &s).await;
        spawn_local(run_sync(c));
        Ok(())
    });
    view! {
        <header class="row"><h1>"Import a statement"</h1><button on:click=move |_| c.screen.set(Screen::Settings)>"Back"</button></header>
        <p class="hint">"Choose a CSV export from your bank. The file is read on this phone and never uploaded."</p>
        <label class="button">"Choose a CSV file"<input type="file" accept=".csv,text/csv,text/plain" class="file" data-test="import-file" on:change=pick /></label>
        <p data-test="import-summary">{move || summary.get()}</p>
        {move || table.get().map(|t| {
            let h = t.header.clone();
            view! {
                <p class="hint">{move || name.get()}</p>
                <label>"Account"
                    <select data-test="import-account" on:change=move |e| if let Ok(i) = event_target_value(&e).parse() { account.set(i) }>
                        {c.data.with_untracked(Data::account_list).into_iter().map(|(i, a)| view! { <option value=i.to_string() selected=move || account.get() == i>{a.name}</option> }).collect_view()}
                    </select>
                </label>
                <details class="filters" open=true>
                    <summary>"Columns"</summary>
                    {column_select(h.clone(), date, "Date", "map-date")}
                    {column_select(h.clone(), amount, "Amount, with a sign", "map-amount")}
                    {column_select(h.clone(), debit, "Money out", "map-debit")}
                    {column_select(h.clone(), credit, "Money in", "map-credit")}
                    {column_select(h.clone(), currency, "Currency", "map-currency")}
                    {column_select(h.clone(), description, "Description", "map-description")}
                    {column_select(h, counterparty, "Counterparty", "map-counterparty")}
                </details>
                {preview}
                <button class="primary" data-test="import-run" disabled=move || c.busy.get() on:click=run>"Import"</button>
            }
        })}
    }
}

/// Links the certain pairs right away: one receipt, same amount, at most a day apart.
async fn link_sure(c: Ctx, s: &Session) -> Result<usize, String> {
    let d = c.data.get_untracked();
    let name = |t: &Transaction| d.merchant_name(t.merchant_id).unwrap_or_else(|| t.note.clone());
    let mut n = 0;
    for m in dedup::receipt_matches(&d.txs, name).into_iter().filter(|m| m.confidence == Confidence::Sure) {
        if let Some((_, b)) = d.txs.iter().find(|t| t.0 == m.bank) {
            data::write(s, m.bank, KIND_TRANSACTION, &Transaction { duplicate_of: Some(m.receipt), ..b.clone() }).await?;
            n += 1;
        }
    }
    Ok(n)
}

#[derive(Clone)]
enum Suggestion {
    Receipt { bank: Tx, receipt: Tx },
    Transfer { out: Tx, inc: Tx },
}

fn suggestions(d: &Data) -> Vec<Suggestion> {
    let name = |t: &Transaction| d.merchant_name(t.merchant_id).unwrap_or_else(|| t.note.clone());
    let find = |id: Uuid| d.txs.iter().find(|t| t.0 == id).cloned();
    let mut out: Vec<Suggestion> = dedup::receipt_matches(&d.txs, name).into_iter()
        .filter_map(|m| Some(Suggestion::Receipt { bank: find(m.bank)?, receipt: find(m.receipt)? }))
        .collect();
    out.extend(dedup::transfer_pairs(&d.txs).into_iter()
        .filter(|(o, i)| [o, i].iter().all(|id| find(**id).is_some_and(|t| !t.1.unique_confirmed)))
        .filter_map(|(o, i)| Some(Suggestion::Transfer { out: find(o)?, inc: find(i)? })));
    out
}

pub fn pending(d: &Data) -> usize {
    suggestions(d).len()
}

fn line(d: &Data, t: &Tx) -> String {
    let who = d.merchant_name(t.1.merchant_id).unwrap_or_else(|| t.1.note.clone());
    format!("{} · {} · {}", t.1.date.display(), who, format_money(t.1.amount, &t.1.currency))
}

pub fn duplicates(c: Ctx) -> impl IntoView {
    let save = move |writes: Vec<(Uuid, Transaction)>| busy_task(c, async move {
        let s = session(c)?;
        for (id, t) in writes {
            data::write(&s, id, KIND_TRANSACTION, &t).await?;
        }
        reload(c, &s).await;
        spawn_local(run_sync(c));
        Ok(())
    });
    let list = move || {
        let d = c.data.get();
        let items = suggestions(&d);
        if items.is_empty() {
            return view! { <p class="hint" data-test="no-duplicates">"Nothing to review."</p> }.into_any();
        }
        items.into_iter().map(|sg| match sg {
            Suggestion::Receipt { bank, receipt } => {
                let (b1, b2) = (bank.clone(), bank.clone());
                let rid = receipt.0;
                view! {
                    <li class="item" data-test="dup-receipt">
                        <p>"Is this one purchase?"</p>
                        <p class="hint">"Receipt: "{line(&d, &receipt)}</p>
                        <p class="hint">"Bank: "{line(&d, &bank)}</p>
                        <div class="row">
                            <button class="primary" data-test="dup-same" on:click=move |_| save(vec![(b1.0, Transaction { duplicate_of: Some(rid), ..b1.1.clone() })])>"Same purchase"</button>
                            <button data-test="dup-different" on:click=move |_| save(vec![(b2.0, Transaction { unique_confirmed: true, ..b2.1.clone() })])>"Different"</button>
                        </div>
                    </li>
                }.into_any()
            }
            Suggestion::Transfer { out, inc } => {
                let (o1, i1, o2, i2) = (out.clone(), inc.clone(), out.clone(), inc.clone());
                view! {
                    <li class="item" data-test="dup-transfer">
                        <p>"A transfer between your accounts?"</p>
                        <p class="hint">"Out: "{line(&d, &out)}</p>
                        <p class="hint">"In: "{line(&d, &inc)}</p>
                        <div class="row">
                            <button class="primary" data-test="transfer-yes" on:click=move |_| save(vec![
                                (o1.0, Transaction { kind: TxKind::Transfer, ..o1.1.clone() }),
                                (i1.0, Transaction { kind: TxKind::Transfer, ..i1.1.clone() }),
                            ])>"Yes, a transfer"</button>
                            <button data-test="transfer-no" on:click=move |_| save(vec![
                                (o2.0, Transaction { unique_confirmed: true, ..o2.1.clone() }),
                                (i2.0, Transaction { unique_confirmed: true, ..i2.1.clone() }),
                            ])>"No"</button>
                        </div>
                    </li>
                }.into_any()
            }
        }).collect_view().into_any()
    };
    view! {
        <header class="row"><h1>"Possible duplicates"</h1><button on:click=move |_| c.screen.set(Screen::Dashboard)>"Back"</button></header>
        <p class="hint">"A receipt and its card payment count once. Transfers between your own accounts count as neither income nor expense."</p>
        <ul class="items">{list}</ul>
    }
}
