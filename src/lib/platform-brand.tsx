import {
  IconBrandBluesky,
  IconBrandFacebook,
  IconBrandInstagram,
  IconBrandLinkedin,
  IconBrandMastodon,
  IconBrandReddit,
  IconBrandThreads,
  IconBrandX,
} from '@tabler/icons-react'
import type * as React from 'react'

import type { PlatformId } from '@/lib/tauri/types'

/**
 * The per-platform identity: mark and brand tone.
 *
 * These tones are the ONLY colours in the app that are not from the palette, and they exist because
 * a queue mixing eight destinations has to be scannable without reading a label. They are defined
 * per theme in `styles/themes.css` — LinkedIn's navy and X's black are both invisible on the dark
 * surfaces, so each has a light and a dark value rather than one fixed hex.
 *
 * Threads, Instagram and Facebook share a developer app but not a row in the queue, so they get
 * three separate marks and three well-separated tones.
 */
interface Brand {
  icon: React.ElementType
  /** A CSS custom property, so the value follows the active theme. */
  tone: string
}

const PLATFORM_BRAND: Record<PlatformId, Brand> = {
  bluesky: { icon: IconBrandBluesky, tone: 'var(--brand-bluesky)' },
  mastodon: { icon: IconBrandMastodon, tone: 'var(--brand-mastodon)' },
  reddit: { icon: IconBrandReddit, tone: 'var(--brand-reddit)' },
  x: { icon: IconBrandX, tone: 'var(--brand-x)' },
  linkedin: { icon: IconBrandLinkedin, tone: 'var(--brand-linkedin)' },
  threads: { icon: IconBrandThreads, tone: 'var(--brand-threads)' },
  instagram: { icon: IconBrandInstagram, tone: 'var(--brand-instagram)' },
  facebook: { icon: IconBrandFacebook, tone: 'var(--brand-facebook)' },
}

export function brandOf(platform: PlatformId): Brand {
  return PLATFORM_BRAND[platform]
}
