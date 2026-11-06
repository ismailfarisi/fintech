import { useState } from 'react'
import { api, ApiError } from '../api'
import { useAuth } from '../auth'

export function SignIn() {
  const { signIn } = useAuth()
  const [mode, setMode] = useState<'login' | 'register'>('login')
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const [name, setName] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const submit = async (e: React.FormEvent) => {
    e.preventDefault()
    setBusy(true)
    setError(null)
    try {
      const me = mode === 'login' ? await api.login(email, password) : await api.register(email, password, name)
      setPassword('')
      signIn(me)
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Could not reach the server')
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="signin">
      <form className="card signin-card" onSubmit={submit}>
        <div className="brand signin-brand">◆ Ledger</div>
        <h1>{mode === 'login' ? 'Sign in' : 'Create your account'}</h1>
        {mode === 'register' && (
          <label>
            Name
            <input value={name} onChange={(e) => setName(e.target.value)} required maxLength={100} autoComplete="name" />
          </label>
        )}
        <label>
          Email
          <input type="email" value={email} onChange={(e) => setEmail(e.target.value)} required autoComplete="email" />
        </label>
        <label>
          Password
          <input
            type="password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            required
            minLength={mode === 'register' ? 12 : undefined}
            maxLength={128}
            autoComplete={mode === 'login' ? 'current-password' : 'new-password'}
          />
          {mode === 'register' && <small>At least 12 characters. A passphrase of a few words works well.</small>}
        </label>
        {error && <p className="notice err" role="alert">{error}</p>}
        <button className="primary buy" disabled={busy}>
          {busy ? 'Please wait…' : mode === 'login' ? 'Sign in' : 'Create account'}
        </button>
        <p className="muted signin-switch">
          {mode === 'login' ? 'New here?' : 'Already have an account?'}{' '}
          <button type="button" className="link" onClick={() => { setMode(mode === 'login' ? 'register' : 'login'); setError(null) }}>
            {mode === 'login' ? 'Create an account' : 'Sign in'}
          </button>
        </p>
        {mode === 'register' && <p className="small-print muted">New accounts start with $100,000 of paper money.</p>}
      </form>
    </div>
  )
}
