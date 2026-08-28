/**
 * Window-chrome constants. Shared because the sidebar header, the pane titlebar and the macOS
 * traffic lights all have to agree on one band height — a mismatch of a couple of pixels is
 * instantly visible as a stepped seam.
 */
export const APP_NAME = 'Yapper'

export const TITLEBAR_H = 44
/**
 * Clears the macOS traffic lights, whose position is set in tauri.conf.json
 * (x: 18). The three buttons end around 77px; this leaves a real gap after them
 * rather than butting the wordmark against the green one.
 */
export const TITLEBAR_INSET_LEFT = 94

export const SIDEBAR_DEFAULT_W = 236
export const SIDEBAR_MIN_W = 190
export const SIDEBAR_MAX_W = 340
export const SIDEBAR_RAIL_W = 56

/** MacOS draws its own traffic lights; the other two need ours. */
export const IS_MACOS =
  typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform ?? '')
