const usd = new Intl.NumberFormat('en-US', { style: 'currency', currency: 'USD' })
const compact = new Intl.NumberFormat('en-US', { style: 'currency', currency: 'USD', notation: 'compact', maximumFractionDigits: 1 })
const qtyFmt = new Intl.NumberFormat('en-US', { maximumFractionDigits: 8 })

export const money = (n: number) => usd.format(n)
export const moneyCompact = (n: number) => compact.format(n)
export const signedMoney = (n: number) => (n > 0 ? '+' : n < 0 ? '−' : '') + usd.format(Math.abs(n))
export const pct = (n: number) => `${n > 0 ? '+' : n < 0 ? '−' : ''}${Math.abs(n).toFixed(2)}%`
export const tone = (n: number) => (n > 0 ? 'up' : n < 0 ? 'down' : 'flat')
export const time = (iso: string) => new Date(iso).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' })
export const dateTime = (iso: string) => new Date(iso).toLocaleString([], { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' })

/** Instrument price: cents for normal prices, more precision for sub-dollar ones. */
export function price(n: number) {
  const digits = n >= 1 ? 2 : n >= 0.01 ? 4 : 8
  return n.toLocaleString('en-US', { style: 'currency', currency: 'USD', minimumFractionDigits: digits, maximumFractionDigits: digits })
}

/** Quantity: whole numbers stay whole, fractions up to 8 decimals. */
export const qty = (n: number) => qtyFmt.format(n)

/** "12s ago", "5m ago", "3h ago" */
export function ago(iso: string, now = Date.now()) {
  const s = Math.max(0, Math.round((now - new Date(iso).getTime()) / 1000))
  if (s < 60) return `${s}s ago`
  if (s < 3600) return `${Math.floor(s / 60)}m ago`
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`
  return `${Math.floor(s / 86400)}d ago`
}

export const SOURCE_LABELS: Record<string, string> = {
  coinbase: 'Crypto · Coinbase',
  alpaca: 'US stocks · Alpaca',
  simulated: 'Simulated market',
}
export const sourceLabel = (id: string) => SOURCE_LABELS[id] ?? id
