import { IconAlertTriangle, IconCircleCheck, IconClock } from '@tabler/icons-react'
import { Link } from '@tanstack/react-router'

import { usePosts, useAccounts } from '@/lib/query'
import { formatRelative } from '@/lib/utils'

/**
 * The island's footer: the one line that answers "is Yapper actually going to post my things?".
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
        {next ? (
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
