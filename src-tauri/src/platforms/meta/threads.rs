//! Threads, through the Threads API on `graph.threads.net`.
//!
//! Publishing is TWO calls, always: build a media container, then publish it by
//! id. A text post is not a special case that skips the container — it is a
//! container with `media_type=TEXT` — so the flow below has one shape for
//! everything and only the container's parameters change.
//!
//! A carousel is that flow nested: one container per item with
//! `is_carousel_item=true`, then a `CAROUSEL` container naming them as
//! `children`, then the publish. Two to twenty items.
//!
//! Anything with video in it is processed ASYNCHRONOUSLY. The container comes
//! back immediately and is not publishable until its `status` reaches `FINISHED`,
//! so [`await_container`] polls before publishing rather than letting the publish
//! fail with an error about a container that was merely not ready yet.
//!
//! Media is FETCHED BY META from a public URL — there is no byte upload — so an
//! attachment is parked through [`crate::webhost`] first.

use serde_json::json;

use super::{Grant, get_json, id_of, post_form, token_get};
use crate::error::{AppError, Result};
use crate::platforms::{
    AccountSecret, AppCredentials, AuthKind, ConnectInput, Connected, FieldSpec, Limits, MediaItem,
    Platform, PlatformId, PlatformInfo, PublishRequest, Published,
};
use crate::webhost;

pub struct Threads;

const AUTHORIZE_URL: &str = "https://threads.net/oauth/authorize";
const TOKEN_URL: &str = "https://graph.threads.net/oauth/access_token";
const EXCHANGE_URL: &str = "https://graph.threads.net/access_token";
const REFRESH_URL: &str = "https://graph.threads.net/refresh_access_token";
pub(crate) const API_BASE: &str = "https://graph.threads.net/v1.0";
const SCOPES: &str = "threads_basic,threads_content_publish";
const LABEL: &str = "Threads";

/// Threads processes video containers off the request path. Twelve tries two
/// seconds apart is a bit over 20 seconds — comfortably enough for an image or a
/// short clip, and short enough that a stuck container fails the target and gets
/// retried by the scheduler rather than pinning a worker thread.
const POLL_TRIES: usize = 12;
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

impl Platform for Threads {
    fn info(&self) -> PlatformInfo {
        PlatformInfo {
            id: PlatformId::Threads,
            name: "Threads",
            auth: AuthKind::OAuth2,
            limits: Limits {
                max_chars: 500,
                // A carousel takes 2–20; a single post takes 1. The cap is the
                // carousel's, and `publish` picks the shape from the count.
                max_media: 20,
                supports_alt_text: true,
                requires_title: false,
                requires_media: false,
            },
            connect_fields: Vec::new(),
            app_fields: vec![
                FieldSpec::text(
                    "client_id",
                    "Threads app ID",
                    "",
                    "From your app's \"Access the Threads API\" use case at \
                     developers.facebook.com. This is the Threads app ID, not the Meta app ID.",
                ),
                FieldSpec::secret(
                    "client_secret",
                    "Threads app secret",
                    "",
                    "Required — Meta has no public-client flow for Threads.",
                ),
                FieldSpec::text("insights_access", "Insights access", "no", "Also request the insights permission, so the stats screen can show what each post earned. Off by default: it widens the consent screen, and a connection made without it must be reconnected to gain it.")
                    .optional()
                    .choosing(&["no", "yes"]),
            ],
            setup_url: Some("https://developers.facebook.com/apps"),
            // Filled in from the companion deployment, because Meta refuses a
            // loopback redirect and the HTTPS one is per-install.
            // Not a constant, and deliberately not read from the credential
            // store here: `info()` runs on every keystroke in the composer. The
            // `get_web_host` command reports it instead.
            redirect_uri: None,
            target_fields: vec![
                FieldSpec::text(
                    "topic_tag",
                    "Topic tag",
                    "",
                    "One topic, without the #. No periods or ampersands.",
                )
                .optional(),
                FieldSpec::text(
                    "reply_control",
                    "Who can reply",
                    "everyone",
                    "Restricts who may reply to the post.",
                )
                .optional()
                .choosing(&["everyone", "accounts_you_follow", "mentioned_only"]),
            ],
            notes: "Needs your own Meta app with the Threads API use case. Register the \
                    HTTPS redirect from Settings → Web deployment on it — Meta \
                    refuses the loopback one.",
        }
    }

