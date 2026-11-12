//! Integration tests on a real PostgreSQL:
//! TEST_DATABASE_URL=postgres://mm:mm-dev@127.0.0.1:55432/mm cargo test -p mm-server
//! WebAuthn is covered separately, in e2e with Chrome's virtual authenticator.

use mm_core::api::*;
use mm_core::crypto::{Dek, Phrase, WrapSlot, phrase_challenge, unwrap_dek, wrap_dek};
use mm_core::record::Envelope;
use mm_core::sync::{Keys, LocalRecord, LocalStore, Remote, Syncer, current, save};
use mm_server::{Config, CSP, Stats, migrate, router, stats};
use serde_json::json;
use sqlx::{Connection, Executor, PgConnection, PgPool, Row, postgres::PgPoolOptions};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use uuid::Uuid;

const CANARY: &str = "CANARY-Biedronka-1234,56-Mleko";
/// Room for every test; the quota test sets a small one and fills it.
const QUOTA: i64 = 64 << 20;

fn base_url() -> String {
    std::env::var("TEST_DATABASE_URL").unwrap_or_else(|_| "postgres://mm:mm-dev@127.0.0.1:55432/mm".into())
}

struct Ctx {
    url: String,
    db: PgPool,
}

/// A stand-in for the NBP API that answers like the real one.
async fn fake_nbp() -> String {
    use axum::{Router, extract::Path, routing::get};
    let app = Router::new().route("/api/exchangerates/rates/a/{code}/{from}/{to}/", get(|Path((code, from, _to)): Path<(String, String, String)>| async move {
        if code == "xxx" || from.starts_with("2099") {
            return (axum::http::StatusCode::NOT_FOUND, String::from("404 NotFound")).into_response_parts();
        }
        (axum::http::StatusCode::OK, format!(r#"{{"table":"A","currency":"euro","code":"{}","rates":[{{"no":"211/A/NBP/2026","effectiveDate":"2026-10-30","mid":4.3012}},{{"no":"212/A/NBP/2026","effectiveDate":"2026-11-02","mid":4.2987}}]}}"#, code.to_uppercase())).into_response_parts()
    }));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    url
}

trait Parts {
    fn into_response_parts(self) -> axum::response::Response;
}

impl Parts for (axum::http::StatusCode, String) {
    fn into_response_parts(self) -> axum::response::Response {
        use axum::response::IntoResponse;
        self.into_response()
    }
}

async fn start_with(limit: u32, behind_proxy: bool) -> Ctx {
    start_cfg(limit, behind_proxy, 1000, QUOTA).await
}

async fn start_cfg(limit: u32, behind_proxy: bool, register_per_hour: u32, quota_bytes: i64) -> Ctx {
    let base = base_url();
    let name = format!("t_{}", Uuid::new_v4().simple());
    let mut admin = PgConnection::connect(&base).await.expect("start the database: docker compose -f compose.dev.yml up -d");
    admin.execute(format!("CREATE DATABASE {name}").as_str()).await.unwrap();
    let (prefix, _) = base.rsplit_once('/').unwrap();
    let db = PgPoolOptions::new().connect(&format!("{prefix}/{name}")).await.unwrap();
    migrate(&db).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let cfg = Config {
        rp_id: "localhost".into(),
        origin: format!("http://localhost:{}", addr.port()),
        static_dir: None,
        behind_proxy,
        auth_per_minute: limit,
        register_per_hour,
        quota_bytes,
        nbp_url: fake_nbp().await,
    };
    let app = router(db.clone(), &cfg).into_make_service_with_connect_info::<SocketAddr>();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Ctx { url: format!("http://127.0.0.1:{}", addr.port()), db }
}

async fn start() -> Ctx {
    start_with(1000, false).await
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().cookie_store(true).build().unwrap()
}

struct Acct {
    user: Uuid,
    dek: Dek,
    phrase: Phrase,
}

/// An account created directly in the database: no passkey involved here.
async fn account(c: &Ctx) -> Acct {
    let user = Uuid::new_v4();
    let dek = Dek::generate().unwrap();
    let phrase = Phrase::generate().unwrap();
    sqlx::query("INSERT INTO users (id, phrase_pub, wrapped_dek_phrase) VALUES ($1,$2,$3)")
        .bind(user)
        .bind(phrase.auth_key().public())
        .bind(wrap_dek(&phrase.kek(), &dek, user, WrapSlot::Phrase).unwrap())
        .execute(&c.db).await.unwrap();
    Acct { user, dek, phrase }
}

async fn recover(c: &Ctx, http: &reqwest::Client, phrase: &Phrase) -> reqwest::Response {
    let key = phrase.auth_key();
    let s: RecoverStartResp = http.post(format!("{}/api/auth/recover/start", c.url))
        .json(&RecoverStartReq { phrase_pub: B64(key.public()) })
        .send().await.unwrap().json().await.unwrap();
    let sig = key.sign(&phrase_challenge("login", s.challenge_id, &s.nonce.0));
    http.post(format!("{}/api/auth/recover/finish", c.url))
        .json(&RecoverFinishReq { challenge_id: s.challenge_id, signature: B64(sig) })
        .send().await.unwrap()
}

async fn login(c: &Ctx, a: &Acct) -> reqwest::Client {
    let http = client();
    assert_eq!(recover(c, &http, &a.phrase).await.status(), 200);
    http
}

#[derive(Default)]
struct Mem {
    recs: RefCell<BTreeMap<Uuid, LocalRecord>>,
    cursor: Cell<u64>,
}

impl LocalStore for Mem {
    type Guard = ();
    async fn lock(&self) -> Result<(), String> {
        Ok(())
    }
    async fn get(&self, id: Uuid) -> Result<Option<LocalRecord>, String> {
        Ok(self.recs.borrow().get(&id).cloned())
    }
    async fn put(&self, r: LocalRecord) -> Result<(), String> {
        self.recs.borrow_mut().insert(r.id, r);
        Ok(())
    }
    async fn all(&self) -> Result<Vec<LocalRecord>, String> {
        Ok(self.recs.borrow().values().cloned().collect())
    }
    async fn cursor(&self) -> Result<u64, String> {
        Ok(self.cursor.get())
    }
    async fn set_cursor(&self, s: u64) -> Result<(), String> {
        self.cursor.set(s);
        Ok(())
    }
}

/// A real HTTP client that remembers everything it sent.
struct Http {
    base: String,
    c: reqwest::Client,
    sent: RefCell<Vec<u8>>,
}

impl Remote for Http {
    type Error = String;
    async fn pull(&self, since: u64) -> Result<PullResp, String> {
        let r = self.c.get(format!("{}/api/sync/pull?since={since}", self.base)).send().await.map_err(|e| e.to_string())?;
        r.json().await.map_err(|e| e.to_string())
    }
    async fn push(&self, items: Vec<PushItem>) -> Result<Vec<PushResult>, String> {
        let req = PushReq { items };
        let body = serde_json::to_vec(&req).unwrap();
        {
            let mut sent = self.sent.borrow_mut();
            sent.extend_from_slice(&body);
            for i in &req.items {
                sent.extend_from_slice(&i.blob.0); // and in decoded form
            }
        }
        let r = self.c.post(format!("{}/api/sync/push", self.base)).json(&req).send().await.map_err(|e| e.to_string())?;
        Ok(r.json::<PushResp>().await.map_err(|e| e.to_string())?.results)
    }
}

async fn run_sync(c: &Ctx, a: &Acct, http: &reqwest::Client, local: &Mem) -> (mm_core::sync::SyncReport, Vec<u8>) {
    let remote = Http { base: c.url.clone(), c: http.clone(), sent: RefCell::default() };
    let rep = Syncer { local, remote: &remote, keys: Keys { user_id: a.user, dek: &a.dek } }.run().await.unwrap();
    (rep, remote.sent.into_inner())
}

/// A full text dump of all tables, including binary fields.
async fn dump(db: &PgPool) -> Vec<u8> {
    let tables: Vec<String> = sqlx::query("SELECT tablename FROM pg_tables WHERE schemaname = 'public'")
        .fetch_all(db).await.unwrap().iter().map(|r| r.get(0)).collect();
    let mut out = vec![];
    for t in tables {
        for r in sqlx::query(&format!("SELECT (t)::text FROM {t} t")).fetch_all(db).await.unwrap() {
            out.extend_from_slice(r.get::<String, _>(0).as_bytes());
            out.push(b'\n');
        }
    }
    for r in sqlx::query("SELECT blob FROM records").fetch_all(db).await.unwrap() {
        out.extend_from_slice(&r.get::<Vec<u8>, _>(0));
    }
    out
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

#[tokio::test]
async fn no_plaintext_in_database_or_requests() {
    let c = start().await;
    let a = account(&c).await;
    let http = login(&c, &a).await;
    let local = Mem::default();
    let id = Uuid::new_v4();
    let keys = Keys { user_id: a.user, dek: &a.dek };
    let env = Envelope::new("note", json!({
        "merchant": CANARY, "amount": CANARY, "category": CANARY, "date": CANARY,
    }));
    save(&local, &keys, id, &env).await.unwrap();
    let (_, sent) = run_sync(&c, &a, &http, &local).await;
    assert!(!sent.is_empty(), "the test must really send something");

    // Control check of the detector itself: it fires on plaintext.
    assert!(contains(&serde_json::to_vec(&env).unwrap(), CANARY.as_bytes()));

    let db = dump(&c.db).await;
    assert!(db.len() > 500);
    for needle in [CANARY.as_bytes(), b"merchant", b"amount", b"category"] {
        assert!(!contains(&db, needle), "plaintext in the database");
        assert!(!contains(&sent, needle), "plaintext in a request");
    }
    // The raw DEK appears nowhere.
    assert!(!contains(&db, a.dek.expose()));
    assert!(!contains(&sent, a.dek.expose()));
}

#[tokio::test]
async fn server_metadata_is_minimal() {
    let c = start().await;
    let cols: Vec<String> = sqlx::query(
        "SELECT column_name FROM information_schema.columns WHERE table_schema = 'public' AND table_name = 'records' ORDER BY 1")
        .fetch_all(&c.db).await.unwrap().iter().map(|r| r.get(0)).collect();
    assert_eq!(cols, ["blob", "id", "revision", "seq", "size", "updated_at", "user_id"]);
}

#[tokio::test]
async fn two_devices_sync_through_the_real_server() {
    let c = start().await;
    let a = account(&c).await;
    let (h1, h2) = (login(&c, &a).await, login(&c, &a).await);
    let (d1, d2) = (Mem::default(), Mem::default());
    let keys = Keys { user_id: a.user, dek: &a.dek };
    let id = Uuid::new_v4();
    save(&d1, &keys, id, &Envelope::new("note", json!({"x": 0, "y": 0}))).await.unwrap();
    run_sync(&c, &a, &h1, &d1).await;
    run_sync(&c, &a, &h2, &d2).await;
    save(&d1, &keys, id, &Envelope::new("note", json!({"x": 1, "y": 0}))).await.unwrap();
    save(&d2, &keys, id, &Envelope::new("note", json!({"x": 0, "y": 1}))).await.unwrap();
    run_sync(&c, &a, &h1, &d1).await;
    let (rep, _) = run_sync(&c, &a, &h2, &d2).await;
    assert_eq!(rep.merged, 1);
    run_sync(&c, &a, &h1, &d1).await;
    for d in [&d1, &d2] {
        let r = d.recs.borrow()[&id].clone();
        assert_eq!(current(&keys, &r).unwrap().unwrap().data, json!({"x": 1, "y": 1}));
    }
}

#[tokio::test]
async fn parallel_pushes_never_overwrite_each_other() {
    let c = start().await;
    let a = account(&c).await;
    let http = login(&c, &a).await;
    let id = Uuid::new_v4();
    let mut tasks = vec![];
    for n in 0..16u8 {
        let (h, u) = (http.clone(), c.url.clone());
        tasks.push(tokio::spawn(async move {
            let req = PushReq { items: vec![PushItem { id, expected_revision: 0, blob: B64(vec![n; 64]) }] };
            let r: PushResp = h.post(format!("{u}/api/sync/push")).json(&req).send().await.unwrap().json().await.unwrap();
            r.results[0].clone()
        }));
    }
    let mut ok = 0;
    for t in tasks {
        ok += matches!(t.await.unwrap(), PushResult::Ok { .. }) as u32;
    }
    assert_eq!(ok, 1, "exactly one write must succeed");
}

#[tokio::test]
async fn replayed_push_is_idempotent() {
    let c = start().await;
    let a = account(&c).await;
    let http = login(&c, &a).await;
    let req = PushReq { items: vec![PushItem { id: Uuid::new_v4(), expected_revision: 0, blob: B64(vec![1; 64]) }] };
    for _ in 0..2 {
        let r: PushResp = http.post(format!("{}/api/sync/push", c.url)).json(&req).send().await.unwrap().json().await.unwrap();
        assert!(matches!(r.results[0], PushResult::Ok { revision: 1, .. }));
    }
    let n: i64 = sqlx::query("SELECT seq FROM users WHERE id = $1").bind(a.user).fetch_one(&c.db).await.unwrap().get(0);
    assert_eq!(n, 1);
}

#[tokio::test]
async fn users_are_isolated() {
    let c = start().await;
    let (a, b) = (account(&c).await, account(&c).await);
    let (ha, hb) = (login(&c, &a).await, login(&c, &b).await);
    let id = Uuid::new_v4();
    let req = PushReq { items: vec![PushItem { id, expected_revision: 0, blob: B64(vec![1; 64]) }] };
    ha.post(format!("{}/api/sync/push", c.url)).json(&req).send().await.unwrap();
    let p: PullResp = hb.get(format!("{}/api/sync/pull?since=0", c.url)).send().await.unwrap().json().await.unwrap();
    assert!(p.changes.is_empty());
    // The same id for another user neither conflicts nor overwrites.
    let r: PushResp = hb.post(format!("{}/api/sync/push", c.url)).json(&req).send().await.unwrap().json().await.unwrap();
    assert!(matches!(r.results[0], PushResult::Ok { .. }));
}

#[tokio::test]
async fn sync_requires_a_session() {
    let c = start().await;
    let r = client().get(format!("{}/api/sync/pull?since=0", c.url)).send().await.unwrap();
    assert_eq!(r.status(), 401);
    let r = client().post(format!("{}/api/sync/push", c.url)).json(&PushReq { items: vec![] }).send().await.unwrap();
    assert_eq!(r.status(), 401);
}

#[tokio::test]
async fn phrase_recovery_returns_the_wrapped_dek() {
    let c = start().await;
    let a = account(&c).await;
    let r: SessionResp = recover(&c, &client(), &a.phrase).await.json().await.unwrap();
    let dek = unwrap_dek(&a.phrase.kek(), &r.wrapped_dek_phrase.unwrap().0, a.user, WrapSlot::Phrase).unwrap();
    assert_eq!(dek, a.dek);
}

#[tokio::test]
async fn wrong_phrase_gets_nothing_and_looks_the_same() {
    let c = start().await;
    let _a = account(&c).await;
    let r = recover(&c, &client(), &Phrase::generate().unwrap()).await;
    assert_eq!(r.status(), 401);
}

#[tokio::test]
async fn signature_for_one_account_does_not_open_another() {
    let c = start().await;
    let (a, b) = (account(&c).await, account(&c).await);
    let http = client();
    let s: RecoverStartResp = http.post(format!("{}/api/auth/recover/start", c.url))
        .json(&RecoverStartReq { phrase_pub: B64(b.phrase.auth_key().public()) })
        .send().await.unwrap().json().await.unwrap();
    let sig = a.phrase.auth_key().sign(&phrase_challenge("login", s.challenge_id, &s.nonce.0));
    let r = http.post(format!("{}/api/auth/recover/finish", c.url))
        .json(&RecoverFinishReq { challenge_id: s.challenge_id, signature: B64(sig) }).send().await.unwrap();
    assert_eq!(r.status(), 401);
}

#[tokio::test]
async fn recovery_challenge_is_single_use() {
    let c = start().await;
    let a = account(&c).await;
    let http = client();
    let key = a.phrase.auth_key();
    let s: RecoverStartResp = http.post(format!("{}/api/auth/recover/start", c.url))
        .json(&RecoverStartReq { phrase_pub: B64(key.public()) }).send().await.unwrap().json().await.unwrap();
    let req = RecoverFinishReq { challenge_id: s.challenge_id, signature: B64(key.sign(&phrase_challenge("login", s.challenge_id, &s.nonce.0))) };
    assert_eq!(http.post(format!("{}/api/auth/recover/finish", c.url)).json(&req).send().await.unwrap().status(), 200);
    assert_eq!(http.post(format!("{}/api/auth/recover/finish", c.url)).json(&req).send().await.unwrap().status(), 400);
}

#[tokio::test]
async fn add_passkey_needs_phrase_not_just_a_cookie() {
    let c = start().await;
    let a = account(&c).await;
    let http = login(&c, &a).await;
    let s: ChallengeResp = http.post(format!("{}/api/passkeys/add/start", c.url)).json(&json!({})).send().await.unwrap().json().await.unwrap();
    // Signed with a foreign key, as with a stolen cookie and no phrase.
    let bad = Phrase::generate().unwrap().auth_key().sign(&phrase_challenge("add-passkey", s.challenge_id, &s.nonce.unwrap().0));
    let r = http.post(format!("{}/api/passkeys/add/finish", c.url))
        .json(&PasskeyAddFinishReq { challenge_id: s.challenge_id, credential: json!({}), wrapped_dek_prf: None, signature: B64(bad) })
        .send().await.unwrap();
    assert_eq!(r.status(), 401);
}

#[tokio::test]
async fn login_rate_limit_applies() {
    let c = start_with(5, false).await;
    let http = client();
    let mut limited = false;
    for _ in 0..12 {
        let r = http.post(format!("{}/api/auth/login/start", c.url)).json(&json!({})).send().await.unwrap();
        limited |= r.status() == 429;
    }
    assert!(limited);
}

/// Cloudflare keeps the client's own X-Forwarded-For and appends the real address: a new made-up first
/// entry on every request must not buy a new allowance.
#[tokio::test]
async fn behind_a_proxy_the_rate_limit_keys_on_the_address_the_proxy_added() {
    let c = start_with(5, true).await;
    let http = client();
    let mut limited = 0;
    for i in 0..12 {
        let r = http.post(format!("{}/api/auth/login/start", c.url)).json(&json!({}))
            .header("x-forwarded-for", format!("198.51.100.{i}, 203.0.113.7"))
            .send().await.unwrap();
        limited += usize::from(r.status() == 429);
    }
    assert!(limited > 0, "a made-up first entry is ignored");
    let r = http.post(format!("{}/api/auth/login/start", c.url)).json(&json!({}))
        .header("x-forwarded-for", "203.0.113.8").send().await.unwrap();
    assert_eq!(r.status(), 200, "another real address has its own allowance");
    let h = |v: &str| { let mut m = axum::http::HeaderMap::new(); m.insert("x-forwarded-for", v.parse().unwrap()); m };
    assert_eq!(mm_server::last_hop_ip(&h("1.1.1.1, 2.2.2.2")), Some("2.2.2.2".parse().unwrap()));
    assert_eq!(mm_server::last_hop_ip(&h("not-an-ip")), None);
}

#[tokio::test]
async fn registration_is_open_and_the_invite_check_is_gone() {
    let c = start().await;
    let http = client();
    let r = http.post(format!("{}/api/auth/register/start", c.url))
        .json(&RegisterStartReq { label: "Test".into() }).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let start: ChallengeResp = r.json().await.unwrap();
    assert!(start.user_id.is_some(), "a new user id comes with the challenge");
    let gone = http.post(format!("{}/api/auth/invite/check", c.url)).json(&serde_json::json!({ "invite": "x" })).send().await.unwrap();
    assert!(gone.status().is_client_error(), "no invite endpoint: {}", gone.status());
}

#[tokio::test]
async fn logout_kills_the_session() {
    let c = start().await;
    let a = account(&c).await;
    let http = login(&c, &a).await;
    assert_eq!(http.get(format!("{}/api/session", c.url)).send().await.unwrap().status(), 200);
    http.post(format!("{}/api/logout", c.url)).json(&json!({})).send().await.unwrap();
    assert_eq!(http.get(format!("{}/api/session", c.url)).send().await.unwrap().status(), 401);
}

#[tokio::test]
async fn deleting_the_account_removes_every_row() {
    let c = start().await;
    let a = account(&c).await;
    let http = login(&c, &a).await;
    let local = Mem::default();
    save(&local, &Keys { user_id: a.user, dek: &a.dek }, Uuid::new_v4(), &Envelope::new("n", json!({}))).await.unwrap();
    run_sync(&c, &a, &http, &local).await;
    assert_eq!(http.post(format!("{}/api/account/delete", c.url)).json(&json!({})).send().await.unwrap().status(), 200);
    for t in ["users", "records", "sessions", "credentials"] {
        let n: i64 = sqlx::query(&format!("SELECT count(*) FROM {t}")).fetch_one(&c.db).await.unwrap().get(0);
        assert_eq!(n, 0, "{t}");
    }
}

#[tokio::test]
async fn oversized_and_empty_records_are_rejected() {
    let c = start().await;
    let a = account(&c).await;
    let http = login(&c, &a).await;
    for len in [0, (1 << 20) + 65] {
        let req = PushReq { items: vec![PushItem { id: Uuid::new_v4(), expected_revision: 0, blob: B64(vec![0; len]) }] };
        assert_eq!(http.post(format!("{}/api/sync/push", c.url)).json(&req).send().await.unwrap().status(), 400);
    }
}

/// A page on another subdomain of the same site gets the SameSite=Strict cookie too, and a plain form POST
/// needs no preflight: without this guard it could delete the account.
#[tokio::test]
async fn requests_from_other_pages_cannot_change_anything() {
    let c = start().await;
    let a = account(&c).await;
    let http = login(&c, &a).await;
    let del = || http.post(format!("{}/api/account/delete", c.url));
    let form = del().header("content-type", "application/x-www-form-urlencoded").body("x=1").send().await.unwrap();
    assert_eq!(form.status(), 403, "a form post");
    assert_eq!(del().send().await.unwrap().status(), 403, "no body at all");
    let other = del().json(&json!({})).header("origin", "https://evil.example.com").send().await.unwrap();
    assert_eq!(other.status(), 403, "a foreign Origin");
    let site = del().json(&json!({})).header("sec-fetch-site", "same-site").send().await.unwrap();
    assert_eq!(site.status(), 403, "a sibling subdomain");
    let n: i64 = sqlx::query("SELECT count(*) FROM users").fetch_one(&c.db).await.unwrap().get(0);
    assert_eq!(n, 1, "the account is still there");
    let own = del().json(&json!({})).header("sec-fetch-site", "same-origin")
        .header("origin", c.url.replace("127.0.0.1", "localhost")).send().await.unwrap();
    assert_eq!(own.status(), 200, "the app's own request");
    assert_eq!(http.get(format!("{}/api/session", c.url)).send().await.unwrap().status(), 401, "reads stay GET");
}

#[tokio::test]
async fn an_account_cannot_store_more_than_its_quota_but_can_always_shrink() {
    let c = start_cfg(1000, false, 1000, 4 << 20).await;
    let a = account(&c).await;
    let http = login(&c, &a).await;
    let push = |items: Vec<PushItem>| http.post(format!("{}/api/sync/push", c.url)).json(&PushReq { items }).send();
    let big = 1 << 20;
    let ids: Vec<Uuid> = (0..4).map(|_| Uuid::new_v4()).collect();
    for id in &ids[..3] {
        assert_eq!(push(vec![PushItem { id: *id, expected_revision: 0, blob: B64(vec![1; big]) }]).await.unwrap().status(), 200);
    }
    let over = push(vec![PushItem { id: ids[3], expected_revision: 0, blob: B64(vec![1; big + 64]) }]).await.unwrap();
    assert_eq!(over.status(), 413);
    let p: PullResp = http.get(format!("{}/api/sync/pull?since=0", c.url)).send().await.unwrap().json().await.unwrap();
    assert_eq!(p.changes.len(), 3, "the refused push left nothing behind");
    // Replacing a record with a smaller one, as a deletion does, passes.
    let r = push(vec![PushItem { id: ids[0], expected_revision: 1, blob: B64(vec![2; 64]) }]).await.unwrap();
    assert_eq!(r.status(), 200);
    let bytes: i64 = sqlx::query("SELECT bytes FROM users WHERE id = $1").bind(a.user).fetch_one(&c.db).await.unwrap().get(0);
    assert_eq!(bytes, 2 * big as i64 + 64);
    assert_eq!(push(vec![PushItem { id: ids[3], expected_revision: 0, blob: B64(vec![1; big]) }]).await.unwrap().status(), 200);
}

#[tokio::test]
async fn revisions_and_cursors_beyond_bigint_are_refused() {
    let c = start().await;
    let a = account(&c).await;
    let http = login(&c, &a).await;
    let req = PushReq { items: vec![PushItem { id: Uuid::new_v4(), expected_revision: u64::MAX, blob: B64(vec![1; 64]) }] };
    assert_eq!(http.post(format!("{}/api/sync/push", c.url)).json(&req).send().await.unwrap().status(), 400);
    let r = http.get(format!("{}/api/sync/pull?since={}", c.url, u64::MAX)).send().await.unwrap();
    assert_eq!(r.status(), 400);
}

#[tokio::test]
async fn new_accounts_have_their_own_tighter_limit() {
    let c = start_cfg(1000, false, 3, QUOTA).await;
    let http = client();
    let mut codes = vec![];
    for _ in 0..5 {
        let r = http.post(format!("{}/api/auth/register/start", c.url)).json(&RegisterStartReq { label: "x".into() }).send().await.unwrap();
        codes.push(r.status().as_u16());
    }
    assert_eq!(codes, [200, 200, 200, 429, 429]);
    let r = http.post(format!("{}/api/auth/login/start", c.url)).json(&json!({})).send().await.unwrap();
    assert_eq!(r.status(), 200, "signing in is not held back");
}

#[tokio::test]
async fn nbp_rates_come_as_exact_integers_for_a_quarter() {
    let c = start().await;
    let a = account(&c).await;
    let http = login(&c, &a).await;
    let r: RatesResp = http.get(format!("{}/api/rates/eur/2026/4", c.url)).send().await.unwrap().json().await.unwrap();
    assert_eq!(r.currency, "EUR");
    assert_eq!(r.rates, vec![Rate { date: "2026-10-30".into(), rate_e4: 43012 }, Rate { date: "2026-11-02".into(), rate_e4: 42987 }]);
    let empty: RatesResp = http.get(format!("{}/api/rates/xxx/2026/4", c.url)).send().await.unwrap().json().await.unwrap();
    assert!(empty.rates.is_empty(), "no rates is not an error");
    for bad in ["/api/rates/pln/2026/4", "/api/rates/eur/2026/5", "/api/rates/e1/2026/1"] {
        assert_eq!(http.get(format!("{}{bad}", c.url)).send().await.unwrap().status(), 400, "{bad}");
    }
    assert_eq!(client().get(format!("{}/api/rates/eur/2026/4", c.url)).send().await.unwrap().status(), 401, "needs a session");
}

#[tokio::test]
async fn a_full_batch_fits_the_body_limit() {
    let c = start().await;
    let a = account(&c).await;
    let http = login(&c, &a).await;
    // The client can send a batch like this at maximum PUSH_MAX_BYTES.
    let items = (0..8).map(|_| PushItem { id: Uuid::new_v4(), expected_revision: 0, blob: B64(vec![1; 1 << 20]) }).collect();
    let r = http.post(format!("{}/api/sync/push", c.url)).json(&PushReq { items }).send().await.unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn security_headers_are_present() {
    let c = start().await;
    let r = client().get(format!("{}/api/session", c.url)).send().await.unwrap();
    let h = r.headers();
    assert_eq!(h["content-security-policy"], CSP);
    assert_eq!(h["x-content-type-options"], "nosniff");
    assert_eq!(h["cache-control"], "no-store");
    assert!(!CSP.contains("unsafe-inline"));
}

#[tokio::test]
async fn session_cookie_is_http_only_and_strict() {
    let c = start().await;
    let a = account(&c).await;
    let r = recover(&c, &client(), &a.phrase).await;
    let v = r.headers()["set-cookie"].to_str().unwrap();
    assert!(v.contains("HttpOnly") && v.contains("SameSite=Strict"));
}

#[tokio::test]
async fn a_passkey_without_prf_gets_its_copy_once_and_only_for_its_own_user() {
    let c = start().await;
    let (a, b) = (account(&c).await, account(&c).await);
    let cred = vec![7u8; 16];
    // A passkey registered where the authenticator returned no PRF.
    sqlx::query("INSERT INTO credentials (cred_id, user_id, passkey) VALUES ($1,$2,'{}')")
        .bind(&cred).bind(a.user).execute(&c.db).await.unwrap();
    let set = |http: reqwest::Client, wrap: Vec<u8>| {
        let (url, cred) = (c.url.clone(), cred.clone());
        async move {
            let r = http.post(format!("{url}/api/passkeys/prf"))
                .json(&PasskeyPrfReq { cred_id: B64(cred), wrapped_dek_prf: B64(wrap) }).send().await.unwrap();
            (r.status().as_u16(), r.json::<serde_json::Value>().await.ok())
        }
    };
    let stored = || async {
        sqlx::query_scalar::<_, Option<Vec<u8>>>("SELECT wrapped_dek_prf FROM credentials WHERE cred_id = $1")
            .bind(&cred).fetch_one(&c.db).await.unwrap()
    };
    assert_eq!(set(client(), vec![1; 80]).await.0, 401, "needs a session");
    let (_, r) = set(login(&c, &b).await, vec![2; 80]).await;
    assert_eq!(r.unwrap()["saved"], false, "another user's passkey is not touched");
    assert_eq!(stored().await, None);
    let ha = login(&c, &a).await;
    let (_, r) = set(ha.clone(), vec![3; 80]).await;
    assert_eq!(r.unwrap()["saved"], true);
    assert_eq!(stored().await, Some(vec![3; 80]));
    let (_, r) = set(ha.clone(), vec![4; 80]).await;
    assert_eq!(r.unwrap()["saved"], false, "an existing copy is never replaced");
    assert_eq!(stored().await, Some(vec![3; 80]));
    assert_eq!(set(ha, vec![]).await.0, 400, "an empty copy is refused");
}

#[tokio::test]
async fn the_owner_can_count_accounts_and_who_is_online() {
    let c = start().await;
    let (a, b, old) = (account(&c).await, account(&c).await, account(&c).await);
    sqlx::query("UPDATE users SET created_at = now() - interval '40 days', seen_at = now() - interval '20 days' WHERE id = $1")
        .bind(old.user).execute(&c.db).await.unwrap();
    let http = login(&c, &a).await;
    assert_eq!(http.get(format!("{}/api/sync/pull?since=0", c.url)).send().await.unwrap().status(), 200);
    sqlx::query("UPDATE users SET seen_at = now() - interval '2 hours' WHERE id = $1").bind(b.user).execute(&c.db).await.unwrap();
    let s = stats(&c.db).await.unwrap();
    assert_eq!(s, Stats { accounts: 3, new_day: 2, new_week: 2, online: 1, active_day: 2, active_week: 2, active_month: 3, signed_in: 1 });
    // The mark is to the minute, so it says little about exactly when an account was used.
    let whole: bool = sqlx::query("SELECT seen_at = date_trunc('minute', seen_at) FROM users WHERE id = $1").bind(a.user)
        .fetch_one(&c.db).await.unwrap().get(0);
    assert!(whole);
}
