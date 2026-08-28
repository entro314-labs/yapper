import { clsx } from 'clsx'
import type { ClassValue } from 'clsx'
import { twMerge } from 'tailwind-merge'

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}

/**
 * Stable positional keys for a fixed-count, stateless list — skeleton placeholders and the like.
 * Mapping over these instead of the array index keeps React keys stable without tripping
 * `no-array-index-key`.
 */
export function repeatKeys(count: number, prefix = 'sk'): string[] {
  return Array.from({ length: count }, (_, i) => `${prefix}-${i}`)
}

const RELATIVE = new Intl.RelativeTimeFormat(undefined, { numeric: 'auto' })
const DAY_AND_TIME = new Intl.DateTimeFormat(undefined, {
  month: 'short',
  day: 'numeric',
  hour: 'numeric',
  minute: '2-digit',
})
const FULL = new Intl.DateTimeFormat(undefined, {
  weekday: 'short',
  year: 'numeric',
  month: 'short',
  day: 'numeric',
  hour: 'numeric',
  minute: '2-digit',
})

/**
 * "in 2 hours" / "3 days ago", falling back to an absolute date past a week. Everything crossing
 * the IPC boundary is UTC; `Date` renders it in the viewer's own zone, which is the only zone a
 * scheduling UI should ever show.
 */
export function formatRelative(iso: string | null | undefined): string {
  if (!iso) return '—'
  const then = new Date(iso).getTime()
  if (!Number.isFinite(then)) return '—'
  const diffMinutes = Math.round((then - Date.now()) / 60_000)
  const diffHours = Math.round(diffMinutes / 60)
  const diffDays = Math.round(diffHours / 24)

  if (Math.abs(diffMinutes) < 1) return 'now'
  if (Math.abs(diffMinutes) < 60) return RELATIVE.format(diffMinutes, 'minute')
  if (Math.abs(diffHours) < 24) return RELATIVE.format(diffHours, 'hour')
  if (Math.abs(diffDays) < 7) return RELATIVE.format(diffDays, 'day')
  return DAY_AND_TIME.format(then)
}

export function formatAbsolute(iso: string | null | undefined): string {
  if (!iso) return '—'
  const at = new Date(iso).getTime()
  return Number.isFinite(at) ? FULL.format(at) : '—'
}

export function formatBytes(bytes: number | null | undefined): string {
  if (bytes == null || !Number.isFinite(bytes)) return '—'
  if (bytes < 1024) return `${bytes} B`
  const units = ['KB', 'MB', 'GB']
  let value = bytes / 1024
  let index = 0
  while (value >= 1024 && index < units.length - 1) {
    value /= 1024
    index += 1
  }
  return `${value.toFixed(value >= 100 ? 0 : 1)} ${units[index] ?? 'KB'}`
}

/**
 * A `datetime-local` input's value for a given instant, in the VIEWER's zone. `toISOString` would
 * render UTC and silently shift the field by the offset — the single most common way a scheduling
 * UI lies about when something fires.
 */
export function toLocalInputValue(date: Date): string {
  const local = new Date(date.getTime() - date.getTimezoneOffset() * 60_000)
  return local.toISOString().slice(0, 16)
}

/** The inverse: a `datetime-local` value read as local wall time, as UTC. */
export function fromLocalInputValue(value: string): string | null {
  if (!value) return null
  const at = new Date(value)
  return Number.isFinite(at.getTime()) ? at.toISOString() : null
}
