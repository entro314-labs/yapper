//! OAuth 2.0 Authorization Code + PKCE as a native public client, shared by every
//! platform that uses a browser handoff.
//!
//! Flow: generate verifier/challenge → bind a loopback listener → open the system
//! browser at the provider's authorize URL → catch the redirect → exchange the
//! code at the token endpoint.
//!
//! ONE redirect URI for most providers — `http://127.0.0.1:8917/callback` — so a
//! user registering their own developer app pastes the same string every time.
//! It must be registered VERBATIM on the provider's side, including the port.
//!
//! META IS THE EXCEPTION. Threads, Instagram and Facebook reject a plain-HTTP
//! redirect outright, so those apps register an HTTPS bounce that answers with a
//! 302 back to the loopback. [`handoff`] therefore takes the URI to ADVERTISE
//! separately from where it listens: the listener below catches the code either
//! way, because the bounce lands the browser on the same loopback URL in the end.
//!
//! The listener binds BEFORE the browser opens, so a port conflict fails
//! immediately instead of after the user has already signed in and been
//! redirected to nothing.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{Duration as ChronoDuration, Utc};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::error::{AppError, Result, from_status, internal};
use crate::http;
use crate::platforms::{AccountSecret, PlatformId};

pub const REDIRECT_PORT: u16 = 8917;
pub const REDIRECT_URI: &str = "http://127.0.0.1:8917/callback";

/// Five minutes: long enough to find a password manager and clear a 2FA prompt,
/// short enough that an abandoned flow releases the port on its own.
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);

pub struct OAuthConfig<'a> {
    pub platform: PlatformId,
    /// Owned rather than borrowed because Mastodon's endpoints are per-instance
    /// and have to be built at call time.
    pub authorize_url: String,
    pub token_url: String,
    pub client_id: &'a str,
    /// Present only for providers that insist on a confidential client. Shipping
    /// one inside a distributed binary would not be a secret, which is exactly
    /// why the user registers their own app and pastes their own.
    pub client_secret: Option<&'a str>,
    pub scopes: &'a str,
    /// Provider-specific authorize-URL parameters (Reddit's `duration=permanent`).
    pub extra_authorize_params: &'a [(&'a str, &'a str)],
    /// Send the client credentials as HTTP Basic rather than form fields. Reddit
    /// requires this; the others accept form fields.
    pub basic_auth: bool,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    /// Seconds. Absent on providers that issue non-expiring tokens.
    expires_in: Option<i64>,
    scope: Option<String>,
}

impl TokenResponse {
    fn into_secret(self, previous_refresh: Option<String>) -> (AccountSecret, Option<String>) {
        let expires_at = self
            .expires_in
            .map(|seconds| (Utc::now() + ChronoDuration::seconds(seconds.max(60))).to_rfc3339());
        let scope = self.scope;
        (
            AccountSecret {
                access_token: self.access_token,
                // A provider that rotates refresh tokens sends a new one; one that
                // does not sends nothing, and dropping the old one on a refresh
                // would silently un-authorize the account on the NEXT publish.
                refresh_token: self.refresh_token.or(previous_refresh),
                expires_at,
                extra: serde_json::Value::Null,
            },
            scope,
        )
    }
}

/// What one browser handoff produced.
pub struct Handoff {
    pub code: String,
    /// The PKCE verifier that has to travel with the code exchange. `None` when
    /// the flow ran without PKCE — Meta's providers do not offer it and reject
    /// an exchange carrying one.
    pub verifier: Option<String>,
}

