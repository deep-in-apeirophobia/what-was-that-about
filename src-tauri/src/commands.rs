//! Tauri commands. Names and argument shapes mirror `src/lib/api.ts`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tauri::{AppHandle, Emitter, Manager, State};
use wwta_core::types::*;
use wwta_core::{audio, export, ggml, models, search};

use crate::events;
use crate::state::{store_settings, AppState};
use crate::worker::{self, Job};

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn anyhow_err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

// ------------------------------------------------------------------ system & settings

#[tauri::command]
pub fn get_system_info(state: State<'_, AppState>) -> SystemInfo {
    use wwta_core::asr::whisper;
    let gpu_available = whisper::GPU_BACKEND != "none" && whisper::runtime_available();
    SystemInfo {
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        app_version: env!("CARGO_PKG_VERSION").into(),
        cpu_cores: std::thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(1),
        total_memory_bytes: crate::sysinfo::total_memory(),
        gpu_backend: whisper::GPU_BACKEND.into(),
        gpu_devices: if gpu_available {
            crate::sysinfo::gpu_devices_cached()
        } else {
            vec![]
        },
        gpu_available: gpu_available && !crate::sysinfo::gpu_devices_cached().is_empty(),
        data_dir: state.paths.data.to_string_lossy().into_owned(),
        engines: Engines {
            whisper: whisper::runtime_available(),
            shenava: true,
        },
    }
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Settings {
    state.settings()
}

#[tauri::command]
pub fn save_settings(state: State<'_, AppState>, settings: Settings) -> CmdResult<Settings> {
    store_settings(&state.paths.settings, &settings).map_err(anyhow_err)?;
    let old = std::mem::replace(&mut *state.settings.write().unwrap(), settings.clone());
    state.analytics.set_enabled(settings.analytics_enabled);
    crate::telemetry::set_crash_reports(settings.crash_reports_enabled);
    let (o, n) = (
        serde_json::to_value(&old).unwrap_or_default(),
        serde_json::to_value(&settings).unwrap_or_default(),
    );
    let changed: Vec<String> = n
        .as_object()
        .map(|m| {
            m.iter()
                .filter(|(k, v)| o.get(*k) != Some(*v))
                .map(|(k, _)| k.clone())
                .collect()
        })
        .unwrap_or_default();
    if !changed.is_empty() {
        state.analytics.capture(
            "settings changed",
            serde_json::json!({
                "changed": changed,
                "llm_model": settings.llm_model,
                "embedding_model": settings.embedding_model,
                "default_model": settings.default_model_id,
                "ai_language": settings.ai_language,
                "use_gpu": settings.use_gpu,
                "whisper_quant": settings.whisper_quant,
                "auto_analyze": settings.auto_analyze,
                "theme": settings.theme,
                "openrouter_key_set": !settings.openrouter_api_key.trim().is_empty(),
            }),
        );
    }
    Ok(settings)
}

/// Product events the UI may record (allow-listed; properties are reduced to scalars).
#[tauri::command]
pub fn track_ui(state: State<'_, AppState>, event: String, props: Option<serde_json::Value>) {
    const ALLOWED: &[&str] = &[
        "onboarding completed",
        "onboarding skipped",
        "doc template used",
        "search opened",
        "audio played",
    ];
    if !ALLOWED.contains(&event.as_str()) {
        return;
    }
    let mut clean = serde_json::Map::new();
    if let Some(serde_json::Value::Object(m)) = props {
        for (k, v) in m.into_iter().take(12) {
            let ok = match &v {
                serde_json::Value::String(s) => s.chars().count() <= 64,
                serde_json::Value::Bool(_) | serde_json::Value::Number(_) => true,
                _ => false,
            };
            if ok {
                clean.insert(k, v);
            }
        }
    }
    state
        .analytics
        .capture(&event, serde_json::Value::Object(clean));
}

// ------------------------------------------------------------------ models

fn model_infos(state: &AppState) -> Vec<ModelInfo> {
    let overrides = state.model_status.lock().unwrap().clone();
    models::catalog()
        .iter()
        .map(|spec| models::model_info(&state.paths.models, spec, overrides.get(spec.id).cloned()))
        .collect()
}

#[tauri::command]
pub fn list_models(state: State<'_, AppState>) -> Vec<ModelInfo> {
    model_infos(&state)
}

fn emit_model(app: &AppHandle, p: ModelProgress) {
    let _ = app.emit(events::MODEL_PROGRESS, p);
}

/// Shared driver for downloads and manual imports.
fn spawn_model_job<F, Fut>(
    app: AppHandle,
    model_id: String,
    method: &'static str,
    run: F,
) -> CmdResult<()>
where
    F: FnOnce(AppHandle, Arc<AtomicBool>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = anyhow::Result<()>> + Send + 'static,
{
    let state = app.state::<AppState>();
    if models::spec(&model_id).is_none() {
        return Err(format!("unknown model {model_id}"));
    }
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut jobs = state.model_jobs.lock().unwrap();
        if jobs.contains_key(&model_id) {
            return Err("this model is already being installed".into());
        }
        jobs.insert(model_id.clone(), cancel.clone());
    }
    state
        .model_status
        .lock()
        .unwrap()
        .insert(model_id.clone(), (ModelStatus::Downloading, None));
    tauri::async_runtime::spawn(async move {
        let started = std::time::Instant::now();
        let result = run(app.clone(), cancel.clone()).await;
        let state = app.state::<AppState>();
        state.model_jobs.lock().unwrap().remove(&model_id);
        let took = started.elapsed().as_secs();
        match &result {
            Ok(()) => {
                let format =
                    models::read_manifest(&state.paths.models, &model_id).map(|m| m.format);
                state.analytics.capture(
                    "model installed",
                    serde_json::json!({ "model": model_id, "method": method, "format": format, "duration_sec": took }),
                );
            }
            Err(_) if cancel.load(Ordering::Relaxed) => state.analytics.capture(
                "model install cancelled",
                serde_json::json!({ "model": model_id, "method": method, "duration_sec": took }),
            ),
            Err(e) => {
                let msg = format!("{e:#}");
                let kind = crate::telemetry::error_kind(&msg);
                state.analytics.capture(
                    "model install failed",
                    serde_json::json!({ "model": model_id, "method": method, "error_kind": kind, "duration_sec": took }),
                );
                if kind != "network" {
                    crate::telemetry::report_error(
                        "model_install",
                        &msg,
                        &[("model", model_id.as_str()), ("method", method)],
                    );
                }
            }
        }
        let (stage, message) = match &result {
            Ok(()) => {
                state.model_status.lock().unwrap().remove(&model_id);
                (ModelProgressStage::Done, None)
            }
            Err(_) if cancel.load(Ordering::Relaxed) => {
                state.model_status.lock().unwrap().remove(&model_id);
                (ModelProgressStage::Cancelled, None)
            }
            Err(e) => {
                let msg = format!("{e:#}");
                log::error!("installing {model_id} failed: {msg}");
                state
                    .model_status
                    .lock()
                    .unwrap()
                    .insert(model_id.clone(), (ModelStatus::Error, Some(msg.clone())));
                (ModelProgressStage::Error, Some(msg))
            }
        };
        emit_model(
            &app,
            ModelProgress {
                model_id,
                stage,
                file: None,
                done_bytes: 0,
                total_bytes: 0,
                message,
            },
        );
    });
    Ok(())
}

fn progress_reporter(app: &AppHandle) -> impl FnMut(ModelProgress) + Send + 'static {
    let app = app.clone();
    move |p: ModelProgress| {
        let status = match p.stage {
            ModelProgressStage::Converting => ModelStatus::Converting,
            _ => ModelStatus::Downloading,
        };
        app.state::<AppState>()
            .model_status
            .lock()
            .unwrap()
            .insert(p.model_id.clone(), (status, None));
        emit_model(&app, p);
    }
}

