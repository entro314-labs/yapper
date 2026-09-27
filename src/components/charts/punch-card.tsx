import * as React from 'react'

import type { Bucket } from '@/lib/tauri/types'

/**
 * When you actually post: weekday down, hour across.
 *
 * An hour breakdown and a weekday breakdown would each collapse one axis of this, and "Tuesday at
 * 09:00" is not recoverable from the two of them — which is why the backend groups by the slot.
 *
 * A magnitude encoding, so the ramp is ONE hue running light → dark (`--ramp-from` → `--ramp-to`),
 * never a rainbow. An empty cell is the surface, not the lightest step: "never posted here" and
 * "posted here once" are different claims and must not look alike.
 */
export function PunchCard({
  buckets,
  onDrill,
}: {
  buckets: Bucket[]
  onDrill: (bucket: Bucket) => void
}) {
  const [hover, setHover] = React.useState<Bucket | null>(null)

  const byKey = React.useMemo(
    () => new Map(buckets.map((bucket) => [bucket.key, bucket])),
    [buckets],
  )
  if (buckets.length === 0) return null
  const peak = Math.max(...buckets.map((bucket) => bucket.published), 1)

  return (
    <section>
      <header className="mb-2 flex items-baseline justify-between gap-2">
        <h2 className="font-display text-xs font-semibold tracking-wide text-muted-foreground uppercase">
          When you post
        </h2>
        <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
          <span>1</span>
          <span
            className="h-2 w-16 rounded-full"
            style={{ background: 'linear-gradient(to right, var(--ramp-from), var(--ramp-to))' }}
            aria-hidden
          />
          <span className="tabular-nums">{peak}</span>
        </div>
      </header>

      <div className="flex gap-1">
        {/* Weekday gutter. Monday first, matching `number_from_monday` on the Rust side. */}
        <ul className="flex shrink-0 flex-col gap-0.5 pt-3.5">
          {WEEKDAYS.map((day) => (
            <li
              key={day}
              className="flex h-3.5 items-center text-[0.625rem] leading-none text-muted-foreground"
            >
              {day}
            </li>
          ))}
        </ul>

        <div className="min-w-0 flex-1">
          {/* Every third hour only — 24 labels in a pane this wide is a smear. */}
          <div className="mb-0.5 flex gap-0.5">
            {HOURS.map((hour) => (
              <span
                key={hour}
                className="min-w-0 flex-1 text-center text-[0.625rem] leading-3 text-muted-foreground tabular-nums"
              >
                {hour % 3 === 0 ? String(hour).padStart(2, '0') : ' '}
              </span>
            ))}
          </div>

          <div className="flex flex-col gap-0.5">
            {WEEKDAYS.map((label, row) => (
              <div key={label} className="flex gap-0.5">
                {HOURS.map((hour) => {
                  const key = `${row + 1}-${String(hour).padStart(2, '0')}`
                  const bucket = byKey.get(key)
                  const count = bucket?.published ?? 0
                  return (
                    <button
                      key={hour}
                      type="button"
                      disabled={!bucket}
                      onClick={() => {
                        if (bucket) onDrill(bucket)
                      }}
                      onPointerEnter={() => {
                        setHover(bucket ?? null)
                      }}
                      onPointerLeave={() => {
                        setHover(null)
                      }}
                      onFocus={() => {
                        setHover(bucket ?? null)
                      }}
                      onBlur={() => {
                        setHover(null)
                      }}
                      aria-label={`${label} ${String(hour).padStart(2, '0')}:00 — ${count} published`}
                      className="h-3.5 min-w-0 flex-1 rounded-[3px] ring-offset-1 ring-offset-background transition-[filter] focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none enabled:hover:brightness-110"
                      style={{
                        // An empty cell keeps the surface tint so the grid stays legible without
                        // claiming a count.
                        background: bucket
                          ? `color-mix(in oklab, var(--ramp-to) ${ratio(count, peak)}%, var(--ramp-from))`
                          : 'color-mix(in oklab, var(--muted) 50%, transparent)',
                      }}
                    />
                  )
                })}
              </div>
            ))}
          </div>
        </div>
      </div>

      <p className="mt-1.5 h-4 text-xs text-muted-foreground">
        {hover ? (
          <span className="tabular-nums">
            <span className="font-medium text-foreground">{hover.label}</span> — {hover.published}{' '}
            published
          </span>
        ) : (
          'Local time. Click a cell for the posts behind it.'
        )}
      </p>
    </section>
  )
}

/**
 * Where a count sits on the ramp, as a percentage toward the dark end.
 *
 * Floored at 12% so a single post is visibly a mark rather than indistinguishable from the empty
 * surface — the difference between "once" and "never" is the most load-bearing one on this chart.
 */
function ratio(count: number, peak: number) {
  if (count <= 0) return 0
  return Math.round(12 + (count / peak) * 88)
}

const WEEKDAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'] as const
const HOURS = Array.from({ length: 24 }, (_, hour) => hour)
