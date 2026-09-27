//! What actually happened: how much went out, where it landed, and what it
//! earned.
//!
//! Two different things live here, and keeping them apart is the point:
//!
//!   * **Delivery** — computed from Windbag's own store. Always available,
//!     always current, costs nothing: how many posts published, how many failed
//!     and why, which platform is flaky, what hour of the day you actually post
//!     at. This is the half that works the moment you have used the app.
//!   * **Engagement** — likes, reposts, replies and impressions, read from each
//!     platform's own official endpoint. Six of the eight give it away:
//!     Bluesky's `getPosts` and Mastodon's status endpoint serve public counts;
//!     Threads and Instagram serve per-media `/insights`; a Facebook Page post
//!     carries its own summary counts; X returns `public_metrics` in a bulk
//!     lookup. Reddit and `LinkedIn` stay unavailable rather than shown as zero —
//!     both need scopes Windbag does not request.
//!
//! Nothing here polls. Engagement is fetched when the user asks, because a
//! background poller against eight APIs is a rate-limit budget spent on numbers
//! nobody is looking at — and on X it is a bill.
//!
//! **Why there are no engagement trend lines.** The `metrics` table keeps one
//! row per destination, replaced on each refresh (see the V2 note in
//! [`crate::db`]). With a manual refresh, a history would be a handful of rows
//! at whatever moments someone happened to press the button — a shape that
//! invites being read as a trend when it is not one. So the charts here graph
//! what is genuinely dense: delivery over time, which every published
//! destination dates exactly, and engagement as a distribution across
//! platforms, posts and posting hours, always labelled `as of fetched_at`.

use std::collections::HashMap;

use chrono::{DateTime, Datelike, Timelike, Utc};
use serde::{Deserialize, Serialize};

use crate::db::{self, Db, Metrics, PostTarget};
use crate::error::{AppError, Result, from_status};
use crate::http;
use crate::platforms::{
    self, PlatformId,
    meta::{self, facebook, instagram, threads},
    x,
};
use crate::secrets;

/// What a stats view is narrowed to. Every field is optional; an empty filter is
/// "everything".
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsFilter {
    /// Inclusive lower bound, RFC 3339 UTC.
    #[serde(default)]
    pub since: Option<String>,
    /// Exclusive upper bound.
    #[serde(default)]
    pub until: Option<String>,
    #[serde(default)]
    pub platforms: Vec<PlatformId>,
    #[serde(default)]
    pub account_ids: Vec<i64>,
}

impl StatsFilter {
    /// Whether one destination, dated `at` by [`dated_at`], is in scope.
    fn matches(&self, account: &db::Account, at: &str) -> bool {
        if !self.platforms.is_empty() && !self.platforms.contains(&account.platform) {
            return false;
        }
        if !self.account_ids.is_empty() && !self.account_ids.contains(&account.id) {
            return false;
        }
        if self.since.as_deref().is_some_and(|since| at < since) {
            return false;
        }
        self.until.as_deref().is_none_or(|until| at < until)
    }
}

/// When a destination happened — the one date both the filter and the delivery
/// chart use, so a destination counted in range is also drawn on a day.
///
/// The fallback to the POST's own time is load-bearing: a FAILED destination has
/// no `published_at`, so dating it by that alone drops every failure out of any
/// range — which would leave "why things failed" silently empty exactly when it
/// matters most. A post is dated by when it was meant to go out, falling back to
/// when it was last touched — the only timestamps a never-published destination
/// has.
fn dated_at<'a>(target: &'a PostTarget, post: &'a db::Post) -> &'a str {
    target
        .published_at
        .as_deref()
        .or(post.scheduled_at.as_deref())
        .unwrap_or(&post.updated_at)
}

/// One row of a breakdown:a label, what it counts, and the ids behind it.
///
/// `target_ids` is what makes a chart a drilldown rather than a picture — the
/// UI hands them straight back to filter the queue.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bucket {
    pub key: String,
    pub label: String,
    pub published: i64,
    pub failed: i64,
    pub post_ids: Vec<i64>,
}

/// Every engagement dimension is `None` when no measured destination in the set
/// reports it — Bluesky and Mastodon publish no impressions, Instagram no
/// reposts — because a summed zero would claim "nobody saw it" where the truth
/// is "the platform does not say".
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngagementTotals {
    pub likes: Option<i64>,
    pub reposts: Option<i64>,
    pub replies: Option<i64>,
    /// Impressions, where the platform reports them — Threads, Instagram and X.
    pub views: Option<i64>,
    /// How many destinations these totals are summed from — without it, "0
    /// likes" and "nothing fetched yet" look the same.
    pub measured: i64,
    /// The oldest fetch in the set: the totals are only as fresh as this.
    pub oldest_fetch: Option<String>,
}

/// One platform's engagement, summed over the destinations actually measured.
///
/// Separate from [`Bucket`] on purpose: a bucket counts destinations, this
/// carries five independent dimensions of which any may be unreported.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngagementRow {
    pub platform: PlatformId,
    pub label: &'static str,
    /// `None` per dimension on the same terms as [`EngagementTotals`].
    pub likes: Option<i64>,
    pub reposts: Option<i64>,
    pub replies: Option<i64>,
    pub views: Option<i64>,
    /// How many destinations these came from — without it, a platform with one
    /// measured post and one with fifty look comparable.
    pub measured: i64,
    /// The posts behind the row, so a bar in the engagement chart is a
    /// drilldown like every other bar.
    pub post_ids: Vec<i64>,
}

/// A published destination that earned something, for the leaderboard.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopPost {
    pub post_id: i64,
    pub platform: PlatformId,
    pub handle: String,
    /// The first line of the body, trimmed — enough to recognise the post.
    pub excerpt: String,
    pub published_at: Option<String>,
    pub remote_url: Option<String>,
    /// `None` where the platform did not report that dimension for this post.
    pub likes: Option<i64>,
    pub reposts: Option<i64>,
    pub replies: Option<i64>,
    pub views: Option<i64>,
    /// Likes + reposts + replies, over the ones reported. What the list is
    /// ranked by, and deliberately not including views: impressions are a reach
    /// number, not an earned one, and only three platforms report them at all.
    pub interactions: i64,
    pub fetched_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub published: i64,
    pub failed: i64,
    pub scheduled: i64,
    pub drafts: i64,
    pub missed: i64,
    /// Destinations, grouped. The renderer draws these as bars.
    pub by_platform: Vec<Bucket>,
    pub by_account: Vec<Bucket>,
    /// Local hour of day, 0–23, over published destinations only.
    pub by_hour: Vec<Bucket>,
    pub by_day: Vec<Bucket>,
    /// Weekday × hour, keyed `"{weekday}-{hour}"` with both zero-padded. The
    /// punch card: "Tuesday at 09:00" is not recoverable from an hour and a
    /// weekday breakdown, each of which collapses one axis of it.
    pub by_slot: Vec<Bucket>,
    /// Failures grouped by their error code — the taxonomy that says whether a
    /// platform is flaky or the posts are wrong.
    pub failures: Vec<Bucket>,
    pub engagement: EngagementTotals,
    /// Engagement per platform, over the destinations that have been measured.
    pub engagement_by_platform: Vec<EngagementRow>,
    /// The best-performing destinations in scope, ranked by interactions.
    pub top_posts: Vec<TopPost>,
    /// Accounts in scope whose engagement Windbag cannot read, with the reason.
    /// Per account rather than per platform: two Threads accounts can differ,
    /// because insights are granted at connect time and one may predate it.
    pub engagement_gaps: Vec<(PlatformId, String)>,
}

