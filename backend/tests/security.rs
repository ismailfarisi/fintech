//! Authentication, session, CSRF and tenant-isolation tests.

mod common;

use axum::http::{StatusCode, header};
use common::{ORIGIN, TestApp};
use serde_json::json;
use sqlx::PgPool;

#[sqlx::test(migrator = "fintech_backend::db::MIGRATOR")]
async fn session_cookie_is_hardened_and_token_is_not_stored(pool: PgPool) {
    let app = TestApp::new(pool.clone());
    let r = app
        .req(
            "POST",
            "/api/v1/auth/register",
            None,
            Some(json!({
                "email": "alice@example.com", "password": "correct horse battery staple", "display_name": "Alice"
            })),
            &[],
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    let c = r.headers.get(header::SET_COOKIE).unwrap().to_str().unwrap();
    assert!(c.starts_with("__Host-ledger_session="), "{c}");
    for attr in ["HttpOnly", "SameSite=Strict", "Secure", "Path=/"] {
        assert!(c.contains(attr), "missing {attr}: {c}");
    }
    assert!(r.body.get("password_hash").is_none() && r.body["user"].get("password_hash").is_none());
    assert_eq!(r.body["accounts"].as_array().unwrap().len(), 1);

    let token = c.split(';').next().unwrap().split_once('=').unwrap().1;
    let stored: Vec<Vec<u8>> = sqlx::query_scalar("SELECT token_hash FROM sessions").fetch_all(&pool).await.unwrap();
    assert_eq!(stored.len(), 1);
    assert_ne!(stored[0], token.as_bytes(), "raw token must not be stored");

    let hash: String = sqlx::query_scalar("SELECT password_hash FROM users").fetch_one(&pool).await.unwrap();
    assert!(hash.starts_with("$argon2id$"));
}

#[sqlx::test(migrator = "fintech_backend::db::MIGRATOR")]
async fn registration_validation(pool: PgPool) {
    let app = TestApp::new(pool);
    let reg = |email: &str, pw: &str| {
        let body = json!({ "email": email, "password": pw, "display_name": "X" });
        let app = &app;
        async move { app.req("POST", "/api/v1/auth/register", None, Some(body), &[]).await }
    };
    assert_eq!(reg("bob@example.com", "short").await.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(reg("not-an-email", "correct horse battery staple").await.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(reg("bob@example.com", "correct horse battery staple").await.status, StatusCode::CREATED);
    let dup = reg("BOB@example.com", "correct horse battery staple").await;
    assert_eq!(dup.status, StatusCode::CONFLICT, "emails are case-insensitive");
    // Unknown fields are rejected, not ignored.
    let r = app
        .req("POST", "/api/v1/auth/register", None, Some(json!({
            "email": "c@example.com", "password": "correct horse battery staple", "display_name": "X", "is_admin": true
        })), &[])
        .await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[sqlx::test(migrator = "fintech_backend::db::MIGRATOR")]
async fn login_failures_are_generic_and_lock_out(pool: PgPool) {
    let app = TestApp::new(pool);
    app.register("alice@example.com").await;
    let login = |email: &'static str, pw: &'static str| {
        let app = &app;
        async move {
            app.req("POST", "/api/v1/auth/login", None, Some(json!({ "email": email, "password": pw })), &[]).await
        }
    };
    let wrong = login("alice@example.com", "wrong password here").await;
    let unknown = login("nobody@example.com", "wrong password here").await;
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);
    assert_eq!(wrong.body, unknown.body, "no user enumeration");

    let ok = login("Alice@Example.com", "correct horse battery staple").await;
    assert_eq!(ok.status, StatusCode::OK);

    // 5 consecutive failures lock the account (or trip the per-email limiter).
    for _ in 0..5 {
        login("alice@example.com", "wrong password here").await;
    }
    let locked = login("alice@example.com", "correct horse battery staple").await;
    assert_eq!(locked.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(locked.headers.contains_key(header::RETRY_AFTER));
}

#[sqlx::test(migrator = "fintech_backend::db::MIGRATOR")]
async fn logout_and_password_change_revoke_sessions(pool: PgPool) {
    let app = TestApp::new(pool);
    let (c1, _) = app.register("alice@example.com").await;
    let r = app
        .req(
            "POST",
            "/api/v1/auth/login",
            None,
            Some(json!({ "email": "alice@example.com", "password": "correct horse battery staple" })),
            &[],
        )
        .await;
    let c2 = r.headers.get(header::SET_COOKIE).unwrap().to_str().unwrap().split(';').next().unwrap().to_string();
    assert_eq!(app.get("/api/v1/auth/sessions", &c1).await.body.as_array().unwrap().len(), 2);

    // Wrong current password is rejected.
    let r = app
        .post(
            "/api/v1/auth/password",
            &c1,
            json!({ "current_password": "nope nope nope", "new_password": "another long passphrase" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let r = app
        .post(
            "/api/v1/auth/password",
            &c1,
            json!({ "current_password": "correct horse battery staple", "new_password": "another long passphrase" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body["other_sessions_revoked"], 1);
    assert_eq!(app.get("/api/v1/auth/me", &c2).await.status, StatusCode::UNAUTHORIZED, "other device signed out");
    assert_eq!(app.get("/api/v1/auth/me", &c1).await.status, StatusCode::OK);

    let r = app.req("POST", "/api/v1/auth/logout", Some(&c1), None, &[]).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert!(r.headers.get(header::SET_COOKIE).unwrap().to_str().unwrap().contains("Max-Age=0"));
    assert_eq!(app.get("/api/v1/auth/me", &c1).await.status, StatusCode::UNAUTHORIZED, "token is dead after logout");

    let activity: Vec<String> =
        sqlx::query_scalar("SELECT action FROM audit_log ORDER BY id").fetch_all(&app.state.db).await.unwrap();
    for a in ["user.registered", "auth.login", "auth.password_change_failed", "auth.password_changed", "auth.logout"] {
        assert!(activity.iter().any(|x| x == a), "missing audit {a}: {activity:?}");
    }
}

#[sqlx::test(migrator = "fintech_backend::db::MIGRATOR")]
async fn csrf_origin_checks(pool: PgPool) {
    let app = TestApp::new(pool);
    let (c, account) = app.register("alice@example.com").await;
    let uri = format!("/api/v1/accounts/{account}/cash/deposits");
    let body = json!({ "amount": "10.00" });
    // Forged cross-site request.
    let evil = app.req("POST", &uri, Some(&c), Some(body.clone()), &[("origin", "https://evil.example")]).await;
    assert_eq!(evil.status, StatusCode::FORBIDDEN);
    // Cookie but no Origin/Referer at all.
    let mut req = axum::http::Request::builder()
        .method("POST")
        .uri(&uri)
        .header(header::COOKIE, &c)
        .header(header::CONTENT_TYPE, "application/json");
    req = req.header("x-test", "1");
    use tower::ServiceExt;
    let res = app.router.clone().oneshot(req.body(axum::body::Body::from(body.to_string())).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN, "cookie={c}");
    // Same-origin is fine.
    let ok = app.req("POST", &uri, Some(&c), Some(body), &[("origin", ORIGIN)]).await;
    assert_eq!(ok.status, StatusCode::CREATED, "{:?}", ok.body);
}

#[sqlx::test(migrator = "fintech_backend::db::MIGRATOR")]
async fn users_cannot_touch_each_others_accounts(pool: PgPool) {
    let app = TestApp::new(pool);
    let (alice, alice_acct) = app.register("alice@example.com").await;
    let (bob, _) = app.register("bob@example.com").await;
    let r = app
        .order(
            &alice,
            &alice_acct,
            json!({ "symbol": "ACME", "side": "buy", "kind": "limit", "quantity": "1", "limit_price": "50" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    let order_id = r.body["order"]["id"].as_str().unwrap().to_string();

    for path in ["portfolio", "orders", "trades", "cash", "equity", "reconcile"] {
        let r = app.get(&format!("/api/v1/accounts/{alice_acct}/{path}"), &bob).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "bob read alice's {path}");
    }
    let r = app
        .order(&bob, &alice_acct, json!({ "symbol": "ACME", "side": "buy", "kind": "market", "quantity": "1" }))
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r =
        app.post(&format!("/api/v1/accounts/{alice_acct}/cash/withdrawals"), &bob, json!({ "amount": "100.00" })).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = app.req("DELETE", &format!("/api/v1/accounts/{alice_acct}/orders/{order_id}"), Some(&bob), None, &[]).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(app.get("/api/v1/accounts", &bob).await.body.as_array().unwrap().len(), 1);
    // Alice's order is untouched.
    let r = app.get(&format!("/api/v1/accounts/{alice_acct}/orders/{order_id}"), &alice).await;
    assert_eq!(r.body["status"], "open");
}

#[sqlx::test(migrator = "fintech_backend::db::MIGRATOR")]
async fn unauthenticated_requests_and_headers(pool: PgPool) {
    let app = TestApp::new(pool);
    for uri in ["/api/v1/auth/me", "/api/v1/accounts", "/api/v1/market/quotes", "/api/v1/watchlist"] {
        let r = app.req("GET", uri, None, None, &[]).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{uri}");
        assert_eq!(r.body["error"]["code"], "unauthenticated");
    }
    let r = app.req("GET", "/api/v1/auth/me", Some("__Host-ledger_session=garbage"), None, &[]).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);

    let r = app.req("GET", "/api/v1/auth/me", None, None, &[]).await;
    for h in [
        "content-security-policy",
        "x-content-type-options",
        "x-frame-options",
        "strict-transport-security",
        "referrer-policy",
        "x-request-id",
    ] {
        assert!(r.headers.contains_key(h), "missing {h}");
    }
    assert_eq!(r.headers.get("cache-control").unwrap(), "no-store");
    assert_eq!(app.req("GET", "/api/v1/nope", None, None, &[]).await.status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrator = "fintech_backend::db::MIGRATOR")]
async fn oversized_bodies_are_rejected(pool: PgPool) {
    let app = TestApp::new(pool);
    let big = "x".repeat(64 * 1024);
    let r = app.req("POST", "/api/v1/auth/login", None, Some(json!({ "email": big, "password": "x" })), &[]).await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE);
}
