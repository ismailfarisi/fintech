import { useEffect, useState } from 'react'
import { api, type EquityPoint } from '../api'
import { LineChart } from '../components/LineChart'
import { Stat } from '../components/Stat'
import { money, moneyCompact, pct, price, qty, signedMoney, tone } from '../format'
import { useLive } from '../live'

export function Dashboard({ go }: { go: (path: string) => void }) {
  const { portfolio: p, revision } = useLive()
  const [history, setHistory] = useState<EquityPoint[]>([])

  // Poll equity history every 5s; the tick stream carries the latest value.
  useEffect(() => {
    const load = () => api.equityHistory().then(setHistory).catch(() => {})
    load()
    const id = setInterval(load, 5000)
    return () => clearInterval(id)
  }, [revision])

  if (!p) return <div className="loading">Loading portfolio…</div>

  const allocation = [
    ...p.positions.map((x) => ({ label: x.symbol, value: x.market_value, weight: x.weight })),
    { label: 'Cash', value: p.cash, weight: p.equity > 0 ? (p.cash / p.equity) * 100 : 0 },
  ]

  return (
    <div className="page">
      <section className="hero">
        <div className="hero-label">Total equity</div>
        <div className="hero-value">{money(p.equity)}</div>
        <div className={`hero-delta ${tone(p.total_pnl)}`}>
          {signedMoney(p.total_pnl)} ({pct(p.total_pnl_pct)}) all time
        </div>
      </section>

      <div className="stats">
        <Stat label="Cash" value={money(p.cash)} sub={`${money(p.buying_power)} buying power`} />
        <Stat label="Invested" value={money(p.positions_value)} sub={`${p.positions.length} positions`} />
        <Stat label="Unrealized P&L" value={signedMoney(p.unrealized_pnl)} delta={{ n: p.unrealized_pnl, text: p.unrealized_pnl >= 0 ? 'open gains' : 'open losses' }} />
        <Stat label="Realized P&L" value={signedMoney(p.realized_pnl)} delta={{ n: p.realized_pnl, text: 'from closed trades' }} />
      </div>

      <section className="card">
        <h2>Equity</h2>
        <LineChart
          ariaLabel="Account equity over time"
          points={history.map((h) => ({ t: h.t, v: h.equity }))}
          format={moneyCompact}
          reference={{ value: p.net_contributions, label: `Invested ${moneyCompact(p.net_contributions)}` }}
        />
      </section>

      <div className="grid-2">
        <section className="card">
          <h2>Holdings</h2>
          {p.positions.length === 0 ? (
            <div className="empty">
              No positions yet. <button className="link" onClick={() => go('/trade')}>Place your first trade →</button>
            </div>
          ) : (
            <div className="table-wrap"><table className="table">
              <thead>
                <tr><th>Symbol</th><th className="num">Qty</th><th className="num">Avg cost</th><th className="num">Price</th><th className="num">Value</th><th className="num">P&L</th></tr>
              </thead>
              <tbody>
                {p.positions.map((x) => (
                  <tr key={x.symbol} className="clickable" onClick={() => go(`/trade/${x.symbol}`)}>
                    <td><strong>{x.symbol}</strong></td>
                    <td className="num">{qty(x.qty)}</td>
                    <td className="num">{price(x.avg_cost)}</td>
                    <td className="num">{price(x.price)}</td>
                    <td className="num">{money(x.market_value)}</td>
                    <td className={`num ${tone(x.unrealized_pnl)}`}>
                      {signedMoney(x.unrealized_pnl)}<br /><small>{pct(x.unrealized_pnl_pct)}</small>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table></div>
          )}
        </section>

        <section className="card">
          <h2>Allocation</h2>
          <ul className="bars">
            {allocation.map((a) => (
              <li key={a.label} title={`${a.label}: ${money(a.value)}`}>
                <span className="bar-label" title={a.label}>{a.label}</span>
                <span className="bar-track"><span className={`bar-fill ${a.label === 'Cash' ? 'cash' : ''}`} style={{ width: `${Math.max(a.weight, 0.5)}%` }} /></span>
                <span className="bar-value">{a.weight.toFixed(1)}%</span>
              </li>
            ))}
          </ul>
        </section>
      </div>
    </div>
  )
}
