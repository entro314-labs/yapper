/**
 * Update-channel resolution and error classification — the pure half of the updater, shared by
 * Settings and the status bar so the two can never disagree about the same download.
 *
 * The tag suffix of the RUNNING version picks the default channel: an alpha build must poll the
 * alpha manifest out of the box, because the stable endpoint 404s until a stable release exists. An
 * explicit choice in Settings always wins over the derivation.
 */

import type { UpdateProgress } from '@/lib/tauri/types'

export type UpdateChannel = 'stable' | 'beta' | 'alpha'

function deriveUpdateChannel(version: string): UpdateChannel {
  if (version.includes('-alpha')) return 'alpha'
  if (version.includes('-beta')) return 'beta'
  return 'stable'
}

/** Effective channel for a check: an explicit preference wins; `auto` derives from the app version. */
export async function resolveUpdateChannel(pref: string): Promise<UpdateChannel> {
  if (pref === 'stable' || pref === 'beta' || pref === 'alpha') return pref
  try {
    const { getVersion } = await import('@tauri-apps/api/app')
    return deriveUpdateChannel(await getVersion())
  } catch {
    // No Tauri runtime (a plain `vite dev` browser tab): the channel is cosmetic there.
    return 'stable'
  }
}

/**
 * The releases host itself did not answer. Rust's `check_channel` (`src-tauri/src/update.rs`)
 * re-asks the releases repository whenever the updater plugin reports an unreadable manifest, and
 * stamps this marker when the repo is missing, private or unreachable — the plugin flattens both
 * cases into the same "Could not fetch a valid release JSON" string. A dead pipeline is NOT a
 * pre-first-release state, so this has to be tested before {@link isNoReleaseYet} can absorb it.
 */
function isUpdateSourceUnreachable(message: string): boolean {
  return /update source unreachable/i.test(message)
}

/**
 * The updater plugin reports a channel with no published release as a fetch/parse error. That is a
 * calm pre-first-release state, not a failure the user can act on — but only once Rust has
 * confirmed the releases repo actually exists.
 */
function isNoReleaseYet(message: string): boolean {
  if (isUpdateSourceUnreachable(message)) return false
  return /could not fetch a valid release json|release not found|404/i.test(message)
}

export interface UpdateErrorInfo {
  title: string
  detail: string
  /** False where retrying is useless or unsafe — a bad signature will not become a good one. */
  retryable: boolean
}

/**
 * Map a raw update failure onto copy a person can act on. Substring-based on purpose: the updater
 * plugin only ever surfaces stringly-typed errors over the wire.
 */
export function describeUpdateError(message: string): UpdateErrorInfo {
  const lower = message.toLowerCase()
  if (lower.includes('signature') || lower.includes('minisign')) {
    return {
      title: 'That download could not be verified',
      detail:
        'The bundle’s signature did not match Windbag’s release key, so it was discarded. Download the release from GitHub instead.',
      retryable: false,
    }
  }
  if (isUpdateSourceUnreachable(lower)) {
    return {
      title: 'The update service is unreachable',
      detail:
        'Windbag could not reach its releases repository at all. Either this machine is offline, or the update pipeline is broken — this is not the same as there being nothing new.',
      retryable: true,
    }
  }
  if (/error sending request|connection|timed? ?out|dns|network/.test(lower)) {
    return {
      title: 'Could not reach the update server',
      detail: 'Check the connection and try again.',
      retryable: true,
    }
  }
  if (isNoReleaseYet(lower)) {
    return {
      title: 'Nothing published on this channel yet',
      detail: 'The releases repository is reachable; this channel simply has no release on it.',
      retryable: true,
    }
  }
  if (lower.includes('no space') || lower.includes('disk')) {
    return {
      title: 'Not enough disk space',
      detail: 'Free some space and try the download again.',
      retryable: true,
    }
  }
  return { title: 'The update check failed', detail: message, retryable: true }
}

/** Format a byte count for update copy ("42.3 MB"); nothing in, nothing out. */
export function formatUpdateSize(bytes: number | null | undefined): string | undefined {
  if (bytes === null || bytes === undefined || !Number.isFinite(bytes) || bytes < 0) {
    return undefined
  }
  const mb = bytes / (1024 * 1024)
  return mb >= 1024 ? `${(mb / 1024).toFixed(2)} GB` : `${mb.toFixed(1)} MB`
}

/**
 * Download percentage, or null when there is nothing honest to show — no download in flight, or a
 * server that sent no Content-Length, which makes the download indeterminate rather than at 0%.
 */
export function updateProgressPercent(progress: UpdateProgress | null): number | null {
  if (!progress) return null
  const { downloaded, total } = progress
  if (total === null || total <= 0) return null
  return Math.min(100, Math.round((downloaded / total) * 100))
}

export const UPDATE_CHANNEL_LABELS: Record<'auto' | UpdateChannel, string> = {
  auto: 'Match this build',
  stable: 'Stable',
  beta: 'Beta',
  alpha: 'Alpha',
}
