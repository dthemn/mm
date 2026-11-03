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
        .init();
    let opts: PgConnectOptions = var("DATABASE_URL").parse().expect("invalid DATABASE_URL");
    let db = PgPoolOptions::new().max_connections(10)
        .connect_with(opts.disable_statement_logging()).await.expect("cannot reach the database");
    mm_server::migrate(&db).await.expect("migrations failed");

    let args: Vec<String> = env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("invite") {
        let days = args.get(1).and_then(|d| d.parse().ok()).unwrap_or(7);
        println!("{}", mm_server::invite::create(&db, days).await.expect("could not create invite"));
        return;
    }

    let cfg = mm_server::Config {
        rp_id: var("RP_ID"),
        origin: var("ORIGIN"),
        static_dir: env::var("STATIC_DIR").ok().map(Into::into),
        behind_proxy: env::var("BEHIND_PROXY").is_ok_and(|v| v == "1"),
        auth_per_minute: env::var("AUTH_PER_MINUTE").ok().and_then(|v| v.parse().ok()).unwrap_or(30),
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
