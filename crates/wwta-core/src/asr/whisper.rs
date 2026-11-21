//! Whisper (Taraneh-WLarge) through whisper.cpp. Audio is fed in VAD-aligned chunks of
//! up to ~28 s so every timestamp (segments and tokens) maps exactly to the original audio.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, bail, Result};
use whisper_rs::whisper_rs_sys::whisper_ahead;
use whisper_rs::{
    DtwMode, DtwParameters, FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters,
};

use super::{effective_threads, Engine, Progress, TranscribeOptions};
use crate::audio::TARGET_RATE;
use crate::chunking::{merge_regions, words_from_byte_tokens};
use crate::types::{GpuDevice, Segment};

/// Compiled GPU backend.
pub const GPU_BACKEND: &str = if cfg!(feature = "vulkan") {
    "vulkan"
} else if cfg!(feature = "metal") {
    "metal"
} else {
    "none"
};

/// Whether the GPU runtime this build links against is present. On Windows the Vulkan
/// loader is delay-loaded, so a machine without GPU drivers can still run the app (and
/// Shenava); Whisper is unavailable there because ggml always probes Vulkan.
pub fn runtime_available() -> bool {
    #[cfg(all(windows, feature = "vulkan"))]
    {
        use std::os::windows::ffi::OsStrExt;
        extern "system" {
            fn LoadLibraryW(name: *const u16) -> *mut core::ffi::c_void;
        }
        let name: Vec<u16> = std::ffi::OsStr::new("vulkan-1.dll")
            .encode_wide()
            .chain([0])
            .collect();
        // SAFETY: plain LoadLibraryW call with a NUL-terminated wide string.
        unsafe { !LoadLibraryW(name.as_ptr()).is_null() }
    }
    #[cfg(not(all(windows, feature = "vulkan")))]
    {
        true
    }
}

/// GPU devices as whisper.cpp counts them (ggml backend devices of type GPU or IGPU, in
/// registry order). Index in this list = whisper's `gpu_device` parameter.
struct GgmlGpu {
    description: String,
    discrete: bool,
    total: usize,
}

fn ggml_gpus() -> Vec<GgmlGpu> {
    if GPU_BACKEND == "none" || !runtime_available() {
        return vec![];
    }
    use whisper_rs::whisper_rs_sys as sys;
    let mut out = Vec::new();
    // SAFETY: read-only queries of ggml's backend registry; strings are owned by ggml.
    unsafe {
        for i in 0..sys::ggml_backend_dev_count() {
            let dev = sys::ggml_backend_dev_get(i);
            let t = sys::ggml_backend_dev_type(dev);
            let discrete = t == sys::ggml_backend_dev_type_GGML_BACKEND_DEVICE_TYPE_GPU;
            if !discrete && t != sys::ggml_backend_dev_type_GGML_BACKEND_DEVICE_TYPE_IGPU {
                continue;
            }
            let (mut free, mut total) = (0usize, 0usize);
            sys::ggml_backend_dev_memory(dev, &mut free, &mut total);
            let d = sys::ggml_backend_dev_description(dev);
            let description = if d.is_null() {
                "GPU".to_string()
            } else {
                std::ffi::CStr::from_ptr(d).to_string_lossy().into_owned()
            };
            out.push(GgmlGpu {
                description,
                discrete,
                total,
            });
        }
    }
    out
}

/// Pick the discrete GPU with the most memory (laptops often list the integrated GPU
/// first), else the first integrated GPU.
fn best_gpu(gpus: &[GgmlGpu]) -> Option<usize> {
    (0..gpus.len()).max_by(|&a, &b| {
        let (ga, gb) = (&gpus[a], &gpus[b]);
        (ga.discrete, if ga.discrete { ga.total } else { 0 })
            .cmp(&(gb.discrete, if gb.discrete { gb.total } else { 0 }))
            .then(b.cmp(&a)) // prefer the earlier device on ties
    })
}

