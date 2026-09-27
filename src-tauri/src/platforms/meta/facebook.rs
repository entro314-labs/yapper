//! A Facebook Page, through the Pages API on `graph.facebook.com`.
//!
//! The odd one out of the three, in the way that matters most: **a Page accepts
//! bytes.** `POST /{page-id}/photos` takes a multipart `source`, so an
//! attachment goes straight from disk to Meta and the companion deployment is
//! needed only for the HTTPS redirect, never for media.
//!
//! Two other things shape this adapter:
//!
//!   * **You post as the Page, not as yourself.** Signing in yields a USER
//!     token, which is then traded at `/me/accounts` for a PAGE token — a
//!     different credential, and the one every publish uses. A Page token
//!     derived from a long-lived user token does not expire on a clock, which is
//!     why `refresh` here has nothing to do.
//!   * **Several photos is not one call.** One photo posts directly; several are
//!     uploaded unpublished, then referenced from a single `/feed` post as
//!     `attached_media`, which is what makes them one post rather than several.

use serde_json::json;

use super::{GRAPH_VERSION, get_json, id_of, map_error, token_get};
use crate::error::{AppError, Result};
use crate::http;
use crate::media;
use crate::platforms::{
    AccountSecret, AppCredentials, AuthKind, ConnectInput, Connected, FieldSpec, Limits, MediaItem,
    Platform, PlatformId, PlatformInfo, PublishRequest, Published,
};
use crate::platforms::{MB, MediaRule};
use crate::webhost;

pub struct Facebook;

const LABEL: &str = "Facebook Page";
const SCOPES: &str = "pages_show_list,pages_manage_posts,pages_read_engagement,business_management";

fn authorize_url() -> String {
    format!("https://www.facebook.com/{GRAPH_VERSION}/dialog/oauth")
}

pub(crate) fn api_base() -> String {
    format!("https://graph.facebook.com/{GRAPH_VERSION}")
}

/// Page photos take JPEG, PNG and GIF up to 10 MB
/// (<https://developers.facebook.com/docs/graph-api/reference/page/photos/>).
/// A video is its own endpoint and cannot share a post with photos, so it goes
/// alone; Meta documents no cap for a single-request upload below the app's
/// own ceiling.
const MEDIA: &[MediaRule] = &[
    MediaRule::up_to("image/png", 10 * MB),
    MediaRule::up_to("image/jpeg", 10 * MB),
    MediaRule::up_to("image/gif", 10 * MB),
    MediaRule::up_to("video/mp4", media::MAX_BYTES).alone(),
];

impl Platform for Facebook {
    fn info(&self) -> PlatformInfo {
        PlatformInfo {
            id: PlatformId::Facebook,
            name: "Facebook Page",
            auth: AuthKind::OAuth2,
            limits: Limits {
                max_chars: 63_206,
                max_media: 10,
                accepts: MEDIA,
                supports_alt_text: true,
                requires_title: false,
                requires_media: false,
                link_is_content: true,
            },
            connect_fields: vec![
                FieldSpec::text(
                    "page_id",
                    "Page ID",
                    "",
                    "Leave blank if you manage exactly one Page — Windbag will find it. \
                     With several, paste the ID of the one to post to (Page → About → \
                     Page transparency).",
                )
                .optional(),
            ],
            app_fields: vec![
                FieldSpec::text(
                    "client_id",
                    "Meta app ID",
                    "",
                    "From your app's settings at developers.facebook.com, with the \
                     \"Manage everything on your Page\" use case added.",
                ),
                FieldSpec::secret(
                    "client_secret",
                    "Meta app secret",
                    "",
                    "Required — Meta has no public-client flow for Facebook Login.",
                ),
                FieldSpec::text(
                    "ads_access",
                    "Ads access",
                    "no",
                    "Also request the ads permissions, which is what Meta's hosted ads MCP                      server authorizes with. Off by default: it widens the consent screen for                      everyone to serve a feature most never use.",
                )
                .optional()
                .choosing(&["no", "yes"]),
            ],
            setup_url: Some("https://developers.facebook.com/apps"),
            // Not a constant, and deliberately not read from the credential
            // store here: `info()` runs on every keystroke in the composer. The
            // `get_web_host` command reports it instead.
            redirect_uri: None,
            target_fields: Vec::new(),
            notes: "Posts as the Page rather than as you. Needs your own Meta app, \
                    registered with the HTTPS redirect from Settings → Web deployment.",
        }
    }

