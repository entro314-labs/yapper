# Changelog

All notable changes to Windbag are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **An icon of its own.** The bag of Aeolus — a tied sack with the winds
  streaming out — in the palette's carrier cyan on its dark ground, drawn to
  Apple's inset squircle so it sits at the same size as its Dock neighbours.
  `app-icon.svg` is the source; `pnpm tauri icon` regenerates every platform
  size from the 1024 export.
- **Meta: Threads, Instagram and Facebook Pages.** Three new destinations behind
  one Meta developer app, each a full adapter — two-step media containers with
  status polling for anything carrying video, carousels, Threads topic tags and
  reply controls, Page selection across several administered Pages, and
  multi-photo Page posts assembled as one post rather than several. Threads
  counts emoji by UTF-8 byte against its 500, so it overrides `count_body` the
  way Bluesky does for graphemes.
- **A companion web deployment** (`site/`), which is what makes Meta reachable
  from a desktop app at all. It publishes the privacy policy, terms and data
  deletion pages app review requires (MDX — a new document is a new file), serves
  the HTTPS OAuth redirect Meta demands and bounces it back to the loopback
  listener, and hosts attachments in Cloudflare R2 because the Threads and
  Instagram publishing APIs fetch media from a URL and accept no upload. Next 16,
  React 19, Tailwind 4. Configured in **Settings → Web deployment**; never
  contacted unless a Meta account is connected.
- **Meta's ads MCP server, through the agent door.** `meta_ads_tools` and
  `meta_ads_call` forward to Meta's hosted server at `mcp.facebook.com/ads`, so
  the catalogue is always Meta's live one rather than a copy that goes stale.
  Opt-in per app credential, since it widens the consent screen and its writes
  spend real money.
- `Limits.requires_media`, enforced everywhere a post is validated. Instagram has
  no text-only post, so a caption-only destination is refused in the composer,
  the scheduler and the agent door rather than failing at publish time.

- **Sign in with Bluesky** — real AT Protocol OAuth alongside the app password,
  and the default for new Bluesky accounts. Scoped, revocable from Bluesky's own
  settings, and DPoP-bound: every authorized request is signed by a P-256 key
  stored beside the tokens. Includes the whole profile — handle → DID resolution
  with the mandatory bidirectional check, PDS and authorization-server
  discovery, pushed authorization requests, `iss` verification on the callback,
  and refresh-token rotation. Both methods land on the same DID, so switching
  upgrades an account in place.
- A **preflight** on the client metadata URL, so an unpublished document is
  reported as a checklist before the browser opens rather than as an opaque
  `invalid_client_metadata` halfway through.
- A **Callback URL** field in the connect dialog, for when the OS cannot route
  the custom scheme back — paste the link the browser could not open.

### Changed

- **Animated icons where a control is live.** The sidebar rows and its collapse
  toggle, New post / Write a post, Post now, Attach, the assistant buttons, the
  stats Refresh, the calendar arrows, the copy chips and the developer-portal
  links now carry hover-animated glyphs from lucide-animated. The whole control
  triggers the animation, not just the glyph, and keyboard focus plays it too.
  Static indicators, window controls and the queue card actions stay as they
  were. Motion now honours the system reduced-motion setting, which the page
  transitions had been missing.
- `oauth::handoff` is now separable from the token exchange, and takes the
  redirect URI to *advertise* independently of where it listens. Meta's token
  endpoints are not a standard exchange — a GET with query parameters, a response
  carrying `user_id` and no `expires_in` — and its apps register an HTTPS bounce
  rather than the loopback.
- Meta failures are classified by Meta's own error envelope rather than by HTTP
  status. An expired token there is a **400** with `code: 190`, which the generic
  mapper would have filed as terminal-but-fine — leaving the account looking
  healthy while every post failed.

- **The app is now Windbag.** Renamed end to end: product and window title,
  bundle identifier (`com.entro314.windbag`), crate and package names, the
  `windbag-mcp` binary, the `windbag://` event channels, the keychain service,
  and the data directory. Two consequences for an existing install — connected
  accounts must be reconnected (keychain items are namespaced by service), and
  the store moves to a `windbag` directory. The Git repository, its Pages URL
  and the `io.github.entro314-labs` callback scheme are unchanged, so the
  Bluesky OAuth client identity still resolves.
