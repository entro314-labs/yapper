import { getCurrentWindow } from '@tauri-apps/api/window'
import * as React from 'react'

/**
 * Whether the window is fullscreen — the one state that moves the macOS traffic lights, because the
 * OS hides them there and the chrome that was clearing them would otherwise sit on empty space.
 *
 * Tauri has no fullscreen event, so this rides `onResized`: entering or leaving always resizes.
 */
export function useIsFullscreen(): boolean {
  const [fullscreen, setFullscreen] = React.useState(false)

  React.useEffect(() => {
    let detach: (() => void) | null = null
    let cancelled = false
    const appWindow = getCurrentWindow()

    const sync = async () => {
      const current = await appWindow.isFullscreen()
      if (!cancelled) setFullscreen(current)
    }

    void sync()
    void (async () => {
      detach = await appWindow.onResized(() => {
        void sync()
      })
    })()

    return () => {
      cancelled = true
      detach?.()
    }
  }, [])

  return fullscreen
}
