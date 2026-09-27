//! The platform contract.
//!
//! Every destination Windbag can post to implements [`Platform`]. The renderer
//! never learns a platform's shape from hardcoded TypeScript — it renders forms
//! from [`PlatformInfo`], so adding an adapter here is the whole change.
//!
//! Two auth shapes cover all eight:
//!   * [`AuthKind::Credentials`] — a handle and an app password typed straight
//!     into the app (Bluesky). Nothing to register anywhere.
//!   * [`AuthKind::OAuth2`] — a browser handoff. Where `app_fields` is
//!     non-empty the user must register THEIR OWN developer app and paste its
//!     client id: X, Reddit, `LinkedIn` and all three Meta surfaces gate posting
//!     behind an approved app, and a client secret shipped inside a distributed
//!     binary is not a secret. Mastodon is the exception — it registers an app
//!     on the fly.
//!
//! The three Meta destinations live under [`meta`] because they share a
//! developer app, an error envelope and a token lifecycle, and because all three
//! need an HTTPS redirect the desktop app cannot serve — see [`crate::webhost`].

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};

pub mod bluesky;
pub mod linkedin;
pub mod mastodon;
pub mod meta;
pub mod reddit;
pub mod x;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlatformId {
    Bluesky,
    Mastodon,
    Reddit,
    X,
    Linkedin,
    Threads,
    Instagram,
    Facebook,
}

impl PlatformId {
    pub const ALL: [Self; 8] = [
        Self::Bluesky,
        Self::Mastodon,
        Self::Reddit,
        Self::X,
        Self::Linkedin,
        Self::Threads,
        Self::Instagram,
        Self::Facebook,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bluesky => "bluesky",
            Self::Mastodon => "mastodon",
            Self::Reddit => "reddit",
            Self::X => "x",
            Self::Linkedin => "linkedin",
            Self::Threads => "threads",
            Self::Instagram => "instagram",
            Self::Facebook => "facebook",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "bluesky" => Ok(Self::Bluesky),
            "mastodon" => Ok(Self::Mastodon),
            "reddit" => Ok(Self::Reddit),
            "x" => Ok(Self::X),
            "linkedin" => Ok(Self::Linkedin),
            "threads" => Ok(Self::Threads),
            "instagram" => Ok(Self::Instagram),
            "facebook" => Ok(Self::Facebook),
            other => Err(AppError::InvalidInput(format!(
                "Unknown platform `{other}`."
            ))),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Bluesky => "Bluesky",
            Self::Mastodon => "Mastodon",
            Self::Reddit => "Reddit",
            Self::X => "X",
            Self::Linkedin => "LinkedIn",
            Self::Threads => "Threads",
            Self::Instagram => "Instagram",
            Self::Facebook => "Facebook Page",
        }
    }

    /// True for the three that share one Meta developer app and one HTTPS
    /// redirect. Used to decide whether a missing web deployment is worth
    /// mentioning, and to group them in Settings.
    pub fn is_meta(self) -> bool {
        matches!(self, Self::Threads | Self::Instagram | Self::Facebook)
    }
}

impl std::fmt::Display for PlatformId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl rusqlite::ToSql for PlatformId {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(rusqlite::types::ToSqlOutput::from(self.as_str()))
    }
}

impl rusqlite::types::FromSql for PlatformId {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        let text = value.as_str()?;
        Self::parse(text).map_err(|_| rusqlite::types::FromSqlError::InvalidType)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AuthKind {
    Credentials,
    OAuth2,
}

/// One input the renderer must draw. `secret` swaps the field to a password
/// input and keeps the value out of any log line.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub placeholder: &'static str,
    pub help: &'static str,
    pub secret: bool,
    pub required: bool,
    /// A closed set of allowed values, drawn as a picker. Empty means free text.
    /// Declared here rather than inferred by the renderer from the help string,
    /// which is what it used to do — a sentence is not a schema.
    pub choices: &'static [&'static str],
    /// A per-destination option the platform bills to the body's character
    /// budget — Mastodon measures `spoiler_text` and the text together.
    pub counted: bool,
}