/// One browser leg of an authorization-code flow: bind the loopback listener,
/// open the provider's consent screen, and hand back the code it redirects with.
///
/// `advertised_redirect` is what the PROVIDER is told, and must match the app
/// registration exactly. It is NOT necessarily where this listens — a Meta app
/// registers an HTTPS bounce, which 302s the browser to the loopback below.
///
/// Split out of [`authorize`] because Meta's token endpoints are not a standard
/// exchange (a GET with query parameters, a response carrying `user_id` and no
/// `expires_in`), so those adapters need the code without the exchange.
pub fn handoff(
    platform: PlatformId,
    authorize_url: &str,
    client_id: &str,
    advertised_redirect: &str,
    scopes: &str,
    extra_params: &[(&str, &str)],
    pkce: bool,
) -> Result<Handoff> {
    let listener = TcpListener::bind(("127.0.0.1", REDIRECT_PORT)).map_err(|e| {
        AppError::Conflict(format!(
            "Port {REDIRECT_PORT} is unavailable ({e}). Another sign-in may still be open — \
             close it, or quit whatever is using the port, and try again."
        ))
    })?;

    let verifier = pkce.then(|| b64url(&rand::random::<[u8; 32]>()));
    let state = b64url(&rand::random::<[u8; 16]>());

    let mut url = url::Url::parse(authorize_url)
        .map_err(|e| internal("The authorize URL is malformed", e))?;
    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("response_type", "code")
            .append_pair("client_id", client_id)
            .append_pair("redirect_uri", advertised_redirect)
            .append_pair("scope", scopes)
            .append_pair("state", &state);
        if let Some(verifier) = &verifier {
            let challenge = b64url(Sha256::digest(verifier.as_bytes()).as_slice());
            query
                .append_pair("code_challenge", &challenge)
                .append_pair("code_challenge_method", "S256");
        }
        for (key, value) in extra_params {
            query.append_pair(key, value);
        }
    }

    opener::open_browser(url.as_str())
        .map_err(|e| AppError::Internal(format!("Could not open the browser: {e}")))?;

    let code = wait_for_code(&listener, &state, platform)?;
    Ok(Handoff { code, verifier })
}

/// Runs the whole flow and blocks until it resolves. Call it from a worker
/// thread — it opens a browser and waits on a human.
///
/// Returns the tokens and the scope string the provider actually granted, which
/// can be narrower than what was asked for.
pub fn authorize(config: &OAuthConfig<'_>) -> Result<(AccountSecret, Option<String>)> {
    let handoff = handoff(
        config.platform,
        &config.authorize_url,
        config.client_id,
        REDIRECT_URI,
        config.scopes,
        config.extra_authorize_params,
        true,
    )?;
    let verifier = handoff.verifier.unwrap_or_default();
    exchange(
        config,
        &[
            ("grant_type", "authorization_code"),
            ("code", &handoff.code),
            ("redirect_uri", REDIRECT_URI),
            ("code_verifier", &verifier),
        ],
        None,
    )
}

/// Trades a refresh token for a fresh access token. `previous` is carried through
/// so a provider that does not rotate refresh tokens keeps the one it issued.
pub fn refresh(config: &OAuthConfig<'_>, refresh_token: &str) -> Result<AccountSecret> {
    let (secret, _) = exchange(
        config,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ],
        Some(refresh_token.to_string()),
    )?;
    Ok(secret)
}

fn exchange(
    config: &OAuthConfig<'_>,
    form: &[(&str, &str)],
    previous_refresh: Option<String>,
) -> Result<(AccountSecret, Option<String>)> {
    let mut fields: Vec<(&str, &str)> = form.to_vec();
    let mut request = http::client().post(&config.token_url);

    if config.basic_auth {
        request = request.basic_auth(config.client_id, config.client_secret);
    } else {
        fields.push(("client_id", config.client_id));
        if let Some(secret) = config.client_secret {
            fields.push(("client_secret", secret));
        }
    }

    let (status, body) = http::read_body(request.form(&fields).send()?);
    if !(200..300).contains(&status) {
        // A rejected exchange is almost always a mis-registered app rather than a
        // transient fault, so it must not come back as something retryable.
        return Err(match from_status(status, &body, config.platform.label()) {
            AppError::Platform(message) | AppError::Network(message) => AppError::Unauthorized(
                format!("{message} Check the client id and the redirect URI on your app."),
            ),
            other => other,
        });
    }

    let token: TokenResponse = serde_json::from_str(&body).map_err(|e| {
        AppError::Platform(format!(
            "{} returned a token response Windbag could not read: {e}",
            config.platform.label()
        ))
    })?;
    Ok(token.into_secret(previous_refresh))
}

