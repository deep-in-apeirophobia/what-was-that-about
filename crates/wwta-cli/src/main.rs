//! `wwta` — headless access to the What Was That About engine.
//!
//! wwta info
//! wwta decode <audio|video|flv|adobe-connect.zip|folder> <out.wav>
//! wwta convert <hf-model-dir> <out.bin> [--f16]
//! wwta transcribe <audio> --model-file <path> --kind whisper|shenava [--cpu] [--json] [--no-dtw]
//! wwta install <model-id> --models-dir <dir> [--f16]

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use wwta_core::{ggml, models};

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

fn has(args: &[String], flag: &str) -> bool {
    args.iter().any(|a| a == flag)
}

fn usage() -> ! {
    eprintln!(
        "usage:\n  wwta info\n  wwta decode <input> <out.wav>\n  wwta convert <hf-model-dir> <out.bin> [--f16]\n  \
         wwta transcribe <audio> --model-file <path> --kind whisper|shenava [--cpu] [--json] [--no-dtw] [--no-digits]\n  \
         wwta install <model-id> --models-dir <dir> [--f16]"
    );
    std::process::exit(2)
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("info") => info(),
        Some("convert") => {
            let (Some(src), Some(out)) = (args.get(1), args.get(2)) else {
                usage()
            };
            let ty = if has(&args, "--f16") {
                ggml::OutType::F16
            } else {
                ggml::OutType::Q8_0
            };
            let sources = ggml::SourceFiles::from_dir(&PathBuf::from(src))?;
            let cancel = AtomicBool::new(false);
            let t = std::time::Instant::now();
            let mut last = 0u64;
            ggml::convert(&sources, &PathBuf::from(out), ty, &cancel, &mut |p| {
                let pct = p.done_bytes * 100 / p.total_bytes.max(1);
                if pct >= last + 10 {
                    last = pct;
                    eprintln!("converting… {pct}%");
                }
            })?;
            eprintln!("done in {:.1}s", t.elapsed().as_secs_f32());
            Ok(())
        }
        Some("decode") => {
            let (Some(input), Some(out)) = (args.get(1), args.get(2)) else {
                usage()
            };
            let cancel = AtomicBool::new(false);
            let pcm =
                wwta_core::audio::decode_to_16k_mono(&PathBuf::from(input), &cancel, &mut |_| {})
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
            wwta_core::audio::write_wav_16k(&PathBuf::from(out), &pcm)?;
            eprintln!(
                "wrote {:.1}s of 16 kHz mono audio",
                wwta_core::audio::duration_secs(pcm.len())
            );
            Ok(())
        }
        Some("install") => {
            let Some(id) = args.get(1) else { usage() };
            let dir =
                PathBuf::from(arg_value(&args, "--models-dir").unwrap_or_else(|| "models".into()));
            let spec = models::spec(id).context("unknown model id")?;
            let ty = if has(&args, "--f16") {
                ggml::OutType::F16
            } else {
                ggml::OutType::Q8_0
            };
            let cancel = AtomicBool::new(false);
            models::install(&dir, &spec, ty, false, &cancel, &mut |p| {
                eprintln!(
                    "{:?} {:?} {}/{} {}",
                    p.stage,
                    p.file,
                    p.done_bytes,
                    p.total_bytes,
                    p.message.unwrap_or_default()
                );
            })
            .await?;
            println!("{}", models::model_file(&dir, id).unwrap().display());
            Ok(())
        }
        #[cfg(feature = "asr")]
        Some("transcribe") => transcribe(&args).await,
        _ => usage(),
    }
}

fn info() -> Result<()> {
    println!("what-was-that-about {}", env!("CARGO_PKG_VERSION"));
    #[cfg(feature = "asr")]
    {
        use wwta_core::asr::whisper;
        println!("gpu backend: {}", whisper::GPU_BACKEND);
        println!("gpu runtime available: {}", whisper::runtime_available());
        for d in whisper::gpu_devices() {
            println!("gpu device: {} ({:?} bytes)", d.name, d.memory_bytes);
        }
    }
    Ok(())
}

#[cfg(feature = "asr")]
async fn transcribe(args: &[String]) -> Result<()> {
    use wwta_core::asr::{self, TranscribeOptions};
    let Some(audio) = args.get(1) else { usage() };
    let model_file = PathBuf::from(arg_value(args, "--model-file").unwrap_or_else(|| usage()));
    let kind = arg_value(args, "--kind").unwrap_or_else(|| usage());
    let spec = models::catalog()
        .into_iter()
        .find(|m| match kind.as_str() {
            "whisper" => m.kind == models::ModelKind::WhisperHf,
            "shenava" => m.kind == models::ModelKind::SherpaNemoCtc,
            _ => false,
        })
        .unwrap_or_else(|| usage());
    let opts = TranscribeOptions {
        use_gpu: !has(args, "--cpu"),
        precise_word_timings: !has(args, "--no-dtw"),
        persian_digits: !has(args, "--no-digits"),
        ..Default::default()
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let t = std::time::Instant::now();
    let samples = wwta_core::audio::decode_to_16k_mono(&PathBuf::from(audio), &cancel, &mut |_| {})
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let secs = wwta_core::audio::duration_secs(samples.len());
    eprintln!(
        "decoded {secs:.1}s of audio in {:.2}s",
        t.elapsed().as_secs_f32()
    );
    let t = std::time::Instant::now();
    let mut engine = asr::load_engine(&spec, &model_file, &opts)?;
    eprintln!(
        "loaded {} on {} in {:.2}s",
        spec.name,
        engine.device(),
        t.elapsed().as_secs_f32()
    );
    let vad = asr::ensure_vad_model(&std::env::temp_dir().join("wwta-vad"))?;
    let t = std::time::Instant::now();
    let max_region = if kind == "whisper" { 25.0 } else { 20.0 };
    let segs = asr::transcribe(
        engine.as_mut(),
        &samples,
        &vad,
        max_region,
        &opts,
        &cancel,
        &mut |p, new| {
            for s in &new {
                eprintln!(
                    "  [{:.0}%] {:.2}-{:.2} {}",
                    p * 100.0,
                    s.start,
                    s.end,
                    s.text
                );
            }
        },
    )?;
    let el = t.elapsed().as_secs_f32();
    eprintln!(
        "transcribed in {el:.2}s ({:.1}x realtime), {} segments",
        secs as f32 / el.max(1e-3),
        segs.len()
    );
    if has(args, "--json") {
        println!("{}", serde_json::to_string_pretty(&segs)?);
    } else {
        for s in &segs {
            println!(
                "[{} - {}] {}",
                wwta_core::text::fmt_time(s.start),
                wwta_core::text::fmt_time(s.end),
                s.text
            );
        }
    }
    if segs.is_empty() {
        bail!("no speech recognized");
    }
    Ok(())
}
