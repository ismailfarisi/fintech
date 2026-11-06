// Typed client for the Rust backend.

export type Side = 'buy' | 'sell'
export type OrderKind = 'market' | 'limit'
export type OrderStatus = 'open' | 'filled' | 'cancelled'

export interface Quote {
  symbol: string
  name: string
  sector: string
  price: number
  open: number
  high: number
  low: number
  change: number
  change_pct: number
  updated_at: string
}

export interface PricePoint { t: string; price: number }
export interface EquityPoint { t: string; equity: number }

export interface PositionView {
  symbol: string
  qty: number
  avg_cost: number
  price: number
  market_value: number
  cost_basis: number
  unrealized_pnl: number
  unrealized_pnl_pct: number
  weight: number
}

export interface Portfolio {
  cash: number
  buying_power: number
  positions_value: number
  equity: number
  net_contributions: number
  total_pnl: number
  total_pnl_pct: number
  realized_pnl: number
  unrealized_pnl: number
  positions: PositionView[]
}

export interface Order {
  id: string
  symbol: string
  side: Side
  kind: OrderKind
  qty: number
  limit_price: number | null
  status: OrderStatus
  created_at: string
  filled_at: string | null
  fill_price: number | null
}

export interface Trade {
  id: string
  order_id: string
  symbol: string
  side: Side
  qty: number
  price: number
  total: number
  realized_pnl: number | null
  at: string
}

export interface CashFlow {
  id: string
  kind: 'deposit' | 'withdrawal'
  amount: number
  at: string
}

export interface OrderRequest {
  symbol: string
  side: Side
  kind: OrderKind
  qty: number
  limit_price?: number
}

export type LiveEvent =
  | { type: 'tick'; quotes: Quote[]; portfolio: Portfolio }
  | { type: 'fill'; trade: Trade }
  | { type: 'account_changed' }

export class ApiError extends Error {}

async function req<T>(method: string, path: string, body?: unknown): Promise<T> {
  const res = await fetch(path, {
    method,
    headers: body === undefined ? undefined : { 'content-type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
  })
  const data = await res.json().catch(() => null)
  if (!res.ok) throw new ApiError(data?.error ?? `${res.status} ${res.statusText}`)
  return data as T
}

export const api = {
  quotes: () => req<Quote[]>('GET', '/api/quotes'),
  quote: (symbol: string) => req<{ quote: Quote; history: PricePoint[] }>('GET', `/api/quotes/${symbol}`),
  portfolio: () => req<Portfolio>('GET', '/api/portfolio'),
  equityHistory: () => req<EquityPoint[]>('GET', '/api/portfolio/history'),
  orders: (status?: OrderStatus) => req<Order[]>('GET', `/api/orders${status ? `?status=${status}` : ''}`),
  placeOrder: (o: OrderRequest) => req<{ order: Order; trade: Trade | null }>('POST', '/api/orders', o),
  cancelOrder: (id: string) => req<Order>('DELETE', `/api/orders/${id}`),
  trades: () => req<Trade[]>('GET', '/api/trades'),
  cashFlows: () => req<CashFlow[]>('GET', '/api/cash'),
  deposit: (amount: number) => req<Portfolio>('POST', '/api/cash/deposit', { amount }),
  withdraw: (amount: number) => req<Portfolio>('POST', '/api/cash/withdraw', { amount }),
  watchlist: () => req<string[]>('GET', '/api/watchlist'),
  watch: (s: string) => req<string[]>('PUT', `/api/watchlist/${s}`),
  unwatch: (s: string) => req<string[]>('DELETE', `/api/watchlist/${s}`),
  reset: (starting_cash: number) => req<Portfolio>('POST', '/api/account/reset', { starting_cash }),
}
