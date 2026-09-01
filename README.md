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
- **Notes.** A scratch surface with no obligation to publish — and the material
  the assistant reads when it drafts for you.
- **An assistant, if you already have one.** Apple's on-device model, or the
  Claude Code or Codex CLI on your PATH. Off by default; Yapper ships no API key.
- **Stats.** What published, what failed and why, and when you actually post —
  every bar is a drilldown into the posts behind it.
- **An agent door.** An MCP server so Claude Code or Codex can read your queue
  and schedule posts, where you approve each tool call.
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

| Platform | What you need | Effort | Notes |
| --- | --- | --- | --- |
| **Bluesky** | A handle | ~1 min | Sign in with Bluesky (OAuth), or an app password. |
| **Mastodon** | Your instance host | ~1 min | Yapper registers itself with the server. |
| **Reddit** | Your own "installed app" | ~5 min | Free for non-commercial use, no secret. |
| **LinkedIn** | Your own developer app | ~10 min | "Share on LinkedIn" is self-serve, approved instantly. |
| **X** | Your own developer app | ~10 min + cost | Pay-per-use. See below. |

### Why you register your own app

Every platform here supports authentication, and Yapper implements all of it —
OAuth 2.0 + PKCE for four of them, an app password for Bluesky. What Yapper
cannot do is hand you an app *identity*, and the reason differs per platform:

- **X** bills per API call against credits on the app the token belongs to
  (tiered plans were retired in February 2026; there is no free tier). A client
  id shipped in Yapper would bill every user's posts to one account.
- **Reddit** rate-limits per OAuth client id, so a shared id would put every
  Yapper user in one bucket — and its Responsible Builder Policy grants access
  per developer, not per app.
- **LinkedIn** ties an app to a person or page, though "Share on LinkedIn" is
  auto-approved with no review queue.

So you register your own and paste the client id into **Settings → Platform
apps**. It is stored in the OS credential store, never in the app's database.
Every OAuth app registers the same redirect URI, which Settings shows for
copying: `http://127.0.0.1:8917/callback`.

Bluesky and Mastodon need none of this — start with either to see the whole
pipeline work in about a minute.

### Sign in with Bluesky

Bluesky is the one platform with two ways in, and the connect dialog picks
between them:

- **OAuth** (default) — real AT Protocol OAuth. Scoped to `atproto
  transition:generic`, revocable from Bluesky's own settings, and never hands
  Yapper a reusable password. Its tokens are DPoP-bound: every request they
  authorize is signed by a P-256 key held next to them, so a stolen token alone
  is useless.
- **App password** — one field, works everywhere, no setup at all.

Both land on the same DID, so switching upgrades the account in place and
scheduled posts survive.

**OAuth needs two things activated before it can run**, because the protocol
has no registration step — a client's identity IS a document published on the
web, which the authorization server fetches during sign-in:

1. **Publish `docs/client-metadata.json`.** Create the GitHub repo, push, and
   enable Pages from `main` / `/docs`, so
   `https://entro314-labs.github.io/yapper/client-metadata.json` resolves.
   Hosting it anywhere else works too — set the URL in
   **Settings → Platform apps → Bluesky**, and the redirect scheme is derived
   from that hostname automatically.
2. **Run a bundled build** (`pnpm tauri:build`). The callback comes back on a
   custom URI scheme (`io.github.entro314-labs:/callback`), which macOS routes
   through the bundle's `Info.plist` — so it cannot work under `tauri dev`.

Until both are done, Yapper says so before opening a browser rather than
failing halfway through. If a callback ever fails to route, the browser shows a
link it could not open: paste it into the dialog's **Callback URL** field and
the same sign-in finishes.

## The assistant

Yapper ships no API key, no gateway and no model catalogue. It borrows an
assistant you already have, and it is **off until you pick one** in
Settings → Assistant:

| Backend | What it needs | Notes |
| --- | --- | --- |
| **Apple Intelligence** | macOS on Apple silicon | On-device, offline, keyless, free. |
| **Claude Code** | `claude` on your PATH | Run one-shot and restricted: no tools, no session saved. |
| **Codex** | `codex` on your PATH | Run with plugins, hooks, memories and apps disabled. |

The assistant reads notes you select plus anything you paste, is told your
destinations' real character limits, and hands back three drafts. **It has no
path to the queue.** A draft you pick fills the composer, where the same
counters, the same validation and the same click still stand between it and a
published post.

## The agent door

For actual agency — "read my notes, draft a week of posts, schedule them" —
Yapper ships an MCP server. An agent host shows you each tool call before it
runs, which is the trust boundary that makes write access reasonable.

```sh
cargo build --release --bin yapper-mcp
claude mcp add yapper -- "$PWD/src-tauri/target/release/yapper-mcp"
```

It opens the same store the app uses, so it works whether or not Yapper is
running — but nothing publishes until the app next runs, and `create_post` says
so in its own answer. Tools: `list_accounts`, `list_posts`, `list_notes`,
`create_note`, `create_post`, `get_stats`. A post that would not fit its
destinations is refused at the tool, with the platform's own message.

## Stats

Two halves, deliberately kept apart:

- **Delivery** is computed from Yapper's own records. Always there, always
  current: what published, what failed and under which error code, which hours
  you actually post at, per platform and per account. Filter by range, platform
  or account; click any bar to see the posts behind it.
- **Engagement** comes from the platforms, and only Bluesky and Mastodon give it
  away on endpoints Yapper already has credentials for. X's metrics need a paid
  tier and LinkedIn's need approved read scopes, so those are named as gaps
  rather than drawn as zeroes. Fetched when you press Refresh — nothing polls.

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
  db.rs                 SQLite + the schema ladder: accounts, posts, targets,
                        media, attempts, notes, metrics
  secrets.rs            OS credential store
  oauth.rs              one OAuth 2.0 + PKCE loopback flow for every provider
  scheduler.rs          the worker: due posts, backoff, missed-post policy
  atproto.rs            AT Protocol OAuth: identity, discovery, PAR, tokens
  dpop.rs               proof-of-possession signing for that flow
  ai.rs                 three assistant backends behind one verb
  stats.rs              delivery figures, and engagement where it is free
  mcp.rs + bin/mcp.rs   the agent door
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