impl FieldSpec {
    const fn text(
        key: &'static str,
        label: &'static str,
        placeholder: &'static str,
        help: &'static str,
    ) -> Self {
        Self {
            key,
            label,
            placeholder,
            help,
            secret: false,
            required: true,
            choices: &[],
            counted: false,
        }
    }

    const fn secret(
        key: &'static str,
        label: &'static str,
        placeholder: &'static str,
        help: &'static str,
    ) -> Self {
        Self {
            key,
            label,
            placeholder,
            help,
            secret: true,
            required: true,
            choices: &[],
            counted: false,
        }
    }

    const fn optional(mut self) -> Self {
        self.required = false;
        self
    }

    const fn choosing(mut self, choices: &'static [&'static str]) -> Self {
        self.choices = choices;
        self
    }

    const fn counted(mut self) -> Self {
        self.counted = true;
        self
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
// Each flag is an independent fact a platform documents, read one at a time by
// `validate` and the composer — a table of facts, not a state to model.
#[expect(clippy::struct_excessive_bools)]
pub struct Limits {
    /// Body length the platform accepts. Counted in Unicode scalar values, which
    /// is what most of them actually measure; Bluesky counts graphemes and
    /// Threads bills emoji by UTF-8 byte, so both override `count_body`.
    pub max_chars: usize,
    pub max_media: usize,
    /// Every attachment type the ADAPTER can actually post, with the size the
    /// platform documents for it. A type missing here is refused at compose
    /// time: an MP4 sent into an image-only upload, or a PNG to a JPEG-only
    /// API, otherwise fails at 3am with nobody watching.
    pub accepts: &'static [MediaRule],
    pub supports_alt_text: bool,
    /// Reddit: a submission without a title is not a submission.
    pub requires_title: bool,
    /// Instagram: a caption is a property of an image or a video, never a post
    /// by itself. There is no text-only post to fall back to, so this is refused
    /// at compose time rather than at 3am.
    pub requires_media: bool,
    /// Reddit and Facebook: a URL with no text is a whole post — a link
    /// submission, a link share. Everywhere else the link rides along with the
    /// text (or is ignored), so it cannot stand in for an empty body.
    pub link_is_content: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformInfo {
    pub id: PlatformId,
    pub name: &'static str,
    pub auth: AuthKind,
    pub limits: Limits,
    /// Rendered by the "Add account" dialog.
    pub connect_fields: Vec<FieldSpec>,
    /// Rendered by Settings → Platform apps. Empty means nothing to register.
    pub app_fields: Vec<FieldSpec>,
    /// Where the user goes to create the developer app `app_fields` asks for.
    pub setup_url: Option<&'static str>,
    /// The exact redirect URI that must be registered on that app.
    ///
    /// `None` for the Meta platforms, whose redirect is the user's OWN
    /// deployment and therefore not a constant. It is reported by the
    /// `get_web_host` command instead — deriving it here would mean a
    /// credential-store read inside `info()`, which the composer calls on every
    /// keystroke through [`validate`].
    pub redirect_uri: Option<&'static str>,
    /// Per-destination options on a post (subreddit, visibility…).
    pub target_fields: Vec<FieldSpec>,
    /// One line the UI shows under the platform name in the connect dialog.
    pub notes: &'static str,
}

/// A mebibyte. Platform docs say "MB"; the binary reading is the lenient one,
/// and a file between the two readings is left for the platform to judge.
pub const MB: u64 = 1024 * 1024;

/// One attachment type a platform takes.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaRule {
    pub mime: &'static str,
    pub max_bytes: u64,
    /// Must be the only attachment on its post: X takes one video or one GIF,
    /// Mastodon and Facebook one video, never beside anything else. Refused
    /// here rather than left to an adapter to pick one and drop the rest.
    pub alone: bool,
}

impl MediaRule {
    const fn up_to(mime: &'static str, max_bytes: u64) -> Self {
        Self {
            mime,
            max_bytes,
            alone: false,
        }
    }

    const fn alone(mut self) -> Self {
        self.alone = true;
        self
    }
}

/// An attachment as validation sees it — its type and size, not its bytes.
/// The composer sends these for files it has only resolved; the scheduler
/// builds them from what it just read off disk.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaSpec {
    pub mime: String,
    pub bytes: u64,
}

