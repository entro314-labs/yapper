/**
 * The IPC contract, mirroring the Rust types in `src-tauri/src`.
 *
 * Hand-written rather than generated: the surface is small enough that a codegen step would cost
 * more than it saves, and every type here is exercised by a real screen. Each block names the Rust
 * struct it mirrors — change one and the other has to follow.
 */

/** `platforms::PlatformId` */
export type PlatformId = 'bluesky' | 'mastodon' | 'reddit' | 'x' | 'linkedin'

/** `platforms::AuthKind` */
export type AuthKind = 'credentials' | 'oAuth2'

/** `platforms::FieldSpec` */
export interface FieldSpec {
  key: string
  label: string
  placeholder: string
  help: string
  secret: boolean
  required: boolean
}

/** `platforms::Limits` */
export interface Limits {
  maxChars: number
  maxMedia: number
  supportsAltText: boolean
  requiresTitle: boolean
}

/** `platforms::PlatformInfo` */
export interface PlatformInfo {
  id: PlatformId
  name: string
  auth: AuthKind
  limits: Limits
  connectFields: FieldSpec[]
  appFields: FieldSpec[]
  setupUrl: string | null
  redirectUri: string | null
  targetFields: FieldSpec[]
  notes: string
}

/** `db::Account`. `status` is `ok` until a publish gets a 401. */
export interface Account {
  id: number
  platform: PlatformId
  remoteId: string
  handle: string
  displayName: string | null
  avatarUrl: string | null
  instance: string | null
  scopes: string | null
  charLimit: number | null
  tokenExpiresAt: string | null
  status: 'ok' | 'needs_reauth'
  createdAt: string
}

export type PostStatus =
  | 'draft'
  | 'scheduled'
  | 'publishing'
  | 'published'
  | 'partial'
  | 'failed'
  | 'missed'

export type TargetStatus = 'pending' | 'publishing' | 'published' | 'failed'

/** `db::PostTarget` */
export interface PostTarget {
  id: number
  postId: number
  accountId: number
  options: Record<string, string> | null
  status: TargetStatus
  remoteId: string | null
  remoteUrl: string | null
  error: string | null
  attempts: number
  nextAttemptAt: string | null
  publishedAt: string | null
}

/** `db::Media` */
export interface Media {
  id: number
  postId: number
  path: string
  mime: string
  bytes: number
  altText: string | null
  position: number
}

/** `db::PostDetail` — the post's own fields are flattened into this object. */
export interface PostDetail {
  id: number
  body: string
  title: string | null
  link: string | null
  /** RFC 3339 UTC. `null` on a draft. */
  scheduledAt: string | null
  status: PostStatus
  createdAt: string
  updatedAt: string
  targets: PostTarget[]
  media: Media[]
}

/** `db::Attempt` */
export interface Attempt {
  id: number
  targetId: number
  at: string
  ok: boolean
  detail: string | null
}

/** `commands::TargetCheck` — one destination's verdict on the current draft. */
export interface TargetCheck {
  accountId: number
  platform: PlatformId
  handle: string
  used: number
  limit: number
  error: string | null
}

/** `commands::AppCredentialsView`. The secret is never sent back, only its presence. */
export interface AppCredentialsView {
  clientId: string
  hasSecret: boolean
  extra: Record<string, string>
}

/** `commands::ResolvedMedia` */
export interface ResolvedMedia {
  path: string
  mime: string
  bytes: number
}

/** `commands::Settings` */
export interface Settings {
  theme: 'system' | 'light' | 'dark'
  windowMaterial: 'off' | 'standard' | 'strong'
  /** `skip` marks an overdue post missed; `post_late` sends it anyway. */
  missedPolicy: 'skip' | 'post_late'
  graceMinutes: number
  launchAtLogin: boolean
}

/** `commands::AuthOutcome`, delivered on `yapper://auth`. */
export interface AuthOutcome {
  ok: boolean
  platform: PlatformId
  message: string
  account: Account | null
}

/** `commands::SavePostInput` */
export interface SavePostInput {
  id: number | null
  body: string
  title: string | null
  link: string | null
  scheduledAt: string | null
  targets: Array<{ accountId: number; options: Record<string, string> }>
  media: Array<{ path: string; altText: string | null }>
}
