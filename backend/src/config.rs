//! Typed configuration from environment variables, validated at startup.
//! Secrets are wrapped in [`Secret`] so they never appear in logs or `Debug` output.

use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, bail};
use rust_decimal::Decimal;

/// A value that must never be logged.
#[derive(Clone)]
pub struct Secret<T>(T);

impl<T> Secret<T> {
    pub fn new(v: T) -> Self {
        Self(v)
    }
    pub fn expose(&self) -> &T {
        &self.0
    }
}

impl<T> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([redacted])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    Text,
    Json,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: Secret<String>,
    pub db_max_connections: u32,
    pub run_migrations: bool,
    pub bind_addr: SocketAddr,
    /// Origins allowed to make state-changing requests (CSRF defence) and,
    /// when different from the serving origin, CORS requests.
    pub allowed_origins: Vec<String>,
    pub cookie_secure: bool,
    /// Trust `X-Forwarded-For` (only behind a reverse proxy you control).
    pub trust_proxy_headers: bool,
    pub session_idle: Duration,
    pub session_absolute: Duration,
    pub allow_registration: bool,
    pub starting_cash: Decimal,
    pub fee_bps: u32,
    pub static_dir: PathBuf,
    pub markets_config: PathBuf,
    pub log_format: LogFormat,
}

fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

fn parse<T: FromStr>(name: &str, default: T) -> anyhow::Result<T>
where
    T::Err: fmt::Display,
{
    match var(name) {
        None => Ok(default),
        Some(v) => v.parse().map_err(|e| anyhow::anyhow!("{name}: invalid value '{v}': {e}")),
    }
}

fn parse_bool(name: &str, default: bool) -> anyhow::Result<bool> {
    match var(name).as_deref().map(str::to_ascii_lowercase).as_deref() {
        None => Ok(default),
        Some("1" | "true" | "yes" | "on") => Ok(true),
        Some("0" | "false" | "no" | "off") => Ok(false),
        Some(v) => bail!("{name}: expected true/false, got '{v}'"),
    }
}

/// Normalise an origin: scheme://host[:port], no trailing slash or path.
pub fn normalize_origin(s: &str) -> anyhow::Result<String> {
    let s = s.trim().trim_end_matches('/');
    let (scheme, rest) = s.split_once("://").with_context(|| format!("origin '{s}' needs a scheme"))?;
    let scheme = scheme.to_ascii_lowercase();
    if !matches!(scheme.as_str(), "http" | "https") || rest.is_empty() || rest.contains('/') {
        bail!("origin '{s}' must look like https://example.com[:port]");
    }
    Ok(format!("{scheme}://{}", rest.to_ascii_lowercase()))
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let database_url = var("DATABASE_URL").context("DATABASE_URL is required (see .env.example)")?;
        let bind_addr: SocketAddr = parse("BIND_ADDR", "127.0.0.1:8080".parse().unwrap())?;

        let public_origin = var("PUBLIC_ORIGIN").unwrap_or_else(|| format!("http://localhost:{}", bind_addr.port()));
        let mut allowed_origins = vec![normalize_origin(&public_origin)?];
        for o in var("ALLOWED_ORIGINS").unwrap_or_default().split(',').filter(|o| !o.trim().is_empty()) {
            allowed_origins.push(normalize_origin(o)?);
        }
        allowed_origins.dedup();

        let cookie_secure = parse_bool("COOKIE_SECURE", true)?;
        let session_idle = Duration::from_secs(60 * parse::<u64>("SESSION_IDLE_MINUTES", 30)?);
        let session_absolute = Duration::from_secs(3600 * parse::<u64>("SESSION_ABSOLUTE_HOURS", 12)?);
        if session_idle.is_zero() || session_absolute < session_idle {
            bail!("SESSION_IDLE_MINUTES must be > 0 and no longer than SESSION_ABSOLUTE_HOURS");
        }
        let starting_cash: Decimal = parse("STARTING_CASH", Decimal::from(100_000))?;
        if starting_cash < Decimal::ZERO || starting_cash.scale() > 2 {
            bail!("STARTING_CASH must be a non-negative amount in dollars and cents");
        }
        let fee_bps: u32 = parse("TRADING_FEE_BPS", 0)?;
        if fee_bps > 1000 {
            bail!("TRADING_FEE_BPS must be at most 1000 (10%)");
        }
        let log_format = match var("LOG_FORMAT").as_deref() {
            None | Some("text") => LogFormat::Text,
            Some("json") => LogFormat::Json,
            Some(v) => bail!("LOG_FORMAT: expected text or json, got '{v}'"),
        };

        Ok(Config {
            database_url: Secret::new(database_url),
            db_max_connections: parse("DB_MAX_CONNECTIONS", 10)?,
            run_migrations: parse_bool("RUN_MIGRATIONS", true)?,
            bind_addr,
            allowed_origins,
            cookie_secure,
            trust_proxy_headers: parse_bool("TRUST_PROXY_HEADERS", false)?,
            session_idle,
            session_absolute,
            allow_registration: parse_bool("ALLOW_REGISTRATION", true)?,
            starting_cash,
            fee_bps,
            static_dir: PathBuf::from(var("STATIC_DIR").unwrap_or_else(|| "../frontend/dist".into())),
            markets_config: PathBuf::from(var("MARKETS_CONFIG").unwrap_or_else(|| "markets.toml".into())),
            log_format,
        })
    }

    /// Settings for tests.
    pub fn for_tests(database_url: &str) -> Self {
        Config {
            database_url: Secret::new(database_url.into()),
            db_max_connections: 5,
            run_migrations: false,
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            allowed_origins: vec!["http://localhost:8080".into()],
            cookie_secure: true,
            trust_proxy_headers: false,
            session_idle: Duration::from_secs(1800),
            session_absolute: Duration::from_secs(12 * 3600),
            allow_registration: true,
            starting_cash: Decimal::from(100_000),
            fee_bps: 0,
            static_dir: PathBuf::from("/nonexistent"),
            markets_config: PathBuf::from("/nonexistent"),
            log_format: LogFormat::Text,
        }
    }

    /// `__Host-` cookies are bound to this exact origin and require `Secure`.
    pub fn session_cookie_name(&self) -> &'static str {
        if self.cookie_secure { "__Host-ledger_session" } else { "ledger_session" }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_redacted() {
        let s = Secret::new("postgres://user:hunter2@db/x".to_string());
        assert_eq!(format!("{s:?}"), "Secret([redacted])");
        assert!(!format!("{:?}", Config::for_tests("postgres://u:hunter2@h/db")).contains("hunter2"));
    }

    #[test]
    fn origins_are_normalized_and_validated() {
        assert_eq!(normalize_origin("HTTPS://App.Example.com/").unwrap(), "https://app.example.com");
        assert_eq!(normalize_origin("http://localhost:5173").unwrap(), "http://localhost:5173");
        assert!(normalize_origin("example.com").is_err());
        assert!(normalize_origin("https://example.com/path").is_err());
        assert!(normalize_origin("javascript://x").is_err());
    }
}
