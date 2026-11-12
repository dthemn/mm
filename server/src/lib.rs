//! The server stores only ciphertext and minimal metadata.
//! It has no access to keys and cannot read record contents.
#![forbid(unsafe_code)]

pub mod auth;
pub mod error;
pub mod rates;
pub mod sync;

use axum::http::{HeaderName, HeaderValue, Method, header};
use axum::{Router, extract::DefaultBodyLimit, middleware::{self, Next}, response::{IntoResponse, Response}, routing::{get, post}};
use sqlx::{PgPool, Row};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tower_governor::{GovernorError, GovernorLayer, governor::GovernorConfigBuilder, key_extractor::{KeyExtractor, PeerIpKeyExtractor}};
use tower_http::{limit::RequestBodyLimitLayer, services::{ServeDir, ServeFile}, set_header::SetResponseHeaderLayer, trace::TraceLayer};
use url::Url;
use webauthn_rs::{Webauthn, WebauthnBuilder};

pub const CSP: &str = "default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; \
    img-src 'self' data:; connect-src 'self'; manifest-src 'self'; worker-src 'self'; font-src 'self'; \
    base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

/// The client splits pushes into batches of up to 8 MiB, about 11 MiB in base64.
const BODY_LIMIT: usize = 16 * 1024 * 1024;

#[derive(Clone)]
pub struct Config {
    pub rp_id: String,
    pub origin: String,
    pub static_dir: Option<PathBuf>,
    /// Behind the reverse proxy the client address comes from Caddy's headers.
    pub behind_proxy: bool,
    /// Login requests per minute from one address.
    pub auth_per_minute: u32,
    /// New accounts per hour from one address. Registration is open, so this bounds how fast one address can
    /// create accounts, each with its own quota.
    pub register_per_hour: u32,
    /// Ciphertext bytes one account may store.
    pub quota_bytes: i64,
    /// Base URL of the NBP API, replaceable in tests.
    pub nbp_url: String,
}

pub struct App {
    pub db: PgPool,
    pub webauthn: Webauthn,
    pub secure_cookie: bool,
    pub http: reqwest::Client,
    pub nbp_url: String,
    pub rates: rates::Cache,
    pub quota_bytes: i64,
    /// The app's own origin: requests that change state must come from it.
    pub origin: String,
}

pub type State = axum::extract::State<Arc<App>>;

/// The client address for the rate limit behind a proxy: the last `X-Forwarded-For` entry, the one the
/// nearest proxy added. Earlier entries come from the client itself: Cloudflare keeps whatever the client
/// sent and appends the real address, so reading the first entry let anyone pick a fresh key per request.
/// Without the header, the peer address. The app must be reachable only through the proxy.
#[derive(Clone, Copy, Debug)]
pub struct LastHopIp;

pub fn last_hop_ip(headers: &axum::http::HeaderMap) -> Option<std::net::IpAddr> {
    let v = headers.get_all("x-forwarded-for").iter().next_back()?.to_str().ok()?;
    v.rsplit(',').next()?.trim().parse().ok()
}

impl KeyExtractor for LastHopIp {
    type Key = std::net::IpAddr;
    fn extract<T>(&self, req: &axum::http::Request<T>) -> Result<Self::Key, GovernorError> {
        last_hop_ip(req.headers()).or_else(|| PeerIpKeyExtractor.extract(req).ok()).ok_or(GovernorError::UnableToExtractKey)
    }
}

/// Cross-site request forgery guard for every request that changes state. SameSite=Strict keeps the cookie
/// away from other sites, but not from other subdomains of the same site, and a plain form POST needs no
/// preflight. The app's own requests are always same-origin fetches with a JSON body, so anything else is refused:
/// a JSON content type cannot be sent cross-origin without a CORS preflight, which this server never allows.
pub fn same_origin(origin: &str, headers: &axum::http::HeaderMap) -> bool {
    let h = |n: &str| headers.get(n).and_then(|v| v.to_str().ok());
    if h("sec-fetch-site").is_some_and(|v| v != "same-origin") {
        return false;
    }
    if h("origin").is_some_and(|v| v != origin) {
        return false;
    }
    h("content-type").is_some_and(|v| v.split(';').next().is_some_and(|t| t.trim().eq_ignore_ascii_case("application/json")))
}

