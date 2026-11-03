use crate::api::{ApiFail, Http};
use crate::flows::{self, LoginResult, Session};
use crate::data::{self, Data};
use crate::{device, finance, store::Kv};
use leptos::prelude::*;
use leptos::task::spawn_local;
use mm_core::crypto::Phrase;
use mm_core::date::{Date, Month};
use mm_core::sync::{IntegrityIssue, Keys, LocalStore, SyncError, Syncer};
use std::rc::Rc;
use uuid::Uuid;
use wasm_bindgen::{JsCast, closure::Closure};

/// The key is wiped after this much user inactivity.
const IDLE_MS: f64 = 5.0 * 60.0 * 1000.0;
/// Or if the app stayed in the background longer than this.
const HIDDEN_MS: f64 = 60.0 * 1000.0;
const SYNC_EVERY_MS: i32 = 30_000;
const CONFIRM_WORDS: usize = 3;

#[derive(Clone, PartialEq)]
pub enum Screen {
    Loading,
    /// Opened over plain HTTP from another device: the browser disables what the app needs.
    Insecure,
    Welcome,
    Invite,
    Warning,
    ShowPhrase,
    ConfirmPhrase,
    Passkey,
    Locked,
    /// Entering the 12 words. `true`: a device without PRF, the key stays on the device.
    Phrase(bool),
    Dashboard,
    Operations,
    /// None creates a new operation.
    Edit(Option<Uuid>),
    Category(Uuid),
    Scan,
    Settings,
    Manage(finance::Manage),
    Export,
    Import,
    Duplicates,
}

#[derive(Clone, Copy)]
pub struct Ctx {
    pub screen: RwSignal<Screen>,
    pub error: RwSignal<String>,
    pub busy: RwSignal<bool>,
    pub session: RwSignal<Option<Session>, LocalStorage>,
    pub data: RwSignal<Data>,
    pub status: RwSignal<String>,
    pub issues: RwSignal<Vec<String>>,
    pub filter: finance::FilterState,
    /// A recognized receipt waiting for review. It never contains the photo.
    pub draft: StoredValue<Option<finance::Draft>, LocalStorage>,
    phrase: StoredValue<Option<Rc<Phrase>>, LocalStorage>,
    invite: RwSignal<String>,
    label: RwSignal<String>,
    /// A new passkey must be added after recovery.
    need_passkey: RwSignal<bool>,
    last_active: StoredValue<f64>,
    hidden_at: StoredValue<Option<f64>>,
}

pub fn today() -> Date {
    let d = js_sys::Date::new_0();
    Date::new(d.get_full_year() as i32, d.get_month() + 1, d.get_date()).unwrap_or(Date { year: 2026, month: 1, day: 1 })
}

fn now() -> f64 {
    js_sys::Date::now()
}

fn online() -> bool {
    web_sys::window().map(|w| w.navigator().on_line()).unwrap_or(false)
}

fn lock(c: Ctx) {
    // Dropping the last reference wipes the key. The connection must be closed too:
    // a leftover one would block deleteDatabase for the rest of the page's life.
    if let Some(s) = c.session.try_update(Option::take).flatten() {
        spawn_local(async move {
            let _ = s.store.close().await;
        });
    }
    c.data.set(Data::default());
    c.draft.set_value(None);
    c.phrase.set_value(None);
    c.issues.set(vec![]);
    c.status.set(String::new());
    c.error.set(String::new());
    c.screen.set(Screen::Locked);
}

pub async fn reload(c: Ctx, s: &Session) {
    match data::load(s).await {
        Ok((d, bad)) => {
            if c.session.with_untracked(|cur| cur.as_ref().is_some_and(|x| x.user == s.user)) {
                c.data.set(d);
                if !bad.is_empty() {
                    c.issues.update(|i| i.extend(bad.iter().map(|id| format!("Record {id} cannot be decrypted"))));
                }
            }
        }
        Err(e) => c.error.set(format!("Could not read local data: {e}")),
    }
}