/// The six breakdowns, filled together as destinations are walked.
#[derive(Default)]
struct Groupers {
    platform: Grouper,
    account: Grouper,
    hour: Grouper,
    day: Grouper,
    slot: Grouper,
    failure: Grouper,
}

impl Groupers {
    /// Records one settled destination, dated `at` by [`dated_at`].
    fn add(
        &mut self,
        post_id: i64,
        target: &PostTarget,
        account: &db::Account,
        at: &str,
        ok: bool,
    ) {
        self.platform.add(
            account.platform.as_str(),
            account.platform.label(),
            post_id,
            ok,
        );
        self.account
            .add(&account.id.to_string(), &account.handle, post_id, ok);

        if !ok && let Some(error) = target.error.as_deref() {
            let code = error_code(error);
            self.failure.add(code, failure_label(code), post_id, false);
        }

        // Local time, because "when do I post" is a question about the person's
        // day, not about UTC.
        let Ok(local) = db::parse_rfc3339(at).map(|at| at.with_timezone(&chrono::Local)) else {
            return;
        };

        // Delivery over time carries both outcomes — its "Failed" series is half
        // the point of drawing it — each on the day the filter dated it by.
        let day = local.format("%Y-%m-%d").to_string();
        self.day.add(&day, &day, post_id, ok);

        // "When do I post" is about what went out, so these count published
        // destinations only.
        if ok {
            self.hour.add(
                &format!("{:02}", local.hour()),
                &format!("{:02}:00", local.hour()),
                post_id,
                true,
            );
            self.slot.add(
                &format!(
                    "{}-{:02}",
                    local.weekday().number_from_monday(),
                    local.hour()
                ),
                &local.format("%a %H:00").to_string(),
                post_id,
                true,
            );
        }
    }
}

/// Everything the stats screen shows, in one read.
pub fn compute(database: &Db, filter: &StatsFilter) -> Result<Stats> {
    let posts = database.list_posts()?;
    let accounts: HashMap<i64, db::Account> = database
        .list_accounts()?
        .into_iter()
        .map(|account| (account.id, account))
        .collect();
    let metrics: HashMap<i64, Metrics> = database
        .list_metrics()?
        .into_iter()
        .map(|row| (row.target_id, row))
        .collect();

    let mut totals = Totals::default();
    let mut engagement = Engagement::default();
    let mut groupers = Groupers::default();
    // Accounts, not platforms: two Threads connections can differ on whether
    // insights were granted, so the gap has to be asked per account.
    let mut accounts_in_scope: Vec<i64> = Vec::new();

    for detail in &posts {
        // Post-level counters describe INTENT and are not filtered by platform:
        // "3 drafts" is a fact about the queue, not about Bluesky.
        match detail.post.status.as_str() {
            db::POST_SCHEDULED | db::POST_PUBLISHING => totals.scheduled += 1,
            db::POST_DRAFT => totals.drafts += 1,
            db::POST_MISSED => totals.missed += 1,
            _ => {}
        }

        for target in &detail.targets {
            let Some(account) = accounts.get(&target.account_id) else {
                continue;
            };
            let at = dated_at(target, &detail.post);
            if !filter.matches(account, at) {
                continue;
            }
            if !accounts_in_scope.contains(&account.id) {
                accounts_in_scope.push(account.id);
            }

            let ok = target.status == db::TARGET_PUBLISHED;
            if !ok && target.status != db::TARGET_FAILED {
                continue;
            }
            if ok {
                totals.published += 1;
            } else {
                totals.failed += 1;
            }
            groupers.add(detail.post.id, target, account, at, ok);

            if ok && let Some(row) = metrics.get(&target.id) {
                engagement.add(row, &detail.post, target, account);
            }
        }
    }

    Ok(Stats {
        published: totals.published,
        failed: totals.failed,
        scheduled: totals.scheduled,
        drafts: totals.drafts,
        missed: totals.missed,
        by_platform: groupers.platform.finish(Sort::Count),
        by_account: groupers.account.finish(Sort::Count),
        by_hour: groupers.hour.finish(Sort::Key),
        by_day: groupers.day.finish(Sort::Key),
        by_slot: groupers.slot.finish(Sort::Key),
        failures: groupers.failure.finish(Sort::Count),
        engagement: engagement.totals.clone(),
        engagement_by_platform: engagement.by_platform(),
        top_posts: engagement.top_posts(),
        engagement_gaps: {
            let mut gaps: Vec<(PlatformId, String)> = Vec::new();
            for id in accounts_in_scope {
                let Some(account) = accounts.get(&id) else {
                    continue;
                };
                if let Some(reason) = engagement_gap(account)
                    && !gaps.iter().any(|(_, existing)| *existing == reason)
                {
                    gaps.push((account.platform, reason));
                }
            }
            gaps
        },
    })
}

/// How many destinations the leaderboard carries. Ten is the length of a list
/// someone reads rather than scrolls.
const TOP_POSTS: usize = 10;

/// The first line of a body, short enough to sit in a table cell. Truncated on
/// a character boundary, which `&body[..80]` would not guarantee.
fn excerpt(body: &str) -> String {
    let line = body
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim();
    if line.chars().count() <= 80 {
        return line.to_string();
    }
    let mut out: String = line.chars().take(79).collect();
    out.push('…');
    out
}

#[derive(Default)]
struct Totals {
    published: i64,
    failed: i64,
    scheduled: i64,
    drafts: i64,
    missed: i64,
}

/// The engagement side of one `compute`, accumulated as destinations are
/// walked: the totals, the same numbers split per platform, and every measured
/// destination as a leaderboard candidate.
#[derive(Default)]
struct Engagement {
    totals: EngagementTotals,
    per_platform: HashMap<PlatformId, EngagementRow>,
    leaderboard: Vec<TopPost>,
}

