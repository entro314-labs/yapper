import { IconChartBar, IconInfoCircle } from '@tabler/icons-react'
import { Link, createFileRoute } from '@tanstack/react-router'
import { ask } from '@tauri-apps/plugin-dialog'
import * as React from 'react'
import { toast } from 'sonner'

import { DeliveryChart } from '@/components/charts/delivery-chart'
import { EngagementChart } from '@/components/charts/engagement-chart'
import { PunchCard } from '@/components/charts/punch-card'
import { TopPosts } from '@/components/charts/top-posts'
import { RefreshCwIcon } from '@/components/icons/refresh-cw'
import { EmptyState } from '@/components/shell/empty-state'
import { QueryErrorState } from '@/components/shell/error-screen'
import { Button } from '@/components/ui/button'
import { Select } from '@/components/ui/select'
import { STATUS_LABEL } from '@/components/ui/status-dot'
import { useAnimatedIcon } from '@/lib/animated-icon'
import { brandOf } from '@/lib/platform-brand'
import {
  readRefreshCost,
  useAccounts,
  usePlatforms,
  usePosts,
  useRefreshEngagement,
  useStats,
} from '@/lib/query'
import { humanMessage } from '@/lib/tauri/client'
import type { Bucket, EngagementRow, PlatformId, StatsFilter } from '@/lib/tauri/types'
import { cn, formatRelative } from '@/lib/utils'

const RANGES = [
  { key: '7', label: 'Last 7 days' },
  { key: '30', label: 'Last 30 days' },
  { key: '90', label: 'Last 90 days' },
  { key: 'all', label: 'All time' },
] as const

type Range = (typeof RANGES)[number]['key']

/**
 * The filters live in the URL rather than in component state, so opening a post from a drilldown
 * and coming back lands on the same view instead of resetting to the last 30 days. Every key is
 * optional and a default is left out of the URL. `platform` is kept as a string here and resolved
 * against the known platforms on the screen, where that list exists.
 */
export const Route = createFileRoute('/stats')({
  component: StatsScreen,
  validateSearch: (
    search: Record<string, unknown>,
  ): { range?: Range; platform?: string; account?: number } => {
    const range = RANGES.find((option) => option.key === search.range)?.key
    const account = Number(search.account)
    return {
      ...(range === undefined ? {} : { range }),
      ...(typeof search.platform === 'string' ? { platform: search.platform } : {}),
      ...(Number.isInteger(account) && account > 0 ? { account } : {}),
    }
  },
})

/**
 * What actually happened.
 *
 * Two halves, kept visibly apart. DELIVERY is computed from Windbag's own records — always there,
 * always current, and the only half that can answer "why did this fail". ENGAGEMENT is read from
 * each platform's official endpoint; the two that refuse are named as gaps rather than drawn as
 * zeroes, because a zero is a claim and "we cannot see it" is the truth.
 *
 * Only the delivery half is drawn over time. Engagement is one snapshot per destination, replaced
 * on each refresh, so there is no honest trend line to draw from it — see the module comment in
 * `stats.rs`. It is shown as a distribution instead: across platforms, across posts.
 *
 * Every bar and every cell is a drilldown: it carries the ids of the posts behind it, and clicking
 * one opens that set. A top post opens in the composer directly.
 */
