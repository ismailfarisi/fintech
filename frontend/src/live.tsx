import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from 'react'
import { api, type LiveEvent, type Portfolio, type Quote, type Trade } from './api'

interface LiveState {
  quotes: Quote[]
  portfolio: Portfolio | null
  connected: boolean
  /** Bumps whenever orders/trades/cash change, so views can refetch. */
  revision: number
  lastFill: Trade | null
  refresh: () => void
}

const LiveContext = createContext<LiveState | null>(null)

export function LiveProvider({ children }: { children: ReactNode }) {
  const [quotes, setQuotes] = useState<Quote[]>([])
  const [portfolio, setPortfolio] = useState<Portfolio | null>(null)
  const [connected, setConnected] = useState(false)
  const [revision, setRevision] = useState(0)
  const [lastFill, setLastFill] = useState<Trade | null>(null)
  const retry = useRef(0)

  const refresh = () => {
    setRevision((r) => r + 1)
    api.portfolio().then(setPortfolio).catch(() => {})
  }

  useEffect(() => {
    api.quotes().then(setQuotes).catch(() => {})
    api.portfolio().then(setPortfolio).catch(() => {})

    let ws: WebSocket | null = null
    let timer: ReturnType<typeof setTimeout> | undefined
    let closed = false

    const connect = () => {
      const proto = location.protocol === 'https:' ? 'wss' : 'ws'
      ws = new WebSocket(`${proto}://${location.host}/ws`)
      ws.onopen = () => { retry.current = 0; setConnected(true) }
      ws.onmessage = (msg) => {
        const ev = JSON.parse(msg.data) as LiveEvent
        if (ev.type === 'tick') {
          setQuotes(ev.quotes)
          setPortfolio(ev.portfolio)
        } else if (ev.type === 'fill') {
          setLastFill(ev.trade)
          setRevision((r) => r + 1)
        } else {
          setRevision((r) => r + 1)
        }
      }
      ws.onclose = () => {
        setConnected(false)
        if (closed) return
        // Exponential backoff, capped at 10s.
        const delay = Math.min(10_000, 500 * 2 ** retry.current++)
        timer = setTimeout(connect, delay)
      }
    }
    connect()
    return () => { closed = true; clearTimeout(timer); ws?.close() }
  }, [])

  return (
    <LiveContext.Provider value={{ quotes, portfolio, connected, revision, lastFill, refresh }}>
      {children}
    </LiveContext.Provider>
  )
}

export function useLive() {
  const ctx = useContext(LiveContext)
  if (!ctx) throw new Error('useLive must be used inside <LiveProvider>')
  return ctx
}