/// What the OS credential store holds for one connected account.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSecret {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// RFC 3339. `None` means the token does not expire on a clock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    /// Publish-time material that is secret-adjacent rather than a token:
    /// Bluesky's app password (its sessions are short and re-created on demand),
    /// Mastodon's per-instance client credentials.
    #[serde(default)]
    pub extra: serde_json::Value,
}

impl AccountSecret {
    pub fn extra_str(&self, key: &str) -> Option<&str> {
        self.extra.get(key).and_then(serde_json::Value::as_str)
    }
}

/// The user's own developer app for a platform, from the OS credential store.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppCredentials {
    pub client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    /// Anything else the platform's `app_fields` asked for beyond the two above —
    /// `LinkedIn`'s `api_version` is the only one today. Keeping it open means a
    /// new adapter can need a third field without a schema change here.
    #[serde(default)]
    pub extra: HashMap<String, String>,
}

impl AppCredentials {
    pub fn extra(&self, key: &str) -> Option<&str> {
        self.extra
            .get(key)
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }
}

/// What the connect dialog collected, plus the developer app when there is one.
pub struct ConnectInput {
    pub fields: HashMap<String, String>,
    pub app: Option<AppCredentials>,
}

impl ConnectInput {
    pub fn field(&self, key: &str) -> Result<&str> {
        self.fields
            .get(key)
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| AppError::InvalidInput(format!("`{key}` is required.")))
    }

    pub fn optional_field(&self, key: &str) -> Option<&str> {
        self.fields
            .get(key)
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    pub fn app(&self) -> Result<&AppCredentials> {
        self.app.as_ref().ok_or_else(|| {
            AppError::InvalidInput(
                "This platform needs your own developer app. Add its client id in \
                 Settings → Platform apps first."
                    .into(),
            )
        })
    }
}

/// A newly connected account, before it has an id in the store.
pub struct Connected {
    pub remote_id: String,
    pub handle: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    /// Mastodon instance host, or the Bluesky PDS. `None` for single-host platforms.
    pub instance: Option<String>,
    pub scopes: Option<String>,
    /// Overrides the platform default when the remote publishes its own limit
    /// (every Mastodon instance sets its own `max_characters`).
    pub char_limit: Option<usize>,
    pub secret: AccountSecret,
}

/// One attachment, already read off disk by the caller.
pub struct MediaItem {
    pub bytes: Vec<u8>,
    pub mime: String,
    pub alt_text: Option<String>,
}

pub struct PublishRequest<'a> {
    /// The `post_targets` row being sent. Identifies THIS post to THIS account,
    /// which is what an idempotency key has to be built from — an account id
    /// would make every post to that account the same request.
    pub target_id: i64,
    pub account: &'a crate::db::Account,
    pub secret: &'a AccountSecret,
    pub body: &'a str,
    pub title: Option<&'a str>,
    pub link: Option<&'a str>,
    pub media: &'a [MediaItem],
    /// Per-target options, keyed by the platform's own `target_fields`.
    pub options: &'a serde_json::Value,
    /// What an earlier attempt at this target left behind to resume from —
    /// Meta's media container, Bluesky's record key — or `None` when there is
    /// nothing to pick up. Resuming is what lets a retry after a lost answer
    /// find the post it already made instead of making a second one.
    pub resume_key: Option<&'a str>,
    /// Stores (or with `None`, forgets) the resume key for the next attempt.
    /// Called BEFORE the request it guards, so a crash in between still leaves
    /// it behind.
    pub keep_resume_key: &'a dyn Fn(Option<&str>) -> Result<()>,
}

impl PublishRequest<'_> {
    pub fn option(&self, key: &str) -> Option<&str> {
        self.options
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }
}

#[derive(Debug, Clone)]
pub struct Published {
    pub remote_id: String,
    pub remote_url: Option<String>,
}

pub trait Platform: Send + Sync {
    fn info(&self) -> PlatformInfo;

    fn connect(&self, input: &ConnectInput) -> Result<Connected>;

