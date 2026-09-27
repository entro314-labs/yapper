//! Bluesky, over the AT Protocol XRPC endpoints on the account's own `PDS`.
//!
//! TWO WAYS IN, both landing on the same DID and therefore the same account row:
//!
//! * **App password** — created at bsky.app → Settings → App Passwords and typed
//!   in. No developer app anywhere, which is why this is the adapter to test the
//!   whole pipeline with. Sessions are deliberately NOT persisted: an
//!   `accessJwt` lives about two hours, shorter than the gap between most
//!   scheduled posts, so a stored one is expired more often than not. Windbag
//!   keeps the app password and mints a session immediately before each publish
//!   — one extra request, and no state machine that can drift.
//!
//! * **Sign in with Bluesky** — real AT Protocol OAuth (see [`crate::atproto`]):
//!   scoped, revocable from the account's own settings, and never handing this
//!   app a reusable password. Its tokens are `DPoP`-bound, so every request they
//!   authorize is signed by a key stored beside them.
//!
//! Which one an account uses is recorded in its stored secret, and `publish`
//! branches on it. Reconnecting an app-password account over OAuth upgrades it
//! in place — same DID, same row, same scheduled posts.

use serde::Deserialize;
use serde_json::json;
use unicode_segmentation::UnicodeSegmentation;

use super::{
    AccountSecret, AppCredentials, AuthKind, ConnectInput, Connected, FieldSpec, Limits, Platform,
    PlatformId, PlatformInfo, PublishRequest, Published,
};
use crate::error::{AppError, Result, from_status};
use crate::{atproto, dpop, http};

pub struct Bluesky;

const DEFAULT_PDS: &str = "https://bsky.social";
const COLLECTION: &str = "app.bsky.feed.post";

impl Platform for Bluesky {
    fn info(&self) -> PlatformInfo {
        PlatformInfo {
            id: PlatformId::Bluesky,
            name: "Bluesky",
            auth: AuthKind::Credentials,
            limits: Limits {
                max_chars: 300,
                max_media: 4,
                supports_alt_text: true,
                requires_title: false,
                requires_media: false,
            },
            connect_fields: vec![
                FieldSpec::text(
                    "method",
                    "Sign in with",
                    METHOD_OAUTH,
                    "OAuth is scoped and revocable from Bluesky itself; an app password is \
                     one field and works everywhere.",
                )
                .choosing(&[METHOD_OAUTH, METHOD_APP_PASSWORD]),
                FieldSpec::text(
                    "handle",
                    "Handle",
                    "you.bsky.social",
                    "Your full handle, without the leading @.",
                ),
                FieldSpec::secret(
                    "app_password",
                    "App password",
                    "xxxx-xxxx-xxxx-xxxx",
                    "App-password method only. Create one at Settings → Privacy and security \
                     → App passwords — never your account password.",
                )
                .optional(),
                FieldSpec::text(
                    "pds",
                    "Server",
                    DEFAULT_PDS,
                    "Leave blank unless you self-host your own PDS.",
                )
                .optional(),
            ],
            app_fields: vec![
                FieldSpec::text(
                    "client_id",
                    "OAuth client metadata URL",
                    atproto::DEFAULT_CLIENT_ID,
                    "Where Windbag's OAuth client document is published. A copy must live on \
                     entro314-labs.github.io (any path): the sign-in returns on a scheme derived \
                     from that host, and it is the only one this build receives.",
                )
                .optional(),
            ],
            setup_url: Some("https://bsky.app/settings/app-passwords"),
            redirect_uri: None,
            target_fields: Vec::new(),
            notes: "No developer app needed. Sign in with Bluesky, or paste an app password.",
        }
    }

    /// Bluesky counts its 300 against grapheme clusters. `chars().count()` reports
    /// 7 for a family emoji that Bluesky calls 1, so a counter built on scalars
    /// would refuse posts the server would have accepted.
    fn count_body(&self, body: &str) -> usize {
        body.graphemes(true).count()
    }

