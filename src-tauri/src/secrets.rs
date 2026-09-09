//! Credential custody. Everything sensitive lives in the OS credential store —
//! Keychain on macOS, Credential Manager on Windows, Secret Service on Linux —
//! and nothing sensitive is ever written to the SQLite store next to it.
//!
//! Three kinds of item, all under the `windbag` service:
//!   * `acct:<platform>:<remote-id>` — one connected account's tokens.
//!   * `app:<platform>[:<instance>]` — the user's OWN developer app for a
//!     platform. Instance-scoped for Mastodon, where an app registration is
//!     only valid on the server that issued it.
//!   * `web-host` — the companion deployment Meta needs for its HTTPS redirect
//!     and its media URLs. One item rather than one per Meta platform, because
//!     it is one deployment.
//!
//! Reads happen at publish time rather than at launch, so a run that never posts
//! never touches the keychain and never raises a prompt.

use keyring::Entry;

use crate::error::{AppError, Result};
use crate::platforms::{AccountSecret, AppCredentials, PlatformId};
use crate::webhost::WebHost;

const SERVICE: &str = "windbag";
const WEB_HOST_KEY: &str = "web-host";

fn entry(key: &str) -> Result<Entry> {
    Entry::new(SERVICE, key).map_err(Into::into)
}

fn account_key(platform: PlatformId, remote_id: &str) -> String {
    format!("acct:{platform}:{remote_id}")
}

fn app_key(platform: PlatformId, instance: Option<&str>) -> String {
    match instance {
        Some(host) => format!("app:{platform}:{host}"),
        None => format!("app:{platform}"),
    }
}

pub fn load_account_secret(platform: PlatformId, remote_id: &str) -> Result<AccountSecret> {
    let raw = entry(&account_key(platform, remote_id))?
        .get_password()
        .map_err(|err| match err {
            keyring::Error::NoEntry => AppError::Unauthorized(format!(
                "No stored credentials for {} — reconnect the account.",
                platform.label()
            )),
            other => other.into(),
        })?;
    serde_json::from_str(&raw).map_err(Into::into)
}

pub fn store_account_secret(
    platform: PlatformId,
    remote_id: &str,
    secret: &AccountSecret,
) -> Result<()> {
    entry(&account_key(platform, remote_id))?
        .set_password(&serde_json::to_string(secret)?)
        .map_err(Into::into)
}

/// Best effort: an account row must still be removable when the credential store
/// is unreachable, otherwise a broken keychain makes an account undeletable.
pub fn forget_account_secret(platform: PlatformId, remote_id: &str) {
    if let Ok(entry) = entry(&account_key(platform, remote_id))
        && let Err(err) = entry.delete_credential()
        && !matches!(err, keyring::Error::NoEntry)
    {
        log::warn!("could not remove the credential-store entry for {platform}: {err}");
    }
}

/// `None` when the user has not registered a developer app for this platform.
/// That is an ordinary state, not an error: Bluesky never needs one and Mastodon
/// registers its own on first connect.
pub fn load_app_credentials(
    platform: PlatformId,
    instance: Option<&str>,
) -> Result<Option<AppCredentials>> {
    match entry(&app_key(platform, instance))?.get_password() {
        Ok(raw) => Ok(Some(serde_json::from_str(&raw)?)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(other) => Err(other.into()),
    }
}

pub fn store_app_credentials(
    platform: PlatformId,
    instance: Option<&str>,
    credentials: &AppCredentials,
) -> Result<()> {
    entry(&app_key(platform, instance))?
        .set_password(&serde_json::to_string(credentials)?)
        .map_err(Into::into)
}

pub fn forget_app_credentials(platform: PlatformId, instance: Option<&str>) -> Result<()> {
    match entry(&app_key(platform, instance))?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(other) => Err(other.into()),
    }
}

/// `None` when no companion deployment is configured. Only the Meta adapters
/// need one, so its absence is not an error until one of them is used.
pub fn load_web_host() -> Result<Option<WebHost>> {
    match entry(WEB_HOST_KEY)?.get_password() {
        Ok(raw) => Ok(Some(serde_json::from_str(&raw)?)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(other) => Err(other.into()),
    }
}

pub fn store_web_host(host: &WebHost) -> Result<()> {
    entry(WEB_HOST_KEY)?
        .set_password(&serde_json::to_string(host)?)
        .map_err(Into::into)
}

pub fn forget_web_host() -> Result<()> {
    match entry(WEB_HOST_KEY)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(other) => Err(other.into()),
    }
}
