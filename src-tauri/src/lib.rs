//! Yapper — a desktop composer and scheduler for social platforms with an open API.

mod ai;
mod atproto;
mod commands;
pub mod db;
mod dpop;
mod error;
mod http;
pub mod mcp;
mod media;
mod oauth;
mod platforms;
mod scheduler;
mod secrets;
mod stats;
mod windowing;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use tauri::{Manager, WindowEvent};

use commands::AppState;
use scheduler::Scheduler;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(
        if cfg!(debug_assertions) {
            "yapper=debug,warn"
        } else {
            "yapper=info,warn"
        },
    ))
    .init();

    tauri::Builder::default()
        // A second launch focuses the running window rather than starting a rival
        // scheduler against the same store.
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // A second launch is how Windows and Linux deliver a deep link: the
            // OS starts the app again with the URL as an argument. Forward it
            // before focusing, so a sign-in completes even if the window was
            // hidden.
            for arg in args.iter().skip(1) {
                if arg.starts_with(atproto::CALLBACK_SCHEME) {
                    atproto::deliver_callback(arg);
                }
            }
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        // AT Protocol OAuth hands its code back on a custom URI scheme; a
        // loopback redirect is invalid there for a native client.
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_dialog::init())
        // Apple's on-device model. Registered unconditionally: the plugin ships
        // its own non-Apple-silicon stub, so one builder covers every platform
        // and the assistant tier simply reports "unavailable" off macOS.
        .plugin(tauri_plugin_apple_intelligence::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        // Started with `--hidden` so a login launch waits in the background: a
        // scheduler that only runs when you happen to open the app is not one.
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--hidden"]),
        ))
        .setup(setup)
        .on_window_event(|window, event| {
            // Closing the window hides it instead of quitting: the scheduler has
            // to keep running, and on macOS closing a window has never meant
            // quitting the app anyway.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_platforms,
            commands::list_accounts,
            commands::connect_account,
            commands::disconnect_account,
            commands::get_app_credentials,
            commands::save_app_credentials,
            commands::forget_app_credentials,
            commands::list_posts,
            commands::list_attempts,
            commands::save_post,
            commands::delete_post,
            commands::publish_now,
            commands::reschedule_post,
            commands::retry_target,
            commands::check_post,
            commands::resolve_media,
            commands::list_notes,
            commands::save_note,
            commands::delete_note,
            commands::ai_availability,
            commands::suggest_posts,
            commands::get_stats,
            commands::refresh_engagement,
            commands::get_settings,
            commands::update_settings,
            commands::oauth_redirect_uri,
            windowing::set_window_material,
        ])
        .build(tauri::generate_context!())
        .expect("Yapper failed to start")
        .run(|app, event| {
            // macOS: clicking the Dock icon does not start a second process, so
            // the single-instance handler never fires and a window hidden by the
            // close button would be unreachable — quit-and-relaunch would be the
            // only way back. This is the one event that offers it.
            if let tauri::RunEvent::Reopen { .. } = event
                && let Some(window) = app.get_webview_window("main")
            {
                let _ = window.show();
                let _ = window.set_focus();
            }
        });
}

/// Everything the app needs standing up before the first frame: the store, the
/// login item reconciled against the stored preference, the deep-link route the
/// Bluesky sign-in comes back on, the scheduler thread, and the window.
///
/// Lifted out of the builder chain because it is the only part of `run` with any
/// logic in it — the rest is plugin registration, which reads as a list.
fn setup(app: &mut tauri::App) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let path = db::data_dir()?.join("yapper.sqlite3");
    let database = Arc::new(db::Db::open_at(&path)?);
    log::info!("store at {}", path.display());

    // The stored preference is the authority: an OS update or a moved
    // .app can drop a login item, and without this the switch would keep
    // reading "on" for a registration that no longer exists.
    let wants_autostart = database
        .get_meta("launch_at_login")
        .ok()
        .flatten()
        .is_some_and(|value| value == "true");
    if let Err(err) = commands::apply_autostart(app.handle(), wants_autostart) {
        log::warn!("could not reconcile the login item: {err}");
    }

    // macOS delivers deep links to the RUNNING process through this
    // event rather than by relaunching, so both routes are wired.
    {
        use tauri_plugin_deep_link::DeepLinkExt;
        app.deep_link().on_open_url(|event| {
            for url in event.urls() {
                atproto::deliver_callback(url.as_str());
            }
        });
    }

    let scheduler = Scheduler::start(app.handle().clone(), Arc::clone(&database));
    app.manage(AppState {
        db: database,
        scheduler,
        connecting: AtomicBool::new(false),
    });

    if let Some(window) = app.get_webview_window("main") {
        windowing::apply_material(&window, "standard");
        // Shown only once the renderer has painted, so the first frame is
        // never an unthemed white flash against a transparent window.
        let launched_hidden = std::env::args().any(|arg| arg == "--hidden");
        if !launched_hidden {
            let _ = window.show();
        }
    }
    Ok(())
}