    fn connect(&self, input: &ConnectInput) -> Result<Connected> {
        if input.optional_field("method").unwrap_or(METHOD_OAUTH) == METHOD_OAUTH {
            return connect_oauth(input);
        }

        let handle = input.field("handle")?.trim_start_matches('@').to_string();
        let app_password = input.field("app_password")?.to_string();
        let pds = normalize_pds(input.optional_field("pds"));

        let session = create_session(&pds, &handle, &app_password)?;
        let profile = fetch_profile(&pds, &session.access_jwt, &session.did).ok();

        Ok(Connected {
            remote_id: session.did.clone(),
            handle: session.handle.clone(),
            display_name: profile.as_ref().and_then(|p| p.display_name.clone()),
            avatar_url: profile.and_then(|p| p.avatar),
            instance: Some(pds.clone()),
            scopes: None,
            char_limit: None,
            secret: AccountSecret {
                access_token: session.access_jwt,
                refresh_token: Some(session.refresh_jwt),
                // Deliberately no expiry: the session is re-minted per publish,
                // so nothing should ever try to refresh this one on a clock.
                expires_at: None,
                extra: json!({
                    "auth": METHOD_APP_PASSWORD,
                    "app_password": app_password,
                    "pds": pds,
                    "handle": handle,
                }),
            },
        })
    }

    fn refresh(
        &self,
        account: &crate::db::Account,
        secret: &AccountSecret,
        _app: Option<&AppCredentials>,
    ) -> Result<Option<AccountSecret>> {
        // App-password accounts mint a session per publish and have nothing to
        // refresh on a clock; only OAuth ones carry an expiring token.
        if !is_oauth(secret) || !crate::oauth::needs_refresh(secret.expires_at.as_deref()) {
            return Ok(None);
        }
        refresh_oauth(&account.remote_id, secret).map(Some)
    }

    fn publish(&self, request: &PublishRequest<'_>) -> Result<Published> {
        if is_oauth(request.secret) {
            return publish_oauth(request);
        }

        let pds = request.secret.extra_str("pds").map_or_else(
            || normalize_pds(request.account.instance.as_deref()),
            str::to_string,
        );
        let app_password = request.secret.extra_str("app_password").ok_or_else(|| {
            AppError::Unauthorized(
                "The stored Bluesky app password is missing. Reconnect the account.".into(),
            )
        })?;

        let session = create_session(&pds, &request.account.remote_id, app_password)?;

        let mut record = post_record(request.body);

        if !request.media.is_empty() {
            let images = request
                .media
                .iter()
                .map(|item| {
                    let blob = upload_blob(&pds, &session.access_jwt, &item.bytes, &item.mime)?;
                    Ok(json!({
                        "alt": item.alt_text.clone().unwrap_or_default(),
                        "image": blob,
                    }))
                })
                .collect::<Result<Vec<_>>>()?;
            record["embed"] = json!({ "$type": "app.bsky.embed.images", "images": images });
        }

        let (status, body) = http::read_body(
            http::client()
                .post(format!("{pds}/xrpc/com.atproto.repo.createRecord"))
                .bearer_auth(&session.access_jwt)
                .json(&json!({
                    "repo": session.did,
                    "collection": COLLECTION,
                    "record": record,
                }))
                .send()?,
        );
        if !(200..300).contains(&status) {
            return Err(from_status(status, &body, "Bluesky"));
        }

        let created: CreateRecord = serde_json::from_str(&body).map_err(|e| {
            AppError::Platform(format!(
                "Bluesky accepted the post but the reply was unreadable: {e}"
            ))
        })?;

        Ok(Published {
            remote_url: Some(permalink(&session.handle, &created.uri)),
            remote_id: created.uri,
        })
    }
}

