//! Reddit, through an "installed app" — a public OAuth client with no secret,
//! which is the app type a desktop program is supposed to register.
//!
//! Two Reddit-specific traps are handled here and nowhere else:
//!   * The token endpoint wants HTTP Basic with an EMPTY password
//!     (`client_id:`), even though there is no secret. Sending the id as a form
//!     field returns 401 with no explanation.
//!   * `/api/submit` answers **200 OK** with the failure inside
//!     `json.errors`. Trusting the status code means recording a post that
//!     Reddit refused as published.

use serde::Deserialize;

use super::{
    AccountSecret, AppCredentials, AuthKind, ConnectInput, Connected, FieldSpec, Limits, Platform,
    PlatformId, PlatformInfo, PublishRequest, Published,
};
use crate::error::{AppError, Result, from_status};
use crate::http;
use crate::oauth::{self, OAuthConfig, REDIRECT_URI};

pub struct Reddit;

const AUTHORIZE_URL: &str = "https://www.reddit.com/api/v1/authorize";
const TOKEN_URL: &str = "https://www.reddit.com/api/v1/access_token";
const API_BASE: &str = "https://oauth.reddit.com";
const SCOPES: &str = "identity submit flair";

impl Platform for Reddit {
    fn info(&self) -> PlatformInfo {
        PlatformInfo {
            id: PlatformId::Reddit,
            name: "Reddit",
            auth: AuthKind::OAuth2,
            limits: Limits {
                max_chars: 40_000,
                // Image submissions go through a separate upload-lease flow that
                // Windbag does not implement yet; a link post covers the case.
                max_media: 0,
                supports_alt_text: false,
                requires_title: true,
                requires_media: false,
            },
            connect_fields: Vec::new(),
            app_fields: vec![FieldSpec::text(
                "client_id",
                "Client ID",
                "",
                "Create an app of type \"installed app\" at reddit.com/prefs/apps. \
                 There is no secret for that type.",
            )],
            setup_url: Some("https://www.reddit.com/prefs/apps"),
            redirect_uri: Some(REDIRECT_URI),
            target_fields: vec![
                FieldSpec::text("subreddit", "Subreddit", "rust", "Without the r/ prefix."),
                FieldSpec::text(
                    "flair_id",
                    "Flair ID",
                    "",
                    "Required by subreddits that enforce post flair.",
                )
                .optional(),
            ],
            notes: "Needs your own \"installed app\" — free, no review, no secret.",
        }
    }

    fn connect(&self, input: &ConnectInput) -> Result<Connected> {
        let app = input.app()?;
        // `duration=permanent` is what makes Reddit issue a refresh token at all.
        // Without it the account silently stops working an hour after connecting.
        let config = config_for(app);
        let (secret, granted_scopes) = oauth::authorize(&config)?;

        let me = fetch_me(&secret.access_token)?;
        Ok(Connected {
            remote_id: me.id,
            handle: format!("u/{}", me.name),
            display_name: Some(me.name),
            avatar_url: me.icon_img.map(|url| url.replace("&amp;", "&")),
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
                "Reddit's client id is missing. Re-add it in Settings → Platform apps.".into(),
            )
        })?;
        let refresh_token = secret.refresh_token.as_deref().ok_or_else(|| {
            AppError::Unauthorized(
                "This Reddit account has no refresh token — reconnect it to get one.".into(),
            )
        })?;
        Ok(Some(oauth::refresh(&config_for(app), refresh_token)?))
    }

    fn publish(&self, request: &PublishRequest<'_>) -> Result<Published> {
        let subreddit = request.option("subreddit").ok_or_else(|| {
            AppError::InvalidInput("Pick a subreddit for this Reddit destination.".into())
        })?;
        let subreddit = subreddit.trim_start_matches("r/").trim_start_matches('/');
        let title = request.title.unwrap_or_default();

        // A link submission when the post carries a URL and no body, a self post
        // otherwise: Reddit rejects `url` and `text` together.
        let is_link = request.link.is_some() && request.body.trim().is_empty();
        let mut form: Vec<(&str, &str)> = vec![
            ("api_type", "json"),
            ("sr", subreddit),
            ("title", title),
            ("kind", if is_link { "link" } else { "self" }),
            // Reddit's default is to send every reply to the account's inbox,
            // which for a scheduling tool is a mailbox nobody reads.
            ("sendreplies", "false"),
        ];
        if is_link {
            form.push(("url", request.link.unwrap_or_default()));
        } else {
            form.push(("text", request.body));
        }
        if let Some(flair) = request.option("flair_id") {
            form.push(("flair_id", flair));
        }

        let (status, body) = http::read_body(
            http::client()
                .post(format!("{API_BASE}/api/submit"))
                .bearer_auth(&request.secret.access_token)
                .form(&form)
                .send()?,
        );
        if !(200..300).contains(&status) {
            return Err(from_status(status, &body, "Reddit"));
        }
        parse_submit(&body)
    }
}

