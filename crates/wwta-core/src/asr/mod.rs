//! Speech recognition pipeline: Silero VAD -> engine (Shenava via sherpa-onnx, Whisper via
//! whisper.cpp) -> post-processing (Persian digits, sentence splitting, ids).

pub mod shenava;
pub mod vad;
pub mod whisper;

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use anyhow::{bail, Result};

use crate::models::{ModelKind, ModelSpec};
use crate::types::Segment;

#[derive(Debug, Clone)]
pub struct TranscribeOptions {
    /// 0 = automatic
    pub threads: i32,
    pub use_gpu: bool,
    pub persian_digits: bool,
    pub precise_word_timings: bool,
    pub language: String,
}

impl Default for TranscribeOptions {
    fn default() -> Self {
        Self {
            threads: 0,
            use_gpu: true,
            persian_digits: true,
            precise_word_timings: true,
            language: "fa".into(),
        }
    }
}

/// Silero VAD model bundled into the binary (643 KB, MIT).
pub const VAD_MODEL: &[u8] = include_bytes!("../../../../assets/vad/silero_vad.onnx");

/// Write the bundled VAD model into `dir` (if missing) and return its path.
pub fn ensure_vad_model(dir: &Path) -> Result<std::path::PathBuf> {
    std::fs::create_dir_all(dir)?;
    let p = dir.join("silero_vad.onnx");
    if std::fs::metadata(&p).map(|m| m.len() as usize).ok() != Some(VAD_MODEL.len()) {
        std::fs::write(&p, VAD_MODEL)?;
    }
    Ok(p)
}

pub fn effective_threads(requested: i32) -> i32 {
    if requested > 0 {
        return requested;
    }
    let n = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4) as i32;
    // leave a core for the UI
    (n - 1).clamp(1, 16)
}

/// Progress callback: fraction done (0..1) and newly finalized segments (ids not final).
pub type Progress<'a> = dyn FnMut(f32, Vec<Segment>) + 'a;

pub trait Engine: Send {
    /// Transcribe `samples` (16 kHz mono) using the given speech regions (sample ranges).
    fn transcribe(
        &mut self,
        samples: &[f32],
        regions: &[(usize, usize)],
        opts: &TranscribeOptions,
        cancel: &Arc<AtomicBool>,
        progress: &mut Progress<'_>,
    ) -> Result<Vec<Segment>>;

    /// Human readable description of where it runs (e.g. "Vulkan: NVIDIA RTX 3070").
    fn device(&self) -> String;
}

pub fn load_engine(
    spec: &ModelSpec,
    model_file: &Path,
    opts: &TranscribeOptions,
) -> Result<Box<dyn Engine>> {
    match spec.kind {
        ModelKind::SherpaNemoCtc => {
            let tokens = model_file.with_file_name("tokens.txt");
            Ok(Box::new(shenava::Shenava::load(
                model_file,
                &tokens,
                effective_threads(opts.threads),
            )?))
        }
        ModelKind::WhisperHf => Ok(Box::new(whisper::Whisper::load(
            model_file,
            opts.use_gpu,
            if opts.precise_word_timings {
                spec.alignment_heads
            } else {
                &[]
            },
        )?)),
    }
}

/// Run the full pipeline on 16 kHz mono audio.
pub fn transcribe(
    engine: &mut dyn Engine,
    samples: &[f32],
    vad_model: &Path,
    max_region_secs: f32,
    opts: &TranscribeOptions,
    cancel: &Arc<AtomicBool>,
    progress: &mut Progress<'_>,
) -> Result<Vec<Segment>> {
    crate::audio::ensure_audible(samples)?;
    let regions = vad::speech_regions(samples, vad_model, max_region_secs, cancel)?;
    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        bail!("cancelled");
    }
    if regions.is_empty() {
        return Ok(vec![]);
    }
    let digits = opts.persian_digits;
    let post = |mut segs: Vec<Segment>| -> Vec<Segment> {
        if digits {
            for s in &mut segs {
                crate::text::itn_segment(s);
            }
        }
        segs.retain(|s| !s.text.trim().is_empty());
        segs
    };
    let mut wrapped = |p: f32, segs: Vec<Segment>| {
        let segs = post(segs);
        progress(p, segs);
    };
    let segs = engine.transcribe(samples, &regions, opts, cancel, &mut wrapped)?;
    let mut segs = post(segs);
    segs.sort_by(|a, b| a.start.total_cmp(&b.start));
    Ok(crate::text::split_segments(segs, 9.0))
}
