//! Dashboard, operations and data management screens.

use crate::app::{Ctx, Screen, busy_task, reload, run_sync, today};
use crate::data::{self, Data};
use crate::flows::Session;
use js_sys::{Array, Uint8Array};
use leptos::prelude::*;
use mm_core::analytics::{self, CategoryLine, Dashboard, Filter, MonthTotal, TREND_MONTHS};
use mm_core::crypto::merchant_id;
use mm_core::date::{Date, Month};
use mm_core::export::{self, Lookup};
use mm_core::model::*;
use mm_core::money::{Qty, amount_input, format_amount, format_money, line_value, parse_amount};
use mm_core::record::{FIELD_DELETED, FieldConflict};
use serde_json::{Value, json};
use uuid::Uuid;
use wasm_bindgen::{JsCast, JsValue};

#[derive(Clone, Copy, PartialEq)]
pub enum Manage {
    Categories,
    Accounts,
    Merchants,
}

#[derive(Clone, Copy)]
pub struct FilterState {
    pub month: RwSignal<Month>,
    /// Empty means the month above. Set both to use a custom period.
    pub from: RwSignal<String>,
    pub to: RwSignal<String>,
    pub account: RwSignal<Option<Uuid>>,
    pub category: RwSignal<Option<Uuid>>,
}

impl FilterState {
    pub fn new(month: Month) -> Self {
        Self {
            month: RwSignal::new(month),
            from: RwSignal::new(String::new()),
            to: RwSignal::new(String::new()),
            account: RwSignal::new(None),
            category: RwSignal::new(None),
        }
    }

    fn period(&self) -> (Date, Date) {
        let m = self.month.get();
        match (Date::parse_iso(&self.from.get()), Date::parse_iso(&self.to.get())) {
            (Some(a), Some(b)) => (a, b),
            _ => (m.first(), m.last()),
        }
    }

    fn build(&self) -> Filter {
        let (from, to) = self.period();
        Filter { from: Some(from), to: Some(to), account: self.account.get(), category: self.category.get() }
    }
}

fn pln(minor: i64) -> String {
    format_money(minor, "PLN")
}

fn session(c: Ctx) -> Result<Session, String> {
    c.session.get_untracked().ok_or_else(|| "The app is locked".to_string())
}

async fn after_write(c: Ctx, s: &Session) {
    reload(c, s).await;
    run_sync(c).await;
}

pub fn nav(c: Ctx) -> impl IntoView {
    let on = move |want: Screen| move || c.screen.with(|s| *s == want);
    // Moving to another tab drops an error that belonged to the previous screen.
    let go = move |to: Screen| {
        c.error.set(String::new());
        c.screen.set(to);
    };
    view! {
        <nav class="tabs">
            <button data-test="nav-dashboard" class:active=on(Screen::Dashboard) on:click=move |_| go(Screen::Dashboard)>"Dashboard"</button>
            <button data-test="nav-ops" class:active=on(Screen::Operations) on:click=move |_| go(Screen::Operations)>"Operations"</button>
            <button class="photo" data-test="nav-photo" aria-label="Add a receipt" on:click=move |_| go(Screen::Edit(None))>
                <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 8h3l2-3h6l2 3h3v11H4z"/><circle cx="12" cy="13" r="3.5"/></svg>
            </button>
            <button data-test="nav-add" on:click=move |_| go(Screen::Edit(None))>"Add"</button>
            <button data-test="nav-settings" class:active=on(Screen::Settings) on:click=move |_| go(Screen::Settings)>"Settings"</button>
        </nav>
    }
}

fn banners(c: Ctx) -> impl IntoView {
    view! {
        <p class="hint" data-test="status">{move || c.status.get()}</p>
        {move || c.issues.get().into_iter().map(|i| view! { <p class="error" data-test="issue">{i}</p> }).collect_view()}
        {move || {
            let n = c.data.with(Data::conflicts);
            (n > 0).then(|| view! { <p class="error" data-test="conflict-banner">{format!("Operations to review: {n}. They changed on two devices.")}</p> })
        }}
    }
}

fn filters(c: Ctx) -> impl IntoView {
    let f = c.filter;
    let shift = move |n: i32| {
        f.from.set(String::new());
        f.to.set(String::new());
        f.month.update(|m| *m = m.offset(n));
    };
    let label = move || {
        if f.from.with(String::is_empty) || f.to.with(String::is_empty) {
            f.month.get().display()
        } else {
            "Custom period".to_string()
        }
    };
    view! {
        <div class="month">
            <button aria-label="Previous month" data-test="month-prev" on:click=move |_| shift(-1)>"‹"</button>
            <strong data-test="month">{label}</strong>
            <button aria-label="Next month" data-test="month-next" on:click=move |_| shift(1)>"›"</button>
        </div>
        <details class="filters">
            <summary>"Filters"</summary>
            <div class="row">
                <label>"From"
                    <input type="date" data-test="filter-from" prop:value=move || f.from.get() on:input=move |e| f.from.set(event_target_value(&e)) />
                </label>
                <label>"To"
                    <input type="date" data-test="filter-to" prop:value=move || f.to.get() on:input=move |e| f.to.set(event_target_value(&e)) />
                </label>
            </div>
            <label>"Account"
                <select data-test="filter-account" on:change=move |e| f.account.set(event_target_value(&e).parse().ok())>
                    <option value="" selected=move || f.account.get().is_none()>"All accounts"</option>
                    {move || c.data.with(Data::account_list).into_iter().map(|(id, a)| view! {
                        <option value=id.to_string() selected=move || f.account.get() == Some(id)>{a.name}</option>
                    }).collect_view()}
                </select>
            </label>
            <label>"Category"
                <select data-test="filter-category" on:change=move |e| f.category.set(event_target_value(&e).parse().ok())>
                    <option value="" selected=move || f.category.get().is_none()>"All categories"</option>
                    {move || c.data.with(Data::active_categories).into_iter().map(|(id, x)| view! {
                        <option value=id.to_string() selected=move || f.category.get() == Some(id)>{x.name}</option>
                    }).collect_view()}
                </select>
            </label>
        </details>
    }
}

