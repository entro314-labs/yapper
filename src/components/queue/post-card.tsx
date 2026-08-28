import {
  IconAlertTriangle,
  IconCalendarClock,
  IconChevronDown,
  IconExternalLink,
  IconPencil,
  IconRefresh,
  IconSend,
  IconTrash,
} from '@tabler/icons-react'
import { Link } from '@tanstack/react-router'
import { openUrl } from '@tauri-apps/plugin-opener'
import * as React from 'react'
import { toast } from 'sonner'

import { Button } from '@/components/ui/button'
import { STATUS_LABEL, StatusDot } from '@/components/ui/status-dot'
import { brandOf } from '@/lib/platform-brand'
import { useAttempts, useDeletePost, usePublishNow, useRetryTarget } from '@/lib/query'
import { humanMessage } from '@/lib/tauri/client'
import type { Account, PostDetail } from '@/lib/tauri/types'
import { cn, formatAbsolute, formatRelative } from '@/lib/utils'

/**
 * One row of the queue: what goes out, where, when, and — when something went wrong — exactly which
 * destination and why.
 *
 * A post fans out to several platforms, and they fail INDEPENDENTLY. Collapsing that into one
 * status would hide the case this app exists to handle: three destinations posted, the fourth was
 * rate-limited. So the destinations are always listed, each with its own state, permalink and
 * retry.
 */
export function PostCard({ post, accounts }: { post: PostDetail; accounts: Map<number, Account> }) {
  const publishNow = usePublishNow()
  const deletePost = useDeletePost()
  const retryTarget = useRetryTarget()
  const [confirmingDelete, setConfirmingDelete] = React.useState(false)
  const [showHistory, setShowHistory] = React.useState(false)

  const needsAttention =
    post.status === 'failed' || post.status === 'partial' || post.status === 'missed'
  const editable = post.status !== 'published' && post.status !== 'publishing'

  const run = React.useCallback(async (action: Promise<unknown>, success: string) => {
    try {
      await action
      toast.success(success)
    } catch (err) {
      toast.error(humanMessage(err))
    }
  }, [])

  return (
    <article
      className={cn(
        'group rounded-lg border bg-card/60 p-3.5 transition-colors',
        needsAttention ? 'border-warning/40' : 'border-border/60',
        'hover:border-border',
      )}
    >
      <header className="mb-2 flex items-center gap-2 text-xs">
        <StatusDot status={post.status} />
        <span className="font-medium">{STATUS_LABEL[post.status]}</span>
        {post.scheduledAt ? (
          <span
            className="text-muted-foreground tabular-nums"
            title={formatAbsolute(post.scheduledAt)}
          >
            {post.status === 'published' ? '' : '· '}
            {formatRelative(post.scheduledAt)}
          </span>
        ) : null}

        <div className="ml-auto flex items-center gap-1 opacity-0 transition-opacity group-hover:opacity-100 focus-within:opacity-100">
          {editable ? (
            <Button
              size="icon-sm"
              variant="ghost"
              aria-label="Edit"
              render={<Link to="/compose" search={{ id: post.id }} />}
            >
              <IconPencil />
            </Button>
          ) : null}
          {editable && post.targets.length > 0 ? (
            <Button
              size="icon-sm"
              variant="ghost"
              aria-label="Post now"
              onClick={() => {
                void run(publishNow.mutateAsync(post.id), 'Sending now')
              }}
            >
              <IconSend />
            </Button>
          ) : null}
          <Button
            size="icon-sm"
            variant={confirmingDelete ? 'destructive' : 'ghost'}
            aria-label={confirmingDelete ? 'Confirm delete' : 'Delete'}
            // Two-step rather than a modal: deleting a draft is cheap to redo and
            // a dialog for every row would be heavier than the action deserves.
            onClick={() => {
              if (!confirmingDelete) {
                setConfirmingDelete(true)
                window.setTimeout(() => {
                  setConfirmingDelete(false)
                }, 3000)
                return
              }
              void run(deletePost.mutateAsync(post.id), 'Deleted')
            }}
          >
            <IconTrash />
          </Button>
        </div>
      </header>

      {post.title ? (
        <p className="font-display mb-1 text-sm font-semibold tracking-tight">{post.title}</p>
      ) : null}
      <p className="line-clamp-4 text-sm leading-relaxed whitespace-pre-wrap text-foreground/90">
        {post.body || <span className="text-muted-foreground">No text</span>}
      </p>
      {post.link ? <p className="mt-1 truncate text-xs text-primary">{post.link}</p> : null}
      {post.media.length > 0 ? (
        <p className="mt-1.5 text-xs text-muted-foreground">
          {post.media.length} attachment{post.media.length === 1 ? '' : 's'}
        </p>
      ) : null}

      {post.status === 'missed' ? (
        <p className="mt-2.5 flex items-start gap-1.5 rounded-md bg-warning/10 px-2 py-1.5 text-xs text-warning-foreground dark:text-warning">
          <IconAlertTriangle className="mt-px size-3.5 shrink-0" />
          Its time passed while Yapper was not running. Reschedule it, or post it now.
        </p>
      ) : null}

      <ul className="mt-2.5 flex flex-col gap-1">
        {post.targets.map((target) => {
          const account = accounts.get(target.accountId)
          if (!account) return null
          const brand = brandOf(account.platform)
          const Icon = brand.icon
          return (
            <li key={target.id} className="flex items-center gap-2 text-xs">
              <Icon className="size-3.5 shrink-0" style={{ color: brand.tone }} />
              <span className="truncate text-muted-foreground">{account.handle}</span>
              {target.options && typeof target.options.subreddit === 'string' ? (
                <span className="truncate text-muted-foreground/70">
                  r/{target.options.subreddit}
                </span>
              ) : null}
              {/* The dot sits WITH the destination it describes. Pushed to the far
                  edge it read as a separate right-hand column, which is exactly
                  the collapsed single-status view this row exists to avoid. */}
              <StatusDot status={target.status} />
              <span className="ml-auto" />

              {target.remoteUrl ? (
                <button
                  type="button"
                  className="flex items-center gap-1 text-muted-foreground hover:text-primary"
                  onClick={() => {
                    void openUrl(target.remoteUrl ?? '')
                  }}
                >
                  View <IconExternalLink className="size-3" />
                </button>
              ) : null}

              {target.status === 'failed' ? (
                <button
                  type="button"
                  className="flex items-center gap-1 text-muted-foreground hover:text-primary"
                  onClick={() => {
                    void run(retryTarget.mutateAsync(target.id), 'Queued for retry')
                  }}
                >
                  Retry <IconRefresh className="size-3" />
                </button>
              ) : null}

              {target.status === 'pending' && target.nextAttemptAt ? (
                <span
                  className="flex items-center gap-1 text-muted-foreground"
                  title={`Attempt ${target.attempts + 1}`}
                >
                  <IconCalendarClock className="size-3" />
                  retry {formatRelative(target.nextAttemptAt)}
                </span>
              ) : null}
            </li>
          )
        })}
      </ul>

      {/* Errors are shown per destination, verbatim. A generic "failed" would
          make the user open a log to learn that a subreddit needs a flair. */}
      {post.targets
        .filter((target) => target.error)
        .map((target) => (
          <p
            key={`error-${target.id}`}
            className="mt-2 rounded-md bg-destructive/10 px-2 py-1.5 text-xs leading-relaxed text-destructive"
          >
            {accounts.get(target.accountId)?.handle}: {humanMessage(target.error ?? '')}
          </p>
        ))}

      {/* The attempt log, behind a toggle and only where there is one worth
          reading. A destination that succeeded on the third try shows nothing in
          its status about the two failures — and those are exactly what tells
          you whether a platform is flaky or your post is wrong. */}
      {post.targets.some((target) => target.attempts > 1 || target.status === 'failed') ? (
        <AttemptLog
          postId={post.id}
          accounts={accounts}
          targets={post.targets}
          open={showHistory}
          onToggle={() => {
            setShowHistory((current) => !current)
          }}
        />
      ) : null}
    </article>
  )
}

