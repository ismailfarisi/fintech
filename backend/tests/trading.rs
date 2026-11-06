//! Money correctness: orders, reservations, matching, idempotency, concurrency,
//! and database-level ledger guarantees.

mod common;

use axum::http::StatusCode;
use common::{TestApp, dec};
use rust_decimal_macros::dec;
use serde_json::json;
use sqlx::PgPool;

#[sqlx::test(migrator = "fintech_backend::db::MIGRATOR")]
async fn market_orders_move_cash_and_positions_exactly(pool: PgPool) {
    let app = TestApp::new(pool);
    let (c, a) = app.register("alice@example.com").await;
    let p = app.portfolio(&c, &a).await;
    assert_eq!(dec(&p["equity"]), dec!(100000));
    assert_eq!(dec(&p["net_contributions"]), dec!(100000));

    let r =
        app.order(&c, &a, json!({ "symbol": "btc-usd", "side": "buy", "kind": "market", "quantity": "0.0125" })).await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    assert_eq!(r.body["order"]["status"], "filled");
    assert_eq!(dec(&r.body["trade"]["gross"]), dec!(937.50));
    assert!(r.body["trade"]["gross"].is_string(), "money is serialized as strings");

    app.set_price("BTC-USD", 80_000.0);
    let p = app.portfolio(&c, &a).await;
    assert_eq!(dec(&p["cash"]), dec!(99062.50));
    assert_eq!(dec(&p["positions"][0]["quantity"]), dec!(0.0125));
    assert_eq!(dec(&p["positions"][0]["market_value"]), dec!(1000));
    assert_eq!(dec(&p["unrealized_pnl"]), dec!(62.50));

    let r =
        app.order(&c, &a, json!({ "symbol": "BTC-USD", "side": "sell", "kind": "market", "quantity": "0.0125" })).await;
    assert_eq!(dec(&r.body["trade"]["realized_pnl"]), dec!(62.50));
    let p = app.portfolio(&c, &a).await;
    assert_eq!(dec(&p["cash"]), dec!(100062.50));
    assert_eq!(p["positions"].as_array().unwrap().len(), 0);
    assert_eq!(dec(&p["realized_pnl"]), dec!(62.50));
    app.assert_reconciled(&c, &a).await;
}

#[sqlx::test(migrator = "fintech_backend::db::MIGRATOR")]
async fn order_validation_errors(pool: PgPool) {
    let app = TestApp::new(pool);
    let (c, a) = app.register("alice@example.com").await;
    let cases = [
        (json!({ "symbol": "ACME", "side": "buy", "kind": "market", "quantity": "1.5" }), "invalid_quantity"),
        (json!({ "symbol": "ACME", "side": "buy", "kind": "market", "quantity": "-1" }), "invalid_quantity"),
        (json!({ "symbol": "ACME", "side": "buy", "kind": "market", "quantity": "2000" }), "insufficient_funds"),
        (json!({ "symbol": "ACME", "side": "sell", "kind": "market", "quantity": "1" }), "insufficient_quantity"),
        (json!({ "symbol": "ACME", "side": "buy", "kind": "limit", "quantity": "1" }), "invalid_price"),
        (json!({ "symbol": "NOPE", "side": "buy", "kind": "market", "quantity": "1" }), "unknown_symbol"),
    ];
    for (body, code) in cases {
        let r = app.order(&c, &a, body.clone()).await;
        assert_eq!(r.body["error"]["code"], code, "{body} -> {:?}", r.body);
    }
    // Floats are not accepted for money: amounts must be strings.
    let r = app.order(&c, &a, json!({ "symbol": "ACME", "side": "buy", "kind": "market", "quantity": 1.0 })).await;
    assert!(r.status.is_client_error(), "{:?}", r.body);
    let orders = app.get(&format!("/api/v1/accounts/{a}/orders"), &c).await;
    assert_eq!(orders.body.as_array().unwrap().len(), 0, "rejected orders are not recorded");
}

