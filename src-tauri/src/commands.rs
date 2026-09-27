//! The IPC surface. Every command returns [`AppError`], which serializes as
//! `"[CODE] message"` — the renderer's query client reads that code to decide
//! what is worth retrying (see `src/lib/query/client.ts`).

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::ai::{self, Availability, Backend, DraftRequest, Suggestion};
use crate::db::{self, Account, Attempt, Db, MediaInput, Note, PostDetail};
use crate::error::{AppError, Result};
use crate::platforms::{self, AppCredentials, ConnectInput, MediaSpec, PlatformId, PlatformInfo};
use crate::scheduler::{
    self, EVENT_ACCOUNTS_CHANGED, EVENT_QUEUE_CHANGED, META_GRACE_MINUTES, META_MISSED_POLICY,
    MissedPolicy, Scheduler,
};
use crate::stats::{self, RefreshCost, RefreshReport, Stats, StatsFilter};
use crate::{media, secrets};

pub const EVENT_AUTH: &str = "windbag://auth";

pub struct AppState {
    pub db: Arc<Db>,
    pub scheduler: Scheduler,
    /// One browser handoff at a time: the loopback listener binds a fixed port,
    /// so a second flow would fail on the port rather than on anything the user
    /// could act on.
    pub connecting: AtomicBool,
}

// ─── Platforms and accounts ─────────────────────────────────────────────────

#[tauri::command]
pub fn list_platforms() -> Vec<PlatformInfo> {
    platforms::all_info()
}

#[tauri::command]
pub fn list_accounts(state: State<'_, AppState>) -> Result<Vec<Account>> {
    state.db.list_accounts()
}

/// The outcome of a browser handoff, delivered on [`EVENT_AUTH`] because the
/// flow outlives the command that started it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthOutcome {
    pub ok: bool,
    pub platform: PlatformId,
    pub message: String,
    pub account: Option<Account>,
}

/// Starts a connection. Returns as soon as the flow is under way; the result
/// arrives on [`EVENT_AUTH`]. Credential-based platforms (Bluesky) finish in
/// well under a second, OAuth ones wait on a human in a browser — both take the
/// same path so the renderer has one thing to listen for.
#[tauri::command]
pub fn connect_account(
    app: AppHandle,
    state: State<'_, AppState>,
    platform: String,
    fields: HashMap<String, String>,
) -> Result<()> {
    let platform = PlatformId::parse(&platform)?;

    if state.connecting.swap(true, Ordering::SeqCst) {
        return Err(AppError::Conflict(
            "A sign-in is already running. Finish it in the browser, or wait for it to \
             time out, before starting another."
                .into(),
        ));
    }

    let database = Arc::clone(&state.db);
    let spawned = std::thread::Builder::new()
        .name("windbag-connect".into())
        .spawn(move || {
            let outcome = run_connect(&database, platform, fields);
            let payload = match outcome {
                Ok(account) => AuthOutcome {
                    ok: true,
                    platform,
                    message: format!("Connected {}", account.handle),
                    account: Some(account),
                },
                Err(err) => {
                    log::warn!("connecting {platform} failed: {err}");
                    AuthOutcome {
                        ok: false,
                        platform,
                        message: err.to_string(),
                        account: None,
                    }
                }
            };
            if let Some(state) = app.try_state::<AppState>() {
                state.connecting.store(false, Ordering::SeqCst);
            }
            let _ = app.emit(EVENT_AUTH, &payload);
            let _ = app.emit(EVENT_ACCOUNTS_CHANGED, ());
        });

    if spawned.is_err() {
        state.connecting.store(false, Ordering::SeqCst);
        return Err(AppError::Internal("Could not start the sign-in.".into()));
    }
    Ok(())
}