    fn connect(&self, input: &ConnectInput) -> Result<Connected> {
        let app = input.app()?;
        let secret_key = client_secret(app)?;
        let host = webhost::require()?;
        let redirect = host.redirect_uri();
        let wanted_page = input.optional_field("page_id");

        let scopes = scopes_for(app);
        let handoff = crate::oauth::handoff(
            PlatformId::Facebook,
            &authorize_url(),
            &app.client_id,
            &redirect,
            &scopes,
            &[],
            false,
        )?;

        // Facebook's code exchange is a GET, unlike Threads' and Instagram's.
        let base = api_base();
        let short = token_get(
            &format!("{base}/oauth/access_token"),
            &[
                ("client_id", &app.client_id),
                ("client_secret", secret_key),
                ("redirect_uri", &redirect),
                ("code", &handoff.code),
            ],
            LABEL,
        )?;
        // The long-lived USER token matters even though it is not what publishes:
        // a Page token derived from a short-lived one expires with it.
        let long_user = token_get(
            &format!("{base}/oauth/access_token"),
            &[
                ("grant_type", "fb_exchange_token"),
                ("client_id", &app.client_id),
                ("client_secret", secret_key),
                ("fb_exchange_token", &short.access_token),
            ],
            LABEL,
        )?;

        let page = pick_page(&long_user.access_token, wanted_page)?;

        Ok(Connected {
            remote_id: page.id,
            handle: page.name.clone(),
            display_name: Some(page.name),
            avatar_url: page.picture,
            instance: None,
            scopes: Some(scopes),
            char_limit: None,
            secret: AccountSecret {
                // The PAGE token is what publishes. It does not expire on a
                // clock, so there is no `expires_at` and nothing to refresh.
                access_token: page.access_token,
                refresh_token: None,
                expires_at: None,
                // Kept so a revoked Page token can be re-derived without a full
                // reconnect, and so the connection can be audited.
                extra: json!({ "user_token": long_user.access_token }),
            },
        })
    }

    fn publish(&self, request: &PublishRequest<'_>) -> Result<Published> {
        let token = &request.secret.access_token;
        let page = &request.account.remote_id;
        let base = api_base();

        let images: Vec<&MediaItem> = request
            .media
            .iter()
            .filter(|item| !item.mime.starts_with("video/"))
            .collect();
        let video = request
            .media
            .iter()
            .find(|item| item.mime.starts_with("video/"));

        // A video is its own endpoint and cannot share a post with photos, so it
        // wins outright rather than being silently dropped from a /feed post.
        if let Some(item) = video {
            let response = upload_bytes(
                &format!("{base}/{page}/videos"),
                item,
                &[
                    ("description", request.body),
                    ("access_token", token.as_str()),
                ],
            )?;
            let id = id_of(&response, LABEL, "video")?;
            return Ok(Published {
                remote_url: Some(format!("https://www.facebook.com/{id}")),
                remote_id: id,
            });
        }

        match images.len() {
            0 => {
                let mut form = vec![("message", request.body), ("access_token", token.as_str())];
                if let Some(link) = request.link {
                    form.push(("link", link));
                }
                let response = post_form_at(&format!("{base}/{page}/feed"), &form)?;
                let id = id_of(&response, LABEL, "post")?;
                Ok(Published {
                    remote_url: Some(format!("https://www.facebook.com/{id}")),
                    remote_id: id,
                })
            }
            1 => {
                let response =
                    upload_photo(&base, page, token, images[0], Some(request.body), true)?;
                // A published photo answers with both its own id and the id of
                // the post wrapping it; the post is what a permalink addresses.
                let id = response
                    .get("post_id")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
                    .map_or_else(|| id_of(&response, LABEL, "photo"), Ok)?;
                Ok(Published {
                    remote_url: Some(format!("https://www.facebook.com/{id}")),
                    remote_id: id,
                })
            }
            _ => {
                // Unpublished first, then one /feed post referencing them all —
                // otherwise each photo becomes a separate post on the Page.
                let mut attached = Vec::with_capacity(images.len());
                for item in &images {
                    let response = upload_photo(&base, page, token, item, None, false)?;
                    attached.push(id_of(&response, LABEL, "photo")?);
                }
                let mut form: Vec<(String, String)> = vec![
                    ("message".to_string(), request.body.to_string()),
                    ("access_token".to_string(), token.clone()),
                ];
                for (index, media_fbid) in attached.iter().enumerate() {
                    form.push((
                        format!("attached_media[{index}]"),
                        json!({ "media_fbid": media_fbid }).to_string(),
                    ));
                }
                let pairs: Vec<(&str, &str)> = form
                    .iter()
                    .map(|(key, value)| (key.as_str(), value.as_str()))
                    .collect();
                let response = post_form_at(&format!("{base}/{page}/feed"), &pairs)?;
                let id = id_of(&response, LABEL, "post")?;
                Ok(Published {
                    remote_url: Some(format!("https://www.facebook.com/{id}")),
                    remote_id: id,
                })
            }
        }
    }
}