fn describe(i: &IntegrityIssue) -> String {
    match i {
        IntegrityIssue::BadCiphertext(id) => format!("The server returned a corrupted or tampered record {id}"),
        IntegrityIssue::Rollback { id, known, got } => format!("The server returned an old version of record {id}: {got} instead of {known}"),
        IntegrityIssue::Forked { id, .. } => format!("The server returned a different record {id} with the same version number"),
        IntegrityIssue::Missing { id, .. } => format!("Record {id} is missing on the server"),
    }
}

pub async fn run_sync(c: Ctx) {
    let Some(s) = c.session.get_untracked() else { return };
    if !online() {
        c.status.set("Offline: changes will be sent when back online".into());
        return;
    }
    let Ok(_one) = s.store.sync_lock().await else { return };
    c.status.set("Syncing…".into());
    let keys = Keys { user_id: s.user, dek: &s.dek };
    let res = Syncer { local: &*s.store, remote: &Http, keys }.run().await;
    match res {
        Ok(rep) => {
            c.issues.set(rep.issues.iter().map(describe).collect());
            c.status.set("Synced".into());
        }
        Err(SyncError::Remote(ApiFail::Network)) => c.status.set("No connection: changes saved on this device".into()),
        Err(SyncError::Remote(ApiFail::Status(401, _))) => c.status.set("Login needed: lock the app and sign in again. Changes saved".into()),
        Err(e) => c.status.set(format!("Sync error: {e}. Changes saved on this device")),
    }
    reload(c, &s).await;
}

async fn enter(c: Ctx, s: Session) {
    c.last_active.set_value(now());
    c.session.set(Some(s.clone()));
    c.error.set(String::new());
    c.screen.set(if c.need_passkey.get_untracked() { Screen::Settings } else { Screen::Dashboard });
    reload(c, &s).await;
    run_sync(c).await;
}

pub fn busy_task<F: Future<Output = Result<(), String>> + 'static>(c: Ctx, f: F) {
    if c.busy.get_untracked() {
        return;
    }
    c.busy.set(true);
    c.error.set(String::new());
    spawn_local(async move {
        if let Err(e) = f.await {
            c.error.set(e);
        }
        c.busy.set(false);
    });
}

fn on_window<F: FnMut() + 'static>(name: &str, f: F) {
    let cb = Closure::<dyn FnMut()>::new(f);
    if let Some(w) = web_sys::window() {
        let _ = w.add_event_listener_with_callback(name, cb.as_ref().unchecked_ref());
    }
    cb.forget();
}

fn watchers(c: Ctx) {
    for ev in ["pointerdown", "keydown"] {
        on_window(ev, move || c.last_active.set_value(now()));
    }
    on_window("online", move || spawn_local(run_sync(c)));
    on_window("offline", move || c.status.set("Offline: changes are saved on this device".into()));

    let vis = Closure::<dyn FnMut()>::new(move || {
        let Some(doc) = web_sys::window().and_then(|w| w.document()) else { return };
        if doc.visibility_state() == web_sys::VisibilityState::Hidden {
            c.hidden_at.set_value(Some(now()));
        } else {
            if c.hidden_at.get_value().is_some_and(|t| now() - t > HIDDEN_MS)
                && c.session.with_untracked(Option::is_some)
            {
                lock(c);
            }
            c.hidden_at.set_value(None);
        }
    });
    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        let _ = doc.add_event_listener_with_callback("visibilitychange", vis.as_ref().unchecked_ref());
    }
    vis.forget();

    let tick = Closure::<dyn FnMut()>::new(move || {
        if c.session.with_untracked(Option::is_some) {
            if now() - c.last_active.get_value() > IDLE_MS {
                lock(c);
            } else {
                spawn_local(run_sync(c));
            }
        }
    });
    if let Some(w) = web_sys::window() {
        let _ = w.set_interval_with_callback_and_timeout_and_arguments_0(tick.as_ref().unchecked_ref(), SYNC_EVERY_MS);
    }
    tick.forget();
}

