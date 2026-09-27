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

use super::{ContainerBuild, Containers, Grant, get_json, id_of, post_form, token_get};
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
pub(crate) const API_BASE: &str = "https://graph.instagram.com/v23.0";
const SCOPES: &str = "instagram_business_basic,instagram_business_content_publish";
const LABEL: &str = "Instagram";

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
                FieldSpec::text("insights_access", "Insights access", "no", "Also request the insights permission, so the stats screen can show what each post earned. Off by default: it widens the consent screen, and a connection made without it must be reconnected to gain it.")
                    .optional()
                    .choosing(&["no", "yes"]),
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
            &scopes_for(app),
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
            scopes: Some(scopes_for(app)),
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
        let containers = format!("{API_BASE}/{}/media", request.account.remote_id);

        // `platforms::validate` has already refused an empty one, but this is the
        // call that would otherwise build a meaningless container.
        if request.media.is_empty() {
            return Err(AppError::InvalidInput(
                "Instagram has no text-only post — attach an image or a video.".into(),
            ));
        }

        // Parked publicly inside the builders, so a resumed container does not
        // upload its media again.
        let host_all = || -> Result<Vec<(&MediaItem, String)>> {
            let host = webhost::require()?;
            request
                .media
                .iter()
                .map(|item| Ok((item, webhost::upload(&host, item)?.url)))
                .collect()
        };
        let children = || -> Result<Vec<String>> {
            host_all()?
                .iter()
                .map(|(item, url)| {
                    let mut form = single_media_form(item, url, token);
                    form.push(("is_carousel_item".into(), "true".into()));
                    create_container(&containers, &form)
                })
                .collect()
        };
        let container = |children: &[String]| -> Result<String> {
            let mut form = if children.is_empty() {
                let (item, url) = host_all()?.pop().ok_or_else(|| {
                    AppError::Internal("An Instagram post reached publishing with no media.".into())
                })?;
                single_media_form(item, &url, token)
            } else {
                vec![
                    ("media_type".to_string(), "CAROUSEL".to_string()),
                    ("children".to_string(), children.join(",")),
                    ("access_token".to_string(), token.clone()),
                ]
            };
            form.push(("caption".into(), request.body.to_string()));
            create_container(&containers, &form)
        };

        let media_id = containers_api().publish(
            request,
            &ContainerBuild {
                children: (request.media.len() > 1)
                    .then_some(&children as &dyn Fn() -> Result<Vec<String>>),
                container: &container,
            },
        )?;

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

/// Where Instagram keeps containers and how long one attempt waits on them.
/// Readiness is `status_code`, not `status`.
///
/// Meta's guidance is to check a container once a minute for up to five
/// minutes. One attempt looks every 5 s for 30 s; a video still transcoding
/// after that fails the attempt retryably, and the scheduler's next attempt — a
/// minute later at the soonest — checks the same container again.
fn containers_api() -> Containers {
    Containers {
        label: LABEL,
        api_base: API_BASE.to_string(),
        state_field: "status_code",
        detail_field: "status",
        publish_edge: "media_publish",
        tries: 6,
        interval: std::time::Duration::from_secs(5),
    }
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

/// The consent screen this connection asks for. Insights are opt-in, so the
/// default is exactly what posting needs — see [`crate::stats`] for what the
/// extra scope unlocks.
fn scopes_for(app: &AppCredentials) -> String {
    if app
        .extra("insights_access")
        .is_some_and(|value| value.eq_ignore_ascii_case("yes"))
    {
        format!("{SCOPES},instagram_business_manage_insights")
    } else {
        SCOPES.to_string()
    }
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

    fn app_with(insights: Option<&str>) -> AppCredentials {
        let mut extra = std::collections::HashMap::new();
        if let Some(value) = insights {
            extra.insert("insights_access".to_string(), value.to_string());
        }
        AppCredentials {
            client_id: "id".into(),
            client_secret: Some("s".into()),
            extra,
        }
    }

    #[test]
    fn insights_are_off_unless_asked_for() {
        // Someone who only schedules posts must never see an insights consent
        // screen.
        assert!(!scopes_for(&app_with(None)).contains("insights"));
        assert!(!scopes_for(&app_with(Some("no"))).contains("insights"));
    }

    #[test]
    fn insights_add_exactly_one_scope_when_opted_in() {
        let scopes = scopes_for(&app_with(Some("yes")));
        assert!(
            scopes.contains("instagram_business_manage_insights"),
            "{scopes}"
        );
        // The posting scopes survive — insights are additive, not a swap.
        assert!(scopes.contains(SCOPES), "{scopes}");
    }
}
