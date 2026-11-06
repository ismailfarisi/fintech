import { useEffect, useState } from 'react'
import { api, ApiError, type AuditEntry, type SessionInfo } from '../api'
import { useAuth } from '../auth'
import { dateTime } from '../format'

const ACTION_LABELS: Record<string, string> = {
  'user.registered': 'Account created',
  'auth.login': 'Signed in',
  'auth.logout': 'Signed out',
  'auth.login_failed': 'Failed sign-in attempt',
  'auth.login_blocked': 'Sign-in blocked (too many attempts)',
  'auth.password_changed': 'Password changed',
  'auth.password_change_failed': 'Failed password change',
  'auth.session_revoked': 'Session signed out',
  'auth.sessions_revoked': 'Other sessions signed out',
  'account.opened': 'Account opened',
  'account.closed': 'Account closed',
  'cash.deposit': 'Deposit',
  'cash.withdrawal': 'Withdrawal',
  'order.placed': 'Order placed',
  'order.cancelled': 'Order cancelled',
  'order.filled': 'Order filled',
}

/** Rough device label from a user agent, for the sessions list. */
function device(ua: string | null) {
  if (!ua) return 'Unknown device'
  const browser = /Firefox\//.test(ua) ? 'Firefox' : /Edg\//.test(ua) ? 'Edge' : /Chrome\//.test(ua) ? 'Chrome' : /Safari\//.test(ua) ? 'Safari' : 'Browser'
  const os = /Windows/.test(ua) ? 'Windows' : /Mac OS X/.test(ua) ? 'macOS' : /Android/.test(ua) ? 'Android' : /iPhone|iPad/.test(ua) ? 'iOS' : /Linux/.test(ua) ? 'Linux' : ''
  return os ? `${browser} on ${os}` : browser
}

export function Settings() {
  const { me } = useAuth()
  const [sessions, setSessions] = useState<SessionInfo[]>([])
  const [log, setLog] = useState<AuditEntry[]>([])
  const [current, setCurrent] = useState('')
  const [next, setNext] = useState('')
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null)
  const [busy, setBusy] = useState(false)

  const load = () => {
    api.sessions().then(setSessions).catch(() => {})
    api.securityLog().then(setLog).catch(() => {})
  }
  useEffect(load, [])

  const changePassword = async (e: React.FormEvent) => {
    e.preventDefault()
    setBusy(true)
    setMsg(null)
    try {
      const r = await api.changePassword(current, next)
      setMsg({ ok: true, text: `Password changed. ${r.other_sessions_revoked} other session(s) were signed out.` })
      setCurrent('')
      setNext('')
      load()
    } catch (err) {
      setMsg({ ok: false, text: err instanceof ApiError ? err.message : 'Could not change password' })
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="page">
      <div className="grid-2 activity-top">
        <section className="card">
          <h2>Profile</h2>
          <dl className="ticket-facts">
            <dt>Name</dt><dd>{me?.user.display_name}</dd>
            <dt>Email</dt><dd>{me?.user.email}</dd>
            <dt>Member since</dt><dd>{me && dateTime(me.user.created_at)}</dd>
          </dl>
        </section>
        <form className="card ticket" onSubmit={changePassword}>
          <h2>Change password</h2>
          <label>
            Current password
            <input type="password" value={current} onChange={(e) => setCurrent(e.target.value)} required autoComplete="current-password" />
          </label>
          <label>
            New password <small>(at least 12 characters)</small>
            <input type="password" value={next} onChange={(e) => setNext(e.target.value)} required minLength={12} maxLength={128} autoComplete="new-password" />
          </label>
          <button disabled={busy}>{busy ? 'Saving…' : 'Change password'}</button>
          {msg && <p className={msg.ok ? 'notice ok' : 'notice err'} role="status">{msg.text}</p>}
        </form>
      </div>

      <section className="card">
        <div className="row-between">
          <h2>Active sessions</h2>
          {sessions.length > 1 && (
            <button className="link" onClick={async () => { await api.revokeOtherSessions(); load() }}>Sign out all other sessions</button>
          )}
        </div>
        <ul className="sources">
          {sessions.map((s) => (
            <li key={s.id}>
              <strong>{device(s.user_agent)}</strong>
              <small>{s.ip ?? 'unknown IP'} · last active {dateTime(s.last_seen_at)} · signed in {dateTime(s.created_at)}</small>
              {s.current ? <span className="pill">This device</span> : (
                <button className="link" onClick={async () => { await api.revokeSession(s.id); load() }}>Sign out</button>
              )}
            </li>
          ))}
        </ul>
      </section>

      <section className="card">
        <h2>Security & account activity</h2>
        <div className="table-wrap"><table className="table">
          <thead><tr><th>When</th><th>Event</th><th>IP</th></tr></thead>
          <tbody>
            {log.map((e, i) => (
              <tr key={i}>
                <td>{dateTime(e.at)}</td>
                <td className={e.action.includes('failed') || e.action.includes('blocked') ? 'down' : ''}>{ACTION_LABELS[e.action] ?? e.action}</td>
                <td>{e.ip ?? '—'}</td>
              </tr>
            ))}
          </tbody>
        </table></div>
      </section>
    </div>
  )
}
