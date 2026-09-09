//! The three Meta destinations: Threads, Instagram and a Facebook Page.
//!
//! They are one family because they share a developer app, an error envelope
//! and a token lifecycle — and all three differ from the rest of this directory
//! in the same four ways:
//!
//!   1. **The redirect must be HTTPS.** A loopback URI is refused at the
//!      authorize step, so a Meta app registers the companion deployment's
//!      `/oauth/meta` and that bounces back to the loopback listener. See
//!      [`crate::webhost`].
//!   2. **There is no refresh token.** Meta issues a SHORT-lived token, which is
//!      traded once for a LONG-lived one (60 days), which is then extended by
//!      presenting itself. So `refresh` here re-presents the access token rather
//!      than exchanging a separate credential, and a token left unused past its
//!      expiry is dead — reconnecting is the only way back.
//!   3. **A dead token is a 400, not a 401.** Meta answers `code: 190` with HTTP
//!      400, which [`crate::error::from_status`] would file as terminal invalid
//!      input — leaving the account looking healthy while every post failed.
//!      [`map_error`] reads the envelope instead.
//!   4. **Media is fetched, not uploaded** (Threads and Instagram). The adapters
//!      park attachments through [`crate::webhost`] first.
//!
//! Everything a client secret touches stays in the user's own app: Meta has no
//! public-client flow for these, so `client_secret` is required rather than
//! optional, and shipping one in the binary was never an option.

use serde::Deserialize;

use crate::error::{AppError, Result};
use crate::http;

pub mod facebook;
pub mod instagram;
pub mod threads;

/// The Graph API version every Facebook and Instagram call is pinned to. Meta
/// supports a version for about two years, so unlike `LinkedIn` this does not
/// need to be user-editable — but it is one constant so the bump is one edit.
pub const GRAPH_VERSION: &str = "v25.0";

/// Meta's error envelope. Every failure from every one of these hosts is shaped
/// like this, which is what makes one mapper enough for three adapters.
#[derive(Debug, Deserialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Debug, Deserialize)]
struct ErrorBody {
    message: String,
    #[serde(default)]
    code: i64,
    #[serde(default)]
    error_subcode: i64,
    #[serde(default)]
    error_user_msg: Option<String>,
    /// Meta's OWN retry signal, and better than inferring one from the code —
    /// it is set on exactly the failures Meta considers worth trying again.
    #[serde(default)]
    is_transient: bool,
}

/// Classifies a Meta failure by its own error code rather than by HTTP status,
/// because Meta's status codes do not carry the distinction the scheduler needs.
///
/// The three outcomes that matter: `Unauthorized` flips the account into
/// "reconnect" in the UI, `Platform` is retried later, and everything else is
/// terminal and shown to the user as-is.
pub fn map_error(status: u16, body: &str, label: &str) -> AppError {
    let Ok(envelope) = serde_json::from_str::<ErrorEnvelope>(body) else {
        return crate::error::from_status(status, body, label);
    };
    let error = envelope.error;
    // `error_user_msg` is Meta's own human-readable text where it exists, and is
    // consistently better than `message`, which is written for a developer.
    let detail = error
        .error_user_msg
        .as_deref()
        .unwrap_or(&error.message)
        .trim()
        .to_string();

    match (error.code, error.error_subcode) {
        // The token is gone: expired past its 60 days, revoked, or invalidated by
        // a password change. None of these are fixed by retrying or by waiting.
        (190 | 102 | 463, _) => AppError::Unauthorized(format!(
            "{label} no longer accepts this connection: {detail} Reconnect the account."
        )),
        // The app itself was rejected — a wrong app id, or a wrong app secret.
        //
        // The detail is DELIBERATELY dropped here. Meta echoes the submitted
        // secret back inside this message ("Invalid client_secret: <secret>"),
        // and this string is stored on the failed target and shown in the UI.
        (101, _) => AppError::Unauthorized(format!(
            "{label} did not accept the app credentials. Check the app ID and secret in \
             Settings → Platform apps."
        )),
        // A sign-in code that was already used or has expired. Meta files this
        // under code 1, which is otherwise its "unknown, try again" bucket — so
        // without this arm a dead code looks retryable and never says what to do.
        (_, 36006) => AppError::InvalidInput(format!(
            "{label} rejected the sign-in code as already used or expired. Start the \
             connection again."
        )),
        // A permission the app was never granted, or that the user removed. Also
        // terminal, but the fix is the app's scopes rather than a fresh sign-in.
        (10 | 200..=299, _) => AppError::Unauthorized(format!(
            "{label} refused the request for lack of a permission: {detail} Check that your \
             Meta app has the required permissions and reconnect the account."
        )),
        // Throttling, in all the shapes Meta spells it. Retryable by definition.
        (4 | 17 | 32 | 613 | 80_000..=80_999, _) => {
            AppError::Platform(format!("{label} rate-limited the request: {detail}"))
        }
        // 1 is "unknown", 2 is "service temporarily unavailable". Meta also flags
        // these with `is_transient`, which is trusted first because it is the
        // signal Meta actually maintains.
        (1 | 2, _) => AppError::Platform(format!("{label} had a temporary failure: {detail}")),
        // Meta could not fetch the media URL it was handed. Terminal: the same
        // URL will fail the same way, and the user needs to hear about it.
        (9004 | 2_207_003 | 2_207_020, _) => AppError::InvalidInput(format!(
            "{label} could not fetch the attachment from the web deployment: {detail} \
             Check that Settings → Web deployment serves media publicly over HTTPS."
        )),
        _ if error.is_transient || (500..600).contains(&status) => {
            AppError::Platform(format!("{label} server error {status}: {detail}"))
        }
        _ => AppError::InvalidInput(format!("{label} rejected the request: {detail}")),
    }
}

