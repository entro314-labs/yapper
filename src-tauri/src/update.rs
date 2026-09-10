//! Deferred auto-updates.
//!
//! The key move: [`install_update`] downloads and verifies the bundle into
//! managed state but does NOT install it — replacing a live bundle breaks the
//! running process's code signature. The staged update installs from
//! `RunEvent::ExitRequested` (the quit path) or explicitly via
//! [`restart_and_install`] (the Settings "Restart now" button). Closing the
//! window only hides Windbag, so the quit path is genuinely the exit: the
//! scheduler keeps running until the user actually quits.
//!
//! Channels map to fixed release manifests; the artifact signature is checked
//! by the updater plugin against the minisign pubkey in `tauri.conf.json`.

use std::sync::Mutex;

use serde::Serialize;
use tauri::{Emitter, Manager};
use tauri_plugin_updater::UpdaterExt;

use crate::error::{AppError, Result};

/// A downloaded-and-verified update waiting for exit to install.
pub struct PendingUpdate(pub Mutex<Option<(tauri_plugin_updater::Update, Vec<u8>)>>);

/// The public releases repository every channel manifest is an asset of. The
/// unreachable probe interrogates this host, so the
/// `endpoints_live_on_the_releases_repo` test keeps it and [`Channel::endpoint`]
/// from drifting apart.
///
/// A separate repo from the app's own, matching every sibling app here. Not for
/// privacy — this app's repo is necessarily public, because the AT Protocol
/// `client_id` is a URL its GitHub Pages site serves. It keeps six platforms'
/// worth of binaries and rolling channel tags out of the repo people read, and
/// it means the updater keeps working if that ever stops being true.
const RELEASES_REPO_URL: &str = "https://github.com/entro314-labs/windbag-releases";

/// Marker prefix the renderer keys off (`isUpdateSourceUnreachable`,
/// `src/lib/update-channel.ts`). The updater plugin flattens every unreadable
/// endpoint into one string ("Could not fetch a valid release JSON"), which
/// makes a missing releases repository indistinguishable from a channel that
/// merely has no release yet — so a permanently broken update pipeline would
/// read to the user as the calm pre-first-release state, forever.
const UNREACHABLE_PREFIX: &str = "update source unreachable";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Channel {
    Stable,
    Beta,
    Alpha,
}

impl Channel {
    fn parse(s: Option<&str>) -> Self {
        match s.unwrap_or("stable") {
            "alpha" => Self::Alpha,
            "beta" => Self::Beta,
            _ => Self::Stable,
        }
    }

    /// Prerelease manifests live on fixed rolling tags in the public releases
    /// repo so the endpoint never moves; stable rides GitHub's `latest` alias.
    fn endpoint(self) -> &'static str {
        match self {
            Self::Stable => {
                "https://github.com/entro314-labs/windbag-releases/releases/latest/download/latest.json"
            }
            Self::Beta => {
                "https://github.com/entro314-labs/windbag-releases/releases/download/latest-beta/latest.json"
            }
            Self::Alpha => {
                "https://github.com/entro314-labs/windbag-releases/releases/download/latest-alpha/latest.json"
            }
        }
    }
}

/// What the renderer shows about a release before the user commits to it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMeta {
    pub version: String,
    pub notes: Option<String>,
    pub date: Option<String>,
    /// Size in bytes of this platform's bundle, read from the release
    /// pipeline's `size` extension in the update manifest. `None` when the
    /// manifest predates the extension or the lookup fails — size display is
    /// progressive enhancement, never a gate.
    pub download_size: Option<u64>,
}

/// The manifest platform key for the running build (mirrors the updater
/// plugin's own `{{target}}-{{arch}}` resolution for the six targets the
/// release pipeline ships).
const fn manifest_platform_key() -> &'static str {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        "darwin-aarch64"
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        "darwin-x86_64"
    }
    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    {
        "windows-aarch64"
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        "windows-x86_64"
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        "linux-aarch64"
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        "linux-x86_64"
    }
}

/// Best-effort lookup of this platform's bundle size from the channel
/// manifest's `size` extension (written by tauri-release-kit; absent from
/// manifests published before it existed). Any failure — network, JSON shape,
/// missing key — resolves to `None`.
///
/// The async client, not [`crate::http::client`]: this runs inside a Tauri
/// command on the async runtime, where a blocking request would stall the
/// executor thread it is scheduled on.
async fn fetch_download_size(channel: Channel) -> Option<u64> {
    let body = reqwest::Client::builder()
        .user_agent(crate::http::user_agent())
        .build()
        .ok()?
        .get(channel.endpoint())
        .send()
        .await
        .ok()?
        .text()
        .await
        .ok()?;
    let manifest: serde_json::Value = serde_json::from_str(&body).ok()?;
    manifest
        .get("platforms")?
        .get(manifest_platform_key())?
        .get("size")?
        .as_u64()
}