#[component]
pub fn App() -> impl IntoView {
    let c = Ctx {
        screen: RwSignal::new(Screen::Loading),
        error: RwSignal::new(String::new()),
        busy: RwSignal::new(false),
        session: RwSignal::new_local(None),
        data: RwSignal::new(Data::default()),
        draft: StoredValue::new_local(None),
        filter: finance::FilterState::new(Month { year: today().year, month: today().month }),
        status: RwSignal::new(String::new()),
        issues: RwSignal::new(vec![]),
        phrase: StoredValue::new_local(None),
        invite: RwSignal::new(String::new()),
        label: RwSignal::new(String::new()),
        need_passkey: RwSignal::new(false),
        last_active: StoredValue::new(now()),
        hidden_at: StoredValue::new(None),
    };
    watchers(c);
    let secure = web_sys::window().is_some_and(|w| w.is_secure_context());
    if !secure {
        c.screen.set(Screen::Insecure);
    }
    spawn_local(async move {
        if !secure {
            return;
        }
        let known = match Kv::open().await {
            Ok(kv) => device::users(&kv).await.map(|u| !u.is_empty()).unwrap_or(false),
            Err(_) => false,
        };
        c.screen.set(if known { Screen::Locked } else { Screen::Welcome });
    });

    view! {
        <main data-test="app">
            {move || (!c.error.get().is_empty()).then(|| view! { <p class="error" data-test="error">{c.error.get()}</p> })}
            {move || match c.screen.get() {
                Screen::Loading => view! { <p>"Loading…"</p> }.into_any(),
                Screen::Insecure => insecure().into_any(),
                Screen::Welcome => welcome(c).into_any(),
                Screen::Invite => invite(c).into_any(),
                Screen::Warning => warning(c).into_any(),
                Screen::ShowPhrase => show_phrase(c).into_any(),
                Screen::ConfirmPhrase => confirm_phrase(c).into_any(),
                Screen::Passkey => passkey(c).into_any(),
                Screen::Locked => locked(c).into_any(),
                Screen::Phrase(remember) => phrase_entry(c, remember).into_any(),
                Screen::Dashboard => finance::dashboard(c).into_any(),
                Screen::Operations => finance::operations(c).into_any(),
                Screen::Edit(id) => finance::edit(c, id).into_any(),
                Screen::Category(id) => finance::category(c, id).into_any(),
                Screen::Scan => crate::scan::scan(c).into_any(),
                Screen::Settings => settings(c).into_any(),
                Screen::Manage(m) => finance::manage(c, m).into_any(),
                Screen::Export => finance::export(c).into_any(),
                Screen::Import => crate::import::import(c).into_any(),
                Screen::Duplicates => crate::import::duplicates(c).into_any(),
            }}
        </main>
        {move || c.session.with(Option::is_some).then(|| finance::nav(c))}
    }
}

fn insecure() -> impl IntoView {
    let here = web_sys::window().and_then(|w| w.location().href().ok()).unwrap_or_default();
    view! {
        <h1>"Open the app over HTTPS"</h1>
        <p data-test="insecure">"This address is not secure: "<strong>{here}</strong></p>
        <p>"On such a page the browser turns off passkeys, the camera, offline mode and safe storage, so the app cannot keep your data safe."</p>
        <p>"On this computer, open "<strong>"http://localhost:8080"</strong>". On a phone, use an address that starts with https://, as described in docs/DEPLOY.md."</p>
    }
}

fn welcome(c: Ctx) -> impl IntoView {
    view! {
        <h1>"mm"</h1>
        <p>"Personal expense tracking. The server sees only encrypted data."</p>
        <button class="primary" data-test="go-register" on:click=move |_| c.screen.set(Screen::Invite)>"Create account"</button>
        <button data-test="go-login" on:click=move |_| c.screen.set(Screen::Locked)>"I already have an account"</button>
    }
}

