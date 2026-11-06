import { useState } from 'react'
import { api, ApiError, newIdempotencyKey, type OrderKind, type Quote, type Side } from '../api'
import { money, price, qty as fmtQty } from '../format'
import { useLive } from '../live'

/** Round down to a multiple of `step`, as a plain decimal string (never "1e-8"). */
function floorToStep(n: number, step: number): string {
  const decimals = Math.min(8, Math.max(0, Math.ceil(-Math.log10(step))))
  return (Math.floor(n / step + 1e-9) * step).toFixed(decimals)
}

export function OrderTicket({ quote }: { quote: Quote }) {
  const { portfolio, refresh } = useLive()
  const step = quote.qty_step
  const [side, setSide] = useState<Side>('buy')
  const [kind, setKind] = useState<OrderKind>('market')
  const [qty, setQty] = useState(step >= 1 ? '1' : floorToStep(Math.max(step, 100 / quote.price), step))
  const [limit, setLimit] = useState('')
  const [busy, setBusy] = useState(false)
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null)

  const q = Number(qty)
  const validQty = Number.isFinite(q) && q > 0
  const px = kind === 'limit' ? Number(limit) : quote.price
  const estimate = validQty && px > 0 ? q * px : 0
  const held = portfolio?.positions.find((p) => p.symbol === quote.symbol)?.available ?? 0
  const buyingPower = portfolio?.buying_power ?? 0
  const maxBuy = px > 0 ? floorToStep(buyingPower / px, step) : '0'

  const submit = async (e: React.FormEvent) => {
    e.preventDefault()
    setBusy(true)
    setMsg(null)
    try {
      // Send exactly what the user typed; the server does the decimal math.
      const { order, trade } = await api.placeOrder(
        { symbol: quote.symbol, side, kind, quantity: qty.trim(), limit_price: kind === 'limit' ? limit.trim() : undefined },
        newIdempotencyKey(),
      )
      setMsg({
        ok: true,
        text: trade
          ? `Filled: ${side} ${fmtQty(trade.qty)} ${trade.symbol} @ ${price(trade.price)}`
          : `Limit order placed: ${side} ${fmtQty(order.qty)} ${order.symbol} @ ${price(order.limit_price!)}`,
      })
      refresh()
    } catch (err) {
      setMsg({ ok: false, text: err instanceof ApiError ? err.message : 'Order failed' })
    } finally {
      setBusy(false)
    }
  }

  return (
    <form className="card ticket" onSubmit={submit}>
      <div className="segmented">
        <button type="button" className={side === 'buy' ? 'active buy' : ''} onClick={() => setSide('buy')}>Buy</button>
        <button type="button" className={side === 'sell' ? 'active sell' : ''} onClick={() => setSide('sell')}>Sell</button>
      </div>
      <div className="segmented small">
        <button type="button" className={kind === 'market' ? 'active' : ''} onClick={() => setKind('market')}>Market</button>
        <button type="button" className={kind === 'limit' ? 'active' : ''} onClick={() => { setKind('limit'); if (!limit) setLimit(String(quote.price)) }}>Limit</button>
      </div>
      <label>
        Quantity {step < 1 && <small>(min {fmtQty(step)})</small>}
        <input type="number" min={step} step={step} value={qty} onChange={(e) => setQty(e.target.value)} required />
      </label>
      {kind === 'limit' && (
        <label>
          Limit price
          <input type="number" min={0} step="any" value={limit} onChange={(e) => setLimit(e.target.value)} required />
        </label>
      )}
      <dl className="ticket-facts">
        <dt>Market price</dt><dd>{price(quote.price)}</dd>
        <dt>Estimated {side === 'buy' ? 'cost' : 'proceeds'}</dt><dd>{money(estimate)}</dd>
        {side === 'buy'
          ? <><dt>Buying power</dt><dd>{money(buyingPower)} <button type="button" className="link" onClick={() => setQty(maxBuy)}>max {fmtQty(Number(maxBuy))}</button></dd></>
          : <><dt>Available</dt><dd>{fmtQty(held)} <button type="button" className="link" onClick={() => setQty(floorToStep(held, step))}>all</button></dd></>}
      </dl>
      <button className={`primary ${side}`} disabled={busy || !validQty}>
        {busy ? 'Submitting…' : `${side === 'buy' ? 'Buy' : 'Sell'} ${validQty ? fmtQty(q) : ''} ${quote.symbol}`}
      </button>
      {msg && <p className={msg.ok ? 'notice ok' : 'notice err'} role="status">{msg.text}</p>}
    </form>
  )
}
