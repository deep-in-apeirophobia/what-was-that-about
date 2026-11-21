//! Audio decoding (Symphonia), resampling to 16 kHz mono, waveform peaks and WAV I/O.

use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{bail, Context, Result};
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

pub const TARGET_RATE: u32 = 16_000;
pub const PEAKS_PER_SECOND: u32 = 50;

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    /// The container/codec is not supported by the native decoder; the UI may try the webview.
    #[error("unsupported audio format: {0}")]
    Unsupported(String),
    #[error("cancelled")]
    Cancelled,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Streaming Kaiser-windowed sinc resampler (mono).
pub struct Resampler {
    step: f64,
    half: usize,
    phases: usize,
    table: Vec<f32>,
    buf: Vec<f32>,
    buf_off: u64,
    next_out: u64,
    total_in: u64,
    passthrough: bool,
}

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let q = x * x / 4.0;
    for k in 1..50 {
        term *= q / (k as f64 * k as f64);
        sum += term;
        if term < 1e-12 * sum {
            break;
        }
    }
    sum
}

impl Resampler {
    pub fn new(in_rate: u32, out_rate: u32) -> Self {
        let step = in_rate as f64 / out_rate as f64;
        let cutoff = if in_rate > out_rate {
            0.97 * out_rate as f64 / in_rate as f64
        } else {
            1.0
        };
        let zero_crossings = 16.0;
        let half_width = zero_crossings / cutoff;
        let half = half_width.ceil() as usize;
        let phases = 512usize;
        let beta = 8.6;
        let i0b = bessel_i0(beta);
        let taps = 2 * half;
        let mut table = vec![0f32; (phases + 1) * taps];
        for p in 0..=phases {
            let frac = p as f64 / phases as f64;
            for k in 0..taps {
                // tap k multiplies x[i0 + k - half + 1]
                let x = (k as f64 - half as f64 + 1.0) - frac;
                let v = if x.abs() >= half_width {
                    0.0
                } else {
                    let s = if x == 0.0 {
                        1.0
                    } else {
                        let a = std::f64::consts::PI * cutoff * x;
                        a.sin() / a
                    };
                    let r = x / half_width;
                    let w = bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / i0b;
                    cutoff * s * w
                };
                table[p * taps + k] = v as f32;
            }
        }
        Self {
            step,
            half,
            phases,
            table,
            buf: Vec::new(),
            buf_off: 0,
            next_out: 0,
            total_in: 0,
            passthrough: in_rate == out_rate,
        }
    }

    fn produce(&mut self, out: &mut Vec<f32>, limit_in: u64) {
        let taps = 2 * self.half;
        loop {
            let t = self.next_out as f64 * self.step;
            let i0 = t.floor() as u64;
            if i0 + self.half as u64 >= limit_in {
                break;
            }
            let mut frac = t - i0 as f64;
            let mut base = i0;
            let mut phase = (frac * self.phases as f64).round() as usize;
            if phase == self.phases {
                phase = 0;
                base += 1;
                frac = 0.0;
            }
            let _ = frac;
            let coeffs = &self.table[phase * taps..(phase + 1) * taps];
            let start = base as i64 - self.half as i64 + 1;
            let mut acc = 0f32;
            for (k, c) in coeffs.iter().enumerate() {
                let idx = start + k as i64;
                if idx < 0 {
                    continue;
                }
                let rel = (idx as u64).wrapping_sub(self.buf_off) as usize;
                if let Some(v) = self.buf.get(rel) {
                    acc += v * c;
                }
            }
            out.push(acc);
            self.next_out += 1;
        }
        // discard input no longer needed
        let t = self.next_out as f64 * self.step;
        let keep_from = (t.floor() as i64 - self.half as i64).max(0) as u64;
        if keep_from > self.buf_off {
            let n = ((keep_from - self.buf_off) as usize).min(self.buf.len());
            self.buf.drain(..n);
            self.buf_off += n as u64;
        }
    }

    pub fn push(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if self.passthrough {
            out.extend_from_slice(input);
            return;
        }
        self.buf.extend_from_slice(input);
        self.total_in += input.len() as u64;
        let limit = self.buf_off + self.buf.len() as u64;
        self.produce(out, limit);
    }

