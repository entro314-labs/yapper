//! X, through the v2 API with OAuth 2.0 + PKCE as a native public client.
//!
//! The flow here is the one proven in Katina (the X analytics app in this same
//! workspace): authorize at `x.com/i/oauth2/authorize`, exchange at
//! `api.x.com/2/oauth2/token`, and keep `offline.access` so the account survives
//! the two-hour access-token life.
//!
//! MEDIA is the chunked v2 flow — `/2/media/upload/initialize`, then one
//! `/append` per segment, then `/finalize`. There is no documented single-shot
//! upload in v2.
//!
//! BILLING, verified 2026-08-28 against `docs.x.com`: X retired its tiered plans
//! in February 2026 and the API is now pay-per-use against purchased credits.
//! There is no free tier, and creating a post costs money on the app the token
//! belongs to. That is the strongest reason the client id must be the USER's own
//! developer app and could never be one Windbag ships — a shared id would bill
//! every user's posts to one account. Errors here therefore separate "out of
//! credit" from "bad credentials": the first is not something reconnecting fixes,
//! and not something retrying fixes either.
//!
//! The 280-character default is the standard one. Premium accounts get far more,
//! which is what the per-account `char_limit` override is for.

use serde::Deserialize;
use serde_json::json;

use super::{
    AccountSecret, AppCredentials, AuthKind, ConnectInput, Connected, FieldSpec, Limits, MediaItem,
    Platform, PlatformId, PlatformInfo, PublishRequest, Published,
};
use crate::error::{AppError, Result, after_send, from_status, unreadable_after_send};
use crate::http;
use crate::oauth::{self, OAuthConfig, REDIRECT_URI};

pub struct X;

const AUTHORIZE_URL: &str = "https://x.com/i/oauth2/authorize";
const TOKEN_URL: &str = "https://api.x.com/2/oauth2/token";
pub(crate) const API_BASE: &str = "https://api.x.com/2";
const SCOPES: &str = "tweet.read tweet.write users.read media.write offline.access";
/// X caps an APPEND segment at 5 MB.
const SEGMENT_BYTES: usize = 4 * 1024 * 1024;

impl Platform for X {
    fn info(&self) -> PlatformInfo {
        PlatformInfo {
            id: PlatformId::X,
            name: "X",
            auth: AuthKind::OAuth2,
            limits: Limits {
                max_chars: 280,
                max_media: 4,
                supports_alt_text: true,
                requires_title: false,
                requires_media: false,
            },
            connect_fields: Vec::new(),
            app_fields: vec![FieldSpec::text(
                "client_id",
                "Client ID",
                "",
                "From your app's User authentication settings in the X developer portal. \
                 Set the app type to \"Native app\" so it is a public client.",
            )],
            setup_url: Some("https://developer.x.com/en/portal/dashboard"),
            redirect_uri: Some(REDIRECT_URI),
            target_fields: vec![
                FieldSpec::text(
                    "reply_settings",
                    "Who can reply",
                    "everyone",
                    "Restricts who may reply to the post.",
                )
                .optional()
                .choosing(&[
                    "everyone",
                    "following",
                    "mentionedUsers",
                    "subscribers",
                    "verified",
                ]),
            ],
            notes: "Needs your own developer app with Write access. X bills the \
                    app per post — there is no free tier.",
        }
    }

    fn connect(&self, input: &ConnectInput) -> Result<Connected> {
        let app = input.app()?;
        let (secret, granted_scopes) = oauth::authorize(&config_for(app))?;
        let me = fetch_me(&secret.access_token)?;

        if secret.refresh_token.is_none() {
            return Err(AppError::Unauthorized(
                "X issued no refresh token. Add `offline.access` to your app's scopes, \
                 otherwise the connection dies after two hours."
                    .into(),
            ));
        }

        Ok(Connected {
            remote_id: me.id,
            handle: format!("@{}", me.username),
            display_name: Some(me.name),
            avatar_url: me.profile_image_url,
            instance: None,
            scopes: granted_scopes.or_else(|| Some(SCOPES.to_string())),
            char_limit: None,
            secret,
        })
    }

