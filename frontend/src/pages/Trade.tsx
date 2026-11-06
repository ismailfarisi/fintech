import { useEffect, useState } from 'react'
import { api, type Order, type PricePoint } from '../api'
import { LineChart } from '../components/LineChart'
import { OrderTicket } from '../components/OrderTicket'
import { money, pct, signedMoney, time, tone } from '../format'
import { useLive } from '../live'

export function Trade({ symbol, go }: { symbol?: string; go: (path: string) => void }) {
  const { quotes, portfolio, revision } = useLive()
  const [watchlist, setWatchlist] = useState<string[]>([])
  const [history, setHistory] = useState<PricePoint[]>([])
  const [openOrders, setOpenOrders] = useState<Order[]>([])
  const [filter, setFilter] = useState('')

  const selected = symbol ?? quotes[0]?.symbol
  const quote = quotes.find((q) => q.symbol === selected)

  useEffect(() => { api.watchlist().then(setWatchlist).catch(() => {}) }, [])
  useEffect(() => { api.orders('open').then(setOpenOrders).catch(() => {}) }, [revision])

  // Load history once per symbol, then extend it from the live tick stream.
  useEffect(() => {
    if (!selected) return
    api.quote(selected).then((d) => setHistory(d.history)).catch(() => setHistory([]))
  }, [selected])
  useEffect(() => {
    if (!quote) return
    setHistory((h) => (h.length && h[h.length - 1].t === quote.updated_at ? h : [...h.slice(-599), { t: quote.updated_at, price: quote.price }]))
  }, [quote])

  const toggleWatch = async (s: string) => {
    setWatchlist(watchlist.includes(s) ? await api.unwatch(s) : await api.watch(s))
  }

  const position = portfolio?.positions.find((p) => p.symbol === selected)
  const shown = quotes
    .filter((q) => !filter || q.symbol.includes(filter.toUpperCase()) || q.name.toLowerCase().includes(filter.toLowerCase()))
    .sort((a, b) => Number(watchlist.includes(b.symbol)) - Number(watchlist.includes(a.symbol)))

  return (
    <div className="page trade-layout">
      <aside className="card market-list">
        <input className="search" placeholder="Search symbols…" value={filter} onChange={(e) => setFilter(e.target.value)} />
        <ul>
          {shown.map((q) => (
            <li key={q.symbol} className={q.symbol === selected ? 'selected' : ''} onClick={() => go(`/trade/${q.symbol}`)}>
              <button
                className={`star ${watchlist.includes(q.symbol) ? 'on' : ''}`}
                aria-label={watchlist.includes(q.symbol) ? `Remove ${q.symbol} from watchlist` : `Add ${q.symbol} to watchlist`}
                onClick={(e) => { e.stopPropagation(); toggleWatch(q.symbol) }}
              >★</button>
              <div className="ml-name"><strong>{q.symbol}</strong><small>{q.name}</small></div>
              <div className="ml-price"><span>{money(q.price)}</span><small className={tone(q.change_pct)}>{pct(q.change_pct)}</small></div>
            </li>
          ))}
        </ul>
      </aside>

      {quote ? (
        <>
          <section className="card instrument">
            <header className="instrument-head">
              <div>
                <h1>{quote.symbol} <small>{quote.name} · {quote.sector}</small></h1>
                <div className="instrument-price">
                  {money(quote.price)}{' '}
                  <span className={tone(quote.change)}>{signedMoney(quote.change)} ({pct(quote.change_pct)})</span>
                </div>
              </div>
              <dl className="ohlc">
                <dt>Open</dt><dd>{money(quote.open)}</dd>
                <dt>High</dt><dd>{money(quote.high)}</dd>
                <dt>Low</dt><dd>{money(quote.low)}</dd>
              </dl>
            </header>
            <LineChart
              ariaLabel={`${quote.symbol} price`}
              points={history.map((h) => ({ t: h.t, v: h.price }))}
              format={money}
              reference={{ value: quote.open, label: 'Open' }}
              height={300}
            />
            {position && (
              <div className="position-strip">
                You hold <strong>{position.qty}</strong> @ {money(position.avg_cost)} · value {money(position.market_value)} ·{' '}
                <span className={tone(position.unrealized_pnl)}>{signedMoney(position.unrealized_pnl)} ({pct(position.unrealized_pnl_pct)})</span>
              </div>
            )}
          </section>
          <div className="side-col">
            <OrderTicket key={quote.symbol} quote={quote} />
            <section className="card">
              <h2>Open orders</h2>
              {openOrders.length === 0 ? <div className="empty">No open orders.</div> : (
                <ul className="order-list">
                  {openOrders.map((o) => (
                    <li key={o.id}>
                      <span className={`pill ${o.side}`}>{o.side}</span>
                      <span>{o.qty} {o.symbol} @ {money(o.limit_price ?? 0)}</span>
                      <small>{time(o.created_at)}</small>
                      <button className="link" onClick={async () => { await api.cancelOrder(o.id); setOpenOrders((xs) => xs.filter((x) => x.id !== o.id)) }}>Cancel</button>
                    </li>
                  ))}
                </ul>
              )}
            </section>
          </div>
        </>
      ) : (
        <div className="loading">Loading market…</div>
      )}
    </div>
  )
}