fn run_connect(
    database: &Arc<Db>,
    platform: PlatformId,
    fields: HashMap<String, String>,
) -> Result<Account> {
    let adapter = platforms::adapter(platform);
    let info = adapter.info();

    // Mastodon scopes its app registration to the instance, so the credential
    // lookup has to know which one before the flow starts.
    let instance = fields
        .get("instance")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(normalize_instance_key);

    let app_credentials = secrets::load_app_credentials(platform, instance.as_deref())?;
    // Only a REQUIRED app field makes credentials a precondition. Bluesky
    // exposes an optional one (the OAuth client metadata URL, which has a
    // working default), and demanding it would block the app-password path that
    // needs no developer app at all.
    let needs_app = info.app_fields.iter().any(|field| field.required);
    if needs_app && app_credentials.is_none() {
        return Err(AppError::InvalidInput(format!(
            "{} needs your own developer app. Add its client id in Settings → Platform apps.",
            info.name
        )));
    }

    let connected = adapter.connect(&ConnectInput {
        fields,
        app: app_credentials,
    })?;
    secrets::store_account_secret(platform, &connected.remote_id, &connected.secret)?;
    let id = database.upsert_account(platform, &connected)?;
    database.get_account(id)
}

#[tauri::command]
pub fn disconnect_account(app: AppHandle, state: State<'_, AppState>, id: i64) -> Result<()> {
    // The credential-store entry goes first: deleting the row first would leave a
    // secret nothing can name any more.
    let account = state.db.get_account(id)?;
    secrets::forget_account_secret(account.platform, &account.remote_id);
    state.db.delete_account(id)?;
    let _ = app.emit(EVENT_ACCOUNTS_CHANGED, ());
    let _ = app.emit(EVENT_QUEUE_CHANGED, ());
    Ok(())
}

/// What Settings shows for a stored developer app. The secret is reported as
/// present or absent and never sent back to the renderer.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppCredentialsView {
    pub client_id: String,
    pub has_secret: bool,
    pub extra: HashMap<String, String>,
}

#[tauri::command]
pub fn get_app_credentials(
    platform: String,
    instance: Option<String>,
) -> Result<Option<AppCredentialsView>> {
    let platform = PlatformId::parse(&platform)?;
    let key = instance.as_deref().map(normalize_instance_key);
    Ok(
        secrets::load_app_credentials(platform, key.as_deref())?.map(|credentials| {
            AppCredentialsView {
                client_id: credentials.client_id,
                has_secret: credentials
                    .client_secret
                    .is_some_and(|value| !value.is_empty()),
                extra: credentials.extra,
            }
        }),
    )
}

#[tauri::command]
pub fn save_app_credentials(
    platform: String,
    instance: Option<String>,
    client_id: String,
    client_secret: Option<String>,
    extra: Option<HashMap<String, String>>,
) -> Result<()> {
    let platform = PlatformId::parse(&platform)?;
    let client_id = client_id.trim().to_string();
    if client_id.is_empty() {
        return Err(AppError::InvalidInput(
            "The client id cannot be empty.".into(),
        ));
    }
    let key = instance.as_deref().map(normalize_instance_key);
    // A blank secret keeps the stored one, as the form promises ("leave blank
    // to keep it"). The item is written whole, so without this, flipping a
    // toggle like Insights access would silently delete the app secret.
    let client_secret = match client_secret
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        Some(secret) => Some(secret),
        None => secrets::load_app_credentials(platform, key.as_deref())?
            .and_then(|stored| stored.client_secret),
    };
    secrets::store_app_credentials(
        platform,
        key.as_deref(),
        &AppCredentials {
            client_id,
            client_secret,
            extra: extra.unwrap_or_default(),
        },
    )
}

#[tauri::command]
pub fn forget_app_credentials(platform: String, instance: Option<String>) -> Result<()> {
    let platform = PlatformId::parse(&platform)?;
    let key = instance.as_deref().map(normalize_instance_key);
    secrets::forget_app_credentials(platform, key.as_deref())
}

/// Instance keys are lowercased and stripped of scheme and trailing slash so the
/// credential-store item for `Mastodon.Social` and `https://mastodon.social/` is
/// the same item.
fn normalize_instance_key(value: &str) -> String {
    value
        .trim()
        .trim_end_matches('/')
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .to_ascii_lowercase()
}