impl Engagement {
    /// Folds one measured destination in. An unreported dimension adds nothing
    /// rather than a zero, and the oldest fetch wins — the totals are only as
    /// fresh as their stalest part.
    fn add(&mut self, row: &Metrics, post: &db::Post, target: &PostTarget, account: &db::Account) {
        let (likes, reposts, replies, views) = (row.likes, row.reposts, row.replies, row.views);

        fold(&mut self.totals.likes, likes);
        fold(&mut self.totals.reposts, reposts);
        fold(&mut self.totals.replies, replies);
        fold(&mut self.totals.views, views);
        self.totals.measured += 1;
        if self
            .totals
            .oldest_fetch
            .as_deref()
            .is_none_or(|current| row.fetched_at.as_str() < current)
        {
            self.totals.oldest_fetch = Some(row.fetched_at.clone());
        }

        let platform = account.platform;
        let entry = self
            .per_platform
            .entry(platform)
            .or_insert_with(|| EngagementRow {
                platform,
                label: platform.label(),
                likes: None,
                reposts: None,
                replies: None,
                views: None,
                measured: 0,
                post_ids: Vec::new(),
            });
        fold(&mut entry.likes, likes);
        fold(&mut entry.reposts, reposts);
        fold(&mut entry.replies, replies);
        fold(&mut entry.views, views);
        entry.measured += 1;
        if !entry.post_ids.contains(&post.id) {
            entry.post_ids.push(post.id);
        }

        self.leaderboard.push(TopPost {
            post_id: post.id,
            platform,
            handle: account.handle.clone(),
            excerpt: excerpt(&post.body),
            published_at: target.published_at.clone(),
            remote_url: target.remote_url.clone(),
            likes,
            reposts,
            replies,
            views,
            interactions: earned(likes, reposts, replies),
            fetched_at: row.fetched_at.clone(),
        });
    }

    /// Per-platform rows, ranked by what was earned — so the platform doing the
    /// work is the first bar rather than whichever one hashed first.
    fn by_platform(&self) -> Vec<EngagementRow> {
        let mut rows: Vec<EngagementRow> = self.per_platform.values().cloned().collect();
        rows.sort_by(|a, b| {
            earned(b.likes, b.reposts, b.replies)
                .cmp(&earned(a.likes, a.reposts, a.replies))
                .then_with(|| a.label.cmp(b.label))
        });
        rows
    }

    /// The leaderboard, longest-earning first and cut to [`TOP_POSTS`].
    fn top_posts(&self) -> Vec<TopPost> {
        let mut rows = self.leaderboard.clone();
        rows.sort_by(|a, b| {
            b.interactions
                .cmp(&a.interactions)
                .then_with(|| b.views.cmp(&a.views))
        });
        rows.truncate(TOP_POSTS);
        rows
    }
}

/// Adds a reported value to a sum that stays `None` until something reports —
/// so "no row says" and "the rows say zero" never collapse into one another.
fn fold(sum: &mut Option<i64>, value: Option<i64>) {
    if let Some(value) = value {
        *sum = Some(sum.unwrap_or(0) + value);
    }
}

/// Likes + reposts + replies over the ones reported: what a ranking can sort by
/// when some dimensions are unknown.
fn earned(likes: Option<i64>, reposts: Option<i64>, replies: Option<i64>) -> i64 {
    [likes, reposts, replies].into_iter().flatten().sum()
}

/// Why this account's engagement is not shown. `None` means it is readable.
///
/// Per account rather than per platform, because two of the gaps are a matter
/// of what the connection was granted rather than what the platform offers:
/// Threads and Instagram serve `/insights` only to a token carrying the
/// insights scope, which is opt-in on the Meta app credential. The granted
/// scopes are on the account row, so this costs no keychain read.
fn engagement_gap(account: &db::Account) -> Option<String> {
    match account.platform {
        // Readable on credentials the app already holds: Bluesky and Mastodon
        // serve public counts, a Page post carries its own summary edges, and
        // X returns `public_metrics` — though every X id in a lookup bills
        // against the app's credits, which is why that refresh confirms the
        // cost first rather than running on its own.
        PlatformId::Bluesky | PlatformId::Mastodon | PlatformId::Facebook | PlatformId::X => None,
        PlatformId::Threads | PlatformId::Instagram => {
            let needed = insights_scope(account.platform)?;
            if granted(account, needed) {
                None
            } else {
                Some(format!(
                    "Insights need the {needed} scope. Turn on \"Insights access\" in \
                     Settings → Platform apps, then reconnect {}.",
                    account.handle
                ))
            }
        }
        PlatformId::Reddit => {
            Some("Reddit's score needs a read scope Windbag does not request.".into())
        }
        PlatformId::Linkedin => {
            Some("LinkedIn's analytics need approved read permissions on your app.".into())
        }
    }
}

/// The scope a platform's per-post insights endpoint requires, where that is
/// the only thing standing between Windbag and the numbers.
pub fn insights_scope(platform: PlatformId) -> Option<&'static str> {
    match platform {
        PlatformId::Threads => Some("threads_manage_insights"),
        PlatformId::Instagram => Some("instagram_business_manage_insights"),
        _ => None,
    }
}

/// Whether the connection actually came back with a scope. Matched on word
/// boundaries against the stored grant: Meta returns them comma-separated, the
/// OAuth spec says space-separated, and `contains` alone would let
/// `threads_manage_insights_foo` pass for the real thing.
fn granted(account: &db::Account, scope: &str) -> bool {
    account.scopes.as_deref().is_some_and(|scopes| {
        scopes
            .split([',', ' '])
            .any(|granted| granted.trim() == scope)
    })
}

/// The `[CODE]` an error string was recorded under. Errors are written by
/// `AppError`'s Display, so the prefix is always there — but a hand-edited store
/// or a future variant should degrade to a bucket rather than to a panic.
fn error_code(error: &str) -> &str {
    error
        .strip_prefix('[')
        .and_then(|rest| rest.split_once(']'))
        .map_or("OTHER", |(code, _)| code)
}

fn failure_label(code: &str) -> &str {
    match code {
        "UNAUTHORIZED" => "Credentials",
        "INVALID_INPUT" => "Rejected by the platform",
        "PLATFORM" => "Rate limit or outage",
        "NETWORK" => "Network",
        "CONFLICT" => "Duplicate or conflict",
        "NOT_FOUND" => "Not found",
        "INTERNAL" => "Windbag",
        other => other,
    }
}

#[derive(Default)]
struct Grouper {
    order: Vec<String>,
    rows: HashMap<String, Bucket>,
}

enum Sort {
    /// Biggest first — for "which platform do I use most".
    Count,
    /// By key — for anything with a natural order (hours, days).
    Key,
}