    pub fn finish(&mut self, out: &mut Vec<f32>) {
        if self.passthrough {
            return;
        }
        let total_out = (self.total_in as f64 / self.step).ceil() as u64;
        self.buf.extend(std::iter::repeat_n(0.0, self.half * 2 + 2));
        let limit = self.buf_off + self.buf.len() as u64;
        self.produce(out, limit);
        out.truncate(out.len() - (self.next_out.saturating_sub(total_out)) as usize);
    }
}

/// Resample a complete mono buffer.
pub fn resample(input: &[f32], in_rate: u32, out_rate: u32) -> Vec<f32> {
    let mut r = Resampler::new(in_rate, out_rate);
    let mut out =
        Vec::with_capacity((input.len() as f64 * out_rate as f64 / in_rate as f64) as usize + 16);
    for chunk in input.chunks(1 << 16) {
        r.push(chunk, &mut out);
    }
    r.finish(&mut out);
    out
}

struct Opened {
    format: Box<dyn symphonia::core::formats::FormatReader>,
    decoder: Box<dyn symphonia::core::codecs::audio::AudioDecoder>,
    track_id: u32,
    num_frames: Option<u64>,
}

enum Input<'a> {
    Path(&'a Path),
    Bytes(Vec<u8>, &'a str),
}

fn open(input: Input<'_>) -> Result<Opened, DecodeError> {
    let (mss, ext) = match input {
        Input::Path(path) => {
            let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
            (
                MediaSourceStream::new(Box::new(file), Default::default()),
                path.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_ascii_lowercase()),
            )
        }
        Input::Bytes(bytes, ext) => (
            MediaSourceStream::new(Box::new(std::io::Cursor::new(bytes)), Default::default()),
            Some(ext.to_string()),
        ),
    };
    let mut hint = Hint::new();
    if let Some(ext) = &ext {
        hint.with_extension(ext);
    }
    let format = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| match e {
            SymError::Unsupported(m) => DecodeError::Unsupported(m.to_string()),
            SymError::IoError(io) if io.kind() == std::io::ErrorKind::UnexpectedEof => {
                DecodeError::Unsupported("unrecognized container".into())
            }
            other => DecodeError::Unsupported(other.to_string()),
        })?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| DecodeError::Unsupported("no audio track".into()))?;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or_else(|| DecodeError::Unsupported("no audio codec parameters".into()))?;
    let decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(|e| DecodeError::Unsupported(e.to_string()))?;
    let track_id = track.id;
    let num_frames = track.num_frames;
    Ok(Opened {
        format,
        decoder,
        track_id,
        num_frames,
    })
}

/// Decode every packet, calling `sink(interleaved_samples, channels, sample_rate)`.
fn decode_with(
    input: Input<'_>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
    sink: &mut dyn FnMut(&[f32], usize, u32),
) -> Result<(), DecodeError> {
    let mut o = open(input)?;
    let mut interleaved: Vec<f32> = Vec::new();
    let mut decoded_any = false;
    let mut last_report = 0f32;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(DecodeError::Cancelled);
        }
        let packet = match o.format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymError::ResetRequired) => break,
            Err(e) => {
                if decoded_any {
                    log::warn!("demux error after partial decode: {e}");
                    break;
                }
                return Err(DecodeError::Unsupported(e.to_string()));
            }
        };
        if packet.track_id != o.track_id {
            continue;
        }
        let pts = packet.pts.get();
        match o.decoder.decode(&packet) {
            Ok(buf) => {
                let spec = buf.spec().clone();
                let ch = spec.channels().count().max(1);
                interleaved.resize(buf.samples_interleaved(), 0.0);
                buf.copy_to_slice_interleaved(&mut interleaved[..]);
                sink(&interleaved, ch, spec.rate());
                decoded_any = true;
            }
            Err(SymError::DecodeError(e)) => {
                log::debug!("skipping undecodable packet: {e}");
            }
            Err(e) => {
                if decoded_any {
                    log::warn!("decoder error after partial decode: {e}");
                    break;
                }
                return Err(DecodeError::Unsupported(e.to_string()));
            }
        }
        if let Some(n) = o.num_frames {
            if n > 0 {
                let p = (pts.max(0) as f64 / n as f64).clamp(0.0, 1.0) as f32;
                if p - last_report > 0.01 {
                    last_report = p;
                    progress(p);
                }
            }
        }
    }
    if !decoded_any {
        return Err(DecodeError::Unsupported("no decodable audio".into()));
    }
    progress(1.0);
    Ok(())
}

