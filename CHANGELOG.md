# Changelog

All notable changes to Windbag are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Bluesky hashtags are real tags.** Posts carry tag facets found by the same
  rules bsky.app uses, so a tag is clickable and shows up in tag search.
- **Engagement bars and top posts drill down too.** A platform's engagement bar
  opens its measured posts; a top-post row opens the post in Windbag as a new
  draft, beside the link to the live post.
- **The stats drilldown comes to you.** Clicking a bar or cell scrolls the post
  list into view and focuses it; rows carry readable status labels, and
  published posts open as a new draft.
- **Duplicate a post as a new draft.** Sent posts in the queue and published
  posts on the calendar open as a copy in the composer — text, destinations,
  options and attachments. Saving creates a new post; the original is never
  touched.
- **Keyboard shortcuts.** Cmd/Ctrl+Enter schedules or saves the draft in the
  composer, Cmd/Ctrl+S saves a note, and Enter submits the connect dialog.
- **A tray icon on Windows and Linux.** Closing the window only hides Windbag,
  so the scheduler keeps running — but those platforms have no app menu, which
  left no way to quit and no way to install a staged update. The tray's menu
  shows the window or quits.
- **Reorder attachments.** Move-up and move-down buttons set the order
  attachments are posted in, which is how a carousel is sequenced.

- **Updates, and the pipeline behind them.** Windbag checks its releases
  repository at launch and once a day after that, and Settings → Updates carries
  the whole flow: the channel to follow (stable, beta, alpha, or matching this
  build), what a check found and how large the download is, the download itself
  with a progress bar, and the restart that applies it. The status bar carries
  the ambient half — an available, downloading or staged update stays visible
  without ever interrupting a compose.
- **Logs you can actually read.** A bundled app has no terminal, so every
  scheduler and platform error used to vanish. They are now also written to the
  OS log directory, rotated at 5 MB with three files kept.
- **An update must name its own version.** The updater refuses a download
  whose signature does not carry the version it was signed for, which closes the
  replay of an older signed build as a newer one.
- **A downloaded update is never installed while the app runs.** Replacing a
  live bundle breaks the running process's code signature, so a verified
  download is staged and swapped in on quit — or immediately, if you ask for the
  restart. Closing the window only hides Windbag, so the quit really is the exit.
- **Failures say which failure they are.** The Tauri updater flattens every
  unreadable manifest into one string, which makes a missing releases repository
  indistinguishable from a channel that has no release yet. Windbag re-asks the
  releases host directly and reports a broken pipeline as broken instead of as
  the calm pre-first-release state. A deb, rpm or Flatpak install is told it is
  managed by its package manager rather than offered a self-update it cannot do.
- **The release pipeline.** `pnpm release` writes the version, rolls the
  changelog and tags; the tag runs a preflight gate and then
  `entro314-labs/tauri-release-kit`, which builds six platform legs, signs the
  updater artifacts, and publishes the release plus its `latest.json` manifest
  on the public `windbag-releases` mirror. See
  [`docs/RELEASING.md`](docs/RELEASING.md).
- **Reporting, off the official APIs.** Engagement is now read from each
  platform's own documented endpoint rather than being reported as unavailable:
  Threads and Instagram per-media `/insights`, a Facebook Page post's own
  summary edges, and X's bulk `public_metrics` lookup (100 ids a call) beside
  the Bluesky and Mastodon counts that were already there. Six of the eight now
  answer; Reddit and LinkedIn keep their stated gap. Impressions are stored
  alongside likes, reposts and replies — Threads, Instagram and X report them.
- **Insights are opt-in per Meta app credential**, the same shape the ads
  permissions already used. The default consent screen is still exactly what
  posting needs, and the gap is reported PER ACCOUNT against the scopes that
  connection actually came back with — so a Threads account connected before you
  turned insights on says so by name instead of the platform going quiet.
- **X reads are confirmed before they are spent.** X bills per id in a lookup,
  so Refresh says how many posts it is about to read and what the free half is
  before it makes the call. `get_refresh_cost` reports it without reading
  anything.
- **Charts.** Delivery over time as a stacked area with a crosshair and keyboard
  stepping, a weekday × hour punch card, engagement per platform, and the ten
  best-performing posts. Dependency-free SVG in the Signal palette. Colour on
  the engagement chart encodes the METRIC and not the platform: the brand tones
  are right on an identity chip but fail as a series palette — two of the eight
  ship black, and Bluesky sits below the normal-vision legibility floor from
  Mastodon — so identity stays on the axis, where it gets a mark and a name. The
  three series hues are validated for deuteranopia, tritanopia and contrast
  against each mode's own surface.
