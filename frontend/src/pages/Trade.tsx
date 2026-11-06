import { useEffect, useMemo, useState } from 'react'
import { api, type Order, type PricePoint, type Quote } from '../api'
import { LineChart } from '../components/LineChart'
import { OrderTicket } from '../components/OrderTicket'
import { ago, pct, price, qty, signedMoney, sourceLabel, time, tone } from '../format'
import { useLive } from '../live'
import { useNow } from '../useNow'

/** No trade for this long: the market is probably closed (or the feed is down). */
const STALE_MS = 5 * 60_000

export function Trade({ symbol, go }: { symbol?: string; go: (path: string) => void }) {
  const { quotes, portfolio, revision, providers } = useLive()
  const [watchlist, setWatchlist] = useState<string[]>([])
  const [history, setHistory] = useState<PricePoint[]>([])
  const [openOrders, setOpenOrders] = useState<Order[]>([])
  const [filter, setFilter] = useState('')
  const now = useNow()

  const selected = symbol ?? quotes[0]?.symbol
  const quote = quotes.find((q) => q.symbol === selected)

  useEffect(() => { api.watchlist().then(setWatchlist).catch(() => {}) }, [])
  useEffect(() => { api.orders('open').then(setOpenOrders).catch(() => {}) }, [revision])

  // History comes from the server (bucketed per provider resolution); refresh it
  // periodically and draw the live price as the final point.
  useEffect(() => {
    if (!selected) return
    let cancelled = false
    const load = () => api.quote(selected).then((d) => { if (!cancelled) setHistory(d.history) }).catch(() => {})
    load()
    const id = setInterval(load, 15_000)
    return () => { cancelled = true; clearInterval(id) }
  }, [selected])

  const points = useMemo(() => {
    const pts = history.map((h) => ({ t: h.t, v: h.price }))
    if (quote && pts.length && new Date(quote.updated_at) >= new Date(pts[pts.length - 1].t)) {
      pts[pts.length - 1] = { t: quote.updated_at, v: quote.price }
    }
    return pts
  }, [history, quote])

  const toggleWatch = async (s: string) => {
    setWatchlist(watchlist.includes(s) ? await api.unwatch(s) : await api.watch(s))
  }

  // Group by provider; watchlisted symbols first within each group.
  const groups = useMemo(() => {
    const f = filter.trim().toLowerCase()
    const shown = quotes.filter((q) => !f || q.symbol.toLowerCase().includes(f) || q.name.toLowerCase().includes(f))
    const by = new Map<string, Quote[]>()
    for (const q of shown) by.set(q.source, [...(by.get(q.source) ?? []), q])
    for (const list of by.values()) list.sort((a, b) => Number(watchlist.includes(b.symbol)) - Number(watchlist.includes(a.symbol)))
    return [...by.entries()]
  }, [quotes, filter, watchlist])

  // Providers that are configured but not delivering (e.g. Alpaca with bad keys).
  const unhealthy = providers.filter((p) => p.state !== 'live')

  const position = portfolio?.positions.find((p) => p.symbol === selected)
  const stale = quote && now - new Date(quote.updated_at).getTime() > STALE_MS

  return (
    <div className="page trade-layout">
      <aside className="card market-list">
        <input className="search" placeholder="Search symbols…" value={filter} onChange={(e) => setFilter(e.target.value)} />
        {groups.map(([source, list]) => (
          <div key={source}>
            <div className="group-label">{sourceLabel(source)}</div>
            <ul>
              {list.map((q) => (
                <li key={q.symbol} className={q.symbol === selected ? 'selected' : ''} onClick={() => go(`/trade/${q.symbol}`)}>
                  <button
                    className={`star ${watchlist.includes(q.symbol) ? 'on' : ''}`}
                    aria-label={watchlist.includes(q.symbol) ? `Remove ${q.symbol} from watchlist` : `Add ${q.symbol} to watchlist`}
                    onClick={(e) => { e.stopPropagation(); toggleWatch(q.symbol) }}
                  >★</button>
                  <div className="ml-name"><strong>{q.symbol}</strong><small>{q.name}</small></div>
                  <div className="ml-price"><span>{price(q.price)}</span><small className={tone(q.change_pct)}>{pct(q.change_pct)}</small></div>
                </li>
              ))}
            </ul>
          </div>
        ))}
        {unhealthy.map((p) => (
          <p key={p.id} className="provider-note">
            <strong>{sourceLabel(p.id)}</strong>: {p.state}
          </p>
        ))}
      </aside>

      {quote ? (
        <>
          <section className="card instrument">
            <header className="instrument-head">
              <div>
                <h1>{quote.symbol} <small>{quote.name}</small></h1>
                <div className="instrument-price">
                  {price(quote.price)}{' '}
                  <span className={tone(quote.change)}>{signedMoney(quote.change)} ({pct(quote.change_pct)})</span>
                </div>
                <div className={`freshness ${stale ? 'stale' : ''}`}>
                  {stale ? '⏸ No recent trades (market closed?)' : '● Live'} · last trade {ago(quote.updated_at, now)} · {sourceLabel(quote.source)}
                </div>
              </div>
              <dl className="ohlc">
                <dt>{quote.reference_label}</dt><dd>{price(quote.reference)}</dd>
                <dt>High</dt><dd>{price(quote.high)}</dd>
                <dt>Low</dt><dd>{price(quote.low)}</dd>
              </dl>
            </header>
            <LineChart
              ariaLabel={`${quote.symbol} price`}
              points={points}
              format={price}
              reference={{ value: quote.reference, label: quote.reference_label }}
              height={300}
            />
            {position && (
              <div className="position-strip">
                You hold <strong>{qty(position.qty)}</strong> @ {price(position.avg_cost)} · value {price(position.market_value)} ·{' '}
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
                      <span>{qty(o.qty)} {o.symbol} @ {price(o.limit_price ?? 0)}</span>
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
        <div className="card loading">
          {quotes.length === 0 ? 'Connecting to market data…' : `No price for ${selected} yet.`}
        </div>
      )}
    </div>
  )
}
