import * as React from 'react'

/** The imperative surface every icon under `components/icons` exposes through its ref. */
export interface AnimatedIconHandle {
  startAnimation: () => void
  stopAnimation: () => void
}

/**
 * Drives an animated icon from the control that contains it. Left alone, the icons animate only
 * while the pointer is over the glyph itself; spreading the handlers onto the button or link makes
 * the whole control the trigger, and keyboard focus gets the same cue.
 *
 * Returns `[ref, handlers]`: the ref goes on the icon, the handlers on its control.
 */
export function useAnimatedIcon() {
  const ref = React.useRef<AnimatedIconHandle>(null)
  const handlers = React.useMemo(
    () => ({
      onMouseEnter: () => ref.current?.startAnimation(),
      onMouseLeave: () => ref.current?.stopAnimation(),
      onFocus: () => ref.current?.startAnimation(),
      onBlur: () => ref.current?.stopAnimation(),
    }),
    [],
  )
  return [ref, handlers] as const
}
