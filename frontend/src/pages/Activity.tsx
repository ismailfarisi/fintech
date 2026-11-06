import { useEffect, useState } from 'react'
import { api, ApiError, type CashFlow, type Order, type Trade } from '../api'
import { dateTime, money, signedMoney, tone } from '../format'
import { useLive } from '../live'

export function Activity() {
  const { revision, refresh, portfolio } = useLive()
  const [trades, setTrades] = useState<Trade[]>([])
  const [orders, setOrders] = useState<Order[]>([])
  const [flows, setFlows] = useState<CashFlow[]>([])
  const [tab, setTab] = useState<'trades' | 'orders' | 'cash'>('trades')
  const [amount, setAmount] = useState('1000')
  const [msg, setMsg] = useState<string | null>(null)

  useEffect(() => {
    api.trades().then(setTrades).catch(() => {})
    api.orders().then(setOrders).catch(() => {})
    api.cashFlows().then(setFlows).catch(() => {})
  }, [revision])

  const run = async (fn: () => Promise<unknown>) => {
    setMsg(null)
    try { await fn(); refresh() } catch (e) { setMsg(e instanceof ApiError ? e.message : 'Request failed') }
  }

  const reset = () => {
    if (confirm('Reset the account? This deletes all positions, orders and history.')) {
      run(() => api.reset(100_000))
    }
  }

  return (
    <div className="page">
      <div className="grid-2 activity-top">
        <section className="card">
          <h2>Cash</h2>
          <p className="muted">Available to withdraw: {money(portfolio?.buying_power ?? 0)}</p>
          <div className="row">
            <input type="number" min={0.01} step={0.01} value={amount} onChange={(e) => setAmount(e.target.value)} aria-label="Amount" />
            <button onClick={() => run(() => api.deposit(Number(amount)))}>Deposit</button>
            <button onClick={() => run(() => api.withdraw(Number(amount)))}>Withdraw</button>
          </div>
          {msg && <p className="notice err">{msg}</p>}
        </section>
        <section className="card">
          <h2>Account</h2>
          <p className="muted">Paper trading on a simulated market. Start over with $100,000.</p>
          <button className="danger" onClick={reset}>Reset account</button>
        </section>
      </div>

      <section className="card">
        <div className="tabs">
          <button className={tab === 'trades' ? 'active' : ''} onClick={() => setTab('trades')}>Trades ({trades.length})</button>
          <button className={tab === 'orders' ? 'active' : ''} onClick={() => setTab('orders')}>Orders ({orders.length})</button>
          <button className={tab === 'cash' ? 'active' : ''} onClick={() => setTab('cash')}>Cash ({flows.length})</button>
        </div>

        {tab === 'trades' && (trades.length === 0 ? <div className="empty">No trades yet.</div> : (
          <div className="table-wrap"><table className="table">
            <thead><tr><th>Time</th><th>Side</th><th>Symbol</th><th className="num">Qty</th><th className="num">Price</th><th className="num">Total</th><th className="num">Realized</th></tr></thead>
            <tbody>
              {trades.map((t) => (
                <tr key={t.id}>
                  <td>{dateTime(t.at)}</td>
                  <td><span className={`pill ${t.side}`}>{t.side}</span></td>
                  <td><strong>{t.symbol}</strong></td>
                  <td className="num">{t.qty}</td>
                  <td className="num">{money(t.price)}</td>
                  <td className="num">{money(t.total)}</td>
                  <td className={`num ${t.realized_pnl === null ? '' : tone(t.realized_pnl)}`}>{t.realized_pnl === null ? '—' : signedMoney(t.realized_pnl)}</td>
                </tr>
              ))}
            </tbody>
          </table></div>
        ))}

        {tab === 'orders' && (orders.length === 0 ? <div className="empty">No orders yet.</div> : (
          <div className="table-wrap"><table className="table">
            <thead><tr><th>Created</th><th>Side</th><th>Symbol</th><th>Type</th><th className="num">Qty</th><th className="num">Limit</th><th className="num">Fill</th><th>Status</th></tr></thead>
            <tbody>
              {orders.map((o) => (
                <tr key={o.id}>
                  <td>{dateTime(o.created_at)}</td>
                  <td><span className={`pill ${o.side}`}>{o.side}</span></td>
                  <td><strong>{o.symbol}</strong></td>
                  <td>{o.kind}</td>
                  <td className="num">{o.qty}</td>
                  <td className="num">{o.limit_price === null ? '—' : money(o.limit_price)}</td>
                  <td className="num">{o.fill_price === null ? '—' : money(o.fill_price)}</td>
                  <td><span className={`status ${o.status}`}>{o.status}</span></td>
                </tr>
              ))}
            </tbody>
          </table></div>
        ))}

        {tab === 'cash' && (flows.length === 0 ? <div className="empty">No cash movements.</div> : (
          <div className="table-wrap"><table className="table">
            <thead><tr><th>Time</th><th>Type</th><th className="num">Amount</th></tr></thead>
            <tbody>
              {flows.map((f) => (
                <tr key={f.id}>
                  <td>{dateTime(f.at)}</td>
                  <td>{f.kind}</td>
                  <td className={`num ${f.kind === 'deposit' ? 'up' : 'down'}`}>{signedMoney(f.kind === 'deposit' ? f.amount : -f.amount)}</td>
                </tr>
              ))}
            </tbody>
          </table></div>
        ))}
      </section>
    </div>
  )
}
