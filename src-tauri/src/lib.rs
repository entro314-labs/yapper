//! Windbag — a desktop composer and scheduler for social platforms with an open API.

mod ai;
mod atproto;
mod commands;
pub mod db;
mod dpop;
mod error;
mod http;
pub mod mcp;
mod media;
pub mod metaads;
mod oauth;
mod platforms;
mod scheduler;
mod secrets;
mod stats;
mod update;
pub mod webhost;
mod windowing;

/// The argument that turns the `windbag` executable into the agent door's stdio
/// MCP server instead of the app (see `main.rs`).
pub const MCP_FLAG: &str = "--mcp";

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use tauri::{Manager, WindowEvent};
use tauri_plugin_log::{RotationStrategy, Target, TargetKind, TimezoneStrategy};

use commands::AppState;
use scheduler::Scheduler;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // First, so every later plugin's setup is logged.
        .plugin(logger())
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
        // Signed auto-updates. The plugin only makes the check possible; when a
        // downloaded bundle is actually swapped in is decided in `update.rs`,
        // and that is deliberately not while the app is running.
        .plugin(tauri_plugin_updater::Builder::new().build())
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
            commands::deliver_auth_callback,
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
            commands::get_refresh_cost,
            commands::refresh_engagement,
            commands::get_settings,
            commands::update_settings,
            commands::oauth_redirect_uri,
            commands::mcp_command,
            commands::get_web_host,
            commands::save_web_host,
            commands::forget_web_host,
            commands::meta_ads_tools,
            commands::meta_ads_call,
            update::check_for_update,
            update::install_update,
            update::restart_and_install,
            update::update_staged,
            update::update_install_support,
            windowing::set_window_material,
        ])
        .build(tauri::generate_context!())
        .expect("Windbag failed to start")
        .run(|app, event| match event {
            // macOS: clicking the Dock icon does not start a second process, so
            // the single-instance handler never fires and a window hidden by the
            // close button would be unreachable — quit-and-relaunch would be the
            // only way back. This is the one event that offers it.
            tauri::RunEvent::Reopen { .. } => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            // The quit path, and the only safe moment to swap the bundle:
            // replacing it while the process runs breaks its code signature.
            // Closing the window only hides Windbag, so this really is the exit.
            tauri::RunEvent::ExitRequested { .. } => update::install_pending_on_exit(app),
            _ => {}
        });
}

/// stderr for `tauri dev`; the log directory (~/Library/Logs/com.entro314.windbag
/// on macOS) for a bundled build, where stderr goes nowhere. Local time, because
/// the question these answer is "what happened at 09:00".
fn logger() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri_plugin_log::Builder::new()
        .clear_targets()
        .target(Target::new(TargetKind::Stderr))
        .target(Target::new(TargetKind::LogDir { file_name: None }))
        .timezone_strategy(TimezoneStrategy::UseLocal)
        .max_file_size(5_000_000)
        .rotation_strategy(RotationStrategy::KeepSome(3))
        .level(log::LevelFilter::Warn)
        .level_for(
            "windbag_lib",
            if cfg!(debug_assertions) {
                log::LevelFilter::Debug
            } else {
                log::LevelFilter::Info
            },
        )
        .build()
}

/// Everything the app needs standing up before the first frame: the store, the
/// login item reconciled against the stored preference, the deep-link route the
/// Bluesky sign-in comes back on, the scheduler thread, and the window.
///
/// Lifted out of the builder chain because it is the only part of `run` with any
/// logic in it — the rest is plugin registration, which reads as a list.
fn setup(app: &mut tauri::App) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let path = db::data_dir()?.join("windbag.sqlite3");
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
    // Empty until something is downloaded; `install_pending_on_exit` reads it on
    // the way out.
    app.manage(update::PendingUpdate(std::sync::Mutex::new(None)));

    // Windows and Linux: closing the window only hides it, and neither gets
    // tauri's default app menu, so without a tray there is no way to quit — and
    // a staged update installs on quit. macOS has the Dock and the app menu.
    // Gated at runtime rather than by `cfg` so every build compiles this code.
    if cfg!(not(target_os = "macos")) {
        install_tray(app)?;
    }

    if let Some(window) = app.get_webview_window("main") {
        windowing::apply_material(&window, "standard");
        // Before the window is shown, so the title bar never jumps.
        #[cfg(target_os = "macos")]
        windowing::configure_titlebar(&window);
        // Shown only once the renderer has painted, so the first frame is
        // never an unthemed white flash against a transparent window.
        let launched_hidden = std::env::args().any(|arg| arg == "--hidden");
        if !launched_hidden {
            let _ = window.show();
        }
    }
    Ok(())
}

/// A tray icon whose menu shows the window and quits the app. Quit goes through
/// `exit`, so it raises `ExitRequested` like any other quit and a staged update
/// is installed on the way out.
fn install_tray(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::TrayIconBuilder;

    let show = MenuItem::with_id(app, "show", "Show Windbag", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Windbag", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;

    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("Windbag")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            "quit" => app.exit(0),
            _ => {}
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}
