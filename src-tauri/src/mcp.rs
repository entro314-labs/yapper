//! The agent door: an MCP server over the same store the app uses.
//!
//! Run as `windbag --mcp`, a stdio JSON-RPC 2.0 server any agent host can spawn:
//!
//! ```text
//! claude mcp add windbag -- /path/to/windbag --mcp
//! codex mcp add windbag -- /path/to/windbag --mcp
//! ```
//!
//! This is where "give it context and let it schedule things" actually lives.
//! Inside Windbag the assistant only ever hands drafts to the composer, because
//! nothing there shows you what it is about to do. An agent host does: every
//! tool call is visible and approvable in the conversation, which is the trust
//! boundary that makes write access reasonable.
//!
//! ## Two properties the design depends on
//!
//! **It works while the app is closed** — it opens the SQLite store directly,
//! and WAL means a second process can read and write alongside the running app.
//! But nothing PUBLISHES until the app next runs, so `create_post` says so in
//! its own answer rather than letting an agent believe a post went out.
//!
//! **It cannot queue what the scheduler would bounce.** `create_post` goes
//! through the composer's own save path, [`commands::store_post`], which runs
//! the same [`platforms::validate`] and refuses with the platform's own
//! message. An over-limit draft rejected at 09:00 tomorrow, with nobody
//! watching, is the failure this prevents.
//!
//! The transport is newline-delimited JSON-RPC on stdin/stdout. Diagnostics go
//! to stderr; stdout carries protocol frames only, because anything else on it
//! corrupts the stream.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::commands::{self, SavePostInput, TargetInput};
use crate::db::Db;
use crate::error::Result;
use crate::platforms::{self, PlatformId};
use crate::scheduler;
use crate::stats::{self, StatsFilter};

/// The spec revision this server implements, echoed back on `initialize`.
const PROTOCOL_VERSION: &str = "2026-07-28";

/// One request/response cycle over the shared store.
pub struct Session {
    db: Arc<Db>,
    initialized: bool,
}

impl Session {
    pub fn new(db: Arc<Db>) -> Self {
        Self {
            db,
            initialized: false,
        }
    }