/// A token grant. Meta's short-lived exchange answers without `expires_in`,
/// which is why it is optional here rather than being assumed present.
#[derive(Debug, Deserialize)]
pub struct Grant {
    pub access_token: String,
    #[serde(default)]
    pub expires_in: Option<i64>,
}

impl Grant {
    /// RFC 3339, or `None` for a grant with no clock on it. The floor keeps a
    /// nonsense-small `expires_in` from making a token look already dead.
    pub fn expires_at(&self) -> Option<String> {
        self.expires_in.map(|seconds| {
            (chrono::Utc::now() + chrono::Duration::seconds(seconds.max(60))).to_rfc3339()
        })
    }
}

/// A Meta token exchange: a GET carrying its parameters in the query string.
///
/// Every other platform here POSTs a form to its token endpoint. Meta's
/// long-lived exchange and its refresh are both GETs, and sending them as a form
/// post returns a parameter error rather than a token.
pub fn token_get(url: &str, params: &[(&str, &str)], label: &str) -> Result<Grant> {
    let (status, body) = http::read_body(http::client().get(url).query(params).send()?);
    if !(200..300).contains(&status) {
        return Err(map_error(status, &body, label));
    }
    serde_json::from_str(&body)
        .map_err(|e| AppError::Platform(format!("{label} returned an unreadable token: {e}")))
}

/// A form POST to a Meta endpoint, returning the parsed JSON body.
pub fn post_form(url: &str, form: &[(&str, &str)], label: &str) -> Result<serde_json::Value> {
    let (status, body) = http::read_body(http::client().post(url).form(form).send()?);
    if !(200..300).contains(&status) {
        return Err(map_error(status, &body, label));
    }
    serde_json::from_str(&body)
        .map_err(|e| AppError::Platform(format!("{label} returned an unreadable response: {e}")))
}

/// A GET against a Graph host, returning the parsed JSON body.
pub fn get_json(url: &str, params: &[(&str, &str)], label: &str) -> Result<serde_json::Value> {
    let (status, body) = http::read_body(http::client().get(url).query(params).send()?);
    if !(200..300).contains(&status) {
        return Err(map_error(status, &body, label));
    }
    serde_json::from_str(&body)
        .map_err(|e| AppError::Platform(format!("{label} returned an unreadable response: {e}")))
}

