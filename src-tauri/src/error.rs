//! One error type for every Tauri command.
//!
//! Serializes as `"[CODE] message"`. The code half is a CONTRACT with the
//! renderer: `lib/query/client.ts` refuses to retry `NOT_FOUND`,
//! `INVALID_INPUT`, `UNAUTHORIZED` and `CONFLICT` because none of them fix
//! themselves, and `UNAUTHORIZED` is what flips an account into the
//! "reconnect" state in the UI. Adding a variant means deciding which of those
//! two behaviours it wants.

use std::fmt;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// The row/account/post does not exist.
    #[error("[NOT_FOUND] {0}")]
    NotFound(String),
    /// The caller sent something the app can reject without asking anyone.
    #[error("[INVALID_INPUT] {0}")]
    InvalidInput(String),
    /// Credentials are missing, expired beyond refresh, or were revoked.
    /// The account is marked `needs_reauth` wherever this surfaces.
    #[error("[UNAUTHORIZED] {0}")]
    Unauthorized(String),
    /// The request contradicts current state (already published, duplicate account).
    #[error("[CONFLICT] {0}")]
    Conflict(String),
    /// The remote said no. Retryable: rate limits and 5xx live here.
    #[error("[PLATFORM] {0}")]
    Platform(String),
    /// Could not reach the remote at all. Always retryable.
    #[error("[NETWORK] {0}")]
    Network(String),
    /// Ours: a broken store, an unreachable keychain, a bug.
    #[error("[INTERNAL] {0}")]
    Internal(String),
}

impl AppError {
    /// Whether the scheduler should try this target again later. Everything the
    /// user must act on (bad credentials, an over-length post) is terminal;
    /// everything the world might fix on its own is not.
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Platform(_) | Self::Network(_))
    }
}

impl serde::Serialize for AppError {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(err: rusqlite::Error) -> Self {
        match err {
            rusqlite::Error::QueryReturnedNoRows => Self::NotFound("No such row.".into()),
            other => Self::Internal(format!("Database error: {other}")),
        }
    }
}

impl From<serde_json::Error> for AppError {
    fn from(err: serde_json::Error) -> Self {
        Self::Internal(format!("Malformed stored JSON: {err}"))
    }
}

impl From<reqwest::Error> for AppError {
    fn from(err: reqwest::Error) -> Self {
        // A transport failure is a network problem; anything else got far enough
        // that the remote had an opinion.
        if err.is_connect() || err.is_timeout() || err.is_request() {
            Self::Network(strip_url(&err))
        } else {
            Self::Platform(strip_url(&err))
        }
    }
}

/// reqwest's Display embeds the full request URL, which for a token exchange can
/// carry query parameters we do not want in a stored error string shown in the UI.
fn strip_url(err: &reqwest::Error) -> String {
    let mut err_no_url = err.to_string();
    if let Some(url) = err.url() {
        err_no_url = err_no_url.replace(url.as_str(), url.host_str().unwrap_or("the server"));
    }
    err_no_url
}

impl From<keyring::Error> for AppError {
    fn from(err: keyring::Error) -> Self {
        match err {
            keyring::Error::NoEntry => {
                Self::Unauthorized("No stored credentials for this account.".into())
            }
            other => Self::Internal(format!("Credential store unavailable: {other}")),
        }
    }
}

pub type Result<T> = std::result::Result<T, AppError>;

/// Turns a non-2xx HTTP response into the right error class. Rate limits and 5xx
/// are `Platform` (retryable); 401/403 is `Unauthorized`; the rest is terminal.
pub fn from_status(status: u16, body: &str, platform: &str) -> AppError {
    let detail: String = body.chars().take(400).collect();
    match status {
        401 | 403 => AppError::Unauthorized(format!(
            "{platform} rejected the stored credentials ({status}). Reconnect the account. {detail}"
        )),
        404 => AppError::NotFound(format!("{platform} returned 404: {detail}")),
        409 => AppError::Conflict(format!("{platform} returned 409: {detail}")),
        429 => AppError::Platform(format!("{platform} rate-limited the request. {detail}")),
        500..=599 => AppError::Platform(format!("{platform} server error {status}. {detail}")),
        _ => AppError::InvalidInput(format!(
            "{platform} rejected the request ({status}): {detail}"
        )),
    }
}

/// `Display` for a boxed cause, used where a third-party error has no `From`.
pub fn internal(context: &str, cause: impl fmt::Display) -> AppError {
    AppError::Internal(format!("{context}: {cause}"))
}
