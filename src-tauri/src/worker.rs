//! Background transcription queue. One job runs at a time (ASR saturates the CPU/GPU);
//! the loaded engine is cached between jobs so large models load once.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use tauri::{AppHandle, Emitter, Manager};
use wwta_core::asr::{self, Engine, TranscribeOptions};
use wwta_core::audio::{self, DecodeError};
use wwta_core::models;
use wwta_core::types::{RecordingStatus, Segment, TranscriptionProgress, TranscriptionStage};

use crate::events;
use crate::state::AppState;

pub struct Job {
    pub recording_id: String,
    pub model_id: String,
}

pub struct Worker {
    tx: Mutex<Sender<Job>>,
    pub queue: Mutex<VecDeque<String>>,
    pub current: Mutex<Option<(String, Arc<AtomicBool>)>>,
}

impl Worker {
    pub fn new() -> (Self, Receiver<Job>) {
        let (tx, rx) = channel();
        (
            Self {
                tx: Mutex::new(tx),
                queue: Mutex::new(VecDeque::new()),
                current: Mutex::new(None),
            },
            rx,
        )
    }

    pub fn enqueue(&self, job: Job) -> Result<()> {
        let mut q = self.queue.lock().unwrap();
        if q.contains(&job.recording_id) {
            return Ok(());
        }
        q.push_back(job.recording_id.clone());
        self.tx
            .lock()
            .unwrap()
            .send(job)
            .map_err(|_| anyhow!("transcription worker stopped"))
    }

    /// Returns true if the job was queued or running and is now being cancelled.
    pub fn cancel(&self, id: &str) -> bool {
        let mut q = self.queue.lock().unwrap();
        if let Some(pos) = q.iter().position(|x| x == id) {
            q.remove(pos);
            return true;
        }
        drop(q);
        if let Some((cur, flag)) = self.current.lock().unwrap().as_ref() {
            if cur == id {
                flag.store(true, Ordering::Relaxed);
                return true;
            }
        }
        false
    }

    pub fn is_active(&self, id: &str) -> bool {
        self.queue.lock().unwrap().iter().any(|x| x == id)
            || self
                .current
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|(c, _)| c == id)
    }
}

pub fn progress(
    app: &AppHandle,
    id: &str,
    stage: TranscriptionStage,
    p: f32,
    message: Option<String>,
    new_segments: Vec<Segment>,
) {
    let _ = app.emit(
        events::TRANSCRIPTION_PROGRESS,
        TranscriptionProgress {
            recording_id: id.into(),
            stage,
            progress: p,
            message,
            new_segments,
        },
    );
}

pub enum Prepared {
    Samples(Vec<f32>),
    NeedsDecode,
}

/// Make sure the 16 kHz PCM cache, waveform peaks and duration exist for a recording.
pub fn prepare_audio(app: &AppHandle, id: &str, cancel: &AtomicBool) -> Result<Prepared> {
    let state = app.state::<AppState>();
    let pcm = state.paths.pcm_path(id);
    if pcm.is_file() {
        return Ok(Prepared::Samples(audio::read_wav_16k(&pcm)?));
    }
    let rec = state.db.lock().unwrap().recording(id)?;
    progress(app, id, TranscriptionStage::Decoding, 0.0, None, vec![]);
    let app2 = app.clone();
    let id2 = id.to_string();
    let decoded = audio::decode_to_16k_mono(Path::new(&rec.audio_path), cancel, &mut |p| {
        progress(&app2, &id2, TranscriptionStage::Decoding, p, None, vec![]);
    });
    let samples = match decoded {
        Ok(s) => s,
        Err(DecodeError::Unsupported(msg)) => {
            log::info!("native decoder can't read {}: {msg}", rec.audio_path);
            let db = state.db.lock().unwrap();
            db.set_status(id, RecordingStatus::NeedsDecode, None)?;
            events::recording_updated(app, &db, id);
            return Ok(Prepared::NeedsDecode);
        }
        Err(DecodeError::Cancelled) => return Err(anyhow!("cancelled")),
        Err(DecodeError::Other(e)) => return Err(e),
    };
    store_pcm(app, id, &samples)?;
    Ok(Prepared::Samples(samples))
}

pub fn store_pcm(app: &AppHandle, id: &str, samples: &[f32]) -> Result<()> {
    let state = app.state::<AppState>();
    std::fs::create_dir_all(state.paths.recording_dir(id))?;
    audio::write_wav_16k(&state.paths.pcm_path(id), samples)?;
    audio::write_peaks(
        &state.paths.peaks_path(id),
        &audio::peaks(samples, audio::PEAKS_PER_SECOND),
    )?;
    let db = state.db.lock().unwrap();
    db.set_duration(id, audio::duration_secs(samples.len()))?;
    events::recording_updated(app, &db, id);
    Ok(())
}

type CachedEngine = Option<(String, Box<dyn Engine>)>;

