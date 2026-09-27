//! Window material — the OS effect drawn behind a transparent window.
//!
//! The renderer stamps `data-material` from what this ACTUALLY applied, never
//! from the preference: macOS and Windows always accept, Linux compositors
//! routinely do not, and a see-through sidebar with nothing frosting behind it
//! shows the desktop wallpaper through the queue.

use tauri::WebviewWindow;

#[cfg(target_os = "macos")]
use objc2::msg_send;
#[cfg(target_os = "macos")]
use objc2_app_kit::{
    NSTitlebarSeparatorStyle, NSWindowStyleMask, NSWindowTitleVisibility, NSWindowToolbarStyle,
};

/// Applies a material and returns what the OS really did — `off` whenever the
/// platform or the compositor refused.
pub fn apply_material(window: &WebviewWindow, requested: &str) -> String {
    #[cfg(target_os = "macos")]
    {
        use window_vibrancy::{NSVisualEffectMaterial, apply_vibrancy, clear_vibrancy};

        let material = match requested {
            "standard" => Some(NSVisualEffectMaterial::Sidebar),
            "strong" => Some(NSVisualEffectMaterial::UnderWindowBackground),
            _ => None,
        };
        // Cleared first on every change: `apply_vibrancy` inserts a NEW effect
        // view each call rather than replacing the last one, so standard → strong
        // would stack a second material over the first.
        let _ = clear_vibrancy(window);
        if let Some(material) = material {
            if apply_vibrancy(window, material, None, None).is_ok() {
                requested.to_string()
            } else {
                "off".to_string()
            }
        } else {
            "off".to_string()
        }
    }

    #[cfg(target_os = "windows")]
    {
        use window_vibrancy::{apply_mica, clear_mica};

        if requested == "off" {
            let _ = clear_mica(window);
            return "off".to_string();
        }
        // Mica has no strength control; both levels map to the one effect and the
        // renderer's own alpha is what differs.
        return if apply_mica(window, None).is_ok() {
            requested.to_string()
        } else {
            "off".to_string()
        };
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        // Linux has no portable equivalent that can be applied from here, so the
        // honest answer is that nothing was applied.
        let _ = (window, requested);
        "off".to_string()
    }
}

/// The macOS title bar the traffic lights live in.
///
/// Placement is left to `AppKit` rather than pinned from `tauri.conf.json`. An
/// EMPTY unified toolbar gives the window the standard toolbar-window title-bar
/// height, and `AppKit` centres the lights in it on every macOS version — the same
/// geometry Finder has. The toolbar paints nothing: the title bar is
/// transparent, the title hidden, the separator off.
///
/// Deliberately NOT `trafficLightPosition`: tao implements it by growing the
/// title-bar container and re-setting only the buttons' x on every drawRect, and
/// macOS 26+ no longer moves the buttons with that container — so `y` is inert
/// there while still shifting them on 14/15, which is one placement per OS.
/// Setting the buttons' frames by hand is no better: `AppKit` re-lays them out.
///
/// The renderer sizes its top strips to the band this produces (`lib/chrome.ts`).
#[cfg(target_os = "macos")]
pub fn configure_titlebar(window: &WebviewWindow) {
    let Ok(ns_window) = window.ns_window() else {
        log::warn!("no NSWindow to configure; the traffic lights keep AppKit's defaults");
        return;
    };
    let ns_window = ns_window.cast::<objc2::runtime::AnyObject>();

    // SAFETY: `ns_window` is the live NSWindow tauri returned for this webview
    // window, and every call is made on the main thread (this runs inside the
    // setup callback). The style-mask assignment ORs into the existing mask so
    // the bits we do not own survive.
    #[expect(unsafe_code)]
    unsafe {
        let _: () = msg_send![ns_window, setTitlebarAppearsTransparent: true];
        let _: () = msg_send![ns_window, setTitleVisibility: NSWindowTitleVisibility::Hidden];

        let current_style: NSWindowStyleMask = msg_send![ns_window, styleMask];
        let _: () = msg_send![
            ns_window,
            setStyleMask: current_style | NSWindowStyleMask::FullSizeContentView
        ];

        let toolbar: *mut objc2::runtime::AnyObject = msg_send![objc2::class!(NSToolbar), new];
        let _: () = msg_send![toolbar, setAllowsUserCustomization: false];
        let _: () = msg_send![ns_window, setToolbar: toolbar];
        let _: () = msg_send![ns_window, setToolbarStyle: NSWindowToolbarStyle::Unified];
        let _: () = msg_send![ns_window, setTitlebarSeparatorStyle: NSTitlebarSeparatorStyle::None];
    }
}

#[tauri::command]
pub fn set_window_material(window: WebviewWindow, material: String) -> String {
    apply_material(&window, &material)
}
