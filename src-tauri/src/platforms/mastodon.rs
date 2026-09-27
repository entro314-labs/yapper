//! Mastodon, against whichever instance the account lives on.
//!
//! The only OAuth platform here that needs nothing from the user beyond the
//! instance host: Mastodon lets a client register itself at `POST /api/v1/apps`,
//! so Windbag mints its own client id per instance on first connect and stores it
//! in the OS credential store keyed by that host. An app registration is only
//! valid on the server that issued it, which is why the key carries the host.
//!
//! Instances set their own character limit — 500 is only the default — so the
//! real one is read at connect time and stored on the account.

use serde::Deserialize;
use serde_json::json;

use super::{
    AccountSecret, AppCredentials, AuthKind, ConnectInput, Connected, FieldSpec, Limits, Platform,
    PlatformId, PlatformInfo, PublishRequest, Published,
};
use crate::error::{AppError, Result, after_send, from_status, unreadable_after_send};
use crate::oauth::{self, OAuthConfig, REDIRECT_URI};
use crate::platforms::{MB, MediaRule};
use crate::{http, secrets};

pub struct Mastodon;

const SCOPES: &str = "read:accounts write:statuses write:media";

/// Mastodon's defaults: images up to 16 MB, one video up to 99 MB
/// (<https://docs.joinmastodon.org/user/posting/#attachments>). A server can
/// change both in its own configuration; these are what almost all of them
/// run. A video beside images is refused by the server
/// (`media_attachments.validations.images_and_video` in
/// <https://github.com/mastodon/mastodon/blob/main/app/services/post_status_service.rb>).
/// An animated GIF is transcoded to a looping video, so it gets the video
/// size limit (`larger_media_format?` in
/// <https://github.com/mastodon/mastodon/blob/main/app/models/media_attachment.rb>)
/// but may still sit beside images.
const MEDIA: &[MediaRule] = &[
    MediaRule::up_to("image/png", 16 * MB),
    MediaRule::up_to("image/jpeg", 16 * MB),
    MediaRule::up_to("image/gif", 99 * MB),
    MediaRule::up_to("image/webp", 16 * MB),
    MediaRule::up_to("video/mp4", 99 * MB).alone(),
];

impl Platform for Mastodon {
    fn info(&self) -> PlatformInfo {
        PlatformInfo {
            id: PlatformId::Mastodon,
            name: "Mastodon",
            auth: AuthKind::OAuth2,
            limits: Limits {
                max_chars: 500,
                max_media: 4,
                accepts: MEDIA,
                supports_alt_text: true,
                requires_title: false,
                requires_media: false,
                link_is_content: false,
            },
            connect_fields: vec![FieldSpec::text(
                "instance",
                "Instance",
                "mastodon.social",
                "The server your account is on. Windbag registers itself there automatically.",
            )],
            app_fields: Vec::new(),
            setup_url: None,
            redirect_uri: Some(REDIRECT_URI),
            target_fields: vec![
                FieldSpec::text(
                    "visibility",
                    "Visibility",
                    "public",
                    "Who sees it on your instance.",
                )
                .optional()
                .choosing(&["public", "unlisted", "private", "direct"]),
                FieldSpec::text(
                    "spoiler_text",
                    "Content warning",
                    "",
                    "Shown in place of the post until the reader expands it. Counts toward \
                     the character limit.",
                )
                .optional()
                .counted(),
            ],
            notes: "No developer app needed — Windbag registers itself with your instance.",
        }
    }