    /// Handles one frame. `None` means the message was a notification and the
    /// protocol requires no reply.
    pub fn handle(&mut self, message: &Value) -> Option<Value> {
        let id = message.get("id").cloned();
        let method = message.get("method").and_then(Value::as_str).unwrap_or("");
        let params = message.get("params").cloned().unwrap_or(Value::Null);

        // Notifications carry no id and must never be answered — a reply to one
        // is a protocol error the host will complain about.
        if id.is_none() {
            if method == "notifications/initialized" {
                self.initialized = true;
            }
            return None;
        }
        let id = id.unwrap_or(Value::Null);

        let outcome = match method {
            "initialize" => Ok(Self::initialize()),
            "tools/list" => Ok(json!({ "tools": tool_definitions() })),
            "tools/call" => self.call(&params),
            // `ping` is the host's liveness check and must answer even before
            // initialize completes.
            "ping" => Ok(json!({})),
            other => {
                return Some(error_frame(
                    &id,
                    -32601,
                    &format!("unknown method `{other}`"),
                ));
            }
        };

        Some(match outcome {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            // A tool that fails is a RESULT with `isError`, not a JSON-RPC
            // error: the model is meant to read the message and adjust, which it
            // cannot do if the frame is a transport-level failure.
            Err(err) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "isError": true,
                    "content": [{ "type": "text", "text": err.to_string() }],
                }
            }),
        })
    }

    fn initialize() -> Value {
        json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": "windbag", "version": env!("CARGO_PKG_VERSION") },
            "instructions":
                "Windbag is a desktop social scheduler. Posts you create here are queued in \
                 the user's local store; they go out only while the Windbag app is running. \
                 Always call list_accounts first — every destination is an account id, and \
                 each platform has its own character limit and media rules. The meta_ads_* \
                 tools are a passthrough to Meta's own hosted ads MCP server and reach live \
                 ad accounts, so treat any write there as spending money."
        })
    }

    fn call(&self, params: &Value) -> Result<Value> {
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        let args = params.get("arguments").cloned().unwrap_or(json!({}));

        let text = match name {
            "list_accounts" => self.list_accounts()?,
            "list_posts" => self.list_posts()?,
            "list_notes" => self.list_notes()?,
            "create_note" => self.create_note(&args)?,
            "create_post" => self.create_post(&args)?,
            "get_stats" => self.get_stats(&args)?,
            "meta_ads_tools" => self.meta_ads_tools()?,
            "meta_ads_call" => self.meta_ads_call(&args)?,
            other => {
                return Err(crate::error::AppError::NotFound(format!(
                    "No tool named `{other}`."
                )));
            }
        };
        Ok(json!({ "content": [{ "type": "text", "text": text }] }))
    }

    // ─── Tools ──────────────────────────────────────────────────────────────

    fn list_accounts(&self) -> Result<String> {
        let accounts = self.db.list_accounts()?;
        if accounts.is_empty() {
            return Ok(
                "No accounts are connected. The user connects them in Windbag → \
                       Accounts."
                    .into(),
            );
        }
        let rows: Vec<Value> = accounts
            .iter()
            .map(|account| {
                let adapter = platforms::adapter(account.platform);
                let limits = adapter.info().limits;
                json!({
                    "id": account.id,
                    "platform": account.platform.as_str(),
                    "handle": account.handle,
                    "charLimit": scheduler::effective_char_limit(account, adapter),
                    "requiresTitle": limits.requires_title,
                    // Instagram has no text-only post. An agent that does not
                    // know this queues a caption and finds out at publish time.
                    "requiresMedia": limits.requires_media,
                    "maxMedia": limits.max_media,
                    "status": account.status,
                })
            })
            .collect();
        Ok(serde_json::to_string_pretty(&rows)?)
    }

    fn list_posts(&self) -> Result<String> {
        let posts = self.db.list_posts()?;
        let rows: Vec<Value> = posts
            .iter()
            .take(100)
            .map(|detail| {
                json!({
                    "id": detail.post.id,
                    "status": detail.post.status,
                    "scheduledAt": detail.post.scheduled_at,
                    "body": detail.post.body,
                    "title": detail.post.title,
                    "destinations": detail.targets.iter().map(|target| json!({
                        "accountId": target.account_id,
                        "status": target.status,
                        "url": target.remote_url,
                        "error": target.error,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        Ok(serde_json::to_string_pretty(&rows)?)
    }

    fn list_notes(&self) -> Result<String> {
        let notes = self.db.list_notes()?;
        let rows: Vec<Value> = notes
            .iter()
            .map(|note| {
                json!({
                    "id": note.id,
                    "title": note.title,
                    "body": note.body,
                    "pinned": note.pinned,
                    "updatedAt": note.updated_at,
                })
            })
            .collect();
        Ok(serde_json::to_string_pretty(&rows)?)
    }

    fn create_note(&self, args: &Value) -> Result<String> {
        let title = args
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let body = args
            .get("body")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| crate::error::AppError::InvalidInput("A note needs a `body`.".into()))?;
        let id = self.db.save_note(None, title, body, false)?;
        Ok(format!("Saved note {id}."))
    }

    fn create_post(&self, args: &Value) -> Result<String> {
        let text = |key: &str| args.get(key).and_then(Value::as_str).map(str::to_owned);
        let account_ids: Vec<i64> = args
            .get("accountIds")
            .and_then(Value::as_array)
            .map(|ids| ids.iter().filter_map(Value::as_i64).collect())
            .unwrap_or_default();
        if account_ids.is_empty() {
            return Err(crate::error::AppError::InvalidInput(
                "`accountIds` must name at least one account — call list_accounts first.".into(),
            ));
        }

        // The composer's own save path, so an agent's post is held to exactly
        // the same rules and validated BEFORE anything is written: an agent
        // that queues an over-limit post gets the platform's own message now,
        // while it can still shorten it, rather than a silent failure tomorrow
        // morning.
        let post_id = commands::store_post(
            &self.db,
            &SavePostInput {
                id: None,
                body: text("body").unwrap_or_default(),
                title: text("title"),
                link: text("link"),
                scheduled_at: text("scheduledAt"),
                targets: account_ids
                    .iter()
                    .map(|id| TargetInput {
                        account_id: *id,
                        options: args
                            .get("options")
                            .and_then(|all| all.get(id.to_string()))
                            .cloned()
                            .unwrap_or(json!({})),
                    })
                    .collect(),
                media: Vec::new(),
            },
        )?;
        // Read back rather than re-derived: the answer names the instant as
        // stored, in canonical UTC.
        let scheduled_at = self.db.get_post(post_id)?.scheduled_at;

        Ok(match scheduled_at {
            Some(at) => format!(
                "Scheduled post {post_id} for {at} to {} destination(s). It will go out only \
                 while the Windbag app is running — if the machine is asleep at that time, \
                 Windbag marks it missed rather than posting it late.",
                account_ids.len()
            ),
            None => format!(
                "Saved post {post_id} as a draft with {} destination(s). It has no time yet, \
                 so nothing will be sent until one is set.",
                account_ids.len()
            ),
        })
    }

    // ─── Meta ads, forwarded to Meta's own MCP server ───────────────────────

    fn meta_ads_tools(&self) -> Result<String> {
        let result = crate::metaads::AdsClient::from_store(&self.db)?.list_tools()?;
        Ok(serde_json::to_string_pretty(&result)?)
    }

    fn meta_ads_call(&self, args: &Value) -> Result<String> {
        let name = args
            .get("tool")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                crate::error::AppError::InvalidInput(
                    "`tool` must name one of the tools from meta_ads_tools.".into(),
                )
            })?;
        let arguments = args.get("arguments").cloned().unwrap_or(json!({}));
        let result =
            crate::metaads::AdsClient::from_store(&self.db)?.call_tool(name, &arguments)?;
        Ok(serde_json::to_string_pretty(&result)?)
    }

    fn get_stats(&self, args: &Value) -> Result<String> {
        let filter = StatsFilter {
            since: args
                .get("since")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| Some(stats::default_since())),
            until: args.get("until").and_then(Value::as_str).map(str::to_owned),
            platforms: args
                .get("platforms")
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .filter_map(|value| PlatformId::parse(value).ok())
                        .collect()
                })
                .unwrap_or_default(),
            account_ids: Vec::new(),
        };
        let computed = stats::compute(&self.db, &filter)?;
        Ok(serde_json::to_string_pretty(&json!({
            "published": computed.published,
            "failed": computed.failed,
            "scheduled": computed.scheduled,
            "drafts": computed.drafts,
            "missed": computed.missed,
            "byPlatform": computed.by_platform.iter().map(|bucket| json!({
                "platform": bucket.key,
                "published": bucket.published,
                "failed": bucket.failed,
            })).collect::<Vec<_>>(),
            "byHour": computed.by_hour.iter().map(|bucket| json!({
                "hour": bucket.label,
                "published": bucket.published,
            })).collect::<Vec<_>>(),
            "failures": computed.failures.iter().map(|bucket| json!({
                "kind": bucket.label,
                "count": bucket.failed,
            })).collect::<Vec<_>>(),
            "engagement": {
                "likes": computed.engagement.likes,
                "reposts": computed.engagement.reposts,
                "replies": computed.engagement.replies,
                "views": computed.engagement.views,
                "measuredDestinations": computed.engagement.measured,
                "asOf": computed.engagement.oldest_fetch,
            },
            "engagementByPlatform": computed.engagement_by_platform.iter().map(|row| json!({
                "platform": row.platform,
                "likes": row.likes,
                "reposts": row.reposts,
                "replies": row.replies,
                "views": row.views,
                "measuredDestinations": row.measured,
            })).collect::<Vec<_>>(),
            "topPosts": computed.top_posts.iter().map(|post| json!({
                "postId": post.post_id,
                "platform": post.platform,
                "handle": post.handle,
                "excerpt": post.excerpt,
                "publishedAt": post.published_at,
                "url": post.remote_url,
                "likes": post.likes,
                "reposts": post.reposts,
                "replies": post.replies,
                "views": post.views,
                "interactions": post.interactions,
            })).collect::<Vec<_>>(),
            // Named so a model does not read a missing platform as a zero.
            "unreadable": computed.engagement_gaps.iter().map(|(platform, reason)| json!({
                "platform": platform,
                "reason": reason,
            })).collect::<Vec<_>>(),
        }))?)
    }
}