    /// Returns replacement secrets when a refresh actually happened, `None` when
    /// the stored ones are still good. Called immediately before every publish.
    fn refresh(
        &self,
        _account: &crate::db::Account,
        _secret: &AccountSecret,
        _app: Option<&AppCredentials>,
    ) -> Result<Option<AccountSecret>> {
        Ok(None)
    }

    fn publish(&self, request: &PublishRequest<'_>) -> Result<Published>;

    /// How this platform counts a body against its own limit. Four of the five
    /// count Unicode scalar values; Bluesky counts graphemes.
    fn count_body(&self, body: &str) -> usize {
        body.chars().count()
    }
}

pub fn adapter(id: PlatformId) -> &'static dyn Platform {
    match id {
        PlatformId::Bluesky => &bluesky::Bluesky,
        PlatformId::Mastodon => &mastodon::Mastodon,
        PlatformId::Reddit => &reddit::Reddit,
        PlatformId::X => &x::X,
        PlatformId::Linkedin => &linkedin::Linkedin,
        PlatformId::Threads => &meta::threads::Threads,
        PlatformId::Instagram => &meta::instagram::Instagram,
        PlatformId::Facebook => &meta::facebook::Facebook,
    }
}

/// The account's credentials, refreshed first if they are about to expire and
/// written back when they rotate.
///
/// Both callers need exactly this: the scheduler before it publishes, and
/// [`crate::stats`] before it reads engagement. X's access tokens live two
/// hours, so a reporting read on a day-old token fails without it.
pub fn live_secret(
    database: &crate::db::Db,
    account: &crate::db::Account,
) -> Result<AccountSecret> {
    let adapter = adapter(account.platform);
    let app_credentials =
        crate::secrets::load_app_credentials(account.platform, account.instance.as_deref())?;
    let secret = crate::secrets::load_account_secret(account.platform, &account.remote_id)?;

    match adapter.refresh(account, &secret, app_credentials.as_ref())? {
        Some(refreshed) => {
            crate::secrets::store_account_secret(account.platform, &account.remote_id, &refreshed)?;
            database.set_account_token_expiry(account.id, refreshed.expires_at.as_deref())?;
            Ok(refreshed)
        }
        None => Ok(secret),
    }
}

pub fn all_info() -> Vec<PlatformInfo> {
    PlatformId::ALL
        .iter()
        .map(|id| adapter(*id).info())
        .collect()
}

/// Everything that can be rejected before a single request goes out. Run at
/// compose time so the counter in the UI and the scheduler agree, and again in
/// the scheduler so an edit between the two cannot smuggle an over-length body
/// past the check.
pub fn validate(
    id: PlatformId,
    body: &str,
    title: Option<&str>,
    link: Option<&str>,
    media: &[MediaSpec],
    options: &serde_json::Value,
    effective_char_limit: usize,
) -> Result<()> {
    let platform = adapter(id);
    let info = platform.info();
    let length = counted_length(id, body, options);

    let stands_on_link =
        info.limits.link_is_content && link.is_some_and(|url| !url.trim().is_empty());
    if body.trim().is_empty() && media.is_empty() && !stands_on_link {
        // Only what this platform can actually carry is offered as the fix.
        let mut options = vec!["text"];
        if info.limits.max_media > 0 {
            options.push("an attachment");
        }
        if info.limits.link_is_content {
            options.push("a link");
        }
        return Err(AppError::InvalidInput(format!(
            "{} needs {}.",
            info.name,
            options.join(" or ")
        )));
    }
    if length > effective_char_limit {
        return Err(AppError::InvalidInput(format!(
            "{} allows {effective_char_limit} characters; this is {length}.",
            info.name
        )));
    }
    if info.limits.requires_title && title.is_none_or(|value| value.trim().is_empty()) {
        return Err(AppError::InvalidInput(format!(
            "{} needs a title.",
            info.name
        )));
    }
    for field in &info.target_fields {
        let value = option_value(options, field.key);
        match value {
            None if field.required => {
                return Err(AppError::InvalidInput(format!(
                    "{} needs a {}.",
                    info.name,
                    field.label.to_lowercase()
                )));
            }
            Some(value) if !field.choices.is_empty() && !field.choices.contains(&value) => {
                return Err(AppError::InvalidInput(format!(
                    "{}: `{value}` is not a {} option. Pick one of {}.",
                    info.name,
                    field.label.to_lowercase(),
                    field.choices.join(", ")
                )));
            }
            _ => {}
        }
    }
    if info.limits.requires_media && media.is_empty() {
        return Err(AppError::InvalidInput(format!(
            "{} has no text-only post — attach an image or a video.",
            info.name
        )));
    }
    check_media(&info, media)
}