async fn csrf(axum::extract::State(app): State, req: axum::extract::Request, next: Next) -> Response {
    if matches!(*req.method(), Method::GET | Method::HEAD) || same_origin(&app.origin, req.headers()) {
        next.run(req).await
    } else {
        error::Error::Forbidden.into_response()
    }
}

/// A per-address rate limit: `per_ms` milliseconds to regain one request, up to `burst` at once.
fn limit<K>(r: Router<Arc<App>>, per_ms: u64, burst: u32, key: K) -> Router<Arc<App>>
where
    K: KeyExtractor + Send + Sync + 'static,
    K::Key: Send + Sync + 'static,
{
    let c = GovernorConfigBuilder::default()
        .per_millisecond(per_ms).burst_size(burst)
        .key_extractor(key).finish().expect("rate limit settings");
    r.layer(GovernorLayer::new(Arc::new(c)))
}

fn limit_by(r: Router<Arc<App>>, per_ms: u64, burst: u32, behind_proxy: bool) -> Router<Arc<App>> {
    if behind_proxy { limit(r, per_ms, burst, LastHopIp) } else { limit(r, per_ms, burst, PeerIpKeyExtractor) }
}

/// The path as logged: rates requests would otherwise record which currencies an account uses.
fn log_path(path: &str) -> &str {
    if path.starts_with("/api/rates/") { "/api/rates" } else { path }
}

pub async fn migrate(db: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("./migrations").run(db).await
}

/// Counts of accounts for the owner, from `mm-server stats`. Online means a request in the last 5 minutes:
/// an open, unlocked app syncs every 30 seconds.
#[derive(Debug, PartialEq, Eq)]
pub struct Stats {
    pub accounts: i64,
    pub new_day: i64,
    pub new_week: i64,
    pub online: i64,
    pub active_day: i64,
    pub active_week: i64,
    pub active_month: i64,
    pub signed_in: i64,
}

