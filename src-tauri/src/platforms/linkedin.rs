//! `LinkedIn`, through the versioned `/rest/posts` API.
//!
//! Three things about `LinkedIn` are unlike every other adapter here:
//!
//!   * **Every request carries a `LinkedIn-Version` header** in `YYYYMM` form,
//!     and each version is sunset roughly a year after release. Pinning a
//!     constant in the binary means the app breaks on a date nobody wrote down,
//!     so the version is an editable field on the app credentials with a known
//!     good default.
//!   * **The created post's id is in a RESPONSE HEADER**, `x-restli-id`, not in
//!     the body — the body of a 201 is empty.
//!   * **PKCE is opt-in**, enabled by `LinkedIn` on request. A standard app is a
//!     confidential client, so the client secret field is offered and sent when
//!     present. The PKCE parameters go out either way; `LinkedIn` ignores them on
//!     an app that does not have the flow enabled.

use serde::Deserialize;
use serde_json::json;

use super::{
    AccountSecret, AppCredentials, AuthKind, ConnectInput, Connected, FieldSpec, Limits, MediaItem,
    Platform, PlatformId, PlatformInfo, PublishRequest, Published,
};
use crate::error::{AppError, Result, from_status};
use crate::http;
use crate::oauth::{self, OAuthConfig, REDIRECT_URI};

pub struct Linkedin;

const AUTHORIZE_URL: &str = "https://www.linkedin.com/oauth/v2/authorization";
const TOKEN_URL: &str = "https://www.linkedin.com/oauth/v2/accessToken";
const API_BASE: &str = "https://api.linkedin.com";
/// The version this adapter was written and checked against. Overridable per
/// install because `LinkedIn` sunsets versions on a rolling schedule.
const DEFAULT_API_VERSION: &str = "202606";
const SCOPES: &str = "openid profile w_member_social";

impl Platform for Linkedin {
    fn info(&self) -> PlatformInfo {
        PlatformInfo {
            id: PlatformId::Linkedin,
            name: "LinkedIn",
            auth: AuthKind::OAuth2,
            limits: Limits {
                max_chars: 3000,
                // One image per post. Several needs the separate MultiImage API,
                // which is a different content shape rather than more of this one.
                max_media: 1,
                supports_alt_text: true,
                requires_title: false,
            },
            connect_fields: Vec::new(),
            app_fields: vec![
                FieldSpec::text(
                    "client_id",
                    "Client ID",
                    "",
                    "From your app's Auth tab at linkedin.com/developers. Add the \
                     \"Share on LinkedIn\" and \"Sign In with LinkedIn using OpenID Connect\" \
                     products to it.",
                ),
                FieldSpec::secret(
                    "client_secret",
                    "Client secret",
                    "",
                    "Leave blank only if LinkedIn has enabled PKCE for your app.",
                )
                .optional(),
                FieldSpec::text(
                    "api_version",
                    "API version",
                    DEFAULT_API_VERSION,
                    "YYYYMM. LinkedIn retires versions about a year after release; \
                     raise this when yours is sunset.",
                )
                .optional(),
            ],
            setup_url: Some("https://www.linkedin.com/developers/apps"),
            redirect_uri: Some(REDIRECT_URI),
            target_fields: vec![
                FieldSpec::text(
                    "visibility",
                    "Visibility",
                    "PUBLIC",
                    "Who the post reaches.",
                )
                .optional()
                .choosing(&["PUBLIC", "CONNECTIONS"]),
            ],
            notes: "Needs your own developer app. Adding its \"Share on LinkedIn\" \
                    product is self-serve and approved instantly.",
        }
    }