fn connect_err(e: anyhow::Error) -> DecodeError {
    if e.to_string() == "cancelled" {
        DecodeError::Cancelled
    } else {
        DecodeError::Other(e)
    }
}

/// Decode any supported input to 16 kHz mono f32: audio/video files (Symphonia), FLV
/// streams, and Adobe Connect recordings (source ZIP or extracted folder).
pub fn decode_to_16k_mono(
    path: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<Vec<f32>, DecodeError> {
    if path.is_dir() {
        if crate::adobe_connect::is_connect_dir(path) {
            return crate::adobe_connect::decode_dir(path, cancel, progress).map_err(connect_err);
        }
        return Err(DecodeError::Unsupported(
            "folders are only supported for extracted Adobe Connect recordings".into(),
        ));
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "zip" => {
            if !crate::adobe_connect::is_connect_zip(path) {
                return Err(DecodeError::Unsupported(
                    "ZIP files are only supported for Adobe Connect recordings".into(),
                ));
            }
            crate::adobe_connect::decode_zip(path, cancel, progress).map_err(connect_err)
        }
        "flv" | "f4a" => match crate::flv::decode_file(path) {
            Ok(Some(pcm)) => {
                progress(1.0);
                Ok(pcm)
            }
            Ok(None) => Err(DecodeError::Unsupported(
                "this FLV file has no audio".into(),
            )),
            Err(e) => Err(DecodeError::Unsupported(format!("{e:#}"))),
        },
        _ => symphonia_to_16k(Input::Path(path), cancel, progress),
    }
}

/// Decode an in-memory file (e.g. an MP3 inside an archive) to 16 kHz mono.
pub fn decode_bytes_to_16k_mono(bytes: Vec<u8>, ext: &str) -> Result<Vec<f32>> {
    symphonia_to_16k(
        Input::Bytes(bytes, ext),
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}

fn symphonia_to_16k(
    input: Input<'_>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<Vec<f32>, DecodeError> {
    let mut out: Vec<f32> = Vec::new();
    let mut resampler: Option<(u32, Resampler)> = None;
    let mut mono: Vec<f32> = Vec::new();
    decode_with(input, cancel, progress, &mut |inter, ch, rate| {
        mono.clear();
        mono.extend(
            inter
                .chunks_exact(ch)
                .map(|f| f.iter().sum::<f32>() / ch as f32),
        );
        let r = match &mut resampler {
            Some((r_rate, r)) if *r_rate == rate => r,
            _ => {
                if let Some((_, mut old)) = resampler.take() {
                    old.finish(&mut out);
                }
                resampler = Some((rate, Resampler::new(rate, TARGET_RATE)));
                &mut resampler.as_mut().unwrap().1
            }
        };
        r.push(&mono, &mut out);
    })?;
    if let Some((_, mut r)) = resampler {
        r.finish(&mut out);
    }
    Ok(out)
}

pub fn write_wav_16k(path: &Path, samples: &[f32]) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: TARGET_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let tmp = path.with_extension("wav.part");
    let mut w = hound::WavWriter::create(&tmp, spec)?;
    {
        let mut i16w = w.get_i16_writer(samples.len() as u32);
        for s in samples {
            i16w.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16);
        }
        i16w.flush()?;
    }
    w.finalize()?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Read a mono WAV (any rate/format hound supports) as f32 at 16 kHz.
pub fn read_wav_16k(path: &Path) -> Result<Vec<f32>> {
    let mut r =
        hound::WavReader::open(path).with_context(|| format!("opening {}", path.display()))?;
    let spec = r.spec();
    let ch = spec.channels.max(1) as usize;
    let raw: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => r.samples::<f32>().collect::<Result<_, _>>()?,
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
            r.samples::<i32>()
                .map(|s| s.map(|v| v as f32 * scale))
                .collect::<Result<_, _>>()?
        }
    };
    let mono: Vec<f32> = if ch == 1 {
        raw
    } else {
        raw.chunks_exact(ch)
            .map(|f| f.iter().sum::<f32>() / ch as f32)
            .collect()
    };
    if spec.sample_rate == TARGET_RATE {
        Ok(mono)
    } else {
        Ok(resample(&mono, spec.sample_rate, TARGET_RATE))
    }
}

