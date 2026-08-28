//! X, through the v2 API with OAuth 2.0 + PKCE as a native public client.
//!
//! The flow here is the one proven in Katina (the X analytics app in this same
//! workspace): authorize at `x.com/i/oauth2/authorize`, exchange at
//! `api.x.com/2/oauth2/token`, and keep `offline.access` so the account survives
//! the two-hour access-token life.
//!
//! MEDIA is the chunked v2 flow — `/2/media/upload/initialize`, then one
//! `/append` per segment, then `/finalize`. There is no documented single-shot
//! upload in v2, and the free tier allows only 17 initialize calls per 24 hours,
//! so an image post is a genuinely scarce operation on that plan.
//!
//! The 280-character default is the free/basic tier's. Premium accounts get far
//! more, which is what the per-account `char_limit` override is for.

use serde::Deserialize;
use serde_json::json;

use super::{
    AccountSecret, AppCredentials, AuthKind, ConnectInput, Connected, FieldSpec, Limits, MediaItem,
    Platform, PlatformId, PlatformInfo, PublishRequest, Published,
};
use crate::error::{AppError, Result, from_status};
use crate::http;
use crate::oauth::{self, OAuthConfig, REDIRECT_URI};

pub struct X;

const AUTHORIZE_URL: &str = "https://x.com/i/oauth2/authorize";
const TOKEN_URL: &str = "https://api.x.com/2/oauth2/token";
const API_BASE: &str = "https://api.x.com/2";
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
                    "everyone, following, mentionedUsers, subscribers or verified.",
                )
                .optional(),
            ],
            notes: "Needs your own developer app with Write access. \
                    The free tier allows a small number of posts per month.",
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

        let (status, body) = http::read_body(
            http::client()
                .post(format!("{API_BASE}/tweets"))
                .bearer_auth(token)
                .json(&payload)
                .send()?,
        );
        if !(200..300).contains(&status) {
            return Err(map_error(status, &body));
        }

        let created: Created = serde_json::from_str(&body).map_err(|e| {
            AppError::Platform(format!(
                "X accepted the post but the reply was unreadable: {e}"
            ))
        })?;

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
    let (status, body) = http::read_body(
        http::client()
            .get(format!("{API_BASE}/users/me"))
            .query(&[("user.fields", "profile_image_url")])
            .bearer_auth(token)
            .send()?,
    );
    if !(200..300).contains(&status) {
        return Err(map_error(status, &body));
    }

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
    let (status, body) = http::read_body(
        http::client()
            .post(format!("{API_BASE}/media/upload/initialize"))
            .bearer_auth(token)
            .json(&json!({
                "media_type": item.mime,
                "total_bytes": item.bytes.len(),
                "media_category": media_category(&item.mime),
            }))
            .send()?,
    );
    if !(200..300).contains(&status) {
        return Err(map_error(status, &body));
    }
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

    let (status, body) = http::read_body(
        http::client()
            .post(format!("{API_BASE}/media/upload/{media_id}/append"))
            .bearer_auth(token)
            .multipart(form)
            .send()?,
    );
    // APPEND answers 204 with no body when it works.
    if !(200..300).contains(&status) {
        return Err(map_error(status, &body));
    }
    Ok(())
}

fn finalize_upload(token: &str, media_id: &str) -> Result<()> {
    let (status, body) = http::read_body(
        http::client()
            .post(format!("{API_BASE}/media/upload/{media_id}/finalize"))
            .bearer_auth(token)
            .send()?,
    );
    if !(200..300).contains(&status) {
        return Err(map_error(status, &body));
    }
    Ok(())
}

fn set_alt_text(token: &str, media_id: &str, alt: &str) -> Result<()> {
    let (status, body) = http::read_body(
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
    );
    if !(200..300).contains(&status) {
        return Err(map_error(status, &body));
    }
    Ok(())
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
    if status == 403 && (lowered.contains("usage") || lowered.contains("cap")) {
        return AppError::Platform(
            "X says this app has hit its monthly post cap. The free tier is small; \
             the post stays queued for retry."
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
    fn a_usage_cap_stays_retryable() {
        let err = map_error(
            403,
            r#"{"title":"UsageCapExceeded","detail":"Monthly product cap"}"#,
        );
        assert!(err.is_retryable(), "a monthly cap clears on its own");
    }

    #[test]
    fn a_plain_403_still_flags_the_account() {
        let err = map_error(403, r#"{"detail":"Unsupported Authentication"}"#);
        assert!(matches!(err, AppError::Unauthorized(_)), "{err}");
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
