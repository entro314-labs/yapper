# Windbag

A desktop composer and scheduler for the social platforms that still have an
open posting API — **Bluesky, Mastodon, Reddit, X, LinkedIn, Threads, Instagram
and Facebook Pages**. Write once, pick where it goes, give it a time. Tauri 2 + Rust + React 19, with everything
in a local SQLite store and every credential in your OS keychain.

## What it does

- **One composer, eight destinations.** Per-destination character counters that
  run the *same* validation the scheduler will — Bluesky's 300 graphemes, X's
  280, LinkedIn's 3,000, Threads' 500 with emoji billed by byte, your Mastodon
  instance's own limit, Reddit's title, Instagram's insistence on an image.
- **A real scheduler.** A worker thread publishes due posts, retries on rate
  limits and outages with backoff, and records every attempt.
- **Per-destination truth.** Three platforms accepting and the fourth being
  rate-limited is the normal case, so the queue shows each destination's own
  status, permalink, error and retry — never a single collapsed "failed".
- **A calendar you can drag on.** Move a post to another day; it keeps its time.
- **Notes.** A scratch surface with no obligation to publish — and the material
  the assistant reads when it drafts for you.
- **An assistant, if you already have one.** Apple's on-device model, or the
  Claude Code or Codex CLI on your PATH. Off by default; Windbag ships no API key.
- **Reporting.** What published, what failed and why, when you actually post,
  and what each post earned — delivery drawn over time, a weekday × hour punch
  card, engagement per platform and a leaderboard of the posts that did best.
  Every bar and every cell is a drilldown into the posts behind it. Engagement
  is read from each platform's own official API: Bluesky, Mastodon, Threads,
  Instagram, Facebook and X. Reddit and LinkedIn are named as gaps rather than
  drawn as zeroes, because a zero is a claim and "we cannot see it" is the
  truth.
- **An agent door, both ways.** An MCP server so Claude Code or Codex can read
  your queue and schedule posts, where you approve each tool call — and a
  passthrough to Meta's own hosted ads MCP server, so the same session can read
  what the campaign behind a post did.
- **Honest about missing.** See "The one real constraint" below.

## The one real constraint

Windbag posts **from your machine**, so it has to be running when a post is due.
There is no server holding your tokens. Launch-at-login (Settings) starts it
hidden with the scheduler running, which covers the ordinary case — but a
machine that is asleep at 09:00 does not post at 09:00.

So Windbag is explicit about it. A post whose time passed while the app was
closed is, by default, marked **missed** rather than published hours late; a
grace window (15 minutes by default) covers briefly closing the laptop. If late
is better than never for your posts, switch **Settings → If a post was missed**
to *Post it late*.

## Accounts

| Platform | What you need | Effort | Notes |
| --- | --- | --- | --- |
| **Bluesky** | A handle | ~1 min | Sign in with Bluesky (OAuth), or an app password. |
| **Mastodon** | Your instance host | ~1 min | Windbag registers itself with the server. |
| **Reddit** | Your own "installed app" | ~5 min | Free for non-commercial use, no secret. |
| **LinkedIn** | Your own developer app | ~10 min | "Share on LinkedIn" is self-serve, approved instantly. |
| **X** | Your own developer app | ~10 min + cost | Pay-per-use. See below. |
| **Threads** | Your own Meta app + the web deployment | ~20 min | HTTPS redirect required. |
| **Instagram** | The same, + a professional account | ~20 min | Every post needs media. |
| **Facebook Page** | The same, + a Page you administer | ~20 min | Posts as the Page. |

### Why you register your own app

Every platform here supports authentication, and Windbag implements all of it —
OAuth 2.0 + PKCE for four of them, an app password for Bluesky. What Windbag
cannot do is hand you an app *identity*, and the reason differs per platform:

- **X** bills per API call against credits on the app the token belongs to
  (tiered plans were retired in February 2026; there is no free tier). A client
  id shipped in Windbag would bill every user's posts to one account.
