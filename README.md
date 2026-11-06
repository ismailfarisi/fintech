# Ledger: portfolio & paper trading

A multi-user portfolio tracker and paper-trading app. The backend is **Rust** (axum, tokio, sqlx) on **PostgreSQL**; the frontend is **React + TypeScript**.
Prices are live: crypto from Coinbase (no key needed) and US stocks from Alpaca (free account).
Orders fill against those real prices on a double-entry ledger, using paper money only.

```
backend/    Rust API: domain · services · repositories · market-data adapters · jobs
  migrations/   PostgreSQL schema (applied automatically on startup)
  tests/        integration tests against a real database
frontend/   React SPA (Vite)
docs/       ARCHITECTURE.md: layers, money model, concurrency, how to extend
SECURITY.md threat model, controls, production checklist
```

## Features

- **Accounts and security:** sign-up and sign-in with Argon2id-hashed passwords, plus hardened session cookies that expire after inactivity and can be revoked per device. Users get a security activity log. See [SECURITY.md](SECURITY.md).
- **Portfolio:** equity, all-time / realized / unrealized P&L, an equity curve, holdings with cost basis, and allocation.
- **Trading:** market and limit orders with fractional quantities using each instrument's lot size. Limit orders reserve funds, are matched automatically and can be cancelled. Retries are safe thanks to idempotency keys.
- **Correct money:** exact decimals end to end, and a double-entry ledger whose invariants (balanced entries, no overdrafts, immutable history) are enforced by PostgreSQL itself. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).
- **Live data:** pluggable market-data adapters, streamed to the browser over an authenticated WebSocket.

## Market data

Providers are configured in [`backend/markets.toml`](backend/markets.toml):

| Provider | Markets | Key needed? | Notes |
|---|---|---|---|
| `coinbase` | Crypto (any Coinbase product, e.g. `BTC-USD`) | No | Public REST + WebSocket `ticker` feed |
| `alpaca` | US stocks and ETFs | Free account | `iex` feed is free and real-time but only covers IEX volume; `sip` needs a paid plan |
| `simulated` | Fictional stocks | No | Offline demos; random-walk prices |

