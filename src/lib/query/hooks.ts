import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import type { UseMutationResult } from '@tanstack/react-query'

import { invokeCommand } from '@/lib/tauri/client'
import { IPC_COMMANDS } from '@/lib/tauri/ipc'
import type {
  Account,
  AiAvailability,
  AppCredentialsView,
  Attempt,
  MediaSpec,
  PlatformId,
  PlatformInfo,
  PostDetail,
  Note,
  RefreshCost,
  RefreshReport,
  ResolvedMedia,
  SaveNoteInput,
  SavePostInput,
  Settings,
  Stats,
  StatsFilter,
  Suggestion,
  TargetCheck,
  TargetInput,
  WebHostView,
} from '@/lib/tauri/types'

import { queryKeys } from './keys'

/**
 * The resolved value of a command that answers with nothing. Rust's `()` serializes as JSON `null`,
 * so this is `null` and not `undefined` — and naming it keeps `void` out of a position where it
 * would only mean "ignore me".
 */
type Nothing = null

// ─── Reads ──────────────────────────────────────────────────────────────────

/**
 * The platform catalogue. Static for the life of the process — Rust builds it from the adapters,
 * and adapters do not change while the app runs.
 */
export function usePlatforms() {
  return useQuery({
    queryKey: queryKeys.platforms.list(),
    queryFn: async () => invokeCommand<PlatformInfo[]>(IPC_COMMANDS.listPlatforms),
    staleTime: Number.POSITIVE_INFINITY,
  })
}

export function useAccounts() {
  return useQuery({
    queryKey: queryKeys.accounts.list(),
    queryFn: async () => invokeCommand<Account[]>(IPC_COMMANDS.listAccounts),
  })
}

export function usePosts() {
  return useQuery({
    queryKey: queryKeys.queue.posts(),
    queryFn: async () => invokeCommand<PostDetail[]>(IPC_COMMANDS.listPosts),
  })
}

export function useAttempts(postId: number | null) {
  return useQuery({
    queryKey: queryKeys.queue.attempts(postId ?? -1),
    queryFn: async () => invokeCommand<Attempt[]>(IPC_COMMANDS.listAttempts, { postId }),
    enabled: postId != null,
  })
}

export function useSettings() {
  return useQuery({
    queryKey: queryKeys.settings.current(),
    queryFn: async () => invokeCommand<Settings>(IPC_COMMANDS.getSettings),
  })
}

export function useRedirectUri() {
  return useQuery({
    queryKey: queryKeys.platforms.redirectUri(),
    queryFn: async () => invokeCommand<string>(IPC_COMMANDS.oauthRedirectUri),
    staleTime: Number.POSITIVE_INFINITY,
  })
}

export function useMcpCommand() {
  return useQuery({
    queryKey: queryKeys.platforms.mcpCommand(),
    queryFn: async () => invokeCommand<string>(IPC_COMMANDS.mcpCommand),
    staleTime: Number.POSITIVE_INFINITY,
  })
}

export function useAppCredentials(platform: PlatformId, instance?: string | null) {
  return useQuery({
    queryKey: queryKeys.platforms.appCredentials(platform, instance),
    queryFn: async () =>
      invokeCommand<AppCredentialsView | null>(IPC_COMMANDS.getAppCredentials, {
        platform,
        instance: instance ?? null,
      }),
  })
}

export function useWebHost() {
  return useQuery({
    queryKey: queryKeys.platforms.webHost(),
    queryFn: async () => invokeCommand<WebHostView | null>(IPC_COMMANDS.getWebHost),
  })
}

/**
 * The composer's live verdict per destination. Runs the SAME validation the scheduler will run, so
 * a counter that says a post fits is a promise the backend keeps.
 */
