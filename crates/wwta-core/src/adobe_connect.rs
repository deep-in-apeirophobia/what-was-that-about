//! Adobe Connect recordings.
//!
//! Connect's downloadable "source" archive (`<recording-url>/output/<name>.zip?download=zip`)
//! holds one FLV per stream plus XML metadata. Voice is in `cameraVoip_*.flv` (Speex or
//! Nellymoser, sometimes with webcam video), phone bridges add `*.mp3`, and `mainstream.xml`
//! records when each stream was added to the meeting. This module decodes every voice stream
//! and mixes them onto one 16 kHz timeline.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{bail, Context, Result};
use quick_xml::events::Event;

use crate::audio::TARGET_RATE;

/// Is this a stream file that may carry voice?
fn is_voice_candidate(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    let base = n.rsplit(['/', '\\']).next().unwrap_or(&n);
    (base.ends_with(".flv") && !base.starts_with("screenshare")) || base.ends_with(".mp3")
}

fn base_name(name: &str) -> String {
    name.rsplit(['/', '\\']).next().unwrap_or(name).to_string()
}

/// Start time (ms, meeting clock) of each stream, from mainstream.xml `playEvent` messages
/// (`streamAdded` with a `streamName` such as `/cameraVoip_1_11`). Keys are file stems.
pub fn stream_starts(xml: &str) -> HashMap<String, u64> {
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut starts: HashMap<String, u64> = HashMap::new();
    let mut stack: Vec<String> = Vec::new();
    let mut time: Option<u64> = None;
    let mut method = String::new();
    let mut strings: Vec<String> = Vec::new();
    let mut stream: Option<String> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = e.local_name().into_inner().to_string();
                if name == "Message" {
                    time = e
                        .attributes()
                        .flatten()
                        .find(|a| a.key.as_ref() == "time")
                        .and_then(|a| a.value.trim().parse::<f64>().ok())
                        .map(|t| t as u64);
                    method.clear();
                    strings.clear();
                    stream = None;
                }
                stack.push(name);
            }
            Ok(Event::Text(t)) => {
                let text = AsRef::<str>::as_ref(&t).trim().to_string();
                if text.is_empty() {
                    continue;
                }
                match stack.last().map(String::as_str) {
                    Some("Method") => method = text,
                    Some("String") => strings.push(text),
                    Some("streamName") => stream = Some(text),
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                let name = e.local_name().into_inner().to_string();
                stack.pop();
                if name == "Message"
                    && method == "playEvent"
                    && strings.iter().any(|s| s == "streamAdded")
                {
                    if let (Some(t), Some(s)) = (time, stream.as_ref()) {
                        let stem = s.trim_start_matches('/').to_string();
                        starts
                            .entry(stem)
                            .and_modify(|v| *v = (*v).min(t))
                            .or_insert(t);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                log::warn!(
                    "mainstream.xml parse error at {}: {e}",
                    reader.buffer_position()
                );
                break;
            }
            _ => {}
        }
    }
    starts
}

/// Numeric parts of a name, for ordering "cameraVoip_2_10" after "cameraVoip_2_9".
fn natural_key(name: &str) -> Vec<u64> {
    name.split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect()
}

struct Track {
    name: String,
    pcm: Vec<f32>,
    start_ms: Option<u64>,
}

fn decode_entry(name: &str, reader: impl Read) -> Result<Option<Vec<f32>>> {
    if name.to_ascii_lowercase().ends_with(".mp3") {
        let mut bytes = Vec::new();
        let mut r = reader;
        r.read_to_end(&mut bytes)?;
        return Ok(Some(crate::audio::decode_bytes_to_16k_mono(bytes, "mp3")?));
    }
    crate::flv::decode_tags_to_16k(crate::flv::read_audio_tags(
        std::io::BufReader::with_capacity(1 << 20, reader),
    )?)
}

