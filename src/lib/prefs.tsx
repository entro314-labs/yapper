import * as React from 'react'

import { SIDEBAR_DEFAULT_W } from '@/lib/chrome'

/**
 * View state that belongs to this machine rather than to the account: how wide the sidebar is,
 * whether it is a rail. Deliberately NOT in the SQLite store — it has no bearing on what gets
 * posted, and round-tripping it through IPC on every drag frame would be absurd.
 */
export type SidebarMode = 'full' | 'rail'

interface PrefsValue {
  sidebarMode: SidebarMode
  setSidebarMode: (mode: SidebarMode) => void
  sidebarWidth: number
  setSidebarWidth: (width: number) => void
}

const PrefsContext = React.createContext<PrefsValue | null>(null)

const STORAGE_KEY = 'yapper.prefs'

interface Stored {
  sidebarMode?: SidebarMode
  sidebarWidth?: number
}

function read(): Stored {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY)
    return raw ? (JSON.parse(raw) as Stored) : {}
  } catch {
    // A private window or blocked site data: defaults are perfectly usable.
    return {}
  }
}

export function PrefsProvider({ children }: { children: React.ReactNode }) {
  // Lazy initialisers: `read()` touches localStorage, which can throw, and doing
  // it once at mount keeps that out of every subsequent render.
  const [sidebarMode, setSidebarMode] = React.useState<SidebarMode>(
    () => read().sidebarMode ?? 'full',
  )
  const [sidebarWidth, setSidebarWidth] = React.useState<number>(
    () => read().sidebarWidth ?? SIDEBAR_DEFAULT_W,
  )

  React.useEffect(() => {
    try {
      window.localStorage.setItem(STORAGE_KEY, JSON.stringify({ sidebarMode, sidebarWidth }))
    } catch {
      // Losing a remembered width is not worth surfacing.
    }
  }, [sidebarMode, sidebarWidth])

  // Written as a custom property rather than an inline style so the seam handle
  // and the sidebar can both read the live value mid-drag without re-rendering.
  React.useEffect(() => {
    document.documentElement.style.setProperty('--pane-sidebar-w', `${sidebarWidth}px`)
  }, [sidebarWidth])

  const value = React.useMemo(
    () => ({ sidebarMode, setSidebarMode, sidebarWidth, setSidebarWidth }),
    [sidebarMode, sidebarWidth],
  )
  return <PrefsContext.Provider value={value}>{children}</PrefsContext.Provider>
}

export function usePrefs(): PrefsValue {
  const context = React.useContext(PrefsContext)
  if (!context) throw new Error('usePrefs must be used inside a PrefsProvider')
  return context
}