fn config_for(app: &AppCredentials) -> OAuthConfig<'_> {
    OAuthConfig {
        platform: PlatformId::Reddit,
        authorize_url: AUTHORIZE_URL.to_string(),
        token_url: TOKEN_URL.to_string(),
        client_id: &app.client_id,
        // An installed app genuinely has none; the Basic header still has to be
        // sent, as `client_id:` with an empty password.
        client_secret: None,
        scopes: SCOPES,
        extra_authorize_params: &[("duration", "permanent")],
        basic_auth: true,
    }
}

#[derive(Debug, Deserialize)]
struct Me {
    id: String,
    name: String,
    icon_img: Option<String>,
}

fn fetch_me(token: &str) -> Result<Me> {
    let (status, body) = http::read_body(
        http::client()
            .get(format!("{API_BASE}/api/v1/me"))
            .bearer_auth(token)
            .send()?,
    );
    if !(200..300).contains(&status) {
        return Err(from_status(status, &body, "Reddit"));
    }
    serde_json::from_str(&body)
        .map_err(|e| AppError::Platform(format!("Reddit returned an unreadable profile: {e}")))
}

/// `/api/submit` returns 200 whether it worked or not. The truth is in
/// `json.errors`, an array of `[CODE, human message, field]` triples.
fn parse_submit(body: &str) -> Result<Published> {
    let parsed: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| AppError::Platform(format!("Reddit returned an unreadable response: {e}")))?;

    if let Some(errors) = parsed
        .pointer("/json/errors")
        .and_then(serde_json::Value::as_array)
        && !errors.is_empty()
    {
        let message = errors
            .iter()
            .map(|entry| {
                let code = entry
                    .get(0)
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("ERROR");
                let detail = entry
                    .get(1)
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                format!("{code}: {detail}")
            })
            .collect::<Vec<_>>()
            .join("; ");
        // RATELIMIT is the one Reddit expects a client to wait out; everything
        // else here is the user's to fix (wrong subreddit, missing flair, a
        // title the sub rejects).
        return Err(if message.contains("RATELIMIT") {
            AppError::Platform(format!("Reddit is rate-limiting submissions. {message}"))
        } else {
            AppError::InvalidInput(format!("Reddit rejected the submission. {message}"))
        });
    }

    let data = parsed.pointer("/json/data");
    let url = data
        .and_then(|value| value.get("url"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let id = data
        .and_then(|value| value.get("name").or_else(|| value.get("id")))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            AppError::Platform("Reddit accepted the post but named no submission.".into())
        })?;
    Ok(Published {
        remote_id: id,
        remote_url: url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_two_hundred_carrying_errors_is_a_failure() {
        let body = r#"{"json":{"errors":[["NO_TEXT","we need something here","title"]]}}"#;
        let err = parse_submit(body).expect_err("errors in the body must not read as success");
        assert!(matches!(err, AppError::InvalidInput(_)), "{err}");
        assert!(err.to_string().contains("we need something here"));
    }

    #[test]
    fn a_rate_limit_error_stays_retryable() {
        let body =
            r#"{"json":{"errors":[["RATELIMIT","you are doing that too much","ratelimit"]]}}"#;
        let err = parse_submit(body).expect_err("rate limit");
        assert!(
            err.is_retryable(),
            "a rate limit must be retried, not surfaced as a dead end"
        );
    }

    #[test]
    fn a_clean_submission_yields_its_id_and_permalink() {
        let body = r#"{"json":{"errors":[],"data":{"url":"https://reddit.com/r/rust/comments/abc/x/","id":"abc","name":"t3_abc"}}}"#;
        let published = parse_submit(body).expect("success");
        assert_eq!(published.remote_id, "t3_abc");
        assert_eq!(
            published.remote_url.as_deref(),
            Some("https://reddit.com/r/rust/comments/abc/x/")
        );
    }

    #[test]
    fn a_response_with_neither_errors_nor_data_is_not_treated_as_success() {
        assert!(parse_submit(r#"{"json":{"errors":[]}}"#).is_err());
    }
}
