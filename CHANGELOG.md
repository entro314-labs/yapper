# Changelog

All notable changes to Yapper are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