pub const METHOD_OAUTH: &str = "oauth";
pub const METHOD_APP_PASSWORD: &str = "app-password";

fn is_oauth(secret: &AccountSecret) -> bool {
    secret.extra_str("auth") == Some(METHOD_OAUTH)
}

/// One post record, with link facets attached.
///
/// Bluesky stores no markup: a URL in the text is inert unless the record also
/// carries a facet pointing at its byte range. Both auth paths build the record
/// through here rather than each remembering to add them.
fn post_record(body: &str) -> serde_json::Value {
    let mut record = json!({
        "$type": COLLECTION,
        "text": body,
        // Bluesky orders timelines by this, not by receipt, so it must be the
        // moment the post actually goes out rather than when it was composed.
        "createdAt": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    });
    let facets = link_facets(body);
    if !facets.is_empty() {
        record["facets"] = json!(facets);
    }
    record
}

// ─── OAuth ──────────────────────────────────────────────────────────────────

fn connect_oauth(input: &ConnectInput) -> Result<Connected> {
    let handle = input.field("handle")?;
    let client_id = input
        .app
        .as_ref()
        .and_then(|app| app.extra("client_id"))
        .unwrap_or(atproto::DEFAULT_CLIENT_ID)
        .to_string();

    let session = atproto::authorize(&client_id, handle)?;
    // Best effort: a profile that will not load is no reason to refuse a
    // connection that otherwise worked.
    let profile = oauth_profile(&session).ok();

    Ok(Connected {
        remote_id: session.did.clone(),
        handle: handle.trim_start_matches('@').to_string(),
        display_name: profile.as_ref().and_then(|p| p.display_name.clone()),
        avatar_url: profile.and_then(|p| p.avatar),
        instance: Some(session.pds.clone()),
        scopes: session.scopes.clone(),
        char_limit: None,
        secret: AccountSecret {
            access_token: session.access_token,
            refresh_token: Some(session.refresh_token),
            expires_at: session.expires_at,
            extra: json!({
                "auth": METHOD_OAUTH,
                // The tokens are BOUND to this key. A refresh signed by any other
                // one is refused, so losing it means reconnecting the account.
                "dpop_key": session.dpop_key,
                "issuer": session.issuer,
                "pds": session.pds,
                "client_id": client_id,
            }),
        },
    })
}

/// The pieces an authenticated OAuth request needs, unpacked and checked once.
struct OAuthContext {
    key: dpop::Key,
    pds: String,
    token: String,
}

fn oauth_context(secret: &AccountSecret) -> Result<OAuthContext> {
    let missing = |what: &str| {
        AppError::Unauthorized(format!(
            "This Bluesky connection is missing its {what}. Reconnect the account."
        ))
    };
    Ok(OAuthContext {
        key: dpop::Key::from_base64(secret.extra_str("dpop_key").ok_or_else(|| missing("key"))?)?,
        pds: secret
            .extra_str("pds")
            .ok_or_else(|| missing("server"))?
            .to_string(),
        token: secret.access_token.clone(),
    })
}

