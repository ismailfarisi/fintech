//! Test harness: the real router + a real (per-test) PostgreSQL database.

#![allow(dead_code)]

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use chrono::Utc;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

use fintech_backend::app::{AppState, SharedState};
use fintech_backend::config::Config;
use fintech_backend::market::{AssetClass, Instrument, MarketEvent, PriceUpdate};

pub const ORIGIN: &str = "http://localhost:8080";

pub struct TestApp {
    pub state: SharedState,
    pub router: Router,
}

pub struct Res {
    pub status: StatusCode,
    pub headers: axum::http::HeaderMap,
    pub body: Value,
}

impl TestApp {
    pub fn new(pool: PgPool) -> Self {
        let state = AppState::new(Config::for_tests("postgres://unused"), pool);
        {
            let mut m = state.market.write().unwrap();
            for (sym, step, class) in [("BTC-USD", 0.00000001, AssetClass::Crypto), ("ACME", 1.0, AssetClass::Equity)] {
                m.register(
                    Instrument {
                        symbol: sym.into(),
                        name: sym.into(),
                        asset_class: class,
                        source: "test".into(),
                        reference_label: "Open".into(),
                        qty_step: step,
                    },
                    60,
                );
            }
        }
        let app = TestApp { router: fintech_backend::api::router(state.clone()), state };
        app.set_price("BTC-USD", 75_000.0);
        app.set_price("ACME", 100.0);
        app
    }

    pub fn set_price(&self, symbol: &str, price: f64) {
        self.state.market.write().unwrap().apply(MarketEvent::Tick(PriceUpdate::new(symbol, price, Utc::now())));
    }

    pub async fn req(
        &self,
        method: &str,
        uri: &str,
        cookie: Option<&str>,
        body: Option<Value>,
        extra: &[(&str, &str)],
    ) -> Res {
        let mut b = Request::builder().method(method).uri(uri);
        if method != "GET" && !extra.iter().any(|(k, _)| k.eq_ignore_ascii_case("origin")) {
            b = b.header(header::ORIGIN, ORIGIN);
        }
        if let Some(c) = cookie {
            b = b.header(header::COOKIE, c);
        }
        for (k, v) in extra {
            b = b.header(*k, *v);
        }
        let body = match body {
            Some(v) => {
                b = b.header(header::CONTENT_TYPE, "application/json");
                Body::from(v.to_string())
            }
            None => Body::empty(),
        };
        let res = self.router.clone().oneshot(b.body(body).unwrap()).await.unwrap();
        let status = res.status();
        let headers = res.headers().clone();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into()))
        };
        Res { status, headers, body }
    }

    pub async fn get(&self, uri: &str, cookie: &str) -> Res {
        self.req("GET", uri, Some(cookie), None, &[]).await
    }

    pub async fn post(&self, uri: &str, cookie: &str, body: Value) -> Res {
        self.req("POST", uri, Some(cookie), Some(body), &[]).await
    }

    /// Register a user; returns (cookie header value, account id).
    pub async fn register(&self, email: &str) -> (String, String) {
        let r = self
            .req(
                "POST",
                "/api/v1/auth/register",
                None,
                Some(json!({
                    "email": email, "password": "correct horse battery staple", "display_name": "Test"
                })),
                &[],
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        let set_cookie = r.headers.get(header::SET_COOKIE).unwrap().to_str().unwrap();
        let cookie = set_cookie.split(';').next().unwrap().to_string();
        let account = r.body["accounts"][0]["id"].as_str().unwrap().to_string();
        (cookie, account)
    }

    pub async fn order(&self, cookie: &str, account: &str, body: Value) -> Res {
        self.post(&format!("/api/v1/accounts/{account}/orders"), cookie, body).await
    }

    pub async fn portfolio(&self, cookie: &str, account: &str) -> Value {
        let r = self.get(&format!("/api/v1/accounts/{account}/portfolio"), cookie).await;
        assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
        r.body
    }

    pub async fn assert_reconciled(&self, cookie: &str, account: &str) {
        let r = self.get(&format!("/api/v1/accounts/{account}/reconcile"), cookie).await;
        assert_eq!(r.body["consistent"], true, "ledger/holdings mismatch: {:?}", r.body);
    }
}

pub fn dec(v: &Value) -> rust_decimal::Decimal {
    v.as_str().unwrap_or_else(|| panic!("expected decimal string, got {v}")).parse().unwrap()
}
