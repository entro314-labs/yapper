import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import type { UseMutationResult } from '@tanstack/react-query'

import { invokeCommand } from '@/lib/tauri/client'
import { IPC_COMMANDS } from '@/lib/tauri/ipc'
import type {
  Account,
  AppCredentialsView,
  Attempt,
  PlatformId,
  PlatformInfo,
  PostDetail,
  ResolvedMedia,
  SavePostInput,
  Settings,
  TargetCheck,
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

/**
 * The composer's live verdict per destination. Runs the SAME validation the scheduler will run, so
 * a counter that says a post fits is a promise the backend keeps.
 */
export function useCheckPost(
  body: string,
  title: string | null,
  mediaCount: number,
  accountIds: number[],
) {
  return useQuery({
    queryKey: queryKeys.queue.check(body, title, mediaCount, accountIds),
    queryFn: async () =>
      invokeCommand<TargetCheck[]>(IPC_COMMANDS.checkPost, {
        body,
        title,
        mediaCount,
        accountIds,
      }),
    enabled: accountIds.length > 0,
    // A draft is only ever checked against what is on screen right now; keeping
    // old verdicts around would let a stale "fits" survive an edit.
    gcTime: 0,
    placeholderData: (previous) => previous,
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
  // Resolves as soon as the flow STARTS; the outcome arrives on `yapper://auth`,
  // because an OAuth handoff outlives any command that could return it.
  return useInvalidating<{ platform: PlatformId; fields: Record<string, string> }, Nothing>(
    IPC_COMMANDS.connectAccount,
    [],
    (input) => ({ platform: input.platform, fields: input.fields }),
  )
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

export function useResolveMedia() {
  return useMutation({
    mutationFn: async (paths: string[]) =>
      invokeCommand<ResolvedMedia[]>(IPC_COMMANDS.resolveMedia, { paths }),
  })
}
