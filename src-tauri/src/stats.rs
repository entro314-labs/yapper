//! What actually happened: how much went out, where it landed, and what it
//! earned.
//!
//! Two different things live here, and keeping them apart is the point:
//!
//!   * **Delivery** — computed from Windbag's own store. Always available,
//!     always current, costs nothing: how many posts published, how many failed
//!     and why, which platform is flaky, what hour of the day you actually post
//!     at. This is the half that works the moment you have used the app.
//!   * **Engagement** — likes, reposts, replies, fetched from the platform.
//!     Only two of the five give it away: Bluesky and Mastodon serve public
//!     counts on endpoints the app already has credentials for. X's metrics need
//!     a paid tier and `LinkedIn`'s need approved read scopes, so those are
//!     reported as unavailable rather than shown as zero.
//!
//! Nothing here polls. Engagement is fetched when the user asks, because a
//! background poller against five APIs is a rate-limit budget spent on numbers
//! nobody is looking at.

use std::collections::HashMap;

use chrono::{DateTime, Datelike, Timelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::db::{self, Db, Metrics, PostTarget};
use crate::error::{AppError, Result, from_status};
use crate::http;
use crate::platforms::PlatformId;
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
    /// Whether one destination is in scope.
    ///
    /// `fallback_at` is the POST's own time, and it is load-bearing: a FAILED
    /// destination has no `published_at`, so dating it by that alone drops every
    /// failure out of any range — which would leave "why things failed" silently
    /// empty exactly when it matters most.
    fn matches(&self, target: &PostTarget, account: &db::Account, fallback_at: &str) -> bool {
        if !self.platforms.is_empty() && !self.platforms.contains(&account.platform) {
            return false;
        }
        if !self.account_ids.is_empty() && !self.account_ids.contains(&account.id) {
            return false;
        }
        let at = target.published_at.as_deref().unwrap_or(fallback_at);
        if self.since.as_deref().is_some_and(|since| at < since) {
            return false;
        }
        self.until.as_deref().is_none_or(|until| at < until)
    }
}

