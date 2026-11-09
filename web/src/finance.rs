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
    /// The day Daily shows. Its month is always `month`.
    pub day: RwSignal<Date>,
    /// Categories tapped off in the Stats chart. They leave every number on that screen.
    pub excluded: RwSignal<Vec<Uuid>>,
    /// Stats shows the split of spending, or of income.
    pub stats_income: RwSignal<bool>,
    /// Words searched on Stats, and whether the search covers all time instead of the chosen period.
    pub search: RwSignal<String>,
    pub search_all: RwSignal<bool>,
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
            day: RwSignal::new(today()),
            excluded: RwSignal::new(vec![]),
            stats_income: RwSignal::new(false),
            search: RwSignal::new(String::new()),
            search_all: RwSignal::new(false),
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
        Filter { from: Some(from), to: Some(to), account: self.account.get(), category: self.category.get(), exclude: vec![] }
    }

    /// Daily: the chosen day, or the custom period when one is set.
    fn build_day(&self) -> Filter {
        let mut f = self.build();
        if self.from.with(String::is_empty) || self.to.with(String::is_empty) {
            let day = self.day.get();
            (f.from, f.to) = (Some(day), Some(day));
        }
        f
    }

    fn custom(&self) -> bool {
        !(self.from.with(String::is_empty) || self.to.with(String::is_empty))
    }

    /// Shows one day: Daily moves to it, and every other view to its month.
    pub fn show_day(&self, day: Date) {
        self.from.set(String::new());
        self.to.set(String::new());
        self.day.set(day);
        self.month.set(day.month_of());
    }
}

/// Opens a month. Daily then shows today if it is in that month, otherwise the month's latest day with
/// operations, or its last day when there are none.
fn show_month(c: Ctx, m: Month) {
    let now = today();
    let day = if now.month_of() == m {
        now
    } else {
        c.data.with_untracked(|d| d.txs.iter().map(|t| t.1.date).filter(|x| x.month_of() == m).max())
            .unwrap_or(if m > now.month_of() { m.first() } else { m.last() })
    };
    c.filter.show_day(day);
}

/// What the arrows beside the period move by.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Day,
    Month,
    Year,
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
            <span class="side-brand" aria-hidden="true">"Money manager ("<span class="logo">"mm"</span>")"</span>
            <button data-test="nav-dashboard" aria-label="Home" class:active=on(Screen::Dashboard) on:click=move |_| go(Screen::Dashboard)>
                {icon(Icon::Home)}<span>"Home"</span>
            </button>
            <button data-test="nav-stats" aria-label="Stats" class:active=on(Screen::Stats) on:click=move |_| go(Screen::Stats)>
                {icon(Icon::Stats)}<span>"Stats"</span>
            </button>
            <button class="photo" data-test="nav-photo" aria-label="Scan a receipt" on:click=move |_| go(Screen::Scan)>{icon(Icon::Camera)}<span>"Scan a receipt"</span></button>
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

