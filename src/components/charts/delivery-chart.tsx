import * as React from 'react'

import type { Bucket } from '@/lib/tauri/types'
import { cn } from '@/lib/utils'

/**
 * Delivery over time: how much went out each day, and how much of it failed.
 *
 * This is the one genuinely dense time series Windbag has. Every published destination carries an
 * exact `published_at`, so a day is a real observation rather than "whenever someone pressed
 * Refresh" — which is precisely why the engagement numbers are NOT drawn this way. See the module
 * comment in `stats.rs`.
 *
 * Drawn as an SVG by hand rather than with a charting library: the whole thing is two stacked
 * areas, a baseline and a crosshair, and the smallest library that does that arrives with an axis
 * engine, a scale package and a locale table.
 *
 * The series is DENSIFIED before it is drawn. `byDay` only carries days that saw activity, and
 * plotting those points evenly spaced would draw a quiet week as a straight line between two busy
 * days — a chart that lies about the shape of the month.
 */
export function DeliveryChart({
  buckets,
  onDrill,
}: {
  buckets: Bucket[]
  onDrill: (bucket: Bucket) => void
}) {
  const days = React.useMemo(() => densify(buckets), [buckets])
  const [hover, setHover] = React.useState<number | null>(null)

  if (days.length < 2) return null

  const peak = Math.max(...days.map((day) => day.published + day.failed), 1)
  // A viewBox in abstract units: the SVG scales to whatever width the pane gives it, so no
  // ResizeObserver and no re-render on resize.
  const W = 720
  const H = 180
  const x = (index: number) => (index / (days.length - 1)) * W
  const y = (value: number) => H - (value / peak) * H

  const area = (pick: (day: Day) => number, base: (day: Day) => number) =>
    [
      `M ${x(0)} ${y(base(days[0]!))}`,
      ...days.map((day, index) => `L ${x(index)} ${y(base(day) + pick(day))}`),
      ...[...days].reverse().map((day, index) => `L ${x(days.length - 1 - index)} ${y(base(day))}`),
      'Z',
    ].join(' ')

  const line = (pick: (day: Day) => number) =>
    days.map((day, index) => `${index === 0 ? 'M' : 'L'} ${x(index)} ${y(pick(day))}`).join(' ')

  const active = hover === null ? null : days[hover]

  return (
    <section>
      <header className="mb-2 flex items-baseline justify-between gap-2">
        <h2 className="font-display text-xs font-semibold tracking-wide text-muted-foreground uppercase">
          Delivery over time
        </h2>
        {/*
          A legend is present whenever there are two series, so identity is never colour alone.
          Only `published` is stroked: the upper edge of a stacked area is the TOTAL, and drawing
          it in the failure colour made a day carrying one failure read as a day that mostly
          failed — the hierarchy inverted, which a screenshot caught and the arithmetic did not.
        */}
        <ul className="flex items-center gap-3 text-xs text-muted-foreground">
          <li className="flex items-center gap-1.5">
            <span className="size-2 rounded-full bg-primary" aria-hidden />
            Published
          </li>
          <li className="flex items-center gap-1.5">
            <span className="size-2 rounded-full bg-destructive/60" aria-hidden />
            Failed
          </li>
        </ul>
      </header>

      <div className="relative">
        {/*
          A button rather than a bare <svg> with a click handler: the plot is genuinely
          interactive, so it takes focus, answers the arrow keys and fires on Enter — which a
          non-interactive element with a pointer listener never does.
        */}
        <button
          type="button"
          className="block w-full rounded-sm focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
          aria-label={`Daily delivery from ${days[0]!.key} to ${days.at(-1)!.key}. Peak ${peak} in a day. Use the arrow keys to step through days.`}
          onPointerMove={(event) => {
            const box = event.currentTarget.getBoundingClientRect()
            const ratio = (event.clientX - box.left) / box.width
            setHover(clamp(Math.round(ratio * (days.length - 1)), 0, days.length - 1))
          }}
          onPointerLeave={() => {
            setHover(null)
          }}
          onFocus={() => {
            setHover((current) => current ?? days.length - 1)
          }}
          onBlur={() => {
            setHover(null)
          }}
          onKeyDown={(event) => {
            const step = event.key === 'ArrowLeft' ? -1 : event.key === 'ArrowRight' ? 1 : 0
            if (step === 0) return
            event.preventDefault()
            setHover((current) => clamp((current ?? days.length - 1) + step, 0, days.length - 1))
          }}
          onClick={() => {
            if (active?.bucket) onDrill(active.bucket)
          }}
        >
          <svg
            viewBox={`0 0 ${W} ${H}`}
            preserveAspectRatio="none"
            className="h-40 w-full touch-none"
            aria-hidden
          >
            {/* Recessive grid: quartiles only, and never in front of the data. */}
            {[0.25, 0.5, 0.75].map((step) => (
              <line
                key={step}
                x1={0}
                x2={W}
                y1={H * step}
                y2={H * step}
                className="stroke-border"
                strokeWidth={1}
                vectorEffect="non-scaling-stroke"
              />
            ))}

            <path
              d={area(
                (day) => day.published,
                () => 0,
              )}
              className="fill-primary/25"
            />
            <path
              d={area(
                (day) => day.failed,
                (day) => day.published,
              )}
              className="fill-destructive/45"
            />
            {/* The 2px surface gap that keeps two stacked fills from reading as one shape. */}
            <path
              d={line((day) => day.published)}
              fill="none"
              className="stroke-background"
              strokeWidth={3}
              vectorEffect="non-scaling-stroke"
            />
            <path
              d={line((day) => day.published)}
              fill="none"
              className="stroke-primary"
              strokeWidth={2}
              strokeLinejoin="round"
              vectorEffect="non-scaling-stroke"
            />
            {active ? (
              <g>
                <line
                  x1={x(hover!)}
                  x2={x(hover!)}
                  y1={0}
                  y2={H}
                  className="stroke-foreground/30"
                  strokeWidth={1}
                  vectorEffect="non-scaling-stroke"
                />
                <circle
                  cx={x(hover!)}
                  cy={y(active.published)}
                  r={4}
                  className="fill-primary stroke-background"
                  strokeWidth={2}
                  vectorEffect="non-scaling-stroke"
                />
              </g>
            ) : null}
          </svg>
        </button>

        {active ? (
          <div
            className={cn(
              'pointer-events-none absolute top-0 z-10 min-w-36 rounded-md border border-border bg-popover px-2.5 py-1.5 shadow-elevated',
              // Flipped once past the midpoint so the card never leaves the pane.
              hover! > days.length / 2 ? '-translate-x-full' : '',
            )}
            style={{ left: `${(hover! / (days.length - 1)) * 100}%` }}
          >
            <p className="font-display text-xs font-semibold">{longDay(active.key)}</p>
            <p className="text-xs text-muted-foreground tabular-nums">
              {active.published} published
              {active.failed > 0 ? ` · ${active.failed} failed` : ''}
            </p>
          </div>
        ) : null}
      </div>

      <div className="mt-1 flex justify-between text-[0.6875rem] text-muted-foreground tabular-nums">
        <span>{shortDay(days[0]!.key)}</span>
        <span>peak {peak}/day</span>
        <span>{shortDay(days.at(-1)!.key)}</span>
      </div>
    </section>
  )
}

