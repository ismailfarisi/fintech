// Typed client for the Rust backend (/api/v1).
//
// Money on the wire is always a decimal *string* ("937.50"). Values are parsed
// to numbers only for display/charting. Anything the user types (quantities,
// prices, amounts) is sent back as the exact string they entered, never via a float.

export type Side = 'buy' | 'sell'
export type OrderKind = 'market' | 'limit'
export type OrderStatus = 'open' | 'filled' | 'cancelled'

export interface Quote {
  symbol: string
  name: string
  asset_class: 'equity' | 'crypto'
  /** Provider id, e.g. "coinbase", "alpaca", "simulated". */
  source: string
  /** What `reference` means: "Prev close", "24h open", ... */
  reference_label: string
  /** Smallest tradable quantity. */
  qty_step: number
  price: number
  reference: number
  high: number
  low: number
  change: number
  change_pct: number
  updated_at: string
}

export interface ProviderStatus {
  id: string
  state: 'connecting' | 'live' | 'reconnecting'
  symbols: number
  last_event_at: string | null
}

export interface PricePoint { t: string; price: number }
export interface EquityPoint { t: string; equity: number }

export interface PositionView {
  symbol: string
  qty: number
  available: number
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
  fee: number
  realized_pnl: number | null
  at: string
}

export interface CashFlow {
  id: string
  kind: 'deposit' | 'withdrawal'
  amount: number
  at: string
}

export interface User { id: string; email: string; display_name: string; created_at: string }
export interface Account { id: string; name: string; kind: string; base_currency: string; created_at: string; closed_at: string | null }
export interface Me { user: User; accounts: Account[] }
export interface SessionInfo { id: string; created_at: string; last_seen_at: string; expires_at: string; ip: string | null; user_agent: string | null; current: boolean }
export interface AuditEntry { at: string; action: string; ip: string | null; user_agent: string | null; details: Record<string, unknown> }

/** Order request: quantity and limit price are exact decimal strings. */
export interface OrderRequest {
  symbol: string
  side: Side
  kind: OrderKind
  quantity: string
  limit_price?: string
}

/** A fill notification from the live stream. */
export interface FillNotice { order_id: string; symbol: string; side: Side; quantity: number; price: number }

export class ApiError extends Error {
  readonly status: number
  readonly code: string
  constructor(status: number, code: string, message: string) {
    super(message)
    this.status = status
    this.code = code
  }
}

// ---- decoding -----------------------------------------------------------

type Raw = Record<string, unknown>
const n = (v: unknown): number => (v === null || v === undefined ? NaN : Number(v))
const nOrNull = (v: unknown): number | null => (v === null || v === undefined ? null : Number(v))

export function decodePortfolio(r: Raw): Portfolio {
  return {
    cash: n(r.cash),
    buying_power: n(r.buying_power),
    positions_value: n(r.positions_value),
    equity: n(r.equity),
    net_contributions: n(r.net_contributions),
    total_pnl: n(r.total_pnl),
    total_pnl_pct: n(r.total_pnl_pct),
    realized_pnl: n(r.realized_pnl),
    unrealized_pnl: n(r.unrealized_pnl),
    positions: (r.positions as Raw[]).map((p) => ({
      symbol: p.symbol as string,
      qty: n(p.quantity),
      available: n(p.available),
      avg_cost: n(p.avg_cost),
      price: p.price === null ? n(p.avg_cost) : n(p.price),
      market_value: n(p.market_value),
      cost_basis: n(p.cost_basis),
      unrealized_pnl: n(p.unrealized_pnl),
      unrealized_pnl_pct: n(p.unrealized_pnl_pct),
      weight: n(p.weight_pct),
    })),
  }
}

const decodeOrder = (o: Raw): Order => ({
  id: o.id as string,
  symbol: o.symbol as string,
  side: o.side as Side,
  kind: o.kind as OrderKind,
  qty: n(o.quantity),
  limit_price: nOrNull(o.limit_price),
  status: o.status as OrderStatus,
  created_at: o.created_at as string,
  filled_at: (o.filled_at as string) ?? null,
  fill_price: nOrNull(o.fill_price),
})

const decodeTrade = (t: Raw): Trade => ({
  id: t.id as string,
  order_id: t.order_id as string,
  symbol: t.symbol as string,
  side: t.side as Side,
  qty: n(t.quantity),
  price: n(t.price),
  total: n(t.gross),
  fee: n(t.fee),
  realized_pnl: nOrNull(t.realized_pnl),
  at: t.executed_at as string,
})

// ---- transport ----------------------------------------------------------

/** Called when the session has ended (401), so the app can show the sign-in screen. */
let onUnauthenticated: () => void = () => {}
export function setUnauthenticatedHandler(fn: () => void) {
  onUnauthenticated = fn
}

