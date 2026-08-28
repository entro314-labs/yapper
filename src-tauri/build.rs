fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    #[cfg(target_os = "macos")]
    prepare_apple_intelligence_dylib();
    tauri_build::build();
}

/// Put `libappleai.dylib` where the linker and the running app can both find it.
///
/// `tauri-plugin-apple-intelligence` links a prebuilt Swift dylib shipped inside
/// its own crate directory. Its build script emits the `-L` search path, so the
/// LINK succeeds — but the rpath it adds applies only to that crate's own test
/// binaries, and its README is explicit that the host app owns bundling and
/// rpaths. Without this, the executable carries a bare `@rpath/libappleai.dylib`
/// load command that nothing resolves, and every run — including `cargo test` —
/// dies at launch with "Library not loaded".
///
/// So: copy the dylib into `resources/`, which `tauri.conf.json` bundles into
/// `Contents/Resources/resources/` of the `.app`, and add the rpaths covering
/// that bundled location, a flat placement, and the crate's own prebuilt
/// directory for a plain `cargo run` or `cargo test` outside any bundle.
#[cfg(target_os = "macos")]
fn prepare_apple_intelligence_dylib() {
    use std::path::PathBuf;
    use std::{env, fs};

    const DYLIB: &str = "libappleai.dylib";

    let Ok(manifest_dir) = env::var("CARGO_MANIFEST_DIR") else {
        return;
    };
    // A build where the dependency is not vendored yet — or a future version
    // that stops shipping the dylib — must fail at the LINK with the linker's
    // own message rather than here with a worse one. Hence the quiet return.
    let Some(source) = locate_prebuilt_dylib(DYLIB) else {
        return;
    };

    let resources = PathBuf::from(&manifest_dir).join("resources");
    if fs::create_dir_all(&resources).is_ok() {
        let _ = fs::copy(&source, resources.join(DYLIB));
    }

    println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Resources/resources");
    println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Resources");
    if let Some(parent) = source.parent() {
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", parent.display());
    }
    println!("cargo:rerun-if-changed={}", source.display());
}

/// Locate the vendored prebuilt dylib via `cargo metadata` rather than a
/// hand-built registry path: the crate may be vendored, patched or
/// path-overridden, and only cargo knows where it actually came from.
#[cfg(target_os = "macos")]
fn locate_prebuilt_dylib(dylib: &str) -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    use std::process::Command;

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let output = Command::new(cargo)
        .args(["metadata", "--format-version", "1"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let manifest = metadata
        .get("packages")?
        .as_array()?
        .iter()
        .find(|package| {
            package.get("name").and_then(serde_json::Value::as_str)
                == Some("tauri-plugin-apple-intelligence")
        })?
        .get("manifest_path")?
        .as_str()?;
    let path = PathBuf::from(manifest)
        .parent()?
        .join("prebuilt")
        .join(dylib);
    path.exists().then_some(path)
}