    /// Threads counts emoji as their UTF-8 BYTE length against the 500, and
    /// everything else as one character.
    ///
    /// The emoji test here is by codepoint range rather than by the full Unicode
    /// property table, which would be a dependency for one limit on one platform.
    /// Where it is wrong it counts something as 4 that Threads counts as 1, so it
    /// refuses a post Threads would have taken — visible and immediately fixable,
    /// where the opposite error is a scheduled post failing at 3am.
    fn count_body(&self, body: &str) -> usize {
        body.chars()
            .map(|ch| if is_emoji(ch) { ch.len_utf8() } else { 1 })
            .sum()
    }

    fn connect(&self, input: &ConnectInput) -> Result<Connected> {
        let app = input.app()?;
        let secret_key = client_secret(app)?;
        let host = webhost::require()?;
        let redirect = host.redirect_uri();

        // No PKCE: Meta does not offer it here, and an exchange carrying a
        // verifier is rejected rather than ignored.
        let handoff = crate::oauth::handoff(
            PlatformId::Threads,
            AUTHORIZE_URL,
            &app.client_id,
            &redirect,
            &scopes_for(app),
            &[],
            false,
        )?;

        // Short-lived first — this is the only call in the family that is a form
        // POST rather than a query GET.
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

        // Then traded once for the 60-day token. Skipping this leaves a
        // connection that dies in an hour with no way to renew it.
        let long = exchange_long_lived(short_token, secret_key)?;
        let me = get_json(
            &format!("{API_BASE}/me"),
            &[
                ("fields", "id,username,name,threads_profile_picture_url"),
                ("access_token", &long.access_token),
            ],
            LABEL,
        )?;

        let username = me
            .get("username")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Threads account");
        Ok(Connected {
            remote_id: id_of(&me, LABEL, "profile")?,
            handle: format!("@{username}"),
            display_name: me
                .get("name")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            avatar_url: me
                .get("threads_profile_picture_url")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            instance: None,
            scopes: Some(scopes_for(app)),
            char_limit: None,
            secret: AccountSecret {
                expires_at: long.expires_at(),
                access_token: long.access_token,
                // Meta issues none: the access token extends ITSELF, which is
                // why `refresh` below presents it rather than a separate one.
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
        let grant = token_get(
            REFRESH_URL,
            &[
                ("grant_type", "th_refresh_token"),
                ("access_token", &secret.access_token),
            ],
            LABEL,
        )?;
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
        let containers = format!("{API_BASE}/{user}/threads");

        // Attachments are parked publicly first — Meta fetches them itself, and
        // only ever over HTTPS.
        let hosted = if request.media.is_empty() {
            Vec::new()
        } else {
            let host = webhost::require()?;
            request
                .media
                .iter()
                .map(|item| Ok((item, webhost::upload(&host, item)?.url)))
                .collect::<Result<Vec<_>>>()?
        };

        let creation_id = match hosted.len() {
            0 => {
                let mut form = vec![
                    ("media_type".to_string(), "TEXT".to_string()),
                    ("text".to_string(), request.body.to_string()),
                    ("access_token".to_string(), token.clone()),
                ];
                // A link on a text-only post becomes a real preview card;
                // Threads rejects the parameter on anything carrying media.
                if let Some(link) = request.link {
                    form.push(("link_attachment".into(), link.to_string()));
                }
                push_options(&mut form, request);
                create_container(&containers, &form)?
            }
            1 => {
                let (item, url) = &hosted[0];
                let mut form = single_media_form(item, url, token);
                form.push(("text".into(), request.body.to_string()));
                push_options(&mut form, request);
                let id = create_container(&containers, &form)?;
                await_container(&id, token)?;
                id
            }
            _ => {
                let mut children = Vec::with_capacity(hosted.len());
                for (item, url) in &hosted {
                    let mut form = single_media_form(item, url, token);
                    form.push(("is_carousel_item".into(), "true".into()));
                    let id = create_container(&containers, &form)?;
                    await_container(&id, token)?;
                    children.push(id);
                }
                let mut form = vec![
                    ("media_type".to_string(), "CAROUSEL".to_string()),
                    ("children".to_string(), children.join(",")),
                    ("text".to_string(), request.body.to_string()),
                    ("access_token".to_string(), token.clone()),
                ];
                push_options(&mut form, request);
                let id = create_container(&containers, &form)?;
                await_container(&id, token)?;
                id
            }
        };

        let published = post_form(
            &format!("{API_BASE}/{user}/threads_publish"),
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

/// The container parameters for one image or video item.
fn single_media_form(item: &MediaItem, url: &str, token: &str) -> Vec<(String, String)> {
    let is_video = item.mime.starts_with("video/");
    let mut form = vec![
        (
            "media_type".to_string(),
            if is_video { "VIDEO" } else { "IMAGE" }.to_string(),
        ),
        (
            if is_video { "video_url" } else { "image_url" }.to_string(),
            url.to_string(),
        ),
        ("access_token".to_string(), token.to_string()),
    ];
    if let Some(alt) = item
        .alt_text
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        form.push(("alt_text".into(), alt.to_string()));
    }
    form
}

/// The per-destination options, applied to whichever container carries the text.
fn push_options(form: &mut Vec<(String, String)>, request: &PublishRequest<'_>) {
    if let Some(tag) = request.option("topic_tag") {
        // Threads rejects the whole container on a tag containing either, and
        // "#rust" is what a user naturally types.
        form.push((
            "topic_tag".into(),
            tag.trim_start_matches('#').replace(['.', '&'], ""),
        ));
    }
    if let Some(control) = request.option("reply_control") {
        form.push(("reply_control".into(), control.to_string()));
    }
}

fn create_container(url: &str, form: &[(String, String)]) -> Result<String> {
    let pairs: Vec<(&str, &str)> = form
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    id_of(&post_form(url, &pairs, LABEL)?, LABEL, "media container")
}

/// Blocks until a container is publishable, or explains why it never will be.
///
/// An image container is usually `FINISHED` on the first look; a video one is
/// not. Publishing an `IN_PROGRESS` container fails with an error about the
/// container rather than about the wait, which is why this exists.
fn await_container(container_id: &str, token: &str) -> Result<()> {
    for attempt in 0..POLL_TRIES {
        let status = get_json(
            &format!("{API_BASE}/{container_id}"),
            &[("fields", "status,error_message"), ("access_token", token)],
            LABEL,
        )?;
        let state = status
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("IN_PROGRESS");
        let message = status
            .get("error_message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("no reason given");

        match state {
            // PUBLISHED can happen on a retry of a target whose publish call
            // succeeded before the result was recorded.
            "FINISHED" | "PUBLISHED" => return Ok(()),
            "ERROR" => {
                return Err(AppError::InvalidInput(format!(
                    "{LABEL} could not process the attachment: {message}"
                )));
            }
            "EXPIRED" => {
                return Err(AppError::InvalidInput(format!(
                    "The {LABEL} media container expired before it was published: {message}"
                )));
            }
            _ if attempt + 1 < POLL_TRIES => std::thread::sleep(POLL_INTERVAL),
            _ => {}
        }
    }
    // Retryable on purpose: the container is still being processed, and the same
    // post may well go out on the scheduler's next pass.
    Err(AppError::Platform(format!(
        "{LABEL} is still processing the attachment after {}s.",
        POLL_TRIES as u64 * POLL_INTERVAL.as_secs()
    )))
}

/// Best effort: the post is already live, so failing to read its permalink must
/// not turn a successful publish into a failed target.
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

fn exchange_long_lived(short_token: &str, client_secret: &str) -> Result<Grant> {
    token_get(
        EXCHANGE_URL,
        &[
            ("grant_type", "th_exchange_token"),
            ("client_secret", client_secret),
            ("access_token", short_token),
        ],
        LABEL,
    )
}

/// The consent screen this connection asks for. Insights are opt-in, so the
/// default is exactly what posting needs — see [`crate::stats`] for what the
/// extra scope unlocks.
fn scopes_for(app: &AppCredentials) -> String {
    if app
        .extra("insights_access")
        .is_some_and(|value| value.eq_ignore_ascii_case("yes"))
    {
        format!("{SCOPES},threads_manage_insights")
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
                "Threads needs your app secret as well as the app ID — Meta has no \
                 public-client flow. Add it in Settings → Platform apps."
                    .into(),
            )
        })
}

/// The emoji planes plus the older symbol blocks that Threads also bills by
/// byte. Deliberately a range test rather than a Unicode property lookup: see
/// [`Threads::count_body`] for why erring high is the right direction.
fn is_emoji(ch: char) -> bool {
    matches!(ch as u32,
        // The emoji planes, which already contain the regional indicators that
        // make up flag sequences.
        0x1F000..=0x1FAFF
        | 0x2600..=0x27BF // misc symbols and dingbats
        | 0x2B00..=0x2BFF // arrows and misc symbols
        | 0xFE00..=0xFE0F // variation selectors
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_counts_one_per_character() {
        assert_eq!(Threads.count_body("hello"), 5);
    }

    #[test]
    fn an_emoji_costs_its_utf8_bytes() {
        // Threads bills emoji by byte, so one grinning face is four of the 500.
        assert_eq!(Threads.count_body("\u{1F600}"), 4);
        assert_eq!(Threads.count_body("hi \u{1F600}"), 7);
    }

    #[test]
    fn accented_latin_still_counts_as_one() {
        // Two UTF-8 bytes, but not an emoji, so it is one character to Threads.
        assert_eq!(Threads.count_body("café"), 4);
    }

    #[test]
    fn a_topic_tag_is_cleaned_of_what_threads_rejects() {
        let options = json!({ "topic_tag": "#rust.lang&more" });
        let account = crate::db::Account {
            id: 1,
            platform: PlatformId::Threads,
            remote_id: "1".into(),
            handle: "@me".into(),
            display_name: None,
            avatar_url: None,
            instance: None,
            scopes: None,
            char_limit: None,
            token_expires_at: None,
            status: crate::db::ACCOUNT_OK.into(),
            created_at: crate::db::now_rfc3339(),
        };
        let secret = AccountSecret::default();
        let request = PublishRequest {
            target_id: 1,
            account: &account,
            secret: &secret,
            body: "",
            title: None,
            link: None,
            media: &[],
            options: &options,
        };
        let mut form = Vec::new();
        push_options(&mut form, &request);
        assert_eq!(form[0].1, "rustlangmore");
    }

    #[test]
    fn alt_text_travels_with_the_container() {
        let item = MediaItem {
            bytes: Vec::new(),
            mime: "image/png".into(),
            alt_text: Some("a chart".into()),
        };
        let form = single_media_form(&item, "https://x/y.png", "tok");
        assert!(form.iter().any(|(k, v)| k == "alt_text" && v == "a chart"));
        assert!(form.iter().any(|(k, _)| k == "image_url"));
    }

    #[test]
    fn a_video_uses_the_video_url_parameter() {
        let item = MediaItem {
            bytes: Vec::new(),
            mime: "video/mp4".into(),
            alt_text: None,
        };
        let form = single_media_form(&item, "https://x/y.mp4", "tok");
        assert!(form.iter().any(|(k, v)| k == "media_type" && v == "VIDEO"));
        assert!(form.iter().any(|(k, _)| k == "video_url"));
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
        assert!(scopes.contains("threads_manage_insights"), "{scopes}");
        // The posting scopes survive — insights are additive, not a swap.
        assert!(scopes.contains(SCOPES), "{scopes}");
    }
}
