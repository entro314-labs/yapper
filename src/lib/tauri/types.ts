/**
 * The IPC contract, mirroring the Rust types in `src-tauri/src`.
 *
 * Hand-written rather than generated: the surface is small enough that a codegen step would cost
 * more than it saves, and every type here is exercised by a real screen. Each block names the Rust
 * struct it mirrors — change one and the other has to follow.
 */

/** `platforms::PlatformId` */
export type PlatformId =
  | 'bluesky'
  | 'mastodon'
  | 'reddit'
  | 'x'
  | 'linkedin'
  | 'threads'
  | 'instagram'
  | 'facebook'

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
  /** A closed set of allowed values, drawn as a picker. Empty means free text. */
  choices: string[]
  /** Billed to the body's character budget (Mastodon's content warning). */
  counted: boolean
}

/** `platforms::MediaRule` — one attachment type a platform takes. */
export interface MediaRule {
  mime: string
  maxBytes: number
  /** Must be the only attachment on its post (a video on X, Mastodon or Facebook; a GIF on X). */
  alone: boolean
}

/** `platforms::MediaSpec` — an attachment as validation sees it: its type and size. */
export interface MediaSpec {
  mime: string
  bytes: number
}

/** `commands::TargetInput` — one destination and its per-destination options. */
export interface TargetInput {
  accountId: number
  options: Record<string, string>
}

/** `platforms::Limits` */
export interface Limits {
  maxChars: number
  maxMedia: number
  /** Every attachment type the adapter can post, with the platform's documented size cap. */
  accepts: MediaRule[]
  supportsAltText: boolean
  requiresTitle: boolean
  /** Instagram: a caption is never a post by itself, so the composer refuses one with no media. */
  requiresMedia: boolean
  /** Reddit and Facebook: a URL with no text is a whole post (a link submission, a link share). */
  linkIsContent: boolean
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

/**
 * `commands::WebHostView` — the companion deployment the three Meta platforms need.
 *
 * Meta refuses a loopback redirect and will not accept uploaded bytes, so one deployment supplies
 * both the HTTPS redirect and the public media URLs. The upload token is never sent back to the
 * renderer, only whether one is stored.
 */
export interface WebHostView {
  baseUrl: string
  hasToken: boolean
  /** Derived from `baseUrl`. This is the exact string to register on the Meta app. */
  redirectUri: string
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
  /** Off by default — the assistant surfaces stay hidden until it is switched on. */
  aiBackend: AiBackend
  /** Empty means the tool's own default model, which is usually the right one. */
  aiModel: string
  aiEffort: string
  /** Which release manifest the updater polls; `auto` derives it from this build's version. */
  updateChannel: 'auto' | 'stable' | 'beta' | 'alpha'
}

/** `update::UpdateMeta` */
export interface UpdateMeta {
  version: string
  /** The release's CHANGELOG section, as published by the release pipeline. */
  notes: string | null
  date: string | null
  /** Absent when the manifest predates the pipeline's `size` extension. */
  downloadSize: number | null
}

/**
 * `update::UpdateInstallSupport` — whether the in-app updater can replace THIS install. A deb, rpm
 * or Flatpak is owned by its package manager, and offering it a self-update it cannot perform is
 * worse than saying so.
 */
export type UpdateInstallSupport = 'supported' | 'packageManager'

/** Payload of `windbag://update-progress`. `total` is null when the server sent no length. */
export interface UpdateProgress {
  downloaded: number
  total: number | null
}

/** `commands::AuthOutcome`, delivered on `windbag://auth`. */
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
  targets: TargetInput[]
  media: Array<{ path: string; altText: string | null }>
}

// ─── Notes ──────────────────────────────────────────────────────────────────

/** `db::Note` */
export interface Note {
  id: number
  title: string
  body: string
  pinned: boolean
  createdAt: string
  updatedAt: string
}

/** `commands::SaveNoteInput` */
export interface SaveNoteInput {
  id: number | null
  title: string
  body: string
  pinned: boolean
}

// ─── Assistant ──────────────────────────────────────────────────────────────

/** `ai::Backend` */
export type AiBackend = 'off' | 'apple' | 'claude' | 'codex'

/** `ai::Availability` */
export interface AiAvailability {
  backend: AiBackend
  label: string
  available: boolean
  /**
   * The framework's or the tool's own words — an assistant that is "unavailable" with no reason is
   * unfixable.
   */
  reason: string
}

/** `ai::Suggestion` */
export interface Suggestion {
  body: string
  title?: string | null
  rationale?: string | null
}

// ─── Stats ──────────────────────────────────────────────────────────────────

/** `stats::StatsFilter` */
export interface StatsFilter {
  since: string | null
  until: string | null
  platforms: PlatformId[]
  accountIds: number[]
}

/** `stats::Bucket`. `postIds` is what makes a bar a drilldown rather than a picture. */
export interface Bucket {
  key: string
  label: string
  published: number
  failed: number
  postIds: number[]
}

/** `stats::EngagementTotals` */
export interface EngagementTotals {
  likes: number
  reposts: number
  replies: number
  /** Impressions, where the platform reports them — Threads, Instagram and X. */
  views: number
  /**
   * How many destinations the totals are summed from — without it, "0 likes" and "nothing fetched
   * yet" look identical.
   */
  measured: number
  oldestFetch: string | null
}

/** `stats::Stats` */
export interface Stats {
  published: number
  failed: number
  scheduled: number
  drafts: number
  missed: number
  byPlatform: Bucket[]
  byAccount: Bucket[]
  byHour: Bucket[]
  byWeekday: Bucket[]
  byDay: Bucket[]
  /** Weekday × hour, keyed `"{weekday}-{hour}"`. The punch card. */
  bySlot: Bucket[]
  failures: Bucket[]
  engagement: EngagementTotals
  engagementByPlatform: EngagementRow[]
  topPosts: TopPost[]
  /** Accounts in scope whose engagement Windbag cannot read, with the reason. */
  engagementGaps: Array<[PlatformId, string]>
}

/** `stats::EngagementRow` — one platform's engagement, over the destinations measured. */
export interface EngagementRow {
  platform: PlatformId
  label: string
  likes: number
  reposts: number
  replies: number
  views: number
  measured: number
}

/** `stats::TopPost` */
export interface TopPost {
  postId: number
  platform: PlatformId
  handle: string
  excerpt: string
  publishedAt: string | null
  remoteUrl: string | null
  likes: number
  reposts: number
  replies: number
  views: number
  /** Likes + reposts + replies. What the list is ranked by; views are reach, not earned. */
  interactions: number
  fetchedAt: string
}

/** `stats::RefreshCost` — what a refresh would read, before it runs. */
export interface RefreshCost {
  /** Destinations on X that would be looked up. Each one is a billed read. */
  billedReads: number
  freeReads: number
}

/** `stats::RefreshReport` */
export interface RefreshReport {
  updated: number
  skipped: number
  problems: string[]
}