    fn refresh(
        &self,
        _account: &crate::db::Account,
        secret: &AccountSecret,
        app: Option<&AppCredentials>,
    ) -> Result<Option<AccountSecret>> {
        if !oauth::needs_refresh(secret.expires_at.as_deref()) {
            return Ok(None);
        }
        let app = app.ok_or_else(|| {
            AppError::Unauthorized(
                "X's client id is missing. Re-add it in Settings → Platform apps.".into(),
            )
        })?;
        let refresh_token = secret.refresh_token.as_deref().ok_or_else(|| {
            AppError::Unauthorized("This X account has no refresh token — reconnect it.".into())
        })?;
        Ok(Some(oauth::refresh(&config_for(app), refresh_token)?))
    }

    fn publish(&self, request: &PublishRequest<'_>) -> Result<Published> {
        let token = &request.secret.access_token;

        let media_ids = request
            .media
            .iter()
            .map(|item| upload_media(token, item))
            .collect::<Result<Vec<_>>>()?;

        let mut payload = json!({ "text": request.body });
        if !media_ids.is_empty() {
            payload["media"] = json!({ "media_ids": media_ids });
        }
        if let Some(reply_settings) = request.option("reply_settings")
            && reply_settings != "everyone"
        {
            payload["reply_settings"] = json!(reply_settings);
        }

        let body = read(
            http::client()
                .post(format!("{API_BASE}/tweets"))
                .bearer_auth(token)
                .json(&payload)
                .send()
                .map_err(after_send)?,
        )?;

        let created: Created =
            serde_json::from_str(&body).map_err(|e| unreadable_after_send("X", e))?;

        let handle = request.account.handle.trim_start_matches('@');
        Ok(Published {
            remote_url: Some(format!("https://x.com/{handle}/status/{}", created.data.id)),
            remote_id: created.data.id,
        })
    }
}

fn config_for(app: &AppCredentials) -> OAuthConfig<'_> {
    OAuthConfig {
        platform: PlatformId::X,
        authorize_url: AUTHORIZE_URL.to_string(),
        token_url: TOKEN_URL.to_string(),
        client_id: &app.client_id,
        // A native app on X is a public client: PKCE stands in for the secret.
        client_secret: None,
        scopes: SCOPES,
        extra_authorize_params: &[],
        basic_auth: false,
    }
}

/// `POST /2/tweets` and `GET /2/users/me` both wrap their payload in `data`.
#[derive(Debug, Deserialize)]
struct Created {
    data: CreatedData,
}

#[derive(Debug, Deserialize)]
struct CreatedData {
    id: String,
}

#[derive(Debug, Deserialize)]
struct Envelope {
    data: MeData,
}

#[derive(Debug, Deserialize)]
struct MeData {
    id: String,
    name: String,
    username: String,
    profile_image_url: Option<String>,
}

fn fetch_me(token: &str) -> Result<MeData> {
    let body = read(
        http::client()
            .get(format!("{API_BASE}/users/me"))
            .query(&[("user.fields", "profile_image_url")])
            .bearer_auth(token)
            .send()?,
    )?;

    let envelope: Envelope = serde_json::from_str(&body)
        .map_err(|e| AppError::Platform(format!("X returned an unreadable profile: {e}")))?;
    Ok(envelope.data)
}

/// initialize → append(×n) → finalize, then a best-effort alt-text call.
fn upload_media(token: &str, item: &MediaItem) -> Result<String> {
    let media_id = initialize_upload(token, item)?;

    for (index, chunk) in item.bytes.chunks(SEGMENT_BYTES).enumerate() {
        append_segment(token, &media_id, index, chunk, &item.mime)?;
    }

    finalize_upload(token, &media_id)?;

    if let Some(alt) = &item.alt_text
        && !alt.trim().is_empty()
    {
        // Alt text is a separate endpoint with its own rate limit, and losing it
        // is not worth losing the post: a failure here is logged, not raised.
        if let Err(err) = set_alt_text(token, &media_id, alt) {
            log::warn!("X accepted the image but rejected its alt text: {err}");
        }
    }
    Ok(media_id)
}

fn initialize_upload(token: &str, item: &MediaItem) -> Result<String> {
    let body = read(
        http::client()
            .post(format!("{API_BASE}/media/upload/initialize"))
            .bearer_auth(token)
            .json(&json!({
                "media_type": item.mime,
                "total_bytes": item.bytes.len(),
                "media_category": media_category(&item.mime),
            }))
            .send()?,
    )?;
    let parsed: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
        AppError::Platform(format!("X returned an unreadable upload response: {e}"))
    })?;
    parsed
        .pointer("/data/id")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| AppError::Platform("X's upload response carried no media id.".into()))
}