- **Reddit** rate-limits per OAuth client id, so a shared id would put every
  Windbag user in one bucket — and its Responsible Builder Policy grants access
  per developer, not per app.
- **LinkedIn** ties an app to a person or page, though "Share on LinkedIn" is
  auto-approved with no review queue.

So you register your own and paste the client id into **Settings → Platform
apps**. It is stored in the OS credential store, never in the app's database.
Every OAuth app registers the same redirect URI, which Settings shows for
copying: `http://127.0.0.1:8917/callback`.

Bluesky and Mastodon need none of this — start with either to see the whole
pipeline work in about a minute.

### Meta needs a web deployment

Threads, Instagram and Facebook are the only destinations here that a desktop
app cannot reach on its own, for two reasons that have nothing to do with each
other:

1. **The sign-in redirect must be HTTPS.** Every other platform accepts
   `http://127.0.0.1:8917/callback`. Meta refuses it outright, and there is no
   native-client flow to fall back on.
2. **Media is fetched, not uploaded.** `POST /{id}/threads` and
   `POST /{ig-id}/media` take an `image_url` that Meta's servers then go and GET.
   There is no byte-upload endpoint for either, so a file on your disk is
   unreachable to them. (A Facebook Page is the exception — it takes a multipart
   upload like everyone else.)

So this repo ships [`site/`](site/): a small Next.js deployment that answers
both. It serves the legal pages Meta's app review asks for, provides the HTTPS
redirect at `/oauth/meta` (which bounces straight back to the loopback listener
the app already has open, storing nothing), and holds attachments in Cloudflare
R2 for as long as it takes Meta to fetch them. The app never pushes the file
through the deployment itself — a Vercel function refuses request bodies over
4.5 MB — so `/api/media` answers with a presigned R2 upload URL, signed for the
declared type and size, and the app uploads straight to R2.

It deploys to **windbag.social**, which makes the strings each Meta app needs:

| Where it goes | Value |
| --- | --- |
| Valid OAuth Redirect URI | `https://windbag.social/oauth/meta` |
| App domain | `windbag.social` |
| Privacy Policy URL | `https://windbag.social/legal/privacy` |
| Terms of Service URL | `https://windbag.social/legal/terms` |
| Data Deletion Instructions | `https://windbag.social/legal/data-deletion` |
| Data Deletion Callback | `https://windbag.social/api/data-deletion` |

Deploy it, then paste the base URL into **Settings → Web deployment**. If you
never connect a Meta account, none of this is ever contacted.

Instagram carries one further constraint worth knowing before you plan around
it: **there is no text-only Instagram post.** A caption is a property of an image
or a video. The composer, the scheduler and the agent door all refuse a
caption-only Instagram destination up front rather than at 3am.

### Meta ads

With **Ads access** set to *yes* on your Meta app credentials, the connection
also requests `ads_read` and `ads_management`, and the agent door gains
`meta_ads_tools` and `meta_ads_call` — a passthrough to Meta's hosted MCP server
at `mcp.facebook.com/ads`. Windbag forwards rather than reimplementing, so the
catalogue is always Meta's live one: reporting, campaign and ad-set management,
catalogues, A/B tests.

It is off by default, because it widens the consent screen for everyone to serve
something most people never open — and because anything it writes spends real
money.

### Sign in with Bluesky

Bluesky is the one platform with two ways in, and the connect dialog picks
between them:

- **OAuth** (default) — real AT Protocol OAuth. Scoped to `atproto
  transition:generic`, revocable from Bluesky's own settings, and never hands
  Windbag a reusable password. Its tokens are DPoP-bound: every request they
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

Until both are done, Windbag says so before opening a browser rather than
failing halfway through. If a callback ever fails to route, the browser shows a
link it could not open: paste it into the dialog's **Callback URL** field and
the same sign-in finishes.

## The assistant

Windbag ships no API key, no gateway and no model catalogue. It borrows an
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
Windbag ships an MCP server. An agent host shows you each tool call before it
runs, which is the trust boundary that makes write access reasonable.