pub fn gpu_devices() -> Vec<GpuDevice> {
    let gpus = ggml_gpus();
    let best = best_gpu(&gpus);
    gpus.iter()
        .enumerate()
        .map(|(i, g)| GpuDevice {
            name: g.description.clone(),
            description: format!(
                "{} · {}{}",
                GPU_BACKEND,
                if g.discrete { "discrete" } else { "integrated" },
                if Some(i) == best {
                    " · used for Whisper"
                } else {
                    ""
                }
            ),
            memory_bytes: (g.total > 0).then_some(g.total as u64),
        })
        .collect()
}

unsafe extern "C" fn abort_requested(user_data: *mut std::ffi::c_void) -> bool {
    // SAFETY: user_data is the AtomicBool behind the job's cancel Arc (see `transcribe`).
    unsafe { (*(user_data as *const AtomicBool)).load(Ordering::Relaxed) }
}

pub struct Whisper {
    ctx: WhisperContext,
    gpu: bool,
    dtw: bool,
    device: String,
}

impl Whisper {
    pub fn load(path: &Path, use_gpu: bool, alignment_heads: &[(i32, i32)]) -> Result<Self> {
        if !runtime_available() {
            bail!(
                "Whisper models need a GPU driver with Vulkan support, which wasn't found on this computer. \
                 Install or update your graphics driver, or use the Shenava model."
            );
        }
        whisper_rs::install_logging_hooks();
        let path_str = path
            .to_str()
            .ok_or_else(|| anyhow!("model path is not valid UTF-8"))?;
        let gpus = if use_gpu { ggml_gpus() } else { vec![] };
        let gpu_index = best_gpu(&gpus);
        let mut params = WhisperContextParameters::default();
        params.use_gpu(use_gpu && gpu_index.is_some());
        if let Some(i) = gpu_index {
            params.gpu_device(i as i32);
        }
        let dtw = !alignment_heads.is_empty();
        // DTW timestamps are incompatible with flash attention in whisper.cpp.
        params.flash_attn(!dtw);
        if dtw {
            // whisper.cpp keeps a pointer to the heads for the context's lifetime.
            let heads: &'static [whisper_ahead] = Box::leak(
                alignment_heads
                    .iter()
                    .map(|&(l, h)| whisper_ahead {
                        n_text_layer: l,
                        n_head: h,
                    })
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            );
            params.dtw_parameters(DtwParameters {
                mode: DtwMode::Custom { aheads: heads },
                dtw_mem_size: 1024 * 1024 * 128,
            });
        }
        let ctx = WhisperContext::new_with_params(path_str, params)
            .map_err(|e| anyhow!("failed to load the Whisper model: {e:?}"))?;
        let device = match gpu_index {
            Some(i) => format!("{} · {}", GPU_BACKEND, gpus[i].description),
            None if use_gpu => "CPU (no GPU found)".into(),
            None => "CPU".into(),
        };
        let gpu = use_gpu && !device.starts_with("CPU");
        Ok(Self {
            ctx,
            gpu,
            dtw,
            device,
        })
    }
}

impl Engine for Whisper {
    fn device(&self) -> String {
        self.device.clone()
    }

    fn transcribe(
        &mut self,
        samples: &[f32],
        regions: &[(usize, usize)],
        opts: &TranscribeOptions,
        cancel: &Arc<AtomicBool>,
        progress: &mut Progress<'_>,
    ) -> Result<Vec<Segment>> {
        let sr = TARGET_RATE as usize;
        let chunks = merge_regions(regions, samples.len(), 28 * sr, sr / 5);
        let mut state = self
            .ctx
            .create_state()
            .map_err(|e| anyhow!("failed to create Whisper state: {e:?}"))?;
        let eot = self.ctx.token_eot();
        let threads = effective_threads(opts.threads);
        let mut out = Vec::new();
        let n_chunks = chunks.len().max(1);
        for (ci, &(s, e)) in chunks.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                bail!("cancelled");
            }
            let strategy = if self.gpu {
                SamplingStrategy::BeamSearch {
                    beam_size: 5,
                    patience: -1.0,
                }
            } else {
                SamplingStrategy::Greedy { best_of: 2 }
            };
            let mut params = FullParams::new(strategy);
            params.set_language(Some(&opts.language));
            params.set_translate(false);
            params.set_n_threads(threads);
            params.set_no_context(true);
            params.set_token_timestamps(true);
            params.set_print_special(false);
            params.set_print_progress(false);
            params.set_print_realtime(false);
            params.set_print_timestamps(false);
            params.set_suppress_blank(true);
            params.set_suppress_nst(true);
            // whisper-rs's `set_abort_callback_safe` casts its boxed closure to the wrong type
            // (it always reads "abort"), so register a plain C callback on the flag instead.
            // SAFETY: `cancel` outlives `state.full` below and is only read atomically.
            unsafe {
                params.set_abort_callback(Some(abort_requested));
                params.set_abort_callback_user_data(Arc::as_ptr(cancel) as *mut std::ffi::c_void);
            }