/// `did` is the account's own: an OAuth account's remote id is the DID it
/// signed in as.
fn refresh_oauth(did: &str, secret: &AccountSecret) -> Result<AccountSecret> {
    let key = dpop::Key::from_base64(secret.extra_str("dpop_key").ok_or_else(|| {
        AppError::Unauthorized("This Bluesky connection lost its key. Reconnect it.".into())
    })?)?;
    let pds = secret.extra_str("pds").ok_or_else(|| {
        AppError::Unauthorized("This Bluesky connection names no server. Reconnect it.".into())
    })?;
    let client_id = secret
        .extra_str("client_id")
        .unwrap_or(atproto::DEFAULT_CLIENT_ID);
    let refresh_token = secret.refresh_token.as_deref().ok_or_else(|| {
        AppError::Unauthorized("This Bluesky connection has no refresh token.".into())
    })?;
    let issuer = secret.extra_str("issuer").ok_or_else(|| {
        AppError::Unauthorized(
            "This Bluesky connection does not record who issued it. Reconnect it.".into(),
        )
    })?;

    // Rediscovered rather than stored: a stale token endpoint would fail every
    // refresh with nothing to explain it. But the server found must still be
    // the one that issued these tokens, or it is not sent the refresh token.
    let server = atproto::discover_auth_server(pds)?;
    atproto::check_refresh_issuer(issuer, &server.issuer)?;
    let tokens = atproto::refresh(&key, &server.token_endpoint, client_id, refresh_token)?;
    atproto::check_refresh_subject(did, &tokens.sub)?;

    let expires_at = tokens.expires_at();
    Ok(AccountSecret {
        access_token: tokens.access_token,
        // AT Protocol rotates refresh tokens; dropping a new one in favour of
        // the old un-authorizes the account at the NEXT refresh, a day later.
        refresh_token: tokens
            .refresh_token
            .or_else(|| secret.refresh_token.clone()),
        expires_at,
        extra: secret.extra.clone(),
    })
}

fn publish_oauth(request: &PublishRequest<'_>) -> Result<Published> {
    let context = oauth_context(request.secret)?;

    let mut record = post_record(request.body);
    if !request.media.is_empty() {
        let images = request
            .media
            .iter()
            .map(|item| {
                let blob = oauth_upload_blob(&context, &item.bytes, &item.mime)?;
                Ok(json!({
                    "alt": item.alt_text.clone().unwrap_or_default(),
                    "image": blob,
                }))
            })
            .collect::<Result<Vec<_>>>()?;
        record["embed"] = json!({ "$type": "app.bsky.embed.images", "images": images });
    }

    let url = format!("{}/xrpc/com.atproto.repo.createRecord", context.pds);
    let payload = json!({
        "repo": request.account.remote_id,
        "collection": COLLECTION,
        "record": record,
    });
    let (status, body) = dpop::send(&context.key, "POST", &url, Some(&context.token), || {
        http::client().post(&url).json(&payload)
    })?;
    if !(200..300).contains(&status) {
        return Err(from_status(status, &body, "Bluesky"));
    }

    let created: CreateRecord = serde_json::from_str(&body).map_err(|e| {
        AppError::Platform(format!(
            "Bluesky accepted the post but the reply was unreadable: {e}"
        ))
    })?;
    Ok(Published {
        remote_url: Some(permalink(&request.account.handle, &created.uri)),
        remote_id: created.uri,
    })
}

fn oauth_upload_blob(
    context: &OAuthContext,
    bytes: &[u8],
    mime: &str,
) -> Result<serde_json::Value> {
    let url = format!("{}/xrpc/com.atproto.repo.uploadBlob", context.pds);
    let (status, body) = dpop::send(&context.key, "POST", &url, Some(&context.token), || {
        http::client()
            .post(&url)
            .header(reqwest::header::CONTENT_TYPE, mime)
            .body(bytes.to_vec())
    })?;
    if !(200..300).contains(&status) {
        return Err(from_status(status, &body, "Bluesky"));
    }
    serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|value| value.get("blob").cloned())
        .ok_or_else(|| AppError::Platform("Bluesky's upload response carried no blob.".into()))
}

fn oauth_profile(session: &atproto::Session) -> Result<Profile> {
    let key = dpop::Key::from_base64(&session.dpop_key)?;
    let url = format!("{}/xrpc/app.bsky.actor.getProfile", session.pds);
    let (status, body) = dpop::send(&key, "GET", &url, Some(&session.access_token), || {
        http::client().get(&url).query(&[("actor", &session.did)])
    })?;
    if !(200..300).contains(&status) {
        return Err(from_status(status, &body, "Bluesky"));
    }
    serde_json::from_str(&body)
        .map_err(|e| AppError::Platform(format!("Bluesky returned an unreadable profile: {e}")))
}

