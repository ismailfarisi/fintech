import { createContext, useCallback, useContext, useEffect, useState, type ReactNode } from 'react'
import { api, setActiveAccount, setUnauthenticatedHandler, type Account, type Me } from './api'

interface AuthState {
  me: Me | null
  /** The open account the app is working with. */
  account: Account | null
  loading: boolean
  signIn: (me: Me) => void
  signOut: () => Promise<void>
  /** Re-read the user and accounts (e.g. after an account reset). */
  reload: () => Promise<void>
}

const AuthContext = createContext<AuthState | null>(null)

function pickAccount(me: Me | null) {
  return me?.accounts.find((a) => !a.closed_at) ?? null
}

export function AuthProvider({ children }: { children: ReactNode }) {
  const [me, setMe] = useState<Me | null>(null)
  const [loading, setLoading] = useState(true)

  const apply = useCallback((m: Me | null) => {
    setActiveAccount(pickAccount(m)?.id ?? null)
    setMe(m)
  }, [])

  const reload = useCallback(async () => {
    try {
      apply(await api.me())
    } catch {
      apply(null)
    }
  }, [apply])

  useEffect(() => {
    // Any 401 from the API (expired/revoked session) returns to sign-in.
    setUnauthenticatedHandler(() => apply(null))
    reload().finally(() => setLoading(false))
  }, [apply, reload])

  const signOut = async () => {
    try {
      await api.logout()
    } finally {
      apply(null)
    }
  }

  return (
    <AuthContext.Provider value={{ me, account: pickAccount(me), loading, signIn: apply, signOut, reload }}>
      {children}
    </AuthContext.Provider>
  )
}

export function useAuth() {
  const ctx = useContext(AuthContext)
  if (!ctx) throw new Error('useAuth must be used inside <AuthProvider>')
  return ctx
}
