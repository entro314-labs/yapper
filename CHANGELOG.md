# Changelog

All notable changes to Yapper are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] — 2026-08-28

### Added

- **Notes.** A scratch surface with no obligation to publish, and the material
  the assistant reads. "Draft from this" opens the composer with the suggest
  panel already pointed at the note.
- **An assistant tier**, off by default: Apple's on-device Foundation model
  through the Tauri plugin's Rust API, or the `claude` / `codex` CLIs invoked
  one-shot with the prompt on stdin. Yapper ships no API key. Drafts are written
  to the real character limits of the destinations you picked, and land in the
  composer — the assistant has no path to the queue.
- **Stats**, split between delivery (from Yapper's own records: what published,
  what failed under which error code, which hours you post at, filtered by
  range, platform and account, with every bar a drilldown) and engagement (only
  from Bluesky and Mastodon, which serve it free; the other three are named as
  gaps rather than shown as zeroes).
- **An MCP server**, `yapper-mcp`, so an agent host can read the queue, read and
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
- **Missed-post policy.** A post whose time passed while Yapper was closed is
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
- X's media upload uses the chunked v2 endpoints; the free API tier allows only
  17 upload initialisations per 24 hours.
- LinkedIn's API version is pinned to `202606` and is overridable per install,
  because LinkedIn sunsets versions on a rolling schedule.