impl Grouper {
    fn add(&mut self, key: &str, label: &str, post_id: i64, ok: bool) {
        let bucket = self.rows.entry(key.to_string()).or_insert_with(|| {
            self.order.push(key.to_string());
            Bucket {
                key: key.to_string(),
                label: label.to_string(),
                published: 0,
                failed: 0,
                post_ids: Vec::new(),
            }
        });
        if ok {
            bucket.published += 1;
        } else {
            bucket.failed += 1;
        }
        // One post can have several destinations in the same bucket; the
        // drilldown wants the post once.
        if !bucket.post_ids.contains(&post_id) {
            bucket.post_ids.push(post_id);
        }
    }

    fn finish(self, sort: Sort) -> Vec<Bucket> {
        let mut rows: Vec<Bucket> = self.rows.into_values().collect();
        match sort {
            Sort::Count => rows.sort_by(|a, b| {
                (b.published + b.failed)
                    .cmp(&(a.published + a.failed))
                    .then_with(|| a.key.cmp(&b.key))
            }),
            Sort::Key => rows.sort_by(|a, b| a.key.cmp(&b.key)),
        }
        rows
    }
}

// ─── Engagement refresh ─────────────────────────────────────────────────────

/// What one refresh did. Reported rather than silently absorbed: a refresh that
/// updated three of eleven destinations needs to say so.
///
/// `updated + skipped + failed` is every published destination, so nothing a
/// refresh touched goes unaccounted for.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshReport {
    /// Destinations whose counts were written — including those an account's
    /// pass wrote before it stopped on an error.
    pub updated: usize,
    /// Destinations not read: accounts Windbag cannot read engagement for, and
    /// posts that no longer exist on the platform.
    pub skipped: usize,
    /// Destinations an account's pass had not reached when it stopped on an
    /// error.
    pub failed: usize,
    /// X ids sent in lookups X answered successfully. Each is billed against the
    /// app's credits whether or not the refresh finished, so it is reported even
    /// when the pass then failed.
    pub billed_reads: usize,
    /// One line per platform or account that could not be read.
    pub problems: Vec<String>,
}

/// What one account's pass got through, filled as it goes so an error midway
/// cannot erase the rows already written.
#[derive(Default)]
struct Tally {
    updated: usize,
    billed: usize,
}

impl RefreshReport {
    /// Folds one account's pass in: what it wrote counts as updated whether or
    /// not it finished, and the rest of its destinations count as failed if it
    /// stopped on an error, or as skipped if it finished without them (a post
    /// deleted on the platform).
    fn record(&mut self, handle: &str, total: usize, tally: &Tally, outcome: Result<()>) {
        self.updated += tally.updated;
        self.billed_reads += tally.billed;
        let rest = total.saturating_sub(tally.updated);
        match outcome {
            Ok(()) => self.skipped += rest,
            Err(err) => {
                self.failed += rest;
                self.problems.push(format!("{handle}: {err}"));
            }
        }
    }
}

/// What a refresh would read, before it runs.
///
/// Exists for one reason: X bills per id in a lookup, so the button that spends
/// that money has to be able to say how much first. Everything else is free and
/// is reported only so the confirmation can say what the spend buys.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshCost {
    /// Destinations on X that would be looked up. Each one is a billed read.
    pub billed_reads: i64,
    /// Destinations on the platforms that serve counts for nothing.
    pub free_reads: i64,
}

/// Counts what [`refresh_engagement`] would read, without reading anything.
pub fn refresh_cost(database: &Db) -> Result<RefreshCost> {
    let accounts: HashMap<i64, db::Account> = database
        .list_accounts()?
        .into_iter()
        .map(|account| (account.id, account))
        .collect();

    let mut cost = RefreshCost::default();
    for (target, account) in database.published_targets()? {
        // A destination with no remote id was never really published, so no
        // lookup would be made for it.
        if target.remote_id.is_none() {
            continue;
        }
        let Some(account) = accounts.get(&account.id) else {
            continue;
        };
        if engagement_gap(account).is_some() {
            continue;
        }
        if account.platform == PlatformId::X {
            cost.billed_reads += 1;
        } else {
            cost.free_reads += 1;
        }
    }
    Ok(cost)
}

/// Fetches engagement for every published destination Windbag can read.
///
/// Bluesky is batched (its `getPosts` takes up to 25 URIs per call); Mastodon is
/// one call per status, which is fine at the volumes a personal queue reaches.
/// A failure on one account never stops the others — the report carries it.
pub fn refresh_engagement(database: &Db) -> Result<RefreshReport> {
    let mut report = RefreshReport::default();
    let published = database.published_targets()?;

    // Grouped by account: Bluesky needs one session per account anyway, and
    // Mastodon needs the account's instance and token.
    let mut by_account: HashMap<i64, (db::Account, Vec<PostTarget>)> = HashMap::new();
    for (target, account) in published {
        if engagement_gap(&account).is_some() {
            report.skipped += 1;
            continue;
        }
        by_account
            .entry(account.id)
            .or_insert_with(|| (account, Vec::new()))
            .1
            .push(target);
    }

    for (account, targets) in by_account.into_values() {
        let mut tally = Tally::default();
        let outcome = match account.platform {
            PlatformId::Bluesky => refresh_bluesky(database, &targets, &mut tally),
            PlatformId::Mastodon => refresh_mastodon(database, &account, &targets, &mut tally),
            PlatformId::Threads => refresh_threads(database, &account, &targets, &mut tally),
            PlatformId::Instagram => refresh_instagram(database, &account, &targets, &mut tally),
            PlatformId::Facebook => refresh_facebook(database, &account, &targets, &mut tally),
            PlatformId::X => refresh_x(database, &account, &targets, &mut tally),
            // Gated above; a new variant lands here and is reported as skipped
            // rather than silently counting as updated.
            PlatformId::Reddit | PlatformId::Linkedin => Ok(()),
        };
        report.record(&account.handle, targets.len(), &tally, outcome);
    }
    Ok(report)
}

