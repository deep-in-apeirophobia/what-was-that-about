//! Shenava (NeMo FastConformer CTC) through sherpa-onnx.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, bail, Result};
use sherpa_onnx::{OfflineNemoEncDecCtcModelConfig, OfflineRecognizer, OfflineRecognizerConfig};

use super::{Engine, Progress, TranscribeOptions};
use crate::audio::TARGET_RATE;
use crate::chunking::{merge_regions, words_from_pieces};
use crate::types::Segment;

pub struct Shenava {
    rec: OfflineRecognizer,
    threads: i32,
}

impl Shenava {
    pub fn load(model: &Path, tokens: &Path, threads: i32) -> Result<Self> {
        if !model.is_file() || !tokens.is_file() {
            bail!("Shenava model files are missing; reinstall the model");
        }
        let mut config = OfflineRecognizerConfig::default();
        config.model_config.nemo_ctc = OfflineNemoEncDecCtcModelConfig {
            model: Some(model.to_string_lossy().into_owned()),
        };
        config.model_config.tokens = Some(tokens.to_string_lossy().into_owned());
        config.model_config.num_threads = threads;
        config.model_config.provider = Some("cpu".into());
        config.feat_config.sample_rate = TARGET_RATE as i32;
        config.feat_config.feature_dim = 80;
        config.decoding_method = Some("greedy_search".into());
        let rec = OfflineRecognizer::create(&config)
            .ok_or_else(|| anyhow!("failed to load the Shenava model (is model.onnx valid?)"))?;
        Ok(Self { rec, threads })
    }
}

impl Engine for Shenava {
    fn device(&self) -> String {
        format!("CPU · {} threads", self.threads)
    }

    fn transcribe(
        &mut self,
        samples: &[f32],
        regions: &[(usize, usize)],
        _opts: &TranscribeOptions,
        cancel: &Arc<AtomicBool>,
        progress: &mut Progress<'_>,
    ) -> Result<Vec<Segment>> {
        let sr = TARGET_RATE as f64;
        // Merge nearby speech into ~12 s windows (more context, fewer fragments); sentence
        // splitting at pauses happens afterwards using the word timings.
        let regions = merge_regions(
            regions,
            samples.len(),
            12 * TARGET_RATE as usize,
            TARGET_RATE as usize / 4,
        );
        let regions = regions.as_slice();
        // Trailing silence lets the limited-context conformer flush its last frames,
        // otherwise final syllables get dropped.
        let tail = vec![0f32; TARGET_RATE as usize / 2];
        let total: usize = regions.iter().map(|(s, e)| e - s).sum::<usize>().max(1);
        let mut done = 0usize;
        let mut out = Vec::new();
        for batch in regions.chunks(4) {
            if cancel.load(Ordering::Relaxed) {
                bail!("cancelled");
            }
            let streams: Vec<_> = batch
                .iter()
                .map(|&(s, e)| {
                    let st = self.rec.create_stream();
                    let mut buf = Vec::with_capacity(e - s + tail.len());
                    buf.extend_from_slice(&samples[s..e]);
                    buf.extend_from_slice(&tail);
                    st.accept_waveform(TARGET_RATE as i32, &buf);
                    st
                })
                .collect();
            let refs: Vec<_> = streams.iter().collect();
            self.rec.decode_multiple_streams(&refs);
            let mut new = Vec::new();
            for (st, &(s, e)) in streams.iter().zip(batch) {
                done += e - s;
                let Some(r) = st.get_result() else { continue };
                let text = r.text.trim().to_string();
                if text.is_empty() {
                    continue;
                }
                let start = s as f64 / sr;
                let end = e as f64 / sr;
                let words = match &r.timestamps {
                    Some(ts) if ts.len() == r.tokens.len() => {
                        words_from_pieces(&r.tokens, ts, start, end)
                    }
                    _ => vec![],
                };
                new.push(Segment {
                    id: 0,
                    start,
                    end,
                    text,
                    words,
                    original_text: None,
                });
            }
            out.extend(new.iter().cloned());
            progress(done as f32 / total as f32, new);
        }
        Ok(out)
    }
}
