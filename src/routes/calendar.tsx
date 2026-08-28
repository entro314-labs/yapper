import { IconCalendarClock, IconChevronLeft, IconChevronRight } from '@tabler/icons-react'
import { Link, createFileRoute } from '@tanstack/react-router'
import * as React from 'react'
import { toast } from 'sonner'

import { EmptyState } from '@/components/shell/empty-state'
import { Button } from '@/components/ui/button'
import { STATUS_LABEL, StatusDot } from '@/components/ui/status-dot'
import { brandOf } from '@/lib/platform-brand'
import { useAccounts, usePosts, useReschedulePost } from '@/lib/query'
import { humanMessage } from '@/lib/tauri/client'
import type { PostDetail } from '@/lib/tauri/types'
import { cn } from '@/lib/utils'

export const Route = createFileRoute('/calendar')({ component: CalendarScreen })

const WEEKDAY = new Intl.DateTimeFormat(undefined, { weekday: 'short' })
const MONTH = new Intl.DateTimeFormat(undefined, { month: 'long', year: 'numeric' })
// 24-hour on purpose: a day cell is roughly ten characters wide, and "5:12 AM"
// wraps onto two lines in it where "05:12" does not.
const TIME = new Intl.DateTimeFormat(undefined, {
  hour: '2-digit',
  minute: '2-digit',
  hourCycle: 'h23',
})

/**
 * A month of the queue, laid out so the shape of a posting week is visible — which is the thing a
 * list cannot show: three posts on Tuesday and nothing for the rest of the week.
 *
 * Everything is drawn in LOCAL time. The store is UTC and the conversion happens here, at the only
 * place a human reads a date.
 */
function CalendarScreen() {
  const posts = usePosts()
  const accounts = useAccounts()
  const reschedule = useReschedulePost()
  const [monthStart, setMonthStart] = React.useState(() => {
    const now = new Date()
    return new Date(now.getFullYear(), now.getMonth(), 1)
  })

  const byId = React.useMemo(
    () => new Map((accounts.data ?? []).map((account) => [account.id, account])),
    [accounts.data],
  )

  const scheduled = React.useMemo(
    () => (posts.data ?? []).filter((post) => post.scheduledAt),
    [posts.data],
  )

  const days = React.useMemo(() => buildGrid(monthStart, scheduled), [monthStart, scheduled])

  if (scheduled.length === 0) {
    return (
      <EmptyState
        icon={IconCalendarClock}
        title="Nothing on the calendar"
        description="Posts with a time appear here. Drafts stay in the queue until you give them one."
        action={<Button render={<Link to="/compose" />}>Write a post</Button>}
      />
    )
  }

  return (
    <div className="flex flex-col gap-3 p-4">
      <header className="flex items-center gap-2">
        <h2 className="font-display text-sm font-semibold tracking-tight">
          {MONTH.format(monthStart)}
        </h2>
        <div className="ml-auto flex items-center gap-1">
          <Button
            size="icon-sm"
            variant="ghost"
            aria-label="Previous month"
            onClick={() => {
              setMonthStart(shiftMonth(monthStart, -1))
            }}
          >
            <IconChevronLeft />
          </Button>
          <Button
            size="sm"
            variant="ghost"
            onClick={() => {
              const now = new Date()
              setMonthStart(new Date(now.getFullYear(), now.getMonth(), 1))
            }}
          >
            Today
          </Button>
          <Button
            size="icon-sm"
            variant="ghost"
            aria-label="Next month"
            onClick={() => {
              setMonthStart(shiftMonth(monthStart, 1))
            }}
          >
            <IconChevronRight />
          </Button>
        </div>
      </header>

      <div className="grid grid-cols-7 gap-px overflow-hidden rounded-lg border border-border/60 bg-border/40">
        {days.slice(0, 7).map((day) => (
          <div
            key={`head-${day.date.toISOString()}`}
            className="bg-card/70 px-2 py-1.5 text-center text-[11px] font-medium tracking-wide text-muted-foreground uppercase"
          >
            {WEEKDAY.format(day.date)}
          </div>
        ))}

        {days.map((day) => (
          <div
            key={day.date.toISOString()}
            onDragOver={(event) => {
              // The whole point of the calendar: drop a post on a day to move it,
              // keeping its time of day. Retiming is the single most common
              // thing you want to do to a schedule.
              event.preventDefault()
            }}
            onDrop={(event) => {
              event.preventDefault()
              const postId = Number(event.dataTransfer.getData('text/yapper-post'))
              const from = event.dataTransfer.getData('text/yapper-at')
              if (!Number.isInteger(postId) || !from) return
              const source = new Date(from)
              const moved = new Date(day.date)
              moved.setHours(source.getHours(), source.getMinutes(), 0, 0)
              void (async () => {
                try {
                  await reschedule.mutateAsync({
                    id: postId,
                    scheduledAt: moved.toISOString(),
                  })
                  toast.success(`Moved to ${moved.toLocaleString()}`)
                } catch (err) {
                  toast.error(humanMessage(err))
                }
              })()
            }}
            className={cn(
              'min-h-24 bg-card/60 p-1.5 transition-colors',
              !day.inMonth && 'bg-card/25 text-muted-foreground/60',
              day.isToday && 'bg-primary/[0.06]',
            )}
          >
            <span
              className={cn(
                'text-[11px] tabular-nums',
                day.isToday ? 'font-semibold text-primary' : 'text-muted-foreground',
              )}
            >
              {day.date.getDate()}
            </span>

            <div className="mt-1 flex flex-col gap-1">
              {day.posts.map((post) => (
                <button
                  key={post.id}
                  type="button"
                  draggable
                  onDragStart={(event) => {
                    event.dataTransfer.setData('text/yapper-post', String(post.id))
                    event.dataTransfer.setData('text/yapper-at', post.scheduledAt ?? '')
                    event.dataTransfer.effectAllowed = 'move'
                  }}
                  title={`${STATUS_LABEL[post.status]} · ${post.body.slice(0, 120)}`}
                  className="flex w-full items-center gap-1 rounded border border-border/50 bg-background/50 px-1 py-0.5 text-left text-[11px] hover:border-border"
                >
                  <StatusDot status={post.status} />
                  <span className="shrink-0 whitespace-nowrap tabular-nums text-muted-foreground">
                    {TIME.format(new Date(post.scheduledAt ?? ''))}
                  </span>
                  <span className="flex -space-x-0.5">
                    {post.targets.slice(0, 3).map((target) => {
                      const account = byId.get(target.accountId)
                      if (!account) return null
                      const brand = brandOf(account.platform)
                      const Icon = brand.icon
                      return (
                        <Icon key={target.id} className="size-2.5" style={{ color: brand.tone }} />
                      )
                    })}
                  </span>
                  <span className="truncate">{post.body}</span>
                </button>
              ))}
            </div>
          </div>
        ))}
      </div>

      <p className="text-xs text-muted-foreground">
        Drag a post to another day to move it. It keeps its time of day.
      </p>
    </div>
  )
}

