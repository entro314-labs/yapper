import * as React from 'react'

import { invokeCommand } from '@/lib/tauri/client'
import { IPC_COMMANDS } from '@/lib/tauri/ipc'
import type { Settings } from '@/lib/tauri/types'

type Theme = Settings['theme']
type Material = Settings['windowMaterial']

interface ThemeContextValue {
  theme: Theme
  resolvedTheme: 'light' | 'dark'
  /** What the OS ACTUALLY applied, which on Linux is routinely `off`. */
  material: Material
  setTheme: (theme: Theme) => void
  setMaterial: (material: Material) => void
}

const ThemeContext = React.createContext<ThemeContextValue | null>(null)

/**
 * Paints the theme onto `<html>` BEFORE React renders.
 *
 * The window is transparent and starts hidden; Rust shows it once the renderer has painted. If the
 * class landed in an effect instead, the first frame would be the light palette on a dark desktop —
 * a white flash on every launch.
 */
function stamp(theme: Theme): 'light' | 'dark' {
  const resolved =
    theme === 'system'
      ? window.matchMedia('(prefers-color-scheme: dark)').matches
        ? 'dark'
        : 'light'
      : theme
  document.documentElement.classList.toggle('dark', resolved === 'dark')
  document.documentElement.style.colorScheme = resolved
  return resolved
}

export function ThemeProvider({
  settings,
  children,
}: {
  settings: Settings | undefined
  children: React.ReactNode
}) {
  const theme = settings?.theme ?? 'system'
  const requested = settings?.windowMaterial ?? 'standard'

  const [resolvedTheme, setResolvedTheme] = React.useState<'light' | 'dark'>(() => stamp(theme))
  const [material, setMaterialState] = React.useState<Material>('off')

  // Re-stamping during render rather than in an effect: the class has to be on
  // `<html>` before the browser paints, and an effect runs after. The stamp is
  // idempotent, so doing it on a render that changes nothing costs nothing.
  const stamped = stamp(theme)
  if (stamped !== resolvedTheme) setResolvedTheme(stamped)

  React.useEffect(() => {
    // Only `system` follows the OS. Watching unconditionally would fight an
    // explicit choice every time the desktop flipped.
    if (theme !== 'system') return
    const query = window.matchMedia('(prefers-color-scheme: dark)')
    const onChange = () => {
      setResolvedTheme(stamp('system'))
    }
    query.addEventListener('change', onChange)
    return () => {
      query.removeEventListener('change', onChange)
    }
  }, [theme])

  const applyMaterial = React.useCallback(async (next: Material) => {
    // Stamped from what the OS DID, never from what was asked for: a see-through
    // sidebar with nothing frosting behind it shows the desktop wallpaper
    // straight through the queue.
    let applied: Material = 'off'
    try {
      applied = await invokeCommand<Material>(IPC_COMMANDS.setWindowMaterial, {
        material: next,
      })
    } catch {
      // A platform that cannot do it is the same outcome as one that refused.
      applied = 'off'
    }
    document.documentElement.dataset.material = applied
    setMaterialState(applied)
  }, [])

  React.useEffect(() => {
    void applyMaterial(requested)
  }, [requested, applyMaterial])

  const value = React.useMemo(
    () => ({
      theme,
      resolvedTheme,
      material,
      setTheme: (next: Theme) => {
        setResolvedTheme(stamp(next))
      },
      setMaterial: (next: Material) => {
        void applyMaterial(next)
      },
    }),
    [theme, resolvedTheme, material, applyMaterial],
  )

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>
}

export function useTheme(): ThemeContextValue {
  const context = React.useContext(ThemeContext)
  if (!context) throw new Error('useTheme must be used inside a ThemeProvider')
  return context
}