fn invite(c: Ctx) -> impl IntoView {
    let next = move |_| busy_task(c, async move {
        flows::check_invite(&c.invite.get_untracked()).await?;
        c.screen.set(Screen::Warning);
        Ok(())
    });
    view! {
        <h1>"New account"</h1>
        <label>"Invite"
            <input data-test="invite" autocomplete="off" prop:value=move || c.invite.get()
                on:input=move |e| c.invite.set(event_target_value(&e)) />
        </label>
        <label>"Name on this device"
            <input data-test="label" autocomplete="off" prop:value=move || c.label.get()
                on:input=move |e| c.label.set(event_target_value(&e)) />
        </label>
        <p class="hint">"The name is shown only in the passkey list. The server does not store it."</p>
        <button class="primary" data-test="invite-next" disabled=move || c.busy.get() || c.invite.get().trim().is_empty()
            on:click=next>"Next"</button>
        <button on:click=move |_| c.screen.set(Screen::Welcome)>"Back"</button>
    }
}

fn warning(c: Ctx) -> impl IntoView {
    let ok = RwSignal::new(false);
    view! {
        <h1>"Important"</h1>
        <p>"There is no password reset. Data is encrypted on your device, and the server has no key."</p>
        <p><strong>"If you lose both the passkey and the 12 words, your data is gone forever. Nobody can recover it."</strong></p>
        <label class="check">
            <input data-test="accept" type="checkbox" on:change=move |e| ok.set(event_target_checked(&e)) />
            "I understand"
        </label>
        <button class="primary" data-test="warning-next" disabled=move || !ok.get() on:click=move |_| {
            match Phrase::generate() {
                Ok(p) => { c.phrase.set_value(Some(Rc::new(p))); c.screen.set(Screen::ShowPhrase); }
                Err(e) => c.error.set(e.to_string()),
            }
        }>"Show the 12 words"</button>
    }
}

fn show_phrase(c: Ctx) -> impl IntoView {
    let words = c.phrase.with_value(|p| p.as_ref().map(|p| p.words()).unwrap_or_default());
    view! {
        <h1>"Write down the 12 words"</h1>
        <p>"They are shown once. Write them on paper and keep it away from your phone."</p>
        <ol class="words" data-test="words">
            {words.into_iter().map(|w| view! { <li>{w}</li> }).collect_view()}
        </ol>
        <button class="primary" data-test="phrase-next" on:click=move |_| c.screen.set(Screen::ConfirmPhrase)>"I wrote them down"</button>
    }
}

fn random_positions() -> Vec<usize> {
    let mut b = [0u8; 16];
    let _ = getrandom::fill(&mut b);
    let mut v = vec![];
    for x in b {
        let i = x as usize % 12;
        if !v.contains(&i) {
            v.push(i);
        }
    }
    v.truncate(CONFIRM_WORDS);
    v.sort();
    v
}

fn confirm_phrase(c: Ctx) -> impl IntoView {
    let pos = random_positions();
    let inputs: Vec<RwSignal<String>> = pos.iter().map(|_| RwSignal::new(String::new())).collect();
    let fields = pos.clone().into_iter().zip(inputs.clone()).map(|(i, sig)| view! {
        <label>{format!("Word no. {}", i + 1)}
            <input data-test="confirm-word" data-pos=i.to_string() autocomplete="off" autocapitalize="none"
                prop:value=move || sig.get() on:input=move |e| sig.set(event_target_value(&e)) />
        </label>
    }).collect_view();
    let check = move |_| {
        let words = c.phrase.with_value(|p| p.as_ref().map(|p| p.words()).unwrap_or_default());
        let ok = pos.iter().zip(&inputs).all(|(i, s)| s.get_untracked().trim().eq_ignore_ascii_case(&words[*i]));
        if ok {
            c.error.set(String::new());
            c.screen.set(Screen::Passkey);
        } else {
            c.error.set("The words do not match. Check what you wrote".into());
        }
    };
    view! {
        <h1>"Check your copy"</h1>
        <p>"Enter the words with these numbers."</p>
        {fields}
        <button class="primary" data-test="confirm-next" on:click=check>"Check"</button>
        <button on:click=move |_| { c.error.set(String::new()); c.screen.set(Screen::ShowPhrase) }>"See the words again"</button>
    }
}

