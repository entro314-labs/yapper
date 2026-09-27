//! One error type for every Tauri command.
//!
//! Serializes as `"[CODE] message"`. The code half is a CONTRACT with the
//! renderer: `lib/query/client.ts` refuses to retry `NOT_FOUND`,
//! `INVALID_INPUT`, `UNAUTHORIZED` and `CONFLICT` because none of them fix
//! themselves, and `UNAUTHORIZED` is what flips an account into the
//! "reconnect" state in the UI. Adding a variant means deciding which of those
//! two behaviours it wants. `UNCONFIRMED` is on the no-retry list too.

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
    /// The remote said no. Retryable: 5xx and other passing failures live here.
    #[error("[PLATFORM] {0}")]
    Platform(String),
    /// The remote asked us to slow down. Retryable, and not before
    /// `retry_after` when the remote said how long — the scheduler waits the
    /// longer of that and its own backoff.
    #[error("[RATE_LIMITED] {message}")]
    RateLimited {
        message: String,
        retry_after: Option<std::time::Duration>,
    },
    /// Could not reach the remote at all. Always retryable.
    #[error("[NETWORK] {0}")]
    Network(String),
    /// Ours: a broken store, an unreachable keychain, a bug.
    #[error("[INTERNAL] {0}")]
    Internal(String),
    /// The request that makes a post public was sent and its answer was lost —
    /// a timeout after connecting, a dropped connection, a success reply with
    /// no readable id. The post may be live. Terminal: only the user can check,
    /// and an automatic retry is how one post becomes two.
    #[error("[UNCONFIRMED] {0}")]
    Unconfirmed(String),
}