- **There is deliberately no engagement trend line.** `metrics` keeps one row
  per destination, replaced on each refresh, and refreshes are manual — so a
  history would be a handful of points at whatever moments someone pressed the
  button, which invites being read as a trend when it is not one. Only delivery,
  which dates every destination exactly, is drawn over time; engagement is drawn
  as a distribution and always labelled `as of`.
- **Bluesky engagement reads unauthenticated**, from the public AppView. It used
  to open a session with the stored app password — which "Sign in with Bluesky"
  accounts do not have, so every OAuth-connected account would have failed the
  refresh citing a credential its owner never created. `getPosts` serves the
  counts to anyone, so the session call is gone rather than duplicated.
- The agent door reports it too: `get_stats` now carries impressions,
  engagement per platform, the leaderboard, and an `unreadable` list with the
  reason, so a model never reads a missing platform as a zero.

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

- **A post's link is never dropped.** Where a platform has nowhere native for it
  (always on Bluesky, X, Mastodon and Instagram; on Threads, Facebook and
  LinkedIn when the post has attachments; on a Reddit self post) it is added to
  the text on its own line and counted in the composer's limit.
- **X posts are counted the way X counts them.** Every URL costs 23 and emoji
  and CJK cost 2, so the composer refuses what X would refuse.
- **A partly sent post can be edited, and saving re-queues only what did not go
  out.** Destinations that already published keep their copy and permalink; the
  composer names them and says the edit applies to the rest. Editing a failed
  or missed post clears old errors and waiting retries.
- **Disconnecting an account tidies up the posts it was on.** A post left with
  no destinations goes back to draft, a partly sent post whose failed
  destination was removed becomes published, and a disconnect waits until the
  account has finished sending.
- **The agent door ships with every install.** It used to be a separate
  `windbag-mcp` binary you had to build from source; it is now the app's own
  executable run with `--mcp`. Settings → Agent door shows the exact
  `claude mcp add` and `codex mcp add` commands for this machine.
- **Claude Code and Codex drafts run with no tools and no MCP servers.**
  Claude's `--restricted` still let the model read and write files, search the
  web and call every claude.ai connector, so a prompt injection in a selected
  note could put local file contents into a draft. Both now run in an empty
  per-draft folder, with the tool list confirmed empty in a live session.
- **Codex drafting no longer reads `~/.codex/config.toml`.** It is the only way
  to keep its MCP servers out; set the model and effort in Settings →
  Assistant.
- **A custom Bluesky client-metadata URL must be hosted on
  entro314-labs.github.io.** The app registers one callback scheme, derived from
  that host, so any other host is refused up front with an explanation instead
  of a sign-in that hangs for five minutes.
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

### Fixed

- **Editing a partly sent post is not blocked by where it already went.** Text
  that no longer fits a platform the post has already published to no longer
  stops the save that re-queues the rest.
- **A rejected upload token no longer flags a Threads or Instagram account for
  reconnection.** The web deployment's token is what is wrong, so the post
  fails with a pointer to Settings → Web deployment instead.
- **No double posts after a lost answer.** A timeout or unreadable reply after a
  post was sent is no longer retried blindly: Bluesky, Threads and Instagram
  pick up the post they already started, and the other platforms stop with
  "may have gone out — check before retrying".
- **Threads and Instagram media is not re-uploaded on every retry.** A container
  still processing is checked again on the next attempt instead of rebuilt.
- **Video and GIF posts work on Mastodon and X.** Windbag waits for the platform
  to finish processing before posting.
- **Instagram carousels with video publish.**
- **A 403 no longer marks a working account "reconnect".** Only a 401, or a 403
  that says the token is bad, does.
- **Rate limits are waited out.** X's named reset, including the daily cap, is
  honoured, and X's spend cap is reported as out of credit instead of being
  retried five times.
- **Threads' counter no longer undercounts emoji sequences** — families,
  keycaps, ©, ⌚ and the like.
- **Large uploads no longer time out after 60 seconds**; the timeout scales with
  the file.
- **An interrupted database upgrade no longer blocks every later launch.**
- **Two refreshes of the same account can no longer collide** and knock out an
  account whose tokens rotate.
- **Instagram uses the same Graph API version as the other Meta calls.**
- **A rescheduled failed post actually sends.** Editing it left the failed
  destination untouched, so it sat as "Scheduled" with nothing to send.
- **A post cannot be changed or deleted while it is being sent.** The queue shows
  it as sending from the moment the send starts, and edits, deletes and
  removing that destination are refused with a message naming it.
