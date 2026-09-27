//! The store: accounts, posts, per-destination targets, media and the attempt log.
//!
//! Plain SQLite, one file in the app data dir. No secret ever lands here — tokens
//! and app credentials live in the OS credential store (see [`crate::secrets`]);
//! this file holds only what a stolen copy of it should be allowed to reveal.
//!
//! TIME IS STORED IN UTC, ALWAYS, as RFC 3339. The renderer schedules against
//! local wall time and converts on the way in. A post set for 09:00 that is
//! stored as a naive local string fires an hour off the next time the clocks move.

use std::path::PathBuf;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::error::{AppError, Result, internal};
use crate::platforms::PlatformId;

const SCHEMA_VERSION: i64 = 3;

pub struct Db {
    conn: Mutex<Connection>,
}

// ─── Row types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: i64,
    pub platform: PlatformId,
    pub remote_id: String,
    pub handle: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub instance: Option<String>,
    pub scopes: Option<String>,
    /// What this specific account accepts, when the remote publishes its own
    /// limit. `None` falls back to the platform default.
    pub char_limit: Option<i64>,
    pub token_expires_at: Option<String>,
    /// `ok` | `needs_reauth`
    pub status: String,
    pub created_at: String,
}

pub const ACCOUNT_OK: &str = "ok";
pub const ACCOUNT_NEEDS_REAUTH: &str = "needs_reauth";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Post {
    pub id: i64,
    pub body: String,
    pub title: Option<String>,
    pub link: Option<String>,
    /// RFC 3339 UTC. `None` on a draft.
    pub scheduled_at: Option<String>,
    /// `draft` | `scheduled` | `publishing` | `published` | `partial` | `failed` | `missed`
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

pub const POST_DRAFT: &str = "draft";
pub const POST_SCHEDULED: &str = "scheduled";
pub const POST_PUBLISHING: &str = "publishing";
pub const POST_PUBLISHED: &str = "published";
pub const POST_PARTIAL: &str = "partial";
pub const POST_FAILED: &str = "failed";
pub const POST_MISSED: &str = "missed";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PostTarget {
    pub id: i64,
    pub post_id: i64,
    pub account_id: i64,
    pub options: serde_json::Value,
    /// `pending` | `publishing` | `published` | `failed`
    pub status: String,
    pub remote_id: Option<String>,
    pub remote_url: Option<String>,
    pub error: Option<String>,
    pub attempts: i64,
    pub next_attempt_at: Option<String>,
    pub published_at: Option<String>,
}

pub const TARGET_PENDING: &str = "pending";
pub const TARGET_PUBLISHING: &str = "publishing";
pub const TARGET_PUBLISHED: &str = "published";
pub const TARGET_FAILED: &str = "failed";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Media {
    pub id: i64,
    pub post_id: i64,
    pub path: String,
    pub mime: String,
    pub bytes: i64,
    pub alt_text: Option<String>,
    pub position: i64,
}

/// A note: the scratch surface, and the context the assistant reads from.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub id: i64,
    pub title: String,
    pub body: String,
    pub pinned: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// What one published destination earned, as of the last refresh. `None` counts
/// are a platform that does not report that dimension, not a zero.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Metrics {
    pub target_id: i64,
    pub fetched_at: String,
    pub likes: Option<i64>,
    pub reposts: Option<i64>,
    pub replies: Option<i64>,
    pub quotes: Option<i64>,
    /// Impressions: how many times the post was seen. Threads and Instagram
    /// call it `views`, X `impression_count`; Bluesky and Mastodon do not
    /// report it at all, which is why it is optional like the rest.
    pub views: Option<i64>,
}

/// A post with everything the renderer draws in one row of the queue.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PostDetail {
    #[serde(flatten)]
    pub post: Post,
    pub targets: Vec<PostTarget>,
    pub media: Vec<Media>,
}

/// One line of the attempt log, kept so a failure that later succeeded still
/// shows what went wrong the first time.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Attempt {
    pub id: i64,
    pub target_id: i64,
    pub at: String,
    pub ok: bool,
    pub detail: Option<String>,
}

/// What the scheduler needs to publish one destination, gathered in a single
/// read so the connection is not held across the network call.
pub struct DueTarget {
    pub target: PostTarget,
    pub post: Post,
    pub account: Account,
}

// ─── Lifecycle ──────────────────────────────────────────────────────────────

pub fn data_dir() -> Result<PathBuf> {
    let dir = dirs::data_dir()
        .ok_or_else(|| AppError::Internal("No OS data directory.".into()))?
        .join("windbag");
    std::fs::create_dir_all(&dir).map_err(|e| internal("Creating the data directory", e))?;
    Ok(dir)
}

impl Db {
    pub fn open_at(path: &std::path::Path) -> Result<Self> {
        let conn = Connection::open(path).map_err(|e| internal("Opening the store", e))?;
        Self::from_connection(conn)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().map_err(|e| internal("Opening the store", e))?;
        Self::from_connection(conn)
    }

