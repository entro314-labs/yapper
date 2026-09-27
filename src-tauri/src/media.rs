//! Attachments. Windbag never copies a picked file — the store keeps the PATH and
//! the bytes are read at publish time. That keeps the data directory small and
//! means editing the image on disk edits what goes out, but it also means a file
//! moved between scheduling and publishing is a real failure the user must see,
//! which is why [`load`] reports the missing path rather than skipping it.

use std::path::Path;

use crate::error::{AppError, Result};
use crate::platforms::MediaItem;

/// 40 MB. Above the largest single-image limit any of the five accepts, so the
/// platform's own rejection is what the user reads for anything smaller — this
/// only stops the app from trying to hold something absurd in memory.
pub const MAX_BYTES: u64 = 40 * 1024 * 1024;

/// What the picker resolved a chosen file to, before it goes into the store.
#[derive(Debug)]
pub struct Resolved {
    pub path: String,
    pub mime: String,
    pub bytes: i64,
}

pub fn resolve(path: &str) -> Result<Resolved> {
    let file = Path::new(path);
    let meta = std::fs::metadata(file)
        .map_err(|e| AppError::InvalidInput(format!("Cannot read `{path}`: {e}")))?;
    if !meta.is_file() {
        return Err(AppError::InvalidInput(format!("`{path}` is not a file.")));
    }
    if meta.len() > MAX_BYTES {
        return Err(AppError::InvalidInput(format!(
            "`{path}` is {} MB. The limit is {} MB.",
            meta.len() / 1_048_576,
            MAX_BYTES / 1_048_576
        )));
    }
    Ok(Resolved {
        path: path.to_string(),
        mime: mime_for(file)?,
        bytes: i64::try_from(meta.len()).unwrap_or(i64::MAX),
    })
}

/// Reads the bytes for one stored attachment at publish time.
pub fn load(media: &crate::db::Media) -> Result<MediaItem> {
    let bytes = std::fs::read(&media.path).map_err(|e| {
        AppError::InvalidInput(format!(
            "The attachment `{}` could not be read ({e}). It may have been moved or deleted \
             since this post was scheduled.",
            media.path
        ))
    })?;
    Ok(MediaItem {
        bytes,
        mime: media.mime.clone(),
        alt_text: media.alt_text.clone(),
    })
}

/// By extension, not by sniffing: every platform here takes the declared type at
/// face value, and the set that can actually be posted is small and closed.
fn mime_for(path: &Path) -> Result<String> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();

    let mime = match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "mp4" => "video/mp4",
        other => {
            return Err(AppError::InvalidInput(format!(
                "`.{other}` files cannot be posted. Use PNG, JPEG, GIF, WebP or MP4."
            )));
        }
    };
    Ok(mime.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_image_types_map_to_their_mime() {
        assert_eq!(mime_for(Path::new("/a/b.PNG")).expect("png"), "image/png");
        assert_eq!(
            mime_for(Path::new("/a/b.jpeg")).expect("jpeg"),
            "image/jpeg"
        );
        assert_eq!(mime_for(Path::new("/a/b.mp4")).expect("mp4"), "video/mp4");
    }

    #[test]
    fn an_unpostable_type_is_rejected_by_name() {
        let err = mime_for(Path::new("/a/notes.pdf")).expect_err("pdf");
        assert!(err.to_string().contains(".pdf"), "{err}");
    }

    #[test]
    fn a_file_with_no_extension_is_rejected() {
        assert!(mime_for(Path::new("/a/screenshot")).is_err());
    }

    #[test]
    fn a_missing_file_names_itself_in_the_error() {
        let err = resolve("/definitely/not/here.png").expect_err("missing");
        assert!(
            err.to_string().contains("/definitely/not/here.png"),
            "{err}"
        );
    }
}
