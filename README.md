# Yapper

A desktop composer and scheduler for the social platforms that still have an
open posting API — **Bluesky, Mastodon, Reddit, X and LinkedIn**. Write once,
pick where it goes, give it a time. Tauri 2 + Rust + React 19, with everything
in a local SQLite store and every credential in your OS keychain.

## What it does

- **One composer, five destinations.** Per-destination character counters that
  run the *same* validation the scheduler will — Bluesky's 300 graphemes, X's
  280, LinkedIn's 3,000, your Mastodon instance's own limit, Reddit's title.
- **A real scheduler.** A worker thread publishes due posts, retries on rate
  limits and outages with backoff, and records every attempt.
- **Per-destination truth.** Three platforms accepting and the fourth being
  rate-limited is the normal case, so the queue shows each destination's own
  status, permalink, error and retry — never a single collapsed "failed".
- **A calendar you can drag on.** Move a post to another day; it keeps its time.
- **Honest about missing.** See "The one real constraint" below.

## The one real constraint

Yapper posts **from your machine**, so it has to be running when a post is due.
There is no server holding your tokens. Launch-at-login (Settings) starts it
hidden with the scheduler running, which covers the ordinary case — but a
machine that is asleep at 09:00 does not post at 09:00.

So Yapper is explicit about it. A post whose time passed while the app was
closed is, by default, marked **missed** rather than published hours late; a
grace window (15 minutes by default) covers briefly closing the laptop. If late
is better than never for your posts, switch **Settings → If a post was missed**
to *Post it late*.

## Accounts

| Platform | What you need | Notes |
| --- | --- | --- |
| **Bluesky** | An app password | Nothing to register. Start here. |
| **Mastodon** | Your instance host | Yapper registers itself with the server. |
| **Reddit** | Your own "installed app" | Free, no review, no client secret. |
| **X** | Your own developer app | Needs Write access; the free tier is small. |
| **LinkedIn** | Your own developer app | Needs the "Share on LinkedIn" product. |

X, Reddit and LinkedIn all gate posting behind a developer app that has to be
registered to a person, and a client secret shipped inside a distributed binary
is not a secret — so you register your own and paste the client id into
**Settings → Platform apps**. It is stored in the OS credential store, never in
the app's database. Every OAuth app registers the same redirect URI, which
Settings shows for copying: `http://127.0.0.1:8917/callback`.

## Development

```sh
pnpm install
pnpm tauri:dev          # the app, with the Vite dev server
pnpm check              # lint, format, types, build, clippy, rustfmt, rust tests
```

Requires Node 24+, Rust 1.98 and pnpm 12 — `mise install` picks all three up
from `mise.toml` and `rust-toolchain.toml`.

### Layout

```
src/                    React 19 + TanStack Router + Tailwind 4
  components/ui/        Base UI primitives with the house chrome
  components/shell/     sidebar, pane titlebar, status bar
  lib/tauri/            the IPC contract: command registry, client, types
  lib/query/            TanStack Query keys, hooks, and the Rust event bridge
  routes/               queue, compose, calendar, accounts, settings
src-tauri/src/
  db.rs                 SQLite: accounts, posts, targets, media, attempts
  secrets.rs            OS credential store
  oauth.rs              one OAuth 2.0 + PKCE loopback flow for every provider
  scheduler.rs          the worker: due posts, backoff, missed-post policy
  platforms/            one adapter per destination behind the Platform trait
```

### Adding a platform

Implement `Platform` in `src-tauri/src/platforms/`, add it to `PlatformId` and
`adapter()`. The renderer draws its connect form, its per-destination options
and its limits from the `PlatformInfo` the adapter returns — no frontend change
is needed to make a new platform appear.

## Privacy

Posts, schedules and account metadata live in one SQLite file in your app data
directory. Tokens, app passwords and your developer-app client ids live in the
OS credential store. Nothing is sent anywhere except to the platform you are
posting to, and every one of those requests is made from Rust — the webview
never sees a token, and its CSP allows no outbound connections at all.
