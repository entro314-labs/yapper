//! The agent door: an MCP server over the same store the app uses.
//!
//! Run as `yapper-mcp`, a stdio JSON-RPC 2.0 server any agent host can spawn:
//!
//! ```text
//! claude mcp add yapper -- /path/to/yapper-mcp
//! codex mcp add yapper -- /path/to/yapper-mcp
//! ```
//!
//! This is where "give it context and let it schedule things" actually lives.
//! Inside Yapper the assistant only ever hands drafts to the composer, because
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
//! **It cannot queue what the scheduler would bounce.** `create_post` runs the
//! same [`platforms::validate`] the composer runs and refuses with the
//! platform's own message. An over-limit draft rejected at 09:00 tomorrow, with
//! nobody watching, is the failure this prevents.
//!
//! The transport is newline-delimited JSON-RPC on stdin/stdout. Diagnostics go
//! to stderr; stdout carries protocol frames only, because anything else on it
//! corrupts the stream.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::db::{self, Db};
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
            "serverInfo": { "name": "yapper", "version": env!("CARGO_PKG_VERSION") },
            "instructions":
                "Yapper is a desktop social scheduler. Posts you create here are queued in \
                 the user's local store; they go out only while the Yapper app is running. \
                 Always call list_accounts first — every destination is an account id, and \
                 each platform has its own character limit."
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
                "No accounts are connected. The user connects them in Yapper → \
                       Accounts."
                    .into(),
            );
        }
        let rows: Vec<Value> = accounts
            .iter()
            .map(|account| {
                let adapter = platforms::adapter(account.platform);
                json!({
                    "id": account.id,
                    "platform": account.platform.as_str(),
                    "handle": account.handle,
                    "charLimit": scheduler::effective_char_limit(account, adapter),
                    "requiresTitle": adapter.info().limits.requires_title,
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
        let body = args
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let title = args
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let link = args
            .get("link")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let scheduled_at = args
            .get("scheduledAt")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if let Some(at) = scheduled_at {
            db::parse_rfc3339(at)?;
        }

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

        // Validated BEFORE anything is written. An agent that queues an
        // over-limit post gets the platform's own message now, while it can
        // still shorten it, rather than a silent failure tomorrow morning.
        for account_id in &account_ids {
            let account = self.db.get_account(*account_id)?;
            let adapter = platforms::adapter(account.platform);
            platforms::validate(
                account.platform,
                &body,
                title,
                0,
                scheduler::effective_char_limit(&account, adapter),
            )?;
        }

        let status = if scheduled_at.is_some() {
            db::POST_SCHEDULED
        } else {
            db::POST_DRAFT
        };
        let post_id = self
            .db
            .create_post(&body, title, link, scheduled_at, status)?;

        let targets: Vec<(i64, Value)> = account_ids
            .iter()
            .map(|id| {
                let options = args
                    .get("options")
                    .and_then(|all| all.get(id.to_string()))
                    .cloned()
                    .unwrap_or(json!({}));
                (*id, options)
            })
            .collect();
        self.db.set_targets(post_id, &targets)?;

        Ok(match scheduled_at {
            Some(at) => format!(
                "Scheduled post {post_id} for {at} to {} destination(s). It will go out only \
                 while the Yapper app is running — if the machine is asleep at that time, \
                 Yapper marks it missed rather than posting it late.",
                account_ids.len()
            ),
            None => format!(
                "Saved post {post_id} as a draft with {} destination(s). It has no time yet, \
                 so nothing will be sent until one is set.",
                account_ids.len()
            ),
        })
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
                "measuredDestinations": computed.engagement.measured,
                "asOf": computed.engagement.oldest_fetch,
            },
        }))?)
    }
}

/// The tool catalogue. Descriptions are written for a model deciding whether to
/// call something, so each says what it is FOR and what it costs, not just what
/// it does.
fn tool_definitions() -> Vec<Value> {
    vec![
        tool(
            "list_accounts",
            "List the connected social accounts, each with its id, platform, handle and \
             character limit. Call this before create_post — destinations are account ids, \
             and the limits differ per platform.",
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
             refused if it does not fit. It is sent only while the Yapper app is running.",
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
                        "description": "Optional URL. Becomes a card on LinkedIn, a link post on Reddit."
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
                             {\"3\": {\"subreddit\": \"rust\"}}. Reddit needs a subreddit."
                    }
                },
                "required": ["body", "accountIds"]
            }),
        ),
        tool(
            "get_stats",
            "Publishing statistics from Yapper's own records: how much published, what \
             failed and why, which hours the user posts at, plus engagement counts where \
             the platform provides them free (Bluesky and Mastodon only).",
            json!({
                "type": "object",
                "properties": {
                    "since": { "type": "string", "description": "RFC 3339 lower bound. Defaults to 30 days ago." },
                    "until": { "type": "string", "description": "RFC 3339 upper bound." },
                    "platforms": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "bluesky | mastodon | reddit | x | linkedin"
                    }
                }
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
    fn creating_a_scheduled_post_says_it_needs_the_app_running() {
        let (mut session, account) = session();
        let frame = call(
            &mut session,
            "create_post",
            json!({
                "body": "from an agent",
                "accountIds": [account],
                "scheduledAt": "2026-12-01T09:00:00Z"
            }),
        );
        let text = text_of(&frame);
        assert!(text.contains("Scheduled post"), "{text}");
        assert!(
            text.contains("while the Yapper app is running"),
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