function StatsScreen() {
  const accounts = useAccounts()
  const platforms = usePlatforms()
  const posts = usePosts()
  const refresh = useRefreshEngagement()
  const [refreshRef, refreshHover] = useAnimatedIcon()
  const search = Route.useSearch()
  const navigate = Route.useNavigate()
  // Any set of posts a chart can open: a delivery bucket, a punch-card cell or an engagement bar.
  const [drilldown, setDrilldown] = React.useState<Pick<Bucket, 'label' | 'postIds'> | null>(null)
  const drillRef = React.useRef<HTMLElement>(null)

  // The drilldown renders below every chart, so a click on a bar near the top would otherwise
  // change something off-screen and look like it did nothing. Scrolled to and focused, so a
  // keyboard or screen-reader user lands on the answer too.
  React.useEffect(() => {
    const section = drillRef.current
    if (!drilldown || !section) return
    const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches
    section.scrollIntoView({ behavior: reduced ? 'auto' : 'smooth', block: 'start' })
    section.focus({ preventScroll: true })
  }, [drilldown])

  const range = search.range ?? '30'
  // The bound is computed when the range CHANGES, not on every render: reading
  // the clock during render makes a fresh query key each pass, so the cached
  // answer is never the one being asked for.
  const since = React.useMemo(() => boundFor(range), [range])
  // The platforms on offer are the ones an account is connected to — not the
  // ones in the current result, which a filter has already narrowed to one.
  const connected = React.useMemo(
    () =>
      (platforms.data ?? []).filter((info) =>
        (accounts.data ?? []).some((account) => account.platform === info.id),
      ),
    [platforms.data, accounts.data],
  )
  // Resolved against what is connected, so a URL naming a platform or account
  // that has since been removed falls back to "all" instead of an empty view.
  const platform = connected.find((info) => info.id === search.platform)?.id
  const accountId = accounts.data?.find((account) => account.id === search.account)?.id

  const filter: StatsFilter = React.useMemo(
    () => ({
      since,
      until: null,
      platforms: platform === undefined ? [] : [platform],
      accountIds: accountId === undefined ? [] : [accountId],
    }),
    [since, platform, accountId],
  )

  /** Replaces one filter in the URL. `replace`, so Back leaves the screen rather than the filter. */
  const setSearch = (next: { range?: Range; platform?: string; account?: number }) => {
    setDrilldown(null)
    void navigate({ search: (prev) => ({ ...prev, ...next }), replace: true })
  }

  const stats = useStats(filter)
  const data = stats.data

  // The drilldown resolves ids to posts here rather than navigating away: the
  // question "which posts are these" is asked while looking at the bar.
  const drilled = React.useMemo(() => {
    if (!drilldown) return []
    const ids = new Set(drilldown.postIds)
    return (posts.data ?? []).filter((post) => ids.has(post.id))
  }, [drilldown, posts.data])

  if (stats.isError || posts.isError || accounts.isError || platforms.isError) {
    return <QueryErrorState what="the stats" queries={[stats, posts, accounts, platforms]} />
  }

  // First run is a fact about the STORE, never about the filtered view: an empty 30-day window
  // with the filters hidden behind this screen would leave no way to widen it. Strictly `=== 0`,
  // so a store still loading does not flash the empty state either.
  if (posts.data?.length === 0) {
    return (
      <EmptyState
        icon={IconChartBar}
        title="Nothing to measure yet"
        description="Once posts have gone out, this is where the delivery record lives — what published, what failed and why, and when you actually post."
        action={<Button render={<Link to="/compose" />}>Write a post</Button>}
      />
    )
  }

  return (
    <div className="mx-auto flex max-w-4xl flex-col gap-5 p-4">
      <header className="flex flex-wrap items-center gap-2">
        <Select
          value={range}
          onChange={(event) => {
            const chosen = RANGES.find((option) => option.key === event.target.value)?.key
            setSearch({ range: chosen === '30' ? undefined : chosen })
          }}
          aria-label="Date range"
          className="w-36"
        >
          {RANGES.map((option) => (
            <option key={option.key} value={option.key}>
              {option.label}
            </option>
          ))}
        </Select>

        <Select
          value={platform ?? 'all'}
          onChange={(event) => {
            setSearch({ platform: event.target.value === 'all' ? undefined : event.target.value })
          }}
          aria-label="Platform"
          className="w-36"
        >
          <option value="all">All platforms</option>
          {connected.map((info) => (
            <option key={info.id} value={info.id}>
              {info.name}
            </option>
          ))}
        </Select>

        <Select
          value={String(accountId ?? 'all')}
          onChange={(event) => {
            setSearch({
              account: event.target.value === 'all' ? undefined : Number(event.target.value),
            })
          }}
          aria-label="Account"
          className="w-44"
        >
          <option value="all">All accounts</option>
          {(accounts.data ?? []).map((account) => (
            <option key={account.id} value={account.id}>
              {account.handle}
            </option>
          ))}
        </Select>
      </header>

      <section className="grid grid-cols-2 gap-2 sm:grid-cols-5">
        <Tile label="Published" value={data?.published ?? 0} tone="success" />
        <Tile label="Failed" value={data?.failed ?? 0} tone="destructive" />
        <Tile label="Scheduled" value={data?.scheduled ?? 0} tone="primary" />
        <Tile label="Missed" value={data?.missed ?? 0} tone="warning" />
        <Tile label="Drafts" value={data?.drafts ?? 0} tone="muted" />
      </section>

      {data && data.published === 0 && data.failed === 0 ? (
        <p className="rounded-lg border border-border/60 bg-card/50 px-3.5 py-3 text-sm text-muted-foreground">
          Nothing published or failed in this view. Widen the range or clear a filter to see more.
        </p>
      ) : null}

      <Breakdown
        title="By platform"
        buckets={data?.byPlatform ?? []}
        onDrill={setDrilldown}
        brand
      />
      <Breakdown title="By account" buckets={data?.byAccount ?? []} onDrill={setDrilldown} />
      <Breakdown
        title="When you post"
        hint="Local time, published destinations only."
        buckets={data?.byHour ?? []}
        onDrill={setDrilldown}
      />
      {(data?.failures.length ?? 0) > 0 ? (
        <Breakdown
          title="Why things failed"
          hint="Credential problems need you; rate limits clear on their own."
          buckets={data?.failures ?? []}
          onDrill={setDrilldown}
          failures
        />
      ) : null}

      {data && data.byDay.length > 1 ? (
        <div className="rounded-lg border border-border/60 bg-card/50 p-3.5">
          <DeliveryChart buckets={data.byDay} onDrill={setDrilldown} />
        </div>
      ) : null}

      {data && data.bySlot.length > 0 ? (
        <div className="rounded-lg border border-border/60 bg-card/50 p-3.5">
          <PunchCard buckets={data.bySlot} onDrill={setDrilldown} />
        </div>
      ) : null}

      <section>
        <div className="mb-2 flex items-baseline gap-2">
          <h2 className="font-display text-xs font-semibold tracking-wide text-muted-foreground uppercase">
            Engagement
          </h2>
          <Button
            size="xs"
            variant="outline"
            className="ml-auto"
            disabled={refresh.isPending}
            {...refreshHover}
            onClick={() => {
              void (async () => {
                // X is the only platform that bills per read, so it is the only one that gets a
                // confirmation. Asking before every free refresh would train the click away.
                //
                // The cost is read HERE, at the click, and a failure to read it aborts the refresh
                // with its error: a cached, pending or failed count defaulting to zero billed reads
                // would spend the money without asking. `ask` rather than `window.confirm` for the
                // same class of reason — the webview's own confirm resolves to a Promise, which is
                // truthy whatever the user clicked, so the guard would never once have held.
                try {
                  const spend = await readRefreshCost()
                  const billed = spend.billedReads
                  if (billed > 0) {
                    const proceed = await ask(
                      `This reads ${billed} post${billed === 1 ? '' : 's'} from X, which bills against your app's credits. The other ${spend.freeReads} are free.`,
                      { title: 'Refresh engagement', kind: 'warning' },
                    )
                    if (!proceed) return
                  }
                  const report = await refresh.mutateAsync()
                  // Every part is said, even on failure: rows written before an error are in the
                  // store, and X reads made before it are on the bill.
                  const plural = report.updated === 1 ? '' : 's'
                  const summary = [
                    `Updated ${report.updated} destination${plural}`,
                    report.skipped > 0 ? `skipped ${report.skipped}` : null,
                    report.failed > 0 ? `failed ${report.failed}` : null,
                    report.billedReads > 0 ? `${report.billedReads} billed X reads` : null,
                  ]
                    .filter((part) => part !== null)
                    .join(', ')
                  const detail =
                    report.problems.length > 0
                      ? { description: report.problems.join('\n'), duration: 10_000 }
                      : undefined
                  if (report.failed > 0) toast.warning(summary, detail)
                  else toast.success(summary, detail)
                } catch (err) {
                  toast.error(humanMessage(err))
                }
              })()
            }}
          >
            <RefreshCwIcon
              ref={refreshRef}
              data-icon="inline-start"
              className={cn(refresh.isPending && 'animate-spin')}
            />
            Refresh
          </Button>
        </div>

        <div className="rounded-lg border border-border/60 bg-card/50 p-3.5">
          {data && data.engagement.measured > 0 ? (
            <>
              <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
                <Figure
                  label="Likes"
                  value={data.engagement.likes}
                  silent={unreported(data.engagementByPlatform, 'likes')}
                />
                <Figure
                  label="Reposts"
                  value={data.engagement.reposts}
                  silent={unreported(data.engagementByPlatform, 'reposts')}
                />
                <Figure
                  label="Replies"
                  value={data.engagement.replies}
                  silent={unreported(data.engagementByPlatform, 'replies')}
                />
                {/* Impressions sit beside the three rather than among them: a reach number on a
                    scale 100× the others is not a fourth interaction. */}
                <Figure
                  label="Impressions"
                  value={data.engagement.views}
                  silent={unreported(data.engagementByPlatform, 'views')}
                  muted
                />
              </div>
              <p className="mt-2.5 text-xs text-muted-foreground">
                Across {data.engagement.measured} destination
                {data.engagement.measured === 1 ? '' : 's'}, as of{' '}
                {formatRelative(data.engagement.oldestFetch)}.
              </p>

              {data.engagementByPlatform.length > 1 ? (
                <div className="mt-4 border-t border-border/50 pt-3.5">
                  <EngagementChart rows={data.engagementByPlatform} onDrill={setDrilldown} />
                </div>
              ) : null}

              {data.topPosts.length > 0 ? (
                <div className="mt-4 border-t border-border/50 pt-3.5">
                  <TopPosts posts={data.topPosts} />
                </div>
              ) : null}
            </>
          ) : (
            <p className="text-sm text-muted-foreground">
              Nothing fetched yet. Refresh pulls public counts for the destinations Windbag can read
              — which is not the same as those destinations having no engagement.
            </p>
          )}

          {(data?.engagementGaps.length ?? 0) > 0 ? (
            <ul className="mt-3 flex flex-col gap-1 border-t border-border/50 pt-2.5">
              {(data?.engagementGaps ?? []).map(([id, reason]) => {
                const brand = brandOf(id)
                const Icon = brand.icon
                return (
                  <li key={id} className="flex items-start gap-1.5 text-xs text-muted-foreground">
                    <Icon className="mt-px size-3.5 shrink-0" style={{ color: brand.tone }} />
                    {reason}
                  </li>
                )
              })}
            </ul>
          ) : null}
        </div>
      </section>

      {drilldown ? (
        <section
          ref={drillRef}
          tabIndex={-1}
          aria-labelledby="stats-drilldown"
          className="scroll-mt-4 rounded-md outline-none focus-visible:ring-2 focus-visible:ring-ring"
        >
          <div className="mb-2 flex items-baseline gap-2">
            <h2
              id="stats-drilldown"
              className="font-display text-xs font-semibold tracking-wide text-muted-foreground uppercase"
            >
              {drilldown.label}
            </h2>
            <span className="text-xs text-muted-foreground tabular-nums">
              {drilled.length} post{drilled.length === 1 ? '' : 's'}
            </span>
            <Button
              size="xs"
              variant="ghost"
              className="ml-auto"
              onClick={() => {
                setDrilldown(null)
              }}
            >
              Clear
            </Button>
          </div>
          <ul className="flex flex-col gap-1.5">
            {drilled.map((post) => (
              <li key={post.id}>
                {/* A published post is history: opening it starts a new draft from it rather than
                    editing what already went out. */}
                <Link
                  to="/compose"
                  search={post.status === 'published' ? { from: post.id } : { id: post.id }}
                  className="flex flex-col gap-0.5 rounded-md border border-border/60 bg-card/50 px-3 py-2 transition-colors hover:border-border"
                >
                  <span className="line-clamp-2 text-sm">{post.body || 'No text'}</span>
                  <span className="text-xs text-muted-foreground">
                    {STATUS_LABEL[post.status]} ·{' '}
                    {formatRelative(post.scheduledAt ?? post.updatedAt)}
                  </span>
                </Link>
              </li>
            ))}
          </ul>
        </section>
      ) : null}
    </div>
  )
}