/// The tool catalogue. Descriptions are written for a model deciding whether to
/// call something, so each says what it is FOR and what it costs, not just what
/// it does.
///
/// Split in two because the halves answer to different owners: [`store_tools`]
/// is this store, [`ads_tools`] is a passthrough to a server somebody else runs
/// against live ad accounts.
fn tool_definitions() -> Vec<Value> {
    let mut tools = store_tools();
    tools.extend(ads_tools());
    tools
}

/// Everything backed by the local store.
fn store_tools() -> Vec<Value> {
    vec![
        tool(
            "list_accounts",
            "List the connected social accounts, each with its id, platform, handle, \
             character limit and media rules. Call this before create_post — destinations \
             are account ids, and the limits differ per platform.",
            json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "list_posts",
            "List the 100 most recent posts with their status and per-destination outcome, \
             including any error. Use it to see what is queued or what went wrong.",
            json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "list_notes",
            "List the user's saved notes — the scratch material they draft posts from.",
            json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "create_note",
            "Save a note. Use it to park research or an idea the user can turn into a post \
             later.",
            json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Optional short title." },
                    "body": { "type": "string", "description": "The note text." }
                },
                "required": ["body"]
            }),
        ),
        tool(
            "create_post",
            "Queue a post. With `scheduledAt` it is scheduled; without one it is saved as a \
             draft. The post is validated against every destination's limits first and \
             refused if it does not fit. It is sent only while the Windbag app is running. \
             Attachments cannot be added here, so an Instagram destination is always \
             refused — Instagram has no text-only post.",
            json!({
                "type": "object",
                "properties": {
                    "body": { "type": "string", "description": "The post text." },
                    "title": {
                        "type": "string",
                        "description": "Required for Reddit destinations, ignored elsewhere."
                    },
                    "link": {
                        "type": "string",
                        "description":
                            "Optional URL. Becomes a card on LinkedIn, a link post on Reddit, \
                             a link attachment on a text-only Threads post."
                    },
                    "scheduledAt": {
                        "type": "string",
                        "description": "RFC 3339 UTC, e.g. 2026-09-01T09:00:00Z. Omit to save a draft."
                    },
                    "accountIds": {
                        "type": "array",
                        "items": { "type": "integer" },
                        "description": "Destination account ids from list_accounts."
                    },
                    "options": {
                        "type": "object",
                        "description":
                            "Per-destination options keyed by account id as a string, e.g. \
                             {\"3\": {\"subreddit\": \"rust\"}}. Reddit needs a subreddit; \
                             Threads takes topic_tag and reply_control."
                    }
                },
                "required": ["body", "accountIds"]
            }),
        ),
        tool(
            "get_stats",
            "Publishing statistics from Windbag's own records: how much published, what \
             failed and why, which hours the user posts at, the best-performing posts, and \
             engagement counts per platform as of the last refresh. Engagement is read from \
             Bluesky, Mastodon, Threads, Instagram, Facebook and X; Reddit and LinkedIn come \
             back under `unreadable` with a reason rather than as zero. Reads only the local \
             store — it never calls a platform, so the numbers are as of the user's last \
             refresh (`engagement.asOf`).",
            json!({
                "type": "object",
                "properties": {
                    "since": { "type": "string", "description": "RFC 3339 lower bound. Defaults to 30 days ago." },
                    "until": { "type": "string", "description": "RFC 3339 upper bound." },
                    "platforms": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description":
                            "bluesky | mastodon | reddit | x | linkedin | threads | \
                             instagram | facebook"
                    }
                }
            }),
        ),
    ]
}

