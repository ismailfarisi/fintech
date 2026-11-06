import { useState } from 'react'
import { api, ApiError, type OrderKind, type Quote, type Side } from '../api'
import { money } from '../format'
import { useLive } from '../live'

export function OrderTicket({ quote }: { quote: Quote }) {
  const { portfolio, refresh } = useLive()
  const [side, setSide] = useState<Side>('buy')
  const [kind, setKind] = useState<OrderKind>('market')
  const [qty, setQty] = useState('1')
  const [limit, setLimit] = useState('')
  const [busy, setBusy] = useState(false)
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null)

  const q = Math.floor(Number(qty))
  const px = kind === 'limit' ? Number(limit) : quote.price
  const estimate = q > 0 && px > 0 ? q * px : 0
  const held = portfolio?.positions.find((p) => p.symbol === quote.symbol)?.qty ?? 0
  const buyingPower = portfolio?.buying_power ?? 0
  const maxBuy = px > 0 ? Math.floor(buyingPower / px) : 0

  const submit = async (e: React.FormEvent) => {
    e.preventDefault()
    setBusy(true)
    setMsg(null)
    try {
      const { order, trade } = await api.placeOrder({
        symbol: quote.symbol, side, kind, qty: q,
        limit_price: kind === 'limit' ? Number(limit) : undefined,
      })
      setMsg({
        ok: true,
        text: trade
          ? `Filled: ${side} ${trade.qty} ${trade.symbol} @ ${money(trade.price)}`
          : `Limit order placed: ${side} ${order.qty} ${order.symbol} @ ${money(order.limit_price!)}`,
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
        <button type="button" className={kind === 'limit' ? 'active' : ''} onClick={() => { setKind('limit'); if (!limit) setLimit(quote.price.toFixed(2)) }}>Limit</button>
      </div>
      <label>
        Quantity
        <input type="number" min={1} step={1} value={qty} onChange={(e) => setQty(e.target.value)} required />
      </label>
      {kind === 'limit' && (
        <label>
          Limit price
          <input type="number" min={0.01} step={0.01} value={limit} onChange={(e) => setLimit(e.target.value)} required />
        </label>
      )}
      <dl className="ticket-facts">
        <dt>Market price</dt><dd>{money(quote.price)}</dd>
        <dt>Estimated {side === 'buy' ? 'cost' : 'proceeds'}</dt><dd>{money(estimate)}</dd>
        {side === 'buy'
          ? <><dt>Buying power</dt><dd>{money(buyingPower)} <button type="button" className="link" onClick={() => setQty(String(maxBuy))}>max {maxBuy}</button></dd></>
          : <><dt>Shares held</dt><dd>{held} <button type="button" className="link" onClick={() => setQty(String(held))}>all</button></dd></>}
      </dl>
      <button className={`primary ${side}`} disabled={busy || !(q > 0)}>
        {busy ? 'Submitting…' : `${side === 'buy' ? 'Buy' : 'Sell'} ${q > 0 ? q : ''} ${quote.symbol}`}
      </button>
      {msg && <p className={msg.ok ? 'notice ok' : 'notice err'} role="status">{msg.text}</p>}
    </form>
  )
}
