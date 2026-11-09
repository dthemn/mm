//! The server stores only ciphertext and minimal metadata.
//! It has no access to keys and cannot read record contents.
#![forbid(unsafe_code)]

pub mod auth;
pub mod error;
pub mod rates;
pub mod sync;

use axum::http::{HeaderName, HeaderValue, header};
use axum::{Router, extract::DefaultBodyLimit, routing::{get, post}};
use sqlx::PgPool;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tower_governor::{GovernorLayer, governor::GovernorConfigBuilder, key_extractor::{PeerIpKeyExtractor, SmartIpKeyExtractor}};
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
}

pub type State = axum::extract::State<Arc<App>>;

pub async fn migrate(db: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("./migrations").run(db).await
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
    });

    let per_ms = (60_000 / cfg.auth_per_minute.max(1)).max(1) as u64;
    let burst = cfg.auth_per_minute.clamp(1, 1000);
    let auth = Router::new()
        .route("/register/start", post(auth::register_start))
        .route("/register/finish", post(auth::register_finish))
        .route("/login/start", post(auth::login_start))
        .route("/login/finish", post(auth::login_finish))
        .route("/recover/start", post(auth::recover_start))
        .route("/recover/finish", post(auth::recover_finish));
    let auth = if cfg.behind_proxy {
        let c = GovernorConfigBuilder::default()
            .per_millisecond(per_ms).burst_size(burst)
            .key_extractor(SmartIpKeyExtractor).finish().expect("rate limit settings");
        auth.layer(GovernorLayer::new(Arc::new(c)))
    } else {
        let c = GovernorConfigBuilder::default()
            .per_millisecond(per_ms).burst_size(burst)
            .key_extractor(PeerIpKeyExtractor).finish().expect("rate limit settings");
        auth.layer(GovernorLayer::new(Arc::new(c)))
    };

    let api = Router::new()
        .nest("/auth", auth)
        .route("/session", get(auth::session))
        .route("/logout", post(auth::logout))
        .route("/passkeys/add/start", post(auth::add_passkey_start))
        .route("/passkeys/add/finish", post(auth::add_passkey_finish))
        .route("/account/delete", post(auth::delete_account))
        .route("/sync/pull", get(sync::pull))
        .route("/sync/push", post(sync::push))
        .route("/rates/{code}/{year}/{quarter}", get(rates::get))
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
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
                tracing::info_span!("http", method = %q.method(), path = %q.uri().path())
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