    fn connect(&self, input: &ConnectInput) -> Result<Connected> {
        let app = input.app()?;
        let (mut secret, granted_scopes) = oauth::authorize(&config_for(app))?;
        let me = fetch_userinfo(&secret.access_token)?;

        // The version the account was connected under travels with the account,
        // so changing the default later cannot silently alter a live connection.
        secret.extra = json!({ "api_version": api_version(app) });

        Ok(Connected {
            // `sub` is the member id; the author URN is built from it at publish.
            remote_id: me.sub,
            handle: me.name.clone().unwrap_or_else(|| "LinkedIn member".into()),
            display_name: me.name,
            avatar_url: me.picture,
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
        let Some(refresh_token) = secret.refresh_token.as_deref() else {
            // LinkedIn only issues refresh tokens to approved apps. Without one
            // the 60-day access token simply runs out, and the honest thing is to
            // say so rather than to fail mid-publish with a bare 401.
            return Err(AppError::Unauthorized(
                "This LinkedIn access token has expired and the app was not issued a \
                 refresh token. Reconnect the account."
                    .into(),
            ));
        };
        let app = app.ok_or_else(|| {
            AppError::Unauthorized(
                "LinkedIn's client id is missing. Re-add it in Settings → Platform apps.".into(),
            )
        })?;
        let mut refreshed = oauth::refresh(&config_for(app), refresh_token)?;
        refreshed.extra = secret.extra.clone();
        Ok(Some(refreshed))
    }

    fn publish(&self, request: &PublishRequest<'_>) -> Result<Published> {
        let token = &request.secret.access_token;
        let version = request
            .secret
            .extra_str("api_version")
            .unwrap_or(DEFAULT_API_VERSION)
            .to_string();
        let author = format!("urn:li:person:{}", request.account.remote_id);

        let mut payload = json!({
            "author": author,
            "commentary": escape_commentary(request.body),
            "visibility": request.option("visibility").unwrap_or("PUBLIC"),
            "distribution": {
                "feedDistribution": "MAIN_FEED",
                "targetEntities": [],
                "thirdPartyDistributionChannels": [],
            },
            "lifecycleState": "PUBLISHED",
            "isReshareDisabledByAuthor": false,
        });

        if let Some(item) = request.media.first() {
            let image_urn = upload_image(token, &version, &author, item)?;
            let mut media = json!({ "id": image_urn });
            if let Some(alt) = &item.alt_text
                && !alt.trim().is_empty()
            {
                media["altText"] = json!(alt);
            }
            payload["content"] = json!({ "media": media });
        } else if let Some(link) = request.link {
            // With no image, a URL becomes a real article card rather than a bare
            // string in the text — LinkedIn does not scrape links itself.
            payload["content"] = json!({
                "article": {
                    "source": link,
                    "title": request.title.unwrap_or(link),
                }
            });
        }

        let response = http::client()
            .post(format!("{API_BASE}/rest/posts"))
            .bearer_auth(token)
            .header("X-Restli-Protocol-Version", "2.0.0")
            .header("LinkedIn-Version", &version)
            .json(&payload)
            .send()?;

        // The id arrives in a header and the 201 body is empty, so the header is
        // read BEFORE the body is consumed.
        let post_urn = response
            .headers()
            .get("x-restli-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let (status, body) = http::read_body(response);

        if !(200..300).contains(&status) {
            return Err(map_error(status, &body, &version));
        }
        let urn = post_urn.ok_or_else(|| {
            AppError::Platform("LinkedIn accepted the post but returned no id header.".into())
        })?;

        Ok(Published {
            remote_url: Some(format!("https://www.linkedin.com/feed/update/{urn}/")),
            remote_id: urn,
        })
    }
}

fn config_for(app: &AppCredentials) -> OAuthConfig<'_> {
    OAuthConfig {
        platform: PlatformId::Linkedin,
        authorize_url: AUTHORIZE_URL.to_string(),
        token_url: TOKEN_URL.to_string(),
        client_id: &app.client_id,
        client_secret: app
            .client_secret
            .as_deref()
            .filter(|value| !value.is_empty()),
        scopes: SCOPES,
        extra_authorize_params: &[],
        basic_auth: false,
    }
}

/// The API version this connection should use: what the user typed on the app
/// credentials when it looks like `YYYYMM`, otherwise the checked default. A
/// malformed value falls back rather than being sent — `LinkedIn` rejects the
/// whole request on a bad version header, and every call would fail the same way.
fn api_version(app: &AppCredentials) -> String {
    app.extra("api_version")
        .filter(|value| value.len() == 6 && value.chars().all(|ch| ch.is_ascii_digit()))
        .unwrap_or(DEFAULT_API_VERSION)
        .to_string()
}

#[derive(Debug, Deserialize)]
struct UserInfo {
    sub: String,
    name: Option<String>,
    picture: Option<String>,
}

fn fetch_userinfo(token: &str) -> Result<UserInfo> {
    let (status, body) = http::read_body(
        http::client()
            .get(format!("{API_BASE}/v2/userinfo"))
            .bearer_auth(token)
            .send()?,
    );
    if !(200..300).contains(&status) {
        return Err(from_status(status, &body, "LinkedIn"));
    }
    serde_json::from_str(&body)
        .map_err(|e| AppError::Platform(format!("LinkedIn returned an unreadable profile: {e}")))
}

/// Images API: ask for an upload lease, PUT the bytes at the URL it hands back,
/// then reference the returned image URN from the post.
fn upload_image(token: &str, version: &str, owner: &str, item: &MediaItem) -> Result<String> {
    let (status, body) = http::read_body(
        http::client()
            .post(format!("{API_BASE}/rest/images?action=initializeUpload"))
            .bearer_auth(token)
            .header("X-Restli-Protocol-Version", "2.0.0")
            .header("LinkedIn-Version", version)
            .json(&json!({ "initializeUploadRequest": { "owner": owner } }))
            .send()?,
    );
    if !(200..300).contains(&status) {
        return Err(map_error(status, &body, version));
    }

    let parsed: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
        AppError::Platform(format!("LinkedIn returned an unreadable upload lease: {e}"))
    })?;
    let upload_url = parsed
        .pointer("/value/uploadUrl")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| AppError::Platform("LinkedIn's upload lease carried no URL.".into()))?;
    let image_urn = parsed
        .pointer("/value/image")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| AppError::Platform("LinkedIn's upload lease carried no image URN.".into()))?
        .to_owned();

    let (upload_status, upload_body) = http::read_body(
        http::client()
            .put(upload_url)
            .bearer_auth(token)
            .header(reqwest::header::CONTENT_TYPE, &item.mime)
            .body(item.bytes.clone())
            .send()?,
    );
    if !(200..300).contains(&upload_status) {
        return Err(from_status(upload_status, &upload_body, "LinkedIn"));
    }
    Ok(image_urn)
}