fn bar(value: i64, max: i64) -> impl IntoView {
    // Whole-number percent is enough for a bar. The exact amount is printed beside it.
    let pct = if max > 0 { (value as i128 * 100 / max as i128).max(1) as i64 } else { 0 };
    view! {
        <svg class="bar" viewBox="0 0 100 4" preserveAspectRatio="none" aria-hidden="true">
            <rect class="track" width="100" height="4"/>
            <rect class="fill" width=pct.to_string() height="4"/>
        </svg>
    }
}

fn scale(value: i64, max: i64, height: i64) -> i64 {
    if max <= 0 || value <= 0 {
        return 0;
    }
    (value as i128 * height as i128 / max as i128).max(2) as i64
}

fn trend_chart(rows: &[MonthTotal]) -> impl IntoView + use<> {
    const H: i64 = 110;
    const STEP: i64 = 53;
    const W: i64 = 18;
    let max = rows.iter().map(|m| m.income.max(m.expense)).max().unwrap_or(0);
    let marks = rows.iter().enumerate().map(|(i, m)| {
        let x = i as i64 * STEP + 3;
        let name = m.month.map(Month::short_name).unwrap_or("");
        let (ih, eh) = (scale(m.income, max, H), scale(m.expense, max, H));
        view! {
            <g>
                <rect class="income" x=x.to_string() y=(H - ih).to_string() width=W.to_string() height=ih.to_string() rx="2">
                    <title>{format!("{name} income: {}", pln(m.income))}</title>
                </rect>
                <rect class="expense" x=(x + W + 2).to_string() y=(H - eh).to_string() width=W.to_string() height=eh.to_string() rx="2">
                    <title>{format!("{name} expenses: {}", pln(m.expense))}</title>
                </rect>
                <text x=(x + W).to_string() y="128" text-anchor="middle">{name}</text>
            </g>
        }
    }).collect_view();
    let viewbox = format!("0 0 {} 134", TREND_MONTHS as i64 * STEP);
    view! {
        <svg class="trend" data-test="trend" viewBox=viewbox role="img" aria-label="Income and expenses over six months">
            <line class="base" x1="0" x2="318" y1=H.to_string() y2=H.to_string()/>
            {marks}
        </svg>
    }
}

fn trend_table(rows: &[MonthTotal]) -> impl IntoView + use<> {
    let body = rows.iter().rev().map(|m| view! {
        <tr>
            <td>{m.month.map(Month::display).unwrap_or_default()}</td>
            <td class="num">{format_amount(m.income)}</td>
            <td class="num">{format_amount(m.expense)}</td>
        </tr>
    }).collect_view();
    view! {
        <details class="table">
            <summary>"Show as a table"</summary>
            <table data-test="trend-table">
                <thead><tr><th>"Month"</th><th class="num">"Income"</th><th class="num">"Expenses"</th></tr></thead>
                <tbody>{body}</tbody>
            </table>
        </details>
    }
}

fn tx_row(c: Ctx, d: &Data, id: Uuid, t: &Transaction) -> impl IntoView + use<> {
    let meta = d.meta.get(&id).cloned().unwrap_or_default();
    let title = d.merchant_name(t.merchant_id).unwrap_or_else(|| {
        if t.note.is_empty() { d.category_name(t.category_id) } else { t.note.clone() }
    });
    let income = t.kind == TxKind::Income;
    let sign = if income { "+" } else { "−" };
    let amount = format!("{sign}{}", format_money(t.amount, &t.currency));
    let second = (t.currency != "PLN").then(|| pln(t.amount_pln));
    view! {
        <li>
            <button class="txrow" data-test="tx-row" on:click=move |_| c.screen.set(Screen::Edit(Some(id)))>
                <span class="grow">
                    <span class="name" data-test="tx-title">{title}</span>
                    <span class="hint">{t.date.display()}" · "{d.category_name(t.category_id)}</span>
                    {meta.pending.then(|| view! { <span class="hint" data-test="pending">"Waiting to send"</span> })}
                    {(!meta.conflicts.is_empty()).then(|| view! { <span class="error" data-test="tx-conflict">"Needs a choice"</span> })}
                </span>
                <span class="amount" class:income=income>
                    <span data-test="tx-amount">{amount}</span>
                    {second.map(|s| view! { <span class="hint">{s}</span> })}
                </span>
            </button>
        </li>
    }
}

