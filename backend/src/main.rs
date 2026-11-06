//! Composition root: config -> database -> market data -> jobs -> HTTP.

use std::net::SocketAddr;

use fintech_backend::config::{Config, LogFormat};
use fintech_backend::market::config::{BuildContext, MarketsConfig};
use fintech_backend::market::hub;
use fintech_backend::{api, app::AppState, db, jobs, net, security};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load .env if present (development convenience; production uses real env vars).
    let _ = dotenvy::dotenv();
    let config = Config::from_env()?;
    let filter = EnvFilter::try_from_env("RUST_LOG").unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn"));
    match config.log_format {
        LogFormat::Text => tracing_subscriber::fmt().with_env_filter(filter).with_target(false).init(),
        LogFormat::Json => tracing_subscriber::fmt().with_env_filter(filter).json().init(),
    }
    net::init_crypto();
    security::password::warm_up();
    tracing::info!(?config, "starting");

    let pool = db::connect(&config).await?;
    if config.run_migrations {
        db::MIGRATOR.run(&pool).await?;
        tracing::info!("database migrations applied");
    }

    let markets = MarketsConfig::load(&config.markets_config)?;
    let state = AppState::new(config, pool);
    let providers = markets.build_all(&BuildContext::default());
    hub::start(providers, state.market.clone(), state.providers.clone());
    jobs::spawn_all(state.clone());

    let addr = state.config.bind_addr;
    let app = api::router(state.clone());
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("listening on http://{addr}");
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    state.db.close().await;
    tracing::info!("shut down cleanly");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = term => {} }
}