/// Mix decoded tracks onto one timeline. Tracks without a known start are appended after the
/// previous track (sorted by their stream numbers).
fn mix(mut tracks: Vec<Track>) -> Result<Vec<f32>> {
    tracks.retain(|t| t.pcm.iter().any(|s| s.abs() > 1e-4));
    if tracks.is_empty() {
        bail!("this Adobe Connect recording has no voice audio");
    }
    tracks.sort_by(|a, b| match (a.start_ms, b.start_ms) {
        (Some(x), Some(y)) => x.cmp(&y),
        _ => natural_key(&a.name).cmp(&natural_key(&b.name)),
    });
    let zero = tracks.iter().filter_map(|t| t.start_ms).min().unwrap_or(0);
    let sr = TARGET_RATE as u64;
    let mut placed: Vec<(usize, &Vec<f32>)> = Vec::new();
    let mut cursor = 0usize;
    for t in &tracks {
        let at = match t.start_ms {
            Some(ms) => ((ms - zero) * sr / 1000) as usize,
            None => cursor,
        };
        cursor = cursor.max(at + t.pcm.len());
        placed.push((at, &t.pcm));
    }
    let total = placed.iter().map(|(at, p)| at + p.len()).max().unwrap_or(0);
    let mut out = vec![0f32; total];
    for (at, pcm) in placed {
        for (o, s) in out[at..at + pcm.len()].iter_mut().zip(pcm) {
            *o += *s;
        }
    }
    let peak = out.iter().fold(0f32, |m, s| m.max(s.abs()));
    if peak > 0.99 {
        let g = 0.98 / peak;
        out.iter_mut().for_each(|s| *s *= g);
    }
    Ok(out)
}

/// Does this ZIP look like an Adobe Connect source archive?
pub fn is_connect_zip(path: &Path) -> bool {
    let Ok(f) = std::fs::File::open(path) else {
        return false;
    };
    let Ok(z) = zip::ZipArchive::new(f) else {
        return false;
    };
    let found = z.file_names().flatten().any(|n| {
        let b = base_name(&n).to_ascii_lowercase();
        b == "mainstream.xml" || (b.starts_with("cameravoip") && b.ends_with(".flv"))
    });
    found
}

/// Decode an Adobe Connect source ZIP to 16 kHz mono.
pub fn decode_zip(
    path: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<Vec<f32>> {
    let f = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut z =
        zip::ZipArchive::new(std::io::BufReader::new(f)).context("reading the ZIP archive")?;
    let mut starts = HashMap::new();
    let mut candidates = Vec::new();
    for i in 0..z.len() {
        let entry = z.by_index(i)?;
        let name = entry.name()?.to_string();
        if base_name(&name).eq_ignore_ascii_case("mainstream.xml") {
            let mut xml = String::new();
            let mut e = entry;
            e.read_to_string(&mut xml).ok();
            starts = stream_starts(&xml);
        } else if is_voice_candidate(&name) {
            candidates.push((i, entry.size()));
        }
    }
    if candidates.is_empty() {
        bail!("no voice streams (cameraVoip_*.flv) found in this archive");
    }
    let total: u64 = candidates.iter().map(|c| c.1).sum::<u64>().max(1);
    let mut done = 0u64;
    let mut tracks = Vec::new();
    for (i, size) in candidates {
        if cancel.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        let entry = z.by_index(i)?;
        let name = base_name(&entry.name()?);
        match decode_entry(&name, entry) {
            Ok(Some(pcm)) => {
                let stem = name
                    .rsplit_once('.')
                    .map(|p| p.0)
                    .unwrap_or(&name)
                    .to_string();
                let start_ms = starts.get(&stem).copied();
                tracks.push(Track {
                    name,
                    pcm,
                    start_ms,
                });
            }
            Ok(None) => {}
            Err(e) => log::warn!("skipping {name}: {e:#}"),
        }
        done += size;
        progress(done as f32 / total as f32);
    }
    mix(tracks)
}

/// Decode an extracted Adobe Connect folder to 16 kHz mono.
pub fn decode_dir(
    dir: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<Vec<f32>> {
    let starts = std::fs::read_to_string(dir.join("mainstream.xml"))
        .map(|x| stream_starts(&x))
        .unwrap_or_default();
    let files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && is_voice_candidate(&p.to_string_lossy()))
        .collect();
    if files.is_empty() {
        bail!("no voice streams (cameraVoip_*.flv) found in this folder");
    }
    let n = files.len();
    let mut tracks = Vec::new();
    for (k, p) in files.into_iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        let name = base_name(&p.to_string_lossy());
        let f = std::fs::File::open(&p)?;
        match decode_entry(&name, f) {
            Ok(Some(pcm)) => {
                let stem = name
                    .rsplit_once('.')
                    .map(|x| x.0)
                    .unwrap_or(&name)
                    .to_string();
                tracks.push(Track {
                    start_ms: starts.get(&stem).copied(),
                    name,
                    pcm,
                });
            }
            Ok(None) => {}
            Err(e) => log::warn!("skipping {name}: {e:#}"),
        }
        progress((k + 1) as f32 / n as f32);
    }
    mix(tracks)
}

