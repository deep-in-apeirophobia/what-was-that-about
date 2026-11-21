//! Convert Hugging Face Whisper checkpoints (safetensors) into the legacy
//! whisper.cpp GGML format, optionally quantizing to Q8_0.
//!
//! This is a Rust port of whisper.cpp's `models/convert-h5-to-ggml.py`
//! followed by `examples/quantize` (Q8_0), so users don't need Python/PyTorch.
//! Output is byte-compatible with what those tools produce.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{bail, Context, Result};
use half::f16;
use safetensors::{Dtype, SafeTensors};
use serde::Deserialize;

const GGML_MAGIC: u32 = 0x6767_6d6c;
const GGML_QNT_VERSION: i32 = 2;
const GGML_QNT_VERSION_FACTOR: i32 = 1000;
const GGML_FTYPE_MOSTLY_F16: i32 = 1;
const GGML_FTYPE_MOSTLY_Q8_0: i32 = 7;
const GGML_TYPE_F32: i32 = 0;
const GGML_TYPE_F16: i32 = 1;
const GGML_TYPE_Q8_0: i32 = 8;
const QK8_0: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutType {
    F16,
    Q8_0,
}

impl OutType {
    pub fn parse(s: &str) -> Self {
        if s.eq_ignore_ascii_case("f16") {
            Self::F16
        } else {
            Self::Q8_0
        }
    }
    pub fn file_name(self) -> &'static str {
        match self {
            Self::F16 => "ggml-model-f16.bin",
            Self::Q8_0 => "ggml-model-q8_0.bin",
        }
    }
}

#[derive(Debug, Deserialize)]
struct HfConfig {
    vocab_size: i32,
    max_source_positions: i32,
    d_model: i32,
    encoder_attention_heads: i32,
    encoder_layers: i32,
    max_length: Option<i32>,
    max_target_positions: Option<i32>,
    decoder_attention_heads: i32,
    decoder_layers: i32,
    num_mel_bins: i32,
}

/// Input files of a Hugging Face Whisper checkpoint.
#[derive(Debug, Clone)]
pub struct SourceFiles {
    pub config: PathBuf,
    pub vocab: PathBuf,
    pub shards: Vec<PathBuf>,
}

impl SourceFiles {
    pub fn from_dir(dir: &Path) -> Result<Self> {
        let index = dir.join("model.safetensors.index.json");
        let shards = if index.exists() {
            Self::shard_names_from_index(&index)?
                .into_iter()
                .map(|n| dir.join(n))
                .collect()
        } else {
            vec![dir.join("model.safetensors")]
        };
        Ok(Self {
            config: dir.join("config.json"),
            vocab: dir.join("vocab.json"),
            shards,
        })
    }

    pub fn shard_names_from_index(index: &Path) -> Result<Vec<String>> {
        #[derive(Deserialize)]
        struct Index {
            weight_map: HashMap<String, String>,
        }
        let idx: Index = serde_json::from_slice(&std::fs::read(index)?)
            .context("parsing model.safetensors.index.json")?;
        let mut names: Vec<String> = idx.weight_map.into_values().collect();
        names.sort();
        names.dedup();
        Ok(names)
    }
}

/// GPT-2 style byte <-> unicode mapping used by Whisper's BPE vocab.
fn byte_decoder() -> HashMap<char, u8> {
    let mut bs: Vec<u32> = (b'!' as u32..=b'~' as u32)
        .chain(0xA1..=0xAC)
        .chain(0xAE..=0xFF)
        .collect();
    let mut cs = bs.clone();
    let mut n = 0;
    for b in 0..256u32 {
        if !bs.contains(&b) {
            bs.push(b);
            cs.push(256 + n);
            n += 1;
        }
    }
    bs.into_iter()
        .zip(cs)
        .map(|(b, c)| (char::from_u32(c).unwrap(), b as u8))
        .collect()
}

fn hz_to_mel(f: f64) -> f64 {
    let f_sp = 200.0 / 3.0;
    let min_log_hz = 1000.0;
    let min_log_mel = min_log_hz / f_sp;
    let logstep = (6.4f64).ln() / 27.0;
    if f >= min_log_hz {
        min_log_mel + (f / min_log_hz).ln() / logstep
    } else {
        f / f_sp
    }
}