fn append_segment(
    token: &str,
    media_id: &str,
    index: usize,
    chunk: &[u8],
    mime: &str,
) -> Result<()> {
    let part = reqwest::blocking::multipart::Part::bytes(chunk.to_vec())
        .file_name("segment")
        .mime_str(mime)
        .map_err(|e| AppError::InvalidInput(format!("Unsupported attachment type: {e}")))?;
    let form = reqwest::blocking::multipart::Form::new()
        .text("segment_index", index.to_string())
        .part("media", part);

    // APPEND answers 204 with no body when it works.
    read(
        http::client()
            .post(format!("{API_BASE}/media/upload/{media_id}/append"))
            .bearer_auth(token)
            .multipart(form)
            .send()?,
    )?;
    Ok(())
}

fn finalize_upload(token: &str, media_id: &str) -> Result<()> {
    read(
        http::client()
            .post(format!("{API_BASE}/media/upload/{media_id}/finalize"))
            .bearer_auth(token)
            .send()?,
    )?;
    Ok(())
}

fn set_alt_text(token: &str, media_id: &str, alt: &str) -> Result<()> {
    read(
        http::client()
            .post(format!("{API_BASE}/media/metadata"))
            .bearer_auth(token)
            .json(&json!({
                "id": media_id,
                // X caps alt text at 1,000 characters and rejects the whole call
                // when it is longer, so it is truncated here rather than lost.
                "metadata": { "alt_text": { "text": truncate(alt, 1000) } },
            }))
            .send()?,
    )?;
    Ok(())
}

/// Every X response goes through here: the body on a 2xx, otherwise the
/// mapped error — carrying, for a rate limit, the wait X's headers named.
fn read(response: reqwest::blocking::Response) -> Result<String> {
    let reset = reset_delay(response.headers(), chrono::Utc::now().timestamp());
    let (status, body) = http::read_body(response);
    if (200..300).contains(&status) {
        Ok(body)
    } else {
        Err(map_error(status, &body).with_retry_after(reset))
    }
}

/// How long a rate-limited response asked us to wait: `Retry-After` when it is
/// sent, otherwise the reset of whichever window is spent — the 15-minute one
/// (`x-rate-limit-*`) or the 24-hour user and app caps on posting
/// (`x-user-limit-24hour-*`, `x-app-limit-24hour-*`). Resets are epoch
/// seconds; the latest spent one wins, because every spent window refuses.
fn reset_delay(headers: &reqwest::header::HeaderMap, now: i64) -> Option<std::time::Duration> {
    let number = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<i64>().ok())
    };
    if let Some(seconds) = number("retry-after") {
        return u64::try_from(seconds)
            .ok()
            .map(std::time::Duration::from_secs);
    }
    ["x-rate-limit", "x-user-limit-24hour", "x-app-limit-24hour"]
        .iter()
        .filter(|window| number(&format!("{window}-remaining")) == Some(0))
        .filter_map(|window| number(&format!("{window}-reset")))
        .max()
        .and_then(|reset| u64::try_from(reset - now).ok())
        .map(std::time::Duration::from_secs)
}

fn media_category(mime: &str) -> &'static str {
    match mime {
        "image/gif" => "tweet_gif",
        mime if mime.starts_with("video/") => "tweet_video",
        _ => "tweet_image",
    }
}

