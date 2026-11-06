import { tone } from '../format'

export function Stat({ label, value, delta, sub }: { label: string; value: string; delta?: { n: number; text: string }; sub?: string }) {
  return (
    <div className="card stat">
      <div className="stat-label">{label}</div>
      <div className="stat-value">{value}</div>
      {delta && <div className={`stat-delta ${tone(delta.n)}`}>{delta.text}</div>}
      {sub && <div className="stat-sub">{sub}</div>}
    </div>
  )
}
