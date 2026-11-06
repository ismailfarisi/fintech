import { useEffect, useState } from 'react'

/** Current time in ms, refreshed every `everyMs` (for "5s ago" style labels). */
export function useNow(everyMs = 1000) {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), everyMs)
    return () => clearInterval(id)
  }, [everyMs])
  return now
}
