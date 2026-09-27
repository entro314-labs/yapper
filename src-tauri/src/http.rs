//! The one HTTP client. Every platform call goes through it, which is what keeps
//! the webview's `connect-src` limited to Tauri's own IPC origin: the renderer
//! never talks to a social API directly, so there is no CORS to negotiate and no
//! token that has to be readable from JavaScript.
//!
//! Blocking on purpose. The scheduler is a plain worker thread and each adapter
//! call is one short request/response; an async runtime would add a dependency
//! and buy nothing.

use std::sync::OnceLock;
use std::time::Duration;

/// Sent on every request. Reddit in particular rejects a generic or absent
/// user-agent outright, and asks that the app identify itself by name.
pub fn user_agent() -> &'static str {
    static UA: OnceLock<String> = OnceLock::new();
    UA.get_or_init(|| {
        format!(
            "Windbag/{} (desktop; +https://github.com/entro314-labs/yapper)",
            env!("CARGO_PKG_VERSION")
        )
    })
}

pub fn client() -> &'static reqwest::blocking::Client {
    static CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .user_agent(user_agent())
            .timeout(Duration::from_secs(60))
            .connect_timeout(Duration::from_secs(15))
            .build()
            // A client that cannot be configured is still better than no client:
            // the default one carries no user-agent, which Reddit will reject
            // with a clear message rather than failing mysteriously here.
            .unwrap_or_else(|_| reqwest::blocking::Client::new())
    })
}

/// The timeout for a request whose body is `bytes` of media. The shared
/// client's 60 s covers the whole exchange, which a 40 MB video on an ordinary
/// uplink cannot finish inside — it failed as a timeout on every try. This
/// keeps the 60 s for the round trip and adds the time to push the body at
/// 128 KB/s (about 1 Mbit/s).
pub fn upload_timeout(bytes: usize) -> Duration {
    const BYTES_PER_SECOND: u64 = 128 * 1024;
    let bytes = u64::try_from(bytes).unwrap_or(u64::MAX);
    Duration::from_secs(60 + bytes / BYTES_PER_SECOND)
}

/// Reads a response body once, whatever the status, so an error path can quote
/// what the remote actually said. Bodies here are small JSON documents.
pub fn read_body(response: reqwest::blocking::Response) -> (u16, String) {
    let status = response.status().as_u16();
    let body = response.text().unwrap_or_default();
    (status, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_upload_timeout_grows_with_the_body() {
        assert_eq!(upload_timeout(0), Duration::from_secs(60));
        let video = upload_timeout(40 * 1024 * 1024);
        assert!(
            video >= Duration::from_mins(5),
            "40 MB must get minutes, not the shared 60 s: {video:?}"
        );
    }
}