/**
 * The lower bound for a named range, rounded down to the hour so the query key stays stable while
 * the screen is open — a millisecond-precision bound would make every render a cache miss.
 */
function boundFor(range: string): string | null {
  if (range === 'all') return null
  const hour = 3_600_000
  const start = Math.floor((Date.now() - Number(range) * 86_400_000) / hour) * hour
  return new Date(start).toISOString()
}

function Tile({
  label,
  value,
  tone,
}: {
  label: string
  value: number
  tone: 'success' | 'destructive' | 'primary' | 'warning' | 'muted'
}) {
  const color = {
    success: 'text-success',
    destructive: 'text-destructive',
    primary: 'text-primary',
    warning: 'text-warning',
    muted: 'text-muted-foreground',
  }[tone]
  return (
    <div className="rounded-lg border border-border/60 bg-card/50 px-3 py-2.5">
      <p className={cn('font-display text-xl font-semibold tabular-nums', color)}>{value}</p>
      <p className="text-xs text-muted-foreground">{label}</p>
    </div>
  )
}

/** The platforms in view that report nothing for one engagement dimension. */
function unreported(
  rows: EngagementRow[],
  key: 'likes' | 'reposts' | 'replies' | 'views',
): string[] {
  return rows.filter((row) => row[key] === null).map((row) => row.label)
}

