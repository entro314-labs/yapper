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
/// miss classification interrogates this host, so the
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

/// Marker for a channel whose release is published without its `latest.json`
/// (`isUpdateManifestMissing` in `src/lib/update-channel.ts`): the release
/// pipeline dropped the manifest, which no amount of waiting fixes.
const MANIFEST_MISSING_PREFIX: &str = "update manifest missing";

/// Marker for a manifest request GitHub refused for now — rate limited or a
/// server error (`isUpdateServerUnavailable` in `src/lib/update-channel.ts`).
const UNAVAILABLE_PREFIX: &str = "update server unavailable";

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

    /// The page of the release the manifest is served from. Stable's follows
    /// GitHub's `latest` redirect: to that release's tag page when one exists,
    /// to the bare release list when none does.
    fn release_page(self) -> &'static str {
        match self {
            Self::Stable => "https://github.com/entro314-labs/windbag-releases/releases/latest",
            Self::Beta => {
                "https://github.com/entro314-labs/windbag-releases/releases/tag/latest-beta"
            }
            Self::Alpha => {
                "https://github.com/entro314-labs/windbag-releases/releases/tag/latest-alpha"
            }
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Beta => "beta",
            Self::Alpha => "alpha",
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
async fn fetch_download_size(channel: Channel) -> Option<u64> {
    let body = http_client()
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

/// The async client for requests to the releases host, not
/// [`crate::http::client`]: these run inside Tauri commands on the async
/// runtime, where a blocking request would stall the executor thread.
fn http_client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(crate::http::user_agent())
        .build()
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

/// One request to the releases host, reduced to what classifying a miss needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reply {
    /// No HTTP answer at all: offline, DNS, TLS, a refused connection.
    Offline,
    Status(u16),
}

impl Reply {
    fn is_success(self) -> bool {
        matches!(self, Self::Status(200..=299))
    }

    /// Rate limited or a server fault: GitHub will likely answer later.
    fn transient(self) -> Option<u16> {
        match self {
            Self::Status(code @ (408 | 429 | 500..=599)) => Some(code),
            _ => None,
        }
    }
}

/// Why a channel's manifest could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Miss {
    Offline,
    /// GitHub refused for now (the status it answered).
    Unavailable(u16),
    /// The channel has a release, but no `latest.json` on it.
    ManifestMissing,
    /// The repository answers and the channel has no release yet — the calm
    /// pre-first-release state the UI may soften.
    NothingPublished,
    /// The repository itself is missing, private or renamed.
    SourceUnreachable,
    /// An answer none of the above explains (the status it answered).
    Other(u16),
}

/// Classifies a manifest miss from three answers: the manifest URL itself,
/// the channel's release page (a 200 only when it landed on a tag page, see
/// [`release_page_reply`]), and the repository's `releases.atom`, which exists
/// for every public repository with or without releases.
///
/// A missing manifest on a channel that has a release is a broken pipeline; a
/// missing manifest on a channel with no release is the pre-first-release
/// state, but only once the repository is known to be there. Deciding "no
/// release" from the repository alone would be wrong: stable's manifest 404s
/// while only prereleases exist, and that is not a fault.
fn classify_miss(manifest: Reply, release: Reply, repo: Reply) -> Miss {
    if let Some(code) = manifest.transient() {
        return Miss::Unavailable(code);
    }
    match manifest {
        Reply::Offline => Miss::Offline,
        // The updater saw a non-2xx a moment ago; a 2xx now is GitHub flapping.
        Reply::Status(code @ 200..=299) => Miss::Unavailable(code),
        Reply::Status(404 | 410) => {
            if release.is_success() {
                return Miss::ManifestMissing;
            }
            if let Some(code) = release.transient().or_else(|| repo.transient()) {
                return Miss::Unavailable(code);
            }
            if release == Reply::Offline {
                Miss::Offline
            } else if repo.is_success() {
                Miss::NothingPublished
            } else {
                Miss::SourceUnreachable
            }
        }
        Reply::Status(code) => Miss::Other(code),
    }
}

async fn fetch_reply(client: &reqwest::Client, url: &str) -> Reply {
    match client.get(url).send().await {
        Ok(response) => Reply::Status(response.status().as_u16()),
        Err(_) => Reply::Offline,
    }
}

/// The channel's release page, as a 200 only when a release is really there.
/// Stable's `/releases/latest` answers 200 either way — it redirects to the
/// bare release list when nothing is published — so a success that did not
/// land on a tag page reads as the 404 it means.
async fn release_page_reply(client: &reqwest::Client, channel: Channel) -> Reply {
    match client.get(channel.release_page()).send().await {
        Ok(response)
            if response.status().is_success()
                && !response.url().path().contains("/releases/tag/") =>
        {
            Reply::Status(404)
        }
        Ok(response) => Reply::Status(response.status().as_u16()),
        Err(_) => Reply::Offline,
    }
}

/// Asks the releases host why the channel's manifest could not be read. Only
/// called on a miss, so the three extra requests cost nothing on the normal
/// path.
async fn diagnose_miss(channel: Channel) -> Miss {
    let Ok(client) = http_client() else {
        return Miss::Offline;
    };
    let manifest = fetch_reply(&client, channel.endpoint()).await;
    let release = release_page_reply(&client, channel).await;
    let repo = fetch_reply(&client, &format!("{RELEASES_REPO_URL}/releases.atom")).await;
    classify_miss(manifest, release, repo)
}

