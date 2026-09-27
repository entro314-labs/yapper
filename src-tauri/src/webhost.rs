//! The companion web deployment, and the two things Meta needs that a desktop
//! app cannot provide on its own.
//!
//! Every other platform here is reachable from a binary on someone's laptop.
//! Meta is not, in two separate ways:
//!
//!   * **The redirect must be HTTPS.** Threads, Instagram and Facebook all
//!     reject `http://127.0.0.1:8917/callback`, so a Meta app registers
//!     `{base}/oauth/meta` instead. That page answers with a 302 to the loopback,
//!     which is where [`crate::oauth::handoff`] has been listening the whole
//!     time — the code still lands in the app, it just arrives via one hop.
//!   * **Media is fetched, not uploaded.** `POST /{id}/threads` and
//!     `POST /{ig-id}/media` take an `image_url` and go and GET it themselves.
//!     There is no byte-upload path for either, so an attachment sitting on a
//!     local disk is unreachable to them. [`upload`] parks it at a public URL
//!     first.
//!
//! Both come from ONE deployment, so this is one setting rather than two. The
//! Next.js app under `site/` in this repo is that deployment; its upload route
//! hands out a presigned Cloudflare R2 URL that the app uploads to directly,
//! and the object is served back on a public path.
//!
//! Objects are deliberately NOT deleted after a publish. Meta fetches an image
//! while the container is being created, but a video container is processed
//! asynchronously and may be read minutes later — deleting on our own schedule
//! would race that and break the post. The bucket's lifecycle rule is what
//! expires them, which is a job for the storage layer rather than for a
//! scheduler thread that might not be running.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};
use crate::http;
use crate::platforms::MediaItem;

/// The one credential-store item, holding both halves of the configuration.
/// Not a [`crate::platforms::AppCredentials`] because it is not a platform: all
/// three Meta adapters share this single deployment.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebHost {
    /// Origin of the deployment, without a trailing slash — `https://windbag.example`.
    pub base_url: String,
    /// The bearer token `site/` checks on its upload route. Shared secret rather
    /// than a signed request: the only caller is this app, and the route does
    /// nothing but accept bytes it will serve back.
    pub upload_token: String,
}

impl WebHost {
    /// The HTTPS redirect a Meta app must register, derived rather than typed —
    /// a user who pastes this by hand gets one shot at it, and a single wrong
    /// character fails the exchange with a message that names nothing useful.
    pub fn redirect_uri(&self) -> String {
        format!("{}/oauth/meta", self.base_url.trim_end_matches('/'))
    }
}

/// `None` when the user has not set one up. An ordinary state — it only stops
/// the three Meta adapters, and only Instagram cannot work around it at all.
pub fn load() -> Result<Option<WebHost>> {
    crate::secrets::load_web_host()
}

/// The configured host, or the error that says which setting is missing. Called
/// from `connect` and from `publish`, both of which are dead in the water
/// without it.
pub fn require() -> Result<WebHost> {
    load()?.ok_or_else(|| {
        AppError::InvalidInput(
            "Meta needs the Windbag web deployment: an HTTPS redirect to sign in with, and a \
             public URL to serve attachments from. Add it in Settings → Web deployment."
                .into(),
        )
    })
}

/// What one parked attachment came back as.
#[derive(Debug, Clone)]
pub struct Hosted {
    pub url: String,
}

/// What `/api/media` answers with: where to put the bytes, and where Meta will
/// find them afterwards.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Reservation {
    upload_url: String,
    url: String,
}

/// What the app declares before uploading. The upload URL comes back signed
/// for exactly this type and length, so R2 refuses anything else.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UploadRequest<'a> {
    content_type: &'a str,
    size: usize,
}

/// The slowest uplink the upload timeout still allows for: 128 KiB/s, poor but
/// real. The shared client's flat 60 s would cut off anything much above 7 MB
/// on an ordinary home connection.
const MIN_UPLOAD_BYTES_PER_SEC: u64 = 128 * 1024;

/// Parks one attachment at a public URL and returns it.
///
/// Two steps, because the bytes cannot pass through the deployment: a Vercel
/// function refuses any request body over 4.5 MB. So the deployment is asked
/// for a presigned R2 upload URL, and the file goes straight to R2.
pub fn upload(host: &WebHost, item: &MediaItem) -> Result<Hosted> {
    let endpoint = format!("{}/api/media", host.base_url.trim_end_matches('/'));
    let (status, body) = http::read_body(
        http::client()
            .post(&endpoint)
            .bearer_auth(&host.upload_token)
            .json(&UploadRequest {
                content_type: &item.mime,
                size: item.bytes.len(),
            })
            .send()?,
    );
    if !(200..300).contains(&status) {
        return Err(reservation_error(status, &body, &endpoint));
    }

    let reservation: Reservation = serde_json::from_str(&body).map_err(|e| {
        AppError::Platform(format!(
            "The web deployment returned an unreadable upload result: {e}"
        ))
    })?;
    if !reservation.url.starts_with("https://") {
        // Meta will not fetch a plain-HTTP media URL, and finding that out as a
        // container error names nothing useful.
        return Err(AppError::InvalidInput(format!(
            "The web deployment served the attachment over {} — Meta only fetches media \
             over HTTPS. Deploy it behind TLS.",
            reservation
                .url
                .split(':')
                .next()
                .unwrap_or("an unknown scheme")
        )));
    }

    // A `Vec` body gives reqwest an exact Content-Length, which is part of the
    // signature along with Content-Type.
    let (status, body) = http::read_body(
        http::client()
            .put(&reservation.upload_url)
            .header(reqwest::header::CONTENT_TYPE, &item.mime)
            .body(item.bytes.clone())
            .timeout(upload_timeout(item.bytes.len()))
            .send()?,
    );
    if !(200..300).contains(&status) {
        return Err(storage_error(status, &body));
    }
    Ok(Hosted {
        url: reservation.url,
    })
}