export function useCheckPost(
  body: string,
  title: string | null,
  link: string | null,
  media: MediaSpec[],
  targets: TargetInput[],
) {
  return useQuery({
    queryKey: queryKeys.queue.check(body, title, link, media, targets),
    queryFn: async () =>
      invokeCommand<TargetCheck[]>(IPC_COMMANDS.checkPost, {
        body,
        title,
        link,
        media,
        targets,
      }),
    enabled: targets.length > 0,
    // A draft is only ever checked against what is on screen right now; keeping
    // old verdicts around would let a stale "fits" survive an edit.
    gcTime: 0,
    placeholderData: (previous) => previous,
  })
}

export function useNotes() {
  return useQuery({
    queryKey: queryKeys.notes.list(),
    queryFn: async () => invokeCommand<Note[]>(IPC_COMMANDS.listNotes),
  })
}

/**
 * The stats view. Reads nothing remote — every figure is computed from Windbag's own records — so
 * it is cheap enough to recompute on every filter change.
 */
export function useStats(filter: StatsFilter) {
  return useQuery({
    queryKey: queryKeys.stats.view(filter),
    queryFn: async () => invokeCommand<Stats>(IPC_COMMANDS.getStats, { filter }),
    placeholderData: (previous) => previous,
  })
}

/**
 * What each assistant backend can do right now. Probing runs a `--version` per CLI, so this is
 * deliberately not folded into `useSettings` — Settings would then pay for two process spawns on
 * every render of an unrelated toggle.
 */
export function useAiAvailability() {
  return useQuery({
    queryKey: queryKeys.ai.availability(),
    queryFn: async () => invokeCommand<AiAvailability[]>(IPC_COMMANDS.aiAvailability),
    staleTime: 30_000,
  })
}

// ─── Writes ─────────────────────────────────────────────────────────────────

/**
 * Every mutation here invalidates rather than writing into the cache by hand. The scheduler mutates
 * the same rows from another thread, so an optimistic local edit would be guessing at state Rust
 * already owns.
 */
function useInvalidating<TArgs, TResult>(
  command: (typeof IPC_COMMANDS)[keyof typeof IPC_COMMANDS],
  domains: ReadonlyArray<readonly string[]>,
  toArgs: (input: TArgs) => Record<string, unknown>,
): UseMutationResult<TResult, Error, TArgs> {
  const client = useQueryClient()
  return useMutation({
    mutationFn: async (input: TArgs) => invokeCommand<TResult>(command, toArgs(input)),
    onSuccess: () => {
      for (const domain of domains) {
        void client.invalidateQueries({ queryKey: domain })
      }
    },
  })
}

export function useConnectAccount() {
  // Resolves as soon as the flow STARTS; the outcome arrives on `windbag://auth`,
  // because an OAuth handoff outlives any command that could return it.
  return useInvalidating<{ platform: PlatformId; fields: Record<string, string> }, Nothing>(
    IPC_COMMANDS.connectAccount,
    [],
    (input) => ({ platform: input.platform, fields: input.fields }),
  )
}

export function useDeliverAuthCallback() {
  // Finishes the sign-in already waiting; its outcome arrives on `windbag://auth`
  // like any other connect.
  return useInvalidating<string, Nothing>(IPC_COMMANDS.deliverAuthCallback, [], (url) => ({
    url,
  }))
}

export function useDisconnectAccount() {
  return useInvalidating<number, Nothing>(
    IPC_COMMANDS.disconnectAccount,
    [queryKeys.accounts.root, queryKeys.queue.root],
    (id) => ({ id }),
  )
}

export function useSaveAppCredentials() {
  return useInvalidating<
    {
      platform: PlatformId
      instance?: string | null
      clientId: string
      clientSecret?: string | null
      extra?: Record<string, string>
    },
    Nothing
  >(IPC_COMMANDS.saveAppCredentials, [queryKeys.platforms.root], (input) => ({
    platform: input.platform,
    instance: input.instance ?? null,
    clientId: input.clientId,
    clientSecret: input.clientSecret ?? null,
    extra: input.extra ?? {},
  }))
}

/**
 * Saving invalidates the whole platforms root, not just the web host: the three Meta adapters
 * report their redirect URI from this value, so `list_platforms` is stale the moment it changes.
 */