fn mel_to_hz(m: f64) -> f64 {
    let f_sp = 200.0 / 3.0;
    let min_log_hz = 1000.0;
    let min_log_mel = min_log_hz / f_sp;
    let logstep = (6.4f64).ln() / 27.0;
    if m >= min_log_mel {
        min_log_hz * (logstep * (m - min_log_mel)).exp()
    } else {
        f_sp * m
    }
}

/// Slaney-normalized mel filterbank, identical to `librosa.filters.mel(sr, n_fft, n_mels)`
/// (which is what OpenAI ships in `mel_filters.npz`). Shape: n_mels x (n_fft/2 + 1).
pub fn mel_filters(sr: u32, n_fft: usize, n_mels: usize) -> Vec<f32> {
    let n_freqs = n_fft / 2 + 1;
    let fftfreqs: Vec<f64> = (0..n_freqs)
        .map(|i| i as f64 * sr as f64 / n_fft as f64)
        .collect();
    let min_mel = hz_to_mel(0.0);
    let max_mel = hz_to_mel(sr as f64 / 2.0);
    let n_pts = n_mels + 2;
    let mel_f: Vec<f64> = (0..n_pts)
        .map(|i| {
            let m = min_mel + (max_mel - min_mel) * i as f64 / (n_pts - 1) as f64;
            mel_to_hz(m)
        })
        .collect();
    let mut weights = vec![0f32; n_mels * n_freqs];
    for i in 0..n_mels {
        let fdiff_lo = mel_f[i + 1] - mel_f[i];
        let fdiff_hi = mel_f[i + 2] - mel_f[i + 1];
        let enorm = 2.0 / (mel_f[i + 2] - mel_f[i]);
        for (j, &f) in fftfreqs.iter().enumerate() {
            let lower = -(mel_f[i] - f) / fdiff_lo;
            let upper = (mel_f[i + 2] - f) / fdiff_hi;
            // Match OpenAI's precomputed filters bit-for-bit: exact zeros keep their sign
            // (the very first coefficient is -0.0), negatives clip to +0.0.
            let w = lower.min(upper);
            let w = if w >= 0.0 { w as f32 } else { 0.0 };
            weights[i * n_freqs + j] = (w as f64 * enorm) as f32;
        }
    }
    weights
}

/// HF tensor name -> whisper.cpp tensor name. Returns None for tensors whisper.cpp doesn't use.
fn map_name(hf: &str) -> Option<String> {
    if hf == "proj_out.weight" {
        return None;
    }
    let rest = hf.strip_prefix("model.")?;
    let nn: Vec<&str> = rest.split('.').collect();
    if nn.len() > 3 && nn[1] == "layers" {
        let inner = nn[3..nn.len() - 1].join(".");
        let mapped = match inner.as_str() {
            "encoder_attn.k_proj" => {
                if nn[0] == "encoder" {
                    "attn.key"
                } else {
                    "cross_attn.key"
                }
            }
            "self_attn.k_proj" => "attn.key",
            "self_attn.q_proj" => "attn.query",
            "self_attn.v_proj" => "attn.value",
            "self_attn.out_proj" => "attn.out",
            "self_attn_layer_norm" => "attn_ln",
            "encoder_attn.q_proj" => "cross_attn.query",
            "encoder_attn.v_proj" => "cross_attn.value",
            "encoder_attn.out_proj" => "cross_attn.out",
            "encoder_attn_layer_norm" => "cross_attn_ln",
            "fc1" => "mlp.0",
            "fc2" => "mlp.2",
            "final_layer_norm" => "mlp_ln",
            _ => return None,
        };
        return Some(format!(
            "{}.blocks.{}.{}.{}",
            nn[0],
            nn[2],
            mapped,
            nn[nn.len() - 1]
        ));
    }
    let name = nn.join(".");
    Some(
        match name.as_str() {
            "encoder.layer_norm.bias" => "encoder.ln_post.bias",
            "encoder.layer_norm.weight" => "encoder.ln_post.weight",
            "encoder.embed_positions.weight" => "encoder.positional_embedding",
            "decoder.layer_norm.bias" => "decoder.ln.bias",
            "decoder.layer_norm.weight" => "decoder.ln.weight",
            "decoder.embed_positions.weight" => "decoder.positional_embedding",
            "decoder.embed_tokens.weight" => "decoder.token_embedding.weight",
            other => other,
        }
        .to_string(),
    )
}