pub fn dashboard(c: Ctx) -> impl IntoView {
    let report = move || {
        let f = c.filter.build();
        let month = c.filter.month.get();
        c.data.with(|d| analytics::dashboard(&d.txs, &d.categories, &f, month))
    };
    let body = move || {
        let r: Dashboard = report();
        let d = c.data.get();
        let max_cat = r.by_category.first().map(|x| x.1).unwrap_or(0);
        let max_shop = r.top_merchants.first().map(|x| x.1).unwrap_or(0);
        let empty = d.txs.is_empty();
        let cats: Vec<_> = r.by_category.iter().map(|(id, v)| {
            let id = *id;
            view! {
                <li>
                    <button class="barrow" data-test="cat-row" on:click=move |_| c.screen.set(Screen::Category(id))>
                        <span class="line"><span>{d.category_name(id)}</span><span class="num">{pln(*v)}</span></span>
                        {bar(*v, max_cat)}
                    </button>
                </li>
            }
        }).collect();
        let shops: Vec<_> = r.top_merchants.iter().map(|(id, v)| view! {
            <li class="barrow" data-test="top-merchant">
                <span class="line"><span>{d.merchant_name(*id).unwrap_or_default()}</span><span class="num">{pln(*v)}</span></span>
                {bar(*v, max_shop)}
            </li>
        }).collect();
        let recent: Vec<_> = r.recent.iter().filter_map(|id| d.txs.iter().find(|t| t.0 == *id)).map(|(id, t)| tx_row(c, &d, *id, t)).collect();
        let has_cats = !r.by_category.is_empty();
        let has_shops = !r.top_merchants.is_empty();
        let has_recent = !r.recent.is_empty();
        view! {
            <section class="hero">
                <span class="hint">"Balance"</span>
                <p class="big" data-test="dash-balance">{pln(r.balance)}</p>
                <div class="row split">
                    <span><span class="hint">"Income"</span><strong data-test="dash-income">{pln(r.income)}</strong></span>
                    <span><span class="hint">"Expenses"</span><strong data-test="dash-expense">{pln(r.expense)}</strong></span>
                </div>
            </section>
            {empty.then(|| view! {
                <p class="hint" data-test="empty">"No operations yet."</p>
                <button class="primary" data-test="add-first" on:click=move |_| c.screen.set(Screen::Edit(None))>"Add the first one"</button>
            })}
            {has_cats.then(|| view! { <h2>"By category"</h2><ul class="bars">{cats}</ul> })}
            <h2>"Last six months"</h2>
            <div class="legend" aria-hidden="true"><span class="key income"></span>"Income"<span class="key expense"></span>"Expenses"</div>
            {trend_chart(&r.trend)}
            {trend_table(&r.trend)}
            {has_shops.then(|| view! { <h2>"Top merchants"</h2><ul class="bars">{shops}</ul> })}
            <h2>"Subscriptions"</h2>
            <p class="hint">"Regular payments will appear here after bank statements can be imported."</p>
            {has_recent.then(|| view! { <h2>"Recent"</h2><ul class="txs">{recent}</ul> })}
        }
    };
    view! {
        <header class="row"><h1>"Dashboard"</h1></header>
        {banners(c)}
        {filters(c)}
        {body}
    }
}

pub fn operations(c: Ctx) -> impl IntoView {
    let list = move || {
        let f = c.filter.build();
        let d = c.data.get();
        let rows = analytics::filter_txs(&d.txs, &f, &d.categories);
        if rows.is_empty() {
            return view! { <p class="hint" data-test="empty-list">"Nothing for this period."</p> }.into_any();
        }
        let items: Vec<_> = rows.iter().map(|(id, t)| tx_row(c, &d, *id, t)).collect();
        view! { <ul class="txs">{items}</ul> }.into_any()
    };
    view! {
        <header class="row">
            <h1>"Operations"</h1>
            <button class="primary" data-test="add-tx" on:click=move |_| c.screen.set(Screen::Edit(None))>"Add"</button>
        </header>
        {banners(c)}
        {filters(c)}
        {list}
    }
}

pub fn category(c: Ctx, id: Uuid) -> impl IntoView {
    let title = c.data.with_untracked(|d| d.category_name(id));
    let list = move || {
        let f = c.filter.build();
        let d = c.data.get();
        let lines: Vec<CategoryLine> = analytics::category_lines(&d.txs, &d.categories, &f, id);
        lines.into_iter().map(|l| {
            let tx = l.tx;
            let name = l.name.clone().or_else(|| d.txs.iter().find(|t| t.0 == tx).and_then(|t| d.merchant_name(t.1.merchant_id))).unwrap_or_else(|| "Operation".into());
            view! {
                <li>
                    <button class="txrow" data-test="line-row" on:click=move |_| c.screen.set(Screen::Edit(Some(tx)))>
                        <span class="grow"><span class="name">{name}</span><span class="hint">{l.date.display()}</span></span>
                        <span class="amount"><span>{pln(l.amount_pln)}</span></span>
                    </button>
                </li>
            }
        }).collect_view()
    };
    view! {
        <header class="row">
            <h1 data-test="category-title">{title}</h1>
            <button on:click=move |_| c.screen.set(Screen::Dashboard)>"Back"</button>
        </header>
        <ul class="txs">{list}</ul>
    }
}