/// One row of a breakdown: a label, what it counts, and the ids behind it.
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngagementTotals {
    pub likes: i64,
    pub reposts: i64,
    pub replies: i64,
    /// How many destinations these totals are summed from — without it, "0
    /// likes" and "nothing fetched yet" look the same.
    pub measured: i64,
    /// The oldest fetch in the set: the totals are only as fresh as this.
    pub oldest_fetch: Option<String>,
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
    pub by_weekday: Vec<Bucket>,
    pub by_day: Vec<Bucket>,
    /// Failures grouped by their error code — the taxonomy that says whether a
    /// platform is flaky or the posts are wrong.
    pub failures: Vec<Bucket>,
    pub engagement: EngagementTotals,
    /// Platforms in scope whose engagement Windbag cannot read, with the reason.
    pub engagement_gaps: Vec<(PlatformId, &'static str)>,
}

/// The six breakdowns, filled together as destinations are walked.
#[derive(Default)]
struct Groupers {
    platform: Grouper,
    account: Grouper,
    hour: Grouper,
    weekday: Grouper,
    day: Grouper,
    failure: Grouper,
}

impl Groupers {
    /// Records one settled destination. Returns whether it published — the
    /// caller needs that to decide whether to look for engagement.
    fn add(&mut self, post_id: i64, target: &PostTarget, account: &db::Account, ok: bool) {
        self.platform.add(
            account.platform.as_str(),
            account.platform.label(),
            post_id,
            ok,
        );
        self.account
            .add(&account.id.to_string(), &account.handle, post_id, ok);

        if ok && let Some(at) = target.published_at.as_deref() {
            // Local time, because "when do I post" is a question about the
            // person's day, not about UTC.
            if let Ok(at) = db::parse_rfc3339(at) {
                let local = at.with_timezone(&chrono::Local);
                self.hour.add(
                    &format!("{:02}", local.hour()),
                    &format!("{:02}:00", local.hour()),
                    post_id,
                    true,
                );
                self.weekday.add(
                    &local.weekday().number_from_monday().to_string(),
                    &local.format("%a").to_string(),
                    post_id,
                    true,
                );
                let key = local.format("%Y-%m-%d").to_string();
                self.day.add(&key, &key, post_id, true);
            }
        }

        if !ok && let Some(error) = target.error.as_deref() {
            let code = error_code(error);
            self.failure.add(code, failure_label(code), post_id, false);
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
    let mut engagement = EngagementTotals {
        likes: 0,
        reposts: 0,
        replies: 0,
        measured: 0,
        oldest_fetch: None,
    };
    let mut groupers = Groupers::default();
    let mut platforms_in_scope: Vec<PlatformId> = Vec::new();

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
            // A post is dated by when it was meant to go out, falling back to
            // when it was last touched — the only timestamps a never-published
            // destination has.
            let fallback = detail
                .post
                .scheduled_at
                .as_deref()
                .unwrap_or(&detail.post.updated_at);
            if !filter.matches(target, account, fallback) {
                continue;
            }
            if !platforms_in_scope.contains(&account.platform) {
                platforms_in_scope.push(account.platform);
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
            groupers.add(detail.post.id, target, account, ok);

            if ok && let Some(row) = metrics.get(&target.id) {
                engagement.add(row);
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
        by_weekday: groupers.weekday.finish(Sort::Key),
        by_day: groupers.day.finish(Sort::Key),
        failures: groupers.failure.finish(Sort::Count),
        engagement,
        engagement_gaps: platforms_in_scope
            .into_iter()
            .filter_map(|id| engagement_gap(id).map(|reason| (id, reason)))
            .collect(),
    })
}

#[derive(Default)]
struct Totals {
    published: i64,
    failed: i64,
    scheduled: i64,
    drafts: i64,
    missed: i64,
}

impl EngagementTotals {
    /// Folds one destination's counts in. An unreported dimension adds nothing
    /// rather than a zero, and the oldest fetch wins — the totals are only as
    /// fresh as their stalest part.
    fn add(&mut self, row: &Metrics) {
        self.likes += row.likes.unwrap_or(0);
        self.reposts += row.reposts.unwrap_or(0);
        self.replies += row.replies.unwrap_or(0);
        self.measured += 1;
        if self
            .oldest_fetch
            .as_deref()
            .is_none_or(|current| row.fetched_at.as_str() < current)
        {
            self.oldest_fetch = Some(row.fetched_at.clone());
        }
    }
}

/// Why a platform's engagement is not shown. `None` means it is readable.
fn engagement_gap(platform: PlatformId) -> Option<&'static str> {
    match platform {
        PlatformId::Bluesky | PlatformId::Mastodon => None,
        PlatformId::X => Some("X's post metrics need a paid API tier."),
        PlatformId::Reddit => Some("Reddit's score needs a read scope Windbag does not request."),
        PlatformId::Linkedin => {
            Some("LinkedIn's analytics need approved read permissions on your app.")
        }
        // All three read insights under a separate `*_manage_insights`
        // permission Windbag does not request: asking for it at connect time
        // would widen the consent screen for every user to serve a panel most
        // never open.
        PlatformId::Threads => Some("Threads insights need the threads_manage_insights scope."),
        PlatformId::Instagram => {
            Some("Instagram insights need the instagram_business_manage_insights scope.")
        }
        PlatformId::Facebook => Some("Page insights need the read_insights permission."),
    }
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
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshReport {
    pub updated: usize,
    pub skipped: usize,
    /// One line per platform or account that could not be read.
    pub problems: Vec<String>,
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
        if engagement_gap(account.platform).is_some() {
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
        let outcome = match account.platform {
            PlatformId::Bluesky => refresh_bluesky(database, &account, &targets),
            PlatformId::Mastodon => refresh_mastodon(database, &account, &targets),
            _ => Ok(0),
        };
        match outcome {
            Ok(count) => report.updated += count,
            Err(err) => {
                report.skipped += targets.len();
                report.problems.push(format!("{}: {err}", account.handle));
            }
        }
    }
    Ok(report)
}

/// Bluesky's public counts, batched 25 URIs at a time.
fn refresh_bluesky(database: &Db, account: &db::Account, targets: &[PostTarget]) -> Result<usize> {
    let secret = secrets::load_account_secret(account.platform, &account.remote_id)?;
    let pds = secret
        .extra_str("pds")
        .unwrap_or("https://bsky.social")
        .to_string();
    let app_password = secret.extra_str("app_password").ok_or_else(|| {
        AppError::Unauthorized("The stored Bluesky app password is missing.".into())
    })?;

    let (status, body) = http::read_body(
        http::client()
            .post(format!("{pds}/xrpc/com.atproto.server.createSession"))
            .json(&json!({ "identifier": account.remote_id, "password": app_password }))
            .send()?,
    );
    if !(200..300).contains(&status) {
        return Err(from_status(status, &body, "Bluesky"));
    }
    let token = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|value| {
            value
                .get("accessJwt")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .ok_or_else(|| AppError::Platform("Bluesky returned an unreadable session.".into()))?;

    let now = db::now_rfc3339();
    let mut updated = 0usize;
    for chunk in targets.chunks(25) {
        let query: Vec<(&str, &str)> = chunk
            .iter()
            .filter_map(|target| target.remote_id.as_deref().map(|uri| ("uris", uri)))
            .collect();
        if query.is_empty() {
            continue;
        }

        let (status, body) = http::read_body(
            http::client()
                .get(format!("{pds}/xrpc/app.bsky.feed.getPosts"))
                .query(&query)
                .bearer_auth(&token)
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
            })?;
            updated += 1;
        }
    }
    Ok(updated)
}

/// Mastodon's per-status counts. One call each; a deleted status 404s and is
/// skipped rather than failing the account.
fn refresh_mastodon(database: &Db, account: &db::Account, targets: &[PostTarget]) -> Result<usize> {
    let instance = account
        .instance
        .as_deref()
        .ok_or_else(|| AppError::Internal("This Mastodon account has no instance.".into()))?;
    let secret = secrets::load_account_secret(account.platform, &account.remote_id)?;

    let now = db::now_rfc3339();
    let mut updated = 0usize;
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
        })?;
        updated += 1;
    }
    Ok(updated)
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
    fn engagement_reports_which_platforms_it_cannot_read() {
        let (db, _, _) = seeded();
        let stats = compute(&db, &StatsFilter::default()).expect("stats");
        let gaps: Vec<PlatformId> = stats.engagement_gaps.iter().map(|(id, _)| *id).collect();
        assert!(gaps.contains(&PlatformId::X), "X's metrics are paid");
        assert!(!gaps.contains(&PlatformId::Bluesky), "Bluesky's are free");
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
        })
        .expect("metrics");

        let stats = compute(&db, &StatsFilter::default()).expect("stats");
        assert_eq!(stats.engagement.likes, 7);
        assert_eq!(
            stats.engagement.replies, 0,
            "an unreported count adds nothing"
        );
        assert_eq!(stats.engagement.measured, 1);
        assert_eq!(
            stats.engagement.oldest_fetch.as_deref(),
            Some("2026-01-01T00:00:00+00:00")
        );
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
