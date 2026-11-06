import { useEffect, useState } from 'react'
import { useAuth, AuthProvider } from './auth'
import { money, price, qty } from './format'
import { LiveProvider, useLive } from './live'
import { Activity } from './pages/Activity'
import { Dashboard } from './pages/Dashboard'
import { Settings } from './pages/Settings'
import { SignIn } from './pages/SignIn'
import { Trade } from './pages/Trade'

// Tiny hash router: #/  #/trade  #/trade/NOVA  #/activity  #/settings
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
  // Keyed by order id so each fill restarts the CSS fade-out animation.
  return (
    <div key={lastFill.order_id} className="toast" role="status">
      ✓ {lastFill.side === 'buy' ? 'Bought' : 'Sold'} {qty(lastFill.quantity)} {lastFill.symbol} @ {price(lastFill.price)}
    </div>
  )
}

function Shell() {
  const [path, go] = useRoute()
  const { me, signOut } = useAuth()
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
          {portfolio && <span className="muted hide-sm">{money(portfolio.equity)}</span>}
          <a
            href="#/activity"
            className={`conn ${!connected ? 'off' : down.length ? 'warn' : 'on'}`}
            title={providers.map((p) => `${p.id}: ${p.state}`).join('\n')}
          >
            {!connected ? '○ Reconnecting' : down.length ? `◐ ${down.length} feed${down.length > 1 ? 's' : ''} down` : '● Live'}
          </a>
          <a href="#/settings" className={`user ${section === 'settings' ? 'active' : ''}`} title={me?.user.email}>
            {me?.user.display_name}
          </a>
          <button className="link" onClick={signOut}>Sign out</button>
        </div>
      </header>
      <main>
        {section === 'trade' ? <Trade symbol={arg?.toUpperCase()} go={go} />
          : section === 'activity' ? <Activity />
          : section === 'settings' ? <Settings />
          : <Dashboard go={go} />}
      </main>
      <Toast />
    </>
  )
}

function Gate() {
  const { loading, me, account } = useAuth()
  if (loading) return <div className="loading">Loading…</div>
  if (!me) return <SignIn />
  if (!account) return <div className="loading">No open account.</div>
  // Remount live state when the account changes (e.g. after a reset).
  return (
    <LiveProvider key={account.id} accountId={account.id}>
      <Shell />
    </LiveProvider>
  )
}

export default function App() {
  return (
    <AuthProvider>
      <Gate />
    </AuthProvider>
  )
}