/// The passthrough to Meta's hosted ads MCP server.
///
/// Advertised unconditionally rather than only when a Facebook Page is
/// connected: `tools/list` is answered before any credential is read, and a
/// catalogue that changes shape depending on stored state is harder for a model
/// to reason about than one whose tools state their own preconditions.
fn ads_tools() -> Vec<Value> {
    vec![
        tool(
            "meta_ads_tools",
            "List the tools Meta's hosted ads MCP server currently offers — reporting, \
             campaign and ad-set management, catalogues, A/B tests. Windbag forwards to Meta \
             rather than mirroring it, so this is always Meta's live catalogue. Needs a \
             connected Facebook Page whose Meta app was granted ads access.",
            json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "meta_ads_call",
            "Run one of the tools from meta_ads_tools against the user's Meta ad accounts \
             and return Meta's answer unchanged. Call meta_ads_tools first for the exact \
             name and argument schema — they are Meta's, not Windbag's. Anything that \
             changes a campaign spends real money, so confirm with the user first.",
            json!({
                "type": "object",
                "properties": {
                    "tool": {
                        "type": "string",
                        "description": "A tool name from meta_ads_tools."
                    },
                    "arguments": {
                        "type": "object",
                        "description": "That tool's own arguments, per its schema."
                    }
                },
                "required": ["tool"]
            }),
        ),
    ]
}

