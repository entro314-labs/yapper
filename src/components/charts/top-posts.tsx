import { IconExternalLink } from '@tabler/icons-react'
import { openUrl } from '@tauri-apps/plugin-opener'

import { brandOf } from '@/lib/platform-brand'
import type { TopPost } from '@/lib/tauri/types'

/**
 * The posts that earned the most, as of the last refresh.
 *
 * Not a chart, on purpose. Ten rows of "which post did well" is a question about identity — you
 * need to read the words to act on it — and a bar chart of ten truncated captions is a worse table
 * with a scale nobody consults. It also serves as the table view the engagement palette's contrast
 * warning obliges: every number here is text.
 *
 * Ranked by interactions rather than impressions: reach is what the platform decided to give a
 * post, interactions are what people did with it.
 */
export function TopPosts({ posts }: { posts: TopPost[] }) {
  if (posts.length === 0) return null
  const peak = Math.max(...posts.map((post) => post.interactions), 1)

  return (
    <section>
      <h2 className="mb-2 font-display text-xs font-semibold tracking-wide text-muted-foreground uppercase">
        Best performing
      </h2>
      <ul className="flex flex-col">
        {posts.map((post) => {
          const brand = brandOf(post.platform)
          const Icon = brand.icon
          return (
            <li
              key={`${post.postId}-${post.platform}`}
              className="group flex items-center gap-2.5 border-b border-border/40 py-1.5 last:border-0"
            >
              <Icon
                className="size-3.5 shrink-0"
                style={{ color: brand.tone }}
                aria-label={post.platform}
              />

              <span className="min-w-0 flex-1">
                <span className="block truncate text-xs">{post.excerpt || '(no text)'}</span>
                <span className="block truncate text-[0.6875rem] text-muted-foreground">
                  {post.handle}
                  {post.views > 0 ? ` · ${post.views.toLocaleString()} impressions` : ''}
                </span>
              </span>

              {/* A sparkbar, not a chart: it makes the ranking scannable without a second axis. */}
              <span className="hidden h-1.5 w-16 shrink-0 overflow-hidden rounded-full bg-muted/50 sm:block">
                <span
                  className="block h-full rounded-full bg-primary"
                  style={{ width: `${(post.interactions / peak) * 100}%` }}
                />
              </span>

              <span
                className="w-12 shrink-0 text-right text-xs font-medium tabular-nums"
                title={`${post.likes} likes · ${post.reposts} reposts · ${post.replies} replies`}
              >
                {post.interactions.toLocaleString()}
              </span>

              {post.remoteUrl ? (
                <button
                  type="button"
                  onClick={() => {
                    void openUrl(post.remoteUrl!)
                  }}
                  aria-label="Open the post on the platform"
                  className="shrink-0 rounded p-0.5 text-muted-foreground opacity-0 transition-opacity hover:text-foreground focus-visible:opacity-100 group-hover:opacity-100"
                >
                  <IconExternalLink className="size-3.5" />
                </button>
              ) : null}
            </li>
          )
        })}
      </ul>
    </section>
  )
}
