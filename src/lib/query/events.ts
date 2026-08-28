import type { QueryClient } from '@tanstack/react-query'

import { subscribeEvent } from '@/lib/tauri/client'
import { IPC_EVENTS } from '@/lib/tauri/ipc'

import { queryKeys } from './keys'

/**
 * Bridges Rust's events onto the query cache. The scheduler runs on its own thread and publishes
 * without anyone asking, so the UI cannot rely on refetch timing — a post going out has to reach
 * the screen the moment it happens.
 *
 * Returns a detach function; the caller owns the lifetime.
 */
export async function attachEventBridge(client: QueryClient): Promise<() => void> {
  const unlisteners = await Promise.all([
    subscribeEvent(IPC_EVENTS.queueChanged, () => {
      void client.invalidateQueries({ queryKey: queryKeys.queue.root })
      // Stats are computed FROM the queue, so anything that moves a post moves
      // them too — without this the figures go stale the moment one publishes.
      void client.invalidateQueries({ queryKey: queryKeys.stats.root })
    }),
    subscribeEvent(IPC_EVENTS.notesChanged, () => {
      void client.invalidateQueries({ queryKey: queryKeys.notes.root })
    }),
    subscribeEvent(IPC_EVENTS.accountsChanged, () => {
      void client.invalidateQueries({ queryKey: queryKeys.accounts.root })
    }),
    // `publishing` fires per destination and carries no new data — the queue
    // refresh that follows it does. Invalidating here too would mean two round
    // trips per post for one visual change.
    subscribeEvent(IPC_EVENTS.publishing, () => {}),
  ])

  return () => {
    for (const unlisten of unlisteners) unlisten()
  }
}