/// Poll a channel manifest, classifying the miss the updater plugin cannot.
/// Shared by [`check_for_update`] and [`install_update`] so both doors tell the
/// user the same story about the same failure.
///
/// The plugin turns every non-2xx manifest answer into one `ReleaseNotFound`
/// ("Could not fetch a valid release JSON"), so a missing `latest.json`, a rate
/// limit and a channel with nothing on it all arrive identically. Its other
/// errors (transport, a malformed manifest, no build for this platform) carry
/// their own cause and pass through as they are.
async fn check_channel(
    app: &tauri::AppHandle,
    channel: Channel,
) -> Result<Option<tauri_plugin_updater::Update>> {
    let updater = build_updater(app, channel)?;
    let err = match updater.check().await {
        Ok(found) => return Ok(found),
        Err(err @ tauri_plugin_updater::Error::ReleaseNotFound) => err,
        Err(err) => return Err(AppError::Network(format!("Update check failed: {err}"))),
    };
    let manifest = channel.endpoint();
    Err(AppError::Network(match diagnose_miss(channel).await {
        Miss::Offline => format!("Update check failed: network error reaching {manifest}"),
        Miss::Unavailable(code) => format!("{UNAVAILABLE_PREFIX}: {manifest} answered HTTP {code}"),
        Miss::ManifestMissing => format!(
            "{MANIFEST_MISSING_PREFIX}: the {} release has no latest.json ({manifest} answered 404)",
            channel.as_str()
        ),
        Miss::NothingPublished => format!("Update check failed: {err}"),
        Miss::SourceUnreachable => {
            format!("{UNREACHABLE_PREFIX}: {RELEASES_REPO_URL} did not answer ({err})")
        }
        Miss::Other(code) => format!("Update check failed: {manifest} answered HTTP {code}"),
    }))
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
    use super::{Channel, Miss, RELEASES_REPO_URL, Reply, classify_miss};

    /// Every channel manifest and release page lives on the releases repo. If
    /// an endpoint is repointed without moving [`RELEASES_REPO_URL`] with it,
    /// the miss classification interrogates the wrong host and a broken
    /// pipeline is mislabelled as the calm pre-first-release state again.
    #[test]
    fn endpoints_live_on_the_releases_repo() {
        for channel in [Channel::Stable, Channel::Beta, Channel::Alpha] {
            assert!(
                channel.endpoint().starts_with(RELEASES_REPO_URL),
                "{channel:?} endpoint is not hosted on {RELEASES_REPO_URL}"
            );
            assert!(
                channel.release_page().starts_with(RELEASES_REPO_URL),
                "{channel:?} release page is not hosted on {RELEASES_REPO_URL}"
            );
        }
    }

    /// The release page for each channel is the tag its manifest is served
    /// from — stable through GitHub's `latest` redirect.
    #[test]
    fn release_pages_match_the_manifest_tags() {
        assert!(Channel::Stable.release_page().ends_with("/releases/latest"));
        assert!(
            Channel::Beta
                .release_page()
                .ends_with("/releases/tag/latest-beta")
        );
        assert!(
            Channel::Alpha
                .release_page()
                .ends_with("/releases/tag/latest-alpha")
        );
    }

    /// Why a manifest could not be read decides what the user is told. Before
    /// this, every miss on a live repository read as "nothing published yet" —
    /// a release missing its latest.json and a GitHub rate limit included.
    #[test]
    fn a_manifest_miss_is_classified_by_what_each_probe_answered() {
        use Reply::{Offline, Status};
        let cases = [
            // (manifest, release page, repository) → verdict
            ((Offline, Offline, Offline), Miss::Offline),
            (
                (Status(429), Status(200), Status(200)),
                Miss::Unavailable(429),
            ),
            ((Status(503), Offline, Offline), Miss::Unavailable(503)),
            (
                (Status(200), Status(200), Status(200)),
                Miss::Unavailable(200),
            ),
            (
                (Status(404), Status(200), Status(200)),
                Miss::ManifestMissing,
            ),
            (
                (Status(404), Status(429), Status(200)),
                Miss::Unavailable(429),
            ),
            ((Status(404), Offline, Status(200)), Miss::Offline),
            (
                (Status(404), Status(404), Status(200)),
                Miss::NothingPublished,
            ),
            (
                (Status(404), Status(404), Status(502)),
                Miss::Unavailable(502),
            ),
            (
                (Status(404), Status(404), Status(404)),
                Miss::SourceUnreachable,
            ),
            ((Status(404), Status(404), Offline), Miss::SourceUnreachable),
            (
                (Status(410), Status(404), Status(200)),
                Miss::NothingPublished,
            ),
            ((Status(403), Status(200), Status(200)), Miss::Other(403)),
        ];
        for ((manifest, release, repo), verdict) in cases {
            assert_eq!(
                classify_miss(manifest, release, repo),
                verdict,
                "manifest {manifest:?}, release page {release:?}, repo {repo:?}"
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

    /// String contract with `isUpdateManifestMissing` in
    /// `src/lib/update-channel.ts`.
    #[test]
    fn manifest_missing_marker_is_the_documented_prefix() {
        assert_eq!(super::MANIFEST_MISSING_PREFIX, "update manifest missing");
    }

    /// String contract with `isUpdateServerUnavailable` in
    /// `src/lib/update-channel.ts`.
    #[test]
    fn unavailable_marker_is_the_documented_prefix() {
        assert_eq!(super::UNAVAILABLE_PREFIX, "update server unavailable");
    }
}