/// The period with its arrows: a day on Daily, a year on Monthly, a month elsewhere.
fn filters(c: Ctx, step: Step) -> impl IntoView {
    let f = c.filter;
    let shift = move |n: i32| match step {
        // From a custom period, the arrows start again from the chosen day.
        Step::Day => f.show_day(if f.custom() { f.day.get_untracked() } else { f.day.get_untracked().add_days(n as i64) }),
        Step::Month => show_month(c, f.month.get_untracked().offset(n)),
        Step::Year => show_month(c, f.month.get_untracked().offset(n * 12)),
    };
    let label = move || match step {
        Step::Year => f.month.get().year.to_string(),
        _ if f.custom() => "Custom period".to_string(),
        Step::Day => f.day.get().day_label(today().year),
        Step::Month => f.month.get().long(),
    };
    view! {
        <div class="month">
            <button aria-label="Previous" data-test="month-prev" on:click=move |_| shift(-1)>"‹"</button>
            <button class="month-label" data-test="month" data-month=move || f.month.get().display() data-date=move || f.day.get().iso()
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
    // A receipt with lines in several categories shows each one under the row, with its lines and sum.
    let groups = analytics::groups(t, &d.categories);
    let mixed = groups.len() > 1;
    // Without a merchant or note the title already is the category name.
    let named = d.merchant_name(t.merchant_id).is_some() || !t.note.is_empty();
    let tail = if mixed {
        format!(" · {} categories{items}", groups.len())
    } else if named {
        format!(" · {}{items}", d.category_name(cat))
    } else {
        items
    };
    let target = open_tx(d, id);
    let split = mixed.then(|| {
        let rows = groups.iter().map(|g| {
            let names = g.items.iter().filter_map(|i| t.items.get(*i)).map(|i| i.name.clone()).collect::<Vec<_>>().join(", ");
            view! {
                <li data-test="split-group">
                    <span class="line"><span class="split-cat">{d.category_name(g.category)}</span>
                        <span class="num">{format!("−{}", pln(g.amount_pln))}</span></span>
                    {(!names.is_empty()).then(|| view! { <span class="split-items">{names}</span> })}
                </li>
            }
        }).collect_view();
        let target = target.clone();
        view! { <ul class="split" data-test="split" on:click=move |_| c.screen.set(target.clone())>{rows}</ul> }
    });
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
            {split}
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
        return view! { <p class="hint" data-test="empty-list">{if c.filter.custom() { "Nothing for this period." } else { "Nothing on this day." }}</p> }.into_any();
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
            show_month(c, month);
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
/// A ring and its list of categories, for the Home summary. A tap opens the category's operations.
fn category_ring(c: Ctx, d: &Data, rows: &[(Uuid, i64)], total: i64, income: bool, test: &'static str) -> impl IntoView {
    let open = move |id: Uuid| c.screen.set(Screen::Category(id));
    let slices: Vec<(Uuid, i64, usize, String)> = rows.iter().filter(|x| x.1 > 0)
        .map(|(id, v)| (*id, *v, color_slot(d, income, *id), d.category_name(*id))).collect();
    let list = rows.iter().map(|(id, v)| {
        let (id, v) = (*id, *v);
        let slot = color_slot(d, income, id);
        view! {
            <li>
                <button class="pie-row" data-test=test on:click=move |_| open(id)>
                    <span class=format!("swatch s{slot}")></span>
                    <span class="pct num">{format!("{}%", percent(v, total))}</span>
                    <span class="grow name">{d.category_name(id)}</span>
                    <span class="num" class:income=income>{if income { format!("+{}", pln(v)) } else { pln(v) }}</span>
                    <span class="chev">{icon(Icon::Chevron)}</span>
                </button>
            </li>
        }
    }).collect_view();
    view! {
        <div class="ring-card" class:income=income>
            {donut(&slices, total, if income { "Income" } else { "Spent" }, open)}
            <ul class="pie-list">{list}</ul>
        </div>
    }
}

fn summary(c: Ctx, d: &Data, f: &Filter, r: &Dashboard) -> AnyView {
    let rows = analytics::filter_txs(&d.txs, f, &d.categories);
    let mut accounts: Vec<(Uuid, i64, i64)> = Vec::new();
    for (_, t) in rows.iter().filter(|(_, t)| t.duplicate_of.is_none() && t.kind != TxKind::Transfer) {
        let (i, e) = if t.kind == TxKind::Income { (t.amount_pln, 0) } else { (0, t.amount_pln) };
        match accounts.iter_mut().find(|x| x.0 == t.account_id) {
            Some(x) => { x.1 += i; x.2 += e; }
            None => accounts.push((t.account_id, i, e)),
        }
    }
    let account_rows = accounts.iter().map(|(id, i, e)| {
        let name = d.accounts.get(id).map(|a| a.name.clone()).unwrap_or_default();
        view! {
            <li class="entry-row" data-test="account-row">
                <span class="badge">{icon(Icon::Wallet)}</span>
                <span class="grow"><span class="name">{name}</span>
                    <span class="hint">{format!("balance {}", signed_pln(*i - *e))}</span></span>
                <span class="flow num"><span class="income">{format!("+{}", pln(*i))}</span><span class="neg">{format!("−{}", pln(*e))}</span></span>
            </li>
        }
    }).collect_view();
    view! {
        <h2>"Expenses by category"</h2>
        {if r.by_category.is_empty() {
            view! { <p class="hint">"No expenses in this period."</p> }.into_any()
        } else {
            category_ring(c, d, &r.by_category, r.expense, false, "cat-row").into_any()
        }}
        {(!r.income_by_category.is_empty()).then(|| view! {
            <h2>"Income by category"</h2>
            {category_ring(c, d, &r.income_by_category, r.income, true, "income-row")}
        })}
        {(!accounts.is_empty()).then(|| view! { <h2>"By account"</h2><ul class="ledger">{account_rows}</ul> })}
    }.into_any()
}

/// "3 operations · 120,00 zł", or that nothing uses it yet.
fn usage_line(n: usize, sum: i64) -> String {
    match n {
        0 => "not used yet".into(),
        1 => format!("1 operation · {}", pln(sum)),
        n => format!("{n} operations · {}", pln(sum)),
    }
}

/// The first letter of a name, for a merchant's badge.
fn initial(name: &str) -> String {
    name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().collect()).unwrap_or_else(|| "?".into())
}

/// Home: totals, then Daily, Monthly or Summary.
pub fn dashboard(c: Ctx) -> impl IntoView {
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
                let f = c.filter.build_day();
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
        {move || filters(c, match c.filter.tab.get() {
            HomeTab::Daily => Step::Day,
            HomeTab::Monthly => Step::Year,
            HomeTab::Summary => Step::Month,
        })}
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
        match day {
            Some(day) => {
                f.show_day(day);
                f.tab.set(HomeTab::Daily);
            }
            None => show_month(c, m),
        }
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
/// The chart color of a category: a fixed slot per category, so a color never moves to another one when a
/// category is tapped off. Built-ins come first in their fixed order; "Other" and anything past eight is gray.
fn color_slot(d: &Data, income: bool, id: Uuid) -> usize {
    if id == CAT_OTHER || id == CAT_INCOME_OTHER {
        return 0;
    }
    let mut ids: Vec<Uuid> = d.categories_of(income).into_iter().map(|x| x.0)
        .filter(|i| *i != CAT_OTHER && *i != CAT_INCOME_OTHER).collect();
    ids.sort_by_key(|i| (!is_builtin(*i), *i));
    ids.iter().position(|i| *i == id).filter(|p| *p < 8).map_or(0, |p| p + 1)
}

fn is_builtin(id: Uuid) -> bool {
    // Built-in ids count up from the first one.
    id.as_u128().wrapping_sub(CAT_FOOD.as_u128()) < 1000
}

/// Whole percent of `part` in `total`, at least 1 for anything above zero.
fn percent(part: i64, total: i64) -> i64 {
    if total <= 0 || part <= 0 {
        return 0;
    }
    ((part as i128 * 100 + total as i128 / 2) / total as i128).max(1) as i64
}

/// A ring of slices, one per category, drawn as dashes on one circle so a single category is a full ring.
fn donut<F: Fn(Uuid) + Copy + Send + Sync + 'static>(slices: &[(Uuid, i64, usize, String)], total: i64, caption: &'static str, on_tap: F) -> impl IntoView + use<F> {
    let gap = if slices.len() > 1 { 0.6 } else { 0.0 };
    let mut start = 0.0f64;
    let rings = slices.iter().map(|(id, v, slot, name)| {
        let share = if total > 0 { *v as f64 * 100.0 / total as f64 } else { 0.0 };
        let len = (share - gap).max(0.15);
        let dash = format!("{len:.3} {:.3}", 100.0 - len);
        let offset = format!("{:.3}", -start);
        start += share;
        let id = *id;
        view! {
            <circle class=format!("slice s{slot}") cx="60" cy="60" r="46" pathLength="100" stroke-dasharray=dash stroke-dashoffset=offset
                data-test="pie-slice" on:click=move |_| on_tap(id)>
                <title>{format!("{name}: {}, {}%", pln(*v), percent(*v, total))}</title>
            </circle>
        }
    }).collect_view();
    view! {
        <svg class="donut" data-test="donut" viewBox="0 0 120 120" role="img" aria-label=format!("{caption} by category")>
            <circle class="ring" cx="60" cy="60" r="46"/>
            <g transform="rotate(-90 60 60)">{rings}</g>
            <text class="donut-label" x="60" y="54" text-anchor="middle">{caption}</text>
            <text class="donut-total" x="60" y="70" text-anchor="middle" data-test="donut-total">{pln(total)}</text>
        </svg>
    }
}

/// What a search found: how often, how much of it, and for how much, then every line.
fn search_results(c: Ctx, d: &Data, f: &Filter, query: &str) -> AnyView {
    let f = if c.filter.search_all.get() { Filter { from: None, to: None, ..f.clone() } } else { f.clone() };
    let r = analytics::search(&d.txs, &d.categories, |m| d.merchant_name(m), &f, query);
    let all = c.filter.search_all.get();
    let scope = view! {
        <div class="segmented">
            <button data-test="search-period" class:active=!all on:click=move |_| c.filter.search_all.set(false)>"This period"</button>
            <button data-test="search-all" class:active=all on:click=move |_| c.filter.search_all.set(true)>"All time"</button>
        </div>
    };
    if r.hits.is_empty() {
        return view! { {scope}<p class="hint" data-test="search-none">{format!("Nothing found for “{}”.", query.trim())}</p> }.into_any();
    }
    let times = if r.purchases == 1 { "1 purchase".to_string() } else { format!("{} purchases", r.purchases) };
    let amounts = r.quantities.iter().map(|(u, q)| format!("{} {u}", q.display())).collect::<Vec<_>>().join(" · ");
    let shown = r.hits.len().min(100);
    let rows = r.hits.iter().take(shown).map(|h| {
        let target = open_tx(d, h.tx);
        let what = [Some(h.date.display()), h.merchant.clone(), h.qty.as_ref().map(|(q, u)| format!("{} {u}", q.display()))]
            .into_iter().flatten().collect::<Vec<_>>().join(" · ");
        let income = h.kind == TxKind::Income;
        let sign = if income { "+" } else { "−" };
        view! {
            <li>
                <button class="txrow" data-test="search-hit" on:click=move |_| c.screen.set(target.clone())>
                    <span class="grow">
                        <span class="what"><span class="name">{h.name.clone()}</span></span>
                        <span class="hint">{what}</span>
                    </span>
                    <span class="amount" class:income=income class:neg=!income><span>{format!("{sign}{}", pln(h.amount_pln))}</span></span>
                </button>
            </li>
        }
    }).collect_view();
    view! {
        {scope}
        <div class="found" data-test="search-summary">
            <p class="hint">{format!("“{}”", query.trim())}</p>
            <p data-test="search-count">{times}{(!amounts.is_empty()).then(|| format!(" · {amounts}"))}</p>
            {(r.spent > 0).then(|| view! { <p class="found-sum neg" data-test="search-spent">{format!("−{}", pln(r.spent))}</p> })}
            {(r.received > 0).then(|| view! { <p class="found-sum income" data-test="search-received">{format!("+{}", pln(r.received))}</p> })}
        </div>
        <ul class="txs">{rows}</ul>
        {(r.hits.len() > shown).then(|| view! { <p class="hint">{format!("The first {shown} of {} lines.", r.hits.len())}</p> })}
    }.into_any()
}

pub fn stats(c: Ctx) -> impl IntoView {
    let toggle = move |id: Uuid| c.filter.excluded.update(|v| if let Some(i) = v.iter().position(|x| *x == id) { v.remove(i); } else { v.push(id); });
    let body = move || {
        let query = c.filter.search.get();
        if !query.trim().is_empty() {
            let d = c.data.get();
            return search_results(c, &d, &c.filter.build(), &query);
        }
        let mut f = c.filter.build();
        let month = c.filter.month.get();
        let income = c.filter.stats_income.get();
        let excluded = c.filter.excluded.get();
        let d = c.data.get();
        let all: Dashboard = analytics::dashboard(&d.txs, &d.categories, &f, month);
        f.exclude = excluded.clone();
        let r: Dashboard = analytics::dashboard(&d.txs, &d.categories, &f, month);
        let rows = if income { &all.income_by_category } else { &all.by_category };
        let shown_total = if income { r.income } else { r.expense };
        let slices: Vec<(Uuid, i64, usize, String)> = rows.iter().filter(|x| !excluded.contains(&x.0) && x.1 > 0)
            .map(|(id, v)| (*id, *v, color_slot(&d, income, *id), d.category_name(*id))).collect();
        let list = rows.iter().map(|(id, v)| {
            let (id, v) = (*id, *v);
            let off = excluded.contains(&id);
            let slot = color_slot(&d, income, id);
            let pct = if off { "off".to_string() } else { format!("{}%", percent(v, shown_total)) };
            view! {
                <li>
                    <button class="pie-row" class:off=off data-test="pie-row" data-cat=id.to_string() aria-pressed=(!off).to_string() on:click=move |_| toggle(id)>
                        <span class=format!("swatch s{slot}")></span>
                        <span class="pct num">{pct}</span>
                        <span class="grow name">{d.category_name(id)}</span>
                        <span class="num">{pln(v)}</span>
                    </button>
                </li>
            }
        }).collect_view();
        let empty = rows.is_empty();
        let hidden = excluded.iter().filter(|e| rows.iter().any(|r| r.0 == **e)).count();
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
            <div class="flowtabs" role="tablist">
                <button role="tab" data-test="stats-income" class:active=income aria-selected=income.to_string()
                    on:click=move |_| c.filter.stats_income.set(true)>
                    <span>"Income"</span><strong class="num">{pln(r.income)}</strong>
                </button>
                <button role="tab" data-test="stats-expense" class:active=!income aria-selected=(!income).to_string()
                    on:click=move |_| c.filter.stats_income.set(false)>
                    <span>"Expenses"</span><strong class="num">{pln(r.expense)}</strong>
                </button>
            </div>
            {if empty {
                view! { <p class="hint" data-test="pie-empty">{if income { "No income in this period." } else { "No spending in this period." }}</p> }.into_any()
            } else {
                view! {
                    <div class="pie-panel">
                        {donut(&slices, shown_total, if income { "Income" } else { "Spent" }, toggle)}
                        <p class="hint center">"Tap a category to leave it out of this screen."</p>
                        <ul class="pie-list">{list}</ul>
                        {(hidden > 0).then(|| view! {
                            <button class="dashed" data-test="pie-reset" on:click=move |_| c.filter.excluded.set(vec![])>
                                {format!("Show all ({hidden} left out)")}
                            </button>
                        })}
                    </div>
                }.into_any()
            }}
            <h2>"Last six months"</h2>
            <div class="legend" aria-hidden="true"><span class="key income"></span>"Income"<span class="key expense"></span>"Expenses"</div>
            {trend_chart(&r.trend)}
            {trend_table(&r.trend)}
            {has_shops.then(|| view! { <h2>"Top merchants"</h2><ul class="bars">{shops}</ul> })}
            <h2>"Subscriptions"</h2>
            {subs}
        }.into_any()
    };
    // The input sits outside `body`, so typing does not rebuild it and keeps the focus.
    let words = move || c.data.with(|d| analytics::suggestions(&d.txs, &d.categories, |m| d.merchant_name(m)))
        .into_iter().take(300).map(|w| view! { <option value=w></option> }).collect_view();
    view! {
        {filters(c, Step::Month)}
        {banners(c)}
        <div class="search">
            {icon(Icon::Search)}
            <input type="search" data-test="search" list="search-words" autocomplete="off" enterkeyhint="search"
                placeholder="Search items, shops, categories" aria-label="Search"
                prop:value=move || c.filter.search.get() on:input=move |e| c.filter.search.set(event_target_value(&e)) />
            <datalist id="search-words">{words}</datalist>
        </div>
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
            ("Category:", analytics::groups(t, &d.categories).iter().map(|g| d.category_name(g.category)).collect::<Vec<_>>().join(", ")),
            ("Account:", account),
            ("Paid by:", t.payment.label().to_string()),
            ("NIP:", t.nip.clone().unwrap_or_default()),
            ("Note:", t.note.clone()),
        ];
        let facts = facts.into_iter().filter(|(_, v)| !v.is_empty()).map(|(k, v)| view! {
            <div><dt>{k}</dt><dd>{v}</dd></div>
        }).collect_view();
        let groups = analytics::groups(t, &d.categories);
        let mixed = groups.len() > 1;
        let line = |i: &LineItem| {
            let unit = if i.unit.is_empty() || i.unit == "szt" || i.unit == "pcs" { String::new() } else { format!(" {}", i.unit) };
            let cat = if mixed { String::new() } else { format!(" · {}", d.category_name(analytics::canonical(&d.categories, i.category_id))) };
            let how = format!("{}{unit} × {}{cat}", i.qty.display(), format_money(i.unit_price, &t.currency));
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
        };
        let lines = if mixed {
            groups.iter().map(|g| {
                let rows = g.items.iter().filter_map(|i| t.items.get(*i)).map(line).collect_view();
                let label = if g.items.is_empty() { format!("{} (not on a line)", d.category_name(g.category)) } else { d.category_name(g.category) };
                view! {
                    <li class="group" data-test="detail-group">
                        <span class="line"><strong>{label}</strong><strong class="num" data-test="group-sum">{pln(g.amount_pln)}</strong></span>
                    </li>
                    {rows}
                }
            }).collect_view().into_any()
        } else {
            t.items.iter().map(line).collect_view().into_any()
        };
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

/// Expense categories, or income ones when `income` is true.
fn category_select(c: Ctx, sig: RwSignal<Uuid>, test: &'static str, income: Signal<bool>) -> impl IntoView {
    view! {
        <select data-test=test on:change=move |e| if let Ok(id) = event_target_value(&e).parse() { sig.set(id) }>
            {move || c.data.with(|d| d.categories_of(income.get())).into_iter().map(|(id, x)| view! {
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
    // Income saved before income categories existed has an expense one: offer the income list instead.
    if o.is_some_and(|t| t.kind == TxKind::Income) && !d.categories.get(&category.get_untracked()).is_some_and(|x| x.income) {
        category.set(CAT_INCOME_OTHER);
    }
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
                    // Show the day of the saved operation, so a receipt from last week does not seem to vanish.
                    c.filter.show_day(t.date);
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
                <div class="item-head">
                    <input class="item-name" data-test="item-name" placeholder="Item name" aria-label="Name" prop:value=move || i.name.get() on:input=move |e| i.name.set(event_target_value(&e)) />
                    <button class="item-remove" data-test="item-remove" aria-label="Remove item" on:click=move |_| items.update(|v| v.retain(|x| x.key != key))>{icon(Icon::Close)}</button>
                </div>
                <div class="igrid">
                    <label>"Qty"<input data-test="item-qty" inputmode="decimal" prop:value=move || i.qty.get() on:input=move |e| i.qty.set(event_target_value(&e)) /></label>
                    <label>"Unit"<input data-test="item-unit" prop:value=move || i.unit.get() on:input=move |e| i.unit.set(event_target_value(&e)) /></label>
                    <label>"Price"<input data-test="item-price" inputmode="decimal" prop:value=move || i.price.get() on:input=move |e| i.price.set(event_target_value(&e)) /></label>
                    <label>"Discount"<input data-test="item-discount" inputmode="decimal" prop:value=move || i.discount.get() on:input=move |e| i.discount.set(event_target_value(&e)) /></label>
                    <label>"VAT %"<input data-test="item-vat" inputmode="decimal" prop:value=move || i.vat.get() on:input=move |e| i.vat.set(event_target_value(&e)) /></label>
                    <label>"Category"{category_select(c, i.category, "item-category", Signal::stored(false))}</label>
                </div>
            </li>
        }
    };
    // Income has its own categories: switching the kind moves the category to the matching list.
    let set_kind = move |k: TxKind| {
        kind.set(k);
        let want_income = k == TxKind::Income;
        let is_income = c.data.with_untracked(|d| d.categories.get(&category.get_untracked()).is_some_and(|x| x.income));
        if want_income != is_income {
            category.set(if want_income { CAT_INCOME_OTHER } else { CAT_OTHER });
        }
    };
    let upload_status = RwSignal::new(String::new());
    let upload = move |ev: leptos::ev::Event| {
        if let Some(input) = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) {
            crate::scan::open_upload(c, input, upload_status);
        }
    };

    view! {
        <header class="row">
            <h1>{if scanned { "Check the receipt" } else if id.is_some() { "Edit operation" } else { "New operation" }}</h1>
            <button data-test="form-back" on:click=move |_| c.screen.set(id.map_or(Screen::Dashboard, Screen::Tx))>"Back"</button>
        </header>
        {(id.is_none() && !scanned).then(|| view! {
            <label class="button upload" class:disabled=move || c.busy.get() data-test="upload">
                {icon(Icon::Upload)}
                <span>"Upload a receipt, PDF or statement"</span>
                <input type="file" class="file" data-test="upload-file" disabled=move || c.busy.get()
                    accept="image/*,.pdf,application/pdf,.csv,text/csv,text/plain" on:change=upload />
            </label>
            <p class="hint" data-test="upload-status">{move || upload_status.get()}</p>
        })}
        {boxes}
        <div class="segmented">
            <button data-test="kind-expense" class:active=move || kind.get() == TxKind::Expense on:click=move |_| set_kind(TxKind::Expense)>"Expense"</button>
            <button data-test="kind-income" class:active=move || kind.get() == TxKind::Income on:click=move |_| set_kind(TxKind::Income)>"Income"</button>
            <button data-test="kind-transfer" class:active=move || kind.get() == TxKind::Transfer on:click=move |_| set_kind(TxKind::Transfer)>"Transfer"</button>
        </div>
        {scanned.then(|| view! {
            <p class="hint" data-test="scan-hint">{if doubts.merchant || doubts.date || doubts.total || doubts.nip || doubts.items.contains(&true) {
                "Check the highlighted fields, then save."
            } else {
                "Everything adds up. Check and save."
            }}</p>
        })}
        <div class="rform" data-test="form">
            {move || match (items_sum(), receipt) {
                (None, _) | (Some(_), true) => view! {
                    <label class="f total" class:doubt=doubts.total>{if receipt { "Receipt total" } else { "Amount" }}
                        <input data-test="f-amount" inputmode="decimal" placeholder="0,00"
                            prop:value=move || amount.get() on:input=move |e| amount.set(event_target_value(&e)) />
                    </label>
                }.into_any(),
                (Some(sum), false) => view! {
                    <p class="f total"><span>"Amount"</span><strong data-test="f-sum">{sum.map(format_amount).unwrap_or_else(|| "check the items".into())}</strong></p>
                }.into_any(),
            }}
            <label class="f">"Currency"<input data-test="f-currency" maxlength="3" autocapitalize="characters" prop:value=move || currency.get() on:input=move |e| currency.set(event_target_value(&e)) /></label>
            {move || (currency.get().trim().to_uppercase() != "PLN").then(|| view! {
                <label class="f">"In PLN"<input data-test="f-pln" inputmode="decimal" prop:value=move || amount_pln.get() on:input=move |e| amount_pln.set(event_target_value(&e)) /></label>
                <button class="link" data-test="f-nbp" on:click=move |_| {
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
                }>"Use the NBP rate"</button>
            })}
            {move || (!rate_note.get().is_empty()).then(|| view! { <p class="hint">{rate_note.get()}</p> })}
            <label class="f" class:doubt=doubts.date>"Date"<input type="date" data-test="f-date" prop:value=move || date.get() on:input=move |e| date.set(event_target_value(&e)) /></label>
            <label class="f">"Time"<input type="time" data-test="f-time" prop:value=move || time.get() on:input=move |e| time.set(event_target_value(&e)) /></label>
            <label class="f" class:doubt=doubts.merchant>"Merchant"
                <input data-test="f-merchant" list="merchants" autocomplete="off" placeholder="Shop or payer" prop:value=move || merchant.get() on:input=move |e| merchant.set(event_target_value(&e)) />
            </label>
            <datalist id="merchants">
                {move || c.data.with(Data::merchant_list).into_iter().map(|(_, m)| view! { <option value=m.name></option> }).collect_view()}
            </datalist>
            <label class="f" class:doubt=doubts.nip>"NIP"<input data-test="f-nip" inputmode="numeric" autocomplete="off" placeholder="optional" prop:value=move || nip.get() on:input=move |e| nip.set(event_target_value(&e)) /></label>
            {move || {
                let n = normalize_nip(&nip.get());
                (!n.is_empty() && !valid_nip(&n)).then(|| view! {
                    <p class="warn" data-test="nip-warning">"This NIP looks wrong. You can still save."</p>
                })
            }}
            <label class="f">"Category"{category_select(c, category, "f-category", Signal::derive(move || kind.get() == TxKind::Income))}</label>
            <label class="f">"Account"
                <select data-test="f-account" on:change=move |e| if let Ok(i) = event_target_value(&e).parse() { account.set(i) }>
                    {move || c.data.with(Data::account_list).into_iter().map(|(i, a)| view! {
                        <option value=i.to_string() selected=move || account.get() == i>{a.name}</option>
                    }).collect_view()}
                </select>
            </label>
            <label class="f">"Payment"
                <select data-test="f-payment" on:change=move |e| payment.set(Payment::from_code(&event_target_value(&e)))>
                    {Payment::ALL.into_iter().map(|p| view! {
                        <option value=p.code() selected=move || payment.get() == p>{p.label()}</option>
                    }).collect_view()}
                </select>
            </label>
            <label class="f">"Note"<input data-test="f-note" placeholder="—" prop:value=move || note.get() on:input=move |e| note.set(event_target_value(&e)) /></label>
        </div>
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
        <h2 class="rule-head">"Items"</h2>
        <ul class="items">
            <For each=move || items.get() key=|i| i.key children=item_row />
        </ul>
        <button class="dashed" data-test="add-item" on:click=move |_| items.update(|v| v.push(ItemDraft::blank(category.get_untracked())))>{icon(Icon::Plus)}"Add item"</button>
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
    let new_income = RwSignal::new(false);
    let add = move |_| {
        let name = draft.get_untracked().trim().to_string();
        if name.is_empty() {
            return;
        }
        draft.set(String::new());
        busy_task(c, async move {
            let s = session(c)?;
            data::write(&s, Uuid::new_v4(), KIND_CATEGORY, &Category { name, merged_into: None, income: new_income.get_untracked() }).await?;
            after_write(c, &s).await;
            Ok(())
        });
    };
    let card = move |income_side: bool| {
        let d = c.data.get();
        let mut used: std::collections::HashMap<Uuid, (usize, i64)> = Default::default();
        for (_, t) in d.txs.iter().filter(|(_, t)| t.duplicate_of.is_none() && t.kind != TxKind::Transfer) {
            for g in analytics::groups(t, &d.categories) {
                let u = used.entry(g.category).or_default();
                u.0 += 1;
                u.1 += g.amount_pln;
            }
        }
        let all = d.active_categories();
        let side: Vec<(Uuid, Category)> = all.iter().filter(|x| x.1.income == income_side).cloned().collect();
        let count = side.len();
        let rows = side.into_iter().map(|(id, cat)| {
            let name = RwSignal::new(cat.name.clone());
            let income = cat.income;
            let fallback = if income { CAT_INCOME_OTHER } else { CAT_OTHER };
            let others: Vec<(Uuid, Category)> = all.iter().filter(|x| x.0 != id && x.1.income == income).cloned().collect();
            let cat_name = cat.name.clone();
            let target = RwSignal::new(if id == fallback { others.first().map(|x| x.0).unwrap_or(id) } else { fallback });
            let rename = move |_| busy_task(c, async move {
                let s = session(c)?;
                data::write(&s, id, KIND_CATEGORY, &Category { name: name.get_untracked().trim().to_string(), merged_into: None, income }).await?;
                after_write(c, &s).await;
                Ok(())
            });
            let original = cat.name.clone();
            let merge = move |_| {
                let original = original.clone();
                busy_task(c, async move {
                    let s = session(c)?;
                    data::write(&s, id, KIND_CATEGORY, &Category { name: original, merged_into: Some(target.get_untracked()), income }).await?;
                    after_write(c, &s).await;
                    Ok(())
                });
            };
            let (n, sum) = used.get(&id).copied().unwrap_or_default();
            let slot = color_slot(&d, income, id);
            view! {
                <li class="entry" data-test="category-item" data-income=income.to_string()>
                    <details>
                        <summary class="entry-row">
                            <span class=format!("badge tint s{slot}")>{icon(crate::icons::for_category(id, income))}</span>
                            <span class="grow"><span class="name">{cat_name}</span><span class="hint">{usage_line(n, sum)}</span></span>
                            <span class="chev">{icon(Icon::Chevron)}</span>
                        </summary>
                        <div class="edit">
                            <div class="row">
                                <input data-test="category-name" aria-label="Name" prop:value=move || name.get() on:input=move |e| name.set(event_target_value(&e)) />
                                <button data-test="category-save" on:click=rename>"Rename"</button>
                            </div>
                            <div class="row">
                                <label class="merge"><span class="hint">"Merge into"</span>
                                    <select data-test="merge-target" on:change=move |e| if let Ok(i) = event_target_value(&e).parse() { target.set(i) }>
                                        {others.into_iter().map(|(i, x)| view! { <option value=i.to_string() selected=move || target.get() == i>{x.name}</option> }).collect_view()}
                                    </select>
                                </label>
                                <button data-test="merge" on:click=merge>"Merge"</button>
                            </div>
                        </div>
                    </details>
                </li>
            }
        }).collect_view();
        view! {
            <section class="slipcard">
                <p class="slipcard-head"><span>{if income_side { "Income" } else { "Expenses" }}</span><span class="num">{count}</span></p>
                <ul class="ledger">{rows}</ul>
            </section>
        }
    };
    view! {
        <div class="segmented">
            <button data-test="new-category-expense" class:active=move || !new_income.get() on:click=move |_| new_income.set(false)>"Expense"</button>
            <button data-test="new-category-income" class:active=move || new_income.get() on:click=move |_| new_income.set(true)>"Income"</button>
        </div>
        <div class="row">
            <input data-test="new-category" placeholder="New category" prop:value=move || draft.get() on:input=move |e| draft.set(event_target_value(&e)) />
            <button class="primary" data-test="add-category" on:click=add>"Add"</button>
        </div>
        <p class="hint">"Tap a category to rename it or merge it into another."</p>
        {move || card(false)}
        {move || card(true)}
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
        let d = c.data.get();
        let list = d.account_list();
        let last = list.len() <= 1;
        list.into_iter().map(|(id, a)| {
            let mine = d.txs.iter().filter(|(_, t)| t.account_id == id && t.duplicate_of.is_none());
            let (n, balance) = mine.fold((0usize, 0i64), |(n, b), (_, t)| match t.kind {
                TxKind::Income => (n + 1, b + t.amount_pln),
                TxKind::Expense => (n + 1, b - t.amount_pln),
                TxKind::Transfer => (n + 1, b),
            });
            let shown = a.name.clone();
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
            let meta = if n == 0 { "no operations yet".to_string() } else { format!("{n} operations · balance {}", signed_pln(balance)) };
            view! {
                <li class="entry" data-test="account-item">
                    <details>
                        <summary class="entry-row">
                            <span class="badge">{icon(Icon::Wallet)}</span>
                            <span class="grow"><span class="name">{shown}</span><span class="hint">{meta}</span></span>
                            <span class="chev">{icon(Icon::Chevron)}</span>
                        </summary>
                        <div class="edit">
                            <input data-test="account-name" aria-label="Name" prop:value=move || name.get() on:input=move |e| name.set(event_target_value(&e)) />
                            <div class="row">
                                <button data-test="account-save" on:click=rename>"Rename"</button>
                                {(!last).then(|| view! { <button class="quiet-danger" data-test="account-delete" on:click=delete>"Delete"</button> })}
                            </div>
                        </div>
                    </details>
                </li>
            }
        }).collect_view()
    };
    view! {
        <div class="row">
            <input data-test="new-account" placeholder="New account" prop:value=move || draft.get() on:input=move |e| draft.set(event_target_value(&e)) />
            <button class="primary" data-test="add-account" on:click=add>"Add"</button>
        </div>
        <section class="slipcard"><ul class="ledger">{rows}</ul></section>
    }
}

fn merchants(c: Ctx) -> impl IntoView {
    let rows = move || {
        let d = c.data.get();
        d.merchant_list().into_iter().map(|(id, m)| {
            let mine = d.txs.iter().filter(|(_, t)| t.merchant_id == Some(id) && t.duplicate_of.is_none() && t.kind == TxKind::Expense);
            let (n, sum) = mine.fold((0usize, 0i64), |(n, s), (_, t)| (n + 1, s + t.amount_pln));
            let shown = m.name.clone();
            let badge = initial(&m.name);
            let meta = match &m.nip {
                Some(x) => format!("NIP {x} · {}", usage_line(n, sum)),
                None => usage_line(n, sum),
            };
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
                <li class="entry" data-test="merchant-item">
                    <details>
                        <summary class="entry-row">
                            <span class="badge letter">{badge}</span>
                            <span class="grow"><span class="name">{shown}</span><span class="hint">{meta}</span></span>
                            <span class="chev">{icon(Icon::Chevron)}</span>
                        </summary>
                        <div class="edit">
                            <label class="f"><span>"Name"</span>
                                <input data-test="merchant-name" prop:value=move || name.get() on:input=move |e| name.set(event_target_value(&e)) /></label>
                            <label class="f"><span>"NIP"</span>
                                <input data-test="merchant-nip" inputmode="numeric" placeholder="optional" prop:value=move || nip.get() on:input=move |e| nip.set(event_target_value(&e)) /></label>
                            <div class="row">
                                <button data-test="merchant-save" on:click=save>"Save"</button>
                                <button class="quiet-danger" data-test="merchant-delete" on:click=delete>"Delete"</button>
                            </div>
                        </div>
                    </details>
                </li>
            }
        }).collect_view()
    };
    view! {
        <p class="hint">"Merchants are added when you enter an operation. Tap one to rename it or add its NIP."</p>
        <section class="slipcard"><ul class="ledger">{rows}</ul></section>
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