#[sqlx::test(migrator = "fintech_backend::db::MIGRATOR")]
async fn limit_orders_reserve_and_match(pool: PgPool) {
    let app = TestApp::new(pool);
    let (c, a) = app.register("alice@example.com").await;
    // Rest a buy for 100 ACME @ 90: reserves $9,000.
    let r = app
        .order(
            &c,
            &a,
            json!({ "symbol": "ACME", "side": "buy", "kind": "limit", "quantity": "100", "limit_price": "90" }),
        )
        .await;
    assert_eq!(r.body["order"]["status"], "open");
    let p = app.portfolio(&c, &a).await;
    assert_eq!(dec(&p["buying_power"]), dec!(91000));
    // Reserved cash can't be withdrawn or spent twice.
    let r = app.post(&format!("/api/v1/accounts/{a}/cash/withdrawals"), &c, json!({ "amount": "95000.00" })).await;
    assert_eq!(r.body["error"]["code"], "insufficient_funds");

    // Price above limit: no fill.
    fintech_backend::jobs::match_orders(&app.state).await.unwrap();
    assert_eq!(
        app.get(&format!("/api/v1/accounts/{a}/orders?status=open"), &c).await.body.as_array().unwrap().len(),
        1
    );
    // Price drops through the limit: fills at the better market price.
    app.set_price("ACME", 89.5);
    assert_eq!(fintech_backend::jobs::match_orders(&app.state).await.unwrap(), 1);
    assert_eq!(fintech_backend::jobs::match_orders(&app.state).await.unwrap(), 0, "never fills twice");
    let p = app.portfolio(&c, &a).await;
    assert_eq!(dec(&p["cash"]), dec!(91050));
    assert_eq!(dec(&p["buying_power"]), dec!(91050), "reservation released");
    assert_eq!(dec(&p["positions"][0]["avg_cost"]), dec!(89.5));

    // Resting sell reserves shares; cancel releases them.
    let r = app
        .order(
            &c,
            &a,
            json!({ "symbol": "ACME", "side": "sell", "kind": "limit", "quantity": "60", "limit_price": "120" }),
        )
        .await;
    let sell_id = r.body["order"]["id"].as_str().unwrap().to_string();
    let r = app.order(&c, &a, json!({ "symbol": "ACME", "side": "sell", "kind": "market", "quantity": "50" })).await;
    assert_eq!(r.body["error"]["code"], "insufficient_quantity", "60 of 100 are reserved");
    let r = app.req("DELETE", &format!("/api/v1/accounts/{a}/orders/{sell_id}"), Some(&c), None, &[]).await;
    assert_eq!(r.body["status"], "cancelled");
    let r = app.req("DELETE", &format!("/api/v1/accounts/{a}/orders/{sell_id}"), Some(&c), None, &[]).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    let r = app.order(&c, &a, json!({ "symbol": "ACME", "side": "sell", "kind": "market", "quantity": "50" })).await;
    assert_eq!(r.status, StatusCode::CREATED);
    app.assert_reconciled(&c, &a).await;
}

#[sqlx::test(migrator = "fintech_backend::db::MIGRATOR")]
async fn idempotency_keys_prevent_duplicate_orders(pool: PgPool) {
    let app = TestApp::new(pool);
    let (c, a) = app.register("alice@example.com").await;
    let uri = format!("/api/v1/accounts/{a}/orders");
    let body = json!({ "symbol": "ACME", "side": "buy", "kind": "market", "quantity": "1" });
    let key = [("idempotency-key", "order-abc-12345")];

    let first = app.req("POST", &uri, Some(&c), Some(body.clone()), &key).await;
    let again = app.req("POST", &uri, Some(&c), Some(body.clone()), &key).await;
    assert_eq!(first.status, StatusCode::CREATED);
    assert_eq!(again.status, StatusCode::CREATED);
    assert_eq!(first.body["order"]["id"], again.body["order"]["id"]);
    assert_eq!(again.headers.get("idempotent-replayed").unwrap(), "true");

    let different = app
        .req(
            "POST",
            &uri,
            Some(&c),
            Some(json!({ "symbol": "ACME", "side": "buy", "kind": "market", "quantity": "2" })),
            &key,
        )
        .await;
    assert_eq!(different.body["error"]["code"], "idempotency_key_reused");

    // Concurrent duplicates: exactly one executes.
    let k2 = [("idempotency-key", "order-concurrent-1")];
    let (r1, r2, r3) = tokio::join!(
        app.req("POST", &uri, Some(&c), Some(body.clone()), &k2),
        app.req("POST", &uri, Some(&c), Some(body.clone()), &k2),
        app.req("POST", &uri, Some(&c), Some(body.clone()), &k2),
    );
    assert_eq!(r1.body["order"]["id"], r2.body["order"]["id"]);
    assert_eq!(r2.body["order"]["id"], r3.body["order"]["id"]);

    let orders = app.get(&uri, &c).await;
    assert_eq!(orders.body.as_array().unwrap().len(), 2, "one per key");
    let bad = app.req("POST", &uri, Some(&c), Some(body), &[("idempotency-key", "x")]).await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrator = "fintech_backend::db::MIGRATOR")]