/// Bluesky's public counts, batched 25 URIs at a time.
///
/// Read from the public `AppView`, unauthenticated. `getPosts` serves the counts
/// to anyone, so this used to open a session with the stored app password for
/// no reason — and once "Sign in with Bluesky" became the default there IS no
/// stored app password, which would have failed the refresh for every new
/// account with a message about a credential the user never created.
fn refresh_bluesky(database: &Db, targets: &[PostTarget], tally: &mut Tally) -> Result<()> {
    let now = db::now_rfc3339();

    for chunk in targets.chunks(BLUESKY_LOOKUP_BATCH) {
        let query: Vec<(&str, &str)> = chunk
            .iter()
            .filter_map(|target| target.remote_id.as_deref().map(|uri| ("uris", uri)))
            .collect();
        if query.is_empty() {
            continue;
        }

        let (status, body) = http::read_body(
            http::client()
                .get(format!("{BLUESKY_APPVIEW}/xrpc/app.bsky.feed.getPosts"))
                .query(&query)
                .send()?,
        );
        if !(200..300).contains(&status) {
            return Err(from_status(status, &body, "Bluesky"));
        }
        let parsed: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| AppError::Platform(format!("Bluesky returned unreadable posts: {e}")))?;
        let Some(posts) = parsed.get("posts").and_then(serde_json::Value::as_array) else {
            continue;
        };

        // Matched back by URI: `getPosts` may return fewer than asked for (a
        // deleted post simply is not in the reply), so position is not an index.
        let by_uri: HashMap<&str, &serde_json::Value> = posts
            .iter()
            .filter_map(|post| {
                post.get("uri")
                    .and_then(serde_json::Value::as_str)
                    .map(|uri| (uri, post))
            })
            .collect();

        for target in chunk {
            let Some(post) = target.remote_id.as_deref().and_then(|uri| by_uri.get(uri)) else {
                continue;
            };
            database.save_metrics(&Metrics {
                target_id: target.id,
                fetched_at: now.clone(),
                likes: count(post, "likeCount"),
                reposts: count(post, "repostCount"),
                replies: count(post, "replyCount"),
                quotes: count(post, "quoteCount"),
                // Bluesky publishes no impression count.
                views: None,
            })?;
            tally.updated += 1;
        }
    }
    Ok(())
}

/// The unauthenticated `AppView`. Not the account's PDS: a PDS serves the
/// repository, the `AppView` serves the aggregated counts.
const BLUESKY_APPVIEW: &str = "https://public.api.bsky.app";

/// How many URIs one `getPosts` accepts.
const BLUESKY_LOOKUP_BATCH: usize = 25;

/// Mastodon's per-status counts. One call each; a deleted status 404s and is
/// skipped rather than failing the account.
fn refresh_mastodon(
    database: &Db,
    account: &db::Account,
    targets: &[PostTarget],
    tally: &mut Tally,
) -> Result<()> {
    let instance = account
        .instance
        .as_deref()
        .ok_or_else(|| AppError::Internal("This Mastodon account has no instance.".into()))?;
    let secret = secrets::load_account_secret(account.platform, &account.remote_id)?;

    let now = db::now_rfc3339();
    for target in targets {
        let Some(id) = target.remote_id.as_deref() else {
            continue;
        };
        let (status, body) = http::read_body(
            http::client()
                .get(format!("{instance}/api/v1/statuses/{id}"))
                .bearer_auth(&secret.access_token)
                .send()?,
        );
        if status == 404 || status == 410 {
            continue;
        }
        if !(200..300).contains(&status) {
            return Err(from_status(status, &body, "Mastodon"));
        }
        let Ok(post) = serde_json::from_str::<serde_json::Value>(&body) else {
            continue;
        };
        database.save_metrics(&Metrics {
            target_id: target.id,
            fetched_at: now.clone(),
            likes: count(&post, "favourites_count"),
            reposts: count(&post, "reblogs_count"),
            replies: count(&post, "replies_count"),
            quotes: None,
            // Mastodon publishes no impression count.
            views: None,
        })?;
        tally.updated += 1;
    }
    Ok(())
}

/// Threads' per-media `/insights`, one call per post.
///
/// Requires `threads_manage_insights`, which [`engagement_gap`] has already
/// checked was granted before this runs.
fn refresh_threads(
    database: &Db,
    account: &db::Account,
    targets: &[PostTarget],
    tally: &mut Tally,
) -> Result<()> {
    let secret = platforms::live_secret(database, account)?;
    let now = db::now_rfc3339();

    for target in targets {
        let Some(id) = target.remote_id.as_deref() else {
            continue;
        };
        let reply = meta::get_json(
            &format!("{}/{id}/insights", threads::API_BASE),
            &[
                ("metric", "views,likes,replies,reposts,quotes,shares"),
                ("access_token", &secret.access_token),
            ],
            "Threads",
        );
        let Some(named) = insight_values(reply)? else {
            continue;
        };
        database.save_metrics(&Metrics {
            target_id: target.id,
            fetched_at: now.clone(),
            likes: named.get("likes").copied(),
            reposts: named.get("reposts").copied(),
            replies: named.get("replies").copied(),
            quotes: named.get("quotes").copied(),
            views: named.get("views").copied(),
        })?;
        tally.updated += 1;
    }
    Ok(())
}

/// Instagram's per-media `/insights`.
///
/// `impressions` is deliberately not requested: Meta deprecated it for media
/// created after 2 July 2024, and asking for it on a newer post fails the whole
/// call rather than omitting one metric. `views` is its replacement. Instagram
/// reports no reposts or quotes, so both stay `None` rather than becoming zero.
fn refresh_instagram(
    database: &Db,
    account: &db::Account,
    targets: &[PostTarget],
    tally: &mut Tally,
) -> Result<()> {
    let secret = platforms::live_secret(database, account)?;
    let now = db::now_rfc3339();

    for target in targets {
        let Some(id) = target.remote_id.as_deref() else {
            continue;
        };
        let reply = meta::get_json(
            &format!("{}/{id}/insights", instagram::API_BASE),
            &[
                ("metric", "likes,comments,saved,shares,views,reach"),
                ("access_token", &secret.access_token),
            ],
            "Instagram",
        );
        let Some(named) = insight_values(reply)? else {
            continue;
        };
        database.save_metrics(&Metrics {
            target_id: target.id,
            fetched_at: now.clone(),
            likes: named.get("likes").copied(),
            reposts: None,
            replies: named.get("comments").copied(),
            quotes: None,
            views: named.get("views").copied(),
        })?;
        tally.updated += 1;
    }
    Ok(())
}

/// A Page post's own summary counts.
///
/// Not `/insights`: the summary edges come back on the Page token the adapter
/// already holds, where Page-level insights would need `read_insights` and the
/// App Review that permission carries. `shares` is Facebook's nearest thing to
/// a repost; it publishes no impression count on a post without insights.
fn refresh_facebook(
    database: &Db,
    account: &db::Account,
    targets: &[PostTarget],
    tally: &mut Tally,
) -> Result<()> {
    // The Page token IS the stored `access_token` — `connect` swaps the user
    // token for the Page's own one and keeps the user token in `extra`. So the
    // post reads back on exactly the credential that wrote it.
    let secret = platforms::live_secret(database, account)?;
    let token = secret.access_token.clone();

    let now = db::now_rfc3339();
    for target in targets {
        let Some(id) = target.remote_id.as_deref() else {
            continue;
        };
        let reply = meta::get_json(
            &format!("{}/{id}", facebook::api_base()),
            &[
                (
                    "fields",
                    "likes.summary(true),comments.summary(true),shares",
                ),
                ("access_token", &token),
            ],
            "Facebook",
        );
        let post = match reply {
            Ok(post) => post,
            // A post deleted on Facebook is not a failure of the refresh.
            Err(AppError::NotFound(_)) => continue,
            Err(err) => return Err(err),
        };
        database.save_metrics(&Metrics {
            target_id: target.id,
            fetched_at: now.clone(),
            likes: summary_total(&post, "likes"),
            reposts: post.get("shares").and_then(|shares| count(shares, "count")),
            replies: summary_total(&post, "comments"),
            quotes: None,
            views: None,
        })?;
        tally.updated += 1;
    }
    Ok(())
}