pub fn is_connect_dir(path: &Path) -> bool {
    path.is_dir()
        && (path.join("mainstream.xml").is_file()
            || std::fs::read_dir(path)
                .map(|d| {
                    d.flatten().any(|e| {
                        let n = e.file_name().to_string_lossy().to_ascii_lowercase();
                        n.starts_with("cameravoip") && n.ends_with(".flv")
                    })
                })
                .unwrap_or(false))
}

// ------------------------------------------------------------------ recording links

/// Parsed Adobe Connect recording link, e.g.
/// `https://connect.example.edu/p1a2b3c4d5e/?session=breezxyz` (the session token is
/// present when copied from a logged-in browser; public recordings work without it).
#[derive(Debug, Clone, PartialEq)]
pub struct RecordingLink {
    pub origin: String,
    pub id: String,
    pub session: Option<String>,
}

pub fn parse_link(url: &str) -> Result<RecordingLink> {
    let u = reqwest::Url::parse(url.trim()).context("not a valid link")?;
    if !matches!(u.scheme(), "http" | "https") {
        bail!("the link must start with http:// or https://");
    }
    let id = u
        .path_segments()
        .and_then(|mut s| s.find(|p| !p.is_empty()))
        .map(String::from)
        .context("the link has no recording id (expected https://server/<id>/…)")?;
    let session = u
        .query_pairs()
        .find(|(k, _)| k == "session")
        .map(|(_, v)| v.into_owned())
        .filter(|v| !v.is_empty());
    let origin = format!(
        "{}://{}{}",
        u.scheme(),
        u.host_str().unwrap_or_default(),
        u.port().map(|p| format!(":{p}")).unwrap_or_default()
    );
    Ok(RecordingLink {
        origin,
        id,
        session,
    })
}

impl RecordingLink {
    /// Candidate source-archive URLs (Connect accepts any file name under /output/).
    pub fn zip_urls(&self) -> Vec<String> {
        let session = self
            .session
            .as_ref()
            .map(|s| format!("&session={s}"))
            .unwrap_or_default();
        [format!("{}.zip", self.id), "download.zip".into()]
            .iter()
            .map(|name| {
                format!(
                    "{}/{}/output/{name}?download=zip{session}",
                    self.origin, self.id
                )
            })
            .collect()
    }
}