/// `LinkedIn`'s `commentary` is "little text": `(`, `)`, `[`, `]`, `{`, `}`, `<`,
/// `>`, `@`, `|`, `~`, `_` and `*` are markup and must be escaped with a
/// backslash, or the post is rejected or renders wrong. `\` itself goes first,
/// otherwise it would double-escape everything after it.
///
/// `#` is deliberately NOT in the set. `LinkedIn`'s own examples post
/// `"Follow best practices #coding"` raw and it renders as a hashtag; escaping
/// it would turn every tag in every post into literal text, which for a social
/// scheduler is a broken feature rather than a rendering nit.
fn escape_commentary(text: &str) -> String {
    const SPECIAL: &[char] = &[
        '\\', '(', ')', '[', ']', '{', '}', '<', '>', '@', '|', '~', '_', '*',
    ];
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if SPECIAL.contains(&ch) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// A sunset API version comes back as a 426 or as a 400 naming the version, and
/// "raise the version in Settings" is the only fix — retrying forever is not.
fn map_error(status: u16, body: &str, version: &str) -> AppError {
    let lowered = body.to_ascii_lowercase();
    if status == 426
        || lowered.contains("unsupported version")
        || lowered.contains("version is not supported")
    {
        return AppError::InvalidInput(format!(
            "LinkedIn no longer accepts API version {version}. Update it in \
             Settings → Platform apps and reconnect the account."
        ));
    }
    from_status(status, body, "LinkedIn")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn little_text_specials_are_escaped() {
        assert_eq!(escape_commentary("a (b) [c]"), r"a \(b\) \[c\]");
        assert_eq!(escape_commentary("50% off*"), r"50% off\*");
    }

    #[test]
    fn a_backslash_is_escaped_before_everything_else() {
        // Escaping `(` first and `\` second would turn `\(` into `\\\(`, which
        // renders as a literal backslash followed by an escaped paren.
        assert_eq!(escape_commentary(r"\("), r"\\\(");
    }

    #[test]
    fn plain_text_passes_through_untouched() {
        assert_eq!(escape_commentary("hello world"), "hello world");
    }

    #[test]
    fn hashtags_survive_so_they_render_as_hashtags() {
        assert_eq!(
            escape_commentary("shipping today #rust"),
            "shipping today #rust"
        );
    }

    fn app(version: Option<&str>) -> AppCredentials {
        let mut extra = std::collections::HashMap::new();
        if let Some(value) = version {
            extra.insert("api_version".to_string(), value.to_string());
        }
        AppCredentials {
            client_id: "id".into(),
            client_secret: None,
            extra,
        }
    }

    #[test]
    fn a_valid_version_override_wins_over_the_default() {
        assert_eq!(api_version(&app(Some("202608"))), "202608");
    }

    #[test]
    fn a_malformed_version_falls_back_instead_of_breaking_every_call() {
        for bad in ["2026-06", "june", "20260", ""] {
            assert_eq!(api_version(&app(Some(bad))), DEFAULT_API_VERSION, "{bad}");
        }
        assert_eq!(api_version(&app(None)), DEFAULT_API_VERSION);
    }

    #[test]
    fn a_sunset_version_is_a_terminal_error_with_the_fix_in_it() {
        let err = map_error(426, "unsupported version", "202401");
        assert!(
            !err.is_retryable(),
            "retrying a sunset version never succeeds"
        );
        assert!(err.to_string().contains("202401"));
    }
}
