import { IconAlertTriangle, IconCircleCheck, IconClock, IconDownload } from '@tabler/icons-react'
import { Link } from '@tanstack/react-router'

import { usePosts, useAccounts } from '@/lib/query'
import { updateProgressPercent } from '@/lib/update-channel'
import { useUpdates } from '@/lib/updates'
import { formatRelative } from '@/lib/utils'

/**
 * The island's footer: the one line that answers "is Windbag actually going to post my things?".
 *
 * It leads with the NEXT post rather than a count, because that is the fact a scheduling tool
 * exists to tell you, and it surfaces any account needing reconnection — a dead credential is
 * silent until a post fails, and by then it is too late.
 */
export function StatusBar() {
  const posts = usePosts()
  const accounts = useAccounts()

  const next = posts.data
    ?.filter((post) => post.status === 'scheduled' && post.scheduledAt)
    .sort((a, b) => (a.scheduledAt ?? '').localeCompare(b.scheduledAt ?? ''))
    .at(0)
  const attention =
    posts.data?.filter(
      (post) => post.status === 'failed' || post.status === 'partial' || post.status === 'missed',
    ).length ?? 0
  const stale = accounts.data?.filter((account) => account.status === 'needs_reauth') ?? []

  return (
    <footer className="flex h-6 shrink-0 items-center gap-3 border-t border-border/50 px-3 text-[11px] text-muted-foreground">
      <span className="flex items-center gap-1.5">
        {/* "Nothing scheduled" over a queue that could not be read would be the
            one reassurance this bar must never give falsely. */}
        {posts.isError ? (
          <Link to="/" className="flex items-center gap-1.5 text-destructive hover:underline">
            <IconAlertTriangle className="size-3" />
            Could not read the queue
          </Link>
        ) : next ? (
          <>
            <IconClock className="size-3" />
            <span className="tabular-nums">Next {formatRelative(next.scheduledAt)}</span>
          </>
        ) : (
          <>
            <IconCircleCheck className="size-3" />
            <span>Nothing scheduled</span>
          </>
        )}
      </span>

      {attention > 0 ? (
        <Link to="/" className="flex items-center gap-1.5 text-warning hover:underline">
          <IconAlertTriangle className="size-3" />
          {attention} need{attention === 1 ? 's' : ''} attention
        </Link>
      ) : null}

      <UpdateReadout />

      {stale.length > 0 ? (
        <Link to="/accounts" className="flex items-center gap-1.5 text-destructive hover:underline">
          <IconAlertTriangle className="size-3" />
          {stale.length} account{stale.length === 1 ? '' : 's'} need
          {stale.length === 1 ? 's' : ''} reconnecting
        </Link>
      ) : null}
    </footer>
  )
}

/**
 * The ambient half of the updater: an available, downloading or staged update stays discoverable
 * without ever interrupting. `downloading` is included deliberately — an indicator that vanishes
 * for exactly as long as something is happening is worth the least when it matters most.
 *
 * The whole flow lives in Settings → Updates, which is where this points.
 */
function UpdateReadout() {
  const { state, meta, progress } = useUpdates()
  if (state !== 'available' && state !== 'downloading' && state !== 'staged') return null

  const percent = updateProgressPercent(progress)
  const label =
    state === 'staged'
      ? 'Update ready — restart to apply'
      : state === 'downloading'
        ? // No Content-Length means no honest percentage to show.
          percent === null
          ? 'Downloading update'
          : `Downloading update ${percent}%`
        : `Version ${meta?.version ?? ''} available`

  return (
    <Link to="/settings" className="flex items-center gap-1.5 text-primary hover:underline">
      <IconDownload className={state === 'downloading' ? 'size-3 animate-pulse' : 'size-3'} />
      <span className="tabular-nums">{label}</span>
    </Link>
  )
}