/// X's `public_metrics`, 100 ids per call.
///
/// Every id in the request is a billed read, which is why this is only ever
/// reached from a manual refresh the user confirmed the cost of.
fn refresh_x(
    database: &Db,
    account: &db::Account,
    targets: &[PostTarget],
    tally: &mut Tally,
) -> Result<()> {
    let secret = platforms::live_secret(database, account)?;
    let now = db::now_rfc3339();

    for chunk in targets.chunks(X_LOOKUP_BATCH) {
        let ids: Vec<&str> = chunk
            .iter()
            .filter_map(|target| target.remote_id.as_deref())
            .collect();
        if ids.is_empty() {
            continue;
        }

        let (status, body) = http::read_body(
            http::client()
                .get(format!("{}/tweets", x::API_BASE))
                .query(&[
                    ("ids", ids.join(",").as_str()),
                    ("tweet.fields", "public_metrics"),
                ])
                .bearer_auth(&secret.access_token)
                .send()?,
        );
        if !(200..300).contains(&status) {
            return Err(from_status(status, &body, "X"));
        }
        // Counted the moment X answers, before anything below can fail: this
        // lookup is on the bill whether or not its rows get written.
        tally.billed += ids.len();
        let parsed: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| AppError::Platform(format!("X returned unreadable posts: {e}")))?;
        let Some(posts) = parsed.get("data").and_then(serde_json::Value::as_array) else {
            continue;
        };

        // Matched back by id: a deleted post is simply absent from `data`, so
        // position is not an index.
        let by_id: HashMap<&str, &serde_json::Value> = posts
            .iter()
            .filter_map(|post| {
                post.get("id")
                    .and_then(serde_json::Value::as_str)
                    .map(|id| (id, post))
            })
            .collect();

        for target in chunk {
            let Some(post) = target.remote_id.as_deref().and_then(|id| by_id.get(id)) else {
                continue;
            };
            let Some(m) = post.get("public_metrics") else {
                continue;
            };
            database.save_metrics(&Metrics {
                target_id: target.id,
                fetched_at: now.clone(),
                likes: count(m, "like_count"),
                reposts: count(m, "repost_count").or_else(|| count(m, "retweet_count")),
                replies: count(m, "reply_count"),
                quotes: count(m, "quote_count"),
                views: count(m, "impression_count"),
            })?;
            tally.updated += 1;
        }
    }
    Ok(())
}

/// How many ids one `GET /2/tweets` accepts.
const X_LOOKUP_BATCH: usize = 100;

/// Flattens a Meta `/insights` reply into `name → value`.
///
/// Meta returns metrics two ways — `values: [{ value }]` on the periodic ones
/// and `total_value: { value }` on the lifetime ones — and which shape a given
/// metric uses is not stable across Graph versions, so both are read. A media
/// object deleted on the platform 404s, which advances nothing and is not an
/// error; `Ok(None)` says "skip this one".
fn insight_values(reply: Result<serde_json::Value>) -> Result<Option<HashMap<String, i64>>> {
    let payload = match reply {
        Ok(payload) => payload,
        Err(AppError::NotFound(_)) => return Ok(None),
        Err(err) => return Err(err),
    };
    let Some(rows) = payload.get("data").and_then(serde_json::Value::as_array) else {
        return Ok(None);
    };

    let mut named = HashMap::new();
    for row in rows {
        let Some(name) = row.get("name").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let value = row
            .get("values")
            .and_then(serde_json::Value::as_array)
            .and_then(|values| values.first())
            .and_then(|first| count(first, "value"))
            .or_else(|| row.get("total_value").and_then(|tv| count(tv, "value")));
        if let Some(value) = value {
            named.insert(name.to_string(), value);
        }
    }
    Ok(Some(named))
}

/// A `likes.summary(true)` style total.
fn summary_total(post: &serde_json::Value, edge: &str) -> Option<i64> {
    post.get(edge)
        .and_then(|edge| edge.get("summary"))
        .and_then(|summary| count(summary, "total_count"))
}

/// A count field, or `None` when the platform did not report that dimension —
/// which is not the same as reporting zero.
fn count(value: &serde_json::Value, key: &str) -> Option<i64> {
    value.get(key).and_then(serde_json::Value::as_i64)
}

/// The default window a stats view opens on: the last 30 days.
pub fn default_since() -> String {
    (Utc::now() - chrono::Duration::days(30)).to_rfc3339()
}