#[tauri::command]
pub fn install_model(app: AppHandle, model_id: String) -> CmdResult<()> {
    let id = model_id.clone();
    spawn_model_job(app, model_id, "download", move |app, cancel| async move {
        let state = app.state::<AppState>();
        let spec = models::spec(&id).unwrap();
        let settings = state.settings();
        let mut report = progress_reporter(&app);
        models::install(
            &state.paths.models,
            &spec,
            ggml::OutType::parse(&settings.whisper_quant),
            settings.keep_source_weights,
            &cancel,
            &mut report,
        )
        .await
    })
}

#[tauri::command]
pub fn cancel_model_install(state: State<'_, AppState>, model_id: String) {
    if let Some(c) = state.model_jobs.lock().unwrap().get(&model_id) {
        c.store(true, Ordering::Relaxed);
    }
}

#[tauri::command]
pub fn import_model_files(app: AppHandle, model_id: String, paths: Vec<String>) -> CmdResult<()> {
    let id = model_id.clone();
    spawn_model_job(app, model_id, "import", move |app, cancel| async move {
        let state = app.state::<AppState>();
        let spec = models::spec(&id).unwrap();
        let quant = ggml::OutType::parse(&state.settings().whisper_quant);
        let root = state.paths.models.clone();
        let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
        let mut report = progress_reporter(&app);
        tokio::task::block_in_place(|| {
            models::import_files(&root, &spec, &paths, quant, &cancel, &mut report)
        })
    })
}

#[tauri::command]
pub fn delete_model(state: State<'_, AppState>, model_id: String) -> CmdResult<()> {
    if state.model_jobs.lock().unwrap().contains_key(&model_id) {
        return Err("cancel the running install first".into());
    }
    state.model_status.lock().unwrap().remove(&model_id);
    models::delete(&state.paths.models, &model_id).map_err(anyhow_err)
}

// ------------------------------------------------------------------ library

#[tauri::command]
pub fn list_recordings(state: State<'_, AppState>) -> CmdResult<Vec<Recording>> {
    state
        .db
        .lock()
        .unwrap()
        .list_recordings()
        .map_err(anyhow_err)
}

