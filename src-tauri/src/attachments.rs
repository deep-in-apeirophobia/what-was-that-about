//! Images and voice clips attached to Ask messages: validation, storage in the app data dir
//! (voice is normalized to 16 kHz mono WAV) and cleanup of files nothing refers to anymore.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use anyhow::{anyhow, bail, Result};
use tauri::{AppHandle, Manager};
use wwta_core::audio::{self, DecodeError};
use wwta_core::types::{AttachmentUpload, ChatAttachment};

use crate::state::AppState;

/// Hard limit for an image file.
pub const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;
/// Larger images are shrunk by the webview first (some providers reject images over 5 MB).
pub const SHRINK_IMAGE_ABOVE: u64 = 3_500_000;
/// Longest voice message; longer audio belongs in the library as a recording.
pub const MAX_AUDIO_SECS: f64 = 20.0 * 60.0;
/// Files bigger than this aren't read as voice messages at all.
const MAX_AUDIO_FILE_BYTES: u64 = 512 * 1024 * 1024;

const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];
/// Image formats only the webview may be able to read; they're converted to JPEG there.
const CONVERT_IMAGE_EXTS: &[&str] = &["bmp", "avif", "heic", "heif", "tif", "tiff", "ico"];
const AUDIO_EXTS: &[&str] = &[
    "mp3", "m4a", "aac", "wav", "flac", "ogg", "opus", "oga", "webm", "mp4", "mkv", "mov", "wma",
    "amr", "3gp", "aif", "aiff", "caf",
];

