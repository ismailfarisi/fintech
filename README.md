# Ledger: personal portfolio & paper-trading app

A personal portfolio tracker and paper-trading app with a **Rust** backend and a **React + TypeScript** frontend.
Prices are **live**: crypto from Coinbase (no account needed) and US stocks from Alpaca (free account).
Trades are paper trades: they fill against real prices in the app's own engine, and no real money moves.

```
backend/   Rust (axum + tokio): market-data adapters, trading engine, REST + WebSocket API
frontend/  React + Vite: portfolio dashboard, trading screen, activity & cash management
```

## Features

- **Portfolio dashboard**: total equity, all-time P&L against money invested, realized and unrealized P&L, an equity curve, holdings with cost basis, and allocation.
- **Trading**: market and limit orders, buy and sell. Resting limit orders reserve cash or shares and fill automatically when the price crosses the limit. Orders can be cancelled.
- **Live market data** from pluggable adapters (Coinbase, Alpaca, or an offline simulator), streamed to the browser over a WebSocket (`/ws`). Includes a price chart per instrument, watchlist starring, search, and per-feed health status.
- **Fractional quantities** using each instrument's lot size (e.g. 0.00000001 BTC, 0.1 DOGE, whole shares for stocks by default).
- **Activity**: trade blotter, order history, deposits and withdrawals, and account reset.
- **Persistence**: the account and last prices are saved to a JSON file (written atomically) and restored on restart.

## Market data

Providers are configured in [`backend/markets.toml`](backend/markets.toml):

| Provider | Markets | Key needed? | Notes |
|---|---|---|---|
| `coinbase` | Crypto (any Coinbase product, e.g. `BTC-USD`) | No | Public REST + WebSocket `ticker` feed |
| `alpaca` | US stocks and ETFs | Free account | `iex` feed is free and real-time but only covers IEX volume; `sip` needs a paid plan |
| `simulated` | Fictional stocks | No | Offline demos; random-walk prices |

**Alpaca setup:** create a free account at [alpaca.markets](https://alpaca.markets), generate API keys (paper keys work), and start the backend with:

```sh
export ALPACA_API_KEY_ID=...  ALPACA_API_SECRET_KEY=...
```

Without the keys, Alpaca is skipped with a warning and everything else keeps working. If no provider can start at all, the backend falls back to the simulator. Feed health (live, reconnecting, last error) is shown on the Activity page and at `GET /api/providers`.

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

Requirements: Rust (stable) and Node 20+.

**Development** (hot reload, with Vite proxying `/api` and `/ws` to the backend):

```sh
cd backend && cargo run              # http://localhost:8080
cd frontend && npm install && npm run dev   # http://localhost:5173
```

**Single binary serving the built UI:**

```sh
cd frontend && npm install && npm run build
cd ../backend && cargo run --release   # open http://localhost:8080
```

### Backend configuration (env vars)

| Variable     | Default              | Meaning                                  |
|--------------|----------------------|------------------------------------------|
| `PORT`       | `8080`               | HTTP port                                |
| `DATA_FILE`  | `data/account.json`  | Where the account is persisted           |
| `STATIC_DIR` | `../frontend/dist`   | Built frontend to serve (if present)     |
| `MARKETS_CONFIG` | `markets.toml`   | Market-data provider config              |
| `TICK_MS`    | `1000`               | Order-matching / UI push interval (ms)   |
| `ALPACA_API_KEY_ID`, `ALPACA_API_SECRET_KEY` | (unset) | Alpaca credentials (also accepts `APCA_*`) |

## API

| Method | Path | Description |
|---|---|---|
| GET | `/api/providers` | Market-data feed status |
| GET | `/api/quotes` | All quotes (symbols that have a price) |
| GET | `/api/quotes/{symbol}` | Quote + recent price history |
| GET | `/api/portfolio` | Cash, equity, P&L, positions |
| GET | `/api/portfolio/history` | Equity curve |
| GET | `/api/orders?status=open\|filled\|cancelled` | Orders, newest first |
| POST | `/api/orders` | `{symbol, side: buy\|sell, kind: market\|limit, qty, limit_price?}` |
| DELETE | `/api/orders/{id}` | Cancel an open order |
| GET | `/api/trades` | Executions |
| GET | `/api/cash` | Deposits and withdrawals |
| POST | `/api/cash/deposit`, `/api/cash/withdraw` | `{amount}` |
| GET / PUT / DELETE | `/api/watchlist[/{symbol}]` | Watchlist |
| POST | `/api/account/reset` | `{starting_cash?}` (default $100,000) |
| WS | `/ws` | Events: `tick` (quotes + portfolio), `fill`, `account_changed` |

Errors return `{"error": "..."}` with 404 (unknown symbol/order), 409 (order not open) or 422 (validation, insufficient funds or shares).

## Design notes

- `engine.rs` is pure. It takes prices and timestamps as inputs, has no I/O, and is unit-tested (`cargo test`).
- Quantities are fractional (8 decimals) and must be a multiple of the instrument's lot size. Cash is rounded to cents after every operation.
- Fills: market orders fill at the last trade price. Marketable limit orders fill immediately at the market price. Resting limits fill at the first tick that crosses them, at that tick's price, which is never worse than the limit. There's no simulated spread, slippage or fees, and fills ignore market hours (outside trading hours you trade at the last price).
- Positions are valued at the live price, or the last saved price while a feed is connecting. Limit orders match on live prices only.
- If you remove a symbol from `markets.toml`, existing positions in it stay, valued at their last saved price, but can't be traded.
- Single-user and unauthenticated, meant to run locally. Add authentication before exposing it to a network.