    fn from_connection(conn: Connection) -> Result<Self> {
        // WAL so the scheduler thread's writes never block a read the UI waits on.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        let db = Self {
            conn: Mutex::new(conn),
        };
        db.migrate()?;
        Ok(db)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        // A poisoned lock means a previous holder panicked mid-write. The store is
        // still consistent (SQLite transactions are atomic), so recovering is
        // strictly better than taking the whole app down with it.
        self.conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The schema ladder.
    ///
    /// Each step is applied once, in order, and commits together with the
    /// version it produces (see [`apply_step`]) — so an interrupted upgrade
    /// resumes rather than re-running a step that already landed. `CREATE TABLE
    /// IF NOT EXISTS` alone stops being enough the first time a column is added
    /// to a table that already holds a user's scheduled posts, which is why this
    /// exists before that happens.
    fn migrate(&self) -> Result<()> {
        let mut conn = self.lock();

        // `meta` first and unconditionally: it is where the version lives, so it
        // cannot itself be gated on the version.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS meta (
                 key   TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );",
        )?;

        let mut version: i64 = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);

        while version < SCHEMA_VERSION {
            let next = version + 1;
            apply_step(&mut conn, next, step_sql(next))?;
            version = next;
        }
        Ok(())
    }

    // ─── Settings (key/value in `meta`) ─────────────────────────────────────

    pub fn get_meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .lock()
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.lock().execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    // ─── Accounts ───────────────────────────────────────────────────────────

    pub fn list_accounts(&self) -> Result<Vec<Account>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT {ACCOUNT_COLUMNS} FROM accounts ORDER BY platform, handle"
        ))?;
        let rows = stmt.query_map([], map_account)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn get_account(&self, id: i64) -> Result<Account> {
        self.lock()
            .query_row(
                &format!("SELECT {ACCOUNT_COLUMNS} FROM accounts WHERE id = ?1"),
                params![id],
                map_account,
            )
            .map_err(|err| match err {
                rusqlite::Error::QueryReturnedNoRows => {
                    AppError::NotFound(format!("No account with id {id}."))
                }
                other => other.into(),
            })
    }

    /// Inserts, or updates in place when the same remote account reconnects —
    /// which is what a re-auth is, and it must keep the id so scheduled posts
    /// aimed at that account survive.
    pub fn upsert_account(
        &self,
        platform: PlatformId,
        connected: &crate::platforms::Connected,
    ) -> Result<i64> {
        let conn = self.lock();
        let now = now_rfc3339();
        conn.execute(
            "INSERT INTO accounts
               (platform, remote_id, handle, display_name, avatar_url, instance, scopes,
                char_limit, token_expires_at, status, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?11, ?10)
             ON CONFLICT(platform, remote_id) DO UPDATE SET
               handle           = excluded.handle,
               display_name     = excluded.display_name,
               avatar_url       = excluded.avatar_url,
               instance         = excluded.instance,
               scopes           = excluded.scopes,
               char_limit       = excluded.char_limit,
               token_expires_at = excluded.token_expires_at,
               -- A reconnect clears a previous `needs_reauth`: the whole point
               -- of reconnecting is that the credentials work again.
               status           = excluded.status",
            params![
                platform,
                connected.remote_id,
                connected.handle,
                connected.display_name,
                connected.avatar_url,
                connected.instance,
                connected.scopes,
                connected
                    .char_limit
                    .map(|value| i64::try_from(value).unwrap_or(i64::MAX)),
                connected.secret.expires_at,
                now,
                ACCOUNT_OK,
            ],
        )?;
        conn.query_row(
            "SELECT id FROM accounts WHERE platform = ?1 AND remote_id = ?2",
            params![platform, connected.remote_id],
            |row| row.get(0),
        )
        .map_err(Into::into)
    }

    pub fn set_account_status(&self, id: i64, status: &str) -> Result<()> {
        self.lock().execute(
            "UPDATE accounts SET status = ?2 WHERE id = ?1",
            params![id, status],
        )?;
        Ok(())
    }

    pub fn set_account_token_expiry(&self, id: i64, expires_at: Option<&str>) -> Result<()> {
        self.lock().execute(
            "UPDATE accounts SET token_expires_at = ?2 WHERE id = ?1",
            params![id, expires_at],
        )?;
        Ok(())
    }

    /// Removes the row. The caller is responsible for the credential-store entry —
    /// deleting the row first would orphan a secret nothing can name any more.
    pub fn delete_account(&self, id: i64) -> Result<()> {
        let changed = self
            .lock()
            .execute("DELETE FROM accounts WHERE id = ?1", params![id])?;
        if changed == 0 {
            return Err(AppError::NotFound(format!("No account with id {id}.")));
        }
        Ok(())
    }

    // ─── Posts ──────────────────────────────────────────────────────────────

    pub fn create_post(
        &self,
        body: &str,
        title: Option<&str>,
        link: Option<&str>,
        scheduled_at: Option<&str>,
        status: &str,
    ) -> Result<i64> {
        let conn = self.lock();
        let now = now_rfc3339();
        conn.execute(
            "INSERT INTO posts (body, title, link, scheduled_at, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            params![body, title, link, scheduled_at, status, now],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn update_post(
        &self,
        id: i64,
        body: &str,
        title: Option<&str>,
        link: Option<&str>,
        scheduled_at: Option<&str>,
        status: &str,
    ) -> Result<()> {
        let changed = self.lock().execute(
            "UPDATE posts
                SET body = ?2, title = ?3, link = ?4, scheduled_at = ?5, status = ?6,
                    updated_at = ?7
              WHERE id = ?1",
            params![id, body, title, link, scheduled_at, status, now_rfc3339()],
        )?;
        if changed == 0 {
            return Err(AppError::NotFound(format!("No post with id {id}.")));
        }
        Ok(())
    }

    pub fn set_post_status(&self, id: i64, status: &str) -> Result<()> {
        self.lock().execute(
            "UPDATE posts SET status = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, status, now_rfc3339()],
        )?;
        Ok(())
    }

    pub fn delete_post(&self, id: i64) -> Result<()> {
        let changed = self
            .lock()
            .execute("DELETE FROM posts WHERE id = ?1", params![id])?;
        if changed == 0 {
            return Err(AppError::NotFound(format!("No post with id {id}.")));
        }
        Ok(())
    }

    pub fn get_post(&self, id: i64) -> Result<Post> {
        self.lock()
            .query_row(
                &format!("SELECT {POST_COLUMNS} FROM posts WHERE id = ?1"),
                params![id],
                map_post,
            )
            .map_err(|err| match err {
                rusqlite::Error::QueryReturnedNoRows => {
                    AppError::NotFound(format!("No post with id {id}."))
                }
                other => other.into(),
            })
    }

    /// Every post with its targets and media, newest intent first: scheduled posts
    /// ordered by when they fire, everything else by when it was last touched.
    pub fn list_posts(&self) -> Result<Vec<PostDetail>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT {POST_COLUMNS} FROM posts
              ORDER BY COALESCE(scheduled_at, updated_at) DESC, id DESC"
        ))?;
        let posts = stmt
            .query_map([], map_post)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut target_stmt = conn.prepare(&format!(
            "SELECT {TARGET_COLUMNS} FROM post_targets WHERE post_id = ?1 ORDER BY id"
        ))?;
        let mut media_stmt = conn.prepare(
            "SELECT id, post_id, path, mime, bytes, alt_text, position
               FROM media WHERE post_id = ?1 ORDER BY position, id",
        )?;

        posts
            .into_iter()
            .map(|post| {
                let targets = target_stmt
                    .query_map(params![post.id], map_target)?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                let media = media_stmt
                    .query_map(params![post.id], map_media)?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(PostDetail {
                    post,
                    targets,
                    media,
                })
            })
            .collect()
    }

    // ─── Targets ────────────────────────────────────────────────────────────

    /// Replaces a post's destinations wholesale. Targets that already published
    /// are kept whatever the new selection says — un-posting is not a thing, and
    /// dropping the row would lose the permalink.
    pub fn set_targets(&self, post_id: i64, targets: &[(i64, serde_json::Value)]) -> Result<()> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        {
            // Account ids are i64 read back from this same store, never user text,
            // so interpolating them into the NOT IN list cannot carry SQL.
            let keep: Vec<String> = targets
                .iter()
                .map(|(account_id, _)| account_id.to_string())
                .collect();
            tx.execute(
                &format!(
                    "DELETE FROM post_targets
                      WHERE post_id = ?1
                        AND status != '{TARGET_PUBLISHED}'
                        AND account_id NOT IN ({})",
                    if keep.is_empty() {
                        "NULL".into()
                    } else {
                        keep.join(",")
                    }
                ),
                params![post_id],
            )?;
            for (account_id, options) in targets {
                tx.execute(
                    "INSERT INTO post_targets (post_id, account_id, options)
                     VALUES (?1, ?2, ?3)
                     ON CONFLICT(post_id, account_id) DO UPDATE SET options = excluded.options
                     WHERE post_targets.status != ?4",
                    params![post_id, account_id, options.to_string(), TARGET_PUBLISHED],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn list_targets(&self, post_id: i64) -> Result<Vec<PostTarget>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT {TARGET_COLUMNS} FROM post_targets WHERE post_id = ?1 ORDER BY id"
        ))?;
        stmt.query_map(params![post_id], map_target)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Everything the scheduler may attempt right now: a scheduled post whose
    /// time has come, a target still pending, and no backoff still running.
    pub fn due_targets(&self, now: DateTime<Utc>, limit: usize) -> Result<Vec<DueTarget>> {
        let conn = self.lock();
        let now = now.to_rfc3339();
        let mut stmt = conn.prepare(&format!(
            "SELECT {TARGET_COLUMNS_T}, {POST_COLUMNS_P}, {ACCOUNT_COLUMNS_A}
               FROM post_targets t
               JOIN posts p    ON p.id = t.post_id
               JOIN accounts a ON a.id = t.account_id
              WHERE p.status IN ('{POST_SCHEDULED}', '{POST_PUBLISHING}', '{POST_PARTIAL}')
                AND p.scheduled_at IS NOT NULL
                AND p.scheduled_at <= ?1
                AND t.status = '{TARGET_PENDING}'
                AND (t.next_attempt_at IS NULL OR t.next_attempt_at <= ?1)
              ORDER BY p.scheduled_at
              LIMIT ?2"
        ))?;
        stmt.query_map(
            params![now, i64::try_from(limit).unwrap_or(i64::MAX)],
            |row| {
                Ok(DueTarget {
                    target: map_target(row)?,
                    post: map_post_at(row, TARGET_COLUMN_COUNT)?,
                    account: map_account_at(row, TARGET_COLUMN_COUNT + POST_COLUMN_COUNT)?,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
    }

    /// Scheduled posts whose time passed while the app was not running. The
    /// scheduler decides what to do with them; this only finds them.
    pub fn overdue_posts(&self, before: DateTime<Utc>) -> Result<Vec<Post>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT {POST_COLUMNS} FROM posts
              WHERE status = '{POST_SCHEDULED}'
                AND scheduled_at IS NOT NULL
                AND scheduled_at < ?1
              ORDER BY scheduled_at"
        ))?;
        stmt.query_map(params![before.to_rfc3339()], map_post)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Moves a target from `pending` to `publishing` and counts the attempt. The
    /// status guard is the whole point: it is atomic, so two passes racing over
    /// the same target cannot both win and post twice.
    pub fn claim_target(&self, target_id: i64) -> Result<bool> {
        let changed = self.lock().execute(
            "UPDATE post_targets
                SET status = ?2, attempts = attempts + 1
              WHERE id = ?1 AND status = ?3",
            params![target_id, TARGET_PUBLISHING, TARGET_PENDING],
        )?;
        Ok(changed == 1)
    }

    pub fn finish_target_ok(
        &self,
        target_id: i64,
        remote_id: &str,
        remote_url: Option<&str>,
    ) -> Result<()> {
        self.lock().execute(
            "UPDATE post_targets
                SET status = ?2, remote_id = ?3, remote_url = ?4, error = NULL,
                    next_attempt_at = NULL, published_at = ?5
              WHERE id = ?1",
            params![
                target_id,
                TARGET_PUBLISHED,
                remote_id,
                remote_url,
                now_rfc3339()
            ],
        )?;
        Ok(())
    }

    /// `retry_at = None` means give up: the target goes to `failed` and the user
    /// has to do something. Anything else parks it back in `pending`.
    pub fn finish_target_err(
        &self,
        target_id: i64,
        message: &str,
        retry_at: Option<DateTime<Utc>>,
    ) -> Result<()> {
        let status = if retry_at.is_some() {
            TARGET_PENDING
        } else {
            TARGET_FAILED
        };
        self.lock().execute(
            "UPDATE post_targets
                SET status = ?2, error = ?3, next_attempt_at = ?4
              WHERE id = ?1",
            params![
                target_id,
                status,
                message,
                retry_at.map(|at| at.to_rfc3339())
            ],
        )?;
        Ok(())
    }

    /// Fails every target still marked `publishing` — between scheduler passes
    /// the only way one can be is a claim that was never settled — and logs the
    /// attempt. Returns the post of each failed target, one entry per
    /// target, so their status can be recomputed. Failed rather than requeued: the send may have landed, and
    /// only the user can check before a retry risks a second copy.
    pub fn fail_interrupted_targets(&self, message: &str) -> Result<Vec<i64>> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let interrupted = {
            let mut stmt = tx.prepare("SELECT id, post_id FROM post_targets WHERE status = ?1")?;
            stmt.query_map(params![TARGET_PUBLISHING], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let now = now_rfc3339();
        for (target_id, _) in &interrupted {
            tx.execute(
                "UPDATE post_targets SET status = ?2, error = ?3, next_attempt_at = NULL
                  WHERE id = ?1",
                params![target_id, TARGET_FAILED, message],
            )?;
            tx.execute(
                "INSERT INTO attempts (target_id, at, ok, detail) VALUES (?1, ?2, 0, ?3)",
                params![target_id, now, message],
            )?;
        }
        tx.commit()?;
        Ok(interrupted.into_iter().map(|(_, post)| post).collect())
    }

    /// Clears the error and backoff so the scheduler picks the target up again.
    pub fn requeue_target(&self, target_id: i64) -> Result<()> {
        self.lock().execute(
            "UPDATE post_targets
                SET status = ?2, error = NULL, next_attempt_at = NULL, attempts = 0
              WHERE id = ?1 AND status != ?3",
            params![target_id, TARGET_PENDING, TARGET_PUBLISHED],
        )?;
        Ok(())
    }

    pub fn log_attempt(&self, target_id: i64, ok: bool, detail: Option<&str>) -> Result<()> {
        self.lock().execute(
            "INSERT INTO attempts (target_id, at, ok, detail) VALUES (?1, ?2, ?3, ?4)",
            params![target_id, now_rfc3339(), ok, detail],
        )?;
        Ok(())
    }

    pub fn list_attempts(&self, post_id: i64) -> Result<Vec<Attempt>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT a.id, a.target_id, a.at, a.ok, a.detail
               FROM attempts a
               JOIN post_targets t ON t.id = a.target_id
              WHERE t.post_id = ?1
              ORDER BY a.at DESC, a.id DESC
              LIMIT 200",
        )?;
        stmt.query_map(params![post_id], |row| {
            Ok(Attempt {
                id: row.get(0)?,
                target_id: row.get(1)?,
                at: row.get(2)?,
                ok: row.get(3)?,
                detail: row.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
    }

    /// Recomputes a post's status from its targets, which is the only place that
    /// status is decided: every target published is `published`, some published
    /// and the rest failed is `partial`, all failed is `failed`, anything still
    /// pending keeps the post in flight.
    pub fn reconcile_post_status(&self, post_id: i64) -> Result<String> {
        let targets = self.list_targets(post_id)?;
        if targets.is_empty() {
            return Ok(self.get_post(post_id)?.status);
        }
        let published = targets
            .iter()
            .filter(|t| t.status == TARGET_PUBLISHED)
            .count();
        let failed = targets.iter().filter(|t| t.status == TARGET_FAILED).count();
        let status = if published == targets.len() {
            POST_PUBLISHED
        } else if published + failed == targets.len() {
            if published == 0 {
                POST_FAILED
            } else {
                POST_PARTIAL
            }
        } else {
            POST_PUBLISHING
        };
        self.set_post_status(post_id, status)?;
        Ok(status.to_string())
    }

    // ─── Media ──────────────────────────────────────────────────────────────

    pub fn set_media(&self, post_id: i64, items: &[MediaInput]) -> Result<()> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM media WHERE post_id = ?1", params![post_id])?;
        for (position, item) in items.iter().enumerate() {
            tx.execute(
                "INSERT INTO media (post_id, path, mime, bytes, alt_text, position, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    post_id,
                    item.path,
                    item.mime,
                    item.bytes,
                    item.alt_text,
                    i64::try_from(position).unwrap_or(i64::MAX),
                    now_rfc3339()
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn list_media(&self, post_id: i64) -> Result<Vec<Media>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, post_id, path, mime, bytes, alt_text, position
               FROM media WHERE post_id = ?1 ORDER BY position, id",
        )?;
        stmt.query_map(params![post_id], map_media)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    // ─── Notes ──────────────────────────────────────────────────────────────

    /// Pinned first, then most recently touched — a note you keep coming back to
    /// should not sink because you edited something else.
    pub fn list_notes(&self) -> Result<Vec<Note>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, title, body, pinned, created_at, updated_at
               FROM notes ORDER BY pinned DESC, updated_at DESC",
        )?;
        stmt.query_map([], map_note)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn get_note(&self, id: i64) -> Result<Note> {
        self.lock()
            .query_row(
                "SELECT id, title, body, pinned, created_at, updated_at
                   FROM notes WHERE id = ?1",
                params![id],
                map_note,
            )
            .map_err(|err| match err {
                rusqlite::Error::QueryReturnedNoRows => {
                    AppError::NotFound(format!("No note with id {id}."))
                }
                other => other.into(),
            })
    }

    pub fn save_note(&self, id: Option<i64>, title: &str, body: &str, pinned: bool) -> Result<i64> {
        let conn = self.lock();
        let now = now_rfc3339();
        if let Some(id) = id {
            let changed = conn.execute(
                "UPDATE notes SET title = ?2, body = ?3, pinned = ?4, updated_at = ?5
                  WHERE id = ?1",
                params![id, title, body, pinned, now],
            )?;
            if changed == 0 {
                return Err(AppError::NotFound(format!("No note with id {id}.")));
            }
            return Ok(id);
        }
        conn.execute(
            "INSERT INTO notes (title, body, pinned, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?4)",
            params![title, body, pinned, now],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn delete_note(&self, id: i64) -> Result<()> {
        let changed = self
            .lock()
            .execute("DELETE FROM notes WHERE id = ?1", params![id])?;
        if changed == 0 {
            return Err(AppError::NotFound(format!("No note with id {id}.")));
        }
        Ok(())
    }

    // ─── Engagement metrics ─────────────────────────────────────────────────

    pub fn save_metrics(&self, metrics: &Metrics) -> Result<()> {
        self.lock().execute(
            "INSERT INTO metrics (target_id, fetched_at, likes, reposts, replies, quotes, views)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(target_id) DO UPDATE SET
               fetched_at = excluded.fetched_at,
               likes      = excluded.likes,
               reposts    = excluded.reposts,
               replies    = excluded.replies,
               views      = excluded.views,
               quotes     = excluded.quotes",
            params![
                metrics.target_id,
                metrics.fetched_at,
                metrics.likes,
                metrics.reposts,
                metrics.replies,
                metrics.quotes,
                metrics.views
            ],
        )?;
        Ok(())
    }

    pub fn list_metrics(&self) -> Result<Vec<Metrics>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT target_id, fetched_at, likes, reposts, replies, quotes, views FROM metrics",
        )?;
        stmt.query_map([], map_metrics)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Every destination that actually published, with the account it went to —
    /// the input to a metrics refresh and to the stats screen alike.
    pub fn published_targets(&self) -> Result<Vec<(PostTarget, Account)>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT {TARGET_COLUMNS_T}, {ACCOUNT_COLUMNS_A}
               FROM post_targets t
               JOIN accounts a ON a.id = t.account_id
              WHERE t.status = '{TARGET_PUBLISHED}' AND t.remote_id IS NOT NULL
              ORDER BY t.published_at DESC"
        ))?;
        stmt.query_map([], |row| {
            Ok((map_target(row)?, map_account_at(row, TARGET_COLUMN_COUNT)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
    }
}

/// Runs one ladder step and stamps the version it produces, as ONE transaction.
///
/// SQLite DDL is transactional, so a step that dies half-way — a crash, a full
/// disk, a statement that fails — rolls back whole and the next launch runs it
/// again from the top. Committed apart, the step's first statements would
/// survive under the old version and the re-run would hit "duplicate column"
/// on every launch after.
fn apply_step(conn: &mut Connection, version: i64, sql: &str) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute_batch(sql)?;
    tx.execute(
        "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![version.to_string()],
    )?;
    tx.commit()?;
    Ok(())
}

/// The SQL for one ladder step. Steps are append-only: once a version has
/// shipped its statement never changes, because a store that already ran it
/// will never run it again.
fn step_sql(version: i64) -> &'static str {
    match version {
        1 => V1_INITIAL,
        2 => V2_NOTES_AND_METRICS,
        3 => V3_METRIC_VIEWS,
        // Unreachable while `SCHEMA_VERSION` and this match move together, and a
        // no-op rather than a panic if they ever do not: a store one version
        // ahead of the binary (a downgrade) is better left alone than crashed on.
        _ => "",
    }
}

const V1_INITIAL: &str = r"
    CREATE TABLE IF NOT EXISTS accounts (
        id               INTEGER PRIMARY KEY,
        platform         TEXT NOT NULL,
        remote_id        TEXT NOT NULL,
        handle           TEXT NOT NULL,
        display_name     TEXT,
        avatar_url       TEXT,
        instance         TEXT,
        scopes           TEXT,
        char_limit       INTEGER,
        token_expires_at TEXT,
        status           TEXT NOT NULL DEFAULT 'ok',
        created_at       TEXT NOT NULL,
        UNIQUE(platform, remote_id)
    );

    CREATE TABLE IF NOT EXISTS posts (
        id           INTEGER PRIMARY KEY,
        body         TEXT NOT NULL,
        title        TEXT,
        link         TEXT,
        scheduled_at TEXT,
        status       TEXT NOT NULL,
        created_at   TEXT NOT NULL,
        updated_at   TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS posts_due ON posts(status, scheduled_at);

    CREATE TABLE IF NOT EXISTS post_targets (
        id              INTEGER PRIMARY KEY,
        post_id         INTEGER NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
        account_id      INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
        options         TEXT NOT NULL DEFAULT '{}',
        status          TEXT NOT NULL DEFAULT 'pending',
        remote_id       TEXT,
        remote_url      TEXT,
        error           TEXT,
        attempts        INTEGER NOT NULL DEFAULT 0,
        next_attempt_at TEXT,
        published_at    TEXT,
        UNIQUE(post_id, account_id)
    );
    CREATE INDEX IF NOT EXISTS targets_by_post ON post_targets(post_id);

    -- Media columns exist from the first schema even where the adapter cannot
    -- upload yet: retrofitting an attachment table onto a store with live
    -- scheduled posts is the expensive path.
    CREATE TABLE IF NOT EXISTS media (
        id         INTEGER PRIMARY KEY,
        post_id    INTEGER NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
        path       TEXT NOT NULL,
        mime       TEXT NOT NULL,
        bytes      INTEGER NOT NULL,
        alt_text   TEXT,
        position   INTEGER NOT NULL DEFAULT 0,
        created_at TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS media_by_post ON media(post_id, position);

    CREATE TABLE IF NOT EXISTS attempts (
        id        INTEGER PRIMARY KEY,
        target_id INTEGER NOT NULL REFERENCES post_targets(id) ON DELETE CASCADE,
        at        TEXT NOT NULL,
        ok        INTEGER NOT NULL,
        detail    TEXT
    );
    CREATE INDEX IF NOT EXISTS attempts_by_target ON attempts(target_id, at DESC);
";

/// Notes (the scratch surface, and what the assistant reads as context) and
/// engagement metrics (what a published destination earned, when last fetched).
const V2_NOTES_AND_METRICS: &str = r"
    CREATE TABLE IF NOT EXISTS notes (
        id         INTEGER PRIMARY KEY,
        title      TEXT NOT NULL DEFAULT '',
        body       TEXT NOT NULL,
        pinned     INTEGER NOT NULL DEFAULT 0,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS notes_recent ON notes(pinned DESC, updated_at DESC);

    -- ONE row per destination, replaced on each refresh rather than appended.
    -- Nothing polls in the background, so a history would be a handful of rows
    -- at whatever moments someone happened to press Refresh — a shape that
    -- invites being read as a trend when it is not one. The counts are `as of
    -- fetched_at`, and the UI says so.
    CREATE TABLE IF NOT EXISTS metrics (
        target_id  INTEGER PRIMARY KEY REFERENCES post_targets(id) ON DELETE CASCADE,
        fetched_at TEXT NOT NULL,
        likes      INTEGER,
        reposts    INTEGER,
        replies    INTEGER,
        quotes     INTEGER
    );
";

/// Impressions, once the official insights endpoints were wired up. Additive:
/// the existing rows keep their counts and report `NULL` views, which the UI
/// already distinguishes from a zero.
const V3_METRIC_VIEWS: &str = r"
    ALTER TABLE metrics ADD COLUMN views INTEGER;
";

/// One attachment as the renderer hands it over: a path on disk the user picked,
/// resolved to its type and size by [`crate::media`] before it is stored.
pub struct MediaInput {
    pub path: String,
    pub mime: String,
    pub bytes: i64,
    pub alt_text: Option<String>,
}

// ─── Column lists and row mappers ───────────────────────────────────────────
//
// The three-way join in `due_targets` selects target, post and account columns
// side by side, so each mapper comes in a positional variant that reads from a
// given offset. The `_COUNT` constants keep those offsets honest; adding a
// column means updating its list AND its count.

const TARGET_COLUMN_COUNT: usize = 11;
const POST_COLUMN_COUNT: usize = 8;

const TARGET_COLUMNS: &str = "id, post_id, account_id, options, status, remote_id, remote_url, \
                              error, attempts, next_attempt_at, published_at";
const TARGET_COLUMNS_T: &str = "t.id, t.post_id, t.account_id, t.options, t.status, t.remote_id, \
                                t.remote_url, t.error, t.attempts, t.next_attempt_at, \
                                t.published_at";
const POST_COLUMNS: &str = "id, body, title, link, scheduled_at, status, created_at, updated_at";
const POST_COLUMNS_P: &str = "p.id, p.body, p.title, p.link, p.scheduled_at, p.status, \
                              p.created_at, p.updated_at";
const ACCOUNT_COLUMNS: &str = "id, platform, remote_id, handle, display_name, avatar_url, \
                               instance, scopes, char_limit, token_expires_at, status, created_at";
const ACCOUNT_COLUMNS_A: &str = "a.id, a.platform, a.remote_id, a.handle, a.display_name, \
                                 a.avatar_url, a.instance, a.scopes, a.char_limit, \
                                 a.token_expires_at, a.status, a.created_at";

fn map_account(row: &rusqlite::Row<'_>) -> rusqlite::Result<Account> {
    map_account_at(row, 0)
}

fn map_account_at(row: &rusqlite::Row<'_>, base: usize) -> rusqlite::Result<Account> {
    Ok(Account {
        id: row.get(base)?,
        platform: row.get(base + 1)?,
        remote_id: row.get(base + 2)?,
        handle: row.get(base + 3)?,
        display_name: row.get(base + 4)?,
        avatar_url: row.get(base + 5)?,
        instance: row.get(base + 6)?,
        scopes: row.get(base + 7)?,
        char_limit: row.get(base + 8)?,
        token_expires_at: row.get(base + 9)?,
        status: row.get(base + 10)?,
        created_at: row.get(base + 11)?,
    })
}

fn map_post(row: &rusqlite::Row<'_>) -> rusqlite::Result<Post> {
    map_post_at(row, 0)
}

fn map_post_at(row: &rusqlite::Row<'_>, base: usize) -> rusqlite::Result<Post> {
    Ok(Post {
        id: row.get(base)?,
        body: row.get(base + 1)?,
        title: row.get(base + 2)?,
        link: row.get(base + 3)?,
        scheduled_at: row.get(base + 4)?,
        status: row.get(base + 5)?,
        created_at: row.get(base + 6)?,
        updated_at: row.get(base + 7)?,
    })
}

fn map_target(row: &rusqlite::Row<'_>) -> rusqlite::Result<PostTarget> {
    let options: String = row.get(3)?;
    Ok(PostTarget {
        id: row.get(0)?,
        post_id: row.get(1)?,
        account_id: row.get(2)?,
        // A hand-edited store should not take the queue down: an unparseable
        // options blob degrades to "no options", which every adapter tolerates.
        options: serde_json::from_str(&options).unwrap_or(serde_json::Value::Null),
        status: row.get(4)?,
        remote_id: row.get(5)?,
        remote_url: row.get(6)?,
        error: row.get(7)?,
        attempts: row.get(8)?,
        next_attempt_at: row.get(9)?,
        published_at: row.get(10)?,
    })
}

fn map_note(row: &rusqlite::Row<'_>) -> rusqlite::Result<Note> {
    Ok(Note {
        id: row.get(0)?,
        title: row.get(1)?,
        body: row.get(2)?,
        pinned: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

fn map_metrics(row: &rusqlite::Row<'_>) -> rusqlite::Result<Metrics> {
    Ok(Metrics {
        target_id: row.get(0)?,
        fetched_at: row.get(1)?,
        likes: row.get(2)?,
        reposts: row.get(3)?,
        replies: row.get(4)?,
        quotes: row.get(5)?,
        views: row.get(6)?,
    })
}

fn map_media(row: &rusqlite::Row<'_>) -> rusqlite::Result<Media> {
    Ok(Media {
        id: row.get(0)?,
        post_id: row.get(1)?,
        path: row.get(2)?,
        mime: row.get(3)?,
        bytes: row.get(4)?,
        alt_text: row.get(5)?,
        position: row.get(6)?,
    })
}

pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

/// Parses a stored timestamp back into UTC. Stored values are always written by
/// `to_rfc3339`, so a parse failure means the value came from somewhere else.
pub fn parse_rfc3339(value: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| AppError::InvalidInput(format!("`{value}` is not an RFC 3339 timestamp: {e}")))
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

    fn seeded() -> (Db, i64, i64) {
        let db = Db::open_in_memory().expect("in-memory store");
        let account = db
            .upsert_account(PlatformId::Bluesky, &connected("did:1", "me.bsky.social"))
            .expect("account");
        let post = db
            .create_post("hello", None, None, Some(&now_rfc3339()), POST_SCHEDULED)
            .expect("post");
        db.set_targets(post, &[(account, serde_json::json!({}))])
            .expect("targets");
        (db, account, post)
    }

    #[test]
    fn a_fresh_store_lands_on_the_current_schema_version() {
        let db = Db::open_in_memory().expect("store");
        assert_eq!(
            db.get_meta("schema_version").expect("version"),
            Some(SCHEMA_VERSION.to_string())
        );
    }

    #[test]
    fn a_v1_store_upgrades_without_losing_anything() {
        // A store as it looked before notes and metrics existed, complete with a
        // scheduled post — the case the ladder exists to protect.
        let conn = Connection::open_in_memory().expect("conn");
        conn.execute_batch("CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);")
            .expect("meta");
        conn.execute_batch(V1_INITIAL).expect("v1");
        conn.execute(
            "INSERT INTO meta (key, value) VALUES ('schema_version', '1')",
            [],
        )
        .expect("stamp");
        conn.execute(
            "INSERT INTO posts (body, title, link, scheduled_at, status, created_at, updated_at)
             VALUES ('kept', NULL, NULL, '2026-01-01T00:00:00Z', 'scheduled', 'x', 'x')",
            [],
        )
        .expect("seed");

        let db = Db::from_connection(conn).expect("upgrade");
        assert_eq!(
            db.get_meta("schema_version").expect("version"),
            Some("3".to_string())
        );
        assert_eq!(db.list_posts().expect("posts").len(), 1);
        // The v2 tables now exist and are empty.
        assert!(db.list_notes().expect("notes").is_empty());
        // And v3's `views` column is selectable, which `list_metrics` would
        // fail on if the ALTER had not run.
        assert!(db.list_metrics().expect("metrics").is_empty());
    }

    #[test]
    fn a_step_that_fails_part_way_leaves_neither_its_changes_nor_its_version() {
        // The first statement lands, the second fails. Were the two not one
        // transaction with the version stamp, the table would survive and the
        // next launch would re-run the step into "table already exists" — or,
        // for an ALTER, "duplicate column" — and never open the store again.
        let db = Db::open_in_memory().expect("store");
        let mut conn = db.lock();
        let err = apply_step(
            &mut conn,
            SCHEMA_VERSION + 1,
            "CREATE TABLE half_done (x INTEGER); ALTER TABLE no_such_table ADD COLUMN y;",
        );
        assert!(err.is_err());
        let version: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .expect("version");
        assert_eq!(version, SCHEMA_VERSION.to_string());
        let leftover: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'half_done'",
                [],
                |row| row.get(0),
            )
            .expect("schema");
        assert_eq!(leftover, 0, "the half-applied step was not rolled back");
    }

    #[test]
    fn migrating_twice_is_a_no_op() {
        let db = Db::open_in_memory().expect("store");
        db.save_note(None, "kept", "body", false).expect("note");
        db.migrate().expect("second run");
        assert_eq!(db.list_notes().expect("notes").len(), 1);
    }

    #[test]
    fn a_note_round_trips_and_updates_in_place() {
        let db = Db::open_in_memory().expect("store");
        let id = db
            .save_note(None, "Idea", "the body", false)
            .expect("create");
        let same = db
            .save_note(Some(id), "Idea", "edited", true)
            .expect("update");
        assert_eq!(id, same);

        let note = db.get_note(id).expect("read");
        assert_eq!(note.body, "edited");
        assert!(note.pinned);
        assert_eq!(db.list_notes().expect("list").len(), 1);
    }

    #[test]
    fn pinned_notes_sort_above_more_recent_ones() {
        let db = Db::open_in_memory().expect("store");
        let pinned = db.save_note(None, "pinned", "a", true).expect("a");
        db.save_note(None, "newer", "b", false).expect("b");
        assert_eq!(db.list_notes().expect("list")[0].id, pinned);
    }

    #[test]
    fn metrics_replace_rather_than_accumulate() {
        let (db, _, post, target) = {
            let db = Db::open_in_memory().expect("store");
            let account = db
                .upsert_account(PlatformId::Bluesky, &connected("did:1", "me"))
                .expect("account");
            let post = db
                .create_post("hi", None, None, Some(&now_rfc3339()), POST_SCHEDULED)
                .expect("post");
            db.set_targets(post, &[(account, serde_json::json!({}))])
                .expect("targets");
            let target = db.list_targets(post).expect("targets")[0].id;
            (db, account, post, target)
        };
        let _ = post;

        for likes in [3, 9] {
            db.save_metrics(&Metrics {
                target_id: target,
                fetched_at: now_rfc3339(),
                likes: Some(likes),
                reposts: Some(1),
                replies: None,
                quotes: None,
                views: None,
            })
            .expect("save");
        }

        let stored = db.list_metrics().expect("metrics");
        assert_eq!(stored.len(), 1, "a refresh replaces, it does not append");
        assert_eq!(stored[0].likes, Some(9));
        assert_eq!(
            stored[0].replies, None,
            "an unreported dimension is not a zero"
        );
    }

    #[test]
    fn published_targets_skips_everything_still_pending() {
        let (db, _, post) = {
            let db = Db::open_in_memory().expect("store");
            let a = db
                .upsert_account(PlatformId::Bluesky, &connected("did:a", "a"))
                .expect("a");
            let b = db
                .upsert_account(PlatformId::Mastodon, &connected("id:b", "b"))
                .expect("b");
            let post = db
                .create_post("hi", None, None, Some(&now_rfc3339()), POST_SCHEDULED)
                .expect("post");
            db.set_targets(
                post,
                &[(a, serde_json::json!({})), (b, serde_json::json!({}))],
            )
            .expect("targets");
            (db, a, post)
        };
        let targets = db.list_targets(post).expect("targets");
        db.finish_target_ok(targets[0].id, "at://1", None)
            .expect("ok");

        let published = db.published_targets().expect("published");
        assert_eq!(published.len(), 1);
        assert_eq!(published[0].0.id, targets[0].id);
    }

    #[test]
    fn reconnecting_the_same_remote_account_keeps_its_id() {
        let db = Db::open_in_memory().expect("store");
        let first = db
            .upsert_account(PlatformId::Bluesky, &connected("did:1", "old.bsky.social"))
            .expect("insert");
        let second = db
            .upsert_account(PlatformId::Bluesky, &connected("did:1", "new.bsky.social"))
            .expect("update");
        assert_eq!(first, second, "a re-auth must not orphan scheduled posts");
        assert_eq!(
            db.get_account(first).expect("read").handle,
            "new.bsky.social"
        );
    }

    #[test]
    fn a_due_post_is_found_and_a_future_one_is_not() {
        let (db, _, _) = seeded();
        assert_eq!(db.due_targets(Utc::now(), 10).expect("due").len(), 1);
        assert!(
            db.due_targets(Utc::now() - chrono::Duration::hours(1), 10)
                .expect("due")
                .is_empty(),
            "a post scheduled for later must not be picked up early"
        );
    }

    #[test]
    fn backoff_hides_a_target_until_its_next_attempt() {
        let (db, _, post) = seeded();
        let target = db.list_targets(post).expect("targets")[0].id;
        db.finish_target_err(
            target,
            "boom",
            Some(Utc::now() + chrono::Duration::minutes(5)),
        )
        .expect("park");
        assert!(db.due_targets(Utc::now(), 10).expect("due").is_empty());
        assert_eq!(
            db.due_targets(Utc::now() + chrono::Duration::minutes(6), 10)
                .expect("due")
                .len(),
            1
        );
    }

    #[test]
    fn claiming_a_target_twice_fails_the_second_time() {
        let (db, _, post) = seeded();
        let target = db.list_targets(post).expect("targets")[0].id;
        assert!(db.claim_target(target).expect("first claim"));
        assert!(
            !db.claim_target(target).expect("second claim"),
            "two scheduler passes must never publish the same target twice"
        );
    }

    #[test]
    fn post_status_follows_its_targets() {
        let db = Db::open_in_memory().expect("store");
        let a = db
            .upsert_account(PlatformId::Bluesky, &connected("did:a", "a"))
            .expect("a");
        let b = db
            .upsert_account(PlatformId::Mastodon, &connected("id:b", "b"))
            .expect("b");
        let post = db
            .create_post("hi", None, None, Some(&now_rfc3339()), POST_SCHEDULED)
            .expect("post");
        db.set_targets(
            post,
            &[(a, serde_json::json!({})), (b, serde_json::json!({}))],
        )
        .expect("targets");
        let targets = db.list_targets(post).expect("targets");

        db.finish_target_ok(targets[0].id, "1", None).expect("ok");
        assert_eq!(
            db.reconcile_post_status(post).expect("status"),
            POST_PUBLISHING
        );

        db.finish_target_err(targets[1].id, "nope", None)
            .expect("fail");
        assert_eq!(
            db.reconcile_post_status(post).expect("status"),
            POST_PARTIAL
        );
    }

    #[test]
    fn retargeting_keeps_a_destination_that_already_published() {
        let (db, account, post) = seeded();
        let target = db.list_targets(post).expect("targets")[0].id;
        db.finish_target_ok(target, "at://1", Some("https://example/1"))
            .expect("published");

        // The user deselects that account and picks nothing at all.
        db.set_targets(post, &[]).expect("retarget");
        let targets = db.list_targets(post).expect("targets");
        assert_eq!(
            targets.len(),
            1,
            "a published destination cannot be un-posted"
        );
        assert_eq!(targets[0].account_id, account);
        assert_eq!(targets[0].remote_url.as_deref(), Some("https://example/1"));
    }

    #[test]
    fn deleting_an_account_takes_its_targets_with_it() {
        let (db, account, post) = seeded();
        db.delete_account(account).expect("delete");
        assert!(db.list_targets(post).expect("targets").is_empty());
    }

    #[test]
    fn overdue_finds_only_posts_still_waiting() {
        let db = Db::open_in_memory().expect("store");
        let past = (Utc::now() - chrono::Duration::hours(3)).to_rfc3339();
        let waiting = db
            .create_post("late", None, None, Some(&past), POST_SCHEDULED)
            .expect("post");
        db.create_post("done", None, None, Some(&past), POST_PUBLISHED)
            .expect("post");

        let overdue = db.overdue_posts(Utc::now()).expect("overdue");
        assert_eq!(overdue.len(), 1);
        assert_eq!(overdue[0].id, waiting);
    }
}
