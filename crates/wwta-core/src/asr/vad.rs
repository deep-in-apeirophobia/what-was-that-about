//! Silero VAD (via sherpa-onnx) to find speech regions.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, bail, Result};
use sherpa_onnx::{SileroVadModelConfig, VadModelConfig, VoiceActivityDetector};

use crate::audio::TARGET_RATE;

/// Returns sample ranges containing speech, each at most `max_speech_secs` long.
pub fn speech_regions(
    samples: &[f32],
    model: &Path,
    max_speech_secs: f32,
    cancel: &Arc<AtomicBool>,
) -> Result<Vec<(usize, usize)>> {
    let cfg = VadModelConfig {
        silero_vad: SileroVadModelConfig {
            model: Some(model.to_string_lossy().into_owned()),
            threshold: 0.4,
            min_silence_duration: 0.35,
            min_speech_duration: 0.2,
            window_size: 512,
            max_speech_duration: max_speech_secs,
        },
        sample_rate: TARGET_RATE as i32,
        num_threads: 1,
        provider: Some("cpu".into()),
        ..Default::default()
    };
    let vad = VoiceActivityDetector::create(&cfg, 120.0)
        .ok_or_else(|| anyhow!("failed to load the VAD model"))?;
    let mut out = Vec::new();
    let drain = |vad: &VoiceActivityDetector, out: &mut Vec<(usize, usize)>| {
        while let Some(seg) = vad.front() {
            let s = seg.start().max(0) as usize;
            out.push((s, s + seg.n().max(0) as usize));
            vad.pop();
        }
    };
    for (i, chunk) in samples.chunks(512).enumerate() {
        if i % 512 == 0 && cancel.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        vad.accept_waveform(chunk);
        drain(&vad, &mut out);
    }
    vad.flush();
    drain(&vad, &mut out);
    out.sort();
    // pad regions slightly so word onsets/offsets aren't clipped
    crate::chunking::pad_regions(&mut out, samples.len(), TARGET_RATE as usize / 10);
    Ok(out)
}