**Alpaca setup:** create a free account at [alpaca.markets](https://alpaca.markets), generate API keys (paper keys work), and put them in `.env` as `ALPACA_API_KEY_ID` and `ALPACA_API_SECRET_KEY`.

Without the keys, Alpaca is skipped with a warning and everything else keeps working. If no provider can start at all, the backend falls back to the simulator. Feed health is shown on the Activity page and at `GET /api/v1/market/providers`; error details go to the server log only.

### Adding a provider

Adapters live in `backend/src/market/`. Each is an implementation of one trait:

```rust
#[async_trait]
pub trait MarketDataProvider: Send + Sync + 'static {
    fn id(&self) -> &str;                      // "binance"
    fn instruments(&self) -> Vec<Instrument>;  // symbols you serve
    fn resolution_secs(&self) -> i64 { 60 }    // history bucket width
    async fn run(&self, sink: MarketSink) -> anyhow::Result<()>;
}
```

`run` connects, sends a `MarketEvent::Snapshot` per symbol (price, stats, history), then streams `MarketEvent::Tick`s until the connection drops. The hub (`hub.rs`) handles everything else: symbol registration (duplicates across providers are rejected), reconnects with exponential backoff, health status, and storage.

To add one:

1. Create `src/market/<name>.rs` with a `<Name>Config` (serde, with defaults) and a provider implementing the trait. Use `crate::net::http_client()` and `crate::net::connect_ws()`, which honour `HTTPS_PROXY`/`NO_PROXY` and the system CA store.
2. Add a variant to `ProviderConfig` in `src/market/config.rs` and a line in `build()`.
3. Add a `[[provider]] kind = "<name>"` block to `markets.toml`.

`coinbase.rs` (no auth) and `alpaca.rs` (key auth) are worked examples. `alpaca.rs` also shows how to test an adapter end to end against a local mock server.

## Running

### With Docker (simplest)

```sh
cp .env.example .env          # then set POSTGRES_PASSWORD (and Alpaca keys if you have them)
docker compose up --build     # http://localhost:8080
```

### For development

Requirements: Rust (stable), Node 22+, PostgreSQL 16 (or `docker compose up db`).

```sh
cp .env.example .env                 # DATABASE_URL must point at your Postgres
cd backend && cargo run              # API on http://localhost:8080 (runs migrations)
cd frontend && npm install && npm run dev   # UI on http://localhost:5173, proxied to the API
```

To serve the built UI from the backend instead, run `npm run build` in `frontend/` and open http://localhost:8080.

### Tests

```sh
cd backend
DATABASE_URL=postgres://ledger:...@localhost:5432/ledger cargo test   # needs CREATEDB; each test gets a fresh database
cargo clippy --all-targets -- -D warnings && cargo fmt --check
cargo deny check                                                      # advisories, licenses, sources
```

CI (`.github/workflows/ci.yml`) runs all of these, plus the frontend lint, build and `npm audit`, and a Docker build.

### Configuration

All settings are environment variables; see [`.env.example`](.env.example) for the full list with explanations. The important ones:

| Variable | Default | |
|---|---|---|
| `DATABASE_URL` | (required) | PostgreSQL connection string |
| `PUBLIC_ORIGIN` | `http://localhost:8080` | The URL users open. Other origins can't make state-changing requests. |
| `ALLOWED_ORIGINS` | (none) | Extra origins, e.g. `http://localhost:5173` for the Vite dev server |
| `COOKIE_SECURE` | `true` | Keep `true` (works on http://localhost too) |
| `TRUST_PROXY_HEADERS` | `false` | Only behind your own reverse proxy |
| `SESSION_IDLE_MINUTES` / `SESSION_ABSOLUTE_HOURS` | `30` / `12` | Session lifetime |
| `STARTING_CASH` / `TRADING_FEE_BPS` | `100000` / `0` | Paper account funding and simulated fees |

## API (v1)

All routes are under `/api/v1`. Authentication is by session cookie. Money is sent and returned as decimal strings. Errors look like `{"error": {"code": "insufficient_funds", "message": "..."}}`.

| Method | Path | |
|---|---|---|
| POST | `/auth/register`, `/auth/login`, `/auth/logout` | Sign up, sign in, sign out |
| GET | `/auth/me` | Current user and accounts |
| POST | `/auth/password` | Change password (signs out other sessions) |
| GET / DELETE | `/auth/sessions[/{id}]`, POST `/auth/sessions/revoke-others` | Device sessions |
| GET | `/auth/activity` | Your security and account activity log |
| GET | `/market/quotes[/{symbol}]`, `/market/providers` | Market data |
| GET / PUT / DELETE | `/watchlist[/{symbol}]` | Watchlist |
| GET | `/accounts`, `/accounts/{id}` | Your accounts |
| GET | `/accounts/{id}/portfolio`, `/equity`, `/trades`, `/cash`, `/reconcile` | Account data |
| GET / POST | `/accounts/{id}/orders` | List, or place (`Idempotency-Key` supported) |
| GET / DELETE | `/accounts/{id}/orders/{order_id}` | Get or cancel |
| POST | `/accounts/{id}/cash/deposits`, `/cash/withdrawals` | Move paper cash (`Idempotency-Key` supported) |
| POST | `/accounts/{id}/reset` | Close the account and open a fresh one |
| GET (WS) | `/ws?account_id=...` | Live `tick` (quotes + portfolio) and `account` events |

Health checks: `GET /healthz` (process up) and `GET /readyz` (database reachable).

## Limitations

- Paper trading only: no real brokerage execution, spread, slippage or market hours. Fees are configurable and default to 0.
- Single instance: the rate limiter and event bus are in-process (see the production checklist in SECURITY.md).
- No email verification, password reset or 2FA yet. These are the next security features to add.
