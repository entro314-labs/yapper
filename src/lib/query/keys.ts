import type { MediaSpec, TargetInput } from '@/lib/tauri/types'

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
    mcpCommand: () => [...queryKeys.platforms.root, 'mcpCommand'] as const,
    appCredentials: (platform: string, instance?: string | null) =>
      [...queryKeys.platforms.root, 'appCredentials', platform, instance ?? null] as const,
    // Under the platforms root because saving it changes what `list_platforms`
    // reports: the Meta adapters derive their redirect URI from this.
    webHost: () => [...queryKeys.platforms.root, 'webHost'] as const,
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
    // the body, the title, the link, the attachments, the destinations or their
    // options change.
    check: (
      body: string,
      title: string | null,
      link: string | null,
      media: MediaSpec[],
      targets: TargetInput[],
    ) => [...queryKeys.queue.root, 'check', body, title, link, media, targets] as const,
  },
  notes: {
    root: ['notes'] as const,
    list: () => [...queryKeys.notes.root, 'list'] as const,
  },
  stats: {
    root: ['stats'] as const,
    // Keyed on the whole filter: a narrowed view is a different question, not a
    // client-side slice of the same answer.
    view: (filter: unknown) => [...queryKeys.stats.root, 'view', filter] as const,
  },
  ai: {
    root: ['ai'] as const,
    availability: () => [...queryKeys.ai.root, 'availability'] as const,
  },
  settings: {
    root: ['settings'] as const,
    current: () => [...queryKeys.settings.root, 'current'] as const,
  },
  updates: {
    root: ['updates'] as const,
    /** Whether a download from THIS session is waiting for the next quit. */
    staged: () => [...queryKeys.updates.root, 'staged'] as const,
    installSupport: () => [...queryKeys.updates.root, 'installSupport'] as const,
  },
} as const
