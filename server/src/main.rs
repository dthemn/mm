use std::{env, net::SocketAddr};
use sqlx::{ConnectOptions, postgres::{PgConnectOptions, PgPoolOptions}};
use tracing_subscriber::EnvFilter;

fn var(k: &str) -> String {
    env::var(k).unwrap_or_else(|_| panic!("variable {k} is not set"))
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,sqlx=warn".into()))
        .with_writer(std::io::stderr)
        .init();
    let opts: PgConnectOptions = var("DATABASE_URL").parse().expect("invalid DATABASE_URL");
    let db = PgPoolOptions::new().max_connections(10)
        .connect_with(opts.disable_statement_logging()).await.expect("cannot reach the database");
    mm_server::migrate(&db).await.expect("migrations failed");
    // `mm-server stats` prints the account counts and exits.
    if env::args().nth(1).as_deref() == Some("stats") {
        let s = mm_server::stats(&db).await.expect("cannot count accounts");
        println!("accounts       {}", s.accounts);
        println!("new, 24 hours  {}", s.new_day);
        println!("new, 7 days    {}", s.new_week);
        println!("online, 5 min  {}", s.online);
        println!("active, 24 h   {}", s.active_day);
        println!("active, 7 days {}", s.active_week);
        println!("active, 30 d   {}", s.active_month);
        println!("signed in      {}", s.signed_in);
        return;
    }

    let cfg = mm_server::Config {
        rp_id: var("RP_ID"),
        origin: var("ORIGIN"),
        static_dir: env::var("STATIC_DIR").ok().map(Into::into),
        behind_proxy: env::var("BEHIND_PROXY").is_ok_and(|v| v == "1"),
        auth_per_minute: env::var("AUTH_PER_MINUTE").ok().and_then(|v| v.parse().ok()).unwrap_or(30),
        register_per_hour: env::var("REGISTER_PER_HOUR").ok().and_then(|v| v.parse().ok()).unwrap_or(10),
        quota_bytes: env::var("QUOTA_MB").ok().and_then(|v| v.parse::<i64>().ok()).unwrap_or(256) * 1024 * 1024,
        nbp_url: env::var("NBP_URL").unwrap_or_else(|_| "https://api.nbp.pl".into()),
    };
    mm_server::spawn_sweeper(db.clone());
    let addr: SocketAddr = env::var("LISTEN").unwrap_or_else(|_| "127.0.0.1:8080".into()).parse().expect("invalid LISTEN");
    let listener = tokio::net::TcpListener::bind(addr).await.expect("port is in use");
    tracing::info!(%addr, "server started");
    axum::serve(listener, mm_server::router(db, &cfg).into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(async { let _ = tokio::signal::ctrl_c().await; })
        .await
        .expect("server stopped with an error");
}