/// Count, type, size and pairing of the attachments against what this
/// platform (and its adapter) can carry.
fn check_media(info: &PlatformInfo, media: &[MediaSpec]) -> Result<()> {
    if media.len() > info.limits.max_media {
        return Err(AppError::InvalidInput(format!(
            "{} takes at most {} attachment(s); this has {}.",
            info.name,
            info.limits.max_media,
            media.len()
        )));
    }
    for item in media {
        let kind = media_label(&item.mime);
        let Some(rule) = info
            .limits
            .accepts
            .iter()
            .find(|rule| rule.mime == item.mime)
        else {
            let accepted: Vec<String> = info
                .limits
                .accepts
                .iter()
                .map(|rule| media_label(rule.mime))
                .collect();
            return Err(AppError::InvalidInput(format!(
                "{} cannot post {kind} files. It takes {}.",
                info.name,
                accepted.join(", ")
            )));
        };
        if item.bytes > rule.max_bytes {
            return Err(AppError::InvalidInput(format!(
                "{} takes {kind} files up to {}; this one is {}.",
                info.name,
                megabytes(rule.max_bytes),
                megabytes(item.bytes)
            )));
        }
        if rule.alone && media.len() > 1 {
            return Err(AppError::InvalidInput(format!(
                "{} posts {kind} files on their own — remove the other attachments.",
                info.name
            )));
        }
    }
    Ok(())
}

/// The length a platform holds against its limit: the body, plus any
/// per-destination option it bills to the same budget (Mastodon's content
/// warning). Shared with the composer's counter so the two cannot disagree.
pub fn counted_length(id: PlatformId, body: &str, options: &serde_json::Value) -> usize {
    let platform = adapter(id);
    platform
        .info()
        .target_fields
        .iter()
        .filter(|field| field.counted)
        .fold(platform.count_body(body), |total, field| {
            total + option_value(options, field.key).map_or(0, |value| platform.count_body(value))
        })
}

/// One per-destination option as the adapter will read it — the same trimming
/// as [`PublishRequest::option`], so what passes here is what gets sent.
fn option_value<'a>(options: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    options
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// `image/jpeg` → `JPEG`: the word a person knows the file by.
fn media_label(mime: &str) -> String {
    mime.rsplit('/').next().unwrap_or(mime).to_ascii_uppercase()
}

