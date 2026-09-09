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
//! Next.js app under `site/` in this repo is that deployment; the upload route
//! writes to Cloudflare R2 and serves the object back on a public path.
//!
//! Objects are deliberately NOT deleted after a publish. Meta fetches an image
//! while the container is being created, but a video container is processed
//! asynchronously and may be read minutes later — deleting on our own schedule
//! would race that and break the post. The bucket's lifecycle rule is what
//! expires them, which is a job for the storage layer rather than for a
//! scheduler thread that might not be running.

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
///
/// The route also answers with the object key. It is not read here — nothing
/// deletes, because a bucket lifecycle rule does that — so it is not modelled.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hosted {
    pub url: String,
}

/// Parks one attachment at a public URL and returns it.
///
/// Raw body rather than multipart: the route takes exactly one file and the
/// content type carries the only other thing it needs, so a multipart envelope
/// would be ceremony on both sides.
pub fn upload(host: &WebHost, item: &MediaItem) -> Result<Hosted> {
    let endpoint = format!("{}/api/media", host.base_url.trim_end_matches('/'));
    let (status, body) = http::read_body(
        http::client()
            .post(&endpoint)
            .bearer_auth(&host.upload_token)
            .header(reqwest::header::CONTENT_TYPE, &item.mime)
            .body(item.bytes.clone())
            .send()?,
    );

    if !(200..300).contains(&status) {
        return Err(match status {
            401 | 403 => AppError::Unauthorized(
                "The web deployment rejected Windbag's upload token. Check it in \
                 Settings → Web deployment."
                    .into(),
            ),
            404 => AppError::InvalidInput(format!(
                "No upload route at {endpoint}. Check the base URL in \
                 Settings → Web deployment, and that the site is deployed."
            )),
            _ => AppError::Platform(format!(
                "The web deployment could not store the attachment ({status}): {}",
                body.trim()
            )),
        });
    }

    let hosted: Hosted = serde_json::from_str(&body).map_err(|e| {
        AppError::Platform(format!(
            "The web deployment returned an unreadable upload result: {e}"
        ))
    })?;
    if !hosted.url.starts_with("https://") {
        // Meta will not fetch a plain-HTTP media URL, and finding that out as a
        // container error names nothing useful.
        return Err(AppError::InvalidInput(format!(
            "The web deployment served the attachment over {} — Meta only fetches media \
             over HTTPS. Deploy it behind TLS.",
            hosted.url.split(':').next().unwrap_or("an unknown scheme")
        )));
    }
    Ok(hosted)
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
}
