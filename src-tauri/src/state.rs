//! Shared application state.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, RwLock};

use anyhow::Result;
use wwta_core::db::Db;
use wwta_core::types::{ModelStatus, Settings};

use crate::worker::Worker;

pub struct Paths {
    pub data: PathBuf,
    pub models: PathBuf,
    pub library: PathBuf,
    pub settings: PathBuf,
    pub vad_model: PathBuf,
}

impl Paths {
    pub fn new(data: PathBuf) -> Result<Self> {
        let models = data.join("models");
        let library = data.join("library");
        std::fs::create_dir_all(&models)?;
        std::fs::create_dir_all(&library)?;
        let vad_model = wwta_core::asr::ensure_vad_model(&data.join("vad"))?;
        Ok(Self {
            settings: data.join("settings.json"),
            data,
            models,
            library,
            vad_model,
        })
    }

    pub fn recording_dir(&self, id: &str) -> PathBuf {
        self.library.join(id)
    }
    pub fn pcm_path(&self, id: &str) -> PathBuf {
        self.recording_dir(id).join("pcm16k.wav")
    }
    pub fn peaks_path(&self, id: &str) -> PathBuf {
        self.recording_dir(id).join("peaks.u8")
    }
}

/// Tiny counting semaphore for blocking threads (limits parallel decodes).
pub struct Slots {
    free: Mutex<usize>,
    cv: std::sync::Condvar,
}

pub struct SlotGuard<'a>(&'a Slots);

impl Slots {
    pub fn new(n: usize) -> Self {
        Self {
            free: Mutex::new(n),
            cv: std::sync::Condvar::new(),
        }
    }
    pub fn acquire(&self) -> SlotGuard<'_> {
        let mut free = self.free.lock().unwrap();
        while *free == 0 {
            free = self.cv.wait(free).unwrap();
        }
        *free -= 1;
        SlotGuard(self)
    }
}

impl Drop for SlotGuard<'_> {
    fn drop(&mut self) {
        *self.0.free.lock().unwrap() += 1;
        self.0.cv.notify_one();
    }
}

pub struct AppState {
    pub paths: Paths,
    pub decode_slots: Slots,
    pub db: Mutex<Db>,
    pub settings: RwLock<Settings>,
    pub worker: Worker,
    /// Running model installs: id -> cancel flag
    pub model_jobs: Mutex<HashMap<String, Arc<AtomicBool>>>,
    /// Transient model status (downloading/converting/error) overriding the on-disk state.
    pub model_status: Mutex<HashMap<String, (ModelStatus, Option<String>)>>,
    /// Running LLM requests: request id -> cancel flag
    pub llm_jobs: Mutex<HashMap<String, Arc<AtomicBool>>>,
    /// Anonymous usage analytics (no-op when disabled or not configured)
    pub analytics: Arc<crate::telemetry::Analytics>,
    /// Cached OpenRouter model list
    pub llm_models: tokio::sync::Mutex<Option<Vec<wwta_core::types::LlmModelInfo>>>,
}

pub fn load_settings(path: &Path) -> Settings {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn store_settings(path: &Path, s: &Settings) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(s)?)?;
    std::fs::rename(&tmp, path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

impl AppState {
    pub fn settings(&self) -> Settings {
        self.settings.read().unwrap().clone()
    }
}
