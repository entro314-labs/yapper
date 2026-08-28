import { IconPencilPlus, IconStack2 } from '@tabler/icons-react'
import { Link, createFileRoute } from '@tanstack/react-router'
import { motion } from 'motion/react'
import * as React from 'react'

import { PostCard } from '@/components/queue/post-card'
import { EmptyState } from '@/components/shell/empty-state'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { listVariants, rowVariants } from '@/lib/motion'
import { useAccounts, usePosts } from '@/lib/query'
import type { PostDetail, PostStatus } from '@/lib/tauri/types'
import { repeatKeys } from '@/lib/utils'

export const Route = createFileRoute('/')({ component: QueueScreen })

/**
 * The queue, in three bands.
 *
 * Ordered by what needs a decision rather than by time: anything that failed or was missed is
 * first, because it is the only part of this screen that is waiting on a person. Then what is
 * coming, then what already went out.
 */
const BANDS: Array<{ id: string; title: string; hint: string; statuses: PostStatus[] }> = [
  {
    id: 'attention',
    title: 'Needs you',
    hint: 'Failed, partly sent, or missed while Yapper was closed.',
    statuses: ['failed', 'partial', 'missed'],
  },
  {
    id: 'upcoming',
    title: 'Upcoming',
    hint: 'Scheduled and in flight.',
    statuses: ['scheduled', 'publishing'],
  },
  { id: 'drafts', title: 'Drafts', hint: 'Saved without a time.', statuses: ['draft'] },
  { id: 'sent', title: 'Sent', hint: '', statuses: ['published'] },
]

function QueueScreen() {
  const posts = usePosts()
  const accounts = useAccounts()

  const byId = React.useMemo(
    () => new Map((accounts.data ?? []).map((account) => [account.id, account])),
    [accounts.data],
  )

  const bands = React.useMemo(() => {
    const all = posts.data ?? []
    return BANDS.map((band) => ({
      id: band.id,
      title: band.title,
      hint: band.hint,
      posts: sort(
        band.id,
        all.filter((post) => band.statuses.includes(post.status)),
      ),
    })).filter((band) => band.posts.length > 0)
  }, [posts.data])

  if (posts.isLoading) {
    return (
      <div className="flex flex-col gap-2 p-4">
        {repeatKeys(4, 'post').map((key) => (
          <Skeleton key={key} className="h-28 w-full rounded-lg" />
        ))}
      </div>
    )
  }

  if (bands.length === 0) {
    return (
      <EmptyState
        icon={IconStack2}
        title="Nothing queued"
        description={
          accounts.data && accounts.data.length === 0
            ? 'Connect an account first, then write something. Bluesky takes about a minute and needs no developer app.'
            : 'Write a post, pick who it goes to, and give it a time.'
        }
        action={
          accounts.data && accounts.data.length === 0 ? (
            <Button render={<Link to="/accounts" />}>Connect an account</Button>
          ) : (
            <Button render={<Link to="/compose" />}>
              <IconPencilPlus data-icon="inline-start" />
              Write a post
            </Button>
          )
        }
      />
    )
  }

  return (
    <motion.div variants={listVariants} initial="initial" animate="animate" className="p-4">
      {bands.map((band) => (
        <section key={band.id} className="mb-6 last:mb-0">
          <div className="mb-2 flex items-baseline gap-2">
            <h2 className="font-display text-xs font-semibold tracking-wide text-muted-foreground uppercase">
              {band.title}
            </h2>
            <span className="text-xs text-muted-foreground/70 tabular-nums">
              {band.posts.length}
            </span>
            {band.hint ? (
              <span className="ml-auto hidden text-xs text-muted-foreground/70 sm:block">
                {band.hint}
              </span>
            ) : null}
          </div>
          <div className="flex flex-col gap-2">
            {band.posts.map((post) => (
              <motion.div key={post.id} variants={rowVariants}>
                <PostCard post={post} accounts={byId} />
              </motion.div>
            ))}
          </div>
        </section>
      ))}
    </motion.div>
  )
}

/**
 * Upcoming reads soonest-first — the next thing to go out is the one you want at the top.
 * Everything else reads newest-first, because a sent or failed post is history and history is
 * scanned backwards.
 */
function sort(bandId: string, posts: PostDetail[]): PostDetail[] {
  const key = (post: PostDetail) => post.scheduledAt ?? post.updatedAt
  return [...posts].sort((a, b) =>
    bandId === 'upcoming' ? key(a).localeCompare(key(b)) : key(b).localeCompare(key(a)),
  )
}