// ─── Posts ──────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetInput {
    pub account_id: i64,
    #[serde(default)]
    pub options: serde_json::Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaFieldInput {
    pub path: String,
    #[serde(default)]
    pub alt_text: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavePostInput {
    /// `None` creates; `Some` updates in place.
    pub id: Option<i64>,
    pub body: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub link: Option<String>,
    /// RFC 3339 UTC. `None` saves a draft.
    #[serde(default)]
    pub scheduled_at: Option<String>,
    #[serde(default)]
    pub targets: Vec<TargetInput>,
    #[serde(default)]
    pub media: Vec<MediaFieldInput>,
}

#[tauri::command]
pub fn list_posts(state: State<'_, AppState>) -> Result<Vec<PostDetail>> {
    state.db.list_posts()
}

#[tauri::command]
pub fn list_attempts(state: State<'_, AppState>, post_id: i64) -> Result<Vec<Attempt>> {
    state.db.list_attempts(post_id)
}

#[tauri::command]
pub fn save_post(app: AppHandle, state: State<'_, AppState>, input: SavePostInput) -> Result<i64> {
    // Parsed rather than trusted, and stored as canonical UTC: a value the store
    // cannot read back would make the post invisible to the due query, and one
    // kept with its offset would be compared as a string and fire at the wrong
    // hour.
    let scheduled_at = input
        .scheduled_at
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| db::parse_rfc3339(value).map(|parsed| parsed.to_rfc3339()))
        .transpose()?;
    let scheduled_at = scheduled_at.as_deref();

    let status = if scheduled_at.is_some() {
        db::POST_SCHEDULED
    } else {
        db::POST_DRAFT
    };
    let title = input
        .title
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty());
    let link = input
        .link
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty());

    // Everything a destination will be judged on is known before anything is
    // written, so refuse here rather than at 09:00 tomorrow when nobody is
    // watching — and refuse without leaving a half-saved post behind (a stored
    // invalid post would fail at its time; an orphaned new one would be
    // duplicated by the next save, since the composer never learned its id).
    let media = input
        .media
        .iter()
        .map(|item| {
            let resolved = media::resolve(&item.path)?;
            Ok(MediaInput {
                path: resolved.path,
                mime: resolved.mime,
                bytes: resolved.bytes,
                alt_text: item
                    .alt_text
                    .clone()
                    .filter(|value| !value.trim().is_empty()),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let specs: Vec<MediaSpec> = media
        .iter()
        .map(|item| MediaSpec {
            mime: item.mime.clone(),
            bytes: item.bytes.unsigned_abs(),
        })
        .collect();
    for target in &input.targets {
        let account = state.db.get_account(target.account_id)?;
        let adapter = platforms::adapter(account.platform);
        platforms::validate(
            account.platform,
            &input.body,
            title,
            link,
            &specs,
            &target.options,
            scheduler::effective_char_limit(&account, adapter),
        )?;
    }

    let post_id = match input.id {
        Some(id) => {
            let existing = state.db.get_post(id)?;
            if existing.status == db::POST_PUBLISHED {
                return Err(AppError::Conflict(
                    "This post has already gone out and cannot be edited.".into(),
                ));
            }
            state
                .db
                .update_post(id, &input.body, title, link, scheduled_at, status)?;
            id
        }
        None => state
            .db
            .create_post(&input.body, title, link, scheduled_at, status)?,
    };
    state.db.set_media(post_id, &media)?;

    let targets: Vec<(i64, serde_json::Value)> = input
        .targets
        .iter()
        .map(|target| (target.account_id, target.options.clone()))
        .collect();
    state.db.set_targets(post_id, &targets)?;

    let _ = app.emit(EVENT_QUEUE_CHANGED, post_id);
    state.scheduler.nudge();
    Ok(post_id)
}

#[tauri::command]
pub fn delete_post(app: AppHandle, state: State<'_, AppState>, id: i64) -> Result<()> {
    state.db.delete_post(id)?;
    let _ = app.emit(EVENT_QUEUE_CHANGED, id);
    Ok(())
}

/// Moves a post to "now" and wakes the worker. Deliberately the same code path
/// as a scheduled post rather than a second publish route — one publisher means
/// one set of retry and validation rules.
#[tauri::command]
pub fn publish_now(app: AppHandle, state: State<'_, AppState>, id: i64) -> Result<()> {
    if state.db.list_targets(id)?.is_empty() {
        return Err(AppError::InvalidInput(
            "This post has no destinations. Pick at least one account.".into(),
        ));
    }
    scheduler::requeue(&state.db, id, &db::now_rfc3339())?;
    let _ = app.emit(EVENT_QUEUE_CHANGED, id);
    state.scheduler.nudge();
    Ok(())
}

#[tauri::command]
pub fn reschedule_post(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    scheduled_at: String,
) -> Result<()> {
    let scheduled_at = db::parse_rfc3339(&scheduled_at)?.to_rfc3339();
    scheduler::requeue(&state.db, id, &scheduled_at)?;
    let _ = app.emit(EVENT_QUEUE_CHANGED, id);
    state.scheduler.nudge();
    Ok(())
}

/// Clears one destination's error and backoff so the next pass tries it again.
#[tauri::command]
pub fn retry_target(app: AppHandle, state: State<'_, AppState>, target_id: i64) -> Result<()> {
    state.db.requeue_target(target_id)?;
    let post_id = state
        .db
        .list_posts()?
        .into_iter()
        .find(|detail| detail.targets.iter().any(|t| t.id == target_id))
        .map(|detail| detail.post.id)
        .ok_or_else(|| AppError::NotFound(format!("No destination with id {target_id}.")))?;
    state.db.reconcile_post_status(post_id)?;
    let _ = app.emit(EVENT_QUEUE_CHANGED, post_id);
    state.scheduler.nudge();
    Ok(())
}

// ─── Composer support ───────────────────────────────────────────────────────

/// One destination's verdict on the current draft, for the live counters in the
/// composer. The same [`platforms::validate`] the scheduler runs, so what the
/// composer says is what will actually happen.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetCheck {
    pub account_id: i64,
    pub platform: PlatformId,
    pub handle: String,
    pub used: usize,
    pub limit: usize,
    pub error: Option<String>,
}

#[tauri::command]
pub fn check_post(
    state: State<'_, AppState>,
    body: String,
    title: Option<String>,
    link: Option<String>,
    media: Vec<MediaSpec>,
    targets: Vec<TargetInput>,
) -> Result<Vec<TargetCheck>> {
    let title = title
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let link = link
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    targets
        .into_iter()
        .map(|target| {
            let account = state.db.get_account(target.account_id)?;
            let adapter = platforms::adapter(account.platform);
            let limit = scheduler::effective_char_limit(&account, adapter);
            let error = platforms::validate(
                account.platform,
                &body,
                title,
                link,
                &media,
                &target.options,
                limit,
            )
            .err()
            .map(|err| err.to_string());
            Ok(TargetCheck {
                account_id: target.account_id,
                platform: account.platform,
                handle: account.handle,
                used: platforms::counted_length(account.platform, &body, &target.options),
                limit,
                error,
            })
        })
        .collect()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedMedia {
    pub path: String,
    pub mime: String,
    pub bytes: i64,
}

#[tauri::command]
pub fn resolve_media(paths: Vec<String>) -> Result<Vec<ResolvedMedia>> {
    paths
        .iter()
        .map(|path| {
            let resolved = media::resolve(path)?;
            Ok(ResolvedMedia {
                path: resolved.path,
                mime: resolved.mime,
                bytes: resolved.bytes,
            })
        })
        .collect()
}

// ─── Notes ──────────────────────────────────────────────────────────────────

pub const EVENT_NOTES_CHANGED: &str = "windbag://notes-changed";

#[tauri::command]
pub fn list_notes(state: State<'_, AppState>) -> Result<Vec<Note>> {
    state.db.list_notes()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveNoteInput {
    pub id: Option<i64>,
    #[serde(default)]
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub pinned: bool,
}

#[tauri::command]
pub fn save_note(app: AppHandle, state: State<'_, AppState>, input: SaveNoteInput) -> Result<i64> {
    let body = input.body.trim();
    if body.is_empty() {
        return Err(AppError::InvalidInput("A note needs some text.".into()));
    }
    let id = state
        .db
        .save_note(input.id, input.title.trim(), body, input.pinned)?;
    let _ = app.emit(EVENT_NOTES_CHANGED, id);
    Ok(id)
}

#[tauri::command]
pub fn delete_note(app: AppHandle, state: State<'_, AppState>, id: i64) -> Result<()> {
    state.db.delete_note(id)?;
    let _ = app.emit(EVENT_NOTES_CHANGED, id);
    Ok(())
}

// ─── Assistant ──────────────────────────────────────────────────────────────

/// What every backend can do right now, for the Settings picker. Probing runs a
/// `--version` per CLI, so this is a command rather than part of `get_settings`.
#[tauri::command(async)]
pub fn ai_availability(app: AppHandle) -> Vec<Availability> {
    ai::all_availability(&app)
}

/// Asks the configured assistant for drafts.
///
/// The drafts come back to the CALLER; nothing is written. They land in the
/// composer, where the same validation and the same human click that guard every
/// other post still apply — the assistant has no path to the queue.
///
/// `async` like every command here that waits on a process or the network:
/// Tauri runs a plain command ON the main thread, which would freeze the window
/// for as long as the CLI takes — up to three minutes.
#[tauri::command(async)]
pub fn suggest_posts(
    app: AppHandle,
    state: State<'_, AppState>,
    context: String,
    instructions: String,
    note_ids: Vec<i64>,
    account_ids: Vec<i64>,
    count: usize,
) -> Result<Vec<Suggestion>> {
    let backend = Backend::parse(state.db.get_meta(ai::META_BACKEND)?.as_deref());
    let model = state.db.get_meta(ai::META_MODEL)?;
    let effort = state.db.get_meta(ai::META_EFFORT)?;

    // Notes are pulled here rather than pasted by the renderer: the assistant
    // should read what is actually saved, not a copy that may have drifted.
    let mut material = context.trim().to_string();
    for id in note_ids {
        let note = state.db.get_note(id)?;
        if !material.is_empty() {
            material.push_str("\n\n");
        }
        if !note.title.trim().is_empty() {
            let _ = writeln!(material, "## {}", note.title.trim());
        }
        material.push_str(note.body.trim());
    }
    if material.is_empty() {
        return Err(AppError::InvalidInput(
            "Give the assistant something to work from — a note, or some text.".into(),
        ));
    }

    // The destinations' real limits go into the prompt, so drafts are written to
    // fit rather than trimmed to fit afterwards.
    let destinations = account_ids
        .iter()
        .map(|id| {
            let account = state.db.get_account(*id)?;
            let adapter = platforms::adapter(account.platform);
            Ok((
                adapter.info().name.to_string(),
                scheduler::effective_char_limit(&account, adapter),
            ))
        })
        .collect::<Result<Vec<_>>>()?;

    ai::suggest(
        &app,
        backend,
        model.as_deref(),
        effort.as_deref(),
        &DraftRequest {
            context: material,
            instructions,
            destinations,
            count,
        },
    )
}

// ─── Stats ──────────────────────────────────────────────────────────────────

#[tauri::command]
pub fn get_stats(state: State<'_, AppState>, filter: StatsFilter) -> Result<Stats> {
    // Bounds are parsed for their side effect: a value the store could not have
    // written would silently match nothing, which reads as "you posted nothing".
    stats::parse_bound(filter.since.as_deref())?;
    stats::parse_bound(filter.until.as_deref())?;
    stats::compute(&state.db, &filter)
}

/// What a refresh would read, so the button that spends money on X can say how
/// much before it does.
#[tauri::command]
pub fn get_refresh_cost(state: State<'_, AppState>) -> Result<RefreshCost> {
    stats::refresh_cost(&state.db)
}

/// Fetches engagement for every published destination Windbag can read. Manual
/// on purpose: a background poller against eight APIs spends a rate-limit
/// budget on numbers nobody is looking at — and on X it spends real credits.
#[tauri::command(async)]
pub fn refresh_engagement(app: AppHandle, state: State<'_, AppState>) -> Result<RefreshReport> {
    let report = stats::refresh_engagement(&state.db)?;
    let _ = app.emit(EVENT_QUEUE_CHANGED, ());
    Ok(report)
}

// ─── Settings ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// `system` | `light` | `dark`
    pub theme: String,
    /// `off` | `standard` | `strong`
    pub window_material: String,
    /// `skip` | `post_late` — see [`MissedPolicy`].
    pub missed_policy: String,
    pub grace_minutes: i64,
    pub launch_at_login: bool,
    /// `off` | `apple` | `claude` | `codex`. Off by default: an assistant panel
    /// that errors on first click because nothing is configured is worse than
    /// one you opted into.
    pub ai_backend: String,
    /// Passed to the CLI backends as `--model` / `-m`. Empty means the tool's
    /// own default, which is almost always the right one.
    pub ai_model: String,
    /// `low` | `medium` | `high` | `xhigh`. Empty means the tool's default.
    pub ai_effort: String,
    /// `auto` | `stable` | `beta` | `alpha` — which release manifest the updater
    /// polls. `auto` derives it from the running build's own version, so a
    /// prerelease install does not sit on the stable endpoint waiting for a
    /// release that will not arrive for months.
    pub update_channel: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "system".into(),
            window_material: "standard".into(),
            missed_policy: MissedPolicy::Skip.as_str().into(),
            grace_minutes: 15,
            launch_at_login: false,
            ai_backend: Backend::Off.as_str().into(),
            ai_model: String::new(),
            ai_effort: String::new(),
            update_channel: "auto".into(),
        }
    }
}

/// `meta` key for the update channel. Kept beside the setting it stores rather
/// than in `update.rs`: the store is `commands`' business, and `update.rs` is
/// handed the resolved channel by the renderer.
const META_UPDATE_CHANNEL: &str = "update_channel";

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Result<Settings> {
    let defaults = Settings::default();
    Ok(Settings {
        theme: state.db.get_meta("theme")?.unwrap_or(defaults.theme),
        window_material: state
            .db
            .get_meta("window_material")?
            .unwrap_or(defaults.window_material),
        missed_policy: MissedPolicy::parse(state.db.get_meta(META_MISSED_POLICY)?.as_deref())
            .as_str()
            .into(),
        grace_minutes: state
            .db
            .get_meta(META_GRACE_MINUTES)?
            .and_then(|value| value.parse::<i64>().ok())
            .map_or(defaults.grace_minutes, |value| {
                value.max(scheduler::MIN_GRACE_MINUTES)
            }),
        launch_at_login: state
            .db
            .get_meta("launch_at_login")?
            .is_some_and(|value| value == "true"),
        ai_backend: Backend::parse(state.db.get_meta(ai::META_BACKEND)?.as_deref())
            .as_str()
            .into(),
        ai_model: state.db.get_meta(ai::META_MODEL)?.unwrap_or_default(),
        ai_effort: state.db.get_meta(ai::META_EFFORT)?.unwrap_or_default(),
        update_channel: state
            .db
            .get_meta(META_UPDATE_CHANNEL)?
            .unwrap_or(defaults.update_channel),
    })
}

#[tauri::command]
pub fn update_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: Settings,
) -> Result<Settings> {
    // Autostart goes FIRST and its failure fails the whole save. The scheduler
    // only runs while Windbag runs, so this switch is the difference between a
    // scheduler and a wish — recording `true` for a registration that did not
    // happen would be the app lying about the one thing it promises.
    apply_autostart(&app, settings.launch_at_login)?;

    state.db.set_meta("theme", &settings.theme)?;
    state
        .db
        .set_meta("window_material", &settings.window_material)?;
    state.db.set_meta(
        META_MISSED_POLICY,
        MissedPolicy::parse(Some(&settings.missed_policy)).as_str(),
    )?;
    state.db.set_meta(
        META_GRACE_MINUTES,
        &settings
            .grace_minutes
            .max(scheduler::MIN_GRACE_MINUTES)
            .to_string(),
    )?;
    state
        .db
        .set_meta("launch_at_login", &settings.launch_at_login.to_string())?;
    state.db.set_meta(
        ai::META_BACKEND,
        Backend::parse(Some(&settings.ai_backend)).as_str(),
    )?;
    state
        .db
        .set_meta(ai::META_MODEL, settings.ai_model.trim())?;
    state
        .db
        .set_meta(ai::META_EFFORT, settings.ai_effort.trim())?;
    // Anything unrecognised falls back to `auto`, which is always answerable:
    // an unknown channel string would send the updater at an endpoint that does
    // not exist and report it as a broken pipeline.
    state.db.set_meta(
        META_UPDATE_CHANNEL,
        match settings.update_channel.as_str() {
            channel @ ("stable" | "beta" | "alpha") => channel,
            _ => "auto",
        },
    )?;
    get_settings(state)
}