#[tauri::command]
pub fn get_recording(state: State<'_, AppState>, id: String) -> CmdResult<RecordingDetail> {
    let settings = state.settings();
    let db = state.db.lock().unwrap();
    Ok(RecordingDetail {
        recording: db.recording(&id).map_err(anyhow_err)?,
        segments: db.segments(&id).map_err(anyhow_err)?,
        notes: db.notes(&id).map_err(anyhow_err)?,
        analysis: db.analysis(&id).map_err(anyhow_err)?,
        docs: db.docs(&id).map_err(anyhow_err)?,
        threads: db.threads(Some(&id)).map_err(anyhow_err)?,
        embeddings_indexed: db
            .has_embeddings(&id, &settings.embedding_model)
            .unwrap_or(false),
    })
}

fn default_model_ready(state: &AppState) -> Option<String> {
    let s = state.settings();
    let ready = |id: &str| models::read_manifest(&state.paths.models, id).is_some();
    if ready(&s.default_model_id) {
        return Some(s.default_model_id);
    }
    models::catalog()
        .into_iter()
        .map(|m| m.id.to_string())
        .find(|id| ready(id))
}

fn allow_asset(app: &AppHandle, path: &str) {
    let _ = app.asset_protocol_scope().allow_file(path);
}

#[tauri::command]
pub async fn import_files(app: AppHandle, paths: Vec<String>) -> CmdResult<Vec<Recording>> {
    let state = app.state::<AppState>();
    let copy = state.settings().copy_into_library;
    let mut created = Vec::new();
    for p in paths {
        let src = PathBuf::from(&p);
        let container = is_container(&src);
        if !src.is_file() && !(container && src.is_dir()) {
            return Err(format!("not a file: {p}"));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let dir = state.paths.recording_dir(&id);
        std::fs::create_dir_all(&dir).map_err(err)?;
        let audio_path = if container {
            // Adobe Connect archives / FLV can't play in a web view: play the decoded mix.
            state.paths.pcm_path(&id)
        } else if copy {
            let ext = src
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .unwrap_or_else(|| "bin".into());
            let dst = dir.join(format!("source.{ext}"));
            let (s, d) = (src.clone(), dst.clone());
            tokio::task::spawn_blocking(move || std::fs::copy(&s, &d))
                .await
                .map_err(err)?
                .map_err(|e| format!("copying {p}: {e}"))?;
            dst
        } else {
            src.clone()
        };
        let title = src
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Recording".into());
        let now = now_iso();
        let rec = Recording {
            id: id.clone(),
            title,
            source_path: p.clone(),
            audio_path: audio_path.to_string_lossy().into_owned(),
            playback_wav_path: None,
            duration_sec: None,
            created_at: now.clone(),
            updated_at: now,
            status: RecordingStatus::New,
            model_id: None,
            error: None,
            segment_count: 0,
            has_analysis: false,
            tags: vec![],
            blurb: None,
        };
        state
            .db
            .lock()
            .unwrap()
            .insert_recording(&rec)
            .map_err(anyhow_err)?;
        allow_asset(&app, &rec.audio_path);
        allow_asset(&app, &rec.source_path);
        created.push(rec);
    }
    // Decode in the background (waveform + duration), then queue transcription.
    for rec in &created {
        prepare_and_queue(&app, rec.id.clone(), "file");
    }
    Ok(created)
}

/// Inputs decoded into the PCM cache rather than played directly: Adobe Connect source
/// archives (ZIP or extracted folder) and FLV streams.
fn is_container(p: &std::path::Path) -> bool {
    if p.is_dir() {
        return wwta_core::adobe_connect::is_connect_dir(p);
    }
    matches!(
        p.extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .as_deref(),
        Some("zip" | "flv" | "f4a")
    )
}

/// Decode a freshly imported recording (waveform, duration) and queue its transcription.
/// Record an import with coarse facts about the media (never its name or content).
fn track_import(app: &AppHandle, id: &str, source: &str, decoded_in_webview: bool) {
    let state = app.state::<AppState>();
    let Ok(rec) = state.db.lock().unwrap().recording(id) else {
        return;
    };
    let src = std::path::Path::new(&rec.source_path);
    let size = if src.is_file() {
        std::fs::metadata(src).map(|m| m.len()).unwrap_or(0)
    } else {
        0
    };
    let secs = rec.duration_sec.unwrap_or(0.0);
    state.analytics.capture(
        "recording imported",
        serde_json::json!({
            "source": source,
            "format": crate::telemetry::format_of(src),
            "duration_sec": secs.round(),
            "duration_bucket": crate::telemetry::duration_bucket(secs),
            "size_bucket": crate::telemetry::size_bucket_mb(size),
            "decoded_in_webview": decoded_in_webview,
        }),
    );
}

fn prepare_and_queue(app: &AppHandle, id: String, source: &'static str) {
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        let app3 = app2.clone();
        let id2 = id.clone();
        let outcome = tauri::async_runtime::spawn_blocking(move || {
            let state = app3.state::<AppState>();
            let _permit = state.decode_slots.acquire();
            worker::prepare_audio(&app3, &id2, &AtomicBool::new(false))
        })
        .await;
        let state = app2.state::<AppState>();
        match outcome {
            Ok(Ok(worker::Prepared::Samples(_))) => {
                track_import(&app2, &id, source, false);
                if let Some(model_id) = default_model_ready(&state) {
                    let _ = queue_transcription(&app2, &id, model_id);
                } else {
                    let db = state.db.lock().unwrap();
                    let _ = db.set_status(&id, RecordingStatus::New, None);
                    events::recording_updated(&app2, &db, &id);
                }
            }
            Ok(Ok(worker::Prepared::NeedsDecode)) => {
                let format = state
                    .db
                    .lock()
                    .unwrap()
                    .recording(&id)
                    .map(|r| crate::telemetry::format_of(std::path::Path::new(&r.source_path)))
                    .unwrap_or_default();
                state.analytics.capture(
                    "recording needs webview decode",
                    serde_json::json!({ "format": format, "source": source }),
                );
            }
            Ok(Err(e)) => {
                let msg = format!("{e:#}");
                state.analytics.capture(
                    "recording import failed",
                    serde_json::json!({ "source": source, "error_kind": crate::telemetry::error_kind(&msg) }),
                );
                crate::telemetry::report_error("import", &msg, &[("source", source)]);
                let db = state.db.lock().unwrap();
                let _ = db.set_status(&id, RecordingStatus::Error, Some(&msg));
                events::recording_updated(&app2, &db, &id);
                worker::progress(
                    &app2,
                    &id,
                    TranscriptionStage::Error,
                    0.0,
                    Some(msg),
                    vec![],
                );
            }
            Err(e) => log::error!("decode task failed: {e}"),
        }
    });
}

