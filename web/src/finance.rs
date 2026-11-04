//! Dashboard, operations and data management screens.

use crate::app::{Ctx, Screen, busy_task, reload, run_sync, today};
use crate::data::{self, Data};
use crate::flows::Session;
use crate::icons::{Icon, icon};
use js_sys::{Array, Uint8Array};
use leptos::prelude::*;
use mm_core::analytics::{self, CategoryLine, Dashboard, Filter, MonthTotal, TREND_MONTHS, Tx};
use mm_core::categorize::fold;
use mm_core::crypto::{merchant_id, named_id};
use mm_core::date::{Date, Month};
use mm_core::export::{self, Lookup};
use mm_core::model::*;
use mm_core::money::{Qty, amount_input, convert, format_amount, format_money, line_value, parse_amount};
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

/// Fields the parser was not sure about. Only these are highlighted.
#[derive(Clone, Debug, Default)]
pub struct Doubts {
    pub merchant: bool,
    pub nip: bool,
    pub date: bool,
    pub total: bool,
    pub items: Vec<bool>,
}

/// A recognized receipt. It holds parsed values only, never the photo.
#[derive(Clone, Debug)]
pub struct Draft {
    pub tx: Transaction,
    pub merchant: String,
    pub doubts: Doubts,
    /// Categories the app suggested, to learn from the user's corrections.
    pub suggested: Vec<Uuid>,
}