- **Retrying a destination of a post with no time says why it cannot**, instead
  of sticking the post in "Sending".
- **A published post cannot be rescheduled or posted again.**
- **"Mark it missed" also covers retries.** A post waiting to retry when Windbag
  closed is marked missed on the next launch instead of going out hours late,
  and a missed post shows as missed straight away.
- **The reconnect warning appears as soon as a send finds an account's sign-in
  no longer works.**
- **Posts and notes an agent creates over MCP show up in the open window**
  within about 20 seconds.
- **The app window can no longer call Meta's ads tools directly.** Nothing used
  them, and that route skipped the per-call approval an agent host requires.
- **The delivery chart's "Failed" series is no longer always zero.** Failed
  destinations are drawn on the day the date filter dates them by.
- **An empty date range no longer hides the stats filters.** The first-run
  screen appears only when there are no posts at all; a filtered view with
  nothing in it keeps its filters and says so.
- **Engagement a platform does not report shows as "—", not 0.** Bluesky and
  Mastodon no longer show "Impressions 0", and the screen names which platforms
  do not report each figure. The agent door's `get_stats` says `null` too.
- **A refresh that fails partway says what it did** — updated, skipped and
  failed destinations, and the X reads already billed — instead of "Updated 0".
- **The X cost prompt cannot be skipped.** If the cost cannot be read, the
  refresh stops with that error instead of spending without asking.
- **Stats filters survive leaving the screen**, and every connected platform is
  always offered.
- **The agent door negotiates its protocol version**, validates `get_stats`
  filters (an offset date selects the right instant; an unknown platform is an
  error naming the valid ones; `accountIds` filters), lists upcoming posts
  first, and answers an unknown tool with a protocol error.
- **The Meta ads tools are listed only when they can work** — a Facebook Page
  connected with the ads scope — decided without reading the keychain, and the
  ads client completes Meta's MCP handshake.
- **Assistant drafts are measured the way each destination measures them.**
  Each draft shows its tightest destination as used/limit from the composer's
  own check, instead of a raw string length no platform uses.
- **A hung assistant CLI can no longer freeze Settings.** The version check that
  decides which backends are available gives up after ten seconds.
- **The assistant finds `claude` and `codex` when Windbag is opened from Finder,
  the Dock or at login.** It asks your login shell for its PATH instead of using
  the system's minimal one, so neither backend reports "not on PATH" in a
  release build.
- **An assistant that runs past its time limit is stopped**, rather than left
  running in the background after the request gave up.
- **An assistant that quits early says why**, with the CLI's own error, and an
  answer to only part of the prompt is not used.
- **Drafts are found even when the reply has brackets before them.** A preamble
  like "[as requested]" no longer turns the whole reply into one draft, and a
  draft missing its text is skipped instead of failing the batch.
- **A missing update manifest or a GitHub rate limit no longer reads as
  "Nothing published on this channel yet".** Settings says which it is.
- **Reddit link posts can be saved.** A post with a title and a link but no body
  was always refused, although that is exactly how a Reddit link submission
  works. A link now counts as content on Reddit and Facebook.
- **Attachments are checked by type and size when you save, not when they
  publish.** An MP4 is no longer accepted for Bluesky or LinkedIn, a PNG for
  Instagram, or an image over a platform's limit (Bluesky's 2 MB). A Facebook
  post with a video and photos is refused instead of quietly losing the photos;
  X takes one GIF or one video on its own, and so does Mastodon for video.
- **Required destination options are enforced.** A Reddit destination without a
  subreddit, or an option outside its allowed values, is an error in the
  composer and the agent door rather than a failure at publish time. Mastodon's
  content warning counts toward the character limit.
- **A scheduled time that has already passed is refused** in the composer, the
  calendar and the agent door, instead of being accepted as scheduled and marked
  missed a few seconds later. A scheduled post needs at least one destination;
  a draft still does not.
- **Saving a post is all-or-nothing.** The post, its attachments and its
  destinations are written in one transaction, and the composer and the agent
  door share that one save path.
- **Save stays disabled until the check for what is on screen is back**, so an
  edit that goes over a limit cannot slip through while it runs.
- **Threads and Instagram attachments over 4.5 MB publish.** The app now uploads
  the file straight to R2 through a short-lived signed URL from the web
  deployment, instead of through a Vercel function, which rejects any body over
  4.5 MB. An attachment the deployment refuses fails at once with its reason
  rather than after five retries.
- **A stray sign-in callback no longer cancels the sign-in in progress.** A
  leftover tab or a foreign `?error=` link is ignored and the app keeps
  waiting, and a callback macOS could silently drop now gets through.