impl AppError {
    /// Whether the scheduler should try this target again later. Everything the
    /// user must act on (bad credentials, an over-length post) is terminal;
    /// everything the world might fix on its own is not.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Platform(_) | Self::RateLimited { .. } | Self::Network(_)
        )
    }

    /// How long the remote said to wait before trying again, if it said.
    pub fn retry_after(&self) -> Option<std::time::Duration> {
        match self {
            Self::RateLimited { retry_after, .. } => *retry_after,
            _ => None,
        }
    }

    /// Attaches the wait a response's headers named to a rate limit. Headers
    /// are read before the body consumes the response, so the error is built
    /// first and told the wait after.
    #[must_use]
    pub fn with_retry_after(self, delay: Option<std::time::Duration>) -> Self {
        match self {
            Self::RateLimited { message, .. } => Self::RateLimited {
                message,
                retry_after: delay,
            },
            other => other,
        }
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

/// Classifies a failed send of the request that makes a post public.
///
/// Only a failure to CONNECT (or to build the request at all) proves nothing
/// reached the remote, so only that stays a retryable [`AppError::Network`].
/// Anything after the connection opened — a timeout waiting for the answer, a
/// reset mid-response — may have come after the platform created the post.
pub fn after_send(err: reqwest::Error) -> AppError {
    if err.is_connect() || err.is_builder() {
        return AppError::from(err);
    }
    AppError::Unconfirmed(format!(
        "The connection failed after the post was sent ({}), so it may have gone out. \
         Check whether it did before retrying.",
        strip_url(&err)
    ))
}

/// A 2xx to the publishing request whose body did not say what was created.
/// The platform accepted it, so the post is probably live — see
/// [`AppError::Unconfirmed`] for why that is terminal.
pub fn unreadable_after_send(platform: &str, cause: impl fmt::Display) -> AppError {
    AppError::Unconfirmed(format!(
        "{platform} accepted the post but its reply could not be read ({cause}), so it has \
         probably gone out. Check whether it did before retrying."
    ))
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
/// are `Platform` (retryable); 401 is `Unauthorized`; the rest is terminal.
///
/// 403 is `Unauthorized` only when the body says the TOKEN is the problem. A
/// 403 otherwise means a working token was refused this one thing — a
/// subreddit ban, an instance limit, a permission the developer app lacks —
/// and flagging the account for reconnection would send the user to fix
/// something that is not broken.
pub fn from_status(status: u16, body: &str, platform: &str) -> AppError {
    let detail: String = body.chars().take(400).collect();
    match status {
        401 => AppError::Unauthorized(format!(
            "{platform} rejected the stored credentials ({status}). Reconnect the account. {detail}"
        )),
        403 if names_the_token(body) => AppError::Unauthorized(format!(
            "{platform} rejected the stored credentials ({status}). Reconnect the account. {detail}"
        )),
        403 => AppError::InvalidInput(format!("{platform} refused this request (403): {detail}")),
        404 => AppError::NotFound(format!("{platform} returned 404: {detail}")),
        409 => AppError::Conflict(format!("{platform} returned 409: {detail}")),
        429 => AppError::RateLimited {
            message: format!("{platform} rate-limited the request. {detail}"),
            retry_after: None,
        },
        500..=599 => AppError::Platform(format!("{platform} server error {status}. {detail}")),
        _ => AppError::InvalidInput(format!(
            "{platform} rejected the request ({status}): {detail}"
        )),
    }
}

/// Whether a 403 body blames the token rather than the request: RFC 6750's
/// `invalid_token` and `insufficient_scope` (a reconnect is what grants a
/// missing scope), Mastodon's scope wording, X's `Unsupported Authentication`,
/// and plain revoked/expired wording.
fn names_the_token(body: &str) -> bool {
    let lowered = body.to_ascii_lowercase();
    [
        "invalid_token",
        "invalid token",
        "insufficient_scope",
        "outside the authorized scopes",
        "unsupported authentication",
        "revoked",
        "token expired",
        "expired token",
        "token has expired",
    ]
    .iter()
    .any(|phrase| lowered.contains(phrase))
}

/// `Display` for a boxed cause, used where a third-party error has no `From`.
pub fn internal(context: &str, cause: impl fmt::Display) -> AppError {
    AppError::Internal(format!("{context}: {cause}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quick_client() -> reqwest::blocking::Client {
        reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_millis(300))
            .build()
            .expect("client")
    }

    #[test]
    fn a_refused_connection_never_reached_the_platform_and_stays_retryable() {
        // Bind then drop: the port is closed, so the connect itself fails.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .expect("bind")
            .local_addr()
            .expect("addr")
            .port();
        let err = quick_client()
            .post(format!("http://127.0.0.1:{port}/"))
            .send()
            .expect_err("nothing listens there");
        let err = after_send(err);
        assert!(err.is_retryable(), "{err}");
    }

    #[test]
    fn a_timeout_after_connecting_may_have_posted_and_is_terminal() {
        // The server accepts the connection and never answers — exactly what a
        // platform that created the post and then stalled looks like.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let holder = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            std::thread::sleep(std::time::Duration::from_millis(800));
            drop(stream);
        });
        let err = quick_client()
            .post(format!("http://127.0.0.1:{port}/"))
            .body("post")
            .send()
            .expect_err("no answer comes");
        let err = after_send(err);
        assert!(matches!(err, AppError::Unconfirmed(_)), "{err}");
        assert!(!err.is_retryable());
        assert!(err.to_string().contains("before retrying"), "{err}");
        holder.join().expect("holder");
    }

    #[test]
    fn an_unreadable_success_is_terminal() {
        let err = unreadable_after_send("Reddit", "EOF");
        assert!(!err.is_retryable(), "{err}");
    }

    #[test]
    fn a_401_is_a_credential_problem() {
        assert!(matches!(
            from_status(401, "", "Reddit"),
            AppError::Unauthorized(_)
        ));
    }

    #[test]
    fn a_403_about_permission_is_terminal_but_leaves_the_account_alone() {
        // A subreddit ban, an instance limit, a LinkedIn product the app lacks:
        // the token works, so flagging the account for reconnection sends the
        // user to fix the wrong thing.
        for (platform, body) in [
            ("Reddit", r#"{"message":"Forbidden","error":403}"#),
            (
                "Mastodon",
                r#"{"error":"Your account is currently limited"}"#,
            ),
            (
                "LinkedIn",
                r#"{"status":403,"serviceErrorCode":100,"code":"ACCESS_DENIED"}"#,
            ),
        ] {
            let err = from_status(403, body, platform);
            assert!(matches!(err, AppError::InvalidInput(_)), "{err}");
            assert!(!err.is_retryable(), "{err}");
        }
    }

    #[test]
    fn a_403_that_names_the_token_still_asks_for_a_reconnect() {
        for body in [
            r#"{"error":"invalid_token"}"#,
            r#"{"error":"This action is outside the authorized scopes"}"#,
            r#"{"error":"insufficient_scope"}"#,
            r#"{"detail":"Unsupported Authentication"}"#,
            r#"{"message":"The access token has been revoked"}"#,
            r#"{"message":"Token expired"}"#,
        ] {
            let err = from_status(403, body, "Mastodon");
            assert!(matches!(err, AppError::Unauthorized(_)), "{body}: {err}");
        }
    }
}