/// Shown while a passkey request runs, so a window that never opens does not look like a frozen app.
fn waiting(c: Ctx) -> impl IntoView {
    view! {
        <p class="hint" data-test="passkey-wait" hidden=move || !c.busy.get()>"Waiting for the passkey window…"</p>
        <button data-test="passkey-cancel" hidden=move || !c.busy.get() on:click=move |_| crate::webauthn::cancel()>"Cancel"</button>
    }
}

fn passkey(c: Ctx) -> impl IntoView {
    let create = move |_| busy_task(c, async move {
        let phrase = c.phrase.with_value(|p| p.clone()).ok_or("phrase not found")?;
        let s = flows::register(&c.invite.get_untracked(), &c.label.get_untracked(), &phrase).await?;
        c.phrase.set_value(None);
        c.invite.set(String::new());
        enter(c, s).await;
        Ok(())
    });
    view! {
        <h1>"Create a passkey"</h1>
        <p>"You will sign in with Face ID, a fingerprint or the device code."</p>
        <button class="primary" data-test="create-passkey" disabled=move || c.busy.get() on:click=create>"Create passkey"</button>
        {waiting(c)}
    }
}

fn locked(c: Ctx) -> impl IntoView {
    let unlock = move |_| busy_task(c, async move {
        match flows::login(online()).await? {
            LoginResult::Unlocked(s) => enter(c, s).await,
            LoginResult::NeedPhrase => c.screen.set(Screen::Phrase(true)),
        }
        Ok(())
    });
    view! {
        <h1>"mm"</h1>
        <button class="primary" data-test="login" disabled=move || c.busy.get() on:click=unlock>"Sign in with passkey"</button>
        {waiting(c)}
        <button data-test="go-recover" on:click=move |_| { c.error.set(String::new()); c.screen.set(Screen::Phrase(false)) }>"I lost my passkey"</button>
        <button on:click=move |_| { c.error.set(String::new()); c.screen.set(Screen::Welcome) }>"Create account"</button>
    }
}

fn phrase_entry(c: Ctx, remember: bool) -> impl IntoView {
    let text = RwSignal::new(String::new());
    let submit = move |_| busy_task(c, async move {
        let phrase = Phrase::parse(&text.get_untracked()).map_err(|_| "Check the words: the phrase is invalid".to_string())?;
        text.set(String::new());
        let s = flows::recover(&phrase, remember).await?;
        if !remember {
            c.need_passkey.set(true);
        }
        enter(c, s).await;
        Ok(())
    });
    view! {
        <h1>"12 words"</h1>
        {if remember {
            view! { <p data-test="no-prf-note">"This device cannot encrypt with a passkey. Enter the words once, and later sign-ins will use the passkey."</p> }.into_any()
        } else {
            view! { <p>"You will need to add a new passkey after signing in."</p> }.into_any()
        }}
        <textarea data-test="phrase-input" autocomplete="off" autocapitalize="none" spellcheck="false" rows="4"
            prop:value=move || text.get() on:input=move |e| text.set(event_target_value(&e))></textarea>
        <button class="primary" data-test="phrase-submit" disabled=move || c.busy.get() on:click=submit>"Sign in"</button>
        <button on:click=move |_| c.screen.set(Screen::Locked)>"Back"</button>
    }
}