/// Why self-update is or isn't available for this install, so the UI can point
/// the user at the right update path instead of offering a check that can never
/// work.
// Deserialize is load-bearing beyond symmetry: on non-Linux builds the
// `PackageManager` variant is only ever constructed by the deserializer, which
// keeps clippy's never-constructed lint satisfied without a cfg'd allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UpdateInstallSupport {
    /// The in-app updater can replace this install.
    Supported,
    /// Managed by an external package manager (deb/rpm/Flatpak on Linux) — the
    /// Tauri updater can only self-update `AppImage` installs.
    PackageManager,
}

/// Report whether the in-app updater can service this install. On Linux the
/// updater only supports `AppImage` (the `APPIMAGE` env var is set by the
/// `AppImage` runtime); deb/rpm/Flatpak installs must update through their package
/// manager. macOS and Windows installs are always self-updatable.
// Commands keep the uniform `Result` surface so the TS contract stays stable if
// the implementation becomes fallible.
#[allow(clippy::unnecessary_wraps)]
#[tauri::command]
pub fn update_install_support() -> Result<UpdateInstallSupport> {
    #[cfg(target_os = "linux")]
    {
        let is_appimage = std::env::var_os("APPIMAGE").is_some();
        let is_flatpak = std::env::var_os("FLATPAK_ID").is_some();
        if is_appimage && !is_flatpak {
            Ok(UpdateInstallSupport::Supported)
        } else {
            Ok(UpdateInstallSupport::PackageManager)
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(UpdateInstallSupport::Supported)
    }
}

/// Whether an update has been downloaded and staged this session (it installs on
/// the next quit). Lets the UI restore its "restart to update" state after a
/// window reload and drive the ambient readout in the status bar. A poisoned
/// lock reports `false` — the exit-time installer logs its own state.
#[allow(clippy::unnecessary_wraps)]
#[tauri::command]
pub fn update_staged(app: tauri::AppHandle) -> Result<bool> {
    Ok(app
        .try_state::<PendingUpdate>()
        .is_some_and(|pending| pending.0.lock().is_ok_and(|guard| guard.is_some())))
}

fn build_updater(
    app: &tauri::AppHandle,
    channel: Channel,
) -> Result<tauri_plugin_updater::Updater> {
    app.updater_builder()
        .endpoints(vec![channel.endpoint().parse().map_err(|err| {
            AppError::Internal(format!("Bad update endpoint: {err}"))
        })?])
        .map_err(|err| AppError::Internal(err.to_string()))?
        .build()
        .map_err(|err| AppError::Internal(err.to_string()))
}

/// Does the releases host itself answer? `releases.atom` is the cheapest
/// endpoint that exists for every public repository with or without releases:
/// 200 when the repo is live, 404 when it is missing, private or renamed. A
/// transport failure counts as unreachable too — offline and repo-gone are
/// different causes but the same user-visible truth, and neither of them is
/// "no release has been published on this channel yet".
async fn releases_repo_reachable() -> bool {
    let Ok(client) = reqwest::Client::builder()
        .user_agent(crate::http::user_agent())
        .build()
    else {
        return false;
    };
    client
        .get(format!("{RELEASES_REPO_URL}/releases.atom"))
        .send()
        .await
        .is_ok_and(|response| response.status().is_success())
}

/// Poll a channel manifest, classifying the miss the updater plugin cannot.
/// Shared by [`check_for_update`] and [`install_update`] so both doors tell the
/// user the same story about the same failure.
async fn check_channel(
    app: &tauri::AppHandle,
    channel: Channel,
) -> Result<Option<tauri_plugin_updater::Update>> {
    let updater = build_updater(app, channel)?;
    let err = match updater.check().await {
        Ok(found) => return Ok(found),
        Err(err) => err,
    };
    if releases_repo_reachable().await {
        // The repo answers, so the manifest is genuinely just not published on
        // this channel — the calm pre-first-release state the UI may soften.
        Err(AppError::Network(format!("Update check failed: {err}")))
    } else {
        // The repo does not answer: the update pipeline is broken, not pending.
        Err(AppError::Network(format!(
            "{UNREACHABLE_PREFIX}: {RELEASES_REPO_URL} did not answer ({err})"
        )))
    }
}

/// Is a newer build published on this channel?
#[tauri::command]
pub async fn check_for_update(
    app: tauri::AppHandle,
    channel: Option<String>,
) -> Result<Option<UpdateMeta>> {
    let channel = Channel::parse(channel.as_deref());
    let Some(update) = check_channel(&app, channel).await? else {
        return Ok(None);
    };
    // Surface the download size BEFORE the user commits to the download.
    let download_size = fetch_download_size(channel).await;
    Ok(Some(UpdateMeta {
        version: update.version.clone(),
        notes: update.body.clone(),
        date: update.date.map(|date| date.to_string()),
        download_size,
    }))
}

/// Download and verify the channel's latest build and stage it for install on
/// exit. Emits throttled `windbag://update-progress` events
/// `{ downloaded, total }`.
#[tauri::command]
pub async fn install_update(app: tauri::AppHandle, channel: Option<String>) -> Result<UpdateMeta> {
    let update = check_channel(&app, Channel::parse(channel.as_deref()))
        .await?
        .ok_or_else(|| AppError::NotFound("Already on the latest version.".into()))?;

    let meta = UpdateMeta {
        version: update.version.clone(),
        notes: update.body.clone(),
        date: update.date.map(|date| date.to_string()),
        // The download starts immediately below — size display is moot here.
        download_size: None,
    };

    let progress_app = app.clone();
    let mut downloaded: u64 = 0;
    let mut last_emit = std::time::Instant::now();
    let bytes = update
        .download(
            move |chunk, total| {
                downloaded += chunk as u64;
                // Throttle to ~10 events a second; the final snapshot is
                // emitted below, so the bar always lands on complete.
                if last_emit.elapsed() >= std::time::Duration::from_millis(100) {
                    last_emit = std::time::Instant::now();
                    let _ = progress_app.emit(
                        EVENT_PROGRESS,
                        serde_json::json!({ "downloaded": downloaded, "total": total }),
                    );
                }
            },
            || {},
        )
        .await
        .map_err(|err| AppError::Network(format!("Update download failed: {err}")))?;
    let _ = app.emit(
        EVENT_PROGRESS,
        serde_json::json!({ "downloaded": bytes.len(), "total": bytes.len() }),
    );

    let pending = app.state::<PendingUpdate>();
    *pending
        .0
        .lock()
        .map_err(|_| AppError::Internal("The update slot was poisoned.".into()))? =
        Some((update, bytes));
    Ok(meta)
}

/// Mirrors `IPC_EVENTS.updateProgress` in `src/lib/tauri/ipc.ts`.
const EVENT_PROGRESS: &str = "windbag://update-progress";

/// Install the staged update now and relaunch into the new version.
#[tauri::command]
pub fn restart_and_install(app: tauri::AppHandle) -> Result<()> {
    let (update, bytes) = app
        .state::<PendingUpdate>()
        .0
        .lock()
        .map_err(|_| AppError::Internal("The update slot was poisoned.".into()))?
        .take()
        .ok_or_else(|| AppError::NotFound("No update staged.".into()))?;
    update
        .install(bytes)
        .map_err(|err| AppError::Internal(format!("Update install failed: {err}")))?;
    app.restart();
}

/// Quit path: install whatever is staged, quietly. Called from
/// `RunEvent::ExitRequested`.
pub fn install_pending_on_exit(app: &tauri::AppHandle) {
    let Some(state) = app.try_state::<PendingUpdate>() else {
        return;
    };
    let staged = state.0.lock().ok().and_then(|mut pending| pending.take());
    if let Some((update, bytes)) = staged {
        if let Err(err) = update.install(bytes) {
            log::error!("failed to install the staged update on exit: {err}");
        } else {
            log::info!("staged update installed; the next launch runs the new version");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Channel, RELEASES_REPO_URL};

    /// Every channel manifest is an asset of the releases repo. If an endpoint
    /// is repointed without moving [`RELEASES_REPO_URL`] with it, the
    /// unreachable probe interrogates the wrong host and a broken pipeline is
    /// mislabelled as the calm pre-first-release state again.
    #[test]
    fn endpoints_live_on_the_releases_repo() {
        for channel in [Channel::Stable, Channel::Beta, Channel::Alpha] {
            assert!(
                channel.endpoint().starts_with(RELEASES_REPO_URL),
                "{channel:?} endpoint is not hosted on {RELEASES_REPO_URL}"
            );
        }
    }

    /// tauri-release-kit publishes prerelease manifests on rolling tags
    /// (`latest-alpha` / `latest-beta`) and stable through GitHub's `latest`
    /// alias; these three shapes are the contract between the two repos.
    #[test]
    fn endpoints_match_the_release_kit_convention() {
        assert!(
            Channel::Stable
                .endpoint()
                .ends_with("/releases/latest/download/latest.json")
        );
        assert!(
            Channel::Beta
                .endpoint()
                .ends_with("/releases/download/latest-beta/latest.json")
        );
        assert!(
            Channel::Alpha
                .endpoint()
                .ends_with("/releases/download/latest-alpha/latest.json")
        );
    }

    /// The unreachable classification is a string contract with
    /// `isUpdateSourceUnreachable` in `src/lib/update-channel.ts`: the renderer
    /// cannot tell a dead pipeline from a channel awaiting its first release
    /// without it.
    #[test]
    fn unreachable_marker_is_the_documented_prefix() {
        assert_eq!(super::UNREACHABLE_PREFIX, "update source unreachable");
    }
}