- `FieldSpec` now declares its allowed values, which the renderer draws as a
  picker. It previously parsed them out of the help sentence.

### Activation

OAuth needs `docs/client-metadata.json` published on the web and a bundled
build; see the README. App passwords keep working with neither.

## [0.2.0] — 2026-08-28

### Added

- **Notes.** A scratch surface with no obligation to publish, and the material
  the assistant reads. "Draft from this" opens the composer with the suggest
  panel already pointed at the note.
- **An assistant tier**, off by default: Apple's on-device Foundation model
  through the Tauri plugin's Rust API, or the `claude` / `codex` CLIs invoked
  one-shot with the prompt on stdin. Windbag ships no API key. Drafts are written
  to the real character limits of the destinations you picked, and land in the
  composer — the assistant has no path to the queue.
- **Stats**, split between delivery (from Windbag's own records: what published,
  what failed under which error code, which hours you post at, filtered by
  range, platform and account, with every bar a drilldown) and engagement (only
  from Bluesky and Mastodon, which serve it free; the other three are named as
  gaps rather than shown as zeroes).
- **An MCP server**, `windbag-mcp`, so an agent host can read the queue, read and
  write notes, and schedule posts — with each tool call visible to you. It runs
  the same validation the composer does and tells an agent that a scheduled post
  needs the app running.

### Changed

- The schema is now a versioned ladder rather than create-if-not-exists, so a
  future column added to a table holding live scheduled posts has a path.

### Fixed

- A failed destination has no publish time, so any date filter on the stats
  screen dropped every failure — "why things failed" was empty exactly when it
  mattered. Destinations now fall back to the post's own time.
- X's free tier no longer exists: tiered plans were retired in February 2026 in
  favour of pay-per-use credits. The connect screen said otherwise, and an
  out-of-credit rejection was treated as a rate limit and retried five times.
  It is now terminal and names the fix.
- LinkedIn's "Share on LinkedIn" product is self-serve and approved instantly;
  the copy implied a review queue that only applies to organization posting.

## [0.1.0] — 2026-08-28

First working version.

### Added

- **Composer** with per-destination character counters that run the same
  validation the scheduler runs, so a counter that says a post fits is a promise
  the backend keeps. Reddit's title field and each platform's own options
  (subreddit, flair, visibility, reply settings) appear only for the
  destinations actually picked.
- **Queue** banded by what needs a decision first — failed, partly sent and
  missed posts above what is upcoming. Each destination shows its own status,
  permalink, error and retry, and an attempt log records the failures that
  preceded a success.
- **Calendar** month view; drag a post to another day to move it, keeping its
  time of day.
- **Scheduler** that publishes due posts on a worker thread, retries rate limits
  and outages with a 1/5/15/60-minute backoff, gives up after five attempts, and
  flags an account for reconnection the moment a platform rejects its
  credentials.
- **Missed-post policy.** A post whose time passed while Windbag was closed is
  marked missed rather than published hours late, with a configurable grace
  window; *Post it late* is available for posts where late beats never.
- **Accounts** for Bluesky (app password), Mastodon (self-registering OAuth
  app), Reddit, X and LinkedIn (your own developer app), all over one OAuth 2.0
  + PKCE loopback flow on `http://127.0.0.1:8917/callback`.
- **Attachments** for Bluesky, Mastodon, X and LinkedIn, with alt text in the
  composer row rather than behind a disclosure.
- **Link facets on Bluesky**, so a posted URL is a real link rather than grey
  text.
- Launch at login, starting hidden with the scheduler running; closing the
  window hides it rather than quitting.
- Light and dark themes, and a window material (macOS vibrancy, Windows Mica)
  stamped from what the OS actually applied rather than from the preference.

### Known limits

- Reddit takes text and link submissions; image submissions go through a
  separate upload-lease flow that is not implemented.
- LinkedIn takes one image per post. Several needs the MultiImage API, which is
  a different content shape.
- X's media upload uses the chunked v2 endpoints. X's API is pay-per-use, so
  every post and upload costs credit on your own developer app.
- LinkedIn's API version is pinned to `202606` and is overridable per install,
  because LinkedIn sunsets versions on a rolling schedule.