/// Bytes as megabytes to one decimal place, in the same binary unit as [`MB`].
fn megabytes(bytes: u64) -> String {
    let tenths = (bytes.saturating_mul(10) + MB / 2) / MB;
    if tenths.is_multiple_of(10) {
        format!("{} MB", tenths / 10)
    } else {
        format!("{}.{} MB", tenths / 10, tenths % 10)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rust() -> serde_json::Value {
        serde_json::json!({ "subreddit": "rust" })
    }

    #[test]
    fn a_required_destination_option_is_enforced() {
        let none = serde_json::json!({});
        let blank = serde_json::json!({ "subreddit": "  " });
        let err =
            validate(PlatformId::Reddit, "b", Some("T"), None, &[], &none, 40_000).unwrap_err();
        assert!(err.to_string().contains("subreddit"), "{err}");
        assert!(
            validate(
                PlatformId::Reddit,
                "b",
                Some("T"),
                None,
                &[],
                &blank,
                40_000
            )
            .is_err()
        );
        assert!(
            validate(
                PlatformId::Reddit,
                "b",
                Some("T"),
                None,
                &[],
                &rust(),
                40_000
            )
            .is_ok()
        );
    }

    #[test]
    fn a_choice_outside_the_declared_set_is_refused() {
        let wrong = serde_json::json!({ "visibility": "everyone" });
        let right = serde_json::json!({ "visibility": "unlisted" });
        let err = validate(PlatformId::Mastodon, "hi", None, None, &[], &wrong, 500).unwrap_err();
        assert!(err.to_string().contains("everyone"), "{err}");
        assert!(validate(PlatformId::Mastodon, "hi", None, None, &[], &right, 500).is_ok());
    }

    #[test]
    fn a_mastodon_content_warning_counts_toward_the_limit() {
        // Mastodon measures spoiler_text and text together against one limit.
        let body = "x".repeat(490);
        let long = serde_json::json!({ "spoiler_text": "y".repeat(20) });
        let short = serde_json::json!({ "spoiler_text": "y".repeat(10) });
        assert!(validate(PlatformId::Mastodon, &body, None, None, &[], &long, 500).is_err());
        assert!(validate(PlatformId::Mastodon, &body, None, None, &[], &short, 500).is_ok());
        assert_eq!(counted_length(PlatformId::Mastodon, &body, &long), 510);
    }

    fn spec(mime: &str, bytes: u64) -> MediaSpec {
        MediaSpec {
            mime: mime.into(),
            bytes,
        }
    }

    #[test]
    fn bluesky_refuses_a_video_it_would_send_into_an_image_embed() {
        let video = [spec("video/mp4", 1000)];
        let err = validate(
            PlatformId::Bluesky,
            "hi",
            None,
            None,
            &video,
            &serde_json::json!({}),
            300,
        )
        .unwrap_err();
        assert!(err.to_string().contains("MP4"), "{err}");
        let image = [spec("image/jpeg", 1000)];
        assert!(
            validate(
                PlatformId::Bluesky,
                "hi",
                None,
                None,
                &image,
                &serde_json::json!({}),
                300
            )
            .is_ok()
        );
    }

    #[test]
    fn an_image_over_the_platform_cap_is_refused() {
        // app.bsky.embed.images caps a blob at 2,000,000 bytes.
        let at_cap = [spec("image/png", 2_000_000)];
        let over = [spec("image/png", 2_000_001)];
        assert!(
            validate(
                PlatformId::Bluesky,
                "hi",
                None,
                None,
                &at_cap,
                &serde_json::json!({}),
                300
            )
            .is_ok()
        );
        assert!(
            validate(
                PlatformId::Bluesky,
                "hi",
                None,
                None,
                &over,
                &serde_json::json!({}),
                300
            )
            .is_err()
        );
    }

    #[test]
    fn instagram_takes_jpeg_only() {
        let png = [spec("image/png", 1000)];
        let jpeg = [spec("image/jpeg", 1000)];
        assert!(
            validate(
                PlatformId::Instagram,
                "",
                None,
                None,
                &png,
                &serde_json::json!({}),
                2200
            )
            .is_err()
        );
        assert!(
            validate(
                PlatformId::Instagram,
                "",
                None,
                None,
                &jpeg,
                &serde_json::json!({}),
                2200
            )
            .is_ok()
        );
    }

    #[test]
    fn linkedin_refuses_a_video_it_would_send_to_the_images_api() {
        let video = [spec("video/mp4", 1000)];
        assert!(
            validate(
                PlatformId::Linkedin,
                "hi",
                None,
                None,
                &video,
                &serde_json::json!({}),
                3000
            )
            .is_err()
        );
    }

    #[test]
    fn facebook_refuses_a_video_beside_images_instead_of_dropping_them() {
        let mixed = [spec("image/jpeg", 1000), spec("video/mp4", 1000)];
        let err = validate(
            PlatformId::Facebook,
            "hi",
            None,
            None,
            &mixed,
            &serde_json::json!({}),
            63_206,
        )
        .unwrap_err();
        assert!(err.to_string().contains("on their own"), "{err}");
        let video = [spec("video/mp4", 1000)];
        let images = [spec("image/jpeg", 1000), spec("image/png", 1000)];
        assert!(
            validate(
                PlatformId::Facebook,
                "hi",
                None,
                None,
                &video,
                &serde_json::json!({}),
                63_206
            )
            .is_ok()
        );
        assert!(
            validate(
                PlatformId::Facebook,
                "hi",
                None,
                None,
                &images,
                &serde_json::json!({}),
                63_206
            )
            .is_ok()
        );
    }

    #[test]
    fn every_type_the_picker_offers_is_accepted_somewhere() {
        // A type no adapter accepts would be a picker option that never posts.
        for mime in [
            "image/png",
            "image/jpeg",
            "image/gif",
            "image/webp",
            "video/mp4",
        ] {
            assert!(
                all_info().iter().any(|info| info
                    .limits
                    .accepts
                    .iter()
                    .any(|rule| rule.mime == mime)),
                "{mime}"
            );
        }
    }

    #[test]
    fn sizes_read_in_megabytes() {
        assert_eq!(megabytes(5 * MB), "5 MB");
        assert_eq!(megabytes(2_000_000), "1.9 MB");
    }

    #[test]
    fn rejects_a_body_over_the_effective_limit() {
        let body = "x".repeat(301);
        let err = validate(
            PlatformId::Bluesky,
            &body,
            None,
            None,
            &[],
            &serde_json::json!({}),
            300,
        )
        .unwrap_err();
        assert!(matches!(err, AppError::InvalidInput(_)), "{err}");
    }

    #[test]
    fn honours_a_per_account_limit_over_the_platform_default() {
        // A Mastodon instance raising its own max_characters must let a longer
        // body through even though the platform default is 500.
        let body = "x".repeat(1200);
        assert!(
            validate(
                PlatformId::Mastodon,
                &body,
                None,
                None,
                &[],
                &serde_json::json!({}),
                5000
            )
            .is_ok()
        );
        assert!(
            validate(
                PlatformId::Mastodon,
                &body,
                None,
                None,
                &[],
                &serde_json::json!({}),
                500
            )
            .is_err()
        );
    }

    #[test]
    fn reddit_needs_a_title() {
        assert!(
            validate(
                PlatformId::Reddit,
                "body",
                None,
                None,
                &[],
                &serde_json::json!({}),
                40_000
            )
            .is_err()
        );
        assert!(
            validate(
                PlatformId::Reddit,
                "body",
                Some("Title"),
                None,
                &[],
                &rust(),
                40_000
            )
            .is_ok()
        );
    }

    #[test]
    fn an_empty_post_with_media_is_allowed() {
        assert!(
            validate(
                PlatformId::Bluesky,
                "",
                None,
                None,
                &[spec("image/png", 1)],
                &serde_json::json!({}),
                300
            )
            .is_ok()
        );
        assert!(
            validate(
                PlatformId::Bluesky,
                "",
                None,
                None,
                &[],
                &serde_json::json!({}),
                300
            )
            .is_err()
        );
    }

    #[test]
    fn a_link_is_content_where_the_platform_posts_one() {
        // A Reddit link submission is a title and a URL — no body, no media.
        let link = Some("https://example.com");
        assert!(
            validate(
                PlatformId::Reddit,
                "",
                Some("Title"),
                link,
                &[],
                &rust(),
                40_000
            )
            .is_ok()
        );
        assert!(
            validate(
                PlatformId::Facebook,
                "",
                None,
                link,
                &[],
                &serde_json::json!({}),
                63_206
            )
            .is_ok()
        );
        // Bluesky ignores the link field, so a link alone would post nothing.
        assert!(
            validate(
                PlatformId::Bluesky,
                "",
                None,
                link,
                &[],
                &serde_json::json!({}),
                300
            )
            .is_err()
        );
        assert!(
            validate(
                PlatformId::Reddit,
                "",
                Some("Title"),
                None,
                &[],
                &serde_json::json!({}),
                40_000
            )
            .is_err()
        );
    }

    #[test]
    fn bluesky_counts_graphemes_not_scalars() {
        // A flag emoji is several scalars but one character to Bluesky.
        let flag = "\u{1F1EC}\u{1F1E7}";
        assert_eq!(bluesky::Bluesky.count_body(flag), 1);
        assert_eq!(flag.chars().count(), 2);
    }

    #[test]
    fn every_platform_id_round_trips() {
        for id in PlatformId::ALL {
            assert_eq!(PlatformId::parse(id.as_str()).unwrap(), id);
        }
    }
}
