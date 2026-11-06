import { useEffect, useMemo, useRef, useState } from 'react'

export interface Point { t: string; v: number }

interface Props {
  points: Point[]
  height?: number
  format: (n: number) => string
  /** Optional dashed reference line (e.g. open price, money invested). */
  reference?: { value: number; label: string }
  ariaLabel: string
}

const PAD = { top: 12, right: 12, bottom: 24, left: 64 }

/** Single-series line chart with recessive grid, crosshair and tooltip. */
export function LineChart({ points, height = 260, format, reference, ariaLabel }: Props) {
  const wrap = useRef<HTMLDivElement>(null)
  const [width, setWidth] = useState(600)
  const [hover, setHover] = useState<number | null>(null)

  useEffect(() => {
    const el = wrap.current
    if (!el) return
    const ro = new ResizeObserver(([e]) => setWidth(Math.max(240, e.contentRect.width)))
    ro.observe(el)
    return () => ro.disconnect()
  }, [])

  const geo = useMemo(() => {
    if (points.length < 2) return null
    const vals = points.map((p) => p.v)
    if (reference) vals.push(reference.value)
    let lo = Math.min(...vals)
    let hi = Math.max(...vals)
    if (hi - lo < 1e-9) { lo -= 1; hi += 1 }
    const span = hi - lo
    lo -= span * 0.08
    hi += span * 0.08
    const iw = width - PAD.left - PAD.right
    const ih = height - PAD.top - PAD.bottom
    const x = (i: number) => PAD.left + (i / (points.length - 1)) * iw
    const y = (v: number) => PAD.top + (1 - (v - lo) / (hi - lo)) * ih
    const d = points.map((p, i) => `${i ? 'L' : 'M'}${x(i).toFixed(1)},${y(p.v).toFixed(1)}`).join('')
    const ticks = Array.from({ length: 4 }, (_, k) => lo + ((hi - lo) * (k + 0.5)) / 4)
    return { x, y, d, ticks, iw }
  }, [points, width, height, reference])

  if (!geo) {
    return <div ref={wrap} className="chart-empty" style={{ height }}>Collecting data…</div>
  }

  const onMove = (e: React.PointerEvent<SVGRectElement>) => {
    const rect = e.currentTarget.getBoundingClientRect()
    const frac = (e.clientX - rect.left) / rect.width
    setHover(Math.round(Math.min(1, Math.max(0, frac)) * (points.length - 1)))
  }

  const h = hover !== null ? points[hover] : null
  const hx = hover !== null ? geo.x(hover) : 0
  const first = points[0]
  const last = points[points.length - 1]

  return (
    <div ref={wrap} className="chart" style={{ height }}>
      <svg width={width} height={height} role="img" aria-label={ariaLabel}>
        {geo.ticks.map((t) => (
          <g key={t}>
            <line className="grid" x1={PAD.left} x2={width - PAD.right} y1={geo.y(t)} y2={geo.y(t)} />
            <text className="tick" x={PAD.left - 8} y={geo.y(t)} dy="0.32em" textAnchor="end">{format(t)}</text>
          </g>
        ))}
        <text className="tick" x={PAD.left} y={height - 6}>{new Date(first.t).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}</text>
        <text className="tick" x={width - PAD.right} y={height - 6} textAnchor="end">{new Date(last.t).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}</text>
        {reference && (
          <g>
            <line className="ref" x1={PAD.left} x2={width - PAD.right} y1={geo.y(reference.value)} y2={geo.y(reference.value)} />
            <text className="tick" x={width - PAD.right} y={geo.y(reference.value) - 5} textAnchor="end">{reference.label}</text>
          </g>
        )}
        <path className="series" d={geo.d} />
        {h && (
          <g>
            <line className="crosshair" x1={hx} x2={hx} y1={PAD.top} y2={height - PAD.bottom} />
            <circle className="marker" cx={hx} cy={geo.y(h.v)} r={4.5} />
          </g>
        )}
        <rect
          x={PAD.left} y={PAD.top} width={geo.iw} height={height - PAD.top - PAD.bottom}
          fill="transparent" onPointerMove={onMove} onPointerLeave={() => setHover(null)}
        />
      </svg>
      {h && (
        <div className="tooltip" style={{ left: Math.min(hx + 12, width - 150), top: PAD.top }}>
          <div className="tooltip-value">{format(h.v)}</div>
          <div className="tooltip-label">{new Date(h.t).toLocaleTimeString()}</div>
        </div>
      )}
    </div>
  )
}
