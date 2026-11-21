//! Minimal FLV demuxer + audio decoding (Speex, Nellymoser, AAC, MP3, PCM) to 16 kHz mono.
//! Flash/Adobe Connect recordings store their voice streams this way, and neither Symphonia
//! nor web views can read them.

use std::io::{Cursor, Read};

use anyhow::{anyhow, bail, Context, Result};

use crate::audio::{resample, TARGET_RATE};

/// FLV SoundFormat values (FLV spec, section E.4.2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundFormat {
    PcmPlatform,
    Adpcm,
    Mp3,
    PcmLe,
    Nellymoser16k,
    Nellymoser8k,
    Nellymoser,
    G711A,
    G711U,
    Aac,
    Speex,
    Mp3_8k,
    Other(u8),
}

impl SoundFormat {
    fn from_bits(b: u8) -> Self {
        match b {
            0 => Self::PcmPlatform,
            1 => Self::Adpcm,
            2 => Self::Mp3,
            3 => Self::PcmLe,
            4 => Self::Nellymoser16k,
            5 => Self::Nellymoser8k,
            6 => Self::Nellymoser,
            7 => Self::G711A,
            8 => Self::G711U,
            10 => Self::Aac,
            11 => Self::Speex,
            14 => Self::Mp3_8k,
            o => Self::Other(o),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AudioTag {
    /// Milliseconds from the start of the file.
    pub time_ms: u32,
    pub format: SoundFormat,
    /// Nominal rate from the tag header (5.5/11/22/44 kHz).
    pub rate: u32,
    pub sixteen_bit: bool,
    pub stereo: bool,
    /// Codec payload (for AAC this excludes the AACPacketType byte).
    pub data: Vec<u8>,
    /// AAC only: true for the AudioSpecificConfig sequence header.
    pub aac_config: bool,
}

/// Read the audio tags of an FLV stream. Video and script tags are skipped without being
/// kept in memory (webcam/screen-share FLVs can be hundreds of MB).
pub fn read_audio_tags<R: Read>(mut r: R) -> Result<Vec<AudioTag>> {
    let mut header = [0u8; 9];
    r.read_exact(&mut header).context("reading FLV header")?;
    if &header[..3] != b"FLV" {
        bail!("not an FLV file");
    }
    let header_len = u32::from_be_bytes([header[5], header[6], header[7], header[8]]) as u64;
    // rest of the header (if any) + PreviousTagSize0
    std::io::copy(
        &mut (&mut r).take(header_len.saturating_sub(9) + 4),
        &mut std::io::sink(),
    )?;
    let mut out = Vec::new();
    let mut th = [0u8; 11];
    loop {
        match r.read_exact(&mut th) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.into()),
        }
        let tag_type = th[0] & 0x1f;
        let size = u32::from_be_bytes([0, th[1], th[2], th[3]]) as u64;
        let ts = u32::from_be_bytes([th[7], th[4], th[5], th[6]]);
        if tag_type != 8 || size == 0 {
            let skipped = std::io::copy(&mut (&mut r).take(size + 4), &mut std::io::sink())?;
            if skipped < size + 4 {
                break; // truncated
            }
            continue;
        }
        let mut data = vec![0u8; size as usize];
        if r.read_exact(&mut data).is_err() {
            break; // truncated final tag
        }
        let mut prev = [0u8; 4];
        let _ = r.read_exact(&mut prev);
        let flags = data[0];
        let format = SoundFormat::from_bits(flags >> 4);
        let rate = [5512, 11025, 22050, 44100][((flags >> 2) & 3) as usize];
        let mut payload = &data[1..];
        let mut aac_config = false;
        if format == SoundFormat::Aac {
            if payload.is_empty() {
                continue;
            }
            aac_config = payload[0] == 0;
            payload = &payload[1..];
        }
        out.push(AudioTag {
            time_ms: ts,
            format,
            rate,
            sixteen_bit: flags & 2 != 0,
            stereo: flags & 1 != 0,
            data: payload.to_vec(),
            aac_config,
        });
    }
    Ok(out)
}

pub fn audio_tags(bytes: &[u8]) -> Result<Vec<AudioTag>> {
    read_audio_tags(bytes)
}

pub fn decode_file(path: &std::path::Path) -> Result<Option<Vec<f32>>> {
    let f = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    decode_tags_to_16k(read_audio_tags(std::io::BufReader::with_capacity(
        1 << 20,
        f,
    ))?)
}

/// Places decoded chunks on a 16 kHz timeline using their tag timestamps, so gaps inside a
/// stream (muted mic, dropped packets) stay silent instead of shifting later speech.
struct Timeline {
    out: Vec<f32>,
}

impl Timeline {
    fn put(&mut self, time_ms: u32, pcm16k: &[f32]) {
        let at = (time_ms as u64 * TARGET_RATE as u64 / 1000) as usize;
        // Small jitter is normal; only open a gap for > 120 ms of missing audio.
        if at > self.out.len() + (TARGET_RATE as usize * 12 / 100) {
            self.out.resize(at, 0.0);
        }
        self.out.extend_from_slice(pcm16k);
    }
}

fn to_16k(samples: Vec<f32>, rate: u32) -> Vec<f32> {
    if rate == TARGET_RATE {
        samples
    } else {
        resample(&samples, rate, TARGET_RATE)
    }
}

fn downmix(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks_exact(channels)
        .map(|f| f.iter().sum::<f32>() / channels as f32)
        .collect()
}

/// Decode all audio in an FLV file to 16 kHz mono. Returns None if it has no audio track.
pub fn decode_to_16k(bytes: &[u8]) -> Result<Option<Vec<f32>>> {
    decode_tags_to_16k(audio_tags(bytes)?)
}

pub fn decode_tags_to_16k(tags: Vec<AudioTag>) -> Result<Option<Vec<f32>>> {
    let Some(first) = tags.iter().find(|t| !t.aac_config) else {
        return Ok(None);
    };
    let t0 = first.time_ms;
    let format = first.format;
    let mut tl = Timeline { out: Vec::new() };
    let rel = |t: &AudioTag| t.time_ms.saturating_sub(t0);

    match format {
        SoundFormat::Speex => {
            // Flash always encodes Speex wideband at 16 kHz, mono.
            let mut dec = oxideav_speex::SpeexStreamDecoder::for_sample_rate(16_000)
                .map_err(|e| anyhow!("speex decoder: {e:?}"))?;
            for t in tags.iter().filter(|t| t.format == SoundFormat::Speex) {
                match dec.decode_packet_pcm_i16(&t.data) {
                    Ok(pcm) => {
                        let f: Vec<f32> = pcm.iter().map(|s| *s as f32 / 32768.0).collect();
                        tl.put(rel(t), &f);
                    }
                    Err(e) => log::debug!("skipping bad speex packet: {e:?}"),
                }
            }
        }
        SoundFormat::Nellymoser16k | SoundFormat::Nellymoser8k | SoundFormat::Nellymoser => {
            let rate = match format {
                SoundFormat::Nellymoser16k => 16_000,
                SoundFormat::Nellymoser8k => 8_000,
                _ => first.rate,
            };
            // One decoder per contiguous run of tags: the codec overlaps blocks (MDCT state), and
            // each new decoder first yields one empty 256-sample frame, which is dropped.
            let tags_n: Vec<&AudioTag> = tags.iter().filter(|t| t.format == format).collect();
            let mut run: Vec<&AudioTag> = Vec::new();
            let flush = |run: &mut Vec<&AudioTag>, tl: &mut Timeline| {
                if run.is_empty() {
                    return;
                }
                let bytes: Vec<u8> = run.iter().flat_map(|t| t.data.iter().copied()).collect();
                // the decoder emits 16-bit-range floats
                let pcm: Vec<f32> = nellymoser_rs::Decoder::new(Cursor::new(bytes), rate)
                    .skip(256)
                    .map(|s| s / 32768.0)
                    .collect();
                tl.put(rel(run[0]), &to_16k(pcm, rate));
                run.clear();
            };
            let mut expected_ms = 0f64;
            for t in tags_n {
                let blocks = t.data.len() / 64;
                if let Some(last) = run.last() {
                    let gap = rel(t) as f64 - expected_ms;
                    if gap > 200.0 || rel(t) < rel(last) {
                        flush(&mut run, &mut tl);
                    }
                }
                if run.is_empty() {
                    expected_ms = rel(t) as f64;
                }
                expected_ms += blocks as f64 * 256.0 * 1000.0 / rate as f64;
                run.push(t);
            }
            flush(&mut run, &mut tl);
        }
        SoundFormat::PcmLe | SoundFormat::PcmPlatform => {
            let ch = if first.stereo { 2 } else { 1 };
            for t in tags.iter().filter(|t| t.format == format) {
                let samples: Vec<f32> = if t.sixteen_bit {
                    t.data
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|c| i16::from_le_bytes(*c) as f32 / 32768.0)
                        .collect()
                } else {
                    t.data.iter().map(|b| (*b as f32 - 128.0) / 128.0).collect()
                };
                tl.put(rel(t), &to_16k(downmix(&samples, ch), t.rate));
            }
        }
        SoundFormat::Aac => decode_aac(&tags, t0, &mut tl)?,
        SoundFormat::Mp3 | SoundFormat::Mp3_8k => {
            // MP3 tags carry whole frames; decode the concatenation as one MP3 stream.
            let mut mp3 = Vec::new();
            for t in tags.iter().filter(|t| t.format == format) {
                mp3.extend_from_slice(&t.data);
            }
            let pcm = crate::audio::decode_bytes_to_16k_mono(mp3, "mp3")?;
            tl.put(0, &pcm);
        }
        other => bail!("unsupported FLV audio codec {other:?}"),
    }
    Ok(Some(tl.out))
}