#[derive(Clone, Copy)]
pub struct FilterState {
    pub month: RwSignal<Month>,
    /// Empty means the month above. Set both to use a custom period.
    pub from: RwSignal<String>,
    pub to: RwSignal<String>,
    pub account: RwSignal<Option<Uuid>>,
    pub category: RwSignal<Option<Uuid>>,
    pub tab: RwSignal<HomeTab>,
    /// The calendar is open.
    pub picker: RwSignal<bool>,
    /// A day picked in the calendar: Home scrolls to it once.
    pub jump: RwSignal<Option<Date>>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum HomeTab {
    #[default]
    Daily,
    Monthly,
    Summary,
}

impl FilterState {
    pub fn new(month: Month) -> Self {
        Self {
            month: RwSignal::new(month),
            from: RwSignal::new(String::new()),
            to: RwSignal::new(String::new()),
            account: RwSignal::new(None),
            category: RwSignal::new(None),
            tab: RwSignal::new(HomeTab::Daily),
            picker: RwSignal::new(false),
            jump: RwSignal::new(None),
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

/// Shows the change at once. Sync runs in the background, so a slow or missing network
/// never keeps the buttons disabled.
async fn after_write(c: Ctx, s: &Session) {
    reload(c, s).await;
    leptos::task::spawn_local(run_sync(c));
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
            <button data-test="nav-dashboard" aria-label="Home" class:active=on(Screen::Dashboard) on:click=move |_| go(Screen::Dashboard)>
                {icon(Icon::Home)}<span>"Home"</span>
            </button>
            <button data-test="nav-stats" aria-label="Stats" class:active=on(Screen::Stats) on:click=move |_| go(Screen::Stats)>
                {icon(Icon::Stats)}<span>"Stats"</span>
            </button>
            <button class="photo" data-test="nav-photo" aria-label="Scan a receipt" on:click=move |_| go(Screen::Scan)>{icon(Icon::Camera)}</button>
            <button data-test="nav-add" aria-label="Add" on:click=move |_| go(Screen::Edit(None))>
                {icon(Icon::Plus)}<span>"Add"</span>
            </button>
            <button data-test="nav-settings" aria-label="Settings" class:active=on(Screen::Settings) on:click=move |_| go(Screen::Settings)>
                {icon(Icon::Settings)}<span>"Settings"</span>
            </button>
        </nav>
    }
}

fn banners(c: Ctx) -> impl IntoView {
    view! {
        // "Synced" is the normal state and needs no space on the screen.
        <p class="hint" data-test="status" hidden=move || c.status.with(|s| s.is_empty() || s == "Synced")>{move || c.status.get()}</p>
        {move || c.issues.get().into_iter().map(|i| view! { <p class="error" data-test="issue">{i}</p> }).collect_view()}
        {move || {
            let n = c.data.with(crate::import::pending);
            (n > 0).then(|| view! {
                <button class="warn-button" data-test="dup-banner" on:click=move |_| c.screen.set(Screen::Duplicates)>
                    {format!("Possible duplicates to review: {n}")}
                </button>
            })
        }}
        {move || {
            let n = c.data.with(Data::conflicts);
            (n > 0).then(|| view! { <p class="error" data-test="conflict-banner">{format!("Operations to review: {n}. They changed on two devices.")}</p> })
        }}
    }
}

/// `yearly`: the arrows move by a year, for the Monthly tab.
fn filters(c: Ctx, yearly: bool) -> impl IntoView {
    let f = c.filter;
    let shift = move |n: i32| {
        f.from.set(String::new());
        f.to.set(String::new());
        f.month.update(|m| *m = m.offset(if yearly { n * 12 } else { n }));
    };
    let label = move || {
        if yearly {
            f.month.get().year.to_string()
        } else if f.from.with(String::is_empty) || f.to.with(String::is_empty) {
            f.month.get().long()
        } else {
            "Custom period".to_string()
        }
    };
    view! {
        <div class="month">
            <button aria-label="Previous" data-test="month-prev" on:click=move |_| shift(-1)>"‹"</button>
            <button class="month-label" data-test="month" data-month=move || f.month.get().display()
                aria-label="Choose a date" on:click=move |_| f.picker.set(true)>
                <strong>{label}</strong>{icon(Icon::Calendar)}
            </button>
            <button aria-label="Next" data-test="month-next" on:click=move |_| shift(1)>"›"</button>
        </div>
        {move || f.picker.get().then(|| picker(c))}
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

fn signed(t: &Transaction) -> String {
    let sign = match t.kind { TxKind::Income => "+", TxKind::Expense => "−", TxKind::Transfer => "⇄ " };
    format!("{sign}{}", format_money(t.amount, &t.currency))
}

fn tx_title(d: &Data, t: &Transaction) -> String {
    d.merchant_name(t.merchant_id).unwrap_or_else(|| {
        if t.note.is_empty() { d.category_name(t.category_id) } else { t.note.clone() }
    })
}

/// Opens the details, or the form at once when the operation needs a choice between two versions.
fn open_tx(d: &Data, id: Uuid) -> Screen {
    if d.meta.get(&id).is_some_and(|m| !m.conflicts.is_empty()) { Screen::Edit(Some(id)) } else { Screen::Tx(id) }
}

/// `amount`: a day with one operation already shows its sum in the day header.
fn tx_row(c: Ctx, d: &Data, id: Uuid, t: &Transaction, amount: bool) -> impl IntoView + use<> {
    let meta = d.meta.get(&id).cloned().unwrap_or_default();
    let title = tx_title(d, t);
    let income = t.kind == TxKind::Income;
    let second = (t.currency != "PLN").then(|| pln(t.amount_pln));
    let cat = analytics::canonical(&d.categories, t.category_id);
    let items = match t.items.len() {
        0 => String::new(),
        1 => " · 1 item".to_string(),
        n => format!(" · {n} items"),
    };
    let picture = crate::icons::for_tx(t, cat);
    // Without a merchant or note the title already is the category name.
    let named = d.merchant_name(t.merchant_id).is_some() || !t.note.is_empty();
    let tail = if named { format!(" · {}{items}", d.category_name(cat)) } else { items };
    let target = open_tx(d, id);
    view! {
        <li>
            <button class="txrow" data-test="tx-row" on:click=move |_| c.screen.set(target.clone())>
                {icon(picture)}
                <span class="grow">
                    <span class="what"><span class="name" data-test="tx-title">{title}</span>
                        <span class="dot">{tail}</span></span>
                    {meta.pending.then(|| view! { <span class="hint" data-test="pending">"Waiting to send"</span> })}
                    {t.duplicate_of.is_some().then(|| view! { <span class="hint" data-test="tx-linked">"Recorded by a receipt, not counted"</span> })}
                    {(t.kind == TxKind::Transfer).then(|| view! { <span class="hint">"Transfer, not counted"</span> })}
                    {(!meta.conflicts.is_empty()).then(|| view! { <span class="error" data-test="tx-conflict">"Needs a choice"</span> })}
                </span>
                <span class="amount" class:income=income class:neg=t.kind == TxKind::Expense hidden=!amount>
                    <span data-test="tx-amount">{signed(t)}</span>
                    {second.map(|s| view! { <span class="hint">{s}</span> })}
                </span>
            </button>
        </li>
    }
}

/// What a day adds to the balance: income minus expenses, without transfers and linked duplicates.
fn day_net(rows: &[&Tx]) -> i64 {
    rows.iter().map(|(_, t)| match t.kind {
        _ if t.duplicate_of.is_some() => 0,
        TxKind::Income => t.amount_pln,
        TxKind::Expense => -t.amount_pln,
        TxKind::Transfer => 0,
    }).sum()
}

fn net(minor: i64) -> String {
    match minor {
        0 => pln(0),
        m if m > 0 => format!("+{}", pln(m)),
        m => format!("−{}", pln(-m)),
    }
}

fn money_class(minor: i64) -> &'static str {
    if minor < 0 { "num neg" } else if minor > 0 { "num income" } else { "num" }
}

fn signed_pln(minor: i64) -> String {
    if minor < 0 { net(minor) } else { pln(minor) }
}

fn slips(income: i64, expense: i64) -> impl IntoView {
    let balance = income - expense;
    view! {
        <section class="summary" aria-label="Totals">
            <div class="slip"><span class="label">"Spent"{icon(Icon::Wallet)}</span><strong data-test="dash-expense">{pln(expense)}</strong></div>
            <div class="slip"><span class="label">"Income"{icon(Icon::Download)}</span><strong class="income" data-test="dash-income">{pln(income)}</strong></div>
            <p class="balance"><span>"Balance"</span><strong class=money_class(balance.min(0)) data-test="dash-balance">{signed_pln(balance)}</strong></p>
        </section>
    }
}

fn tabs(c: Ctx) -> impl IntoView {
    let t = c.filter.tab;
    let go = move |x: HomeTab| move |_| t.set(x);
    view! {
        <nav class="hometabs" aria-label="Views">
            <button data-test="tab-daily" class:active=move || t.get() == HomeTab::Daily on:click=go(HomeTab::Daily)>"Daily"</button>
            <button data-test="tab-monthly" class:active=move || t.get() == HomeTab::Monthly on:click=go(HomeTab::Monthly)>"Monthly"</button>
            <button data-test="tab-summary" class:active=move || t.get() == HomeTab::Summary on:click=go(HomeTab::Summary)>"Summary"</button>
        </nav>
    }
}

fn days_of(rows: Vec<&Tx>) -> Vec<(Date, Vec<&Tx>)> {
    let mut days: Vec<(Date, Vec<&Tx>)> = Vec::new();
    for tx in rows {
        match days.last_mut() {
            Some((day, list)) if *day == tx.1.date => list.push(tx),
            _ => days.push((tx.1.date, vec![tx])),
        }
    }
    days
}

fn daily(c: Ctx, d: &Data, f: &Filter) -> AnyView {
    let days = days_of(analytics::filter_txs(&d.txs, f, &d.categories));
    let year = today().year;
    if d.txs.is_empty() {
        return view! {
            <p class="hint" data-test="empty">"No operations yet. Scan a receipt or add one by hand."</p>
            <button class="primary" data-test="add-first" on:click=move |_| c.screen.set(Screen::Edit(None))>"Add the first one"</button>
        }.into_any();
    }
    if days.is_empty() {
        return view! { <p class="hint" data-test="empty-list">"Nothing for this period."</p> }.into_any();
    }
    days.iter().map(|(day, list)| {
        let total = day_net(list);
        let many = list.len() > 1;
        let rows: Vec<_> = list.iter().map(|(id, t)| tx_row(c, d, *id, t, many)).collect();
        let wd = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"][day.weekday() as usize];
        let rest = if day.year == year { day.month_of().name().to_string() } else { day.month_of().long() };
        view! {
            <section class="day" data-test="day" data-date=day.iso() id=format!("day-{}", day.iso())>
                <div class="dayhead" data-test="day-label" aria-label=day.day_label(year)>
                    <span class="dnum">{format!("{:02}", day.day)}</span>
                    <span class="wd">{wd}</span>
                    <span class="dmonth">{rest}</span>
                    <span class=money_class(total) data-test="day-total">{net(total)}</span>
                </div>
                <ul class="txs">{rows}</ul>
            </section>
        }
    }).collect_view().into_any()
}

/// Twelve months of the year, newest first, with what came in and went out.
fn monthly(c: Ctx, d: &Data, f: &Filter, year: i32) -> AnyView {
    let now = today().month_of();
    let rows = (1..=12u32).rev().filter_map(|m| {
        let month = Month { year, month: m };
        let mf = Filter { from: Some(month.first()), to: Some(month.last()), ..f.clone() };
        let r = analytics::dashboard(&d.txs, &d.categories, &mf, month);
        let future = (year, m) > (now.year, now.month);
        (!(future && r.income == 0 && r.expense == 0)).then_some((month, r.income, r.expense))
    }).map(|(month, inc, exp)| {
        let total = inc - exp;
        let open = move |_| {
            c.filter.from.set(String::new());
            c.filter.to.set(String::new());
            c.filter.month.set(month);
            c.filter.tab.set(HomeTab::Daily);
        };
        view! {
            <li>
                <button class="monthrow" data-test="month-row" data-month=month.display() on:click=open>
                    <span class="mname">{month.name()}</span>
                    <span class="mfig"><span class="hint">"In"</span><span class="num income">{pln(inc)}</span></span>
                    <span class="mfig"><span class="hint">"Out"</span><span class=money_class(-exp)>{if exp > 0 { format!("−{}", pln(exp)) } else { pln(0) }}</span></span>
                    <span class="mfig"><span class="hint">"Total"</span><strong class=money_class(total)>{signed_pln(total)}</strong></span>
                </button>
            </li>
        }
    }).collect_view();
    view! { <ul class="months" data-test="months">{rows}</ul> }.into_any()
}

/// Where the money of the period went, by category and by account.
fn summary(c: Ctx, d: &Data, f: &Filter, r: &Dashboard) -> AnyView {
    let max_cat = r.by_category.first().map(|x| x.1).unwrap_or(0);
    let spent = r.expense.max(1);
    let cats: Vec<_> = r.by_category.iter().map(|(id, v)| {
        let id = *id;
        let share = (*v as i128 * 100 / spent as i128) as i64;
        view! {
            <li>
                <button class="barrow" data-test="cat-row" on:click=move |_| c.screen.set(Screen::Category(id))>
                    <span class="line"><span>{d.category_name(id)}<span class="hint">{format!("  {share}%")}</span></span><span class="num">{pln(*v)}</span></span>
                    {bar(*v, max_cat)}
                </button>
            </li>
        }
    }).collect();
    let rows = analytics::filter_txs(&d.txs, f, &d.categories);
    let mut income: Vec<(Uuid, i64)> = Vec::new();
    let mut accounts: Vec<(Uuid, i64, i64)> = Vec::new();
    for (_, t) in rows.iter().filter(|(_, t)| t.duplicate_of.is_none() && t.kind != TxKind::Transfer) {
        if t.kind == TxKind::Income {
            let cat = analytics::canonical(&d.categories, t.category_id);
            match income.iter_mut().find(|x| x.0 == cat) {
                Some(x) => x.1 += t.amount_pln,
                None => income.push((cat, t.amount_pln)),
            }
        }
        let (i, e) = if t.kind == TxKind::Income { (t.amount_pln, 0) } else { (0, t.amount_pln) };
        match accounts.iter_mut().find(|x| x.0 == t.account_id) {
            Some(x) => { x.1 += i; x.2 += e; }
            None => accounts.push((t.account_id, i, e)),
        }
    }
    income.sort_by_key(|x| -x.1);
    let income_rows = income.iter().map(|(id, v)| view! {
        <li class="line" data-test="income-row"><span>{d.category_name(*id)}</span><span class="num income">{format!("+{}", pln(*v))}</span></li>
    }).collect_view();
    let account_rows = accounts.iter().map(|(id, i, e)| {
        let name = d.accounts.get(id).map(|a| a.name.clone()).unwrap_or_default();
        view! {
            <li class="line" data-test="account-row"><span>{name}</span>
                <span class="num"><span class="income">{format!("+{}", pln(*i))}</span>" "<span class="neg">{format!("−{}", pln(*e))}</span></span></li>
        }
    }).collect_view();
    let has_income = !income.is_empty();
    view! {
        <h2>"Expenses by category"</h2>
        {if cats.is_empty() {
            view! { <p class="hint">"No expenses in this period."</p> }.into_any()
        } else {
            view! { <ul class="bars">{cats}</ul> }.into_any()
        }}
        {has_income.then(|| view! { <h2>"Income"</h2><ul class="plain">{income_rows}</ul> })}
        {(!accounts.is_empty()).then(|| view! { <h2>"By account"</h2><ul class="plain">{account_rows}</ul> })}
    }.into_any()
}

/// Home: totals, then Daily, Monthly or Summary.
pub fn dashboard(c: Ctx) -> impl IntoView {
    // A day picked in the calendar: scroll to it, or to the nearest earlier day with operations.
    Effect::new(move |_| {
        let Some(want) = c.filter.jump.get() else { return };
        let f = c.filter.build();
        let target = c.data.with_untracked(|d| {
            days_of(analytics::filter_txs(&d.txs, &f, &d.categories)).iter().map(|x| x.0).find(|day| *day <= want)
        });
        c.filter.jump.set(None);
        let Some(day) = target else { return };
        leptos::task::spawn_local(async move {
            crate::scan::sleep(30).await;
            if let Some(el) = web_sys::window().and_then(|w| w.document()).and_then(|d| d.get_element_by_id(&format!("day-{}", day.iso()))) {
                el.scroll_into_view();
            }
        });
    });
    let body = move || {
        let tab = c.filter.tab.get();
        let d = c.data.get();
        let month = c.filter.month.get();
        let f = c.filter.build();
        match tab {
            HomeTab::Monthly => {
                let year = month.year;
                let yf = Filter { from: Date::new(year, 1, 1), to: Date::new(year, 12, 31), ..f.clone() };
                let r = analytics::dashboard(&d.txs, &d.categories, &yf, month);
                view! { {slips(r.income, r.expense)}{monthly(c, &d, &f, year)} }.into_any()
            }
            HomeTab::Daily => {
                let r = analytics::dashboard(&d.txs, &d.categories, &f, month);
                view! { {slips(r.income, r.expense)}{daily(c, &d, &f)} }.into_any()
            }
            HomeTab::Summary => {
                let r = analytics::dashboard(&d.txs, &d.categories, &f, month);
                view! { {slips(r.income, r.expense)}{summary(c, &d, &f, &r)} }.into_any()
            }
        }
    };
    view! {
        {tabs(c)}
        {move || filters(c, c.filter.tab.get() == HomeTab::Monthly)}
        {banners(c)}
        {body}
    }
}

/// Year, month and day in one sheet. A day opens Daily at that day.
fn picker(c: Ctx) -> impl IntoView {
    let f = c.filter;
    let start = f.month.get_untracked();
    let year = RwSignal::new(start.year);
    let month = RwSignal::new(start.month);
    let close = move || f.picker.set(false);
    let apply = move |m: Month, day: Option<Date>| {
        f.from.set(String::new());
        f.to.set(String::new());
        f.month.set(m);
        if day.is_some() {
            f.tab.set(HomeTab::Daily);
        }
        f.jump.set(day);
        close();
    };
    let months = move || (1..=12u32).map(|m| {
        let pick = move |_| month.set(m);
        view! {
            <button data-test="picker-month" class:active=move || month.get() == m on:click=pick>
                {Month { year: year.get_untracked(), month: m }.short_name()}
            </button>
        }
    }).collect_view();
    let days = move || {
        let m = Month { year: year.get(), month: month.get() };
        let first = m.first();
        let busy: std::collections::HashSet<Date> = c.data.with(|d| d.txs.iter().map(|t| t.1.date).filter(|x| x.month_of() == m).collect());
        let now = today();
        let blanks = (0..first.weekday()).map(|_| view! { <span></span> }).collect_view();
        let cells = (1..=m.last().day).filter_map(|n| Date::new(m.year, m.month, n)).map(|day| {
            let has = busy.contains(&day);
            view! {
                <button data-test="picker-day" data-date=day.iso() class:has=has class:today=day == now
                    on:click=move |_| apply(m, Some(day))>{day.day}</button>
            }
        }).collect_view();
        view! { {blanks}{cells} }
    };
    view! {
        <div class="sheet-bg" on:click=move |_| close()></div>
        <section class="sheet" data-test="picker" role="dialog" aria-label="Choose a date">
            <div class="month">
                <button aria-label="Previous year" data-test="picker-prev" on:click=move |_| year.update(|y| *y -= 1)>"‹"</button>
                <strong data-test="picker-year">{move || year.get()}</strong>
                <button aria-label="Next year" data-test="picker-next" on:click=move |_| year.update(|y| *y += 1)>"›"</button>
            </div>
            <div class="mgrid">{months}</div>
            <div class="dgrid">
                {["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"].map(|w| view! { <span class="hint">{w}</span> }).collect_view()}
                {days}
            </div>
            <button class="primary" data-test="picker-apply" on:click=move |_| apply(Month { year: year.get_untracked(), month: month.get_untracked() }, None)>
                {move || format!("Show {}", Month { year: year.get(), month: month.get() }.long())}
            </button>
            <button data-test="picker-today" on:click=move |_| { let t = today(); apply(t.month_of(), Some(t)) }>"Today"</button>
        </section>
    }
}

/// Trends over time: six months, top merchants and subscriptions.
pub fn stats(c: Ctx) -> impl IntoView {
    let body = move || {
        let f = c.filter.build();
        let month = c.filter.month.get();
        let d = c.data.get();
        let r: Dashboard = analytics::dashboard(&d.txs, &d.categories, &f, month);
        let max_shop = r.top_merchants.first().map(|x| x.1).unwrap_or(0);
        let shops: Vec<_> = r.top_merchants.iter().map(|(id, v)| view! {
            <li class="barrow" data-test="top-merchant">
                <span class="line"><span>{d.merchant_name(*id).unwrap_or_default()}</span><span class="num">{pln(*v)}</span></span>
                {bar(*v, max_shop)}
            </li>
        }).collect();
        let has_shops = !r.top_merchants.is_empty();
        let subs = mm_core::dedup::recurring(&d.txs, |t| d.merchant_name(t.merchant_id).unwrap_or_else(|| t.note.clone()));
        let subs = if subs.is_empty() {
            view! { <p class="hint" data-test="no-subscriptions">"Payments that repeat every week, month or year will appear here."</p> }.into_any()
        } else {
            let rows = subs.into_iter().map(|r| {
                let every = match r.period_days { 6..=8 => "weekly", 360.. => "yearly", _ => "monthly" };
                view! {
                    <li class="txrow" data-test="subscription">
                        <span class="grow"><span class="name">{r.name.clone()}</span>
                            <span class="hint">{format!("{every}, {} payments, last {}", r.count, r.last.display())}</span></span>
                        <span class="amount neg"><span>{format!("−{}", pln(r.amount_pln))}</span></span>
                    </li>
                }
            }).collect_view();
            view! { <ul class="txs">{rows}</ul> }.into_any()
        };
        view! {
            <h2>"Last six months"</h2>
            <div class="legend" aria-hidden="true"><span class="key income"></span>"Income"<span class="key expense"></span>"Expenses"</div>
            {trend_chart(&r.trend)}
            {trend_table(&r.trend)}
            {has_shops.then(|| view! { <h2>"Top merchants"</h2><ul class="bars">{shops}</ul> })}
            <h2>"Subscriptions"</h2>
            {subs}
        }
    };
    view! {
        {filters(c, false)}
        {banners(c)}
        {body}
    }
}

/// One operation as recorded, laid out like the receipt, with every line. Editing is one tap away.
pub fn tx_detail(c: Ctx, id: Uuid) -> impl IntoView {
    let body = move || {
        let d = c.data.get();
        let Some((_, t)) = d.txs.iter().find(|t| t.0 == id) else {
            return view! { <p class="hint" data-test="tx-gone">"This operation was deleted."</p> }.into_any();
        };
        let meta = d.meta.get(&id).cloned().unwrap_or_default();
        let account = d.accounts.get(&t.account_id).map(|a| a.name.clone()).unwrap_or_default();
        let facts = [
            ("Category:", d.category_name(analytics::canonical(&d.categories, t.category_id))),
            ("Account:", account),
            ("Paid by:", t.payment.label().to_string()),
            ("NIP:", t.nip.clone().unwrap_or_default()),
            ("Note:", t.note.clone()),
        ];
        let facts = facts.into_iter().filter(|(_, v)| !v.is_empty()).map(|(k, v)| view! {
            <div><dt>{k}</dt><dd>{v}</dd></div>
        }).collect_view();
        let lines = t.items.iter().map(|i| {
            let unit = if i.unit.is_empty() || i.unit == "szt" || i.unit == "pcs" { String::new() } else { format!(" {}", i.unit) };
            let how = format!("{}{unit} × {} · {}", i.qty.display(), format_money(i.unit_price, &t.currency),
                d.category_name(analytics::canonical(&d.categories, i.category_id)));
            view! {
                <li data-test="detail-item">
                    <span class="grow">
                        <span class="name">{i.name.clone()}</span>
                        <span class="hint">{how}</span>
                        {(i.discount != 0).then(|| view! { <span class="hint">{format!("Discount −{}", format_money(i.discount, &t.currency))}</span> })}
                    </span>
                    <span class="num">{format_money(i.total, &t.currency)}</span>
                </li>
            }
        }).collect_view();
        let n = t.items.len();
        let income = t.kind == TxKind::Income;
        view! {
            <h1 class="title" data-test="detail-title">{tx_title(&d, t)}</h1>
            <p class="huge" class:income=income class:neg=t.kind == TxKind::Expense data-test="detail-amount">{signed(t)}</p>
            {(t.currency != "PLN").then(|| view! { <p class="hint">{format!("{} at the NBP rate", pln(t.amount_pln))}</p> })}
            {meta.pending.then(|| view! { <p class="hint">"Waiting to send"</p> })}
            {t.duplicate_of.is_some().then(|| view! { <p class="hint">"Recorded by a receipt, not counted twice"</p> })}
            {(t.kind == TxKind::Transfer).then(|| view! { <p class="hint">"Transfer between own accounts, not counted"</p> })}
            <dl class="facts">{facts}</dl>
            {(n > 0).then(|| view! {
                <h2>{if n == 1 { "1 item".to_string() } else { format!("{n} items") }}</h2>
                <ul class="lines" data-test="detail-items">{lines}</ul>
                <div class="line total"><strong>"Total"</strong><strong class="num">{format_money(t.amount, &t.currency)}</strong></div>
            })}
            <button class="primary" data-test="tx-edit" on:click=move |_| c.screen.set(Screen::Edit(Some(id)))>"Edit"</button>
        }.into_any()
    };
    let when = move || c.data.with(|d| d.txs.iter().find(|t| t.0 == id).map(|(_, t)| match &t.time {
        Some(h) if !h.is_empty() => format!("{} · {h}", t.date.day_label(today().year)),
        _ => t.date.day_label(today().year),
    }).unwrap_or_default());
    view! {
        <header class="detailhead">
            <button class="back" data-test="detail-back" aria-label="Back" on:click=move |_| c.screen.set(Screen::Dashboard)>{icon(Icon::Back)}</button>
            <span class="hint" data-test="detail-when">{when}</span>
        </header>
        {body}
    }
}

/// Theme picker. The choice is a record like any other, so it is encrypted and follows the user to every device.
pub fn appearance(c: Ctx) -> impl IntoView {
    let pick = move |theme: Theme| busy_task(c, async move {
        let s = session(c)?;
        let mut prefs = c.data.with_untracked(|d| d.prefs.clone());
        prefs.theme = theme;
        apply_theme(theme);
        data::write(&s, named_id(&s.dek, b"prefs", "prefs"), KIND_PREFS, &prefs).await?;
        after_write(c, &s).await;
        Ok(())
    });
    let on = move |t: Theme| move || c.data.with(|d| d.prefs.theme == t);
    view! {
        <h2>"Appearance"</h2>
        <div class="segmented">
            <button data-test="theme-dark" class:active=on(Theme::Dark) on:click=move |_| pick(Theme::Dark)>"Dark"</button>
            <button data-test="theme-light" class:active=on(Theme::Light) on:click=move |_| pick(Theme::Light)>"Light"</button>
        </div>
    }
}

/// Sets the theme on the page, and remembers it on this device so the sign-in screen opens in it too.
pub fn apply_theme(theme: Theme) {
    let Some(w) = web_sys::window() else { return };
    if let Some(root) = w.document().and_then(|d| d.document_element()) {
        let _ = root.set_attribute("data-theme", theme.code());
    }
    if let Ok(Some(ls)) = w.local_storage() {
        let _ = ls.set_item("mmTheme", theme.code());
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
                    <button class="txrow" data-test="line-row" on:click=move |_| c.screen.set(Screen::Tx(tx))>
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
            <button on:click=move |_| { c.filter.tab.set(HomeTab::Summary); c.screen.set(Screen::Dashboard) }>"Back"</button>
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
    doubt: bool,
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
            doubt: false,
            name: RwSignal::new(String::new()),
            qty: RwSignal::new("1".into()),
            unit: RwSignal::new("pcs".into()),
            price: RwSignal::new(String::new()),
            discount: RwSignal::new(String::new()),
            vat: RwSignal::new(String::new()),
            category: RwSignal::new(category),
        }
    }

    fn from(i: &LineItem, doubt: bool) -> Self {
        Self {
            key: Uuid::new_v4(),
            raw_name: i.raw_name.clone(),
            doubt,
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
        c.screen.set(Screen::Dashboard);
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
    let mut draft: Option<Draft> = None;
    if id.is_none() {
        c.draft.update_value(|v| draft = v.take());
    }
    let doubts = draft.as_ref().map(|x| x.doubts.clone()).unwrap_or_default();
    let suggested = StoredValue::new(draft.as_ref().map(|x| x.suggested.clone()).unwrap_or_default());
    let scanned = draft.is_some();
    let old: Option<Transaction> = id.and_then(|i| d.txs.iter().find(|t| t.0 == i).map(|t| t.1.clone()))
        .or_else(|| draft.as_ref().map(|x| x.tx.clone()));
    let conflicts = id.and_then(|i| d.meta.get(&i)).map(|m| m.conflicts.clone()).unwrap_or_default();
    let default_account = d.account_list().first().map(|a| a.0).unwrap_or(ACCOUNT_MAIN);
    let o = old.as_ref();

    let kind = RwSignal::new(o.map(|t| t.kind).unwrap_or_default());
    let date = RwSignal::new(o.map(|t| t.date).unwrap_or_else(today).iso());
    let time = RwSignal::new(o.and_then(|t| t.time.clone()).unwrap_or_default());
    let merchant = RwSignal::new(o.and_then(|t| d.merchant_name(t.merchant_id))
        .or_else(|| draft.as_ref().map(|x| x.merchant.clone())).unwrap_or_default());
    let nip = RwSignal::new(o.and_then(|t| t.nip.clone()).unwrap_or_default());
    let amount = RwSignal::new(o.map(|t| amount_input(t.amount)).unwrap_or_default());
    let currency = RwSignal::new(o.map(|t| t.currency.clone()).unwrap_or_else(|| "PLN".into()));
    let amount_pln = RwSignal::new(o.filter(|t| t.currency != "PLN").map(|t| amount_input(t.amount_pln)).unwrap_or_default());
    let payment = RwSignal::new(o.map(|t| t.payment).unwrap_or_default());
    let account = RwSignal::new(o.map(|t| t.account_id).unwrap_or(default_account));
    let category = RwSignal::new(o.map(|t| t.category_id).unwrap_or(CAT_OTHER));
    let note = RwSignal::new(o.map(|t| t.note.clone()).unwrap_or_default());
    let items = RwSignal::new(o.map(|t| t.items.iter().enumerate()
        .map(|(i, it)| ItemDraft::from(it, doubts.items.get(i).copied().unwrap_or(false))).collect::<Vec<_>>()).unwrap_or_default());
    // A receipt keeps its printed total, so a misread item shows up as a difference.
    let receipt = o.is_some_and(|t| t.source == Source::Receipt);
    let confirm = RwSignal::new(o.is_some_and(|t| t.mismatch_confirmed));
    let (source, review) = (o.map(|t| t.source).unwrap_or_default(), o.map(|t| t.review).unwrap_or_default());
    // Editing must keep a link to a receipt, or the purchase would count twice again.
    let (duplicate_of, unique_confirmed) = (o.and_then(|t| t.duplicate_of), o.is_some_and(|t| t.unique_confirmed));
    let form_error = RwSignal::new(String::new());
    let rate_note = RwSignal::new(String::new());

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
            let sum = lines.iter().try_fold(0i64, |a, l| a.checked_add(l.total)).ok_or("The amount is too large")?;
            let total = if lines.is_empty() || receipt {
                parse_amount(&amount.get_untracked()).map_err(|_| "Enter the amount, like 12,34".to_string())?
            } else {
                sum
            };
            let mismatch = receipt && !lines.is_empty() && sum != total;
            if mismatch && !confirm.get_untracked() {
                return Err(format!(
                    "The items add up to {}, but the receipt total is {}. Fix an item, or confirm below to save it as it is",
                    format_amount(sum), format_amount(total),
                ));
            }
            let in_pln = if currency == "PLN" {
                total
            } else {
                parse_amount(&amount_pln.get_untracked()).map_err(|_| "Enter the amount in PLN".to_string())?
            };
            let name = merchant.get_untracked().trim().to_string();
            let nip_s = normalize_nip(&nip.get_untracked());
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
                review: if mismatch || (review == Review::NeedsReview && !receipt) { Review::NeedsReview } else { Review::Reviewed },
                note: note.get_untracked().trim().to_string(),
                items: lines,
                mismatch_confirmed: mismatch,
                duplicate_of,
                unique_confirmed,
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
                    // A category the user changed becomes a rule for the next receipts from this shop.
                    for (i, line) in t.items.iter().enumerate() {
                        if suggested.with_value(|v| v.get(i).is_some_and(|sug| *sug != line.category_id)) {
                            let pattern = fold(&line.name);
                            let key = format!("{}|{pattern}", t.merchant_id.map(|m| m.to_string()).unwrap_or_default());
                            let rule = CategoryRule { merchant_id: t.merchant_id, pattern, category_id: line.category_id };
                            data::write(&s, named_id(&s.dek, b"rule", &key), KIND_RULE, &rule).await?;
                        }
                    }
                    data::write(&s, id.unwrap_or_else(Uuid::new_v4), KIND_TRANSACTION, &t).await?;
                    // Show the month of the saved operation, so a receipt from last month does not seem to vanish.
                    c.filter.from.set(String::new());
                    c.filter.to.set(String::new());
                    c.filter.month.set(t.date.month_of());
                    c.filter.tab.set(HomeTab::Daily);
                    c.screen.set(Screen::Dashboard);
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
        c.screen.set(Screen::Dashboard);
        after_write(c, &s).await;
        Ok(())
    });

    let boxes = conflicts.into_iter().filter_map(|f| id.map(|i| conflict_box(c, i, f))).collect_view();
    let item_row = move |i: ItemDraft| {
        let key = i.key;
        view! {
            <li class="item" class:doubt=i.doubt data-test="item">
                {i.doubt.then(|| view! { <p class="warn" data-test="item-doubt">"Check this item"</p> })}
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
            <h1>{if scanned { "Check the receipt" } else if id.is_some() { "Edit operation" } else { "New operation" }}</h1>
            <button data-test="form-back" on:click=move |_| c.screen.set(id.map_or(Screen::Dashboard, Screen::Tx))>"Back"</button>
        </header>
        {boxes}
        <div class="segmented">
            <button data-test="kind-expense" class:active=move || kind.get() == TxKind::Expense on:click=move |_| kind.set(TxKind::Expense)>"Expense"</button>
            <button data-test="kind-income" class:active=move || kind.get() == TxKind::Income on:click=move |_| kind.set(TxKind::Income)>"Income"</button>
            <button data-test="kind-transfer" class:active=move || kind.get() == TxKind::Transfer on:click=move |_| kind.set(TxKind::Transfer)>"Transfer"</button>
        </div>
        {scanned.then(|| view! {
            <p class="hint" data-test="scan-hint">{if doubts.merchant || doubts.date || doubts.total || doubts.nip || doubts.items.contains(&true) {
                "Check the highlighted fields, then save."
            } else {
                "Everything adds up. Check and save."
            }}</p>
        })}
        {move || match (items_sum(), receipt) {
            (None, _) | (Some(_), true) => view! {
                <label class:doubt=doubts.total>{if receipt { "Receipt total" } else { "Amount" }}
                    <input class="big-input" data-test="f-amount" inputmode="decimal" placeholder="0,00"
                        prop:value=move || amount.get() on:input=move |e| amount.set(event_target_value(&e)) />
                </label>
            }.into_any(),
            (Some(sum), false) => view! {
                <p>"Amount: "<strong data-test="f-sum">{sum.map(format_amount).unwrap_or_else(|| "check the items".into())}</strong></p>
            }.into_any(),
        }}
        {move || {
            let sum = items_sum().flatten()?;
            let total = parse_amount(&amount.get()).ok()?;
            (receipt && sum != total).then(|| view! {
                <div class="conflict" data-test="mismatch">
                    <p>{format!("The items add up to {}, but the receipt total is {}.", format_amount(sum), format_amount(total))}</p>
                    <label class="check">
                        <input type="checkbox" data-test="confirm-mismatch" prop:checked=move || confirm.get()
                            on:change=move |e| confirm.set(event_target_checked(&e)) />
                        "Save it with this difference"
                    </label>
                </div>
            })
        }}
        <div class="row">
            <label>"Currency"<input data-test="f-currency" maxlength="3" autocapitalize="characters" prop:value=move || currency.get() on:input=move |e| currency.set(event_target_value(&e)) /></label>
            {move || (currency.get().trim().to_uppercase() != "PLN").then(|| view! {
                <label>"In PLN"<input data-test="f-pln" inputmode="decimal" prop:value=move || amount_pln.get() on:input=move |e| amount_pln.set(event_target_value(&e)) /></label>
                <button data-test="f-nbp" on:click=move |_| {
                    let (cur, day, amt) = (currency.get_untracked().trim().to_uppercase(), Date::parse_iso(&date.get_untracked()), parse_amount(&amount.get_untracked()));
                    leptos::task::spawn_local(async move {
                        let (Some(day), Ok(amt)) = (day, amt) else { rate_note.set("Enter the amount and date first".into()); return };
                        match crate::import::nbp_rate(&cur, day, &mut Default::default()).await {
                            Ok(Some((when, rate))) => {
                                amount_pln.set(convert(amt, rate).map(amount_input).unwrap_or_default());
                                rate_note.set(format!("NBP rate {} from {}", mm_core::money::format_rate(rate), when.display()));
                            }
                            Ok(None) => rate_note.set("NBP has no rate for this currency and date".into()),
                            Err(e) => rate_note.set(e),
                        }
                    });
                }>"NBP rate"</button>
            })}
        </div>
        <div class="row">
            <label class:doubt=doubts.date>"Date"<input type="date" data-test="f-date" prop:value=move || date.get() on:input=move |e| date.set(event_target_value(&e)) /></label>
            <label>"Time"<input type="time" data-test="f-time" prop:value=move || time.get() on:input=move |e| time.set(event_target_value(&e)) /></label>
        </div>
        <label class:doubt=doubts.merchant>"Merchant"
            <input data-test="f-merchant" list="merchants" autocomplete="off" prop:value=move || merchant.get() on:input=move |e| merchant.set(event_target_value(&e)) />
            <datalist id="merchants">
                {move || c.data.with(Data::merchant_list).into_iter().map(|(_, m)| view! { <option value=m.name></option> }).collect_view()}
            </datalist>
        </label>
        <label class:doubt=doubts.nip>"NIP (optional)"<input data-test="f-nip" inputmode="numeric" autocomplete="off" prop:value=move || nip.get() on:input=move |e| nip.set(event_target_value(&e)) /></label>
        {move || {
            let n = normalize_nip(&nip.get());
            (!n.is_empty() && !valid_nip(&n)).then(|| view! {
                <p class="warn" data-test="nip-warning">"This NIP looks wrong. You can still save."</p>
            })
        }}
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
                let n = normalize_nip(&nip.get_untracked());
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
                    <label>"NIP (optional)"<input data-test="merchant-nip" inputmode="numeric" prop:value=move || nip.get() on:input=move |e| nip.set(event_target_value(&e)) /></label>
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
