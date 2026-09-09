//! Instagram, through the Instagram API with Instagram Login on
//! `graph.instagram.com`.
//!
//! Same two-step container flow as Threads, and the same "Meta fetches the media
//! itself" constraint — but with one difference that shapes the whole adapter:
//!
//! **Instagram has no text-only post.** There is no `media_type=TEXT`. A caption
//! is a property of an image or a video, never a post by itself, so an
//! attachment is REQUIRED rather than optional. That is what `requires_media` on
//! [`Limits`] exists for: the composer, the scheduler and the MCP door all refuse
//! a caption-only Instagram destination up front, instead of building a container
//! that Meta rejects.
//!
//! Sign-in is Instagram's own — `instagram.com/oauth/authorize` against an
//! Instagram app ID, not a Facebook Login flow — so a professional (Business or
//! Creator) account connects without a Facebook Page in the middle.

use serde_json::json;

use super::{Grant, get_json, id_of, post_form, token_get};
use crate::error::{AppError, Result};
use crate::platforms::{
    AccountSecret, AppCredentials, AuthKind, ConnectInput, Connected, FieldSpec, Limits, MediaItem,
    Platform, PlatformId, PlatformInfo, PublishRequest, Published,
};
use crate::webhost;

pub struct Instagram;

const AUTHORIZE_URL: &str = "https://www.instagram.com/oauth/authorize";
const TOKEN_URL: &str = "https://api.instagram.com/oauth/access_token";
const EXCHANGE_URL: &str = "https://graph.instagram.com/access_token";
const REFRESH_URL: &str = "https://graph.instagram.com/refresh_access_token";
const API_BASE: &str = "https://graph.instagram.com/v23.0";
const SCOPES: &str = "instagram_business_basic,instagram_business_content_publish";
const LABEL: &str = "Instagram";

/// Instagram transcodes video off the request path and is slower at it than
/// Threads. Twenty tries three seconds apart is a minute, after which the target
/// fails retryably and the scheduler comes back to it.
const POLL_TRIES: usize = 20;
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3);

impl Platform for Instagram {
    fn info(&self) -> PlatformInfo {
        PlatformInfo {
            id: PlatformId::Instagram,
            name: "Instagram",
            auth: AuthKind::OAuth2,
            limits: Limits {
                max_chars: 2200,
                max_media: 10,
                // The caption's `alt_text` is not offered on the container for a
                // feed post, so promising it here would be a lie in the composer.
                supports_alt_text: false,
                requires_title: false,
                requires_media: true,
            },
            connect_fields: Vec::new(),
            app_fields: vec![
                FieldSpec::text(
                    "client_id",
                    "Instagram app ID",
                    "",
                    "From your app's \"Manage messaging & content on Instagram\" use case at \
                     developers.facebook.com — the Instagram app ID, not the Meta app ID.",
                ),
                FieldSpec::secret(
                    "client_secret",
                    "Instagram app secret",
                    "",
                    "Required — Meta has no public-client flow for Instagram.",
                ),
            ],
            setup_url: Some("https://developers.facebook.com/apps"),
            // Not a constant, and deliberately not read from the credential
            // store here: `info()` runs on every keystroke in the composer. The
            // `get_web_host` command reports it instead.
            redirect_uri: None,
            target_fields: Vec::new(),
            notes: "Needs a professional (Business or Creator) Instagram account and your own \
                    Meta app, registered with the HTTPS redirect from Settings → Web \
                    deployment. Every Instagram post requires an image or video.",
        }
    }

    fn connect(&self, input: &ConnectInput) -> Result<Connected> {
        let app = input.app()?;
        let secret_key = client_secret(app)?;
        let host = webhost::require()?;
        let redirect = host.redirect_uri();

        let handoff = crate::oauth::handoff(
            PlatformId::Instagram,
            AUTHORIZE_URL,
            &app.client_id,
            &redirect,
            SCOPES,
            &[],
            false,
        )?;

        let short = post_form(
            TOKEN_URL,
            &[
                ("client_id", &app.client_id),
                ("client_secret", secret_key),
                ("grant_type", "authorization_code"),
                ("redirect_uri", &redirect),
                ("code", &handoff.code),
            ],
            LABEL,
        )?;
        let short_token = short
            .get("access_token")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| AppError::Platform(format!("{LABEL} returned no access token.")))?;