/// Peak envelope for the waveform view, normalized so the loudest peak is 1.
pub fn peaks(samples: &[f32], per_second: u32) -> Vec<f32> {
    let win = (TARGET_RATE / per_second).max(1) as usize;
    let mut p: Vec<f32> = samples
        .chunks(win)
        .map(|c| c.iter().fold(0f32, |m, v| m.max(v.abs())))
        .collect();
    let max = p.iter().copied().fold(0f32, f32::max);
    if max > 0.0 {
        for v in &mut p {
            *v /= max;
        }
    }
    p
}

pub fn write_peaks(path: &Path, peaks: &[f32]) -> Result<()> {
    let bytes: Vec<u8> = peaks
        .iter()
        .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect();
    std::fs::write(path, bytes)?;
    Ok(())
}

pub fn read_peaks(path: &Path) -> Result<Vec<f32>> {
    let bytes = std::fs::read(path)?;
    Ok(bytes.into_iter().map(|b| b as f32 / 255.0).collect())
}

pub fn duration_secs(samples_16k: usize) -> f64 {
    samples_16k as f64 / TARGET_RATE as f64
}

/// Guard against absurdly short inputs before transcription.
pub fn ensure_audible(samples: &[f32]) -> Result<()> {
    if samples.len() < (TARGET_RATE as usize) / 10 {
        bail!("audio is shorter than 0.1 seconds");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, freq: f32, secs: f32) -> Vec<f32> {
        (0..(rate as f32 * secs) as usize)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin() * 0.5)
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn resample_preserves_length_and_passband() {
        let x = sine(48_000, 440.0, 2.0);
        let y = resample(&x, 48_000, 16_000);
        assert_eq!(y.len(), 32_000);
        // 440 Hz tone keeps its energy (0.5 amplitude -> rms 0.3535)
        let r = rms(&y[1000..31_000]);
        assert!((r - 0.3535).abs() < 0.01, "rms {r}");
    }

    #[test]
    fn resample_attenuates_above_nyquist() {
        let x = sine(44_100, 12_000.0, 1.0);
        let y = resample(&x, 44_100, 16_000);
        assert!(rms(&y[500..15_000]) < 0.01);
    }

    #[test]
    fn resample_streaming_matches_oneshot() {
        let x = sine(22_050, 1000.0, 1.3);
        let a = resample(&x, 22_050, 16_000);
        let mut r = Resampler::new(22_050, 16_000);
        let mut b = Vec::new();
        for c in x.chunks(777) {
            r.push(c, &mut b);
        }
        r.finish(&mut b);
        assert_eq!(a.len(), b.len());
        for (p, q) in a.iter().zip(&b) {
            assert!((p - q).abs() < 1e-6);
        }
    }

    #[test]
    fn upsample_8k() {
        let x = sine(8_000, 300.0, 1.0);
        let y = resample(&x, 8_000, 16_000);
        assert_eq!(y.len(), 16_000);
        assert!((rms(&y[500..15_500]) - 0.3535).abs() < 0.01);
    }

    #[test]
    fn wav_roundtrip_and_peaks() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.wav");
        let x = sine(16_000, 200.0, 1.0);
        write_wav_16k(&p, &x).unwrap();
        let y = read_wav_16k(&p).unwrap();
        assert_eq!(y.len(), x.len());
        let pk = peaks(&y, PEAKS_PER_SECOND);
        assert_eq!(pk.len(), 50);
        assert!((pk.iter().copied().fold(0f32, f32::max) - 1.0).abs() < 1e-6);

        // the native decoder reads the WAV we wrote
        let cancel = AtomicBool::new(false);
        let z = decode_to_16k_mono(&p, &cancel, &mut |_| {}).unwrap();
        assert_eq!(z.len(), x.len());
    }

    #[test]
    fn garbage_is_unsupported() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.opus");
        std::fs::write(&p, b"OggS\0garbage-not-really-ogg").unwrap();
        let cancel = AtomicBool::new(false);
        match decode_to_16k_mono(&p, &cancel, &mut |_| {}) {
            Err(DecodeError::Unsupported(_)) => {}
            other => panic!("expected unsupported, got {:?}", other.map(|v| v.len())),
        }
    }
}