/// One Page the signed-in user administers.
struct Page {
    id: String,
    name: String,
    access_token: String,
    picture: Option<String>,
}

/// Resolves which Page to connect.
///
/// With one Page and no preference, that Page. With several and no preference,
/// an error that LISTS them — "pick one" without saying what the choices are
/// would send the user back to Facebook to hunt for an id.
fn pick_page(user_token: &str, wanted: Option<&str>) -> Result<Page> {
    let response = get_json(
        &format!("{}/me/accounts", api_base()),
        &[
            ("fields", "id,name,access_token,picture{url}"),
            ("access_token", user_token),
        ],
        LABEL,
    )?;
    let pages = response
        .get("data")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();

    if pages.is_empty() {
        return Err(AppError::InvalidInput(
            "This Facebook account administers no Pages, or the app was not granted access \
             to any. Grant the Page on the consent screen and try again."
                .into(),
        ));
    }

    let chosen = match wanted {
        Some(id) => pages
            .iter()
            .find(|page| page.get("id").and_then(serde_json::Value::as_str) == Some(id))
            .ok_or_else(|| {
                AppError::InvalidInput(format!(
                    "No Page with ID `{id}` was granted to this app. Available: {}",
                    describe(&pages)
                ))
            })?,
        None if pages.len() == 1 => &pages[0],
        None => {
            return Err(AppError::InvalidInput(format!(
                "This account administers several Pages. Re-run the connection with the \
                 Page ID filled in. Available: {}",
                describe(&pages)
            )));
        }
    };

    Ok(Page {
        id: id_of(chosen, LABEL, "Page")?,
        name: chosen
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Facebook Page")
            .to_string(),
        access_token: chosen
            .get("access_token")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                AppError::Unauthorized(format!(
                    "{LABEL} returned no access token for this Page. The app needs the \
                     pages_manage_posts permission."
                ))
            })?
            .to_string(),
        picture: chosen
            .pointer("/picture/data/url")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
    })
}