/**
 * One engagement total. `null` is drawn as "—", never as 0: no measured platform reported it, and a
 * zero would claim nobody engaged. `silent` names the platforms that do not report this dimension,
 * so a partial total says which part of the picture it is missing.
 */
function Figure({
  label,
  value,
  silent,
  muted = false,
}: {
  label: string
  value: number | null
  silent: string[]
  muted?: boolean
}) {
  const hint = silent.length > 0 ? `Not reported by ${silent.join(', ')}` : undefined
  return (
    <div title={hint}>
      <p
        className={cn(
          'font-display text-xl font-semibold tabular-nums',
          (muted || value === null) && 'text-muted-foreground',
        )}
      >
        {value === null ? '—' : value.toLocaleString()}
      </p>
      <p className="text-xs text-muted-foreground">{label}</p>
      {hint ? <p className="text-[0.6875rem] text-muted-foreground/70">{hint}</p> : null}
    </div>
  )
}

/**
 * Bars as divs, scaled to the largest row in the set.
 *
 * Deliberately no charting library: every breakdown here is a single series of labelled counts,
 * which a flex row draws exactly and a chart library draws with 40kB of axis machinery nobody asked
 * for. The published/failed split is two segments of one bar, so a platform that mostly fails
 * cannot look busy and healthy at a glance.
 */