/// Image type from the file's first bytes (only formats every vision model accepts).
pub fn sniff_image(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

fn ext_of(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default()
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn register(app: &AppHandle, a: &ChatAttachment) -> Result<()> {
    let state = app.state::<AppState>();
    state.db.lock().unwrap().add_attachment(a)?;
    let _ = app.asset_protocol_scope().allow_file(&a.path);
    Ok(())
}

/// Store image bytes (PNG, JPEG, GIF or WebP).
pub fn save_image(app: &AppHandle, name: &str, bytes: &[u8]) -> Result<ChatAttachment> {
    if bytes.len() > MAX_IMAGE_BYTES {
        bail!("the image is too large (max 20 MB)");
    }
    let mime = sniff_image(bytes)
        .ok_or_else(|| anyhow!("unsupported image type; use PNG, JPEG, WebP or GIF"))?;
    let id = new_id();
    let ext = mime.trim_start_matches("image/");
    let path = app
        .state::<AppState>()
        .paths
        .attachments
        .join(format!("{id}.{ext}"));
    std::fs::write(&path, bytes)?;
    let a = ChatAttachment {
        id,
        kind: "image".into(),
        name: clean_name(name, "Image"),
        mime: mime.into(),
        size_bytes: bytes.len() as u64,
        duration_sec: None,
        path: path.to_string_lossy().into_owned(),
        text: None,
        text_model: None,
    };
    register(app, &a)?;
    Ok(a)
}

/// Store a voice clip given as 16 kHz mono samples.
pub fn save_audio(app: &AppHandle, name: &str, samples: &[f32]) -> Result<ChatAttachment> {
    let secs = audio::duration_secs(samples.len());
    if secs < 0.3 {
        bail!("the voice message is empty");
    }
    // a little slack: recordings that auto-stop at the limit end mid-buffer
    if secs > MAX_AUDIO_SECS + 2.0 {
        bail!(
            "voice messages can be up to 20 minutes; import longer audio into the library instead"
        );
    }
    audio::ensure_audible(samples)?;
    let id = new_id();
    let path = app
        .state::<AppState>()
        .paths
        .attachments
        .join(format!("{id}.wav"));
    audio::write_wav_16k(&path, samples)?;
    let a = ChatAttachment {
        id,
        kind: "audio".into(),
        name: clean_name(name, "Voice message"),
        mime: "audio/wav".into(),
        size_bytes: std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
        duration_sec: Some((secs * 10.0).round() / 10.0),
        path: path.to_string_lossy().into_owned(),
        text: None,
        text_model: None,
    };
    register(app, &a)?;
    Ok(a)
}

fn clean_name(name: &str, fallback: &str) -> String {
    let n: String = name
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(120)
        .collect();
    if n.is_empty() {
        fallback.into()
    } else {
        n
    }
}

/// Attach a file the user picked or dropped.
pub async fn from_path(app: &AppHandle, path: &str) -> Result<AttachmentUpload> {
    let p = Path::new(path);
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = ext_of(p);
    let size = std::fs::metadata(p)
        .map_err(|e| anyhow!("can't read {name}: {e}"))?
        .len();
    let convert = |kind: &str| {
        let _ = app.asset_protocol_scope().allow_file(p);
        Ok(AttachmentUpload::Convert {
            path: path.to_string(),
            kind: kind.into(),
        })
    };
    if IMAGE_EXTS.contains(&ext.as_str()) {
        if size > SHRINK_IMAGE_ABOVE {
            return convert("image");
        }
        let bytes = tokio::fs::read(p).await?;
        if sniff_image(&bytes).is_none() {
            return convert("image");
        }
        let attachment = save_image(app, &name, &bytes)?;
        return Ok(AttachmentUpload::Ready { attachment });
    }
    if CONVERT_IMAGE_EXTS.contains(&ext.as_str()) {
        return convert("image");
    }
    if AUDIO_EXTS.contains(&ext.as_str()) {
        if size > MAX_AUDIO_FILE_BYTES {
            bail!("{name} is too large for a voice message; import it into the library instead");
        }
        let owned = p.to_path_buf();
        let decoded = tokio::task::spawn_blocking(move || {
            audio::decode_to_16k_mono(&owned, &AtomicBool::new(false), &mut |_| {})
        })
        .await?;
        return match decoded {
            Ok(samples) => Ok(AttachmentUpload::Ready {
                attachment: save_audio(app, &name, &samples)?,
            }),
            Err(DecodeError::Unsupported(msg)) => {
                log::info!("native decoder can't read attachment {name}: {msg}");
                convert("audio")
            }
            Err(DecodeError::Cancelled) => bail!("cancelled"),
            Err(DecodeError::Other(e)) => Err(e),
        };
    }
    bail!("{name}: attach images (PNG, JPEG, WebP, GIF) or audio files")
}

/// Undo a percent-encoded header value (the webview sends file names URI-encoded).
pub fn percent_decode(s: &str) -> String {
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Delete files in the attachments folder that no message refers to. Pending attachments
/// don't survive a restart, so this runs at startup.
pub fn sweep(app: &AppHandle) {
    let state = app.state::<AppState>();
    let keep = match state.db.lock().unwrap().prune_attachments() {
        Ok(k) => k,
        Err(e) => {
            log::warn!("attachment cleanup skipped: {e:#}");
            return;
        }
    };
    let Ok(dir) = std::fs::read_dir(&state.paths.attachments) else {
        return;
    };
    for entry in dir.flatten() {
        let path = entry.path();
        if !keep.contains(path.to_string_lossy().as_ref()) {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Remove attachment files (after their messages were deleted).
pub fn remove_files(paths: &[String]) {
    for p in paths {
        let _ = std::fs::remove_file(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_image_types() {
        assert_eq!(sniff_image(b"\x89PNG\r\n\x1a\nrest"), Some("image/png"));
        assert_eq!(sniff_image(b"\xff\xd8\xff\xe0"), Some("image/jpeg"));
        assert_eq!(sniff_image(b"GIF89a.."), Some("image/gif"));
        assert_eq!(sniff_image(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff_image(b"RIFF\0\0\0\0WAVEfmt "), None);
        assert_eq!(sniff_image(b"<svg"), None);
    }

    #[test]
    fn decodes_percent_encoded_names() {
        assert_eq!(percent_decode("voice%20note.wav"), "voice note.wav");
        assert_eq!(percent_decode("%D8%B3%D9%84%D8%A7%D9%85.png"), "سلام.png");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
    }

    #[test]
    fn cleans_names() {
        assert_eq!(clean_name("  a\u{0007}b.png ", "Image"), "ab.png");
        assert_eq!(clean_name("", "Image"), "Image");
    }
}