/// A validated operation, a merchant to create if it is new, and the session to save with.
type Prepared = (Transaction, Option<(Uuid, Merchant)>, Session);

#[derive(Clone)]
struct ItemDraft {
    key: Uuid,
    raw_name: String,
    name: RwSignal<String>,
    qty: RwSignal<String>,
    unit: RwSignal<String>,
    price: RwSignal<String>,
    discount: RwSignal<String>,
    vat: RwSignal<String>,
    category: RwSignal<Uuid>,
}

impl ItemDraft {
    fn blank(category: Uuid) -> Self {
        Self {
            key: Uuid::new_v4(),
            raw_name: String::new(),
            name: RwSignal::new(String::new()),
            qty: RwSignal::new("1".into()),
            unit: RwSignal::new("pcs".into()),
            price: RwSignal::new(String::new()),
            discount: RwSignal::new(String::new()),
            vat: RwSignal::new(String::new()),
            category: RwSignal::new(category),
        }
    }

    fn from(i: &LineItem) -> Self {
        Self {
            key: Uuid::new_v4(),
            raw_name: i.raw_name.clone(),
            name: RwSignal::new(i.name.clone()),
            qty: RwSignal::new(i.qty.display()),
            unit: RwSignal::new(i.unit.clone()),
            price: RwSignal::new(amount_input(i.unit_price)),
            discount: RwSignal::new(if i.discount == 0 { String::new() } else { amount_input(i.discount) }),
            vat: RwSignal::new(i.vat_rate.map(|r| amount_input(r as i64)).unwrap_or_default()),
            category: RwSignal::new(i.category_id),
        }
    }

    fn build(&self) -> Result<LineItem, String> {
        let name = self.name.get_untracked().trim().to_string();
        let qty = Qty::parse(&self.qty.get_untracked()).map_err(|_| format!("Check the quantity of \"{name}\""))?;
        let unit_price = parse_amount(&self.price.get_untracked()).map_err(|_| format!("Check the price of \"{name}\""))?;
        let d = self.discount.get_untracked();
        let discount = if d.trim().is_empty() { 0 } else { parse_amount(&d).map_err(|_| format!("Check the discount of \"{name}\""))? };
        let v = self.vat.get_untracked();
        let vat_rate = if v.trim().is_empty() {
            None
        } else {
            Some(parse_amount(&v).map_err(|_| format!("Check the VAT rate of \"{name}\""))? as u32)
        };
        let total = line_value(qty, unit_price).and_then(|t| t.checked_sub(discount)).filter(|t| *t >= 0)
            .ok_or_else(|| format!("The discount of \"{name}\" is larger than its value"))?;
        let raw_name = if self.raw_name.is_empty() { name.clone() } else { self.raw_name.clone() };
        Ok(LineItem { raw_name, name, qty, unit: self.unit.get_untracked().trim().to_string(), unit_price, discount, total, vat_rate, category_id: self.category.get_untracked() })
    }
}

fn category_select(c: Ctx, sig: RwSignal<Uuid>, test: &'static str) -> impl IntoView {
    view! {
        <select data-test=test on:change=move |e| if let Ok(id) = event_target_value(&e).parse() { sig.set(id) }>
            {move || c.data.with(Data::active_categories).into_iter().map(|(id, x)| view! {
                <option value=id.to_string() selected=move || analytics_same(c, sig.get(), id)>{x.name}</option>
            }).collect_view()}
        </select>
    }
}

fn analytics_same(c: Ctx, a: Uuid, b: Uuid) -> bool {
    c.data.with_untracked(|d| analytics::canonical(&d.categories, a) == b)
}

fn conflict_value(field: &str, v: &Value) -> String {
    match field {
        FIELD_DELETED => if v.as_bool() == Some(true) { "Deleted".into() } else { "Kept".into() },
        "amount" | "amount_pln" => format_amount(v.as_i64().unwrap_or(0)),
        "items" => format!("{} items", v.as_array().map(Vec::len).unwrap_or(0)),
        _ => match v {
            Value::Null => "Empty".into(),
            Value::String(s) => s.clone(),
            other => other.to_string(),
        },
    }
}

fn conflict_box(c: Ctx, id: Uuid, f: FieldConflict) -> impl IntoView {
    let (a, b) = (f.field.clone(), f.field.clone());
    let title = if f.field == FIELD_DELETED {
        "Deleted on one device and changed on another".to_string()
    } else {
        format!("\"{}\" changed on two devices", f.field.replace('_', " "))
    };
    let (mine, theirs) = (conflict_value(&f.field, &f.mine), conflict_value(&f.field, &f.theirs));
    let pick = move |field: String, keep_theirs: bool| busy_task(c, async move {
        let s = session(c)?;
        data::resolve(&s, id, &field, keep_theirs).await?;
        // The form still shows the values from before the choice, so saving it would undo it.
        c.screen.set(Screen::Operations);
        after_write(c, &s).await;
        Ok(())
    });
    view! {
        <div class="conflict" data-test="conflict">
            <p>{title}". Choose a version:"</p>
            <button data-test="keep-mine" on:click=move |_| pick(a.clone(), false)>{mine}</button>
            <button data-test="keep-theirs" on:click=move |_| pick(b.clone(), true)>{theirs}</button>
        </div>
    }
}