/// Import an Adobe Connect recording from its link (downloads the source archive).
#[tauri::command]
pub async fn import_link(app: AppHandle, url: String) -> CmdResult<Recording> {
    use wwta_core::adobe_connect;
    let link = adobe_connect::parse_link(&url).map_err(anyhow_err)?;
    let state = app.state::<AppState>();
    let id = uuid::Uuid::new_v4().to_string();
    let dir = state.paths.recording_dir(&id);
    std::fs::create_dir_all(&dir).map_err(err)?;
    let zip = dir.join("source.zip");
    let now = now_iso();
    let rec = Recording {
        id: id.clone(),
        title: link.id.clone(),
        source_path: zip.to_string_lossy().into_owned(),
        audio_path: state.paths.pcm_path(&id).to_string_lossy().into_owned(),
        playback_wav_path: None,
        duration_sec: None,
        created_at: now.clone(),
        updated_at: now,
        status: RecordingStatus::Transcribing,
        model_id: None,
        error: None,
        segment_count: 0,
        has_analysis: false,
        tags: vec![],
        blurb: None,
    };
    state
        .db
        .lock()
        .unwrap()
        .insert_recording(&rec)
        .map_err(anyhow_err)?;
    let app2 = app.clone();
    let id2 = id.clone();
    tauri::async_runtime::spawn(async move {
        let msg = "Downloading from Adobe Connect…".to_string();
        worker::progress(
            &app2,
            &id2,
            TranscriptionStage::Decoding,
            0.0,
            Some(msg),
            vec![],
        );
        let mut last = std::time::Instant::now();
        let (app3, id3) = (app2.clone(), id2.clone());
        let result =
            adobe_connect::download(&link, &zip, &AtomicBool::new(false), &mut |done, total| {
                if last.elapsed() < std::time::Duration::from_millis(250) {
                    return;
                }
                last = std::time::Instant::now();
                let p = if total > 0 {
                    done as f32 / total as f32
                } else {
                    0.0
                };
                let msg = format!(
                    "Downloading from Adobe Connect… {:.1} MB",
                    done as f64 / 1e6
                );
                worker::progress(
                    &app3,
                    &id3,
                    TranscriptionStage::Decoding,
                    p,
                    Some(msg),
                    vec![],
                );
            })
            .await;
        match result {
            Ok(()) => prepare_and_queue(&app2, id2, "adobe_connect_link"),
            Err(e) => {
                let msg = format!("{e:#}");
                let state = app2.state::<AppState>();
                state.analytics.capture(
                    "recording import failed",
                    serde_json::json!({
                        "source": "adobe_connect_link",
                        "error_kind": crate::telemetry::error_kind(&msg),
                        "login_page": msg.contains("login"),
                    }),
                );
                let db = state.db.lock().unwrap();
                let _ = db.set_status(&id2, RecordingStatus::Error, Some(&msg));
                events::recording_updated(&app2, &db, &id2);
                worker::progress(
                    &app2,
                    &id2,
                    TranscriptionStage::Error,
                    0.0,
                    Some(msg),
                    vec![],
                );
            }
        }
    });
    Ok(rec)
}