        let long = token_get(
            EXCHANGE_URL,
            &[
                ("grant_type", "ig_exchange_token"),
                ("client_secret", secret_key),
                ("access_token", short_token),
            ],
            LABEL,
        )?;

        let me = get_json(
            &format!("{API_BASE}/me"),
            &[
                ("fields", "user_id,username,name,profile_picture_url"),
                ("access_token", &long.access_token),
            ],
            LABEL,
        )?;
        // This endpoint answers with `user_id`, not `id` — the id every
        // publishing call is then addressed to.
        let user_id = me
            .get("user_id")
            .and_then(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| value.as_i64().map(|n| n.to_string()))
            })
            .ok_or_else(|| AppError::Platform(format!("{LABEL} returned no user id.")))?;
        let username = me
            .get("username")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Instagram account");

        Ok(Connected {
            remote_id: user_id,
            handle: format!("@{username}"),
            display_name: me
                .get("name")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            avatar_url: me
                .get("profile_picture_url")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            instance: None,
            scopes: Some(SCOPES.to_string()),
            char_limit: None,
            secret: AccountSecret {
                expires_at: long.expires_at(),
                access_token: long.access_token,
                refresh_token: None,
                extra: json!({}),
            },
        })
    }

    fn refresh(
        &self,
        _account: &crate::db::Account,
        secret: &AccountSecret,
        _app: Option<&AppCredentials>,
    ) -> Result<Option<AccountSecret>> {
        if !crate::oauth::needs_refresh(secret.expires_at.as_deref()) {
            return Ok(None);
        }
        // Instagram refuses to extend a long-lived token less than 24 hours old.
        // That is not a failure worth propagating — the token is good for another
        // 59 days, so the honest answer is "nothing changed".
        let grant: Grant = match token_get(
            REFRESH_URL,
            &[
                ("grant_type", "ig_refresh_token"),
                ("access_token", &secret.access_token),
            ],
            LABEL,
        ) {
            Ok(grant) => grant,
            Err(err) if is_too_soon(&err) => return Ok(None),
            Err(err) => return Err(err),
        };
        Ok(Some(AccountSecret {
            expires_at: grant.expires_at(),
            access_token: grant.access_token,
            refresh_token: None,
            extra: secret.extra.clone(),
        }))
    }

    fn publish(&self, request: &PublishRequest<'_>) -> Result<Published> {
        let token = &request.secret.access_token;
        let user = &request.account.remote_id;
        let containers = format!("{API_BASE}/{user}/media");

        // `platforms::validate` has already refused an empty one, but this is the
        // call that would otherwise build a meaningless container.
        if request.media.is_empty() {
            return Err(AppError::InvalidInput(
                "Instagram has no text-only post — attach an image or a video.".into(),
            ));
        }

        let host = webhost::require()?;
        let hosted = request
            .media
            .iter()
            .map(|item| Ok((item, webhost::upload(&host, item)?.url)))
            .collect::<Result<Vec<_>>>()?;

        let creation_id = if hosted.len() == 1 {
            let (item, url) = &hosted[0];
            let mut form = single_media_form(item, url, token);
            form.push(("caption".into(), request.body.to_string()));
            let id = create_container(&containers, &form)?;
            await_container(&id, token)?;
            id
        } else {
            let mut children = Vec::with_capacity(hosted.len());
            for (item, url) in &hosted {
                let mut form = single_media_form(item, url, token);
                form.push(("is_carousel_item".into(), "true".into()));
                let id = create_container(&containers, &form)?;
                await_container(&id, token)?;
                children.push(id);
            }
            let form = vec![
                ("media_type".to_string(), "CAROUSEL".to_string()),
                ("children".to_string(), children.join(",")),
                ("caption".to_string(), request.body.to_string()),
                ("access_token".to_string(), token.clone()),
            ];
            let id = create_container(&containers, &form)?;
            await_container(&id, token)?;
            id
        };

        let published = post_form(
            &format!("{API_BASE}/{user}/media_publish"),
            &[
                ("creation_id", creation_id.as_str()),
                ("access_token", token.as_str()),
            ],
            LABEL,
        )?;
        let media_id = id_of(&published, LABEL, "published post")?;

        Ok(Published {
            remote_url: permalink(&media_id, token),
            remote_id: media_id,
        })
    }
}