            if let Err(err) = state.full(params, &samples[s..e]) {
                if cancel.load(Ordering::Relaxed) {
                    bail!("cancelled");
                }
                return Err(anyhow!("Whisper failed: {err:?}"));
            }
            let offset = s as f64 / sr as f64;
            let chunk_end = e as f64 / sr as f64;
            let mut new = Vec::new();
            for i in 0..state.full_n_segments() {
                let Some(seg) = state.get_segment(i) else {
                    continue;
                };
                let t0 = offset + seg.start_timestamp() as f64 / 100.0;
                let t1 = (offset + seg.end_timestamp() as f64 / 100.0).min(chunk_end);
                let text = seg
                    .to_str_lossy()
                    .map(|t| t.trim().to_string())
                    .unwrap_or_default();
                if text.is_empty() {
                    continue;
                }
                let mut toks: Vec<(Vec<u8>, f64, f64)> = Vec::new();
                for k in 0..seg.n_tokens() {
                    let Some(tok) = seg.get_token(k) else {
                        continue;
                    };
                    if tok.token_id() >= eot {
                        continue;
                    }
                    let d = tok.token_data();
                    let bytes = tok.to_bytes().map(|b| b.to_vec()).unwrap_or_default();
                    let (a, b) = if self.dtw && d.t_dtw >= 0 {
                        let t = offset + d.t_dtw as f64 / 100.0;
                        (t, t)
                    } else {
                        (offset + d.t0 as f64 / 100.0, offset + d.t1 as f64 / 100.0)
                    };
                    toks.push((bytes, a, b));
                }
                let mut words = words_from_byte_tokens(&toks);
                if self.dtw {
                    let n = words.len();
                    for w in 0..n {
                        let next = if w + 1 < n { words[w + 1].start } else { t1 };
                        words[w].end = next.min(words[w].start + 1.5).max(words[w].start + 0.04);
                    }
                }
                // Taraneh sometimes starts a segment with a bare " 1" token that has no audio
                // behind it (a fine-tuning artifact also seen in whisper.cpp's CLI). Drop it when
                // it is glued to the next word or shares its timestamp.
                let mut text = text;
                if toks.len() > 1 {
                    let first = String::from_utf8_lossy(&toks[0].0).to_string();
                    let digits = first.trim();
                    let glued = !toks[1].0.starts_with(b" ");
                    let same_time = (toks[1].1 - toks[0].1).abs() < 0.05;
                    if !digits.is_empty()
                        && digits.len() <= 2
                        && digits.chars().all(|c| c.is_ascii_digit())
                        && (glued || same_time)
                    {
                        toks.remove(0);
                        text = text
                            .trim_start()
                            .strip_prefix(digits)
                            .unwrap_or(&text)
                            .trim()
                            .to_string();
                        words = words_from_byte_tokens(&toks);
                        if self.dtw {
                            let n = words.len();
                            for w in 0..n {
                                let next = if w + 1 < n { words[w + 1].start } else { t1 };
                                words[w].end =
                                    next.min(words[w].start + 1.5).max(words[w].start + 0.04);
                            }
                        }
                    }
                }
                for w in &mut words {
                    w.start = w.start.clamp(t0, t1);
                    w.end = w.end.clamp(w.start, t1.max(w.start));
                }
                new.push(Segment {
                    id: 0,
                    start: t0,
                    end: t1.max(t0),
                    text,
                    words,
                });
            }
            out.extend(new.iter().cloned());
            progress((ci + 1) as f32 / n_chunks as f32, new);
        }
        Ok(out)
    }
}
