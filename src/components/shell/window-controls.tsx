import { IconMinus, IconSquare, IconX } from '@tabler/icons-react'
import { getCurrentWindow } from '@tauri-apps/api/window'

import { IS_MACOS } from '@/lib/chrome'
import { cn } from '@/lib/utils'

/**
 * Minimise / maximise / close on the platforms whose system frame we removed.
 *
 * They carry the desktop's own three colours rather than Windbag's palette: close is red
 * everywhere, and a user whose OS accent is cyan still expects that. macOS renders nothing here —
 * it keeps the real traffic lights, which AppKit places in the title-bar band the sidebar header is
 * sized to (see windowing.rs and lib/chrome.ts).
 */
export function WindowControls({ className }: { className?: string }) {
  if (IS_MACOS) return null

  const window = getCurrentWindow()
  return (
    <div className={cn('flex items-center', className)}>
      <button
        type="button"
        aria-label="Minimise"
        onClick={() => {
          void window.minimize()
        }}
        className="window-control window-control-min grid h-8 w-11 place-items-center"
      >
        <IconMinus className="size-3.5" />
      </button>
      <button
        type="button"
        aria-label="Maximise"
        onClick={() => {
          void window.toggleMaximize()
        }}
        className="window-control window-control-max grid h-8 w-11 place-items-center"
      >
        <IconSquare className="size-3" />
      </button>
      <button
        type="button"
        aria-label="Close"
        // Closing HIDES the window: the scheduler has to keep running, so Rust
        // intercepts the close request and hides instead of quitting.
        onClick={() => {
          void window.close()
        }}
        className="window-control window-control-close grid h-8 w-11 place-items-center"
      >
        <IconX className="size-3.5" />
      </button>
    </div>
  )
}