/// Parses a bound, rejecting anything the store could not have written.
pub fn parse_bound(value: Option<&str>) -> Result<Option<DateTime<Utc>>> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(db::parse_rfc3339)
        .transpose()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::platforms::{AccountSecret, Connected};

    fn connected(remote_id: &str, handle: &str) -> Connected {
        Connected {
            remote_id: remote_id.into(),
            handle: handle.into(),
            display_name: None,
            avatar_url: None,
            instance: None,
            scopes: None,
            char_limit: None,
            secret: AccountSecret::default(),
        }
    }

    /// Two accounts, three posts: one fully published, one failed, one draft.
    fn seeded() -> (Db, i64, i64) {
        let db = Db::open_in_memory().expect("store");
        let bluesky = db
            .upsert_account(PlatformId::Bluesky, &connected("did:1", "me.bsky.social"))
            .expect("bluesky");
        let x = db
            .upsert_account(PlatformId::X, &connected("x:1", "@me"))
            .expect("x");

        let sent = db
            .create_post(
                "sent",
                None,
                None,
                Some(&db::now_rfc3339()),
                db::POST_SCHEDULED,
            )
            .expect("post");
        db.set_targets(sent, &[(bluesky, json!({})), (x, json!({}))])
            .expect("targets");
        let targets = db.list_targets(sent).expect("targets");
        db.finish_target_ok(targets[0].id, "at://1", None)
            .expect("ok");
        db.finish_target_err(targets[1].id, "[PLATFORM] rate limited", None)
            .expect("fail");
        db.reconcile_post_status(sent).expect("status");

        db.create_post("draft", None, None, None, db::POST_DRAFT)
            .expect("draft");
        (db, bluesky, x)
    }

    #[test]
    fn totals_count_destinations_and_intent_separately() {
        let (db, _, _) = seeded();
        let stats = compute(&db, &StatsFilter::default()).expect("stats");
        assert_eq!(stats.published, 1);
        assert_eq!(stats.failed, 1);
        assert_eq!(stats.drafts, 1, "a draft is a post, not a destination");
        assert_eq!(stats.scheduled, 0, "the post finished, partly");
    }

    #[test]
    fn a_platform_filter_narrows_destinations() {
        let (db, _, _) = seeded();
        let stats = compute(
            &db,
            &StatsFilter {
                platforms: vec![PlatformId::Bluesky],
                ..Default::default()
            },
        )
        .expect("stats");
        assert_eq!(stats.published, 1);
        assert_eq!(stats.failed, 0, "X's failure is out of scope");
        assert_eq!(stats.by_platform.len(), 1);
    }

    #[test]
    fn an_account_filter_narrows_to_one_handle() {
        let (db, _, x) = seeded();
        let stats = compute(
            &db,
            &StatsFilter {
                account_ids: vec![x],
                ..Default::default()
            },
        )
        .expect("stats");
        assert_eq!(stats.published, 0);
        assert_eq!(stats.failed, 1);
    }

    #[test]
    fn buckets_carry_the_posts_behind_them_for_drilldown() {
        let (db, _, _) = seeded();
        let stats = compute(&db, &StatsFilter::default()).expect("stats");
        let bluesky = stats
            .by_platform
            .iter()
            .find(|bucket| bucket.key == "bluesky")
            .expect("bluesky bucket");
        assert_eq!(bluesky.post_ids.len(), 1);
        assert!(bluesky.post_ids[0] > 0);
    }

    #[test]
    fn failures_are_grouped_by_their_code() {
        let (db, _, _) = seeded();
        let stats = compute(&db, &StatsFilter::default()).expect("stats");
        assert_eq!(stats.failures.len(), 1);
        assert_eq!(stats.failures[0].key, "PLATFORM");
        assert_eq!(stats.failures[0].label, "Rate limit or outage");
    }

    #[test]
    fn an_error_without_a_code_still_lands_in_a_bucket() {
        assert_eq!(error_code("no code here"), "OTHER");
        assert_eq!(error_code("[UNAUTHORIZED] token revoked"), "UNAUTHORIZED");
    }

    #[test]
    fn engagement_reports_which_accounts_it_cannot_read() {
        let (db, _, _) = seeded();
        let stats = compute(&db, &StatsFilter::default()).expect("stats");
        let gaps: Vec<PlatformId> = stats.engagement_gaps.iter().map(|(id, _)| *id).collect();
        // Both seeded platforms are readable now that X's bulk lookup is wired:
        // X costs money per read, which is a confirmation, not a gap.
        assert!(
            !gaps.contains(&PlatformId::X),
            "X's public_metrics are read"
        );
        assert!(!gaps.contains(&PlatformId::Bluesky), "Bluesky's are free");
    }

    #[test]
    fn reddit_and_linkedin_stay_gapped() {
        let db = Db::open_in_memory().expect("store");
        for (platform, remote) in [(PlatformId::Reddit, "r:1"), (PlatformId::Linkedin, "li:1")] {
            let id = db
                .upsert_account(platform, &connected(remote, "@me"))
                .expect("account");
            let post = db
                .create_post(
                    "sent",
                    None,
                    None,
                    Some(&db::now_rfc3339()),
                    db::POST_SCHEDULED,
                )
                .expect("post");
            db.set_targets(post, &[(id, json!({}))]).expect("targets");
            let targets = db.list_targets(post).expect("targets");
            db.finish_target_ok(targets[0].id, "1", None).expect("ok");
            db.reconcile_post_status(post).expect("status");
        }

        let stats = compute(&db, &StatsFilter::default()).expect("stats");
        let gaps: Vec<PlatformId> = stats.engagement_gaps.iter().map(|(id, _)| *id).collect();
        assert!(gaps.contains(&PlatformId::Reddit));
        assert!(gaps.contains(&PlatformId::Linkedin));
    }

    /// The insights gate is per account, so a Threads connection made before
    /// insights were turned on is reported while a later one is not.
    #[test]
    fn threads_insights_are_gated_on_the_granted_scope() {
        let db = Db::open_in_memory().expect("store");

        let mut without = connected("th:1", "@old");
        without.scopes = Some("threads_basic,threads_content_publish".into());
        let mut with = connected("th:2", "@new");
        with.scopes = Some("threads_basic,threads_content_publish,threads_manage_insights".into());

        for account in [&without, &with] {
            let id = db
                .upsert_account(PlatformId::Threads, account)
                .expect("account");
            let post = db
                .create_post(
                    "sent",
                    None,
                    None,
                    Some(&db::now_rfc3339()),
                    db::POST_SCHEDULED,
                )
                .expect("post");
            db.set_targets(post, &[(id, json!({}))]).expect("targets");
            let targets = db.list_targets(post).expect("targets");
            db.finish_target_ok(targets[0].id, "1", None).expect("ok");
            db.reconcile_post_status(post).expect("status");
        }

        let stats = compute(&db, &StatsFilter::default()).expect("stats");
        assert_eq!(
            stats.engagement_gaps.len(),
            1,
            "only the connection missing the scope is a gap: {:?}",
            stats.engagement_gaps
        );
        assert!(stats.engagement_gaps[0].1.contains("@old"));
    }

    #[test]
    fn a_scope_matches_on_word_boundaries_not_substrings() {
        let mut account = connected("th:1", "@me");
        account.scopes = Some("threads_basic,threads_manage_insights_extra".into());
        let db = Db::open_in_memory().expect("store");
        let id = db
            .upsert_account(PlatformId::Threads, &account)
            .expect("account");
        let stored = db
            .list_accounts()
            .expect("accounts")
            .into_iter()
            .find(|row| row.id == id)
            .expect("row");

        assert!(
            !granted(&stored, "threads_manage_insights"),
            "a longer scope name must not pass for the real one"
        );
    }

    #[test]
    fn a_meta_insights_reply_is_flattened_from_either_shape() {
        let payload = json!({
            "data": [
                { "name": "views", "values": [{ "value": 1_200 }] },
                { "name": "likes", "total_value": { "value": 34 } },
                { "name": "replies", "values": [] }
            ]
        });
        let named = insight_values(Ok(payload)).expect("ok").expect("some");

        assert_eq!(named.get("views"), Some(&1_200));
        assert_eq!(named.get("likes"), Some(&34), "total_value is read too");
        assert!(
            !named.contains_key("replies"),
            "a metric with no value is absent, not zero"
        );
    }

    #[test]
    fn a_deleted_media_object_is_skipped_rather_than_failing_the_account() {
        let missing = insight_values(Err(AppError::NotFound("gone".into()))).expect("not an error");
        assert!(missing.is_none());
    }

    #[test]
    fn an_excerpt_takes_the_first_real_line_and_keeps_char_boundaries() {
        assert_eq!(excerpt("\n\n  hello there \nsecond"), "hello there");
        let wide = "\u{1f600}".repeat(200);
        let cut = excerpt(&wide);
        assert_eq!(cut.chars().count(), 80, "79 chars plus the ellipsis");
        assert!(cut.ends_with('\u{2026}'));
    }

    #[test]
    fn nothing_measured_is_distinguishable_from_zero_engagement() {
        let (db, _, _) = seeded();
        let stats = compute(&db, &StatsFilter::default()).expect("stats");
        assert_eq!(stats.engagement.measured, 0);
        assert!(stats.engagement.oldest_fetch.is_none());
    }

    #[test]
    fn measured_engagement_sums_and_reports_its_oldest_fetch() {
        let (db, _, _) = seeded();
        let target = db
            .published_targets()
            .expect("published")
            .first()
            .expect("one")
            .0
            .id;
        db.save_metrics(&Metrics {
            target_id: target,
            fetched_at: "2026-01-01T00:00:00+00:00".into(),
            likes: Some(7),
            reposts: Some(2),
            replies: None,
            quotes: None,
            views: None,
        })
        .expect("metrics");

        let stats = compute(&db, &StatsFilter::default()).expect("stats");
        assert_eq!(stats.engagement.likes, Some(7));
        assert_eq!(
            stats.engagement.replies, None,
            "a dimension no measured row reports is unknown, not zero"
        );
        assert_eq!(stats.engagement.views, None);
        assert_eq!(stats.engagement.measured, 1);

        let row = &stats.engagement_by_platform[0];
        assert_eq!((row.likes, row.replies, row.views), (Some(7), None, None));
        assert_eq!(
            row.post_ids,
            vec![stats.top_posts[0].post_id],
            "the engagement bar drills down to the measured post"
        );
        let top = &stats.top_posts[0];
        assert_eq!((top.likes, top.replies, top.views), (Some(7), None, None));
        assert_eq!(top.interactions, 9, "ranked on what was reported");
        assert_eq!(
            stats.engagement.oldest_fetch.as_deref(),
            Some("2026-01-01T00:00:00+00:00")
        );
    }

    #[test]
    fn a_pass_that_fails_midway_still_reports_what_it_wrote_and_billed() {
        // The regression this guards: an error after 150 billed X reads used to
        // report "updated 0, skipped 250", discarding rows already written and
        // hiding money already spent.
        let mut report = RefreshReport::default();
        let tally = Tally {
            updated: 150,
            billed: 200,
        };
        report.record(
            "@me",
            250,
            &tally,
            Err(AppError::Platform("rate limited".into())),
        );
        assert_eq!(report.updated, 150);
        assert_eq!(report.failed, 100, "the destinations it never reached");
        assert_eq!(report.skipped, 0);
        assert_eq!(report.billed_reads, 200);
        assert_eq!(report.problems.len(), 1);
        assert!(report.problems[0].starts_with("@me: "));
    }

    #[test]
    fn a_finished_pass_counts_what_it_could_not_find_as_skipped() {
        let mut report = RefreshReport::default();
        let tally = Tally {
            updated: 9,
            billed: 0,
        };
        report.record("me.bsky.social", 10, &tally, Ok(()));
        assert_eq!((report.updated, report.skipped, report.failed), (9, 1, 0));
        assert!(report.problems.is_empty());
    }

    #[test]
    fn a_dimension_is_summed_over_the_rows_that_report_it() {
        let mut sum = None;
        fold(&mut sum, None);
        assert_eq!(sum, None, "nothing reported yet");
        fold(&mut sum, Some(0));
        assert_eq!(sum, Some(0), "a reported zero is a zero");
        fold(&mut sum, Some(5));
        fold(&mut sum, None);
        assert_eq!(sum, Some(5));
    }

    #[test]
    fn a_failed_destination_stays_in_range_despite_having_no_publish_time() {
        // The regression this guards: a failure has no `published_at`, so dating
        // it by that alone dropped every one of them out of the default view and
        // "why things failed" was always empty.
        let (db, _, _) = seeded();
        let stats = compute(
            &db,
            &StatsFilter {
                since: Some((Utc::now() - chrono::Duration::days(30)).to_rfc3339()),
                ..Default::default()
            },
        )
        .expect("stats");
        assert_eq!(stats.failed, 1, "a failure must survive a date filter");
        assert_eq!(stats.failures.len(), 1);
    }

    #[test]
    fn a_failure_is_drawn_on_the_day_the_filter_dated_it() {
        // The regression this guards: `by_day` was filled only for published
        // destinations, so the delivery chart's "Failed" series was always zero.
        let (db, _, _) = seeded();
        let stats = compute(&db, &StatsFilter::default()).expect("stats");
        let failed: i64 = stats.by_day.iter().map(|day| day.failed).sum();
        let published: i64 = stats.by_day.iter().map(|day| day.published).sum();
        assert_eq!(failed, 1, "the failure is on the chart: {:?}", stats.by_day);
        assert_eq!(published, 1);
    }

    #[test]
    fn a_date_range_excludes_what_falls_outside_it() {
        let (db, _, _) = seeded();
        let future = (Utc::now() + chrono::Duration::days(1)).to_rfc3339();
        let stats = compute(
            &db,
            &StatsFilter {
                since: Some(future),
                ..Default::default()
            },
        )
        .expect("stats");
        assert_eq!(stats.published, 0);
        assert_eq!(stats.failed, 0);
    }

    #[test]
    fn hours_sort_by_clock_and_platforms_by_volume() {
        let mut grouper = Grouper::default();
        grouper.add("09", "09:00", 1, true);
        grouper.add("02", "02:00", 2, true);
        let hours = grouper.finish(Sort::Key);
        assert_eq!(hours[0].key, "02");

        let mut grouper = Grouper::default();
        grouper.add("rare", "Rare", 1, true);
        grouper.add("common", "Common", 2, true);
        grouper.add("common", "Common", 3, true);
        let volume = grouper.finish(Sort::Count);
        assert_eq!(volume[0].key, "common");
    }

    #[test]
    fn one_post_with_two_destinations_in_a_bucket_is_counted_once_for_drilldown() {
        let mut grouper = Grouper::default();
        grouper.add("bluesky", "Bluesky", 42, true);
        grouper.add("bluesky", "Bluesky", 42, true);
        let rows = grouper.finish(Sort::Count);
        assert_eq!(rows[0].published, 2, "two destinations published");
        assert_eq!(rows[0].post_ids, vec![42], "but it is one post to open");
    }
}