```sh
cargo build --release --bin windbag-mcp
claude mcp add windbag -- "$PWD/src-tauri/target/release/windbag-mcp"
```

It opens the same store the app uses, so it works whether or not Windbag is
running — but nothing publishes until the app next runs, and `create_post` says
so in its own answer. Tools: `list_accounts`, `list_posts`, `list_notes`,
`create_note`, `create_post`, `get_stats`. A post that would not fit its
destinations is refused at the tool, with the platform's own message.

## Stats

Two halves, deliberately kept apart:

- **Delivery** is computed from Windbag's own records. Always there, always
  current: what published, what failed and under which error code, which hours
  you actually post at, per platform and per account. Filter by range, platform
  or account; click any bar to see the posts behind it.
- **Engagement** is read from each platform's own official API when you press
  Refresh — nothing polls. Bluesky and Mastodon serve public counts, Threads and
  Instagram serve per-media insights, a Page post carries its own summary counts,
  and X returns `public_metrics` in a bulk lookup. Reddit's score and LinkedIn's
  analytics need scopes Windbag does not request, so those are named as gaps
  rather than drawn as zeroes.

  Two things gate the rest. **Threads and Instagram insights are opt-in** — set
  *Insights access* to `yes` in **Settings → Platform apps** and reconnect the
  account, because the extra scope widens the consent screen and a connection
  made without it does not gain it retroactively. **X reads cost money**: it
  bills per id in a lookup, so Refresh tells you how many posts it is about to
  read before it makes the call.

There are no engagement trend lines, on purpose. Windbag keeps one row of counts
per destination, replaced on each refresh, and refreshes are manual — so a
history would be a handful of points at whatever moments you happened to press
the button, which invites being read as a trend when it is not one. Delivery is
the half that dates every destination exactly, so that is the half drawn over
time; engagement is drawn as a distribution and always labelled *as of*.

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
  stats.rs              delivery figures, and engagement off each platform's API
  update.rs             signed auto-updates, staged and installed on quit
  mcp.rs + bin/mcp.rs   the agent door
  platforms/            one adapter per destination behind the Platform trait
```

### Releasing

`pnpm release patch` writes the version, rolls the changelog and pushes a tag;
the tag runs the pipeline that builds every platform, signs the updater
artifacts and publishes them on the public `windbag-releases` mirror. The
updater endpoints and the minisign public key that verifies every download are
in `src-tauri/tauri.conf.json`. Full ritual and the one-time secrets in
[`docs/RELEASING.md`](docs/RELEASING.md).

### Adding a platform

Implement `Platform` in `src-tauri/src/platforms/`, add it to `PlatformId` and
`adapter()`. The renderer draws its connect form, its per-destination options
and its limits from the `PlatformInfo` the adapter returns — the only frontend
change a new platform needs is a mark and a brand tone in `platform-brand.tsx`.

Keep `info()` cheap: the composer calls it on every keystroke through
`platforms::validate`, so nothing in it may touch the credential store or the
network.

## Privacy

Posts, schedules and account metadata live in one SQLite file in your app data
directory. Tokens, app passwords and your developer-app client ids live in the
OS credential store. Nothing is sent anywhere except to the platform you are
posting to, and every one of those requests is made from Rust — the webview
never sees a token, and its CSP gives scripts no network access beyond Tauri's
own IPC (`connect-src`). The one thing it loads from the web is images over
HTTPS (`img-src https:`), which is how account avatars display; those are plain
GETs of the avatar URL and carry no credentials.

The one exception is Meta, and only when you post to Threads or Instagram *with
an attachment*: that file is uploaded to your own web deployment's storage
first, because Meta will not accept it any other way. While it is there it is
publicly readable by anyone holding the URL — it has to be, since Meta fetches
it anonymously — so the key is 32 random bytes and a bucket lifecycle rule
expires it. See
[`site/app/legal/privacy/page.mdx`](site/app/legal/privacy/page.mdx).
