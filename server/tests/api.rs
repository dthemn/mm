//! Integration tests on a real PostgreSQL:
//! TEST_DATABASE_URL=postgres://mm:mm-dev@127.0.0.1:55432/mm cargo test -p mm-server
//! WebAuthn is covered separately, in e2e with Chrome's virtual authenticator.

use mm_core::api::*;
use mm_core::crypto::{Dek, Phrase, WrapSlot, phrase_challenge, unwrap_dek, wrap_dek};
use mm_core::record::Envelope;
use mm_core::sync::{Keys, LocalRecord, LocalStore, Remote, Syncer, current, save};
use mm_server::{Config, CSP, invite, migrate, router};
use serde_json::json;
use sqlx::{Connection, Executor, PgConnection, PgPool, Row, postgres::PgPoolOptions};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use uuid::Uuid;

const CANARY: &str = "CANARY-Biedronka-1234,56-Mleko";

fn base_url() -> String {
    std::env::var("TEST_DATABASE_URL").unwrap_or_else(|_| "postgres://mm:mm-dev@127.0.0.1:55432/mm".into())
}

struct Ctx {
    url: String,
    db: PgPool,
}

async fn start_with(limit: u32) -> Ctx {
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
        behind_proxy: false,
        auth_per_minute: limit,
    };
    let app = router(db.clone(), &cfg).into_make_service_with_connect_info::<SocketAddr>();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Ctx { url: format!("http://127.0.0.1:{}", addr.port()), db }
}

async fn start() -> Ctx {
    start_with(1000).await
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
    let s: ChallengeResp = http.post(format!("{}/api/passkeys/add/start", c.url)).send().await.unwrap().json().await.unwrap();
    // Signed with a foreign key, as with a stolen cookie and no phrase.
    let bad = Phrase::generate().unwrap().auth_key().sign(&phrase_challenge("add-passkey", s.challenge_id, &s.nonce.unwrap().0));
    let r = http.post(format!("{}/api/passkeys/add/finish", c.url))
        .json(&PasskeyAddFinishReq { challenge_id: s.challenge_id, credential: json!({}), wrapped_dek_prf: None, signature: B64(bad) })
        .send().await.unwrap();
    assert_eq!(r.status(), 401);
}

#[tokio::test]
async fn login_rate_limit_applies() {
    let c = start_with(5).await;
    let http = client();
    let mut limited = false;
    for _ in 0..12 {
        let r = http.post(format!("{}/api/auth/login/start", c.url)).send().await.unwrap();
        limited |= r.status() == 429;
    }
    assert!(limited);
}

#[tokio::test]
async fn registration_requires_a_valid_invite() {
    let c = start().await;
    let http = client();
    let start = |code: String| {
        let (h, u) = (http.clone(), c.url.clone());
        async move {
            h.post(format!("{u}/api/auth/register/start"))
                .json(&RegisterStartReq { invite: code, label: "Test".into() }).send().await.unwrap().status()
        }
    };
    assert_eq!(start("no such invite".into()).await, 400);
    let good = invite::create(&c.db, 1).await.unwrap();
    assert_eq!(start(good).await, 200);
    let expired = invite::create(&c.db, -1).await.unwrap();
    assert_eq!(start(expired).await, 400);
}

#[tokio::test]
async fn logout_kills_the_session() {
    let c = start().await;
    let a = account(&c).await;
    let http = login(&c, &a).await;
    assert_eq!(http.get(format!("{}/api/session", c.url)).send().await.unwrap().status(), 200);
    http.post(format!("{}/api/logout", c.url)).send().await.unwrap();
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
    assert_eq!(http.post(format!("{}/api/account/delete", c.url)).send().await.unwrap().status(), 200);
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