interface Day {
  key: string
  published: number
  failed: number
  /** The originating bucket, so a click can still drill into the posts. Absent on a filled day. */
  bucket: Bucket | null
}

/**
 * Every calendar day between the first and last with activity, so a gap is drawn as a gap.
 *
 * Capped at a year, because a two-year "all time" range is 730 points into a 720-unit viewBox —
 * more marks than pixels. The cap moves the START forward rather than cutting the tail: truncating
 * from the beginning would silently drop the most recent days, which are the ones being looked at.
 */
function densify(buckets: Bucket[]): Day[] {
  if (buckets.length === 0) return []
  const byKey = new Map(buckets.map((bucket) => [bucket.key, bucket]))
  const sorted = [...buckets].sort((a, b) => a.key.localeCompare(b.key))

  const end = new Date(`${sorted.at(-1)!.key}T00:00:00Z`)
  const first = new Date(`${sorted[0]!.key}T00:00:00Z`)
  const earliest = new Date(end.getTime() - (MAX_POINTS - 1) * 86_400_000)
  const start = first > earliest ? first : earliest
  const span = Math.round((end.getTime() - start.getTime()) / 86_400_000) + 1
  if (span <= 1) return []

  const days: Day[] = []
  for (let index = 0; index < span; index += 1) {
    const at = new Date(start.getTime() + index * 86_400_000)
    const key = at.toISOString().slice(0, 10)
    const bucket = byKey.get(key)
    days.push({
      key,
      published: bucket?.published ?? 0,
      failed: bucket?.failed ?? 0,
      bucket: bucket ?? null,
    })
  }
  return days
}

const MAX_POINTS = 366

function clamp(value: number, low: number, high: number) {
  return Math.min(high, Math.max(low, value))
}

function shortDay(key: string) {
  return new Date(`${key}T00:00:00Z`).toLocaleDateString(undefined, {
    month: 'short',
    day: 'numeric',
    timeZone: 'UTC',
  })
}

function longDay(key: string) {
  return new Date(`${key}T00:00:00Z`).toLocaleDateString(undefined, {
    weekday: 'short',
    month: 'short',
    day: 'numeric',
    timeZone: 'UTC',
  })
}