    fn connect(&self, input: &ConnectInput) -> Result<Connected> {
        let instance = normalize_instance(input.field("instance")?);

        // Reuse a registration this instance already issued; only register when
        // there is none, so reconnecting does not litter the server with apps.
        let app = if let Some(existing) =
            secrets::load_app_credentials(PlatformId::Mastodon, Some(&instance))?
        {
            existing
        } else {
            let registered = register_app(&instance)?;
            secrets::store_app_credentials(PlatformId::Mastodon, Some(&instance), &registered)?;
            registered
        };

        let config = config_for(&instance, &app);
        let (secret, granted_scopes) = oauth::authorize(&config)?;

        let profile = verify_credentials(&instance, &secret.access_token)?;
        let char_limit = instance_char_limit(&instance);

        Ok(Connected {
            remote_id: profile.id,
            handle: format!("@{}@{}", profile.username, host_of(&instance)),
            display_name: Some(profile.display_name).filter(|name| !name.is_empty()),
            avatar_url: profile.avatar,
            instance: Some(instance),
            scopes: granted_scopes.or_else(|| Some(SCOPES.to_string())),
            char_limit,
            secret: AccountSecret {
                extra: json!({}),
                ..secret
            },
        })
    }

    fn publish(&self, request: &PublishRequest<'_>) -> Result<Published> {
        let instance = request.account.instance.as_deref().ok_or_else(|| {
            AppError::Internal("This Mastodon account has no instance recorded.".into())
        })?;

        let media_ids = request
            .media
            .iter()
            .map(|item| upload_media(instance, &request.secret.access_token, item))
            .collect::<Result<Vec<_>>>()?;

        let mut payload = json!({ "status": request.body });
        if let Some(visibility) = request.option("visibility") {
            payload["visibility"] = json!(visibility);
        }
        if let Some(spoiler) = request.option("spoiler_text") {
            payload["spoiler_text"] = json!(spoiler);
        }
        if !media_ids.is_empty() {
            payload["media_ids"] = json!(media_ids);
        }

        let (status, body) = http::read_body(
            http::client()
                .post(format!("{instance}/api/v1/statuses"))
                .bearer_auth(&request.secret.access_token)
                // A retry after a timeout must not produce a second post. Mastodon
                // honours this header for up to an hour and answers a REUSED key with
                // the original status — so it has to name this destination and no
                // other. Keyed on the account, two different posts to the same
                // account inside that window would collapse into one, and the
                // second would report the first one's id as its own.
                .header(
                    "Idempotency-Key",
                    format!("windbag-target-{}", request.target_id),
                )
                .json(&payload)
                .send()
                // The key only holds for about an hour, less than the retry
                // ladder spans, so a lost answer is still the user's to check.
                .map_err(after_send)?,
        );
        if !(200..300).contains(&status) {
            return Err(from_status(status, &body, "Mastodon"));
        }

        let created: Status =
            serde_json::from_str(&body).map_err(|e| unreadable_after_send("Mastodon", e))?;
        Ok(Published {
            remote_id: created.id,
            remote_url: created.url,
        })
    }
}

#[derive(Debug, Deserialize)]
struct Registered {
    client_id: String,
    client_secret: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Attachment {
    id: String,
}

#[derive(Debug, Deserialize)]
struct Status {
    id: String,
    url: Option<String>,
}

/// Mastodon's OAuth endpoints live on the instance, so they are derived per
/// connect rather than being constants like every other adapter's.
fn config_for<'a>(instance: &str, app: &'a AppCredentials) -> OAuthConfig<'a> {
    OAuthConfig {
        platform: PlatformId::Mastodon,
        authorize_url: format!("{instance}/oauth/authorize"),
        token_url: format!("{instance}/oauth/token"),
        client_id: &app.client_id,
        client_secret: app.client_secret.as_deref(),
        scopes: SCOPES,
        extra_authorize_params: &[],
        basic_auth: false,
    }
}

fn register_app(instance: &str) -> Result<AppCredentials> {
    let (status, body) = http::read_body(
        http::client()
            .post(format!("{instance}/api/v1/apps"))
            .json(&json!({
                "client_name": "Windbag",
                "redirect_uris": REDIRECT_URI,
                "scopes": SCOPES,
                "website": "https://github.com/entro314-labs/yapper",
            }))
            .send()?,
    );
    if !(200..300).contains(&status) {
        return Err(match status {
            404 => {
                AppError::InvalidInput(format!("{instance} does not look like a Mastodon server."))
            }
            _ => from_status(status, &body, "Mastodon"),
        });
    }

    let registered: Registered = serde_json::from_str(&body).map_err(|e| {
        AppError::Platform(format!(
            "{instance} returned an unreadable app registration: {e}"
        ))
    })?;
    Ok(AppCredentials {
        client_id: registered.client_id,
        client_secret: registered.client_secret,
        extra: std::collections::HashMap::new(),
    })
}