fn decode_aac(tags: &[AudioTag], t0: u32, tl: &mut Timeline) -> Result<()> {
    use symphonia::core::codecs::audio::well_known::CODEC_ID_AAC;
    use symphonia::core::codecs::audio::{AudioCodecParameters, AudioDecoderOptions};
    use symphonia::core::packet::Packet;
    use symphonia::core::units::{Duration, Timestamp};

    let config = tags
        .iter()
        .find(|t| t.format == SoundFormat::Aac && t.aac_config)
        .context("AAC stream without a sequence header")?;
    // AudioSpecificConfig: 5 bits object type, 4 bits frequency index, 4 bits channels.
    let asc = &config.data;
    let rate = if asc.len() >= 2 {
        let idx = ((asc[0] & 7) << 1) | (asc[1] >> 7);
        [
            96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
        ]
        .get(idx as usize)
        .copied()
        .unwrap_or(44100)
    } else {
        44100
    };
    let mut params = AudioCodecParameters::new();
    params
        .for_codec(CODEC_ID_AAC)
        .with_sample_rate(rate)
        .with_extra_data(asc.clone().into_boxed_slice());
    let mut dec = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .map_err(|e| anyhow!("AAC decoder: {e}"))?;
    let mut buf: Vec<f32> = Vec::new();
    for t in tags
        .iter()
        .filter(|t| t.format == SoundFormat::Aac && !t.aac_config)
    {
        let pkt = Packet::new(
            0,
            Timestamp::new(t.time_ms as i64),
            Duration::new(0),
            t.data.clone(),
        );
        match dec.decode(&pkt) {
            Ok(audio) => {
                let ch = audio.spec().channels().count().max(1);
                let r = audio.spec().rate();
                buf.resize(audio.samples_interleaved(), 0.0);
                audio.copy_to_slice_interleaved(&mut buf[..]);
                tl.put(t.time_ms.saturating_sub(t0), &to_16k(downmix(&buf, ch), r));
            }
            Err(e) => log::debug!("skipping bad AAC frame: {e}"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flv_with_tags(tags: &[(u8, u32, Vec<u8>)]) -> Vec<u8> {
        let mut b = b"FLV\x01\x04\x00\x00\x00\x09".to_vec();
        b.extend_from_slice(&0u32.to_be_bytes());
        for (ty, ts, data) in tags {
            b.push(*ty);
            let n = data.len() as u32;
            b.extend_from_slice(&n.to_be_bytes()[1..]);
            b.extend_from_slice(&ts.to_be_bytes()[1..]);
            b.push((ts >> 24) as u8);
            b.extend_from_slice(&[0, 0, 0]);
            b.extend_from_slice(data);
            b.extend_from_slice(&(n + 11).to_be_bytes());
        }
        b
    }

    #[test]
    fn parses_audio_tags_and_skips_video() {
        // PCM LE, 44 kHz flag, 16-bit, mono: format 3 => 0x3E
        let pcm: Vec<u8> = std::iter::repeat_n([0x00u8, 0x40], 100).flatten().collect();
        let mut a = vec![0x3E];
        a.extend_from_slice(&pcm);
        let bytes = flv_with_tags(&[(9, 0, vec![0x17, 0, 0]), (8, 0, a.clone()), (8, 1000, a)]);
        let tags = audio_tags(&bytes).unwrap();
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0].format, SoundFormat::PcmLe);
        assert_eq!(tags[0].rate, 44100);
        assert!(tags[0].sixteen_bit);
        assert_eq!(tags[1].time_ms, 1000);
        // second tag lands at 1 s on the 16 kHz timeline (gap filled with silence)
        let pcm16 = decode_to_16k(&bytes).unwrap().unwrap();
        assert!(pcm16.len() >= 16_000);
        assert!(pcm16[8_000].abs() < 1e-6);
    }

    #[test]
    fn rejects_non_flv() {
        assert!(audio_tags(b"RIFF....").is_err());
    }
}