export function useSaveWebHost() {
  return useInvalidating<{ baseUrl: string; uploadToken?: string | null }, WebHostView>(
    IPC_COMMANDS.saveWebHost,
    [queryKeys.platforms.root],
    (input) => ({ baseUrl: input.baseUrl, uploadToken: input.uploadToken ?? null }),
  )
}

export function useForgetWebHost() {
  return useInvalidating<Record<string, never>, Nothing>(
    IPC_COMMANDS.forgetWebHost,
    [queryKeys.platforms.root],
    () => ({}),
  )
}

export function useForgetAppCredentials() {
  return useInvalidating<{ platform: PlatformId; instance?: string | null }, Nothing>(
    IPC_COMMANDS.forgetAppCredentials,
    [queryKeys.platforms.root],
    (input) => ({ platform: input.platform, instance: input.instance ?? null }),
  )
}

export function useSavePost() {
  return useInvalidating<SavePostInput, number>(
    IPC_COMMANDS.savePost,
    [queryKeys.queue.root],
    (input) => ({ input }),
  )
}

export function useDeletePost() {
  return useInvalidating<number, Nothing>(
    IPC_COMMANDS.deletePost,
    [queryKeys.queue.root],
    (id) => ({
      id,
    }),
  )
}

export function usePublishNow() {
  return useInvalidating<number, Nothing>(
    IPC_COMMANDS.publishNow,
    [queryKeys.queue.root],
    (id) => ({
      id,
    }),
  )
}

export function useReschedulePost() {
  return useInvalidating<{ id: number; scheduledAt: string }, Nothing>(
    IPC_COMMANDS.reschedulePost,
    [queryKeys.queue.root],
    (input) => ({ id: input.id, scheduledAt: input.scheduledAt }),
  )
}

export function useRetryTarget() {
  return useInvalidating<number, Nothing>(
    IPC_COMMANDS.retryTarget,
    [queryKeys.queue.root],
    (targetId) => ({ targetId }),
  )
}

export function useUpdateSettings() {
  return useInvalidating<Settings, Settings>(
    IPC_COMMANDS.updateSettings,
    [queryKeys.settings.root],
    (settings) => ({ settings }),
  )
}

export function useSaveNote() {
  return useInvalidating<SaveNoteInput, number>(
    IPC_COMMANDS.saveNote,
    [queryKeys.notes.root],
    (input) => ({ input }),
  )
}

export function useDeleteNote() {
  return useInvalidating<number, Nothing>(
    IPC_COMMANDS.deleteNote,
    [queryKeys.notes.root],
    (id) => ({
      id,
    }),
  )
}

/**
 * Asks the assistant for drafts.
 *
 * Deliberately a mutation and not a query: it spends real time and, on the CLI backends, a real
 * quota. Nothing should ever re-run it on a refetch.
 */
export function useSuggestPosts() {
  return useMutation({
    mutationFn: async (input: {
      context: string
      instructions: string
      noteIds: number[]
      accountIds: number[]
      count: number
    }) => invokeCommand<Suggestion[]>(IPC_COMMANDS.suggestPosts, input),
  })
}

/**
 * What a refresh would read, so the button that spends money on X can say how much first.
 *
 * A plain read at the moment of the click rather than a cached query: a count that is stale, still
 * loading or failed must never stand in for the real one, and this REJECTS when the count cannot be
 * read — so the caller aborts instead of treating an unknown cost as zero and spending unasked.
 */
export async function readRefreshCost(): Promise<RefreshCost> {
  return invokeCommand<RefreshCost>(IPC_COMMANDS.getRefreshCost)
}

export function useRefreshEngagement() {
  const client = useQueryClient()
  return useMutation({
    mutationFn: async () => invokeCommand<RefreshReport>(IPC_COMMANDS.refreshEngagement),
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: queryKeys.stats.root })
    },
  })
}

export function useResolveMedia() {
  return useMutation({
    mutationFn: async (paths: string[]) =>
      invokeCommand<ResolvedMedia[]>(IPC_COMMANDS.resolveMedia, { paths }),
  })
}