pub async fn stats(db: &PgPool) -> Result<Stats, sqlx::Error> {
    let r = sqlx::query(
        "SELECT count(*),
           count(*) FILTER (WHERE created_at > now() - interval '1 day'),
           count(*) FILTER (WHERE created_at > now() - interval '7 days'),
           count(*) FILTER (WHERE seen_at > now() - interval '5 minutes'),
           count(*) FILTER (WHERE seen_at > now() - interval '1 day'),
           count(*) FILTER (WHERE seen_at > now() - interval '7 days'),
           count(*) FILTER (WHERE seen_at > now() - interval '30 days'),
           (SELECT count(DISTINCT user_id) FROM sessions WHERE expires_at > now())
         FROM users").fetch_one(db).await?;
    Ok(Stats {
        accounts: r.get(0), new_day: r.get(1), new_week: r.get(2), online: r.get(3),
        active_day: r.get(4), active_week: r.get(5), active_month: r.get(6), signed_in: r.get(7),
    })
}

/// Expired challenges and sessions only add metadata.
pub async fn sweep(db: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM challenges WHERE expires_at < now()").execute(db).await?;
    sqlx::query("DELETE FROM sessions WHERE expires_at < now()").execute(db).await?;
    Ok(())
}

pub fn router(db: PgPool, cfg: &Config) -> Router {
    let origin = Url::parse(&cfg.origin).expect("ORIGIN must be a URL");
    let webauthn = WebauthnBuilder::new(&cfg.rp_id, &origin)
        .expect("invalid RP_ID or ORIGIN")
        .rp_name("mm")
        .build()
        .expect("could not create WebAuthn");
    let secure = origin.scheme() == "https";
    let app = Arc::new(App {
        db,
        webauthn,
        secure_cookie: secure,
        http: reqwest::Client::builder().user_agent("mm").build().expect("HTTP client"),
        nbp_url: cfg.nbp_url.trim_end_matches('/').to_string(),
        rates: rates::Cache::default(),
        quota_bytes: cfg.quota_bytes,
        origin: origin.origin().ascii_serialization(),
    });

    let per_ms = (60_000 / cfg.auth_per_minute.max(1)).max(1) as u64;
    let burst = cfg.auth_per_minute.clamp(1, 1000);
    let register = Router::new().route("/register/start", post(auth::register_start));
    let per_hour = cfg.register_per_hour.max(1);
    let register = limit_by(register, (3_600_000 / per_hour).max(1) as u64, per_hour.clamp(1, 1000), cfg.behind_proxy);
    let auth = Router::new()
        .merge(register)
        .route("/register/finish", post(auth::register_finish))
        .route("/login/start", post(auth::login_start))
        .route("/login/finish", post(auth::login_finish))
        .route("/recover/start", post(auth::recover_start))
        .route("/recover/finish", post(auth::recover_finish));
    let auth = limit_by(auth, per_ms, burst, cfg.behind_proxy);

    let api = Router::new()
        .nest("/auth", auth)
        .route("/session", get(auth::session))
        .route("/logout", post(auth::logout))
        .route("/passkeys/add/start", post(auth::add_passkey_start))
        .route("/passkeys/add/finish", post(auth::add_passkey_finish))
        .route("/passkeys/prf", post(auth::set_passkey_prf))
        .route("/account/delete", post(auth::delete_account))
        .route("/sync/pull", get(sync::pull))
        .route("/sync/push", post(sync::push))
        .route("/rates/{code}/{year}/{quarter}", get(rates::get))
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
        .layer(middleware::from_fn_with_state(app.clone(), csrf))
        .with_state(app);

    let mut r = Router::new().nest("/api", api);
    if let Some(dir) = &cfg.static_dir {
        let index = dir.join("index.html");
        // File names never change between releases, and index.html pins each file by its hash.
        // A cache in between (Cloudflare, a proxy) must ask again every time, or a new index.html
        // would meet an old script, fail the integrity check and leave a blank page.
        let files = ServeDir::new(dir).fallback(ServeFile::new(index));
        r = r.fallback_service(tower_http::set_header::SetResponseHeader::if_not_present(
            files, header::CACHE_CONTROL, HeaderValue::from_static("no-cache")));
    }
    let r = r
        .layer(DefaultBodyLimit::disable())
        .layer(RequestBodyLimitLayer::new(BODY_LIMIT))
        .layer(TraceLayer::new_for_http()
            // Method, path and status only. Bodies and headers never reach the logs.
            .make_span_with(|q: &axum::http::Request<_>| {
                tracing::info_span!("http", method = %q.method(), path = %log_path(q.uri().path()))
            }));
    let mut r = headers(r);
    if secure {
        r = r.layer(SetResponseHeaderLayer::overriding(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=63072000; includeSubDomains"),
        ));
    }
    r
}

fn headers(r: Router) -> Router {
    let set = |n: HeaderName, v: &'static str| SetResponseHeaderLayer::overriding(n, HeaderValue::from_static(v));
    r.layer(set(header::CONTENT_SECURITY_POLICY, CSP))
        .layer(set(header::X_CONTENT_TYPE_OPTIONS, "nosniff"))
        .layer(set(header::REFERRER_POLICY, "no-referrer"))
        .layer(set(HeaderName::from_static("cross-origin-opener-policy"), "same-origin"))
        .layer(set(HeaderName::from_static("cross-origin-embedder-policy"), "require-corp"))
        .layer(set(HeaderName::from_static("permissions-policy"), "camera=(self), microphone=(), geolocation=()"))
}

pub fn spawn_sweeper(db: PgPool) {
    tokio::spawn(async move {
        let mut t = tokio::time::interval(Duration::from_secs(600));
        loop {
            t.tick().await;
            if let Err(e) = sweep(&db).await {
                tracing::warn!(error = %e, "cleanup failed");
            }
        }
    });
}