// ─── Wire calls ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct CreateRecord {
    uri: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Session {
    did: String,
    handle: String,
    access_jwt: String,
    refresh_jwt: String,
}

fn create_session(pds: &str, identifier: &str, password: &str) -> Result<Session> {
    let (status, body) = http::read_body(
        http::client()
            .post(format!("{pds}/xrpc/com.atproto.server.createSession"))
            .json(&json!({ "identifier": identifier, "password": password }))
            .send()?,
    );
    if !(200..300).contains(&status) {
        // 400 with `AuthenticationRequired` is what a revoked app password looks
        // like; the generic mapper would call that InvalidInput and the account
        // would never be flagged for reconnection.
        if status == 400 && body.contains("AuthenticationRequired") {
            return Err(AppError::Unauthorized(
                "Bluesky rejected the handle or app password. Create a new app password \
                 and reconnect."
                    .into(),
            ));
        }
        return Err(from_status(status, &body, "Bluesky"));
    }
    serde_json::from_str(&body)
        .map_err(|e| AppError::Platform(format!("Bluesky returned an unreadable session: {e}")))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Profile {
    display_name: Option<String>,
    avatar: Option<String>,
}

fn fetch_profile(pds: &str, access_jwt: &str, did: &str) -> Result<Profile> {
    let (status, body) = http::read_body(
        http::client()
            .get(format!("{pds}/xrpc/app.bsky.actor.getProfile"))
            .query(&[("actor", did)])
            .bearer_auth(access_jwt)
            .send()?,
    );
    if !(200..300).contains(&status) {
        return Err(from_status(status, &body, "Bluesky"));
    }
    serde_json::from_str(&body)
        .map_err(|e| AppError::Platform(format!("Bluesky returned an unreadable profile: {e}")))
}

/// Uploads one image and returns the `blob` reference to embed in a record.
fn upload_blob(pds: &str, access_jwt: &str, bytes: &[u8], mime: &str) -> Result<serde_json::Value> {
    let (status, body) = http::read_body(
        http::client()
            .post(format!("{pds}/xrpc/com.atproto.repo.uploadBlob"))
            .bearer_auth(access_jwt)
            .header(reqwest::header::CONTENT_TYPE, mime)
            .body(bytes.to_vec())
            .send()?,
    );
    if !(200..300).contains(&status) {
        return Err(from_status(status, &body, "Bluesky"));
    }
    let parsed: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
        AppError::Platform(format!(
            "Bluesky returned an unreadable upload response: {e}"
        ))
    })?;
    parsed
        .get("blob")
        .cloned()
        .ok_or_else(|| AppError::Platform("Bluesky's upload response carried no blob.".into()))
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn normalize_pds(value: Option<&str>) -> String {
    let raw = value.unwrap_or(DEFAULT_PDS).trim().trim_end_matches('/');
    if raw.is_empty() {
        DEFAULT_PDS.to_string()
    } else if raw.starts_with("http://") || raw.starts_with("https://") {
        raw.to_string()
    } else {
        format!("https://{raw}")
    }
}

/// `at://did:plc:abc/app.bsky.feed.post/3k…` → the public permalink.
fn permalink(handle: &str, uri: &str) -> String {
    let rkey = uri.rsplit('/').next().unwrap_or(uri);
    format!("https://bsky.app/profile/{handle}/post/{rkey}")
}