fn queue_transcription(app: &AppHandle, id: &str, model_id: String) -> anyhow::Result<()> {
    let state = app.state::<AppState>();
    // A fix still running would write corrections meant for the old transcript.
    if let Some(c) = state
        .llm_jobs
        .lock()
        .unwrap()
        .get(&crate::ai::fix_job_id(id))
    {
        c.store(true, Ordering::Relaxed);
    }
    {
        let db = state.db.lock().unwrap();
        db.set_status(id, RecordingStatus::Queued, None)?;
        events::recording_updated(app, &db, id);
    }
    worker::progress(app, id, TranscriptionStage::Queued, 0.0, None, vec![]);
    state.worker.enqueue(Job {
        recording_id: id.to_string(),
        model_id,
    })
}

#[tauri::command]
pub fn submit_decoded_audio(
    app: AppHandle,
    request: tauri::ipc::Request<'_>,
) -> CmdResult<Recording> {
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err("expected raw PCM body".into());
    };
    let header = |k: &str| {
        request
            .headers()
            .get(k)
            .and_then(|v| v.to_str().ok())
            .map(String::from)
    };
    let id = header("x-recording-id").ok_or("missing x-recording-id")?;
    let rate: u32 = header("x-sample-rate")
        .and_then(|r| r.parse().ok())
        .ok_or("missing x-sample-rate")?;
    let samples: Vec<f32> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect();
    let samples = if rate == audio::TARGET_RATE {
        samples
    } else {
        audio::resample(&samples, rate, audio::TARGET_RATE)
    };
    if samples.len() < (audio::TARGET_RATE / 10) as usize {
        return Err("decoded audio is empty".into());
    }
    worker::store_pcm(&app, &id, &samples).map_err(anyhow_err)?;
    let state = app.state::<AppState>();
    let pcm = state.paths.pcm_path(&id).to_string_lossy().into_owned();
    {
        let db = state.db.lock().unwrap();
        db.set_playback_wav(&id, &pcm).map_err(anyhow_err)?;
        db.set_status(&id, RecordingStatus::New, None)
            .map_err(anyhow_err)?;
    }
    allow_asset(&app, &pcm);
    track_import(&app, &id, "file", true);
    if let Some(model_id) = default_model_ready(&state) {
        queue_transcription(&app, &id, model_id).map_err(anyhow_err)?;
    }
    let rec = state.db.lock().unwrap().recording(&id);
    rec.map_err(anyhow_err)
}

#[tauri::command]
pub fn rename_recording(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    title: String,
) -> CmdResult<Recording> {
    let title = title.trim();
    if title.is_empty() {
        return Err("title can't be empty".into());
    }
    let db = state.db.lock().unwrap();
    db.set_title(&id, title).map_err(anyhow_err)?;
    events::recording_updated(&app, &db, &id);
    db.recording(&id).map_err(anyhow_err)
}

#[tauri::command]
pub fn delete_recording(app: AppHandle, state: State<'_, AppState>, id: String) -> CmdResult<()> {
    state.worker.cancel(&id);
    let attachments = {
        let db = state.db.lock().unwrap();
        let paths = db.recording_attachment_paths(&id).unwrap_or_default();
        db.delete_recording(&id).map_err(anyhow_err)?;
        paths
    };
    crate::attachments::remove_files(&attachments);
    let dir = state.paths.recording_dir(&id);
    if dir.starts_with(&state.paths.library) && dir.exists() {
        let _ = std::fs::remove_dir_all(dir);
    }
    let _ = app.emit(events::RECORDING_DELETED, id);
    state
        .analytics
        .capture("recording deleted", serde_json::json!({}));
    Ok(())
}

#[tauri::command]
pub fn transcribe(app: AppHandle, id: String, model_id: Option<String>) -> CmdResult<()> {
    let state = app.state::<AppState>();
    if state.worker.is_active(&id) {
        return Err("this recording is already being transcribed".into());
    }
    let model_id = model_id.unwrap_or_else(|| state.settings().default_model_id);
    let spec = models::spec(&model_id).ok_or_else(|| format!("unknown model {model_id}"))?;
    if models::read_manifest(&state.paths.models, spec.id).is_none() {
        return Err(format!(
            "{} is not installed yet. Install it from the Models page.",
            spec.name
        ));
    }
    queue_transcription(&app, &id, model_id).map_err(anyhow_err)
}

#[tauri::command]
pub fn cancel_transcription(app: AppHandle, state: State<'_, AppState>, id: String) {
    let was_queued = state.worker.queue.lock().unwrap().contains(&id);
    if state.worker.cancel(&id) && was_queued {
        let db = state.db.lock().unwrap();
        let ready = db
            .recording(&id)
            .map(|r| r.segment_count > 0)
            .unwrap_or(false);
        let _ = db.set_status(
            &id,
            if ready {
                RecordingStatus::Ready
            } else {
                RecordingStatus::New
            },
            None,
        );
        events::recording_updated(&app, &db, &id);
        worker::progress(&app, &id, TranscriptionStage::Cancelled, 0.0, None, vec![]);
    }
}