fn settings(c: Ctx) -> impl IntoView {
    let phrase_text = RwSignal::new(String::new());
    let done = RwSignal::new(String::new());
    let prf = move || c.session.with(|s| s.as_ref().map(|s| s.prf).unwrap_or(false));
    let add = move |_| busy_task(c, async move {
        let phrase = Phrase::parse(&phrase_text.get_untracked()).map_err(|_| "Check the words: the phrase is invalid".to_string())?;
        phrase_text.set(String::new());
        let s = c.session.get_untracked().ok_or("no session")?;
        let with_prf = flows::add_passkey(&s, &phrase).await?;
        c.need_passkey.set(false);
        done.set(if with_prf { "Passkey added".into() } else { "Passkey added, this device has no PRF".into() });
        Ok(())
    });
    let confirm_delete = RwSignal::new(false);
    let delete = move |_| busy_task(c, async move {
        let s = c.session.get_untracked().ok_or("no session")?;
        flows::delete_account(&s).await?;
        lock(c);
        c.screen.set(Screen::Welcome);
        Ok(())
    });
    let forget = move |_| busy_task(c, async move {
        let s = c.session.get_untracked().ok_or("no session")?;
        let unsent = s.store.all().await?.iter().filter(|r| r.pending_blob.is_some()).count();
        if unsent > 0 {
            return Err(format!("This device has unsent changes: {unsent}. Sync first"));
        }
        let _ = crate::api::post_done("/api/logout").await;
        flows::wipe_local(&s).await?;
        lock(c);
        c.screen.set(Screen::Welcome);
        Ok(())
    });
    view! {
        <header class="row">
            <h1>"Settings"</h1>
        </header>
        <p data-test="prf-status">{move || if prf() { "Passkey encryption: on" } else { "Passkey encryption: off, the key is stored on this device" }}</p>
        <button data-test="lock" on:click=move |_| lock(c)>"Lock"</button>
        <h2>"Data"</h2>
        <button data-test="manage-categories" on:click=move |_| c.screen.set(Screen::Manage(finance::Manage::Categories))>"Categories"</button>
        <button data-test="manage-accounts" on:click=move |_| c.screen.set(Screen::Manage(finance::Manage::Accounts))>"Accounts"</button>
        <button data-test="manage-merchants" on:click=move |_| c.screen.set(Screen::Manage(finance::Manage::Merchants))>"Merchants"</button>
        <button data-test="go-import" on:click=move |_| c.screen.set(Screen::Import)>"Import a bank statement"</button>
        <button data-test="go-export" on:click=move |_| c.screen.set(Screen::Export)>"Export data"</button>
        <h2>"New passkey"</h2>
        {move || c.need_passkey.get().then(|| view! { <p class="error" data-test="need-passkey">"Add a new passkey to sign in without the 12 words again."</p> })}
        <p class="hint">"Adding one requires the 12 words."</p>
        <textarea data-test="add-phrase" autocomplete="off" autocapitalize="none" spellcheck="false" rows="4"
            prop:value=move || phrase_text.get() on:input=move |e| phrase_text.set(event_target_value(&e))></textarea>
        <button class="primary" data-test="add-passkey" disabled=move || c.busy.get() on:click=add>"Add passkey"</button>
        <p data-test="add-done">{move || done.get()}</p>
        <h2>"Device and account"</h2>
        <button data-test="forget-device" on:click=forget>"Sign out and erase this device"</button>
        <button class="danger" data-test="delete-account" hidden=move || confirm_delete.get()
            on:click=move |_| confirm_delete.set(true)>"Delete account"</button>
        <div class="confirm" hidden=move || !confirm_delete.get()>
            <p class="error">"The account and all data will be deleted permanently."</p>
            <button class="danger" data-test="delete-confirm" disabled=move || c.busy.get() on:click=delete>"Delete forever"</button>
            <button on:click=move |_| confirm_delete.set(false)>"Cancel"</button>
        </div>
    }
}