fn truncate(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

/// X answers a duplicate post with 403 and a `duplicate` detail. That is not a
/// credential problem, and letting the generic mapper call it `Unauthorized`
/// would flag a perfectly good account for reconnection.
fn map_error(status: u16, body: &str) -> AppError {
    let lowered = body.to_ascii_lowercase();
    if status == 403 && lowered.contains("duplicate") {
        return AppError::Conflict(
            "X refused this as a duplicate of something already posted from this account.".into(),
        );
    }
    // Out of credit is neither a credential problem nor something a retry fixes:
    // it clears when the app's owner buys more. Terminal, with the fix named,
    // beats five silent retries into a dead end.
    //
    // The spend cap also arrives as a 429 — the rate limit's status — told
    // apart only by its problem type, `.../problems/usage-capped` (seen in
    // developer-community reports from mid-2026; X's docs do not list it).
    let usage_capped = lowered.contains("usage-capped") || lowered.contains("usagecapexceeded");
    if ((status == 403 || status == 402)
        && (lowered.contains("usage")
            || lowered.contains("cap")
            || lowered.contains("credit")
            || lowered.contains("payment")))
        || (status == 429 && usage_capped)
    {
        return AppError::InvalidInput(
            "X refused this because the developer app is out of API credit. Posting is \
             pay-per-use; top the app up in the X developer portal, then retry."
                .into(),
        );
    }
    from_status(status, body, "X")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_duplicate_is_a_conflict_not_a_credential_problem() {
        let err = map_error(
            403,
            r#"{"detail":"You are not allowed to create a Tweet with duplicate content."}"#,
        );
        assert!(matches!(err, AppError::Conflict(_)), "{err}");
        assert!(
            !err.is_retryable(),
            "reposting the same text will fail the same way"
        );
    }

    #[test]
    fn running_out_of_credit_is_terminal_and_says_how_to_fix_it() {
        for (status, body) in [
            (
                403,
                r#"{"title":"UsageCapExceeded","detail":"Product cap"}"#,
            ),
            (402, r#"{"detail":"Insufficient credit for this request"}"#),
        ] {
            let err = map_error(status, body);
            assert!(
                !err.is_retryable(),
                "retrying does nothing to refill an empty balance: {err}"
            );
            assert!(err.to_string().contains("credit"), "{err}");
        }
    }

    fn headers(pairs: &[(&'static str, &str)]) -> reqwest::header::HeaderMap {
        let mut map = reqwest::header::HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, value.parse().expect("header value"));
        }
        map
    }

    #[test]
    fn a_spent_daily_cap_waits_for_its_reset_not_the_window() {
        // The 15-minute window still has room; the 24-hour posting cap does
        // not, and it is the one that decides when a retry can work.
        let now = 1_000_000;
        let delay = reset_delay(
            &headers(&[
                ("x-rate-limit-remaining", "40"),
                ("x-rate-limit-reset", "1000900"),
                ("x-user-limit-24hour-remaining", "0"),
                ("x-user-limit-24hour-reset", "1036000"),
            ]),
            now,
        );
        assert_eq!(delay, Some(std::time::Duration::from_secs(36_000)));
    }

    #[test]
    fn retry_after_wins_and_no_spent_window_means_no_named_wait() {
        assert_eq!(
            reset_delay(&headers(&[("retry-after", "30")]), 0),
            Some(std::time::Duration::from_secs(30))
        );
        assert_eq!(
            reset_delay(
                &headers(&[
                    ("x-rate-limit-remaining", "3"),
                    ("x-rate-limit-reset", "99")
                ]),
                0
            ),
            None
        );
    }

    #[test]
    fn a_usage_cap_is_terminal_even_though_it_arrives_as_a_429() {
        // Pay-per-use: the spend cap comes back as 429, the same status as a
        // rate limit, and only the problem type tells them apart. Five retries
        // into a spent balance fix nothing.
        for body in [
            r#"{"title":"Too Many Requests","detail":"Too Many Requests","type":"https://api.x.com/2/problems/usage-capped"}"#,
            r#"{"title":"UsageCapExceeded","detail":"Usage cap exceeded: Monthly product cap","type":"https://api.x.com/2/problems/usage-capped"}"#,
        ] {
            let err = map_error(429, body);
            assert!(!err.is_retryable(), "{err}");
            assert!(err.to_string().contains("credit"), "{err}");
        }
    }

    #[test]
    fn a_plain_rate_limit_stays_retryable() {
        let err = map_error(
            429,
            r#"{"title":"Too Many Requests","detail":"Too Many Requests","type":"about:blank","status":429}"#,
        );
        assert!(err.is_retryable(), "{err}");
    }

    #[test]
    fn a_403_that_blames_the_authentication_flags_the_account() {
        let err = map_error(403, r#"{"detail":"Unsupported Authentication"}"#);
        assert!(matches!(err, AppError::Unauthorized(_)), "{err}");
    }

    #[test]
    fn a_403_for_a_forbidden_action_leaves_the_account_alone() {
        let err = map_error(
            403,
            r#"{"detail":"You are not permitted to perform this action.","status":403}"#,
        );
        assert!(matches!(err, AppError::InvalidInput(_)), "{err}");
    }

    #[test]
    fn media_categories_follow_the_mime_type() {
        assert_eq!(media_category("image/png"), "tweet_image");
        assert_eq!(media_category("image/gif"), "tweet_gif");
        assert_eq!(media_category("video/mp4"), "tweet_video");
    }

    #[test]
    fn alt_text_is_truncated_by_characters_not_bytes() {
        let greek = "α".repeat(1200);
        assert_eq!(truncate(&greek, 1000).chars().count(), 1000);
    }
}