#[tauri::command]
pub fn update_segment_text(
    state: State<'_, AppState>,
    id: String,
    segment_id: u32,
    text: String,
) -> CmdResult<Segment> {
    state
        .db
        .lock()
        .unwrap()
        .update_segment_text(&id, segment_id, text.trim())
        .map_err(anyhow_err)
}

/// Correct recognition errors with the OpenRouter model. Progress (and the changed
/// segments) arrive on `transcript://fix-progress`; `cancel_llm("fix:<id>")` stops it.
#[tauri::command]
pub async fn fix_transcript(app: AppHandle, id: String) -> CmdResult<FixSummary> {
    crate::ai::fix_transcript(&app, &id)
        .await
        .map_err(anyhow_err)
}

/// Put back the recognizer's text of AI-fixed segments (`segment_ids` null = all).
#[tauri::command]
pub fn revert_transcript_fixes(
    state: State<'_, AppState>,
    id: String,
    segment_ids: Option<Vec<u32>>,
) -> CmdResult<Vec<Segment>> {
    let reverted = state
        .db
        .lock()
        .unwrap()
        .revert_fixes(&id, segment_ids.as_deref())
        .map_err(anyhow_err)?;
    state.analytics.capture(
        "transcript fixes reverted",
        serde_json::json!({
            "scope": if segment_ids.is_some() { "segments" } else { "all" },
            "segments": reverted.len(),
        }),
    );
    Ok(reverted)
}

#[tauri::command]
pub fn get_waveform(state: State<'_, AppState>, id: String) -> CmdResult<Waveform> {
    let peaks = audio::read_peaks(&state.paths.peaks_path(&id))
        .map_err(|_| "waveform not ready yet".to_string())?;
    let duration = state
        .db
        .lock()
        .unwrap()
        .recording(&id)
        .map_err(anyhow_err)?
        .duration_sec;
    Ok(Waveform {
        duration_sec: duration.unwrap_or(peaks.len() as f64 / audio::PEAKS_PER_SECOND as f64),
        peaks,
        peaks_per_second: audio::PEAKS_PER_SECOND,
    })
}

#[tauri::command]
pub async fn prepare_playback_wav(app: AppHandle, id: String) -> CmdResult<String> {
    // The 16 kHz mono PCM cache made for transcription plays in every webview; reuse it
    // instead of decoding the file a second time.
    let state = app.state::<AppState>();
    let pcm = state.paths.pcm_path(&id);
    if !pcm.is_file() {
        let (app2, id2) = (app.clone(), id.clone());
        let prepared = tokio::task::spawn_blocking(move || {
            worker::prepare_audio(&app2, &id2, &AtomicBool::new(false))
        })
        .await
        .map_err(err)?
        .map_err(anyhow_err)?;
        if let worker::Prepared::NeedsDecode = prepared {
            return Err("this file has to be decoded before it can play".into());
        }
    }
    let p = pcm.to_string_lossy().into_owned();
    state
        .db
        .lock()
        .unwrap()
        .set_playback_wav(&id, &p)
        .map_err(anyhow_err)?;
    allow_asset(&app, &p);
    Ok(p)
}

#[tauri::command]
pub fn export_transcript(
    state: State<'_, AppState>,
    id: String,
    format: String,
    path: String,
) -> CmdResult<()> {
    let db = state.db.lock().unwrap();
    let rec = db.recording(&id).map_err(anyhow_err)?;
    let segs = db.segments(&id).map_err(anyhow_err)?;
    let analysis = db.analysis(&id).map_err(anyhow_err)?;
    let notes = db.notes(&id).map_err(anyhow_err)?;
    let body = export::render(&format, &rec.title, &segs, analysis.as_ref(), &notes)
        .ok_or("unknown export format")?;
    state.analytics.capture(
        "transcript exported",
        serde_json::json!({ "format": format, "segments": segs.len(), "with_notes": !notes.is_empty() }),
    );
    std::fs::write(&path, body).map_err(err)
}

// ------------------------------------------------------------------ notes

#[tauri::command]
pub fn add_note(
    state: State<'_, AppState>,
    recording_id: String,
    start: f64,
    end: f64,
    body: String,
    color: Option<String>,
) -> CmdResult<Note> {
    if body.trim().is_empty() {
        return Err("a note can't be empty".into());
    }
    let db = state.db.lock().unwrap();
    db.recording(&recording_id).map_err(anyhow_err)?;
    let note = db
        .add_note(&recording_id, start, end, &body, color.as_deref())
        .map_err(anyhow_err)?;
    drop(db);
    state.analytics.capture(
        "note created",
        serde_json::json!({
            "chars": note.body.chars().count(),
            "has_link": note.body.contains("http") || note.body.contains("[rec:"),
            "color": note.color,
            "span_sec": (note.end - note.start).round(),
        }),
    );
    Ok(note)
}

#[tauri::command]
pub fn update_note(
    state: State<'_, AppState>,
    id: String,
    start: f64,
    end: f64,
    body: String,
    color: Option<String>,
) -> CmdResult<Note> {
    if body.trim().is_empty() {
        return Err("a note can't be empty".into());
    }
    state
        .db
        .lock()
        .unwrap()
        .update_note(&id, start, end, &body, color.as_deref())
        .map_err(anyhow_err)
}