/** The account all account-scoped calls go to (set by the auth layer). */
let accountId: string | null = null
export function setActiveAccount(id: string | null) {
  accountId = id
}
function acct() {
  if (!accountId) throw new ApiError(0, 'no_account', 'no account selected')
  return `/api/v1/accounts/${accountId}`
}

/** Unique per user action; lets the server de-duplicate retried requests. */
export const newIdempotencyKey = () => crypto.randomUUID()

async function req<T>(method: string, path: string, body?: unknown, headers: Record<string, string> = {}): Promise<T> {
  const res = await fetch(path, {
    method,
    credentials: 'same-origin',
    headers: { ...(body === undefined ? {} : { 'content-type': 'application/json' }), ...headers },
    body: body === undefined ? undefined : JSON.stringify(body),
  })
  if (res.status === 204) return undefined as T
  const data = await res.json().catch(() => null)
  if (!res.ok) {
    const code = data?.error?.code ?? 'http_error'
    if (res.status === 401 && code === 'unauthenticated') onUnauthenticated()
    throw new ApiError(res.status, code, data?.error?.message ?? `${res.status} ${res.statusText}`)
  }
  return data as T
}

const decodeQuote = (q: Raw): Quote => q as unknown as Quote // market data is already numeric

export const api = {
  // auth
  me: () => req<Me>('GET', '/api/v1/auth/me'),
  login: (email: string, password: string) => req<Me>('POST', '/api/v1/auth/login', { email, password }),
  register: (email: string, password: string, display_name: string) =>
    req<Me>('POST', '/api/v1/auth/register', { email, password, display_name }),
  logout: () => req<void>('POST', '/api/v1/auth/logout'),
  changePassword: (current_password: string, new_password: string) =>
    req<{ other_sessions_revoked: number }>('POST', '/api/v1/auth/password', { current_password, new_password }),
  sessions: () => req<SessionInfo[]>('GET', '/api/v1/auth/sessions'),
  revokeSession: (id: string) => req<void>('DELETE', `/api/v1/auth/sessions/${id}`),
  revokeOtherSessions: () => req<{ revoked: number }>('POST', '/api/v1/auth/sessions/revoke-others'),
  securityLog: () => req<AuditEntry[]>('GET', '/api/v1/auth/activity'),

  // market
  providers: () => req<ProviderStatus[]>('GET', '/api/v1/market/providers'),
  quotes: () => req<Raw[]>('GET', '/api/v1/market/quotes').then((qs) => qs.map(decodeQuote)),
  quote: (symbol: string) =>
    req<{ quote: Quote; history: PricePoint[] }>('GET', `/api/v1/market/quotes/${encodeURIComponent(symbol)}`),
  watchlist: () => req<string[]>('GET', '/api/v1/watchlist'),
  watch: (s: string) => req<string[]>('PUT', `/api/v1/watchlist/${encodeURIComponent(s)}`),
  unwatch: (s: string) => req<string[]>('DELETE', `/api/v1/watchlist/${encodeURIComponent(s)}`),

  // account-scoped
  portfolio: () => req<Raw>('GET', `${acct()}/portfolio`).then(decodePortfolio),
  equityHistory: () =>
    req<Raw[]>('GET', `${acct()}/equity?hours=24`).then((ps) => ps.map((p) => ({ t: p.at as string, equity: n(p.equity) }))),
  orders: (status?: OrderStatus) =>
    req<Raw[]>('GET', `${acct()}/orders${status ? `?status=${status}` : ''}`).then((os) => os.map(decodeOrder)),
  placeOrder: (o: OrderRequest, idempotencyKey: string) =>
    req<{ order: Raw; trade: Raw | null }>('POST', `${acct()}/orders`, o, { 'Idempotency-Key': idempotencyKey }).then((r) => ({
      order: decodeOrder(r.order),
      trade: r.trade ? decodeTrade(r.trade) : null,
    })),
  cancelOrder: (id: string) => req<Raw>('DELETE', `${acct()}/orders/${id}`).then(decodeOrder),
  trades: () => req<Raw[]>('GET', `${acct()}/trades`).then((ts) => ts.map(decodeTrade)),
  cashFlows: () =>
    req<Raw[]>('GET', `${acct()}/cash`).then((fs) =>
      fs.map((f) => ({ id: f.id as string, kind: f.kind as CashFlow['kind'], amount: n(f.amount), at: f.created_at as string })),
    ),
  deposit: (amount: string, key: string) => req<Raw>('POST', `${acct()}/cash/deposits`, { amount }, { 'Idempotency-Key': key }),
  withdraw: (amount: string, key: string) => req<Raw>('POST', `${acct()}/cash/withdrawals`, { amount }, { 'Idempotency-Key': key }),
  reset: () => req<{ account_id: string }>('POST', `${acct()}/reset`, {}),
  wsUrl: () => {
    const proto = location.protocol === 'https:' ? 'wss' : 'ws'
    return `${proto}://${location.host}/api/v1/ws?account_id=${encodeURIComponent(accountId ?? '')}`
  },
}