/// Registers or clears the login item. Registering the plugin in the builder
/// only makes this callable — it enables nothing by itself, which is how a
/// switch like this ends up looking wired while doing nothing at all.
pub fn apply_autostart(app: &AppHandle, enabled: bool) -> Result<()> {
    use tauri_plugin_autostart::ManagerExt;

    let manager = app.autolaunch();
    let outcome = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };
    outcome.map_err(|err| {
        AppError::Internal(format!(
            "Could not {} launch at login: {err}",
            if enabled { "turn on" } else { "turn off" }
        ))
    })
}

/// The loopback URI every OAuth app must register. Shown in Settings so it can
/// be copied rather than retyped — a single wrong character there fails the
/// exchange with a message that names nothing useful.
#[tauri::command]
pub fn oauth_redirect_uri() -> &'static str {
    crate::oauth::REDIRECT_URI
}

// ─── The web deployment ─────────────────────────────────────────────────────

/// What Settings shows for the companion deployment. The upload token is
/// reported as present or absent and never sent back to the renderer, the same
/// way a client secret is not.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebHostView {
    pub base_url: String,
    pub has_token: bool,
    /// The HTTPS redirect this deployment provides, derived rather than typed.
    /// It is the string that must be registered on the Meta app, so it is shown
    /// to be copied.
    pub redirect_uri: String,
}

