/**
 * Centralised query keys.
 *
 * The first segment is the domain. `events.ts` invalidates by domain root when Rust signals a
 * change, so every read in a domain refreshes together — which only works while every key here
 * starts with its root.
 */
export const queryKeys = {
  platforms: {
    root: ['platforms'] as const,
    list: () => [...queryKeys.platforms.root, 'list'] as const,
    redirectUri: () => [...queryKeys.platforms.root, 'redirectUri'] as const,
    appCredentials: (platform: string, instance?: string | null) =>
      [...queryKeys.platforms.root, 'appCredentials', platform, instance ?? null] as const,
  },
  accounts: {
    root: ['accounts'] as const,
    list: () => [...queryKeys.accounts.root, 'list'] as const,
  },
  queue: {
    root: ['queue'] as const,
    posts: () => [...queryKeys.queue.root, 'posts'] as const,
    attempts: (postId: number) => [...queryKeys.queue.root, 'attempts', postId] as const,
    // Keyed on the draft itself: the composer's counters must re-run whenever
    // the body, the title, the attachments or the destinations change.
    check: (body: string, title: string | null, mediaCount: number, accountIds: number[]) =>
      [...queryKeys.queue.root, 'check', body, title, mediaCount, accountIds] as const,
  },
  settings: {
    root: ['settings'] as const,
    current: () => [...queryKeys.settings.root, 'current'] as const,
  },
} as const
