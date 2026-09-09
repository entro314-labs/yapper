import { stagger } from 'motion/react'
import type { Variants } from 'motion/react'

/**
 * Route transitions. Short and vertical: the shell never moves, so the only thing that should read
 * as changing is the content column.
 *
 * `prefers-reduced-motion` is honoured by the `MotionConfig` at the root, which drops the movement
 * and keeps the fade — these variants do not need to check it themselves.
 */
export const pageVariants: Variants = {
  initial: { opacity: 0, y: 6 },
  animate: {
    opacity: 1,
    y: 0,
    transition: { duration: 0.22, ease: [0.22, 1, 0.36, 1] },
  },
  exit: {
    opacity: 0,
    y: -4,
    transition: { duration: 0.14, ease: [0.4, 0, 1, 1] },
  },
}

/** A list that reveals itself top-down rather than all at once. */
export const listVariants: Variants = {
  animate: { transition: { delayChildren: stagger(0.03) } },
}

export const rowVariants: Variants = {
  initial: { opacity: 0, y: 4 },
  animate: { opacity: 1, y: 0, transition: { duration: 0.2, ease: [0.22, 1, 0.36, 1] } },
}
