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
use crate::platforms::MediaRule;
use crate::{atproto, dpop, http};

pub struct Bluesky;

const DEFAULT_PDS: &str = "https://bsky.social";
const COLLECTION: &str = "app.bsky.feed.post";

/// `app.bsky.embed.images` takes any `image/*` blob up to 2,000,000 bytes:
/// <https://github.com/bluesky-social/atproto/blob/main/lexicons/app/bsky/embed/images.json>
/// Video is a different embed (`app.bsky.embed.video`) behind its own upload
/// service, which this adapter does not implement — so an MP4 is refused here
/// instead of being sent into an image embed.
const MEDIA: &[MediaRule] = &[
    MediaRule::up_to("image/png", 2_000_000),
    MediaRule::up_to("image/jpeg", 2_000_000),
    MediaRule::up_to("image/gif", 2_000_000),
    MediaRule::up_to("image/webp", 2_000_000),
];

impl Platform for Bluesky {
    fn info(&self) -> PlatformInfo {
        PlatformInfo {
            id: PlatformId::Bluesky,
            name: "Bluesky",
            auth: AuthKind::Credentials,
            limits: Limits {
                max_chars: 300,
                max_media: 4,
                accepts: MEDIA,
                supports_alt_text: true,
                requires_title: false,
                requires_media: false,
                link_is_content: false,
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
        if let Some(done) = already_created(request, &pds)? {
            return Ok(done);
        }
        let rkey = record_key(request)?;
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
                    "rkey": rkey,
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
    let mut facets = link_facets(body);
    facets.extend(tag_facets(body));
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
    if let Some(done) = already_created(request, &context.pds)? {
        return Ok(done);
    }
    let rkey = record_key(request)?;

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
        "rkey": rkey,
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

// ─── Record keys ────────────────────────────────────────────────────────────
//
// Every post is created under a record key chosen HERE and stored on the target
// before `createRecord` goes out. A retry therefore asks the PDS whether that
// key already holds a record — the earlier attempt landed and only its answer
// was lost — and if so reports that post instead of creating a second one.
// Asking first matters: the reference PDS answers a create on a taken key with
// a bare 500, which would read as an outage and be retried for ever.
//
// Minted fresh rather than derived from the target id: target ids restart at 1
// in a new or reset store, and the same key in the same repo would make an old
// post read as this one.

const TID_ALPHABET: &str = "234567abcdefghijklmnopqrstuvwxyz";

/// A TID, the key type `app.bsky.feed.post` declares: the top bit 0, 53 bits of
/// microseconds since the epoch, 10 bits of clock id, written as 13
/// base32-sortable characters.
fn tid(micros: u64, clock: u16) -> String {
    let value = ((micros & ((1 << 53) - 1)) << 10) | u64::from(clock & 0x3FF);
    let alphabet = TID_ALPHABET.as_bytes();
    (0..13u32)
        .rev()
        .map(|digit| {
            let index = usize::try_from((value >> (digit * 5)) & 31).unwrap_or_default();
            char::from(alphabet[index])
        })
        .collect()
}

/// The post an earlier attempt already created under this target's key, if
/// there is one.
fn already_created(request: &PublishRequest<'_>, pds: &str) -> Result<Option<Published>> {
    let Some(rkey) = request.resume_key else {
        return Ok(None);
    };
    // `getRecord` is public on every PDS, so neither auth path needs a token.
    let (status, body) = http::read_body(
        http::client()
            .get(format!("{pds}/xrpc/com.atproto.repo.getRecord"))
            .query(&[
                ("repo", request.account.remote_id.as_str()),
                ("collection", COLLECTION),
                ("rkey", rkey),
            ])
            .send()?,
    );
    Ok(found_record(status, &body)?.map(|uri| Published {
        remote_url: Some(permalink(&request.account.handle, &uri)),
        remote_id: uri,
    }))
}

/// A `getRecord` answer: the record's URI, `None` for a key nothing holds.
fn found_record(status: u16, body: &str) -> Result<Option<String>> {
    if (200..300).contains(&status) {
        let record: CreateRecord = serde_json::from_str(body).map_err(|e| {
            AppError::Platform(format!("Bluesky returned an unreadable record: {e}"))
        })?;
        return Ok(Some(record.uri));
    }
    if status == 400 && body.contains("RecordNotFound") {
        return Ok(None);
    }
    Err(from_status(status, body, "Bluesky"))
}

/// The key this attempt creates the post under: the stored one on a retry, a
/// fresh TID — stored before it is used — otherwise.
fn record_key(request: &PublishRequest<'_>) -> Result<String> {
    if let Some(rkey) = request.resume_key {
        return Ok(rkey.to_string());
    }
    let micros = u64::try_from(chrono::Utc::now().timestamp_micros()).unwrap_or_default();
    let rkey = tid(micros, rand::random::<u16>());
    (request.keep_resume_key)(Some(&rkey))?;
    Ok(rkey)
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

/// Hashtags, found by the rules Bluesky's own client uses (`TAG_REGEX` and
/// `detectFacets` in `@atproto/api`), so a tag Windbag posts is the tag
/// bsky.app would have made of the same text. Like a link, a `#word` with no
/// facet is plain text: not clickable, not searchable as a tag.
///
/// A tag starts with `#` or `＃` at the start of the text or after whitespace
/// — which is also what keeps a URL's `#fragment` out — and runs to the next
/// whitespace or invisible separator. It must hold at least one character that
/// is neither a digit nor punctuation (`#1` is not a tag, `#1st` is), loses its
/// trailing punctuation, and is dropped past 64 graphemes. The facet spans the
/// `#` and the tag; the tag value is stored without it.
fn tag_facets(text: &str) -> Vec<serde_json::Value> {
    // Zero-width and soft separators the reference regex also stops at.
    const INVISIBLE: [char; 7] = [
        '\u{00AD}', '\u{2060}', '\u{200A}', '\u{200B}', '\u{200C}', '\u{200D}', '\u{20E2}',
    ];
    let ends_tag = |ch: char| ch.is_whitespace() || INVISIBLE.contains(&ch);

    let mut facets = Vec::new();
    let mut previous: Option<char> = None;
    for (start, hash) in text.char_indices() {
        let after_space = previous.is_none_or(char::is_whitespace);
        previous = Some(hash);
        if !(after_space && (hash == '#' || hash == '＃')) {
            continue;
        }
        let rest = &text[start + hash.len_utf8()..];
        let run = &rest[..rest.find(ends_tag).unwrap_or(rest.len())];
        // `#️⃣` is the keycap emoji, not a tag.
        if run.starts_with('\u{FE0F}')
            || !run
                .chars()
                .any(|ch| !ch.is_ascii_digit() && !is_punctuation(ch))
        {
            continue;
        }
        let tag = run.trim_end_matches(is_punctuation);
        if tag.is_empty() || tag.graphemes(true).count() > 64 {
            continue;
        }
        let end = start + hash.len_utf8() + tag.len();
        facets.push(json!({
            "index": { "byteStart": start, "byteEnd": end },
            "features": [{ "$type": "app.bsky.richtext.facet#tag", "tag": tag }],
        }));
    }
    facets
}

/// Unicode's punctuation category (`\p{P}`) for the scripts a post is likely to
/// use: ASCII, Latin-1, General Punctuation, CJK and fullwidth forms. Not the
/// symbols — `$`, `+`, `<`, `=`, `>`, `^`, `` ` ``, `|`, `~` are not
/// punctuation to Unicode, and the reference client keeps them in a tag.
fn is_punctuation(ch: char) -> bool {
    matches!(ch,
        '!' | '"' | '#' | '%' | '&' | '\'' | '(' | ')' | '*' | ',' | '-' | '.' | '/'
        | ':' | ';' | '?' | '@' | '[' | '\\' | ']' | '_' | '{' | '}'
        | '\u{A1}' | '\u{A7}' | '\u{AB}' | '\u{B6}' | '\u{B7}' | '\u{BB}' | '\u{BF}'
        | '\u{2010}'..='\u{2027}' | '\u{2030}'..='\u{2043}' | '\u{2045}'..='\u{2051}'
        | '\u{2053}'..='\u{205E}'
        | '\u{3001}'..='\u{3003}' | '\u{3008}'..='\u{3011}' | '\u{3014}'..='\u{301F}'
        | '\u{3030}' | '\u{303D}' | '\u{30FB}'
        | '\u{FF01}'..='\u{FF03}' | '\u{FF05}'..='\u{FF0A}' | '\u{FF0C}'..='\u{FF0F}'
        | '\u{FF1A}' | '\u{FF1B}' | '\u{FF1F}' | '\u{FF20}' | '\u{FF3B}'..='\u{FF3D}'
        | '\u{FF3F}' | '\u{FF5B}' | '\u{FF5D}' | '\u{FF5F}'..='\u{FF65}'
    )
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
    fn a_record_key_is_a_valid_tid() {
        // The atproto TID spec: 13 base32-sortable characters, the first one
        // limited because the top bit is always 0.
        for (micros, clock) in [(0, 0), (1_790_000_000_000_000, 1023), ((1 << 53) - 1, 7)] {
            let key = tid(micros, clock);
            assert_eq!(key.len(), 13, "{key}");
            assert!("234567abcdefghij".contains(&key[..1]), "{key}");
            assert!(key.chars().all(|ch| TID_ALPHABET.contains(ch)), "{key}");
        }
        assert_eq!(tid(0, 0), "2222222222222");
    }

    #[test]
    fn later_record_keys_sort_after_earlier_ones() {
        assert!(tid(1_790_000_000_000_001, 0) > tid(1_790_000_000_000_000, 1023));
    }

    #[test]
    fn a_missing_record_is_not_an_error_and_a_found_one_yields_its_uri() {
        assert_eq!(
            found_record(
                400,
                r#"{"error":"RecordNotFound","message":"Could not locate record"}"#
            )
            .expect("not found"),
            None
        );
        assert_eq!(
            found_record(
                200,
                r#"{"uri":"at://did:plc:a/app.bsky.feed.post/3k","cid":"b","value":{}}"#
            )
            .expect("found"),
            Some("at://did:plc:a/app.bsky.feed.post/3k".to_string())
        );
        assert!(
            found_record(503, "down")
                .expect_err("outage")
                .is_retryable()
        );
    }

    /// The tags `tag_facets` found, with the text each facet spans.
    fn tags(text: &str) -> Vec<(String, String)> {
        tag_facets(text)
            .iter()
            .map(|facet| {
                let (start, end) = span(facet);
                (
                    facet["features"][0]["tag"]
                        .as_str()
                        .expect("tag")
                        .to_string(),
                    text[start..end].to_string(),
                )
            })
            .collect()
    }

    #[test]
    fn a_hashtag_gets_a_tag_facet_over_its_byte_range() {
        assert_eq!(
            tags("shipping #rust today"),
            vec![("rust".into(), "#rust".into())]
        );
    }

    #[test]
    fn trailing_punctuation_is_not_part_of_the_tag() {
        assert_eq!(
            tags("so good #rust!"),
            vec![("rust".into(), "#rust".into())]
        );
        assert_eq!(tags("(#rust)."), vec![], "a tag must follow a space");
    }

    #[test]
    fn digits_alone_are_not_a_tag_but_digits_with_letters_are() {
        assert!(tags("we are #1").is_empty());
        assert_eq!(tags("#1st"), vec![("1st".into(), "#1st".into())]);
    }

    #[test]
    fn a_fragment_inside_a_url_is_not_a_tag() {
        assert!(tags("see https://example.com/#section").is_empty());
        assert!(tags("a#b").is_empty());
    }

    #[test]
    fn tag_ranges_are_utf8_bytes_and_non_latin_tags_count() {
        assert_eq!(
            tags("καλημέρα #ελλάδα"),
            vec![("ελλάδα".into(), "#ελλάδα".into())]
        );
        assert_eq!(tags("＃日本"), vec![("日本".into(), "＃日本".into())]);
    }

    #[test]
    fn a_keycap_and_an_overlong_tag_are_skipped() {
        assert!(tags("press #\u{FE0F}\u{20E3}").is_empty());
        assert!(tags(&format!("#{}", "a".repeat(65))).is_empty());
        assert_eq!(tags(&format!("#{}", "a".repeat(64))).len(), 1);
    }

    #[test]
    fn a_record_carries_link_and_tag_facets_together() {
        let record = post_record("read https://example.com #rust");
        assert_eq!(record["facets"].as_array().expect("facets").len(), 2);
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