/// HF tensor names in PyTorch `state_dict()` order, so output matches the Python converter.
fn hf_tensor_order(enc_layers: i32, dec_layers: i32) -> Vec<String> {
    let mut v = Vec::new();
    for t in [
        "conv1.weight",
        "conv1.bias",
        "conv2.weight",
        "conv2.bias",
        "embed_positions.weight",
    ] {
        v.push(format!("model.encoder.{t}"));
    }
    let attn = |p: &str, a: &str, v: &mut Vec<String>| {
        v.push(format!("{p}.{a}.k_proj.weight"));
        for proj in ["v_proj", "q_proj", "out_proj"] {
            v.push(format!("{p}.{a}.{proj}.weight"));
            v.push(format!("{p}.{a}.{proj}.bias"));
        }
        v.push(format!("{p}.{a}_layer_norm.weight"));
        v.push(format!("{p}.{a}_layer_norm.bias"));
    };
    let mlp = |p: &str, v: &mut Vec<String>| {
        for t in [
            "fc1.weight",
            "fc1.bias",
            "fc2.weight",
            "fc2.bias",
            "final_layer_norm.weight",
            "final_layer_norm.bias",
        ] {
            v.push(format!("{p}.{t}"));
        }
    };
    for i in 0..enc_layers {
        let p = format!("model.encoder.layers.{i}");
        attn(&p, "self_attn", &mut v);
        mlp(&p, &mut v);
    }
    v.push("model.encoder.layer_norm.weight".into());
    v.push("model.encoder.layer_norm.bias".into());
    v.push("model.decoder.embed_tokens.weight".into());
    v.push("model.decoder.embed_positions.weight".into());
    for i in 0..dec_layers {
        let p = format!("model.decoder.layers.{i}");
        attn(&p, "self_attn", &mut v);
        attn(&p, "encoder_attn", &mut v);
        mlp(&p, &mut v);
    }
    v.push("model.decoder.layer_norm.weight".into());
    v.push("model.decoder.layer_norm.bias".into());
    v
}

fn to_f32(dtype: Dtype, data: &[u8]) -> Result<Vec<f32>> {
    Ok(match dtype {
        Dtype::F32 => data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect(),
        Dtype::F16 => data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| f16::from_le_bytes(*c).to_f32())
            .collect(),
        Dtype::BF16 => data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| half::bf16::from_le_bytes(*c).to_f32())
            .collect(),
        other => bail!("unsupported tensor dtype {other:?}"),
    })
}

/// Q8_0 reference quantization (ggml `quantize_row_q8_0_ref`). `x.len()` must be a multiple of 32.
pub fn quantize_q8_0(x: &[f32], out: &mut Vec<u8>) {
    for block in x.as_chunks::<QK8_0>().0 {
        let amax = block.iter().fold(0f32, |m, v| m.max(v.abs()));
        let d = amax / 127.0;
        let id = if d != 0.0 { 1.0 / d } else { 0.0 };
        out.extend_from_slice(&f16::from_f32(d).to_le_bytes());
        for &v in block {
            out.push((v * id).round() as i8 as u8);
        }
    }
}

pub struct ConvertProgress {
    pub done_bytes: u64,
    pub total_bytes: u64,
}