async fn concurrent_orders_cannot_overdraw(pool: PgPool) {
    let app = TestApp::new(pool);
    let (c, a) = app.register("alice@example.com").await;
    // 30 concurrent buys of $5,000 each against $100,000: at most 20 can succeed.
    let body = json!({ "symbol": "ACME", "side": "buy", "kind": "market", "quantity": "50" });
    let futs = (0..30).map(|_| app.order(&c, &a, body.clone()));
    let results = futures_util::future::join_all(futs).await;
    let ok = results.iter().filter(|r| r.status == StatusCode::CREATED).count();
    assert_eq!(ok, 20, "{:?}", results.iter().map(|r| r.status).collect::<Vec<_>>());
    let p = app.portfolio(&c, &a).await;
    assert_eq!(dec(&p["cash"]), dec!(0));
    assert_eq!(dec(&p["positions"][0]["quantity"]), dec!(1000));
    app.assert_reconciled(&c, &a).await;
}

#[sqlx::test(migrator = "fintech_backend::db::MIGRATOR")]
async fn ledger_is_balanced_and_immutable(pool: PgPool) {
    let app = TestApp::new(pool.clone());
    let (c, a) = app.register("alice@example.com").await;
    app.order(&c, &a, json!({ "symbol": "BTC-USD", "side": "buy", "kind": "market", "quantity": "0.5" })).await;
    app.post(&format!("/api/v1/accounts/{a}/cash/deposits"), &c, json!({ "amount": "250.25" })).await;

    let unbalanced: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM (SELECT entry_id, asset FROM postings GROUP BY entry_id, asset HAVING sum(amount) <> 0) x",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(unbalanced, 0);
    for sql in [
        "UPDATE trades SET price = 1",
        "DELETE FROM postings",
        "UPDATE ledger_entries SET description = 'x'",
        "DELETE FROM audit_log",
        "TRUNCATE trades",
    ] {
        let err = sqlx::query(sql).execute(&pool).await;
        assert!(err.is_err(), "{sql} should be rejected");
    }
    // Overdraft is impossible even with direct SQL.
    let err = sqlx::query("UPDATE holdings SET quantity = -1 WHERE asset = 'USD'").execute(&pool).await;
    assert!(err.is_err());
    app.assert_reconciled(&c, &a).await;
}

#[sqlx::test(migrator = "fintech_backend::db::MIGRATOR")]
async fn reset_closes_account_and_opens_a_new_one(pool: PgPool) {
    let app = TestApp::new(pool);
    let (c, a) = app.register("alice@example.com").await;
    app.order(
        &c,
        &a,
        json!({ "symbol": "ACME", "side": "buy", "kind": "limit", "quantity": "10", "limit_price": "50" }),
    )
    .await;
    let r = app.post(&format!("/api/v1/accounts/{a}/reset"), &c, json!({})).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let new = r.body["account_id"].as_str().unwrap().to_string();
    assert_ne!(new, a);
    // Old account: closed, order cancelled, no further trading.
    let orders = app.get(&format!("/api/v1/accounts/{a}/orders"), &c).await;
    assert_eq!(orders.body[0]["status"], "cancelled");
    let r = app.order(&c, &a, json!({ "symbol": "ACME", "side": "buy", "kind": "market", "quantity": "1" })).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert_eq!(dec(&app.portfolio(&c, &new).await["equity"]), dec!(100000));
    app.assert_reconciled(&c, &a).await;
}

#[sqlx::test(migrator = "fintech_backend::db::MIGRATOR")]
async fn equity_snapshots(pool: PgPool) {
    let app = TestApp::new(pool);
    let (c, a) = app.register("alice@example.com").await;
    assert_eq!(fintech_backend::jobs::snapshot_once(&app.state).await.unwrap(), 1);
    let r = app.get(&format!("/api/v1/accounts/{a}/equity"), &c).await;
    assert_eq!(dec(&r.body[0]["equity"]), dec!(100000));
}
