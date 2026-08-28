import { QueryClient } from '@tanstack/react-query'

import { errorCode } from '@/lib/tauri/client'

/**
 * One shared QueryClient. Reads are cheap (local SQLite over IPC) so the stale time is short and
 * the event bridge pushes invalidation when Rust says something changed — polling is never the
 * mechanism.
 */
export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 20_000,
      gcTime: 5 * 60_000,
      retry: (failureCount, error) => {
        // These are the codes nothing fixes by trying again: a missing row, a
        // rejected input, dead credentials, a contradicted state. Retrying them
        // only delays the message the user needs to read.
        const code = errorCode(error)
        if (
          code === 'NOT_FOUND' ||
          code === 'INVALID_INPUT' ||
          code === 'UNAUTHORIZED' ||
          code === 'CONFLICT'
        ) {
          return false
        }
        return failureCount < 2
      },
      retryDelay: (attempt) => Math.min(1000 * 2 ** attempt, 8000),
      refetchOnWindowFocus: false,
    },
    mutations: { retry: 0 },
  },
})