pub fn edit(c: Ctx, id: Option<Uuid>) -> impl IntoView {
    let d = c.data.get_untracked();
    let old: Option<Transaction> = id.and_then(|i| d.txs.iter().find(|t| t.0 == i).map(|t| t.1.clone()));
    let conflicts = id.and_then(|i| d.meta.get(&i)).map(|m| m.conflicts.clone()).unwrap_or_default();
    let default_account = d.account_list().first().map(|a| a.0).unwrap_or(ACCOUNT_MAIN);
    let o = old.as_ref();

    let kind = RwSignal::new(o.map(|t| t.kind).unwrap_or_default());
    let date = RwSignal::new(o.map(|t| t.date).unwrap_or_else(today).iso());
    let time = RwSignal::new(o.and_then(|t| t.time.clone()).unwrap_or_default());
    let merchant = RwSignal::new(o.and_then(|t| d.merchant_name(t.merchant_id)).unwrap_or_default());
    let nip = RwSignal::new(o.and_then(|t| t.nip.clone()).unwrap_or_default());
    let amount = RwSignal::new(o.map(|t| amount_input(t.amount)).unwrap_or_default());
    let currency = RwSignal::new(o.map(|t| t.currency.clone()).unwrap_or_else(|| "PLN".into()));
    let amount_pln = RwSignal::new(o.filter(|t| t.currency != "PLN").map(|t| amount_input(t.amount_pln)).unwrap_or_default());
    let payment = RwSignal::new(o.map(|t| t.payment).unwrap_or_default());
    let account = RwSignal::new(o.map(|t| t.account_id).unwrap_or(default_account));
    let category = RwSignal::new(o.map(|t| t.category_id).unwrap_or(CAT_OTHER));
    let note = RwSignal::new(o.map(|t| t.note.clone()).unwrap_or_default());
    let items = RwSignal::new(o.map(|t| t.items.iter().map(ItemDraft::from).collect::<Vec<_>>()).unwrap_or_default());
    let (source, review) = (o.map(|t| t.source).unwrap_or_default(), o.map(|t| t.review).unwrap_or_default());
    let form_error = RwSignal::new(String::new());

    // With items the amount is their sum, so the two can never disagree.
    let items_sum = move || {
        items.with(|v| {
            if v.is_empty() {
                return None;
            }
            Some(v.iter().try_fold(0i64, |sum, i| {
                let q = Qty::parse(&i.qty.get()).ok()?;
                let p = parse_amount(&i.price.get()).ok()?;
                let dc = i.discount.with(|s| if s.trim().is_empty() { Some(0) } else { parse_amount(s).ok() })?;
                sum.checked_add(line_value(q, p)?.checked_sub(dc)?)
            }))
        })
    };

    let save = move |_| {
        let build = move || -> Result<Prepared, String> {
            let s = session(c)?;
            let date = Date::parse_iso(&date.get_untracked()).ok_or("Enter a date")?;
            let lines = items.get_untracked().iter().map(ItemDraft::build).collect::<Result<Vec<_>, _>>()?;
            let currency = currency.get_untracked().trim().to_uppercase();
            let total = if lines.is_empty() {
                parse_amount(&amount.get_untracked()).map_err(|_| "Enter the amount, like 12,34".to_string())?
            } else {
                lines.iter().try_fold(0i64, |a, l| a.checked_add(l.total)).ok_or("The amount is too large")?
            };
            let in_pln = if currency == "PLN" {
                total
            } else {
                parse_amount(&amount_pln.get_untracked()).map_err(|_| "Enter the amount in PLN".to_string())?
            };
            let name = merchant.get_untracked().trim().to_string();
            let nip_s = nip.get_untracked().trim().to_string();
            let mut new_merchant = None;
            let merchant_id = (!name.is_empty()).then(|| {
                let id = merchant_id(&s.dek, &name);
                let known = c.data.with_untracked(|d| d.merchants.get(&id).cloned());
                let nip = (!nip_s.is_empty()).then(|| nip_s.clone());
                if known.is_none() {
                    new_merchant = Some((id, Merchant { name: name.clone(), nip }));
                }
                id
            });
            let t = Transaction {
                kind: kind.get_untracked(),
                date,
                time: Some(time.get_untracked()).filter(|t| !t.is_empty()),
                merchant_id,
                nip: Some(nip_s).filter(|n| !n.is_empty()),
                amount: total,
                currency,
                amount_pln: in_pln,
                payment: payment.get_untracked(),
                account_id: account.get_untracked(),
                category_id: category.get_untracked(),
                source,
                review,
                note: note.get_untracked().trim().to_string(),
                items: lines,
            };
            t.validate().map_err(|e| e.to_string())?;
            Ok((t, new_merchant, s))
        };
        match build() {
            Err(e) => form_error.set(e),
            Ok((t, m, s)) => {
                form_error.set(String::new());
                busy_task(c, async move {
                    if let Some((mid, m)) = m {
                        data::write(&s, mid, KIND_MERCHANT, &m).await?;
                    }
                    data::write(&s, id.unwrap_or_else(Uuid::new_v4), KIND_TRANSACTION, &t).await?;
                    c.screen.set(Screen::Operations);
                    after_write(c, &s).await;
                    Ok(())
                });
            }
        }
    };

    let delete = move |_| busy_task(c, async move {
        let s = session(c)?;
        if let Some(i) = id {
            data::remove(&s, i, KIND_TRANSACTION, json!({})).await?;
        }
        c.screen.set(Screen::Operations);
        after_write(c, &s).await;
        Ok(())
    });

    let boxes = conflicts.into_iter().filter_map(|f| id.map(|i| conflict_box(c, i, f))).collect_view();
    let item_row = move |i: ItemDraft| {
        let key = i.key;
        view! {
            <li class="item" data-test="item">
                <label>"Name"<input data-test="item-name" prop:value=move || i.name.get() on:input=move |e| i.name.set(event_target_value(&e)) /></label>
                <div class="row">
                    <label>"Quantity"<input data-test="item-qty" inputmode="decimal" prop:value=move || i.qty.get() on:input=move |e| i.qty.set(event_target_value(&e)) /></label>
                    <label>"Unit"<input data-test="item-unit" prop:value=move || i.unit.get() on:input=move |e| i.unit.set(event_target_value(&e)) /></label>
                </div>
                <div class="row">
                    <label>"Price"<input data-test="item-price" inputmode="decimal" prop:value=move || i.price.get() on:input=move |e| i.price.set(event_target_value(&e)) /></label>
                    <label>"Discount"<input data-test="item-discount" inputmode="decimal" prop:value=move || i.discount.get() on:input=move |e| i.discount.set(event_target_value(&e)) /></label>
                </div>
                <div class="row">
                    <label>"VAT, %"<input data-test="item-vat" inputmode="decimal" prop:value=move || i.vat.get() on:input=move |e| i.vat.set(event_target_value(&e)) /></label>
                    <label>"Category"{category_select(c, i.category, "item-category")}</label>
                </div>
                <button data-test="item-remove" on:click=move |_| items.update(|v| v.retain(|x| x.key != key))>"Remove item"</button>
            </li>
        }
    };

    view! {
        <header class="row">
            <h1>{if id.is_some() { "Edit operation" } else { "New operation" }}</h1>
            <button data-test="form-back" on:click=move |_| c.screen.set(Screen::Operations)>"Back"</button>
        </header>
        {boxes}
        <div class="segmented">
            <button data-test="kind-expense" class:active=move || kind.get() == TxKind::Expense on:click=move |_| kind.set(TxKind::Expense)>"Expense"</button>
            <button data-test="kind-income" class:active=move || kind.get() == TxKind::Income on:click=move |_| kind.set(TxKind::Income)>"Income"</button>
        </div>
        {move || match items_sum() {
            None => view! {
                <label>"Amount"
                    <input class="big-input" data-test="f-amount" inputmode="decimal" placeholder="0,00"
                        prop:value=move || amount.get() on:input=move |e| amount.set(event_target_value(&e)) />
                </label>
            }.into_any(),
            Some(sum) => view! {
                <p>"Amount: "<strong data-test="f-sum">{sum.map(format_amount).unwrap_or_else(|| "check the items".into())}</strong></p>
            }.into_any(),
        }}
        <div class="row">
            <label>"Currency"<input data-test="f-currency" maxlength="3" autocapitalize="characters" prop:value=move || currency.get() on:input=move |e| currency.set(event_target_value(&e)) /></label>
            {move || (currency.get().trim().to_uppercase() != "PLN").then(|| view! {
                <label>"In PLN"<input data-test="f-pln" inputmode="decimal" prop:value=move || amount_pln.get() on:input=move |e| amount_pln.set(event_target_value(&e)) /></label>
            })}
        </div>
        <div class="row">
            <label>"Date"<input type="date" data-test="f-date" prop:value=move || date.get() on:input=move |e| date.set(event_target_value(&e)) /></label>
            <label>"Time"<input type="time" data-test="f-time" prop:value=move || time.get() on:input=move |e| time.set(event_target_value(&e)) /></label>
        </div>
        <label>"Merchant"
            <input data-test="f-merchant" list="merchants" autocomplete="off" prop:value=move || merchant.get() on:input=move |e| merchant.set(event_target_value(&e)) />
            <datalist id="merchants">
                {move || c.data.with(Data::merchant_list).into_iter().map(|(_, m)| view! { <option value=m.name></option> }).collect_view()}
            </datalist>
        </label>
        <label>"NIP"<input data-test="f-nip" inputmode="numeric" prop:value=move || nip.get() on:input=move |e| nip.set(event_target_value(&e)) /></label>
        <label>"Category"{category_select(c, category, "f-category")}</label>
        <div class="row">
            <label>"Account"
                <select data-test="f-account" on:change=move |e| if let Ok(i) = event_target_value(&e).parse() { account.set(i) }>
                    {move || c.data.with(Data::account_list).into_iter().map(|(i, a)| view! {
                        <option value=i.to_string() selected=move || account.get() == i>{a.name}</option>
                    }).collect_view()}
                </select>
            </label>
            <label>"Payment"
                <select data-test="f-payment" on:change=move |e| payment.set(Payment::from_code(&event_target_value(&e)))>
                    {Payment::ALL.into_iter().map(|p| view! {
                        <option value=p.code() selected=move || payment.get() == p>{p.label()}</option>
                    }).collect_view()}
                </select>
            </label>
        </div>
        <label>"Note"<input data-test="f-note" prop:value=move || note.get() on:input=move |e| note.set(event_target_value(&e)) /></label>
        <h2>"Items"</h2>
        <ul class="items">
            <For each=move || items.get() key=|i| i.key children=item_row />
        </ul>
        <button data-test="add-item" on:click=move |_| items.update(|v| v.push(ItemDraft::blank(category.get_untracked())))>"Add item"</button>
        {move || (!form_error.get().is_empty()).then(|| view! { <p class="error" data-test="form-error">{form_error.get()}</p> })}
        <button class="primary" data-test="f-save" disabled=move || c.busy.get() on:click=save>"Save"</button>
        {id.is_some().then(|| view! { <button class="danger" data-test="f-delete" disabled=move || c.busy.get() on:click=delete>"Delete"</button> })}
    }
}

