//! The platform contract.
//!
//! Every destination Yapper can post to implements [`Platform`]. The renderer
//! never learns a platform's shape from hardcoded TypeScript — it renders forms
//! from [`PlatformInfo`], so adding an adapter here is the whole change.
//!
//! Two auth shapes cover all five:
//!   * [`AuthKind::Credentials`] — a handle and an app password typed straight
//!     into the app (Bluesky). Nothing to register anywhere.
//!   * [`AuthKind::OAuth2`] — a browser handoff. Where `app_fields` is
//!     non-empty the user must register THEIR OWN developer app and paste its
//!     client id: X, Reddit and `LinkedIn` all gate posting behind an approved
//!     app, and a client secret shipped inside a distributed binary is not a
//!     secret. Mastodon is the exception — it registers an app on the fly.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};

pub mod bluesky;
pub mod linkedin;
pub mod mastodon;
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
}

impl PlatformId {
    pub const ALL: [Self; 5] = [
        Self::Bluesky,
        Self::Mastodon,
        Self::Reddit,
        Self::X,
        Self::Linkedin,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bluesky => "bluesky",
            Self::Mastodon => "mastodon",
            Self::Reddit => "reddit",
            Self::X => "x",
            Self::Linkedin => "linkedin",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "bluesky" => Ok(Self::Bluesky),
            "mastodon" => Ok(Self::Mastodon),
            "reddit" => Ok(Self::Reddit),
            "x" => Ok(Self::X),
            "linkedin" => Ok(Self::Linkedin),
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
        }
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
        }
    }

    const fn optional(mut self) -> Self {
        self.required = false;
        self
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    /// Body length the platform accepts. Counted in Unicode scalar values, which
    /// is what four of the five actually measure; Bluesky counts graphemes and
    /// overrides `count_body`.
    pub max_chars: usize,
    pub max_media: usize,
    pub supports_alt_text: bool,
    /// Reddit: a submission without a title is not a submission.
    pub requires_title: bool,
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
    pub redirect_uri: Option<&'static str>,
    /// Per-destination options on a post (subreddit, visibility…).
    pub target_fields: Vec<FieldSpec>,
    /// One line the UI shows under the platform name in the connect dialog.
    pub notes: &'static str,
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
    pub account: &'a crate::db::Account,
    pub secret: &'a AccountSecret,
    pub body: &'a str,
    pub title: Option<&'a str>,
    pub link: Option<&'a str>,
    pub media: &'a [MediaItem],
    /// Per-target options, keyed by the platform's own `target_fields`.
    pub options: &'a serde_json::Value,
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
    media_count: usize,
    effective_char_limit: usize,
) -> Result<()> {
    let platform = adapter(id);
    let info = platform.info();
    let length = platform.count_body(body);

    if body.trim().is_empty() && media_count == 0 {
        return Err(AppError::InvalidInput(format!(
            "{} needs text or an attachment.",
            info.name
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
    if media_count > info.limits.max_media {
        return Err(AppError::InvalidInput(format!(
            "{} takes at most {} attachment(s); this has {media_count}.",
            info.name, info.limits.max_media
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_a_body_over_the_effective_limit() {
        let body = "x".repeat(301);
        let err = validate(PlatformId::Bluesky, &body, None, 0, 300).unwrap_err();
        assert!(matches!(err, AppError::InvalidInput(_)), "{err}");
    }

    #[test]
    fn honours_a_per_account_limit_over_the_platform_default() {
        // A Mastodon instance raising its own max_characters must let a longer
        // body through even though the platform default is 500.
        let body = "x".repeat(1200);
        assert!(validate(PlatformId::Mastodon, &body, None, 0, 5000).is_ok());
        assert!(validate(PlatformId::Mastodon, &body, None, 0, 500).is_err());
    }

    #[test]
    fn reddit_needs_a_title() {
        assert!(validate(PlatformId::Reddit, "body", None, 0, 40_000).is_err());
        assert!(validate(PlatformId::Reddit, "body", Some("Title"), 0, 40_000).is_ok());
    }

    #[test]
    fn an_empty_post_with_media_is_allowed() {
        assert!(validate(PlatformId::Bluesky, "", None, 1, 300).is_ok());
        assert!(validate(PlatformId::Bluesky, "", None, 0, 300).is_err());
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
