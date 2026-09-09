import * as React from 'react'

import { brandOf } from '@/lib/platform-brand'
import type { EngagementRow } from '@/lib/tauri/types'

/**
 * What each platform earned, as of the last refresh.
 *
 * Two decisions worth stating, because both are easy to get wrong:
 *
 * **Colour encodes the METRIC, not the platform.** The obvious move is a bar per platform in its
 * brand tone, and it fails: X and Threads ship black, so two of the eight carry no hue, and Bluesky
 * sits ΔE 12.7 from Mastodon for a full-colour reader — below the legibility floor, which direct
 * labels do not excuse. Platform identity lives on the axis instead, where it gets a mark AND a
 * name, and the three validated series hues do the work colour is actually good at.
 *
 * **Impressions are not a fourth segment.** Views run 10–1000× the interaction counts, so stacking
 * them would flatten every other segment to a sliver — and putting them on a second axis is the one
 * thing a chart must never do. They get their own column, on their own scale.
 */
export function EngagementChart({ rows }: { rows: EngagementRow[] }) {
  const [hover, setHover] = React.useState<string | null>(null)
  if (rows.length === 0) return null

  const peak = Math.max(...rows.map((row) => row.likes + row.reposts + row.replies), 1)
  const anyViews = rows.some((row) => row.views > 0)

  return (
    <section>
      <header className="mb-2 flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
        <h2 className="font-display text-xs font-semibold tracking-wide text-muted-foreground uppercase">
          What it earned
        </h2>
        <ul className="flex items-center gap-3 text-xs text-muted-foreground">
          {SERIES.map((series) => (
            <li key={series.key} className="flex items-center gap-1.5">
              <span
                className="size-2 rounded-full"
                style={{ backgroundColor: series.token }}
                aria-hidden
              />
              {series.label}
            </li>
          ))}
        </ul>
      </header>

      <ul className="flex flex-col gap-1.5">
        {rows.map((row) => {
          const total = row.likes + row.reposts + row.replies
          const brand = brandOf(row.platform)
          const Icon = brand.icon
          return (
            <li
              key={row.platform}
              onPointerEnter={() => {
                setHover(row.platform)
              }}
              onPointerLeave={() => {
                setHover(null)
              }}
              className="flex items-center gap-2 rounded-md px-1.5 py-1"
            >
              <span className="flex w-24 shrink-0 items-center gap-1.5 text-xs text-muted-foreground">
                <Icon className="size-3.5 shrink-0" style={{ color: brand.tone }} aria-hidden />
                <span className="truncate">{row.label}</span>
              </span>

              {/* A 2px surface gap between segments, so two adjacent fills never read as one. */}
              <span className="flex h-3 min-w-0 flex-1 gap-0.5 overflow-hidden rounded-sm bg-muted/40">
                {SERIES.map((series) => {
                  const value = row[series.key]
                  if (value <= 0) return null
                  return (
                    <span
                      key={series.key}
                      className="h-full rounded-sm"
                      style={{
                        width: `${(value / peak) * 100}%`,
                        backgroundColor: series.token,
                      }}
                    />
                  )
                })}
              </span>

              {/* The visible label the contrast check obliges: the number is never colour-only. */}
              <span className="w-10 shrink-0 text-right text-xs font-medium tabular-nums">
                {compact(total)}
              </span>
              {anyViews ? (
                <span
                  className="w-20 shrink-0 text-right text-xs whitespace-nowrap tabular-nums"
                  style={{ color: 'var(--series-views)' }}
                  title={`${row.views.toLocaleString()} impressions`}
                >
                  {row.views > 0 ? `${compact(row.views)} seen` : '—'}
                </span>
              ) : null}
            </li>
          )
        })}
      </ul>

      <p className="mt-1.5 h-4 text-xs text-muted-foreground">
        {hover
          ? breakdownOf(rows.find((row) => row.platform === hover)!)
          : 'Bars are likes, reposts and replies on one scale. Impressions are counted separately.'}
      </p>
    </section>
  )
}

/**
 * The exact numbers behind one bar. The table view that a sub-3:1 series colour obliges — and the
 * reason a reader never has to distinguish two segments by hue alone.
 */
function breakdownOf(row: EngagementRow) {
  const parts = SERIES.filter((series) => row[series.key] > 0).map(
    (series) => `${row[series.key].toLocaleString()} ${series.label.toLowerCase()}`,
  )
  const seen = row.views > 0 ? ` · ${row.views.toLocaleString()} impressions` : ''
  return (
    <span className="tabular-nums">
      <span className="font-medium text-foreground">{row.label}</span> —{' '}
      {parts.length > 0 ? parts.join(' · ') : 'nothing yet'}
      {seen} · from {row.measured} {row.measured === 1 ? 'post' : 'posts'}
    </span>
  )
}

/** Fixed order, and never cycled — a series keeps its hue whatever the filter leaves standing. */
const SERIES = [
  { key: 'likes', label: 'Likes', token: 'var(--series-likes)' },
  { key: 'reposts', label: 'Reposts', token: 'var(--series-reposts)' },
  { key: 'replies', label: 'Replies', token: 'var(--series-replies)' },
] as const satisfies ReadonlyArray<{
  key: keyof Pick<EngagementRow, 'likes' | 'reposts' | 'replies'>
  label: string
  token: string
}>

/** 12400 → "12.4k". Four characters is what the column has. */
function compact(value: number) {
  if (value < 1000) return String(value)
  if (value < 1_000_000) return `${(value / 1000).toFixed(value < 10_000 ? 1 : 0)}k`
  return `${(value / 1_000_000).toFixed(1)}M`
}