pub fn manage(c: Ctx, m: Manage) -> impl IntoView {
    let title = match m {
        Manage::Categories => "Categories",
        Manage::Accounts => "Accounts",
        Manage::Merchants => "Merchants",
    };
    let back = move |_| c.screen.set(Screen::Settings);
    let list = move || match m {
        Manage::Categories => categories(c).into_any(),
        Manage::Accounts => accounts(c).into_any(),
        Manage::Merchants => merchants(c).into_any(),
    };
    view! {
        <header class="row"><h1>{title}</h1><button data-test="manage-back" on:click=back>"Back"</button></header>
        {list}
    }
}

fn categories(c: Ctx) -> impl IntoView {
    let draft = RwSignal::new(String::new());
    let add = move |_| {
        let name = draft.get_untracked().trim().to_string();
        if name.is_empty() {
            return;
        }
        draft.set(String::new());
        busy_task(c, async move {
            let s = session(c)?;
            data::write(&s, Uuid::new_v4(), KIND_CATEGORY, &Category { name, merged_into: None }).await?;
            after_write(c, &s).await;
            Ok(())
        });
    };
    let rows = move || {
        let all = c.data.with(Data::active_categories);
        all.iter().cloned().map(|(id, cat)| {
            let name = RwSignal::new(cat.name.clone());
            let target = RwSignal::new(if id == CAT_OTHER { all.iter().find(|x| x.0 != id).map(|x| x.0).unwrap_or(id) } else { CAT_OTHER });
            let others: Vec<(Uuid, Category)> = all.iter().filter(|x| x.0 != id).cloned().collect();
            let rename = move |_| busy_task(c, async move {
                let s = session(c)?;
                data::write(&s, id, KIND_CATEGORY, &Category { name: name.get_untracked().trim().to_string(), merged_into: None }).await?;
                after_write(c, &s).await;
                Ok(())
            });
            let original = cat.name.clone();
            let merge = move |_| {
                let original = original.clone();
                busy_task(c, async move {
                    let s = session(c)?;
                    data::write(&s, id, KIND_CATEGORY, &Category { name: original, merged_into: Some(target.get_untracked()) }).await?;
                    after_write(c, &s).await;
                    Ok(())
                });
            };
            view! {
                <li class="item" data-test="category-item">
                    <input data-test="category-name" prop:value=move || name.get() on:input=move |e| name.set(event_target_value(&e)) />
                    <button data-test="category-save" on:click=rename>"Rename"</button>
                    <label>"Merge into"
                        <select data-test="merge-target" on:change=move |e| if let Ok(i) = event_target_value(&e).parse() { target.set(i) }>
                            {others.into_iter().map(|(i, x)| view! { <option value=i.to_string() selected=move || target.get() == i>{x.name}</option> }).collect_view()}
                        </select>
                    </label>
                    <button data-test="merge" on:click=merge>"Merge"</button>
                </li>
            }
        }).collect_view()
    };
    view! {
        <div class="row">
            <input data-test="new-category" placeholder="New category" prop:value=move || draft.get() on:input=move |e| draft.set(event_target_value(&e)) />
            <button class="primary" data-test="add-category" on:click=add>"Add"</button>
        </div>
        <ul class="items">{rows}</ul>
    }
}