/// Convert the HF checkpoint `src` into a whisper.cpp model at `out_path`.
/// Writes to a temporary file first and renames on success.
pub fn convert(
    src: &SourceFiles,
    out_path: &Path,
    out_type: OutType,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(ConvertProgress),
) -> Result<()> {
    let cfg: HfConfig =
        serde_json::from_slice(&std::fs::read(&src.config).context("reading config.json")?)
            .context("parsing config.json")?;
    let vocab: HashMap<String, i64> =
        serde_json::from_slice(&std::fs::read(&src.vocab).context("reading vocab.json")?)
            .context("parsing vocab.json")?;

    // Map every tensor name to the shard holding it.
    let shard_paths = &src.shards;
    let mut maps = Vec::new();
    for p in shard_paths {
        let f = File::open(p).with_context(|| format!("opening {}", p.display()))?;
        // SAFETY: the file is opened read-only and not modified while mapped.
        let mmap = unsafe { memmap2::Mmap::map(&f)? };
        maps.push(mmap);
    }
    let shards: Vec<SafeTensors> = maps
        .iter()
        .zip(shard_paths)
        .map(|(m, p)| {
            SafeTensors::deserialize(m).with_context(|| format!("parsing {}", p.display()))
        })
        .collect::<Result<_>>()?;
    let mut location: HashMap<String, usize> = HashMap::new();
    for (i, st) in shards.iter().enumerate() {
        for name in st.names() {
            location.insert(name.to_string(), i);
        }
    }

    let order = hf_tensor_order(cfg.encoder_layers, cfg.decoder_layers);
    for name in &order {
        if !location.contains_key(name) {
            bail!("checkpoint is missing tensor {name}; is this a Whisper model?");
        }
    }
    let total_bytes: u64 = order
        .iter()
        .map(|n| {
            shards[location[n]]
                .tensor(n)
                .map(|t| t.data().len() as u64)
                .unwrap_or(0)
        })
        .sum();

    let tmp_path = out_path.with_extension("bin.part");
    let mut w = BufWriter::with_capacity(8 << 20, File::create(&tmp_path)?);
    let wi = |w: &mut BufWriter<File>, v: i32| w.write_all(&v.to_le_bytes());

    let ftype = match out_type {
        OutType::F16 => GGML_FTYPE_MOSTLY_F16,
        OutType::Q8_0 => GGML_QNT_VERSION * GGML_QNT_VERSION_FACTOR + GGML_FTYPE_MOSTLY_Q8_0,
    };
    w.write_all(&GGML_MAGIC.to_le_bytes())?;
    for v in [
        cfg.vocab_size,
        cfg.max_source_positions,
        cfg.d_model,
        cfg.encoder_attention_heads,
        cfg.encoder_layers,
        cfg.max_length.or(cfg.max_target_positions).unwrap_or(448),
        cfg.d_model,
        cfg.decoder_attention_heads,
        cfg.decoder_layers,
        cfg.num_mel_bins,
        ftype,
    ] {
        wi(&mut w, v)?;
    }

    // mel filters
    let n_mels = cfg.num_mel_bins as usize;
    let n_fft = 400usize;
    let filters = mel_filters(16000, n_fft, n_mels);
    wi(&mut w, n_mels as i32)?;
    wi(&mut w, (n_fft / 2 + 1) as i32)?;
    for f in &filters {
        w.write_all(&f.to_le_bytes())?;
    }

    // vocab (byte-decoded BPE tokens, sorted by id)
    let dec = byte_decoder();
    let mut tokens: Vec<(&String, &i64)> = vocab.iter().collect();
    tokens.sort_by_key(|(_, id)| **id);
    wi(&mut w, tokens.len() as i32)?;
    for (tok, _) in tokens {
        let bytes: Vec<u8> = tok
            .chars()
            .map(|c| dec.get(&c).copied().unwrap_or(b'?'))
            .collect();
        wi(&mut w, bytes.len() as i32)?;
        w.write_all(&bytes)?;
    }

    let mut done_bytes = 0u64;
    let mut qbuf = Vec::new();
    for hf_name in &order {
        if cancel.load(Ordering::Relaxed) {
            drop(w);
            let _ = std::fs::remove_file(&tmp_path);
            bail!("cancelled");
        }
        let Some(name) = map_name(hf_name) else {
            continue;
        };
        let view = shards[location[hf_name]].tensor(hf_name)?;
        let mut shape: Vec<usize> = view.shape().iter().copied().filter(|&d| d != 1).collect();
        if name == "encoder.conv1.bias" || name == "encoder.conv2.bias" {
            shape = vec![shape[0], 1];
        }
        let n_dims = shape.len();
        let data = to_f32(view.dtype(), view.data())?;
        // Round through f16 exactly like the Python converter + quantizer pipeline.
        let keep_f32 = n_dims < 2
            || matches!(
                name.as_str(),
                "encoder.conv1.bias"
                    | "encoder.conv2.bias"
                    | "encoder.positional_embedding"
                    | "decoder.positional_embedding"
            );
        let quantize = out_type == OutType::Q8_0 && !keep_f32 && n_dims == 2;
        let ttype = if keep_f32 {
            GGML_TYPE_F32
        } else if quantize {
            GGML_TYPE_Q8_0
        } else {
            GGML_TYPE_F16
        };

        wi(&mut w, n_dims as i32)?;
        wi(&mut w, name.len() as i32)?;
        wi(&mut w, ttype)?;
        for i in 0..n_dims {
            wi(&mut w, shape[n_dims - 1 - i] as i32)?;
        }
        w.write_all(name.as_bytes())?;

        match ttype {
            GGML_TYPE_F32 => {
                for v in &data {
                    // numpy: f32 -> f16 -> f32
                    w.write_all(&f16::from_f32(*v).to_f32().to_le_bytes())?;
                }
            }
            GGML_TYPE_F16 => {
                let mut buf = Vec::with_capacity(data.len() * 2);
                for v in &data {
                    buf.extend_from_slice(&f16::from_f32(*v).to_le_bytes());
                }
                w.write_all(&buf)?;
            }
            _ => {
                if !shape[n_dims - 1].is_multiple_of(QK8_0) {
                    bail!("tensor {name} row size is not a multiple of {QK8_0}");
                }
                let rounded: Vec<f32> = data.iter().map(|v| f16::from_f32(*v).to_f32()).collect();
                qbuf.clear();
                quantize_q8_0(&rounded, &mut qbuf);
                w.write_all(&qbuf)?;
            }
        }
        done_bytes += view.data().len() as u64;
        progress(ConvertProgress {
            done_bytes,
            total_bytes,
        });
    }
    w.flush()?;
    drop(w);
    std::fs::rename(&tmp_path, out_path)?;
    Ok(())
}