#[tauri::command]
pub fn get_web_host() -> Result<Option<WebHostView>> {
    Ok(secrets::load_web_host()?.map(|host| WebHostView {
        redirect_uri: host.redirect_uri(),
        has_token: !host.upload_token.is_empty(),
        base_url: host.base_url,
    }))
}

#[tauri::command]
pub fn save_web_host(base_url: String, upload_token: Option<String>) -> Result<WebHostView> {
    let base_url = base_url.trim().trim_end_matches('/').to_string();
    if base_url.is_empty() {
        return Err(AppError::InvalidInput(
            "The web deployment needs its base URL.".into(),
        ));
    }
    // Meta will not redirect to, or fetch media over, plain HTTP. Catching it
    // here beats catching it as an opaque authorize-step rejection later.
    if !base_url.starts_with("https://") {
        return Err(AppError::InvalidInput(
            "The web deployment must be served over HTTPS — Meta refuses a plain-HTTP              redirect and will not fetch media from one."
                .into(),
        ));
    }

    // An omitted token keeps the stored one, so re-saving the URL alone does not
    // silently wipe the credential the upload route checks.
    let existing = secrets::load_web_host()?;
    let upload_token = upload_token
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| existing.map(|host| host.upload_token))
        .unwrap_or_default();

    let host = crate::webhost::WebHost {
        base_url,
        upload_token,
    };
    secrets::store_web_host(&host)?;
    Ok(WebHostView {
        redirect_uri: host.redirect_uri(),
        has_token: !host.upload_token.is_empty(),
        base_url: host.base_url,
    })
}

#[tauri::command]
pub fn forget_web_host() -> Result<()> {
    secrets::forget_web_host()
}

// ─── Meta ads, through Meta's own MCP server ────────────────────────────────

/// Meta's current ads tool catalogue, fetched live rather than mirrored — see
/// [`crate::metaads`] for why Windbag forwards instead of reimplementing.
#[tauri::command(async)]
pub fn meta_ads_tools(state: State<'_, AppState>) -> Result<serde_json::Value> {
    crate::metaads::AdsClient::from_store(&state.db)?.list_tools()
}

#[tauri::command(async)]
pub fn meta_ads_call(
    state: State<'_, AppState>,
    name: String,
    arguments: Option<serde_json::Value>,
) -> Result<serde_json::Value> {
    let arguments = arguments.unwrap_or_else(|| serde_json::json!({}));
    crate::metaads::AdsClient::from_store(&state.db)?.call_tool(&name, &arguments)
}