/// The `id` out of a Meta response, or the error that says it was not there.
pub fn id_of(value: &serde_json::Value, label: &str, what: &str) -> Result<String> {
    value
        .get("id")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| AppError::Platform(format!("{label} returned no id for the {what}.")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(code: i64, message: &str) -> String {
        format!(r#"{{"error":{{"message":"{message}","type":"OAuthException","code":{code}}}}}"#)
    }

    #[test]
    fn an_expired_token_is_unauthorized_even_though_meta_sends_a_400() {
        // The whole point of this mapper: `from_status` would call a 400
        // terminal-but-fine, leaving the account looking healthy for ever.
        let err = map_error(400, &envelope(190, "Session has expired"), "Threads");
        assert!(matches!(err, AppError::Unauthorized(_)), "{err}");
        assert!(!err.is_retryable());
    }

    #[test]
    fn a_rate_limit_is_retryable() {
        let err = map_error(
            400,
            &envelope(4, "Application request limit reached"),
            "Instagram",
        );
        assert!(err.is_retryable(), "{err}");
    }

    #[test]
    fn a_missing_permission_is_terminal_and_says_so() {
        let err = map_error(403, &envelope(200, "Permissions error"), "Facebook Page");
        assert!(matches!(err, AppError::Unauthorized(_)), "{err}");
        assert!(err.to_string().contains("permission"));
    }

    #[test]
    fn an_unfetchable_attachment_names_the_web_deployment() {
        let err = map_error(
            400,
            &envelope(9004, "The media could not be fetched"),
            "Instagram",
        );
        assert!(
            !err.is_retryable(),
            "the same URL fails the same way: {err}"
        );
        assert!(err.to_string().contains("Web deployment"), "{err}");
    }

    // ── Payloads captured from Meta on 9 September 2026 ─────────────────────
    //
    // Verbatim, so these test what Meta actually sends rather than what this
    // module assumed it sends. Two of the three corrected a real misreading.

    #[test]
    fn a_spent_authorization_code_is_terminal_despite_arriving_as_code_1() {
        let body = r#"{"error":{"message":"Invalid verification code","type":"OAuthException",
                       "code":1,"error_subcode":36006,"fbtrace_id":"A2qLpDNt"}}"#;
        let err = map_error(400, body, "Threads");
        assert!(
            !err.is_retryable(),
            "code 1 is Meta's transient bucket, but a spent code never becomes valid: {err}"
        );
        assert!(err.to_string().contains("again"), "{err}");
    }

    #[test]
    fn a_rejected_app_secret_never_reaches_the_stored_error() {
        // Meta echoes the submitted secret straight back in this message. It is
        // written to the failed target and shown in the UI, so it must not
        // survive the mapping.
        let body = r#"{"error":{"message":"Invalid client_secret: hunter2secretvalue",
                       "type":"OAuthException","code":101,"fbtrace_id":"AbadH0"}}"#;
        let err = map_error(400, body, "Threads");
        assert!(
            !err.to_string().contains("hunter2secretvalue"),
            "the app secret leaked into a stored error: {err}"
        );
        assert!(matches!(err, AppError::Unauthorized(_)), "{err}");
    }

    #[test]
    fn metas_own_transient_flag_is_trusted() {
        let body = r#"{"error":{"message":"Service temporarily unavailable",
                       "type":"THApiException","is_transient":true,"code":2,
                       "fbtrace_id":"AIGU"}}"#;
        assert!(map_error(400, body, "Threads").is_retryable());
    }

    #[test]
    fn a_body_that_is_not_a_meta_envelope_falls_back_to_the_status() {
        let err = map_error(503, "<html>gateway</html>", "Threads");
        assert!(err.is_retryable(), "{err}");
    }

    #[test]
    fn error_user_msg_wins_over_the_developer_message() {
        let body = r#"{"error":{"message":"dev text","code":100,"error_user_msg":"human text"}}"#;
        assert!(
            map_error(400, body, "Threads")
                .to_string()
                .contains("human text")
        );
    }

    #[test]
    fn a_grant_without_an_expiry_has_no_deadline() {
        let grant = Grant {
            access_token: "t".into(),
            expires_in: None,
        };
        assert!(grant.expires_at().is_none());
    }
}
