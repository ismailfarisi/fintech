use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use fintech_backend::market::config::{BuildContext, MarketsConfig};
use fintech_backend::market::hub;
use fintech_backend::{api, net, state};
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.into())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_target(false).init();
    net::init_crypto();

    let port: u16 = env_or("PORT", "8080").parse()?;
    let data_path = PathBuf::from(env_or("DATA_FILE", "data/account.json"));
    let static_dir = PathBuf::from(env_or("STATIC_DIR", "../frontend/dist"));
    let markets_path = PathBuf::from(env_or("MARKETS_CONFIG", "markets.toml"));
    let tick_ms: u64 = env_or("TICK_MS", "1000").parse()?;

    let state = state::AppState::new(Some(data_path.clone()));

    // Market data adapters -> hub -> store.
    let config = MarketsConfig::load(&markets_path)?;
    let ctx = BuildContext { saved_prices: state.saved_prices.clone() };
    let providers = config.build_all(&ctx);
    hub::start(providers, state.market.clone(), state.providers.clone());

    // Order matching + live updates to browsers.
    tokio::spawn(state::run_engine(state.clone(), Duration::from_millis(tick_ms)));

    let mut app = api::router(state.clone()).layer(CorsLayer::permissive());
    if static_dir.join("index.html").exists() {
        // Serve the built frontend, falling back to index.html for client-side routes.
        let spa = ServeDir::new(&static_dir).fallback(ServeFile::new(static_dir.join("index.html")));
        app = app.fallback_service(spa);
        tracing::info!("serving frontend from {}", static_dir.display());
    }

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!("listening on http://{addr} (account file: {})", data_path.display());
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    state.save();
    Ok(())
}
