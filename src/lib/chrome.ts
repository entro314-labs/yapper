/**
 * Window-chrome constants. Shared because the sidebar header, the pane titlebar and the macOS
 * traffic lights all have to agree on one band height — a mismatch of a couple of pixels is
 * instantly visible as a stepped seam.
 */
export const APP_NAME = 'Windbag'

/** MacOS draws its own traffic lights; the other two need ours. */
export const IS_MACOS =
  typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform ?? '')

/**
 * The macOS title-bar band. AppKit places the traffic lights itself — Rust installs an empty
 * unified toolbar for exactly that (windowing.rs), rather than pinning them from tauri.conf.json —
 * and centres them in the standard 52px toolbar-window title bar, so the lights' centre line is y = 26.
 * Making the top strips BE that band is what puts them on the same centre; anything shorter, flush
 * to the window's top edge, leaves the lights riding low in it.
 */
const MAC_TITLEBAR_BAND = 52

/**
 * Width the traffic lights occupy from the window's left edge: measured on macOS 27, 14px buttons
 * on a 23px pitch starting at x = 19, so the green button's right edge sits at 79. (macOS 14/15
 * draws 12px buttons on a 20px pitch ending ~72 — there the clearances below simply run ~8px
 * looser; sizing to the larger, current metric is what keeps the green light clear of the seam.)
 */
const MAC_TRAFFIC_LIGHTS_W = 80

/**
 * Leading inset for anything at the left edge of a top drag region on macOS. Clears the lights with
 * a real 16px gap rather than butting the wordmark against the green one. In fullscreen the OS
 * hides them, so only a small gutter is left.
 */
export const MAC_TITLEBAR_INSET_LEFT = MAC_TRAFFIC_LIGHTS_W + 16
export const MAC_TITLEBAR_INSET_LEFT_FULLSCREEN = 12

/** Off macOS the band carries no lights, so it keeps the tighter height. */
export const TITLEBAR_H = IS_MACOS ? MAC_TITLEBAR_BAND : 44

export const SIDEBAR_DEFAULT_W = 236
export const SIDEBAR_MIN_W = 190
export const SIDEBAR_MAX_W = 340
/**
 * The rail is the window's leading chrome, so on macOS it has to be wide enough for the lights to
 * live entirely inside its column — at 56 the green one would straddle the rail / island seam.
 */
export const SIDEBAR_RAIL_W = IS_MACOS ? MAC_TRAFFIC_LIGHTS_W + 4 : 56