fn tool(name: &str, description: &str, schema: Value) -> Value {
    json!({ "name": name, "description": description, "inputSchema": schema })
}

fn error_frame(id: &Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platforms::{AccountSecret, Connected};

    fn session() -> (Session, i64) {
        let db = Arc::new(Db::open_in_memory().expect("store"));
        let account = db
            .upsert_account(
                PlatformId::Bluesky,
                &Connected {
                    remote_id: "did:1".into(),
                    handle: "me.bsky.social".into(),
                    display_name: None,
                    avatar_url: None,
                    instance: None,
                    scopes: None,
                    char_limit: None,
                    secret: AccountSecret::default(),
                },
            )
            .expect("account");
        (Session::new(db), account)
    }

    fn call(session: &mut Session, name: &str, args: Value) -> Value {
        session
            .handle(&json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": name, "arguments": args }
            }))
            .expect("a call always answers")
    }

    fn text_of(frame: &Value) -> String {
        frame
            .pointer("/result/content/0/text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    #[test]
    fn initialize_answers_with_the_protocol_version() {
        let (mut session, _) = session();
        let reply = session
            .handle(&json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize" }))
            .expect("reply");
        assert_eq!(
            reply
                .pointer("/result/protocolVersion")
                .and_then(Value::as_str),
            Some(PROTOCOL_VERSION)
        );
    }

    #[test]
    fn a_notification_is_never_answered() {
        let (mut session, _) = session();
        assert!(
            session
                .handle(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
                .is_none(),
            "replying to a notification is a protocol error"
        );
        assert!(session.initialized);
    }

    #[test]
    fn every_advertised_tool_is_actually_callable() {
        let (mut session, _) = session();
        let listed = session
            .handle(&json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }))
            .expect("reply");
        let tools = listed
            .pointer("/result/tools")
            .and_then(Value::as_array)
            .expect("tools");
        assert!(!tools.is_empty());

        for entry in tools {
            let name = entry.get("name").and_then(Value::as_str).expect("name");
            let frame = call(&mut session, name, json!({}));
            // Missing required arguments is a fine answer; "no such tool" is not.
            assert!(
                !text_of(&frame).contains("No tool named"),
                "{name} is advertised but not routed"
            );
        }
    }

    #[test]
    fn an_unknown_method_is_a_jsonrpc_error() {
        let (mut session, _) = session();
        let reply = session
            .handle(&json!({ "jsonrpc": "2.0", "id": 1, "method": "nope" }))
            .expect("reply");
        assert_eq!(
            reply.pointer("/error/code").and_then(Value::as_i64),
            Some(-32601)
        );
    }

    #[test]
    fn a_tool_failure_is_a_result_the_model_can_read_not_a_transport_error() {
        let (mut session, _) = session();
        let frame = call(&mut session, "create_post", json!({ "body": "hi" }));
        assert!(
            frame.get("error").is_none(),
            "a tool failure is not a JSON-RPC error"
        );
        assert_eq!(
            frame.pointer("/result/isError").and_then(Value::as_bool),
            Some(true)
        );
        assert!(text_of(&frame).contains("accountIds"));
    }

    #[test]
    fn a_time_with_an_offset_is_stored_as_the_same_instant_in_utc() {
        // The due query compares stored strings, so only one canonical UTC form
        // orders correctly: 09:00+02:00 is 07:00Z and must fire at 07:00Z.
        let (mut session, account) = session();
        call(
            &mut session,
            "create_post",
            json!({
                "body": "offset",
                "accountIds": [account],
                "scheduledAt": "2099-12-01T09:00:00+02:00"
            }),
        );
        let stored = session.db.list_posts().expect("posts")[0]
            .post
            .scheduled_at
            .clone();
        assert_eq!(stored.as_deref(), Some("2099-12-01T07:00:00+00:00"));
    }

    #[test]
    fn creating_a_scheduled_post_says_it_needs_the_app_running() {
        let (mut session, account) = session();
        let frame = call(
            &mut session,
            "create_post",
            json!({
                "body": "from an agent",
                "accountIds": [account],
                "scheduledAt": "2099-12-01T09:00:00Z"
            }),
        );
        let text = text_of(&frame);
        assert!(text.contains("Scheduled post"), "{text}");
        assert!(
            text.contains("while the Windbag app is running"),
            "an agent must not believe this already went out: {text}"
        );
    }

    #[test]
    fn an_over_limit_post_is_refused_before_anything_is_written() {
        let (mut session, account) = session();
        let frame = call(
            &mut session,
            "create_post",
            json!({ "body": "x".repeat(301), "accountIds": [account] }),
        );
        assert_eq!(
            frame.pointer("/result/isError").and_then(Value::as_bool),
            Some(true)
        );
        assert!(text_of(&frame).contains("300"));

        // And nothing was queued.
        let listed = text_of(&call(&mut session, "list_posts", json!({})));
        assert_eq!(listed.trim(), "[]");
    }

    #[test]
    fn a_time_already_past_is_refused_before_anything_is_written() {
        // catch_up would mark it missed within one tick while the agent was
        // told "Scheduled".
        let (mut session, account) = session();
        let frame = call(
            &mut session,
            "create_post",
            json!({
                "body": "too late",
                "accountIds": [account],
                "scheduledAt": "2020-01-01T09:00:00Z"
            }),
        );
        assert_eq!(
            frame.pointer("/result/isError").and_then(Value::as_bool),
            Some(true)
        );
        assert!(
            text_of(&frame).contains("already passed"),
            "{}",
            text_of(&frame)
        );
        assert!(session.db.list_posts().expect("posts").is_empty());
    }

    #[test]
    fn a_draft_is_created_without_a_time_and_says_so() {
        let (mut session, account) = session();
        let text = text_of(&call(
            &mut session,
            "create_post",
            json!({ "body": "no time yet", "accountIds": [account] }),
        ));
        assert!(text.contains("draft"), "{text}");
        assert!(text.contains("nothing will be sent"), "{text}");
    }

    #[test]
    fn a_reddit_destination_without_a_title_is_refused() {
        let db = Arc::new(Db::open_in_memory().expect("store"));
        let reddit = db
            .upsert_account(
                PlatformId::Reddit,
                &Connected {
                    remote_id: "r:1".into(),
                    handle: "u/me".into(),
                    display_name: None,
                    avatar_url: None,
                    instance: None,
                    scopes: None,
                    char_limit: None,
                    secret: AccountSecret::default(),
                },
            )
            .expect("account");
        let mut session = Session::new(db);
        let frame = call(
            &mut session,
            "create_post",
            json!({ "body": "body only", "accountIds": [reddit] }),
        );
        assert!(text_of(&frame).contains("title"));
    }

    #[test]
    fn accounts_are_listed_with_the_limit_an_agent_must_write_to() {
        let (mut session, _) = session();
        let text = text_of(&call(&mut session, "list_accounts", json!({})));
        assert!(text.contains("\"charLimit\": 300"), "{text}");
    }

    #[test]
    fn a_note_created_by_an_agent_shows_up_in_the_list() {
        let (mut session, _) = session();
        call(&mut session, "create_note", json!({ "body": "an idea" }));
        assert!(text_of(&call(&mut session, "list_notes", json!({}))).contains("an idea"));
    }
}