pub fn spawn(app: AppHandle, rx: Receiver<Job>) {
    std::thread::Builder::new()
        .name("transcriber".into())
        .spawn(move || {
            let mut cache: CachedEngine = None;
            while let Ok(job) = rx.recv() {
                let state = app.state::<AppState>();
                {
                    let mut q = state.worker.queue.lock().unwrap();
                    match q.iter().position(|x| x == &job.recording_id) {
                        Some(pos) => {
                            q.remove(pos);
                        }
                        None => continue, // cancelled while queued
                    }
                }
                let cancel = Arc::new(AtomicBool::new(false));
                *state.worker.current.lock().unwrap() =
                    Some((job.recording_id.clone(), cancel.clone()));
                let result = run_job(&app, &job, &cancel, &mut cache);
                *state.worker.current.lock().unwrap() = None;
                let id = &job.recording_id;
                match result {
                    Ok(()) => {}
                    Err(e) if cancel.load(Ordering::Relaxed) || e.to_string() == "cancelled" => {
                        let db = state.db.lock().unwrap();
                        let has_segments = db
                            .recording(id)
                            .map(|r| r.segment_count > 0)
                            .unwrap_or(false);
                        let status = if has_segments {
                            RecordingStatus::Ready
                        } else {
                            RecordingStatus::New
                        };
                        let _ = db.set_status(id, status, None);
                        events::recording_updated(&app, &db, id);
                        progress(&app, id, TranscriptionStage::Cancelled, 0.0, None, vec![]);
                    }
                    Err(e) => {
                        let msg = format!("{e:#}");
                        log::error!("transcription of {id} failed: {msg}");
                        let db = state.db.lock().unwrap();
                        let _ = db.set_status(id, RecordingStatus::Error, Some(&msg));
                        events::recording_updated(&app, &db, id);
                        progress(&app, id, TranscriptionStage::Error, 0.0, Some(msg), vec![]);
                    }
                }
            }
        })
        .expect("spawn transcriber thread");
}

fn run_job(
    app: &AppHandle,
    job: &Job,
    cancel: &Arc<AtomicBool>,
    cache: &mut CachedEngine,
) -> Result<()> {
    let state = app.state::<AppState>();
    let id = job.recording_id.as_str();
    {
        let db = state.db.lock().unwrap();
        db.set_status(id, RecordingStatus::Transcribing, None)?;
        events::recording_updated(app, &db, id);
    }
    let samples = match prepare_audio(app, id, cancel)? {
        Prepared::Samples(s) => s,
        Prepared::NeedsDecode => return Ok(()),
    };

    let spec =
        models::spec(&job.model_id).ok_or_else(|| anyhow!("unknown model {}", job.model_id))?;
    let model_file = models::model_file(&state.paths.models, spec.id).ok_or_else(|| {
        anyhow!(
            "{} is not installed. Install it from the Models page.",
            spec.name
        )
    })?;
    let settings = state.settings();
    let opts = TranscribeOptions {
        threads: settings.threads as i32,
        use_gpu: settings.use_gpu,
        persian_digits: settings.persian_digits,
        precise_word_timings: settings.precise_word_timings,
        language: "fa".into(),
    };
    let key = format!(
        "{}|{}|{}|{}|{}",
        spec.id,
        model_file.display(),
        opts.use_gpu,
        opts.precise_word_timings,
        opts.threads
    );
    if cache.as_ref().map(|(k, _)| k != &key).unwrap_or(true) {
        *cache = None; // free the previous model (RAM/VRAM) first
        progress(
            app,
            id,
            TranscriptionStage::LoadingModel,
            0.0,
            Some(format!("Loading {}…", spec.name)),
            vec![],
        );
        let engine = asr::load_engine(&spec, &model_file, &opts)?;
        log::info!("loaded {} on {}", spec.name, engine.device());
        *cache = Some((key, engine));
    }
    let engine = &mut cache.as_mut().unwrap().1;
    let device = engine.device();
    progress(
        app,
        id,
        TranscriptionStage::Transcribing,
        0.0,
        Some(format!("{} · {device}", spec.name)),
        vec![],
    );

    let max_region = if spec.engine == wwta_core::types::EngineKind::Whisper {
        25.0
    } else {
        20.0
    };
    let mut preview_id = 0u32;
    let app2 = app.clone();
    let t0 = std::time::Instant::now();
    let segments = asr::transcribe(
        engine.as_mut(),
        &samples,
        &state.paths.vad_model,
        max_region,
        &opts,
        cancel,
        &mut |p, mut new| {
            for s in &mut new {
                s.id = preview_id;
                preview_id += 1;
            }
            progress(&app2, id, TranscriptionStage::Transcribing, p, None, new);
        },
    )?;
    log::info!(
        "transcribed {:.0}s of audio in {:.1}s with {}",
        audio::duration_secs(samples.len()),
        t0.elapsed().as_secs_f32(),
        spec.name
    );
    progress(app, id, TranscriptionStage::Saving, 1.0, None, vec![]);
    {
        let mut db = state.db.lock().unwrap();
        db.replace_segments(id, spec.id, &segments)?;
        events::recording_updated(app, &db, id);
    }
    progress(
        app,
        id,
        TranscriptionStage::Done,
        1.0,
        if segments.is_empty() {
            Some("No speech was detected in this recording.".into())
        } else {
            None
        },
        vec![],
    );
    if settings.auto_analyze
        && !settings.openrouter_api_key.trim().is_empty()
        && !segments.is_empty()
    {
        let app3 = app.clone();
        let id3 = id.to_string();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = crate::ai::analyze(&app3, &id3).await {
                log::warn!("auto analysis failed: {e:#}");
            }
        });
    }
    Ok(())
}