#[tauri::command]
pub fn delete_note(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    state
        .db
        .lock()
        .unwrap()
        .delete_note(&id)
        .map_err(anyhow_err)?;
    state
        .analytics
        .capture("note deleted", serde_json::json!({}));
    Ok(())
}

#[tauri::command]
pub fn list_all_notes(state: State<'_, AppState>) -> CmdResult<Vec<NoteWithRecording>> {
    state.db.lock().unwrap().all_notes().map_err(anyhow_err)
}

// ------------------------------------------------------------------ search

#[tauri::command]
pub async fn search(
    app: AppHandle,
    query: String,
    recording_id: Option<String>,
    limit: Option<usize>,
) -> CmdResult<Vec<SearchHit>> {
    let limit = limit.unwrap_or(50).clamp(1, 500);
    let state = app.state::<AppState>();
    let (lexical, tags, notes) = {
        let db = state.db.lock().unwrap();
        (
            search::lexical(&db, &query, recording_id.as_deref(), limit).map_err(anyhow_err)?,
            search::tag_hits(&db, &query, recording_id.as_deref()).map_err(anyhow_err)?,
            search::note_hits(&db, &query, recording_id.as_deref()).map_err(anyhow_err)?,
        )
    };
    let semantic = if query.trim().chars().count() >= 3 {
        crate::ai::semantic_search(&app, &query, recording_id.as_deref(), limit.min(30)).await
    } else {
        vec![]
    };
    let semantic_n = semantic.len();
    let hits = search::merge_hits(vec![notes, lexical, tags, semantic], limit);
    let count = |k: HitKind| hits.iter().filter(|h| h.kind == k).count();
    state.analytics.capture(
        "search performed",
        serde_json::json!({
            "scope": if recording_id.is_some() { "recording" } else { "library" },
            "query_chars": query.trim().chars().count(),
            "results": hits.len(),
            "lexical": count(HitKind::Lexical),
            "semantic": count(HitKind::Semantic),
            "tag": count(HitKind::Tag),
            "note": count(HitKind::Note),
            "semantic_available": semantic_n > 0,
        }),
    );
    Ok(hits)
}

#[tauri::command]
pub fn related_segments(
    state: State<'_, AppState>,
    id: String,
    segment_id: u32,
    limit: Option<usize>,
) -> CmdResult<Vec<SearchHit>> {
    let limit = limit.unwrap_or(10).clamp(1, 100);
    let model = state.settings().embedding_model;
    let db = state.db.lock().unwrap();
    let rows = db.embeddings(&model, None).map_err(anyhow_err)?;
    if let Some(own) = rows
        .iter()
        .find(|r| r.recording_id == id && r.start_seg <= segment_id && r.end_seg >= segment_id)
    {
        let hits =
            search::semantic_hits(&db, &rows, &own.vector, Some((&id, segment_id)), limit, 0.5)
                .map_err(anyhow_err)?;
        if !hits.is_empty() {
            return Ok(hits);
        }
    }
    search::lexical_related(&db, &id, segment_id, limit).map_err(anyhow_err)
}

#[tauri::command]
pub fn list_tags(state: State<'_, AppState>) -> CmdResult<Vec<TagSummary>> {
    search::list_tags(&state.db.lock().unwrap()).map_err(anyhow_err)
}

#[tauri::command]
pub fn segments_for_tag(state: State<'_, AppState>, tag: String) -> CmdResult<Vec<SearchHit>> {
    search::segments_for_tag(&state.db.lock().unwrap(), &tag).map_err(anyhow_err)
}

// ------------------------------------------------------------------ AI

#[tauri::command]
pub async fn list_llm_models(state: State<'_, AppState>) -> CmdResult<Vec<LlmModelInfo>> {
    let mut cache = state.llm_models.lock().await;
    if let Some(m) = cache.as_ref() {
        return Ok(m.clone());
    }
    let models = wwta_core::llm::list_models()
        .await
        .map_err(|e| format!("couldn't load the model list (offline?): {e:#}"))?;
    *cache = Some(models.clone());
    Ok(models)
}

#[tauri::command]
pub async fn test_api_key(api_key: String) -> CmdResult<String> {
    wwta_core::llm::Client::new(&api_key)
        .map_err(anyhow_err)?
        .key_info()
        .await
        .map_err(anyhow_err)
}

#[tauri::command]
pub async fn analyze(
    app: AppHandle,
    id: String,
    parts: Option<Vec<String>>,
) -> CmdResult<Analysis> {
    let _ = parts; // the full analysis is cheap enough to always produce together
    crate::ai::analyze(&app, &id).await.map_err(anyhow_err)
}

#[tauri::command]
pub async fn index_embeddings(app: AppHandle, id: String) -> CmdResult<()> {
    crate::ai::index_embeddings(&app, &id)
        .await
        .map_err(anyhow_err)
}

