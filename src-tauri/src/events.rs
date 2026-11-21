//! Event names (mirrored in src/lib/api.ts) and helpers.

use tauri::{AppHandle, Emitter};
use wwta_core::db::Db;

pub const MODEL_PROGRESS: &str = "model://progress";
pub const TRANSCRIPTION_PROGRESS: &str = "transcription://progress";
pub const LLM_STREAM: &str = "llm://stream";
pub const RECORDING_UPDATED: &str = "library://recording-updated";
pub const RECORDING_DELETED: &str = "library://recording-deleted";

pub fn recording_updated(app: &AppHandle, db: &Db, id: &str) {
    if let Ok(Some(r)) = db.get_recording(id) {
        let _ = app.emit(RECORDING_UPDATED, r);
    }
}
