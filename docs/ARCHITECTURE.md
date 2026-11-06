# Architecture

Ledger is a multi-user portfolio and paper-trading app: a Rust backend (axum, tokio, sqlx) on PostgreSQL, plus a React frontend. This document explains how the backend is organised and how to extend it without weakening its guarantees.

## Layers

```
                 ┌──────────────────────────────────────────────────────────┐
  HTTP / WS ───▶ │ api/         routing, DTOs, auth extractor, idempotency   │  thin
                 ├──────────────────────────────────────────────────────────┤
                 │ services/    use-cases in DB transactions; emit events   │
                 ├───────────────────────────┬──────────────────────────────┤
                 │ domain/  pure rules       │ db/  repositories (SQL only) │
                 │ (no I/O, Decimal math)    │                              │
                 └───────────────────────────┴──────────────────────────────┘
  jobs/     background loops: order matching, equity snapshots, cleanup
  market/   market-data adapters -> hub -> in-memory MarketStore
  security/ password hashing, tokens, rate limiting, HTTP hardening
  config.rs typed env config (secrets redacted)   app.rs  shared state
```

Rules of thumb:

| Layer | May depend on | Must not |
|---|---|---|
| `domain` | nothing but `rust_decimal`/`serde` | do I/O, know about HTTP or SQL |
| `db` | `domain` types | contain business rules; build SQL dynamically |
| `services` | `domain`, `db`, `events` | read HTTP headers or cookies |
| `api` | `services` | touch SQL directly for writes; skip `get_owned` |

## Money model

**Exact decimals everywhere.** Amounts are `rust_decimal::Decimal` in Rust, `NUMERIC(38,8)` in Postgres, and decimal *strings* in JSON (`"937.50"`). Clients must send strings: JSON floats are rejected. Market data arrives as `f64` and is converted once, at the boundary (`domain::money::price_from_f64`).

**Double-entry ledger.** Every money movement is a `ledger_entries` row plus `postings` that sum to zero per asset:

| Event | Postings |
|---|---|
| Deposit $100 | account USD +100, external USD −100 |
| Buy 2 ACME @ $50, $1 fee | account USD −101, market USD +100, fees USD +1, account ACME +2, market ACME −2 |
| Sell | the mirror image |

`holdings` caches the `account`-book balance per (account, asset), plus `reserved` (held by open orders) and `cost_basis` (for P&L). It's updated in the same transaction as the postings. `GET /accounts/{id}/reconcile` and the test suite check that holdings always equal the ledger.

**The database enforces the invariants**, so an application bug can't corrupt balances:

- A deferred constraint trigger rejects any transaction whose entry doesn't balance.
- CHECK constraints reject a negative balance, or a reservation larger than the holding.
- Triggers make `ledger_entries`, `postings`, `trades` and `audit_log` append-only, rejecting UPDATE, DELETE and TRUNCATE.
- Orders have state CHECKs; for example, a filled order must have a fill price.

Because history is immutable, "reset account" closes the account and opens a new one, instead of deleting anything.

## Concurrency

Each money-moving use-case runs in one transaction that first runs `SELECT … FROM accounts WHERE id = $1 FOR UPDATE`. That serialises all operations on one account (place, cancel, fill, deposit, withdraw) with no deadlocks, while different accounts proceed in parallel. Holdings rows are locked in a fixed order, and the CHECK constraints are the last line of defence. The test `concurrent_orders_cannot_overdraw` fires 30 simultaneous buys at an account that can afford 20 and asserts exactly 20 succeed.

The matcher (`jobs::match_orders`) runs every second. It finds open limit orders that the current price crosses, then fills each one in its own transaction after re-checking that the order is still open, under the same account lock. An order can never be filled twice or filled after it was cancelled.

## Idempotency

`POST /orders`, `/cash/deposits` and `/cash/withdrawals` accept an `Idempotency-Key` header, and the frontend sends a fresh UUID per user action. The handler takes a Postgres advisory lock on (user, key), replays the stored response if the key was seen before, and otherwise runs the operation and stores the response in the **same transaction**. A retried or double-submitted request can't place two orders. Reusing a key for a different request returns `422 idempotency_key_reused`. Keys expire after a day.

## Events and live updates

Services return `Event`s, which are published to the in-process `EventBus` **after commit** (never for rolled-back work). Each WebSocket connection subscribes to the bus and receives:

- `tick`: quotes, plus the portfolio valued from a cached `AccountBook`
- `account`: a change on its own account, which triggers a reload of the book

To scale to several instances, replace `EventBus` with Postgres `LISTEN/NOTIFY` or a broker. The interface stays the same.

## Market data

Covered in the README ("Market data"). Adapters implement `MarketDataProvider`. The hub supervises reconnects and writes into `MarketStore`. Prices are in memory only (they're observations, not records); the equity chart uses `equity_snapshots`, which are written every minute.

## Adding a feature: checklist

1. **Schema:** add a new migration `backend/migrations/NNNN_name.sql`. Never edit an applied one. Add CHECK constraints for every invariant you can express.
2. **Domain:** pure functions and types with unit tests. Money is `Decimal`.
3. **Repository:** static SQL only, bound parameters, and every account-scoped query filtered by the owner.
4. **Service:** takes `&mut PgConnection` if it moves money, locks the account first, writes an `audit::record`, and returns events.
5. **API:**
   - Call `services::accounts::get_owned` before anything account-scoped.
   - Put `#[serde(deny_unknown_fields)]` on request DTOs.
   - Accept money as `Decimal`.
   - Wrap money-moving POSTs in `idempotency::run`.
6. **Tests:** add an integration test in `backend/tests/` using `#[sqlx::test]` (a real database). Test both the happy path and someone else's account (expect 404).

Ideas that fit the existing structure:

- **Multiple accounts per user.** The schema and API already support them; the UI only needs an account switcher.
- **Stop and stop-limit orders.** Add the order kinds, the matching rule in the domain, and a matcher clause.
- **Real brokerage execution.** Add an `Executor` trait next to `MarketDataProvider`, with a paper implementation (today's `execute`) and an Alpaca implementation.
- **2FA (TOTP), email verification, password reset.** Add tables and auth services; sessions already support revocation.
- **Statements and exports.** These are read-only queries over `ledger_entries`/`postings`.
- **Admin and ops:** reconciliation across all accounts, plus metrics (Prometheus) built on the request-id traces.