interface Day {
  date: Date
  inMonth: boolean
  isToday: boolean
  posts: PostDetail[]
}

/**
 * Six weeks starting on the Monday on or before the first of the month, so the grid never changes
 * height between months — a calendar that reflows as you page through it makes the posts jump under
 * the cursor.
 */
function buildGrid(monthStart: Date, posts: PostDetail[]): Day[] {
  const first = new Date(monthStart)
  // getDay() is Sunday-based; this shifts the week to start on Monday.
  const offset = (first.getDay() + 6) % 7
  const start = new Date(first)
  start.setDate(first.getDate() - offset)

  const today = new Date()
  const byDay = new Map<string, PostDetail[]>()
  for (const post of posts) {
    if (!post.scheduledAt) continue
    const key = dayKey(new Date(post.scheduledAt))
    byDay.set(key, [...(byDay.get(key) ?? []), post])
  }
  for (const list of byDay.values()) {
    list.sort((a, b) => (a.scheduledAt ?? '').localeCompare(b.scheduledAt ?? ''))
  }

  return Array.from({ length: 42 }, (_, index) => {
    const date = new Date(start)
    date.setDate(start.getDate() + index)
    return {
      date,
      inMonth: date.getMonth() === monthStart.getMonth(),
      isToday: dayKey(date) === dayKey(today),
      posts: byDay.get(dayKey(date)) ?? [],
    }
  })
}

function dayKey(date: Date): string {
  return `${date.getFullYear()}-${date.getMonth()}-${date.getDate()}`
}

/**
 * Month arithmetic via the day-1 constructor rather than `setMonth`: stepping back a month from the
 * 31st with `setMonth` lands in the following month.
 */
function shiftMonth(from: Date, delta: number): Date {
  return new Date(from.getFullYear(), from.getMonth() + delta, 1)
}