function AttemptLog({
  postId,
  accounts,
  targets,
  open,
  onToggle,
}: {
  postId: number
  accounts: Map<number, Account>
  targets: PostDetail['targets']
  open: boolean
  onToggle: () => void
}) {
  // Only fetched once opened: the queue can hold hundreds of posts and none of
  // them need their history until someone asks.
  const attempts = useAttempts(open ? postId : null)
  const handleFor = React.useCallback(
    (targetId: number) => {
      const target = targets.find((candidate) => candidate.id === targetId)
      return target ? (accounts.get(target.accountId)?.handle ?? 'unknown') : 'unknown'
    },
    [targets, accounts],
  )

  return (
    <div className="mt-2">
      <button
        type="button"
        onClick={onToggle}
        className="flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground"
      >
        <IconChevronDown className={cn('size-3 transition-transform', open && 'rotate-180')} />
        History
      </button>

      {open ? (
        <ol className="mt-1.5 flex flex-col gap-1 border-l border-border/60 pl-2.5">
          {(attempts.data ?? []).map((attempt) => (
            <li key={attempt.id} className="flex gap-2 text-[11px] leading-relaxed">
              <span className="shrink-0 tabular-nums text-muted-foreground">
                {formatRelative(attempt.at)}
              </span>
              <span className="shrink-0 text-muted-foreground">{handleFor(attempt.targetId)}</span>
              <span className={cn('min-w-0', attempt.ok ? 'text-success' : 'text-destructive')}>
                {attempt.ok ? 'sent' : humanMessage(attempt.detail ?? 'failed')}
              </span>
            </li>
          ))}
          {attempts.data?.length === 0 ? (
            <li className="text-[11px] text-muted-foreground">Nothing recorded yet.</li>
          ) : null}
        </ol>
      ) : null}
    </div>
  )
}