#[derive(Debug, Deserialize)]
struct CredentialAccount {
    id: String,
    username: String,
    display_name: String,
    avatar: Option<String>,
}

fn verify_credentials(instance: &str, token: &str) -> Result<CredentialAccount> {
    let (status, body) = http::read_body(
        http::client()
            .get(format!("{instance}/api/v1/accounts/verify_credentials"))
            .bearer_auth(token)
            .send()?,
    );
    if !(200..300).contains(&status) {
        return Err(from_status(status, &body, "Mastodon"));
    }
    serde_json::from_str(&body)
        .map_err(|e| AppError::Platform(format!("Mastodon returned an unreadable profile: {e}")))
}

/// The instance's own `max_characters`. Best effort: a server that does not
/// publish it (or an older API shape) simply leaves the platform default in
/// place, which is what the field being `Option` is for.
fn instance_char_limit(instance: &str) -> Option<usize> {
    let response = http::client()
        .get(format!("{instance}/api/v2/instance"))
        .send()
        .ok()?;
    let (status, body) = http::read_body(response);
    if !(200..300).contains(&status) {
        return None;
    }
    let parsed: serde_json::Value = serde_json::from_str(&body).ok()?;
    parsed
        .pointer("/configuration/statuses/max_characters")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| *value > 0)
}

fn upload_media(instance: &str, token: &str, item: &super::MediaItem) -> Result<String> {
    let part = reqwest::blocking::multipart::Part::bytes(item.bytes.clone())
        .file_name("upload")
        .mime_str(&item.mime)
        .map_err(|e| AppError::InvalidInput(format!("Unsupported attachment type: {e}")))?;
    let mut form = reqwest::blocking::multipart::Form::new().part("file", part);
    if let Some(alt) = &item.alt_text {
        form = form.text("description", alt.clone());
    }

    let (status, body) = http::read_body(
        http::client()
            .post(format!("{instance}/api/v2/media"))
            .bearer_auth(token)
            .multipart(form)
            .send()?,
    );
    // 202 means the server took the file and is still processing it. The id is
    // already usable in a status, so this is a success, not a wait.
    if !(200..300).contains(&status) {
        return Err(from_status(status, &body, "Mastodon"));
    }

    let attachment: Attachment = serde_json::from_str(&body).map_err(|e| {
        AppError::Platform(format!(
            "Mastodon returned an unreadable upload response: {e}"
        ))
    })?;
    Ok(attachment.id)
}

/// `mastodon.social`, `https://mastodon.social/` and `HTTPS://Mastodon.Social`
/// all name the same server; the stored form is the canonical origin.
fn normalize_instance(value: &str) -> String {
    // Lowercased BEFORE the scheme is stripped: `strip_prefix` is case-sensitive,
    // so a pasted `HTTPS://` would survive and produce `https://HTTPS://host`.
    let lowered = value.trim().trim_end_matches('/').to_ascii_lowercase();
    let host = lowered
        .strip_prefix("https://")
        .or_else(|| lowered.strip_prefix("http://"))
        .unwrap_or(&lowered);
    format!("https://{host}")
}

fn host_of(instance: &str) -> &str {
    instance
        .trim_start_matches("https://")
        .trim_end_matches('/')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instances_normalize_to_one_canonical_origin() {
        for input in [
            "mastodon.social",
            "https://mastodon.social",
            "HTTPS://Mastodon.Social/",
        ] {
            assert_eq!(normalize_instance(input), "https://mastodon.social");
        }
    }

    #[test]
    fn the_host_is_what_a_handle_is_built_from() {
        assert_eq!(host_of("https://mastodon.social"), "mastodon.social");
    }
}