/// A refused reservation. Whatever the deployment rejects on its own judgement
/// — too large, a type it does not host, a malformed request — is the same
/// answer next time, so only a rate limit or a server fault is worth the
/// scheduler trying again.
fn reservation_error(status: u16, body: &str, endpoint: &str) -> AppError {
    let detail: String = body.trim().chars().take(400).collect();
    match status {
        401 | 403 => AppError::Unauthorized(
            "The web deployment rejected Windbag's upload token. Check it in \
             Settings → Web deployment."
                .into(),
        ),
        404 => AppError::InvalidInput(format!(
            "No upload route at {endpoint}. Check the base URL in \
             Settings → Web deployment, and that the site is deployed."
        )),
        429 | 500..=599 => AppError::Platform(format!(
            "The web deployment could not take the attachment right now ({status}): {detail}"
        )),
        _ => AppError::InvalidInput(format!(
            "The web deployment refused the attachment ({status}): {detail}"
        )),
    }
}

/// A refused upload to R2 itself. A 4xx there is a signature or policy refusal
/// of a URL minted a moment ago, which repeating will not change; throttling
/// and 5xx are R2 having a bad minute.
fn storage_error(status: u16, body: &str) -> AppError {
    let detail: String = body.trim().chars().take(400).collect();
    match status {
        408 | 429 | 500..=599 => AppError::Platform(format!(
            "The media store could not take the attachment right now ({status}): {detail}"
        )),
        _ => AppError::InvalidInput(format!(
            "The media store refused the attachment ({status}): {detail}"
        )),
    }
}

/// Long enough to push `len` bytes at [`MIN_UPLOAD_BYTES_PER_SEC`], on top of
/// the 60 s the shared client allows any request.
fn upload_timeout(len: usize) -> Duration {
    let len = u64::try_from(len).unwrap_or(u64::MAX);
    Duration::from_secs(60 + len / MIN_UPLOAD_BYTES_PER_SEC)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(base: &str) -> WebHost {
        WebHost {
            base_url: base.into(),
            upload_token: "t".into(),
        }
    }

    #[test]
    fn the_redirect_is_derived_from_the_base_url() {
        assert_eq!(
            host("https://windbag.example").redirect_uri(),
            "https://windbag.example/oauth/meta"
        );
    }

    #[test]
    fn a_trailing_slash_does_not_double_up() {
        // A pasted URL very often ends in one, and `//oauth/meta` would not match
        // the string registered on the Meta app.
        assert_eq!(
            host("https://windbag.example/").redirect_uri(),
            "https://windbag.example/oauth/meta"
        );
    }

    #[test]
    fn an_attachment_the_deployment_refuses_is_not_retried() {
        // 413 is what an oversized attachment gets. The old body upload mapped
        // it to a retryable error: five attempts, then an opaque failure.
        for status in [400, 413, 415] {
            let err =
                reservation_error(status, r#"{"error":"nope"}"#, "https://w.example/api/media");
            assert!(!err.is_retryable(), "{status} must be terminal: {err}");
            assert!(
                err.to_string().contains("nope"),
                "the reason must survive: {err}"
            );
        }
    }

    #[test]
    fn a_deployment_fault_is_retried() {
        for status in [429, 500, 502, 503] {
            assert!(
                reservation_error(status, "", "https://w.example/api/media").is_retryable(),
                "{status} should be retried"
            );
        }
    }

    #[test]
    fn a_bad_upload_token_points_at_the_setting() {
        let err = reservation_error(401, "", "https://w.example/api/media");
        assert!(matches!(err, AppError::Unauthorized(_)), "{err}");
        assert!(err.to_string().contains("Web deployment"), "{err}");
    }

    #[test]
    fn the_media_store_retries_only_what_can_fix_itself() {
        assert!(storage_error(503, "").is_retryable());
        assert!(storage_error(429, "").is_retryable());
        assert!(
            !storage_error(403, "<Code>SignatureDoesNotMatch</Code>").is_retryable(),
            "a refused signature is the same refusal next time"
        );
    }

    #[test]
    fn the_upload_timeout_grows_with_the_attachment() {
        assert_eq!(upload_timeout(0), Duration::from_secs(60));
        // 40 MB, the app's ceiling, gets a little over six minutes.
        assert_eq!(
            upload_timeout(40 * 1024 * 1024),
            Duration::from_secs(60 + 320)
        );
    }

    #[test]
    fn the_declared_upload_is_what_the_route_reads() {
        // `site/app/api/media/route.ts` reads exactly these two keys.
        let body = serde_json::to_value(UploadRequest {
            content_type: "video/mp4",
            size: 12,
        })
        .expect("serialize");
        assert_eq!(
            body,
            serde_json::json!({ "contentType": "video/mp4", "size": 12 })
        );
    }
}