function Breakdown({
  title,
  hint,
  buckets,
  onDrill,
  brand = false,
  failures = false,
}: {
  title: string
  hint?: string
  buckets: Bucket[]
  onDrill: (bucket: Bucket) => void
  brand?: boolean
  failures?: boolean
}) {
  if (buckets.length === 0) return null
  const max = Math.max(...buckets.map((bucket) => bucket.published + bucket.failed), 1)

  return (
    <section>
      <div className="mb-2 flex items-baseline gap-2">
        <h2 className="font-display text-xs font-semibold tracking-wide text-muted-foreground uppercase">
          {title}
        </h2>
        {hint ? (
          <span className="flex items-center gap-1 text-xs text-muted-foreground/70">
            <IconInfoCircle className="size-3" />
            {hint}
          </span>
        ) : null}
      </div>
      <ul className="flex flex-col gap-1">
        {buckets.map((bucket) => {
          const total = bucket.published + bucket.failed
          const tone = brand ? brandOf(bucket.key as PlatformId).tone : undefined
          return (
            <li key={bucket.key}>
              <button
                type="button"
                onClick={() => {
                  onDrill(bucket)
                }}
                className="group flex w-full items-center gap-2 rounded-md px-1.5 py-1 text-left transition-colors hover:bg-muted/50"
                title={`${bucket.published} published, ${bucket.failed} failed`}
              >
                <span className="w-24 shrink-0 truncate text-xs text-muted-foreground">
                  {bucket.label}
                </span>
                <span className="flex h-3 min-w-0 flex-1 gap-px overflow-hidden rounded-sm bg-muted/40">
                  {bucket.published > 0 ? (
                    <span
                      className={cn('h-full rounded-sm', !tone && 'bg-primary')}
                      style={{
                        width: `${(bucket.published / max) * 100}%`,
                        ...(tone ? { backgroundColor: tone } : {}),
                      }}
                    />
                  ) : null}
                  {bucket.failed > 0 ? (
                    <span
                      className={cn(
                        'h-full rounded-sm',
                        failures ? 'bg-warning' : 'bg-destructive',
                      )}
                      style={{ width: `${(bucket.failed / max) * 100}%` }}
                    />
                  ) : null}
                </span>
                <span className="w-8 shrink-0 text-right text-xs text-muted-foreground tabular-nums">
                  {total}
                </span>
              </button>
            </li>
          )
        })}
      </ul>
    </section>
  )
}
