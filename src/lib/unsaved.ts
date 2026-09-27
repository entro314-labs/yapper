import { useBlocker } from '@tanstack/react-router'
import { ask } from '@tauri-apps/plugin-dialog'
import * as React from 'react'

/**
 * Keeps unsaved edits from being dropped silently. While `dirty`, leaving the route asks first, and
 * `confirmDiscard` asks the same question for an in-screen action that would replace the edits
 * (opening another note, starting a new one).
 *
 * One pattern everywhere — ask rather than auto-save on the way out — because only the person knows
 * whether the edits are worth keeping, and saving a half-rewritten post or note behind their back
 * would overwrite the version they may have wanted. (An action whose whole point is the saved
 * version, like a note's "Draft from this", saves as part of the click instead.)
 *
 * `allowNextNavigation` is for leaving on purpose right after a save: the saved state has not
 * re-rendered as clean yet, and asking "discard?" about something just saved would be wrong.
 */
export function useUnsavedGuard(dirty: boolean, what: string) {
  const bypass = React.useRef(false)

  const confirmDiscard = React.useCallback(async () => {
    if (!dirty) return true
    // The native dialog rather than `window.confirm`, whose webview version
    // resolves to a (truthy) Promise whatever is clicked.
    return ask(`Your changes to ${what} have not been saved.`, {
      title: 'Discard unsaved changes?',
      kind: 'warning',
      okLabel: 'Discard',
      cancelLabel: 'Keep editing',
    })
  }, [dirty, what])

  const shouldBlockFn = React.useCallback(async () => {
    if (bypass.current) {
      bypass.current = false
      return false
    }
    return !(await confirmDiscard())
  }, [confirmDiscard])

  useBlocker({
    shouldBlockFn,
    // The only unload a desktop app has is quitting, and a webview's
    // beforeunload prompt is not something the native shell will show.
    enableBeforeUnload: false,
  })

  const allowNextNavigation = React.useCallback(() => {
    bypass.current = true
  }, [])

  return { confirmDiscard, allowNextNavigation }
}
