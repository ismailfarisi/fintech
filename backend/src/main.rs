use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use fintech_backend::{api, state};
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_target(false).init();

    let port: u16 = std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8080);
    let data_path = PathBuf::from(std::env::var("DATA_FILE").unwrap_or_else(|_| "data/account.json".into()));
    let static_dir = PathBuf::from(std::env::var("STATIC_DIR").unwrap_or_else(|_| "../frontend/dist".into()));
    let tick_ms: u64 = std::env::var("TICK_MS").ok().and_then(|t| t.parse().ok()).unwrap_or(1000);

    let state = state::AppState::new(Some(data_path.clone()));
    tokio::spawn(state::run_market(state.clone(), Duration::from_millis(tick_ms)));

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
        .with_graceful_shutdown(async { let _ = tokio::signal::ctrl_c().await; })
        .await?;
    state.save();
    Ok(())
}
