import * as React from 'react'

import { useSettings } from '@/lib/query'
import { humanMessage, invokeCommand, subscribeEvent } from '@/lib/tauri/client'
import { IPC_COMMANDS, IPC_EVENTS } from '@/lib/tauri/ipc'
import type { UpdateInstallSupport, UpdateMeta, UpdateProgress } from '@/lib/tauri/types'
import { resolveUpdateChannel } from '@/lib/update-channel'

/**
 * The session's update state, owned in one place so the status bar and Settings can never disagree
 * about the same download.
 *
 * Not a query: this is a state machine with a long-running side effect, not a cached read. The one
 * fact that outlives a window reload — whether something is already staged — is re-asked from Rust
 * on mount, because the staged bundle lives in Rust's managed state, not here.
 */
export type UpdateState = 'idle' | 'checking' | 'available' | 'downloading' | 'staged' | 'error'

interface UpdatesValue {
  state: UpdateState
  meta: UpdateMeta | null
  progress: UpdateProgress | null
  /** The raw Rust message; `describeUpdateError` turns it into copy. */
  error: string | null
  /** `packageManager` installs cannot self-update — the UI must say so instead of offering a check. */
  support: UpdateInstallSupport | null
  /** Poll the channel. `manual` surfaces failures; a background check keeps them quiet. */
  check: (manual?: boolean) => Promise<void>
  download: () => Promise<void>
  restartNow: () => Promise<void>
}

const UpdatesContext = React.createContext<UpdatesValue | null>(null)

/**
 * How often a long-lived session re-asks. Windbag is meant to stay running, so launch-only would
 * mean a machine that never restarts is never told about a fix.
 */
const RECHECK_INTERVAL_MS = 24 * 60 * 60 * 1000

export function UpdatesProvider({ children }: { children: React.ReactNode }) {
  const settings = useSettings()
  const channelPref = settings.data?.updateChannel ?? 'auto'

  const [state, setState] = React.useState<UpdateState>('idle')
  const [meta, setMeta] = React.useState<UpdateMeta | null>(null)
  const [progress, setProgress] = React.useState<UpdateProgress | null>(null)
  const [error, setError] = React.useState<string | null>(null)
  const [support, setSupport] = React.useState<UpdateInstallSupport | null>(null)

  const check = React.useCallback(
    async (manual = false) => {
      // A download already in flight, or a bundle already waiting for the quit,
      // both outrank a fresh check — re-checking would throw away either.
      setState((current) => {
        if (current === 'downloading' || current === 'staged') return current
        return 'checking'
      })
      try {
        const channel = await resolveUpdateChannel(channelPref)
        const found = await invokeCommand<UpdateMeta | null>(IPC_COMMANDS.checkForUpdate, {
          channel,
        })
        setMeta(found)
        setError(null)
        setState((current) => {
          if (current === 'downloading' || current === 'staged') return current
          return found ? 'available' : 'idle'
        })
      } catch (err) {
        // A background check that fails offline is not news; a manual one is the
        // user asking a direct question and deserves an answer either way.
        setError(humanMessage(err))
        setState((current) => {
          if (current === 'downloading' || current === 'staged') return current
          return manual ? 'error' : 'idle'
        })
      }
    },
    [channelPref],
  )

  const download = React.useCallback(async () => {
    setProgress(null)
    setError(null)
    setState('downloading')
    try {
      const channel = await resolveUpdateChannel(channelPref)
      const installed = await invokeCommand<UpdateMeta>(IPC_COMMANDS.installUpdate, { channel })
      setMeta(installed)
      setState('staged')
    } catch (err) {
      setError(humanMessage(err))
      setState('error')
    }
  }, [channelPref])

  const restartNow = React.useCallback(async () => {
    try {
      await invokeCommand<null>(IPC_COMMANDS.restartAndInstall)
    } catch (err) {
      setError(humanMessage(err))
      setState('error')
    }
  }, [])

  // Whether the in-app updater can service this install at all. A deb or Flatpak
  // is owned by its package manager, and every other effect below is pointless
  // there — so this gate runs first and the launch check hangs off its answer.
  React.useEffect(() => {
    let cancelled = false
    void (async () => {
      try {
        const answer = await invokeCommand<UpdateInstallSupport>(IPC_COMMANDS.updateInstallSupport)
        if (!cancelled) setSupport(answer)
      } catch {
        // Off a Tauri runtime the command does not exist. Reporting "unsupported"
        // would be a lie about the shipped app; staying null hides the surface.
      }
    })()
    return () => {
      cancelled = true
    }
  }, [])

  // A window reload drops this component's state but not Rust's: a bundle
  // downloaded before the reload is still waiting for the quit, and the UI has
  // to keep offering the restart rather than pretending nothing happened.
  React.useEffect(() => {
    void (async () => {
      try {
        if (await invokeCommand<boolean>(IPC_COMMANDS.updateStaged)) setState('staged')
      } catch {
        // No runtime: nothing was staged either.
      }
    })()
  }, [])

  React.useEffect(() => {
    let detach: (() => void) | null = null
    void (async () => {
      detach = await subscribeEvent<UpdateProgress>(IPC_EVENTS.updateProgress, setProgress)
    })()
    return () => {
      detach?.()
    }
  }, [])

  React.useEffect(() => {
    if (support !== 'supported') return
    void check()
    const timer = window.setInterval(() => {
      void check()
    }, RECHECK_INTERVAL_MS)
    return () => {
      window.clearInterval(timer)
    }
  }, [support, check])

  const value = React.useMemo(
    () => ({ state, meta, progress, error, support, check, download, restartNow }),
    [state, meta, progress, error, support, check, download, restartNow],
  )
  return <UpdatesContext.Provider value={value}>{children}</UpdatesContext.Provider>
}

export function useUpdates(): UpdatesValue {
  const context = React.useContext(UpdatesContext)
  if (!context) throw new Error('useUpdates must be used inside an UpdatesProvider')
  return context
}