fn b64url(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Accepts loopback connections until the redirect arrives, then answers with a
/// small page telling the user to come back. Non-blocking accept so the timeout
/// is enforceable.
fn wait_for_code(
    listener: &TcpListener,
    expected_state: &str,
    platform: PlatformId,
) -> Result<String> {
    listener
        .set_nonblocking(true)
        .map_err(|e| internal("Preparing the sign-in listener", e))?;
    let deadline = Instant::now() + CALLBACK_TIMEOUT;

    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let Some(target) = request_target(&mut stream) else {
                    continue;
                };
                // Browsers also ask for /favicon.ico — only the callback counts.
                if !target.starts_with("/callback") {
                    respond(&mut stream, "Not found.");
                    continue;
                }
                let parsed = url::Url::parse(&format!("http://127.0.0.1{target}"))
                    .map_err(|e| AppError::Platform(format!("Malformed callback: {e}")))?;
                let param = |name: &str| {
                    parsed
                        .query_pairs()
                        .find(|(key, _)| key == name)
                        .map(|(_, value)| value.into_owned())
                };

                if let Some(error) = param("error") {
                    respond(
                        &mut stream,
                        "Sign-in was cancelled. You can close this tab.",
                    );
                    return Err(AppError::Unauthorized(match error.as_str() {
                        "access_denied" => format!(
                            "Sign-in was cancelled on the {} consent screen.",
                            platform.label()
                        ),
                        other => format!("{} returned an error: {other}", platform.label()),
                    }));
                }
                // The state check is the CSRF defence for the whole flow: without
                // it an attacker-supplied code could be exchanged into this app's
                // account. A mismatch aborts rather than retries.
                if param("state").as_deref() != Some(expected_state) {
                    respond(&mut stream, "State mismatch — sign-in aborted.");
                    return Err(AppError::Unauthorized(
                        "The sign-in callback did not match the request Windbag started. \
                         Nothing was connected; try again."
                            .into(),
                    ));
                }
                let Some(code) = param("code") else {
                    respond(&mut stream, "Missing authorization code.");
                    return Err(AppError::Unauthorized(
                        "The callback arrived without an authorization code.".into(),
                    ));
                };
                respond(
                    &mut stream,
                    "You're connected. Close this tab and return to Windbag.",
                );
                return Ok(code);
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(AppError::Unauthorized(
                        "Timed out waiting for the browser sign-in (5 minutes).".into(),
                    ));
                }
                std::thread::sleep(Duration::from_millis(150));
            }
            Err(e) => return Err(internal("The sign-in listener failed", e)),
        }
    }
}

/// The request target from a request line: `GET /callback?... HTTP/1.1`.
fn request_target(stream: &mut TcpStream) -> Option<String> {
    let mut buffer = [0u8; 8192];
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let read = stream.read(&mut buffer).ok()?;
    let request = String::from_utf8_lossy(&buffer[..read]);
    request
        .lines()
        .next()?
        .split_whitespace()
        .nth(1)
        .map(str::to_owned)
}

fn respond(stream: &mut TcpStream, message: &str) {
    let body = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>Windbag</title>\
         <body style=\"font-family:system-ui;display:grid;place-items:center;height:100vh;\
         margin:0;background:#131820;color:#eef2f6\">\
         <p style=\"font-size:17px\">{message}</p></body>"
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
}

/// True when a token is inside the two-minute window where it should be renewed
/// before use. A token with no expiry never needs one.
pub fn needs_refresh(expires_at: Option<&str>) -> bool {
    let Some(raw) = expires_at else { return false };
    match crate::db::parse_rfc3339(raw) {
        Ok(at) => at - Utc::now() <= ChronoDuration::seconds(120),
        // An unreadable expiry is treated as expired: refreshing needlessly costs
        // one request, while skipping a needed refresh loses a scheduled post.
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_with_no_expiry_never_needs_refreshing() {
        assert!(!needs_refresh(None));
    }

    #[test]
    fn refresh_is_due_inside_the_two_minute_window() {
        let soon = (Utc::now() + ChronoDuration::seconds(30)).to_rfc3339();
        let later = (Utc::now() + ChronoDuration::hours(1)).to_rfc3339();
        assert!(needs_refresh(Some(&soon)));
        assert!(!needs_refresh(Some(&later)));
    }

    #[test]
    fn an_unreadable_expiry_is_treated_as_expired() {
        assert!(needs_refresh(Some("not a timestamp")));
    }

    #[test]
    fn a_refresh_that_returns_no_new_token_keeps_the_old_one() {
        let response = TokenResponse {
            access_token: "new".into(),
            refresh_token: None,
            expires_in: Some(3600),
            scope: None,
        };
        let (secret, _) = response.into_secret(Some("original".into()));
        assert_eq!(secret.refresh_token.as_deref(), Some("original"));
    }

    #[test]
    fn a_rotated_refresh_token_replaces_the_old_one() {
        let response = TokenResponse {
            access_token: "new".into(),
            refresh_token: Some("rotated".into()),
            expires_in: Some(3600),
            scope: None,
        };
        let (secret, _) = response.into_secret(Some("original".into()));
        assert_eq!(secret.refresh_token.as_deref(), Some("rotated"));
    }
}