/// A video item is posted as a REEL — since 2024 that is what a feed video IS on
/// Instagram, and `media_type=VIDEO` is the deprecated spelling of it.
fn single_media_form(item: &MediaItem, url: &str, token: &str) -> Vec<(String, String)> {
    let is_video = item.mime.starts_with("video/");
    let mut form = vec![("access_token".to_string(), token.to_string())];
    if is_video {
        form.push(("media_type".into(), "REELS".into()));
        form.push(("video_url".into(), url.to_string()));
    } else {
        form.push(("image_url".into(), url.to_string()));
    }
    form
}

fn create_container(url: &str, form: &[(String, String)]) -> Result<String> {
    let pairs: Vec<(&str, &str)> = form
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    id_of(&post_form(url, &pairs, LABEL)?, LABEL, "media container")
}

/// Instagram reports container readiness as `status_code`, not `status`.
fn await_container(container_id: &str, token: &str) -> Result<()> {
    for attempt in 0..POLL_TRIES {
        let status = get_json(
            &format!("{API_BASE}/{container_id}"),
            &[("fields", "status_code,status"), ("access_token", token)],
            LABEL,
        )?;
        let state = status
            .get("status_code")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("IN_PROGRESS");
        let detail = status
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("no reason given");

        match state {
            "FINISHED" | "PUBLISHED" => return Ok(()),
            "ERROR" => {
                return Err(AppError::InvalidInput(format!(
                    "{LABEL} could not process the attachment: {detail}"
                )));
            }
            "EXPIRED" => {
                return Err(AppError::InvalidInput(format!(
                    "The {LABEL} media container expired before it was published: {detail}"
                )));
            }
            _ if attempt + 1 < POLL_TRIES => std::thread::sleep(POLL_INTERVAL),
            _ => {}
        }
    }
    Err(AppError::Platform(format!(
        "{LABEL} is still processing the attachment after {}s.",
        POLL_TRIES as u64 * POLL_INTERVAL.as_secs()
    )))
}

fn permalink(media_id: &str, token: &str) -> Option<String> {
    get_json(
        &format!("{API_BASE}/{media_id}"),
        &[("fields", "permalink"), ("access_token", token)],
        LABEL,
    )
    .ok()?
    .get("permalink")
    .and_then(serde_json::Value::as_str)
    .map(str::to_owned)
}

/// Instagram's "not old enough to refresh" rejection, which is a no-op rather
/// than a failure.
fn is_too_soon(err: &AppError) -> bool {
    let text = err.to_string().to_ascii_lowercase();
    text.contains("24 hours") || text.contains("less than 24")
}

fn client_secret(app: &AppCredentials) -> Result<&str> {
    app.client_secret
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AppError::InvalidInput(
                "Instagram needs your app secret as well as the app ID — Meta has no \
                 public-client flow. Add it in Settings → Platform apps."
                    .into(),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instagram_declares_that_it_cannot_post_without_media() {
        // The one property that separates it from every other adapter here.
        assert!(Instagram.info().limits.requires_media);
    }

    #[test]
    fn a_video_is_posted_as_a_reel() {
        let item = MediaItem {
            bytes: Vec::new(),
            mime: "video/mp4".into(),
            alt_text: None,
        };
        let form = single_media_form(&item, "https://x/y.mp4", "tok");
        assert!(form.iter().any(|(k, v)| k == "media_type" && v == "REELS"));
        assert!(form.iter().any(|(k, _)| k == "video_url"));
    }

    #[test]
    fn an_image_container_carries_no_media_type() {
        // Instagram infers IMAGE, and sending the parameter is an error.
        let item = MediaItem {
            bytes: Vec::new(),
            mime: "image/jpeg".into(),
            alt_text: None,
        };
        let form = single_media_form(&item, "https://x/y.jpg", "tok");
        assert!(!form.iter().any(|(k, _)| k == "media_type"));
        assert!(form.iter().any(|(k, _)| k == "image_url"));
    }

    #[test]
    fn a_refresh_that_is_merely_too_soon_is_not_a_failure() {
        let err = AppError::InvalidInput(
            "Instagram rejected the request: The access token must be at least 24 hours old".into(),
        );
        assert!(is_too_soon(&err));
        assert!(!is_too_soon(&AppError::Unauthorized("revoked".into())));
    }
}