/// Minimal header check for an existing whisper.cpp GGML model file.
pub fn looks_like_ggml(path: &Path) -> bool {
    let Ok(mut f) = File::open(path) else {
        return false;
    };
    let mut magic = [0u8; 4];
    use std::io::Read;
    f.read_exact(&mut magic).is_ok() && u32::from_le_bytes(magic) == GGML_MAGIC
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_mapping_matches_python_converter() {
        assert_eq!(
            map_name("model.encoder.layers.3.self_attn.k_proj.weight").unwrap(),
            "encoder.blocks.3.attn.key.weight"
        );
        assert_eq!(
            map_name("model.decoder.layers.0.encoder_attn.k_proj.weight").unwrap(),
            "decoder.blocks.0.cross_attn.key.weight"
        );
        assert_eq!(
            map_name("model.decoder.layers.31.final_layer_norm.bias").unwrap(),
            "decoder.blocks.31.mlp_ln.bias"
        );
        assert_eq!(
            map_name("model.decoder.embed_tokens.weight").unwrap(),
            "decoder.token_embedding.weight"
        );
        assert_eq!(
            map_name("model.encoder.conv1.weight").unwrap(),
            "encoder.conv1.weight"
        );
        assert!(map_name("proj_out.weight").is_none());
    }

    #[test]
    fn tensor_order_count_matches_large_v3() {
        // 1259 tensors in the HF index for large-v3 (no proj_out stored)
        assert_eq!(hf_tensor_order(32, 32).len(), 1259);
    }

    #[test]
    fn mel_filters_are_slaney_normalized() {
        let f = mel_filters(16000, 400, 80);
        assert_eq!(f.len(), 80 * 201);
        // first filter peaks around bin 1 with known librosa value 0.02486259
        assert!((f[1] - 0.024_862_59).abs() < 1e-6, "{}", f[1]);
        assert!(f.iter().all(|v| *v >= 0.0));
    }

    #[test]
    fn byte_decoder_roundtrip_space() {
        let d = byte_decoder();
        assert_eq!(d[&'Ġ'], b' ');
        assert_eq!(d[&'a'], b'a');
        assert_eq!(d.len(), 256);
    }

    #[test]
    fn q8_0_block() {
        let mut x = vec![0f32; 32];
        x[0] = 1.0;
        x[1] = -0.5;
        let mut out = Vec::new();
        quantize_q8_0(&x, &mut out);
        assert_eq!(out.len(), 34);
        assert_eq!(out[2] as i8, 127);
        assert_eq!(out[3] as i8, -64); // -0.5*127 = -63.5 -> rounds away from zero
    }
}