- **The Bluesky "paste the link" fallback works.** The connect dialog stays open
  while the sign-in waits, and pasting the link the browser could not open
  finishes that same sign-in.
- **Bluesky token refreshes check who they talk to.** The refresh token is only
  sent to the server that issued it, and refreshed tokens must belong to the
  account.
- **Failed loads are shown as errors, not as empty screens.** A store that
  cannot be read no longer shows "Nothing queued" or "No accounts yet"; each
  screen says what failed, with a Retry.
- **Reconnect actually reconnects.** The marker on an account that needs
  re-authorising is now a button that opens sign-in for that account and
  updates it in place.
- **Calendar posts open when clicked, and published ones cannot be dragged.**
  Dragging a published post used to put it back in the queue, where it later
  showed as missed.
- **Disconnecting, removing a platform app and deleting a note ask first.**
  Disconnect says it removes that account's destinations, delivery history and
  stats from Windbag, and that published posts stay on the platform.
- **Unsaved edits are not lost silently.** Leaving the composer, or switching
  or starting a note with unsaved changes, asks before discarding; "Draft from
  this" saves the note first.
- **Pinning a note no longer saves half-finished edits.**
- **The assistant Model and Grace window fields no longer drop characters.**
  They save when you leave the field; an empty or out-of-range grace window
  reverts instead of saving.
- **A destination that needs reconnecting can be removed from a post.**
- **The assistant panel shows every selected note, names its backend, and sits
  below Destinations**, so drafts are written to the chosen destinations'
  limits.
- **Settings controls have accessible names.**

- **The macOS traffic lights sit where macOS puts them.** Their position was
  pinned from `tauri.conf.json`, which macOS 26 stopped honouring vertically —
  leaving the buttons off-centre in a header band sized for coordinates the OS
  was ignoring. The window now carries an empty unified toolbar instead, so
  AppKit places and centres them itself the way Finder's are, and the sidebar
  header is sized to that band. The collapsed rail widened to match: the lights
  span 80px from the window edge, and at 56px the green one straddled the seam
  between the rail and the content pane. In fullscreen, where the OS hides them,
  the wordmark stops reserving the gap.
- **Changing the window material replaces it instead of layering it.** Each
  switch on macOS added another frosted layer over the previous one, so moving
  between Standard and Strong a few times left several materials stacked.
- **A zero grace window no longer marks every post missed.** The missed-post
  check runs before each publishing pass, so with the window at 0 a post was
  marked missed in the pass that should have sent it — "Post now" included. The
  window now has a one-minute floor, in Settings and in the scheduler.
- **A send interrupted by a crash no longer sticks.** A destination claimed by a
  run that then died stayed in `publishing` forever: never retried, with no
  Retry button, and on a single-destination post later offered "Post now",
  which could send a second copy. Such destinations — left by a crash, or by a
  record that failed to save after the send — are now failed at the start of
  the next scheduler pass with a message saying to check the platform before
  retrying, and the attempt is logged.
- **A time with a UTC offset fires at the right moment.** The agent door (and
  any caller of `save_post` or `reschedule_post`) stored `scheduledAt` exactly
  as sent, and the due query compares stored strings — so `09:00+02:00` went
  out at 09:00 UTC, two hours late. Every writer now stores the same instant in
  canonical UTC.
- **Saving a platform app no longer deletes its stored secret.** The form says
  "leave blank to keep it", but the save overwrote the whole credential, so
  turning on *Insights access* or *Ads access* without retyping the Meta app
  secret wiped it and the reconnect that followed failed. A blank secret now
  keeps the stored one.
- **Drafting and refreshing no longer freeze the window.** Asking the assistant
  for drafts (up to three minutes), refreshing engagement (one request per
  post on some platforms), probing the assistant CLIs and the Meta ads calls
  ran on the main thread, so the whole app stopped responding until they
  returned. They now run on a worker.
- **A post that does not fit is refused before it is saved.** The composer's
  save wrote the post, its attachments and its destinations first and
  validated afterwards, so a refused save still left a scheduled post that
  would fail at its time — and on a new post, the next save created a
  duplicate. It now validates first, as the agent door already did.
- **A failed token refresh is read the right way round.** Every refresh failure
  was treated like a misconfigured app: a brief 429 or 503 from X, Reddit,
  LinkedIn or Bluesky failed the post for good and flagged a healthy account
  for reconnection, while a revoked or expired refresh token (a 400
  `invalid_grant`) failed every post without ever asking you to reconnect. An
  outage now retries, and a dead grant flags the account.

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
