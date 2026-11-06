import { useEffect, useState } from 'react'
import { money, price, qty } from './format'
import { LiveProvider, useLive } from './live'
import { Activity } from './pages/Activity'
import { Dashboard } from './pages/Dashboard'
import { Trade } from './pages/Trade'

// Tiny hash router: #/  #/trade  #/trade/NOVA  #/activity
function useRoute() {
  const read = () => location.hash.replace(/^#/, '') || '/'
  const [path, setPath] = useState(read)
  useEffect(() => {
    const on = () => setPath(read())
    window.addEventListener('hashchange', on)
    return () => window.removeEventListener('hashchange', on)
  }, [])
  return [path, (p: string) => { location.hash = p }] as const
}

function Toast() {
  const { lastFill } = useLive()
  if (!lastFill) return null
  // Keyed by trade id so each fill restarts the CSS fade-out animation.
  return (
    <div key={lastFill.id} className="toast" role="status">
      ✓ {lastFill.side === 'buy' ? 'Bought' : 'Sold'} {qty(lastFill.qty)} {lastFill.symbol} @ {price(lastFill.price)}
    </div>
  )
}

function Shell() {
  const [path, go] = useRoute()
  const { connected, portfolio, providers } = useLive()
  const down = providers.filter((p) => p.state !== 'live')
  const [, section, arg] = path.split('/')

  return (
    <>
      <header className="topbar">
        <div className="brand">◆ <span className="brand-text">Ledger</span></div>
        <nav>
          <a href="#/" className={!section ? 'active' : ''}>Portfolio</a>
          <a href="#/trade" className={section === 'trade' ? 'active' : ''}>Trade</a>
          <a href="#/activity" className={section === 'activity' ? 'active' : ''}>Activity</a>
        </nav>
        <div className="topbar-right">
          {portfolio && <span className="muted">{money(portfolio.equity)}</span>}
          <a
            href="#/activity"
            className={`conn ${!connected ? 'off' : down.length ? 'warn' : 'on'}`}
            title={providers.map((p) => `${p.id}: ${p.state}${p.last_error ? ` (${p.last_error})` : ''}`).join('\n')}
          >
            {!connected ? '○ Reconnecting' : down.length ? `◐ ${down.length} feed${down.length > 1 ? 's' : ''} down` : '● Live'}
          </a>
        </div>
      </header>
      <main>
        {section === 'trade' ? <Trade symbol={arg?.toUpperCase()} go={go} />
          : section === 'activity' ? <Activity />
          : <Dashboard go={go} />}
      </main>
      <Toast />
    </>
  )
}

export default function App() {
  return <LiveProvider><Shell /></LiveProvider>
}