fn accounts(c: Ctx) -> impl IntoView {
    let draft = RwSignal::new(String::new());
    let add = move |_| {
        let name = draft.get_untracked().trim().to_string();
        if name.is_empty() {
            return;
        }
        draft.set(String::new());
        busy_task(c, async move {
            let s = session(c)?;
            data::write(&s, Uuid::new_v4(), KIND_ACCOUNT, &Account { name, currency: "PLN".into() }).await?;
            after_write(c, &s).await;
            Ok(())
        });
    };
    let rows = move || {
        let list = c.data.with(Data::account_list);
        let last = list.len() <= 1;
        list.into_iter().map(|(id, a)| {
            let name = RwSignal::new(a.name.clone());
            let currency = a.currency.clone();
            let rename = move |_| {
                let currency = currency.clone();
                busy_task(c, async move {
                    let s = session(c)?;
                    data::write(&s, id, KIND_ACCOUNT, &Account { name: name.get_untracked().trim().to_string(), currency }).await?;
                    after_write(c, &s).await;
                    Ok(())
                });
            };
            let delete = move |_| busy_task(c, async move {
                let used = c.data.with_untracked(|d| d.txs.iter().filter(|t| t.1.account_id == id).count());
                if used > 0 {
                    return Err(format!("This account has {used} operations. Move them first"));
                }
                let s = session(c)?;
                data::remove(&s, id, KIND_ACCOUNT, json!({ "name": "", "currency": "PLN" })).await?;
                after_write(c, &s).await;
                Ok(())
            });
            view! {
                <li class="item" data-test="account-item">
                    <input data-test="account-name" prop:value=move || name.get() on:input=move |e| name.set(event_target_value(&e)) />
                    <div class="row">
                        <button data-test="account-save" on:click=rename>"Rename"</button>
                        {(!last).then(|| view! { <button data-test="account-delete" on:click=delete>"Delete"</button> })}
                    </div>
                </li>
            }
        }).collect_view()
    };
    view! {
        <div class="row">
            <input data-test="new-account" placeholder="New account" prop:value=move || draft.get() on:input=move |e| draft.set(event_target_value(&e)) />
            <button class="primary" data-test="add-account" on:click=add>"Add"</button>
        </div>
        <ul class="items">{rows}</ul>
    }
}