/// Download a recording's source archive to `dest`. `progress(done, total)`.
pub async fn download(
    link: &RecordingLink,
    dest: &Path,
    cancel: &AtomicBool,
    progress: &mut (dyn FnMut(u64, u64) + Send),
) -> Result<()> {
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;

    let client = crate::models::http_client()?;
    let mut last_err = None;
    for url in link.zip_urls() {
        let mut req = client.get(&url);
        if let Some(s) = &link.session {
            req = req.header(reqwest::header::COOKIE, format!("BREEZESESSION={s}"));
        }
        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                last_err = Some(anyhow::anyhow!("{e}"));
                continue;
            }
        };
        let ctype = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !resp.status().is_success() || ctype.contains("text/html") {
            last_err = Some(anyhow::anyhow!(
                "the server answered {} ({})",
                resp.status(),
                if ctype.contains("html") {
                    "a web page, probably a login page"
                } else {
                    ctype.as_str()
                }
            ));
            continue;
        }
        let total = resp.content_length().unwrap_or(0);
        let tmp = dest.with_extension("zip.part");
        let mut out = tokio::fs::File::create(&tmp).await?;
        let mut stream = resp.bytes_stream();
        let mut done = 0u64;
        while let Some(chunk) = stream.next().await {
            if cancel.load(Ordering::Relaxed) {
                drop(out);
                let _ = tokio::fs::remove_file(&tmp).await;
                bail!("cancelled");
            }
            let chunk = chunk?;
            out.write_all(&chunk).await?;
            done += chunk.len() as u64;
            progress(done, total);
        }
        out.flush().await?;
        drop(out);
        if !is_connect_zip(&tmp) {
            let _ = tokio::fs::remove_file(&tmp).await;
            last_err = Some(anyhow::anyhow!(
                "the downloaded file isn't an Adobe Connect recording archive"
            ));
            continue;
        }
        tokio::fs::rename(&tmp, dest).await?;
        return Ok(());
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("download failed")).context(
        "couldn't download the recording. If it needs a login, open it in your browser and copy the \
         link including \"?session=…\", or download the ZIP yourself (add /output/download.zip?download=zip \
         to the recording link) and import that file",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<root>
  <Message time="1000" type="0"><Method>setTitle</Method><String>Lecture</String></Message>
  <Message time="5000"><Method>playEvent</Method><Array><Object><streamName>/screenshare_1_2</streamName></Object></Array><String>streamAdded</String></Message>
  <Message time="7000"><Method>playEvent</Method><Array><Object><streamName>/cameraVoip_0_3</streamName><streamType>cameraVoip</streamType></Object></Array><String>streamAdded</String></Message>
  <Message time="9500"><Method>playEvent</Method><Array><Object><streamName>/cameraVoip_0_3</streamName></Object></Array><String>streamRemoved</String></Message>
  <Message time="12000"><Method>playEvent</Method><Array><Object><streamName>/cameraVoip_4_7</streamName></Object></Array><String>streamAdded</String></Message>
</root>"#;

    #[test]
    fn parses_stream_starts() {
        let s = stream_starts(XML);
        assert_eq!(s.get("cameraVoip_0_3"), Some(&7000));
        assert_eq!(s.get("cameraVoip_4_7"), Some(&12000));
        assert_eq!(s.get("screenshare_1_2"), Some(&5000));
        assert_eq!(s.len(), 3);
    }

    #[test]
    fn mixes_on_the_meeting_timeline() {
        let one_sec = vec![0.5f32; TARGET_RATE as usize];
        let out = mix(vec![
            Track {
                name: "cameraVoip_4_7.flv".into(),
                pcm: one_sec.clone(),
                start_ms: Some(12000),
            },
            Track {
                name: "cameraVoip_0_3.flv".into(),
                pcm: one_sec.clone(),
                start_ms: Some(7000),
            },
        ])
        .unwrap();
        // zero = 7 s; second track starts 5 s later and lasts 1 s
        assert_eq!(out.len(), 6 * TARGET_RATE as usize);
        assert!(out[TARGET_RATE as usize * 3].abs() < 1e-6);
        assert!((out[TARGET_RATE as usize * 5 + 10] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn appends_tracks_without_timing_in_numeric_order() {
        let a = vec![0.1f32; 100];
        let b = vec![0.2f32; 100];
        let out = mix(vec![
            Track {
                name: "cameraVoip_2_10.flv".into(),
                pcm: b,
                start_ms: None,
            },
            Track {
                name: "cameraVoip_2_9.flv".into(),
                pcm: a,
                start_ms: None,
            },
        ])
        .unwrap();
        assert_eq!(out.len(), 200);
        assert!((out[0] - 0.1).abs() < 1e-6 && (out[150] - 0.2).abs() < 1e-6);
    }

    #[test]
    fn parses_recording_links() {
        let l = parse_link("https://vc.example.ac.ir/p8x2k9z1q3/?session=breezabc123&proto=true")
            .unwrap();
        assert_eq!(l.origin, "https://vc.example.ac.ir");
        assert_eq!(l.id, "p8x2k9z1q3");
        assert_eq!(l.session.as_deref(), Some("breezabc123"));
        assert_eq!(
            l.zip_urls()[0],
            "https://vc.example.ac.ir/p8x2k9z1q3/output/p8x2k9z1q3.zip?download=zip&session=breezabc123"
        );
        let public = parse_link("http://connect.example.com:8080/rec42").unwrap();
        assert_eq!(public.origin, "http://connect.example.com:8080");
        assert!(public.session.is_none());
        assert!(parse_link("ftp://x/y").is_err());
        assert!(parse_link("https://example.com/").is_err());
    }
}
