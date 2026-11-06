import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from 'react'
import { api, decodePortfolio, type FillNotice, type Portfolio, type ProviderStatus, type Quote } from './api'

interface LiveState {
  quotes: Quote[]
  portfolio: Portfolio | null
  connected: boolean
  /** Market-data provider health, polled from the backend. */
  providers: ProviderStatus[]
  /** Bumps whenever orders/trades/cash change, so views can refetch. */
  revision: number
  lastFill: FillNotice | null
  refresh: () => void
}

const LiveContext = createContext<LiveState | null>(null)

type WsMessage =
  | { type: 'tick'; quotes: Quote[]; portfolio: Record<string, unknown> }
  | { type: 'account'; change: { kind: string; order_id?: string; symbol?: string; side?: 'buy' | 'sell'; quantity?: string; price?: string } }

/** Live quotes + portfolio for one account over an authenticated WebSocket. */
export function LiveProvider({ accountId, children }: { accountId: string; children: ReactNode }) {
  const [quotes, setQuotes] = useState<Quote[]>([])
  const [portfolio, setPortfolio] = useState<Portfolio | null>(null)
  const [connected, setConnected] = useState(false)
  const [providers, setProviders] = useState<ProviderStatus[]>([])
  const [revision, setRevision] = useState(0)
  const [lastFill, setLastFill] = useState<FillNotice | null>(null)
  const retry = useRef(0)

  const refresh = () => {
    setRevision((r) => r + 1)
    api.portfolio().then(setPortfolio).catch(() => {})
  }

  useEffect(() => {
    api.quotes().then(setQuotes).catch(() => {})
    api.portfolio().then(setPortfolio).catch(() => {})
    const loadProviders = () => api.providers().then(setProviders).catch(() => {})
    loadProviders()
    const providerTimer = setInterval(loadProviders, 10_000)

    let ws: WebSocket | null = null
    let timer: ReturnType<typeof setTimeout> | undefined
    let closed = false

    const connect = () => {
      ws = new WebSocket(api.wsUrl())
      ws.onopen = () => {
        retry.current = 0
        setConnected(true)
      }
      ws.onmessage = (msg) => {
        const ev = JSON.parse(msg.data) as WsMessage
        if (ev.type === 'tick') {
          setQuotes(ev.quotes)
          setPortfolio(decodePortfolio(ev.portfolio))
        } else {
          const c = ev.change
          if (c.kind === 'order_filled' && c.order_id && c.symbol && c.side) {
            setLastFill({ order_id: c.order_id, symbol: c.symbol, side: c.side, quantity: Number(c.quantity), price: Number(c.price) })
          }
          setRevision((r) => r + 1)
        }
      }
      ws.onclose = () => {
        setConnected(false)
        if (closed) return
        // Exponential backoff, capped at 10s. If the session ended, the next
        // API call returns 401 and the app shows the sign-in screen.
        const delay = Math.min(10_000, 500 * 2 ** retry.current++)
        timer = setTimeout(() => {
          api.me().then(connect).catch(() => {})
        }, delay)
      }
    }
    connect()
    return () => {
      closed = true
      clearTimeout(timer)
      clearInterval(providerTimer)
      ws?.close()
    }
  }, [accountId])

  return (
    <LiveContext.Provider value={{ quotes, portfolio, connected, providers, revision, lastFill, refresh }}>
      {children}
    </LiveContext.Provider>
  )
}

export function useLive() {
  const ctx = useContext(LiveContext)
  if (!ctx) throw new Error('useLive must be used inside <LiveProvider>')
  return ctx
}
