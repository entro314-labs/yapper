//! Window material — the OS effect drawn behind a transparent window.
//!
//! The renderer stamps `data-material` from what this ACTUALLY applied, never
//! from the preference: macOS and Windows always accept, Linux compositors
//! routinely do not, and a see-through sidebar with nothing frosting behind it
//! shows the desktop wallpaper through the queue.

use tauri::WebviewWindow;

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
        if let Some(material) = material {
            if apply_vibrancy(window, material, None, None).is_ok() {
                requested.to_string()
            } else {
                "off".to_string()
            }
        } else {
            let _ = clear_vibrancy(window);
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

#[tauri::command]
pub fn set_window_material(window: WebviewWindow, material: String) -> String {
    apply_material(&window, &material)
}