#[tauri::command]
pub async fn ask(
    app: AppHandle,
    recording_id: Option<String>,
    thread_id: Option<String>,
    question: String,
    attachment_ids: Option<Vec<String>>,
    request_id: String,
) -> CmdResult<AskResult> {
    crate::ai::ask(
        &app,
        recording_id,
        thread_id,
        question,
        attachment_ids.unwrap_or_default(),
        request_id,
    )
    .await
    .map_err(anyhow_err)
}

#[tauri::command]
pub fn list_chat_threads(
    state: State<'_, AppState>,
    recording_id: Option<String>,
) -> CmdResult<Vec<ChatThread>> {
    state
        .db
        .lock()
        .unwrap()
        .threads(recording_id.as_deref())
        .map_err(anyhow_err)
}

#[tauri::command]
pub fn list_chat_messages(
    state: State<'_, AppState>,
    thread_id: String,
) -> CmdResult<Vec<ChatMessage>> {
    state
        .db
        .lock()
        .unwrap()
        .thread_messages(&thread_id)
        .map_err(anyhow_err)
}

#[tauri::command]
pub fn rename_chat_thread(
    state: State<'_, AppState>,
    thread_id: String,
    title: String,
) -> CmdResult<ChatThread> {
    state
        .db
        .lock()
        .unwrap()
        .rename_thread(&thread_id, &title)
        .map_err(anyhow_err)
}

#[tauri::command]
pub fn delete_chat_thread(state: State<'_, AppState>, thread_id: String) -> CmdResult<()> {
    let files = state
        .db
        .lock()
        .unwrap()
        .delete_thread(&thread_id)
        .map_err(anyhow_err)?;
    crate::attachments::remove_files(&files);
    Ok(())
}

/// Attach a file from disk (picked or dropped) to the message being written.
#[tauri::command]
pub async fn add_chat_attachment_file(app: AppHandle, path: String) -> CmdResult<AttachmentUpload> {
    crate::attachments::from_path(&app, &path)
        .await
        .map_err(anyhow_err)
}

/// Attach raw bytes: an image (`x-kind: image`), or a voice clip as little-endian f32 mono
/// samples at `x-sample-rate` (`x-kind: audio`). `x-name` is the URI-encoded display name.
#[tauri::command]
pub async fn add_chat_attachment(
    app: AppHandle,
    request: tauri::ipc::Request<'_>,
) -> CmdResult<ChatAttachment> {
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err("expected a raw body".into());
    };
    let header = |k: &str| {
        request
            .headers()
            .get(k)
            .and_then(|v| v.to_str().ok())
            .map(String::from)
    };
    let name = crate::attachments::percent_decode(&header("x-name").unwrap_or_default());
    let kind = header("x-kind");
    let rate: Option<u32> = header("x-sample-rate").and_then(|r| r.parse().ok());
    let bytes = bytes.clone();
    // converting and writing long clips takes a moment; keep it off the main thread
    tokio::task::spawn_blocking(move || match kind.as_deref() {
        Some("image") => crate::attachments::save_image(&app, &name, &bytes).map_err(anyhow_err),
        Some("audio") => {
            let rate = rate.ok_or("missing x-sample-rate")?;
            let samples: Vec<f32> = bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| f32::from_le_bytes(*c))
                .collect();
            let samples = if rate == audio::TARGET_RATE {
                samples
            } else {
                audio::resample(&samples, rate, audio::TARGET_RATE)
            };
            crate::attachments::save_audio(&app, &name, &samples).map_err(anyhow_err)
        }
        _ => Err("x-kind must be image or audio".into()),
    })
    .await
    .map_err(err)?
}

/// Remove an attachment the user took off the message before sending it.
#[tauri::command]
pub fn discard_chat_attachment(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let path = state
        .db
        .lock()
        .unwrap()
        .discard_attachment(&id)
        .map_err(anyhow_err)?;
    if let Some(p) = path {
        crate::attachments::remove_files(&[p]);
    }
    Ok(())
}

#[tauri::command]
pub async fn generate_doc(
    app: AppHandle,
    id: String,
    prompt: String,
    request_id: String,
) -> CmdResult<Doc> {
    crate::ai::generate_doc(&app, id, prompt, request_id)
        .await
        .map_err(anyhow_err)
}

#[tauri::command]
pub async fn revise_doc(
    app: AppHandle,
    doc_id: String,
    instruction: String,
    request_id: String,
) -> CmdResult<Doc> {
    crate::ai::revise_doc(&app, doc_id, instruction, request_id)
        .await
        .map_err(anyhow_err)
}

#[tauri::command]
pub fn delete_doc(state: State<'_, AppState>, doc_id: String) -> CmdResult<()> {
    state
        .db
        .lock()
        .unwrap()
        .delete_doc(&doc_id)
        .map_err(anyhow_err)
}

#[tauri::command]
pub fn cancel_llm(state: State<'_, AppState>, request_id: String) {
    if let Some(c) = state.llm_jobs.lock().unwrap().get(&request_id) {
        c.store(true, Ordering::Relaxed);
    }
}
