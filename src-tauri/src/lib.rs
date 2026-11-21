//! What Was That About — desktop app (Tauri).

mod ai;
mod commands;
mod docproto;
mod events;
mod state;
mod sysinfo;
mod worker;

use std::sync::{Mutex, RwLock};

use tauri::Manager;
use wwta_core::db::Db;

use crate::state::{load_settings, AppState, Paths, Slots};

pub fn run() {
    tauri::Builder::default()
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
        .setup(|app| {
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
            });
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
            commands::get_waveform,
            commands::prepare_playback_wav,
            commands::export_transcript,
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
