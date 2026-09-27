//! The scheduler: the part that makes "schedule" mean something.
//!
//! A worker thread wakes every [`TICK`], asks the store for targets whose time
//! has come, and publishes them one at a time. It is deliberately serial — five
//! platforms with five different rate limits are not worth parallelising, and a
//! single worker means the claim/settle cycle has exactly one writer.
//!
//! THE HONEST CONSTRAINT: a desktop app only runs when it is running. Windbag can
//! launch at login and sit in the background, but a machine that is asleep at
//! 09:00 does not post at 09:00. [`catch_up`] is what makes that visible instead
//! of surprising — see [`MissedPolicy`].

use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::Duration;

use chrono::Utc;
use tauri::{AppHandle, Emitter};

use crate::db::{self, Db, DueTarget, POST_MISSED, POST_SCHEDULED};
use crate::error::{AppError, Result};
use crate::media;
use crate::platforms::{self, MediaSpec, PublishRequest, Published};

/// How often the worker looks for work. Twenty seconds is well inside the
/// smallest scheduling granularity the UI offers (one minute) while costing a
/// single indexed query when there is nothing to do.
const TICK: Duration = Duration::from_secs(20);

/// After this many tries a target stops on its own. Five attempts across the
/// backoff ladder below spans roughly ninety minutes — long enough to ride out
/// a rate limit or an outage, short enough that a genuinely broken destination
/// does not retry forever.
const MAX_ATTEMPTS: i64 = 5;

/// At most this many destinations per pass, so one crowded minute cannot hold
/// the worker in a single tick for minutes on end.
const BATCH: usize = 8;

pub const EVENT_QUEUE_CHANGED: &str = "windbag://queue-changed";
pub const EVENT_ACCOUNTS_CHANGED: &str = "windbag://accounts-changed";
pub const EVENT_PUBLISHING: &str = "windbag://publishing";

/// What to do with a post whose time passed while Windbag was not running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissedPolicy {
    /// Post it anyway, late. Right for an evergreen link, wrong for anything
    /// time-bound.
    PostLate,
    /// Mark it `missed` and leave it for the user to re-time. The default,
    /// because a post arriving eleven hours late is usually worse than one that
    /// visibly did not go out.
    Skip,
}

impl MissedPolicy {
    pub fn parse(value: Option<&str>) -> Self {
        match value {
            Some("post_late") => Self::PostLate,
            _ => Self::Skip,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::PostLate => "post_late",
            Self::Skip => "skip",
        }
    }
}

pub const META_MISSED_POLICY: &str = "missed_policy";
pub const META_GRACE_MINUTES: &str = "grace_minutes";
/// A post is only "missed" once it is this far past due. Inside the window it
/// simply goes out — closing the laptop for ten minutes should not cost a post.
const DEFAULT_GRACE_MINUTES: i64 = 15;
/// The smallest window the policy honours. [`catch_up`] runs before the due
/// query in every pass, so a zero window would mark a post missed in the very
/// pass meant to publish it — "Post now" included, since it sets the post to
/// the current instant. One minute is the UI's scheduling granularity and three
/// ticks of the worker.
pub const MIN_GRACE_MINUTES: i64 = 1;

/// Handle the rest of the app uses to nudge the worker. Dropping it stops the
/// thread at its next tick.
pub struct Scheduler {
    wake: Sender<()>,
}

impl Scheduler {
    /// Starts the worker and runs the launch catch-up pass.
    pub fn start(app: AppHandle, database: Arc<Db>) -> Self {
        let (wake, wakeups) = channel();

        if let Err(err) = catch_up(&database) {
            log::error!("catch-up pass failed: {err}");
        }

        let worker_db = Arc::clone(&database);
        std::thread::Builder::new()
            .name("windbag-scheduler".into())
            .spawn(move || run(&app, &worker_db, &wakeups))
            .map_err(|e| log::error!("could not start the scheduler thread: {e}"))
            .ok();

        Self { wake }
    }

    /// Runs a pass now instead of waiting for the next tick — used after "Post
    /// now" and after a manual retry, where waiting up to twenty seconds for
    /// something the user just asked for reads as the app ignoring them.
    pub fn nudge(&self) {
        // A full channel means a wake-up is already pending, which is the same
        // outcome; a closed one means the worker is gone and there is nothing to
        // wake. Neither is worth surfacing.
        let _ = self.wake.send(());
    }
}