fn describe(pages: &[serde_json::Value]) -> String {
    pages
        .iter()
        .map(|page| {
            format!(
                "{} ({})",
                page.get("name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unnamed"),
                page.get("id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("?")
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// One photo, as bytes. `published` false parks it for an `attached_media`
/// reference instead of putting it on the Page by itself.
fn upload_photo(
    base: &str,
    page: &str,
    token: &str,
    item: &MediaItem,
    caption: Option<&str>,
    published: bool,
) -> Result<serde_json::Value> {
    let published = if published { "true" } else { "false" };
    let mut fields: Vec<(&str, &str)> = vec![("published", published), ("access_token", token)];
    if let Some(caption) = caption {
        fields.push(("caption", caption));
    }
    let alt = item
        .alt_text
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(alt) = alt {
        fields.push(("alt_text_custom", alt));
    }
    upload_bytes(&format!("{base}/{page}/photos"), item, &fields)
}

/// A multipart POST carrying the file as `source` — the byte path no other Meta
/// surface offers.
fn upload_bytes(url: &str, item: &MediaItem, fields: &[(&str, &str)]) -> Result<serde_json::Value> {
    let part = reqwest::blocking::multipart::Part::bytes(item.bytes.clone())
        .file_name("upload")
        .mime_str(&item.mime)
        .map_err(|e| {
            AppError::InvalidInput(format!("`{}` is not a usable media type: {e}", item.mime))
        })?;
    let mut form = reqwest::blocking::multipart::Form::new().part("source", part);
    for (key, value) in fields {
        form = form.text((*key).to_string(), (*value).to_string());
    }

    let (status, body) = http::read_body(http::client().post(url).multipart(form).send()?);
    if !(200..300).contains(&status) {
        return Err(map_error(status, &body, LABEL));
    }
    serde_json::from_str(&body)
        .map_err(|e| AppError::Platform(format!("{LABEL} returned an unreadable response: {e}")))
}

fn post_form_at(url: &str, form: &[(&str, &str)]) -> Result<serde_json::Value> {
    super::post_form(url, form, LABEL)
}

/// The consent screen this connection asks for. Ads permissions are opt-in, so
/// the default connection is exactly what posting to a Page needs and nothing
/// more — see [`crate::metaads`] for what the extra half unlocks.
fn scopes_for(app: &AppCredentials) -> String {
    if app
        .extra("ads_access")
        .is_some_and(|value| value.eq_ignore_ascii_case("yes"))
    {
        format!("{SCOPES},ads_read,ads_management")
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
                "Facebook needs your app secret as well as the app ID — Meta has no \
                 public-client flow. Add it in Settings → Platform apps."
                    .into(),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pages(count: usize) -> Vec<serde_json::Value> {
        (0..count)
            .map(|n| json!({ "id": n.to_string(), "name": format!("Page {n}"), "access_token": "t" }))
            .collect()
    }

    #[test]
    fn several_pages_are_named_in_the_error_rather_than_left_to_a_hunt() {
        let listed = describe(&pages(2));
        assert!(listed.contains("Page 0 (0)"), "{listed}");
        assert!(listed.contains("Page 1 (1)"), "{listed}");
    }

    #[test]
    fn a_page_post_needs_no_media_unlike_instagram() {
        assert!(!Facebook.info().limits.requires_media);
    }

    fn app(ads: Option<&str>) -> AppCredentials {
        let mut extra = std::collections::HashMap::new();
        if let Some(value) = ads {
            extra.insert("ads_access".to_string(), value.to_string());
        }
        AppCredentials {
            client_id: "id".into(),
            client_secret: Some("s".into()),
            extra,
        }
    }

    #[test]
    fn ads_permissions_are_off_unless_asked_for() {
        // A user who only schedules posts must never see an ads consent screen.
        assert!(!scopes_for(&app(None)).contains("ads_"));
        assert!(!scopes_for(&app(Some("no"))).contains("ads_"));
    }

    #[test]
    fn opting_in_adds_both_ads_scopes() {
        let scopes = scopes_for(&app(Some("yes")));
        assert!(scopes.contains("ads_read"), "{scopes}");
        assert!(scopes.contains("ads_management"), "{scopes}");
        // And still everything a Page post needs.
        assert!(scopes.contains("pages_manage_posts"), "{scopes}");
    }

    #[test]
    fn the_page_id_field_is_optional_because_one_page_needs_no_choice() {
        let info = Facebook.info();
        let field = info
            .connect_fields
            .iter()
            .find(|field| field.key == "page_id")
            .expect("page_id");
        assert!(!field.required);
    }
}
