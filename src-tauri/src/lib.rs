//! What Was That About — desktop app (Tauri).

mod ai;
mod commands;
mod docproto;
mod events;
mod state;
mod sysinfo;
mod telemetry;
mod worker;

use std::sync::{Mutex, RwLock};

use tauri::Manager;
use wwta_core::db::Db;

use crate::state::{load_settings, AppState, Paths, Slots};

const IDENTIFIER: &str = "com.whatwasthatabout.desktop";

pub fn run() {
    // Crash reporting starts before anything else (and re-runs in its crash-reporter process).
    let (analytics_on, crash_reports_on) = telemetry::early_settings(IDENTIFIER);
    let sentry = telemetry::init_sentry(crash_reports_on);
    let (_sentry_guard, sentry_plugin) = match sentry {
        Some((g, p)) => (Some(g), Some(p)),
        None => (None, None),
    };
    let mut builder = tauri::Builder::default();
    if let Some(p) = sentry_plugin {
        builder = builder.plugin(p);
    }
    builder
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .level_for("whisper_rs", log::LevelFilter::Warn)
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .register_uri_scheme_protocol("wwta-doc", |ctx, req| {
            docproto::handle(ctx.app_handle(), req)
        })
        .setup(move |app| {
            let data = app.path().app_data_dir()?;
            let paths = Paths::new(data)?;
            let db = Db::open(&paths.data.join("library.sqlite"))?;
            db.reset_interrupted()?;
            let scope = app.asset_protocol_scope();
            for r in db.list_recordings()? {
                let _ = scope.allow_file(&r.audio_path);
                let _ = scope.allow_file(&r.source_path);
                if let Some(p) = &r.playback_wav_path {
                    let _ = scope.allow_file(p);
                }
            }
            let settings = load_settings(&paths.settings);
            let analytics =
                telemetry::Analytics::new(&paths.data, settings.analytics_enabled && analytics_on);
            analytics.spawn_flusher();
            let (worker, rx) = worker::Worker::new();
            log::info!("data dir: {}", paths.data.display());
            app.manage(AppState {
                paths,
                db: Mutex::new(db),
                settings: RwLock::new(settings),
                worker,
                decode_slots: Slots::new(2),
                model_jobs: Mutex::default(),
                model_status: Mutex::default(),
                llm_jobs: Mutex::default(),
                llm_models: tokio::sync::Mutex::new(None),
                analytics,
            });
            telemetry_app_opened(app.handle().clone());
            worker::spawn(app.handle().clone(), rx);
            // Debug builds only: evaluate JS lines written to the FIFO named by
            // WWTA_E2E_EVAL_FIFO in the main webview (headless end-to-end runs).
            #[cfg(debug_assertions)]
            if let Ok(fifo) = std::env::var("WWTA_E2E_EVAL_FIFO") {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    while let Ok(text) = std::fs::read_to_string(&fifo) {
                        if let Some(w) = handle.get_webview_window("main") {
                            let _ = w.eval(text);
                        }
                    }
                });
            }
            // Debug builds only: import files listed in WWTA_E2E_IMPORT on startup (used for
            // headless end-to-end runs).
            #[cfg(debug_assertions)]
            if let Ok(list) = std::env::var("WWTA_E2E_IMPORT") {
                let paths: Vec<String> = std::env::split_paths(&list)
                    .map(|p| p.to_string_lossy().into_owned())
                    .filter(|p| !p.is_empty())
                    .collect();
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    if paths.is_empty() {
                        return;
                    }
                    if let Err(e) = commands::import_files(handle, paths).await {
                        log::error!("e2e import failed: {e}");
                    }
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_system_info,
            commands::get_settings,
            commands::save_settings,
            commands::track_ui,
            commands::list_models,
            commands::install_model,
            commands::cancel_model_install,
            commands::import_model_files,
            commands::delete_model,
            commands::list_recordings,
            commands::get_recording,
            commands::import_files,
            commands::import_link,
            commands::submit_decoded_audio,
            commands::rename_recording,
            commands::delete_recording,
            commands::transcribe,
            commands::cancel_transcription,
            commands::update_segment_text,
            commands::fix_transcript,
            commands::revert_transcript_fixes,
            commands::get_waveform,
            commands::prepare_playback_wav,
            commands::export_transcript,
            commands::add_note,
            commands::update_note,
            commands::delete_note,
            commands::list_all_notes,
            commands::search,
            commands::related_segments,
            commands::list_tags,
            commands::segments_for_tag,
            commands::list_llm_models,
            commands::test_api_key,
            commands::analyze,
            commands::index_embeddings,
            commands::ask,
            commands::clear_chat,
            commands::list_library_chat,
            commands::generate_doc,
            commands::revise_doc,
            commands::delete_doc,
            commands::cancel_llm,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the application");
}

/// Detect the GPU off the main thread, then record the launch with coarse machine facts.
fn telemetry_app_opened(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        let devices = sysinfo::gpu_devices_cached();
        let a = &state.analytics;
        a.set_base(
            "gpu_backend",
            serde_json::json!(wwta_core::asr::whisper::GPU_BACKEND),
        );
        a.set_base("gpu_available", serde_json::json!(!devices.is_empty()));
        if let Some(g) = devices.first() {
            a.set_base("gpu", serde_json::json!(g.name));
            a.set_base(
                "gpu_discrete",
                serde_json::json!(g.description.contains("discrete")),
            );
        }
        let installed: Vec<String> = wwta_core::models::catalog()
            .iter()
            .filter(|m| wwta_core::models::read_manifest(&state.paths.models, m.id).is_some())
            .map(|m| m.id.to_string())
            .collect();
        let (recordings, notes) = {
            let db = state.db.lock().unwrap();
            (
                db.list_recordings().map(|r| r.len()).unwrap_or(0),
                db.all_notes().map(|n| n.len()).unwrap_or(0),
            )
        };
        let s = state.settings();
        a.capture(
            "app opened",
            serde_json::json!({
                "models_installed": installed,
                "recordings": recordings,
                "notes": notes,
                "cpu_cores": std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
                "ram_gb": (sysinfo::total_memory() as f64 / 1e9).round(),
                "default_model": s.default_model_id,
                "llm_model": if s.openrouter_api_key.trim().is_empty() { None } else { Some(s.llm_model) },
                "ai_enabled": !s.openrouter_api_key.trim().is_empty(),
                "ai_language": s.ai_language,
                "theme": s.theme,
            }),
        );
    });
}