/// Bluesky stores no markup: a URL in the text is inert unless the record also
/// carries a facet pointing at its BYTE range. Without this, every link Windbag
/// posts is plain grey text — the post looks broken in a way the API never
/// complains about.
///
/// Ranges are UTF-8 byte offsets, not character indices, which is why this walks
/// `char_indices` rather than counting characters.
fn link_facets(text: &str) -> Vec<serde_json::Value> {
    let mut facets = Vec::new();
    let bytes = text.as_bytes();
    let mut cursor = 0usize;

    while let Some(found) = find_scheme(text, cursor) {
        let start = found;
        let mut end = start;
        while end < bytes.len() && !is_url_terminator(bytes[end]) {
            end += 1;
        }
        // Trailing punctuation belongs to the sentence, not the link: "see
        // https://example.com." must not link the full stop.
        while end > start
            && matches!(
                bytes[end - 1],
                b'.' | b',' | b';' | b':' | b'!' | b'?' | b')'
            )
        {
            end -= 1;
        }
        let url = &text[start..end];
        // A bare scheme with no host is not a link.
        if url.len() > "https://".len() {
            facets.push(json!({
                "index": { "byteStart": start, "byteEnd": end },
                "features": [{ "$type": "app.bsky.richtext.facet#link", "uri": url }],
            }));
        }
        cursor = end.max(start + 1);
    }
    facets
}

fn find_scheme(text: &str, from: usize) -> Option<usize> {
    let rest = text.get(from..)?;
    let http = rest.find("http://");
    let https = rest.find("https://");
    match (http, https) {
        (Some(a), Some(b)) => Some(from + a.min(b)),
        (Some(a), None) => Some(from + a),
        (None, Some(b)) => Some(from + b),
        (None, None) => None,
    }
}

fn is_url_terminator(byte: u8) -> bool {
    byte.is_ascii_whitespace() || byte == b'<' || byte == b'>' || byte == b'"'
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The byte span one facet covers, as `link_facets` wrote it.
    fn span(facet: &serde_json::Value) -> (usize, usize) {
        let read = |key: &str| {
            usize::try_from(facet["index"][key].as_u64().expect("byte offset")).expect("in range")
        };
        (read("byteStart"), read("byteEnd"))
    }

    #[test]
    fn a_bare_host_gets_https() {
        assert_eq!(
            normalize_pds(Some("pds.example.com")),
            "https://pds.example.com"
        );
        assert_eq!(
            normalize_pds(Some("https://pds.example.com/")),
            "https://pds.example.com"
        );
        assert_eq!(normalize_pds(None), DEFAULT_PDS);
        assert_eq!(normalize_pds(Some("   ")), DEFAULT_PDS);
    }

    #[test]
    fn a_record_uri_becomes_a_public_permalink() {
        assert_eq!(
            permalink(
                "me.bsky.social",
                "at://did:plc:abc/app.bsky.feed.post/3kabc"
            ),
            "https://bsky.app/profile/me.bsky.social/post/3kabc"
        );
    }

    #[test]
    fn a_link_gets_a_facet_over_its_byte_range() {
        let text = "read https://example.com/x now";
        let facets = link_facets(text);
        assert_eq!(facets.len(), 1);
        let (start, end) = span(&facets[0]);
        assert_eq!(&text[start..end], "https://example.com/x");
    }

    #[test]
    fn byte_ranges_survive_multibyte_text_before_the_link() {
        // "καλημέρα" is 16 bytes and 8 characters: a facet built on character
        // indices would point at the wrong span here.
        let text = "καλημέρα https://example.com";
        let facets = link_facets(text);
        let (start, end) = span(&facets[0]);
        assert_eq!(&text[start..end], "https://example.com");
        assert_ne!(start, "καλημέρα ".chars().count());
    }

    #[test]
    fn trailing_sentence_punctuation_stays_out_of_the_link() {
        let text = "see https://example.com.";
        let facets = link_facets(text);
        let (start, end) = span(&facets[0]);
        assert_eq!(&text[start..end], "https://example.com");
    }

    #[test]
    fn two_links_produce_two_facets() {
        let facets = link_facets("https://a.example and https://b.example");
        assert_eq!(facets.len(), 2);
    }

    #[test]
    fn text_without_links_produces_no_facets() {
        assert!(link_facets("just some words").is_empty());
    }
}