fn run(app: &AppHandle, database: &Arc<Db>, wakeups: &Receiver<()>) {
    loop {
        match pass(app, database) {
            Ok(0) => {}
            Ok(count) => log::info!("scheduler settled {count} destination(s)"),
            Err(err) => log::error!("scheduler pass failed: {err}"),
        }
        match wakeups.recv_timeout(TICK) {
            Ok(()) | Err(RecvTimeoutError::Timeout) => {}
            // Every sender is gone: the app is shutting down.
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// One sweep. Returns how many destinations were settled, either way.
fn pass(app: &AppHandle, database: &Arc<Db>) -> Result<usize> {
    // Recovery first. Only this serial worker claims targets, and every claim
    // is settled within the pass that made it, so a target still `publishing`
    // when a pass begins was abandoned — by a crash in an earlier run, or by a
    // settle that failed to write after the send.
    match recover_interrupted(database) {
        Ok(0) => {}
        Ok(count) => log::warn!("{count} destination(s) were interrupted mid-send"),
        Err(err) => log::error!("recovering interrupted sends failed: {err}"),
    }
    // Catch-up runs on EVERY pass, not only at launch. A machine that sleeps
    // overnight with Windbag open wakes to hours of overdue posts, and firing
    // them all at once is exactly what the missed-post policy exists to
    // prevent — a policy that only holds at boot is not the policy Settings
    // describes. It is idempotent, costs one indexed query, and returns
    // immediately under `PostLate`.
    match catch_up(database) {
        Ok(0) => {}
        // Nothing else would tell the open UI: a post just marked missed kept
        // showing as upcoming until something unrelated refetched the queue.
        Ok(_) => {
            let _ = app.emit(EVENT_QUEUE_CHANGED, ());
        }
        Err(err) => log::error!("catch-up inside the scheduler pass failed: {err}"),
    }

    let due = database.due_targets(Utc::now(), BATCH)?;
    if due.is_empty() {
        return Ok(0);
    }

    let mut settled = 0usize;
    for item in due {
        // The claim is the concurrency guard: a target already taken by an
        // earlier pass (or by "Post now") returns false and is skipped.
        if !database.claim_target(item.target.id)? {
            continue;
        }
        let target_id = item.target.id;
        let post_id = item.post.id;
        let attempts = item.target.attempts + 1;

        let _ = app.emit(EVENT_PUBLISHING, target_id);
        let outcome = publish_one(database, &item);
        settle(database, target_id, post_id, attempts, outcome)?;
        settled += 1;
        let _ = app.emit(EVENT_QUEUE_CHANGED, post_id);
    }
    Ok(settled)
}

/// Everything between a claimed target and a result: fetch credentials, refresh
/// them if they are about to expire, re-validate the body, read the attachments,
/// and hand it all to the adapter.
fn publish_one(database: &Arc<Db>, item: &DueTarget) -> Result<Published> {
    let account = &item.account;
    let adapter = platforms::adapter(account.platform);
    let secret = platforms::live_secret(database, account)?;

    let stored_media = database.list_media(item.post.id)?;
    let media = stored_media
        .iter()
        .map(media::load)
        .collect::<Result<Vec<_>>>()?;

    // Validated again here, not only in the composer: the post may have been
    // edited after it was scheduled, and a limit that was fine then may not be
    // now (a Mastodon instance can lower its own).
    // Judged on the bytes just read, not the size recorded at save: the file
    // on disk is what goes out, and it may have been replaced since.
    let specs: Vec<MediaSpec> = media
        .iter()
        .map(|item| MediaSpec {
            mime: item.mime.clone(),
            bytes: item.bytes.len() as u64,
        })
        .collect();
    let limit = effective_char_limit(account, adapter);
    platforms::validate(
        account.platform,
        &item.post.body,
        item.post.title.as_deref(),
        item.post.link.as_deref(),
        &specs,
        &item.target.options,
        limit,
    )?;

    adapter.publish(&PublishRequest {
        target_id: item.target.id,
        account,
        secret: &secret,
        body: &item.post.body,
        title: item.post.title.as_deref(),
        link: item.post.link.as_deref(),
        media: &media,
        options: &item.target.options,
    })
}

/// What this account actually accepts: its own recorded limit where the remote
/// published one, the platform default otherwise.
pub fn effective_char_limit(account: &db::Account, adapter: &dyn platforms::Platform) -> usize {
    account
        .char_limit
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| *value > 0)
        .unwrap_or_else(|| adapter.info().limits.max_chars)
}

/// Records an outcome and decides what happens next. This is the whole retry
/// policy, kept apart from the network so it can be tested against a store alone.
pub fn settle(
    database: &Arc<Db>,
    target_id: i64,
    post_id: i64,
    attempts: i64,
    outcome: Result<Published>,
) -> Result<()> {
    match outcome {
        Ok(published) => {
            database.finish_target_ok(
                target_id,
                &published.remote_id,
                published.remote_url.as_deref(),
            )?;
            database.log_attempt(target_id, true, published.remote_url.as_deref())?;
        }
        Err(err) => {
            let message = err.to_string();
            // Bad credentials flag the account so the UI can say "reconnect"
            // instead of showing the same 401 on every future post.
            if let AppError::Unauthorized(_) = err
                && let Some(target) = database
                    .list_targets(post_id)?
                    .into_iter()
                    .find(|candidate| candidate.id == target_id)
            {
                database.set_account_status(target.account_id, db::ACCOUNT_NEEDS_REAUTH)?;
            }

            let retry_at = if err.is_retryable() && attempts < MAX_ATTEMPTS {
                Some(Utc::now() + backoff(attempts))
            } else {
                None
            };
            database.finish_target_err(target_id, &message, retry_at)?;
            database.log_attempt(target_id, false, Some(&message))?;
        }
    }
    database.reconcile_post_status(post_id)?;
    Ok(())
}

/// Backoff after `attempts` failures: 1, 5, 15 then 60 minutes. Long steps on
/// purpose — every retryable failure here is a rate limit or an outage, and both
/// are measured in minutes, not seconds.
pub fn backoff(attempts: i64) -> chrono::Duration {
    let minutes = match attempts {
        0 | 1 => 1,
        2 => 5,
        3 => 15,
        _ => 60,
    };
    chrono::Duration::minutes(minutes)
}

/// The launch pass over posts whose time came while the app was closed.
///
/// Inside the grace window nothing happens and the normal pass picks them up.
/// Past it, [`MissedPolicy`] decides: `PostLate` also leaves them for the normal
/// pass, `Skip` marks them `missed` so they show up as something that did not go
/// out rather than quietly arriving hours late. That includes a retry whose
/// backoff ended while the app was closed (see [`Db::overdue_posts`]); its
/// unpublished destinations are left for the user to re-time. Returns how many
/// posts it marked.
pub fn catch_up(database: &Arc<Db>) -> Result<usize> {
    let policy = MissedPolicy::parse(database.get_meta(META_MISSED_POLICY)?.as_deref());
    if policy == MissedPolicy::PostLate {
        return Ok(0);
    }
    let grace = grace_minutes(database)?;
    let cutoff = Utc::now() - chrono::Duration::minutes(grace);
    let overdue = database.overdue_posts(cutoff)?;
    for post in &overdue {
        database.set_post_status(post.id, POST_MISSED)?;
        log::warn!(
            "post {} was due at {} and is marked missed — Windbag was not running",
            post.id,
            post.scheduled_at.as_deref().unwrap_or("?")
        );
    }
    Ok(overdue.len())
}

/// The missed-post window from Settings, never below [`MIN_GRACE_MINUTES`].
pub fn grace_minutes(database: &Db) -> Result<i64> {
    Ok(database
        .get_meta(META_GRACE_MINUTES)?
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value >= 0)
        .unwrap_or(DEFAULT_GRACE_MINUTES)
        .max(MIN_GRACE_MINUTES))
}

/// Refuses a time further in the past than the grace window. Such a post is
/// marked missed by the very next pass (or sent hours late under `PostLate`),
/// so accepting it and answering "Scheduled" would be a lie — almost always
/// a wrong date or a wrong zone rather than an intent. Inside the window it
/// is simply due, which is what "Post now" relies on.
pub fn refuse_past(database: &Db, at: chrono::DateTime<Utc>) -> Result<()> {
    let grace = grace_minutes(database)?;
    if at < Utc::now() - chrono::Duration::minutes(grace) {
        return Err(AppError::InvalidInput(format!(
            "That time ({}) has already passed. Pick a time in the future.",
            at.to_rfc3339()
        )));
    }
    Ok(())
}

/// Fails destinations that were claimed and never settled — a crash, a
/// force-quit, or a failed write after the send. Runs at the top of every pass
/// (see [`pass`] for why nothing live can be caught by it). Each is
/// failed with a message that says to check the platform first, and its post's
/// status is recomputed. Returns how many destinations it failed.
pub fn recover_interrupted(database: &Arc<Db>) -> Result<usize> {
    let mut posts = database.fail_interrupted_targets(
        "Interrupted mid-send: no answer from the platform was recorded. \
         Check whether it went out before retrying.",
    )?;
    let failed = posts.len();
    posts.sort_unstable();
    posts.dedup();
    for post_id in posts {
        database.reconcile_post_status(post_id)?;
    }
    Ok(failed)
}

/// Puts a missed or failed post back in the queue at a new time. Targets that
/// already published keep their permalink; everything else is cleared of its
/// error and its backoff.
///
/// A published post is refused, as the save path refuses to edit one: with
/// nothing left to send it would sit `scheduled` and later read as missed.
pub fn requeue(database: &Arc<Db>, post_id: i64, scheduled_at: &str) -> Result<()> {
    let post = database.get_post(post_id)?;
    if post.status == db::POST_PUBLISHED {
        return Err(AppError::Conflict(
            "This post has already gone out and cannot be rescheduled.".into(),
        ));
    }
    database.update_post(
        post_id,
        &post.body,
        post.title.as_deref(),
        post.link.as_deref(),
        Some(scheduled_at),
        POST_SCHEDULED,
    )?;
    for target in database.list_targets(post_id)? {
        if target.status != db::TARGET_PUBLISHED {
            database.requeue_target(target.id)?;
        }
    }
    Ok(())
}

/// Clears one destination's error and backoff so the next pass tries it
/// again. Returns its post.
///
/// Refused on a post with no time rather than giving it "now": a draft is
/// explicitly something that does not go out yet, and a retry button that
/// quietly publishes one is the surprising reading. "Post now" is one click
/// away for the other intent.
pub fn retry(database: &Db, target_id: i64) -> Result<i64> {
    let post_id = database
        .list_posts()?
        .into_iter()
        .find(|detail| detail.targets.iter().any(|t| t.id == target_id))
        .map(|detail| detail.post.id)
        .ok_or_else(|| AppError::NotFound(format!("No destination with id {target_id}.")))?;
    if database.get_post(post_id)?.scheduled_at.is_none() {
        return Err(AppError::InvalidInput(
            "This post has no time, so a retry would never send it. Give it a time, or use \
             Post now."
                .into(),
        ));
    }
    database.requeue_target(target_id)?;
    database.reconcile_post_status(post_id)?;
    Ok(post_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{
        POST_FAILED, POST_PUBLISHED, POST_PUBLISHING, TARGET_FAILED, TARGET_PENDING,
        TARGET_PUBLISHED,
    };
    use crate::platforms::{AccountSecret, Connected, PlatformId};

    fn store() -> (Arc<Db>, i64, i64, i64) {
        let database = Arc::new(Db::open_in_memory().expect("store"));
        let account = database
            .upsert_account(
                PlatformId::Bluesky,
                &Connected {
                    remote_id: "did:1".into(),
                    handle: "me".into(),
                    display_name: None,
                    avatar_url: None,
                    instance: None,
                    scopes: None,
                    char_limit: None,
                    secret: AccountSecret::default(),
                },
            )
            .expect("account");
        let post = database
            .create_post(
                "hello",
                None,
                None,
                Some(&db::now_rfc3339()),
                POST_SCHEDULED,
            )
            .expect("post");
        database
            .set_targets(post, &[(account, serde_json::json!({}))])
            .expect("targets");
        let target = database.list_targets(post).expect("targets")[0].id;
        (database, account, post, target)
    }

    #[test]
    fn a_success_publishes_the_target_and_the_post() {
        let (database, _, post, target) = store();
        settle(
            &database,
            target,
            post,
            1,
            Ok(Published {
                remote_id: "at://1".into(),
                remote_url: Some("https://example.test/1".into()),
            }),
        )
        .expect("settle");

        let stored = &database.list_targets(post).expect("targets")[0];
        assert_eq!(stored.status, TARGET_PUBLISHED);
        assert_eq!(stored.remote_url.as_deref(), Some("https://example.test/1"));
        assert_eq!(
            database.get_post(post).expect("post").status,
            POST_PUBLISHED
        );
    }

    #[test]
    fn a_retryable_failure_parks_the_target_instead_of_failing_it() {
        let (database, _, post, target) = store();
        settle(
            &database,
            target,
            post,
            1,
            Err(AppError::Platform("429".into())),
        )
        .expect("settle");

        let stored = &database.list_targets(post).expect("targets")[0];
        assert_eq!(
            stored.status, TARGET_PENDING,
            "a rate limit is not a dead end"
        );
        assert!(
            stored.next_attempt_at.is_some(),
            "it must be scheduled to try again"
        );
        assert!(
            database
                .due_targets(Utc::now(), 10)
                .expect("due")
                .is_empty()
        );
    }

    #[test]
    fn a_terminal_failure_stops_immediately_however_few_attempts_it_had() {
        let (database, _, post, target) = store();
        settle(
            &database,
            target,
            post,
            1,
            Err(AppError::InvalidInput("too long".into())),
        )
        .expect("settle");

        let stored = &database.list_targets(post).expect("targets")[0];
        assert_eq!(
            stored.status, TARGET_FAILED,
            "retrying a too-long post never helps"
        );
        assert!(stored.next_attempt_at.is_none());
        assert_eq!(database.get_post(post).expect("post").status, POST_FAILED);
    }

    #[test]
    fn retries_stop_at_the_attempt_ceiling() {
        let (database, _, post, target) = store();
        settle(
            &database,
            target,
            post,
            MAX_ATTEMPTS,
            Err(AppError::Platform("still 429".into())),
        )
        .expect("settle");

        let stored = &database.list_targets(post).expect("targets")[0];
        assert_eq!(
            stored.status, TARGET_FAILED,
            "a retryable error still has to give up"
        );
    }

    #[test]
    fn bad_credentials_flag_the_account_for_reconnection() {
        let (database, account, post, target) = store();
        settle(
            &database,
            target,
            post,
            1,
            Err(AppError::Unauthorized("token revoked".into())),
        )
        .expect("settle");

        assert_eq!(
            database.get_account(account).expect("account").status,
            db::ACCOUNT_NEEDS_REAUTH
        );
    }

    #[test]
    fn the_attempt_log_keeps_the_failure_that_preceded_a_success() {
        let (database, _, post, target) = store();
        settle(
            &database,
            target,
            post,
            1,
            Err(AppError::Platform("boom".into())),
        )
        .expect("first");
        database.requeue_target(target).expect("requeue");
        settle(
            &database,
            target,
            post,
            2,
            Ok(Published {
                remote_id: "1".into(),
                remote_url: None,
            }),
        )
        .expect("second");

        let log = database.list_attempts(post).expect("attempts");
        assert_eq!(log.len(), 2);
        assert!(
            log.iter()
                .any(|entry| !entry.ok && entry.detail.as_deref().unwrap_or("").contains("boom"))
        );
    }

    #[test]
    fn backoff_grows_and_then_flattens() {
        assert_eq!(backoff(1), chrono::Duration::minutes(1));
        assert_eq!(backoff(2), chrono::Duration::minutes(5));
        assert_eq!(backoff(3), chrono::Duration::minutes(15));
        assert_eq!(backoff(4), chrono::Duration::minutes(60));
        assert_eq!(backoff(9), chrono::Duration::minutes(60));
    }

    #[test]
    fn a_time_past_the_grace_window_is_refused() {
        let database = Db::open_in_memory().expect("store");
        let err = refuse_past(&database, Utc::now() - chrono::Duration::hours(2)).unwrap_err();
        assert!(err.to_string().contains("already passed"), "{err}");
        // Inside the window it would simply go out on the next pass.
        assert!(refuse_past(&database, Utc::now() - chrono::Duration::minutes(3)).is_ok());
        assert!(refuse_past(&database, Utc::now() + chrono::Duration::hours(2)).is_ok());
    }

    #[test]
    fn the_past_time_check_uses_the_configured_grace() {
        let database = Db::open_in_memory().expect("store");
        database.set_meta(META_GRACE_MINUTES, "180").expect("grace");
        assert!(refuse_past(&database, Utc::now() - chrono::Duration::hours(2)).is_ok());
    }

    #[test]
    fn catch_up_marks_a_long_overdue_post_missed() {
        let database = Arc::new(Db::open_in_memory().expect("store"));
        let long_ago = (Utc::now() - chrono::Duration::hours(6)).to_rfc3339();
        let post = database
            .create_post("late", None, None, Some(&long_ago), POST_SCHEDULED)
            .expect("post");

        assert_eq!(catch_up(&database).expect("catch up"), 1);
        assert_eq!(database.get_post(post).expect("post").status, POST_MISSED);
    }

    #[test]
    fn catch_up_never_touches_a_target_already_in_flight() {
        // Running catch-up on every pass must not reach into a post the
        // scheduler is mid-way through: only `scheduled` posts are candidates,
        // and a claimed one has moved to `publishing`.
        let (database, _, post, target) = store();
        assert!(database.claim_target(target).expect("claim"));

        assert_eq!(catch_up(&database).expect("catch up"), 0);
        assert_eq!(
            database.get_post(post).expect("post").status,
            POST_PUBLISHING
        );
    }

    /// A post whose one destination is waiting out a backoff that ends at
    /// `retry_at`.
    fn backing_off(retry_at: chrono::DateTime<Utc>) -> (Arc<Db>, i64) {
        let (database, _, post, target) = store();
        database
            .finish_target_err(target, "429", Some(retry_at))
            .expect("park");
        assert_eq!(
            database.reconcile_post_status(post).expect("status"),
            POST_PUBLISHING
        );
        (database, post)
    }

    #[test]
    fn catch_up_marks_a_retry_the_app_was_not_running_for_missed() {
        // Decision H-2: the policy covers a retry that fell due while Windbag
        // was closed, not only a first send — otherwise a rate-limited post
        // goes out hours late despite Skip.
        let (database, post) = backing_off(Utc::now() - chrono::Duration::hours(2));

        assert_eq!(catch_up(&database).expect("catch up"), 1);
        assert_eq!(database.get_post(post).expect("post").status, POST_MISSED);
        assert_eq!(
            database.list_targets(post).expect("targets")[0].status,
            TARGET_PENDING,
            "left for the user to re-time"
        );
        assert!(
            database
                .due_targets(Utc::now(), 10)
                .expect("due")
                .is_empty()
        );
    }

    #[test]
    fn catch_up_leaves_a_retry_still_waiting_its_backoff_alone() {
        let (database, post) = backing_off(Utc::now() + chrono::Duration::minutes(5));
        assert_eq!(catch_up(&database).expect("catch up"), 0);
        assert_eq!(
            database.get_post(post).expect("post").status,
            POST_PUBLISHING
        );
    }

    #[test]
    fn catch_up_leaves_a_post_with_a_destination_mid_send_alone() {
        let (database, first, post, parked) = store();
        let second = database
            .upsert_account(
                PlatformId::Mastodon,
                &Connected {
                    remote_id: "id:2".into(),
                    handle: "other".into(),
                    display_name: None,
                    avatar_url: None,
                    instance: None,
                    scopes: None,
                    char_limit: None,
                    secret: AccountSecret::default(),
                },
            )
            .expect("account");
        database
            .set_targets(
                post,
                &[
                    (first, serde_json::json!({})),
                    (second, serde_json::json!({})),
                ],
            )
            .expect("targets");
        database
            .finish_target_err(parked, "429", Some(Utc::now() - chrono::Duration::hours(2)))
            .expect("park");
        let sending = database
            .list_targets(post)
            .expect("targets")
            .into_iter()
            .find(|t| t.account_id == second)
            .expect("second")
            .id;
        assert!(database.claim_target(sending).expect("claim"));

        assert_eq!(catch_up(&database).expect("catch up"), 0);
        assert_eq!(
            database.get_post(post).expect("post").status,
            POST_PUBLISHING
        );
    }

    #[test]
    fn catch_up_leaves_a_post_inside_the_grace_window_alone() {
        let database = Arc::new(Db::open_in_memory().expect("store"));
        let just_now = (Utc::now() - chrono::Duration::minutes(3)).to_rfc3339();
        let post = database
            .create_post("barely late", None, None, Some(&just_now), POST_SCHEDULED)
            .expect("post");

        assert_eq!(catch_up(&database).expect("catch up"), 0);
        assert_eq!(
            database.get_post(post).expect("post").status,
            POST_SCHEDULED,
            "closing the laptop for three minutes must not cost a post"
        );
    }

    #[test]
    fn a_send_interrupted_by_a_crash_fails_instead_of_sticking_or_reposting() {
        // A target claimed and never settled: the process died mid-send. The
        // remote may or may not have the post, so it must neither stay stuck in
        // `publishing` nor go back to `pending` and risk a second copy.
        let (database, _, post, target) = store();
        assert!(database.claim_target(target).expect("claim"));

        assert_eq!(recover_interrupted(&database).expect("recover"), 1);

        let targets = database.list_targets(post).expect("targets");
        assert_eq!(targets[0].status, TARGET_FAILED);
        assert!(
            targets[0]
                .error
                .as_deref()
                .is_some_and(|e| e.contains("Check whether it went out"))
        );
        assert_eq!(database.get_post(post).expect("post").status, POST_FAILED);
        let attempts = database.list_attempts(post).expect("attempts");
        assert_eq!(attempts.len(), 1);
        assert!(!attempts[0].ok);

        assert_eq!(
            recover_interrupted(&database).expect("recover again"),
            0,
            "a second launch finds nothing left to recover"
        );
    }

    #[test]
    fn a_zero_grace_window_does_not_mark_a_post_that_just_came_due_missed() {
        // catch_up runs BEFORE the due query in every pass, so a zero window
        // would mark a post missed in the same pass that should publish it —
        // including one "Post now" just set to the current instant.
        let database = Arc::new(Db::open_in_memory().expect("store"));
        database.set_meta(META_GRACE_MINUTES, "0").expect("grace");
        let just_due = (Utc::now() - chrono::Duration::seconds(5)).to_rfc3339();
        let post = database
            .create_post("now", None, None, Some(&just_due), POST_SCHEDULED)
            .expect("post");

        assert_eq!(catch_up(&database).expect("catch up"), 0);
        assert_eq!(
            database.get_post(post).expect("post").status,
            POST_SCHEDULED
        );
    }

    #[test]
    fn the_post_late_policy_publishes_instead_of_skipping() {
        let database = Arc::new(Db::open_in_memory().expect("store"));
        database
            .set_meta(META_MISSED_POLICY, MissedPolicy::PostLate.as_str())
            .expect("policy");
        let long_ago = (Utc::now() - chrono::Duration::hours(6)).to_rfc3339();
        let post = database
            .create_post("late", None, None, Some(&long_ago), POST_SCHEDULED)
            .expect("post");

        assert_eq!(catch_up(&database).expect("catch up"), 0);
        assert_eq!(
            database.get_post(post).expect("post").status,
            POST_SCHEDULED
        );
    }

    #[test]
    fn requeuing_clears_the_error_but_keeps_what_already_published() {
        let database = Arc::new(Db::open_in_memory().expect("store"));
        let a = database
            .upsert_account(
                PlatformId::Bluesky,
                &Connected {
                    remote_id: "did:a".into(),
                    handle: "a".into(),
                    display_name: None,
                    avatar_url: None,
                    instance: None,
                    scopes: None,
                    char_limit: None,
                    secret: AccountSecret::default(),
                },
            )
            .expect("a");
        let b = database
            .upsert_account(
                PlatformId::Mastodon,
                &Connected {
                    remote_id: "id:b".into(),
                    handle: "b".into(),
                    display_name: None,
                    avatar_url: None,
                    instance: None,
                    scopes: None,
                    char_limit: None,
                    secret: AccountSecret::default(),
                },
            )
            .expect("b");
        let an_hour_ago = (Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        let post = database
            .create_post("x", None, None, Some(&an_hour_ago), POST_SCHEDULED)
            .expect("post");
        database
            .set_targets(
                post,
                &[(a, serde_json::json!({})), (b, serde_json::json!({}))],
            )
            .expect("targets");
        let targets = database.list_targets(post).expect("targets");
        settle(
            &database,
            targets[0].id,
            post,
            1,
            Ok(Published {
                remote_id: "1".into(),
                remote_url: Some("https://x.test/1".into()),
            }),
        )
        .expect("ok");
        settle(
            &database,
            targets[1].id,
            post,
            5,
            Err(AppError::Platform("boom".into())),
        )
        .expect("fail");

        let later = (Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
        requeue(&database, post, &later).expect("requeue");

        let after = database.list_targets(post).expect("targets");
        let published = after.iter().find(|t| t.id == targets[0].id).expect("first");
        let retried = after
            .iter()
            .find(|t| t.id == targets[1].id)
            .expect("second");
        assert_eq!(
            published.status, TARGET_PUBLISHED,
            "it must not be posted a second time"
        );
        assert_eq!(published.remote_url.as_deref(), Some("https://x.test/1"));
        assert_eq!(retried.status, TARGET_PENDING);
        assert!(retried.error.is_none());
        assert_eq!(
            database.get_post(post).expect("post").status,
            POST_SCHEDULED
        );
    }

    #[test]
    fn a_published_post_cannot_be_requeued() {
        // Dragging a published post on the calendar used to set it back to
        // `scheduled` with nothing left to send; it later showed as missed.
        let (database, _, post, target) = store();
        settle(
            &database,
            target,
            post,
            1,
            Ok(Published {
                remote_id: "at://1".into(),
                remote_url: None,
            }),
        )
        .expect("published");

        let later = (Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
        assert!(matches!(
            requeue(&database, post, &later),
            Err(AppError::Conflict(_))
        ));
        assert_eq!(
            database.get_post(post).expect("post").status,
            POST_PUBLISHED
        );
    }

    #[test]
    fn retrying_a_destination_of_a_post_with_no_time_is_refused() {
        // Nothing sends a post without a time, so the retry used to leave it
        // `publishing` with nothing ever due — only deleting it got out.
        let (database, _, post, target) = store();
        settle(
            &database,
            target,
            post,
            1,
            Err(AppError::InvalidInput("too long".into())),
        )
        .expect("fail");
        let current = database.get_post(post).expect("post");
        database
            .update_post(post, &current.body, None, None, None, db::POST_DRAFT)
            .expect("unscheduled");

        let err = retry(&database, target).unwrap_err();
        assert!(matches!(err, AppError::InvalidInput(_)), "{err}");
        assert_eq!(
            database.list_targets(post).expect("targets")[0].status,
            TARGET_FAILED
        );
        assert_eq!(
            database.get_post(post).expect("post").status,
            db::POST_DRAFT
        );
    }

    #[test]
    fn retrying_a_failed_destination_puts_its_post_back_in_flight() {
        let (database, _, post, target) = store();
        settle(
            &database,
            target,
            post,
            1,
            Err(AppError::InvalidInput("too long".into())),
        )
        .expect("fail");

        assert_eq!(retry(&database, target).expect("retry"), post);
        assert_eq!(
            database.list_targets(post).expect("targets")[0].status,
            TARGET_PENDING
        );
        assert_eq!(
            database.get_post(post).expect("post").status,
            POST_PUBLISHING
        );
    }

    #[test]
    fn a_recorded_account_limit_beats_the_platform_default() {
        let adapter = platforms::adapter(PlatformId::Mastodon);
        let mut account = db::Account {
            id: 1,
            platform: PlatformId::Mastodon,
            remote_id: "1".into(),
            handle: "a".into(),
            display_name: None,
            avatar_url: None,
            instance: None,
            scopes: None,
            char_limit: Some(11_000),
            token_expires_at: None,
            status: db::ACCOUNT_OK.into(),
            created_at: db::now_rfc3339(),
        };
        assert_eq!(effective_char_limit(&account, adapter), 11_000);

        account.char_limit = None;
        assert_eq!(effective_char_limit(&account, adapter), 500);
        account.char_limit = Some(0);
        assert_eq!(
            effective_char_limit(&account, adapter),
            500,
            "0 is not a limit"
        );
    }
}
