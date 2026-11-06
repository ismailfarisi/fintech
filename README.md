# Ledger: personal portfolio & paper-trading app

A personal portfolio tracker and trading app with a **Rust** backend and a **React + TypeScript** frontend.
Prices come from a built-in simulated market, so it runs with no API keys or brokerage account.

```
backend/   Rust (axum + tokio): market simulator, trading engine, REST + WebSocket API
frontend/  React + Vite: portfolio dashboard, trading screen, activity & cash management
```

## Features

- **Portfolio dashboard**: total equity, all-time P&L against money invested, realized and unrealized P&L, an equity curve, holdings with cost basis, and allocation.
- **Trading**: market and limit orders, buy and sell. Resting limit orders reserve cash or shares and fill automatically when the price crosses the limit. Orders can be cancelled.
- **Live prices** streamed over a WebSocket (`/ws`), with a price chart per instrument, watchlist starring, and search.
- **Activity**: trade blotter, order history, deposits and withdrawals, and account reset.
- **Persistence**: the account and last prices are saved to a JSON file (written atomically) and restored on restart.

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
| `TICK_MS`    | `1000`               | Market tick interval in milliseconds     |

## API

| Method | Path | Description |
|---|---|---|
| GET | `/api/quotes` | All quotes |
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
- Quantities are whole shares (`u64`). Cash and prices are rounded to cents after every operation.
- Fills: market orders fill at the current price. Marketable limit orders fill immediately at the market price. Resting limits fill at the first tick that crosses them, at that tick's price, which is never worse than the limit.
- `market.rs` is the only price source. Swapping in a real data feed means replacing it while keeping `prices()`, `quotes()` and `history()`.
- Single-user and unauthenticated, meant to run locally. Add authentication before exposing it to a network.