fn merchants(c: Ctx) -> impl IntoView {
    let rows = move || {
        c.data.with(Data::merchant_list).into_iter().map(|(id, m)| {
            let name = RwSignal::new(m.name.clone());
            let nip = RwSignal::new(m.nip.clone().unwrap_or_default());
            let save = move |_| busy_task(c, async move {
                let n = nip.get_untracked().trim().to_string();
                if !n.is_empty() && !valid_nip(&n) {
                    return Err("NIP must have 10 digits and a valid check digit".to_string());
                }
                let s = session(c)?;
                let m = Merchant { name: name.get_untracked().trim().to_string(), nip: Some(n).filter(|x| !x.is_empty()) };
                data::write(&s, id, KIND_MERCHANT, &m).await?;
                after_write(c, &s).await;
                Ok(())
            });
            let delete = move |_| busy_task(c, async move {
                let used = c.data.with_untracked(|d| d.txs.iter().filter(|t| t.1.merchant_id == Some(id)).count());
                if used > 0 {
                    return Err(format!("This merchant has {used} operations"));
                }
                let s = session(c)?;
                data::remove(&s, id, KIND_MERCHANT, json!({ "name": "" })).await?;
                after_write(c, &s).await;
                Ok(())
            });
            view! {
                <li class="item" data-test="merchant-item">
                    <input data-test="merchant-name" prop:value=move || name.get() on:input=move |e| name.set(event_target_value(&e)) />
                    <label>"NIP"<input data-test="merchant-nip" inputmode="numeric" prop:value=move || nip.get() on:input=move |e| nip.set(event_target_value(&e)) /></label>
                    <div class="row">
                        <button data-test="merchant-save" on:click=save>"Save"</button>
                        <button data-test="merchant-delete" on:click=delete>"Delete"</button>
                    </div>
                </li>
            }
        }).collect_view()
    };
    view! {
        <p class="hint">"Merchants are added when you enter an operation."</p>
        <ul class="items">{rows}</ul>
    }
}

fn download(name: &str, mime: &str, text: &str) -> Result<(), String> {
    let e = |x: JsValue| format!("{x:?}");
    let parts = Array::of1(&Uint8Array::from(text.as_bytes()));
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type(mime);
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts).map_err(e)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(e)?;
    let doc = web_sys::window().and_then(|w| w.document()).ok_or("no document")?;
    let a: web_sys::HtmlAnchorElement = doc.create_element("a").map_err(e)?.dyn_into().map_err(|_| "no anchor")?;
    a.set_href(&url);
    a.set_download(name);
    a.click();
    web_sys::Url::revoke_object_url(&url).map_err(e)
}

pub fn export(c: Ctx) -> impl IntoView {
    let run = move |json: bool| {
        let d = c.data.get_untracked();
        let l = Lookup { categories: &d.categories, accounts: &d.accounts, merchants: &d.merchants };
        let day = today();
        let res = if json {
            let body = export::to_json(&d.txs, &l, day);
            download(&format!("mm-{}.json", day.iso()), "application/json", &serde_json::to_string_pretty(&body).unwrap_or_default())
        } else {
            download(&format!("mm-{}.csv", day.iso()), "text/csv;charset=utf-8", &export::to_csv(&d.txs, &l))
        };
        if let Err(e) = res {
            c.error.set(e);
        }
    };
    view! {
        <header class="row">
            <h1>"Export data"</h1>
            <button on:click=move |_| c.screen.set(Screen::Settings)>"Back"</button>
        </header>
        <p>"The file is created on this device and is not encrypted. Anyone who gets it can read all of your data."</p>
        <button data-test="export-csv" on:click=move |_| run(false)>"Download CSV"</button>
        <button data-test="export-json" on:click=move |_| run(true)>"Download JSON"</button>
    }
}
