//! A client for Meta's hosted ads MCP server at `https://mcp.facebook.com/ads`.
//!
//! This is the OTHER direction from [`crate::mcp`]. That module is a server:
//! Windbag's own store, exposed to an agent host. This one is a client: Meta's
//! ads tools, pulled INTO Windbag so the same agent session that schedules a post
//! can also read what the campaign behind it did.
//!
//! Windbag deliberately does not reimplement ad management. It forwards
//! `tools/list` and `tools/call` and returns whatever Meta answers, so the tool
//! catalogue is always Meta's current one rather than a copy of it that goes
//! stale. Reporting, campaign and ad-set management, catalogues, A/B tests: all
//! of it arrives without this file knowing any of their names.
//!
//! ## Auth
//!
//! Bearer, using a connected Facebook Page account's own token — the "connect
//! with your own developer app" route, which fits Windbag's credential model
//! exactly and needs no second sign-in. The ads permissions are NOT part of the
//! ordinary Page connection: `ads_access` on the Meta app credentials opts into
//! them, so a user who only schedules posts is never shown an ads consent
//! screen.
//!
//! ## Transport
//!
//! Streamable HTTP. One POST per JSON-RPC message; the response is either a JSON
//! body or an SSE stream carrying the same frame, and Meta returns either
//! depending on the tool. [`extract_frame`] reads both.

use serde_json::{Value, json};

use crate::db::Db;
use crate::error::{AppError, Result};
use crate::http;
use crate::platforms::PlatformId;

pub const ENDPOINT: &str = "https://mcp.facebook.com/ads";
/// The revision this client negotiates. Meta's server picks the highest it and
/// the client both know, so a newer server stays compatible.
const PROTOCOL_VERSION: &str = "2026-07-28";
const LABEL: &str = "the Meta ads MCP server";

/// One connection, holding the session id Meta hands back on `initialize`.
pub struct AdsClient {
    token: String,
    session: Option<String>,
}

impl AdsClient {
    /// Builds a client from the first connected Facebook Page account.
    ///
    /// The token is the PAGE token stored for that account — the same credential
    /// that publishes — so an ads call needs no separate connection, only the
    /// broader permissions the app asked for at connect time.
    pub fn from_store(db: &Db) -> Result<Self> {
        let account = db
            .list_accounts()?
            .into_iter()
            .find(|account| account.platform == PlatformId::Facebook)
            .ok_or_else(|| {
                AppError::NotFound(
                    "No Facebook Page is connected. Meta's ads tools authorize with a Meta \
                     account, so connect one in Windbag → Accounts first."
                        .into(),
                )
            })?;
        let secret = crate::secrets::load_account_secret(PlatformId::Facebook, &account.remote_id)?;
        // The USER token is what carries ads permissions — a Page token is scoped
        // to the Page and Meta rejects it for ad accounts.
        let token = secret
            .extra_str("user_token")
            .unwrap_or(&secret.access_token)
            .to_string();
        Ok(Self {
            token,
            session: None,
        })
    }

    /// Negotiates the session. Cheap enough to run per command — this is a
    /// short-lived client, not a daemon holding a connection open.
    pub fn initialize(&mut self) -> Result<Value> {
        let result = self.rpc(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "windbag", "version": env!("CARGO_PKG_VERSION") },
            }),
        )?;
        Ok(result)
    }

    pub fn list_tools(&mut self) -> Result<Value> {
        if self.session.is_none() {
            self.initialize()?;
        }
        self.rpc("tools/list", json!({}))
    }

    pub fn call_tool(&mut self, name: &str, arguments: &Value) -> Result<Value> {
        if self.session.is_none() {
            self.initialize()?;
        }
        self.rpc(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        )
    }

    /// One JSON-RPC round trip.
    fn rpc(&mut self, method: &str, params: Value) -> Result<Value> {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        });

        let mut request = http::client()
            .post(ENDPOINT)
            .bearer_auth(&self.token)
            // Both are required by the streamable-HTTP transport: the server
            // chooses which one it answers with.
            .header(
                reqwest::header::ACCEPT,
                "application/json, text/event-stream",
            )
            .header("MCP-Protocol-Version", PROTOCOL_VERSION)
            .json(&body);
        if let Some(session) = &self.session {
            request = request.header("Mcp-Session-Id", session);
        }

        let response = request.send()?;
        // Read before the body is consumed — this is how the session is issued.
        let issued = response
            .headers()
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let (status, raw) = http::read_body(response);

        if !(200..300).contains(&status) {
            return Err(match status {
                401 | 403 => AppError::Unauthorized(format!(
                    "{LABEL} rejected the credentials. The connected Facebook account needs ads \
                     permissions: set \"Ads access\" to yes in Settings → Platform apps and \
                     reconnect the Page. {}",
                    raw.chars().take(200).collect::<String>()
                )),
                _ => crate::platforms::meta::map_error(status, &raw, LABEL),
            });
        }
        if let Some(session) = issued {
            self.session = Some(session);
        }

        let frame = extract_frame(&raw)?;
        if let Some(error) = frame.get("error") {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("no message");
            return Err(AppError::Platform(format!(
                "{LABEL} returned an error: {message}"
            )));
        }
        Ok(frame.get("result").cloned().unwrap_or(Value::Null))
    }
}

/// Pulls the JSON-RPC frame out of a streamable-HTTP response body.
///
/// A plain JSON body IS the frame. An SSE body is a sequence of `data:` lines,
/// possibly several events, of which the one carrying `result` or `error` is the
/// answer — the others are progress notifications that would otherwise be
/// mistaken for it.
fn extract_frame(raw: &str) -> Result<Value> {
    let trimmed = raw.trim_start();
    if trimmed.starts_with('{') {
        return serde_json::from_str(trimmed)
            .map_err(|e| AppError::Platform(format!("{LABEL} returned unreadable JSON: {e}")));
    }

    let mut last = None;
    for line in raw.lines() {
        let Some(payload) = line.strip_prefix("data:") else {
            continue;
        };
        let payload = payload.trim();
        if payload.is_empty() || payload == "[DONE]" {
            continue;
        }
        if let Ok(frame) = serde_json::from_str::<Value>(payload) {
            // A notification has no id and is not the reply being waited on.
            if frame.get("result").is_some() || frame.get("error").is_some() {
                last = Some(frame);
            }
        }
    }
    last.ok_or_else(|| {
        AppError::Platform(format!(
            "{LABEL} sent no readable response frame: {}",
            raw.chars().take(200).collect::<String>()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_json_body_is_the_frame() {
        let frame =
            extract_frame(r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}"#).expect("frame");
        assert!(frame.pointer("/result/tools").is_some());
    }

    #[test]
    fn an_sse_body_yields_the_frame_carrying_the_result() {
        let body = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}\n\n\
                    event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}\n\n";
        let frame = extract_frame(body).expect("frame");
        assert_eq!(
            frame.pointer("/result/ok").and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn a_progress_notification_alone_is_not_mistaken_for_an_answer() {
        let body = "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/message\"}\n\n";
        assert!(extract_frame(body).is_err());
    }

    #[test]
    fn an_error_frame_survives_extraction_so_the_caller_can_report_it() {
        let body = "data: {\"jsonrpc\":\"2.0\",\"id\":1,\"error\":{\"code\":-32601,\"message\":\"nope\"}}\n";
        assert!(extract_frame(body).expect("frame").get("error").is_some());
    }
}
