//! OpenRouter client (chat completions with streaming, embeddings, model list) and the
//! prompts behind analysis, Q&A, interactive docs, transcript condensing and chat compaction.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine as _;
use futures_util::StreamExt;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::context::estimate_tokens;
use crate::text::fmt_time;
use crate::types::{Analysis, Chapter, LlmModelInfo, Segment, Tag};

pub const BASE_URL: &str = "https://openrouter.ai/api/v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Msg {
    pub role: String,
    pub content: Content,
}

/// Message content: plain text, or parts mixing text with images and audio.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Content {
    Text(String),
    Parts(Vec<Part>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Part {
    Text { text: String },
    ImageUrl { image_url: ImageUrl },
    InputAudio { input_audio: InputAudio },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageUrl {
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputAudio {
    /// Base64 audio bytes.
    pub data: String,
    pub format: String,
}

/// Rough prompt cost of one image; providers bill 500–1,600 tokens for typical sizes.
pub const IMAGE_TOKENS: usize = 1_200;

impl Part {
    pub fn text(s: impl Into<String>) -> Self {
        Part::Text { text: s.into() }
    }

    pub fn image(mime: &str, bytes: &[u8]) -> Self {
        let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
        Part::ImageUrl {
            image_url: ImageUrl {
                url: format!("data:{mime};base64,{b64}"),
            },
        }
    }

    pub fn wav(bytes: &[u8]) -> Self {
        Part::InputAudio {
            input_audio: InputAudio {
                data: base64::engine::general_purpose::STANDARD.encode(bytes),
                format: "wav".into(),
            },
        }
    }

    pub fn tokens(&self) -> usize {
        match self {
            Part::Text { text } => estimate_tokens(text),
            Part::ImageUrl { .. } => IMAGE_TOKENS,
            // 16 kHz 16-bit WAV is 32 kB/s and models bill ~32 tokens per second of audio
            Part::InputAudio { input_audio } => input_audio.data.len() * 3 / 4 / 1_000 + 16,
        }
    }
}

impl Content {
    pub fn tokens(&self) -> usize {
        match self {
            Content::Text(t) => estimate_tokens(t),
            Content::Parts(p) => p.iter().map(Part::tokens).sum(),
        }
    }
}

impl Msg {
    pub fn system(c: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: Content::Text(c.into()),
        }
    }
    pub fn user(c: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: Content::Text(c.into()),
        }
    }
    pub fn assistant(c: impl Into<String>) -> Self {
        Self {
            role: "assistant".into(),
            content: Content::Text(c.into()),
        }
    }
    /// A user message with images/audio; a single text part collapses to plain text.
    pub fn user_parts(mut parts: Vec<Part>) -> Self {
        let content = match parts.as_slice() {
            [Part::Text { .. }] => match parts.remove(0) {
                Part::Text { text } => Content::Text(text),
                _ => unreachable!(),
            },
            _ => Content::Parts(parts),
        };
        Self {
            role: "user".into(),
            content,
        }
    }
    pub fn tokens(&self) -> usize {
        self.content.tokens() + 4
    }
}

/// Token usage and cost reported by OpenRouter for one request.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    /// USD, when OpenRouter reports it.
    pub cost: Option<f64>,
}

impl Usage {
    fn from_json(v: &Value) -> Option<Self> {
        let u = v.get("usage")?;
        if !u.is_object() {
            return None;
        }
        Some(Self {
            prompt_tokens: u["prompt_tokens"].as_u64().unwrap_or(0),
            completion_tokens: u["completion_tokens"].as_u64().unwrap_or(0),
            cost: u["cost"].as_f64(),
        })
    }
}

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    key: String,
    base: String,
}

impl Client {
    pub fn new(key: &str) -> Result<Self> {
        if key.trim().is_empty() {
            bail!("Add your OpenRouter API key in Settings to use AI features");
        }
        crate::ensure_crypto_provider();
        let http = reqwest::Client::builder()
            .user_agent(concat!("what-was-that-about/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(20))
            .read_timeout(Duration::from_secs(180))
            .build()?;
        Ok(Self {
            http,
            key: key.trim().to_string(),
            base: std::env::var("WWTA_OPENROUTER_BASE").unwrap_or_else(|_| BASE_URL.into()),
        })
    }

    fn req(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{}{}", self.base, path))
            .bearer_auth(&self.key)
            .header(
                "HTTP-Referer",
                "https://github.com/deep-in-apeirophobia/what-was-that-about",
            )
            .header("X-Title", "What Was That About")
    }

    async fn check(resp: reqwest::Response) -> Result<reqwest::Response> {
        if resp.status().is_success() {
            return Ok(resp);
        }
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        let msg = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|v| v["error"]["message"].as_str().map(String::from))
            .unwrap_or(body);
        Err(match status.as_u16() {
            401 => anyhow!("OpenRouter rejected the API key (401). Check it in Settings."),
            402 => anyhow!("OpenRouter: insufficient credits (402). {msg}"),
            429 => anyhow!("OpenRouter rate limit reached (429). Try again shortly. {msg}"),
            _ => anyhow!("OpenRouter error {status}: {msg}"),
        })
    }

    /// Returns a short human-readable description of the key's usage/limits.
    pub async fn key_info(&self) -> Result<String> {
        let resp = Self::check(self.req(reqwest::Method::GET, "/key").send().await?).await?;
        let v: Value = resp.json().await?;
        let d = &v["data"];
        let usage = d["usage"].as_f64().unwrap_or(0.0);
        let limit = d["limit"].as_f64();
        Ok(match limit {
            Some(l) => format!("Key OK · used ${usage:.4} of ${l:.2} limit"),
            None => format!("Key OK · used ${usage:.4}"),
        })
    }

    /// Non-streaming completion. `json_mode` asks for a JSON object response.
    pub async fn complete(
        &self,
        model: &str,
        messages: &[Msg],
        json_mode: bool,
        max_tokens: u32,
    ) -> Result<(String, Option<Usage>)> {
        let mut body = json!({
            "model": model,
            "messages": messages,
            "max_tokens": max_tokens,
            "temperature": 0.2,
            "reasoning": { "exclude": true },
            "usage": { "include": true },
        });
        if json_mode {
            body["response_format"] = json!({ "type": "json_object" });
        }
        let resp = Self::check(
            self.req(reqwest::Method::POST, "/chat/completions")
                .json(&body)
                .send()
                .await?,
        )
        .await?;
        let v: Value = resp.json().await?;
        if let Some(err) = v["error"]["message"].as_str() {
            bail!("OpenRouter: {err}");
        }
        let text = v["choices"][0]["message"]["content"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| anyhow!("empty response from model"))?;
        Ok((text, Usage::from_json(&v)))
    }

    /// Streaming completion; `on_delta` receives text pieces as they arrive.
    pub async fn stream(
        &self,
        model: &str,
        messages: &[Msg],
        max_tokens: u32,
        cancel: &AtomicBool,
        on_delta: &mut (dyn FnMut(&str) + Send),
    ) -> Result<(String, Option<Usage>)> {
        let body = json!({
            "model": model,
            "messages": messages,
            "max_tokens": max_tokens,
            "temperature": 0.3,
            "stream": true,
            "reasoning": { "exclude": true },
            "usage": { "include": true },
        });
        let resp = Self::check(
            self.req(reqwest::Method::POST, "/chat/completions")
                .json(&body)
                .send()
                .await?,
        )
        .await?;
        let mut stream = resp.bytes_stream();
        let mut buf: Vec<u8> = Vec::new();
        let mut full = String::new();
        let mut usage: Option<Usage> = None;
        'outer: while let Some(chunk) = stream.next().await {
            if cancel.load(Ordering::Relaxed) {
                bail!("cancelled");
            }
            buf.extend_from_slice(&chunk?);
            while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = buf.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&line);
                let line = line.trim();
                let Some(data) = line.strip_prefix("data:") else {
                    continue;
                };
                let data = data.trim();
                if data == "[DONE]" {
                    break 'outer;
                }
                let Ok(v) = serde_json::from_str::<Value>(data) else {
                    continue;
                };
                if let Some(err) = v["error"]["message"].as_str() {
                    bail!("OpenRouter: {err}");
                }
                // the final chunk carries token usage and cost
                if let Some(u) = Usage::from_json(&v) {
                    usage = Some(u);
                }
                if let Some(d) = v["choices"][0]["delta"]["content"].as_str() {
                    if !d.is_empty() {
                        full.push_str(d);
                        on_delta(d);
                    }
                }
            }
        }
        if full.is_empty() {
            bail!("the model returned an empty answer");
        }
        Ok((full, usage))
    }

    pub async fn embed(&self, model: &str, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
        let mut out = Vec::with_capacity(inputs.len());
        for batch in inputs.chunks(64) {
            let body = json!({ "model": model, "input": batch });
            let resp = Self::check(
                self.req(reqwest::Method::POST, "/embeddings")
                    .json(&body)
                    .send()
                    .await?,
            )
            .await?;
            let v: Value = resp.json().await?;
            if let Some(err) = v["error"]["message"].as_str() {
                bail!("OpenRouter embeddings: {err}");
            }
            let mut data: Vec<(usize, Vec<f32>)> = v["data"]
                .as_array()
                .ok_or_else(|| anyhow!("unexpected embeddings response"))?
                .iter()
                .enumerate()
                .map(|(i, d)| {
                    let idx = d["index"].as_u64().map(|x| x as usize).unwrap_or(i);
                    let e = d["embedding"]
                        .as_array()
                        .map(|a| a.iter().map(|x| x.as_f64().unwrap_or(0.0) as f32).collect())
                        .unwrap_or_default();
                    (idx, e)
                })
                .collect();
            data.sort_by_key(|(i, _)| *i);
            if data.len() != batch.len() {
                bail!(
                    "embeddings response has {} vectors for {} inputs",
                    data.len(),
                    batch.len()
                );
            }
            out.extend(data.into_iter().map(|(_, e)| e));
        }
        Ok(out)
    }
}

/// Public model list (no key needed).
pub async fn list_models() -> Result<Vec<LlmModelInfo>> {
    crate::ensure_crypto_provider();
    let http = reqwest::Client::builder()
        .user_agent(concat!("what-was-that-about/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(30))
        .build()?;
    let base = std::env::var("WWTA_OPENROUTER_BASE").unwrap_or_else(|_| BASE_URL.into());
    let v: Value = http
        .get(format!("{base}/models"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let price = |p: &Value| {
        p.as_str()
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| p.as_f64())
            .map(|x| x * 1_000_000.0)
    };
    let mut models: Vec<LlmModelInfo> = v["data"]
        .as_array()
        .context("unexpected models response")?
        .iter()
        .filter(|m| {
            let out = &m["architecture"]["output_modalities"];
            out.as_array().is_none_or(|a| a.iter().any(|x| x == "text"))
        })
        .map(|m| LlmModelInfo {
            id: m["id"].as_str().unwrap_or_default().to_string(),
            name: m["name"].as_str().unwrap_or_default().to_string(),
            context_length: m["context_length"].as_u64(),
            prompt_price: price(&m["pricing"]["prompt"]),
            completion_price: price(&m["pricing"]["completion"]),
            input_modalities: m["architecture"]["input_modalities"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            created: m["created"].as_u64(),
        })
        .filter(|m| !m.id.is_empty())
        .collect();
    models.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(models)
}

/// Describes images for chat models that can't see them, when the model list is unavailable.
pub const LUNA_FALLBACK: &str = "openai/gpt-6-luna";

/// Numeric parts of a model id ("openai/gpt-6.1-luna" -> [6, 1]), for ordering versions.
fn version_key(id: &str) -> Vec<u32> {
    id.split(|c: char| !c.is_ascii_digit())
        .filter_map(|p| p.parse().ok())
        .collect()
}

/// The newest OpenAI Luna model that accepts images: highest version among plain
/// `openai/gpt-<version>-luna` ids (other Luna variants only if there are none), newest
/// first on ties. Falls back to [`LUNA_FALLBACK`].
pub fn latest_luna(models: &[LlmModelInfo]) -> String {
    let plain = Regex::new(r"^openai/gpt-[0-9][0-9.]*-luna$").unwrap();
    let lunas: Vec<&LlmModelInfo> = models
        .iter()
        .filter(|m| {
            m.id.starts_with("openai/")
                && m.id.contains("luna")
                && !m.id.contains(':')
                && m.accepts("image")
        })
        .collect();
    let pool: Vec<&LlmModelInfo> = if lunas.iter().any(|m| plain.is_match(&m.id)) {
        lunas
            .into_iter()
            .filter(|m| plain.is_match(&m.id))
            .collect()
    } else {
        lunas
    };
    pool.into_iter()
        .max_by(|a, b| {
            (version_key(&a.id), a.created.unwrap_or(0))
                .cmp(&(version_key(&b.id), b.created.unwrap_or(0)))
        })
        .map(|m| m.id.clone())
        .unwrap_or_else(|| LUNA_FALLBACK.into())
}

// ------------------------------------------------------------------ prompts

pub fn language_instruction(ai_language: &str) -> &'static str {
    match ai_language {
        "en" => "Write every piece of prose you produce (titles, summaries, tags, answers) in English, even though the transcript is in another language.",
        "fa" => "Write every piece of prose you produce (titles, summaries, tags, answers) in Persian (فارسی).",
        _ => "Write every piece of prose you produce in the same language as the transcript (usually Persian/فارسی), unless the user writes to you in another language.",
    }
}

/// One transcript line with its stable id: `[12] (03:21) text`.
pub fn transcript_line(s: &Segment) -> String {
    format!("[{}] ({}) {}\n", s.id, fmt_time(s.start), s.text.trim())
}

/// Transcript lines with stable ids: `[12] (03:21) text`.
pub fn transcript_lines(segs: &[Segment], max_chars: usize) -> String {
    let mut out = String::new();
    for s in segs {
        let line = transcript_line(s);
        if out.len() + line.len() > max_chars {
            out.push_str("[…transcript truncated…]\n");
            break;
        }
        out.push_str(&line);
    }
    out
}

/// Transcript prepared for a prompt: every line, or — when that doesn't fit the model's
/// context — condensed notes per section (plus verbatim excerpts for questions).
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptText {
    pub text: String,
    pub condensed: bool,
}

impl TranscriptText {
    pub fn full(segs: &[Segment]) -> Self {
        Self {
            text: transcript_lines(segs, usize::MAX),
            condensed: false,
        }
    }
}

/// Heading of one condensed section: `### [120–185] 10:00–19:58`.
pub fn section_heading(first_id: u32, last_id: u32, start: f64, end: f64) -> String {
    format!(
        "### [{first_id}–{last_id}] {}–{}\n",
        fmt_time(start),
        fmt_time(end)
    )
}

const CONDENSED_NOTE: &str = "The transcript is too long to include in full, so it was condensed: under each \
\"### [first–last] time\" heading are notes on that stretch of segments, each line ending with the segment ids it \
comes from in square brackets.";

pub fn analysis_messages(
    title: &str,
    segs: &[Segment],
    transcript: &TranscriptText,
    ai_language: &str,
) -> Vec<Msg> {
    let last = segs.last().map(|s| s.id).unwrap_or(0);
    let system = format!(
        "You are an expert editor who turns raw speech-recognition transcripts into navigable notes. \
         The transcript comes from automatic speech recognition of a Persian audio recording, so expect \
         recognition errors, missing punctuation and spoken-style language; infer intended meaning. \
         {lang}\n\n\
         Respond with ONE JSON object and nothing else, with exactly these keys:\n\
         {{\n\
           \"title\": short descriptive title for the recording (max 80 chars),\n\
           \"summary\": markdown summary: one short opening paragraph, then the essential content as bullet points,\n\
           \"keyPoints\": array of 3-10 short strings,\n\
           \"chapters\": array of {{\"title\", \"startSegment\", \"endSegment\", \"summary\", \"tags\"}} — contiguous topic \
         sections in order that together cover segments 0..{last}; each 1-3 sentence summary; tags are 1-4 short topic labels,\n\
           \"tags\": array of {{\"name\", \"description\", \"segments\"}} — 4-15 recurring topics/entities/themes; \"segments\" lists \
         every segment id where the topic is discussed,\n\
           \"actionItems\": array of strings (decisions, tasks, follow-ups; empty array if none),\n\
           \"questions\": array of 3-5 questions a listener might want to ask about this recording\n\
         }}\n\
         Segment ids are the numbers in square brackets. Use only ids that exist. Tag names should be short (1-3 words) \
         and reusable across recordings (prefer general topics over one-off phrases).{condensed}",
        lang = language_instruction(ai_language),
        condensed = if transcript.condensed {
            format!("\n\n{CONDENSED_NOTE} Use those ids for chapters and tags.")
        } else {
            String::new()
        },
    );
    let user = format!(
        "Recording: {title}\nSegments: 0..{last}\n\nTranscript:\n{}",
        transcript.text
    );
    vec![Msg::system(system), Msg::user(user)]
}
/// Extract the first top-level JSON object from a model response.
pub fn extract_json(s: &str) -> Result<Value> {
    let trimmed = s.trim();
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        return Ok(v);
    }
    let start = trimmed
        .find('{')
        .ok_or_else(|| anyhow!("no JSON object in model response"))?;
    let end = trimmed
        .rfind('}')
        .ok_or_else(|| anyhow!("no JSON object in model response"))?;
    serde_json::from_str(&trimmed[start..=end]).context("model returned malformed JSON")
}

fn as_u32(v: &Value) -> Option<u32> {
    v.as_u64()
        .map(|x| x as u32)
        .or_else(|| v.as_f64().map(|x| x.max(0.0) as u32))
        .or_else(|| {
            v.as_str()
                .and_then(|s| s.trim().trim_matches(['[', ']']).parse().ok())
        })
}

fn str_list(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Turn the model's JSON into a validated Analysis (ids clamped, times filled in).
pub fn parse_analysis(v: &Value, segs: &[Segment], model: &str, language: &str) -> Analysis {
    let n = segs.len() as u32;
    let max_id = n.saturating_sub(1);
    let mut chapters: Vec<Chapter> = v["chapters"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|c| {
                    let s = as_u32(&c["startSegment"])?.min(max_id);
                    let e = as_u32(&c["endSegment"]).unwrap_or(s).clamp(s, max_id);
                    Some(Chapter {
                        title: c["title"].as_str().unwrap_or("").trim().to_string(),
                        start_segment: s,
                        end_segment: e,
                        start: 0.0,
                        end: 0.0,
                        summary: c["summary"].as_str().unwrap_or("").trim().to_string(),
                        tags: str_list(&c["tags"]),
                    })
                })
                .filter(|c| !c.title.is_empty())
                .collect()
        })
        .unwrap_or_default();
    chapters.sort_by_key(|c| c.start_segment);
    chapters.dedup_by_key(|c| c.start_segment);
    // make chapters contiguous and non-overlapping
    let len = chapters.len();
    for i in 0..len {
        let next_start = if i + 1 < len {
            chapters[i + 1].start_segment
        } else {
            n
        };
        chapters[i].end_segment = next_start.saturating_sub(1).max(chapters[i].start_segment);
    }
    if let Some(first) = chapters.first_mut() {
        first.start_segment = 0;
    }
    for c in &mut chapters {
        if let (Some(a), Some(b)) = (
            segs.get(c.start_segment as usize),
            segs.get(c.end_segment as usize),
        ) {
            c.start = a.start;
            c.end = b.end;
        }
    }
    let tags: Vec<Tag> = v["tags"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|t| {
                    let name = t["name"]
                        .as_str()?
                        .trim()
                        .trim_start_matches('#')
                        .to_string();
                    if name.is_empty() {
                        return None;
                    }
                    let mut segments: Vec<u32> = t["segments"]
                        .as_array()
                        .map(|a| a.iter().filter_map(as_u32).filter(|&i| i < n).collect())
                        .unwrap_or_default();
                    segments.sort_unstable();
                    segments.dedup();
                    Some(Tag {
                        name,
                        description: t["description"].as_str().unwrap_or("").trim().to_string(),
                        segments,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Analysis {
        title: v["title"]
            .as_str()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        summary: v["summary"].as_str().unwrap_or("").trim().to_string(),
        key_points: str_list(&v["keyPoints"]),
        chapters,
        tags,
        action_items: str_list(&v["actionItems"]),
        questions: str_list(&v["questions"]),
        model: model.to_string(),
        language: language.to_string(),
        created_at: crate::types::now_iso(),
    }
}

pub fn qa_system(
    title: &str,
    transcript: &TranscriptText,
    analysis: Option<&Analysis>,
    notes: &[crate::types::Note],
    ai_language: &str,
) -> String {
    let mut summary = analysis
        .map(|a| format!("\nExisting summary of the recording:\n{}\n", a.summary))
        .unwrap_or_default();
    if !notes.is_empty() {
        summary.push_str("\nThe user's own notes on this recording (time range: note):\n");
        for n in notes {
            summary.push_str(&format!(
                "- ({}–{}) {}\n",
                fmt_time(n.start),
                fmt_time(n.end),
                n.body.trim()
            ));
        }
    }
    let (label, condensed_rule) = if transcript.condensed {
        (
            "Transcript (condensed notes, then verbatim excerpts)",
            format!(
                "- {CONDENSED_NOTE} After the notes come verbatim excerpts (\"[id] (time) text\") picked for the \
                 latest question. Cite segment ids from either. If a detail isn't there, say the answer may be in \
                 parts of the recording you can't see in full.\n"
            ),
        )
    } else {
        ("Transcript (\"[id] (time) text\")", String::new())
    };
    format!(
        "You answer questions about one audio recording using its automatic transcript. {lang}\n\
         Rules:\n\
         - Ground every claim in the transcript. After each claim cite the supporting segment ids in square brackets, \
         e.g. [12] or a range [12-15]. Cite precisely; these become clickable links to the audio.\n\
         - If the transcript does not contain the answer, say so plainly instead of guessing.\n\
         - The transcript comes from speech recognition and may contain misrecognized words; interpret sensibly.\n\
         - The user may attach images or voice messages; use them together with the transcript.\n\
         {condensed_rule}\
         - Be concise and use markdown (short paragraphs or bullets).\n\
         Recording: {title}\n{summary}\n{label}:\n{}",
        transcript.text,
        lang = language_instruction(ai_language),
    )
}

pub struct LibraryContextItem {
    pub recording_id: String,
    pub recording_title: String,
    pub segments: Vec<Segment>,
}

/// Library-wide Q&A uses short aliases (R1, R2…) for recordings; answers are rewritten to
/// `[rec:<uuid>#<seg>]` citations by [`CitationRewriter`].
pub fn library_qa_system(
    items: &[LibraryContextItem],
    ai_language: &str,
    max_tokens: usize,
) -> String {
    let mut ctx = String::new();
    let mut used = 0usize;
    'items: for (i, it) in items.iter().enumerate() {
        let heading = format!("\n## R{} — {}\n", i + 1, it.recording_title);
        used += estimate_tokens(&heading);
        ctx.push_str(&heading);
        for s in &it.segments {
            let line = format!(
                "[R{}#{}] ({}) {}\n",
                i + 1,
                s.id,
                fmt_time(s.start),
                s.text.trim()
            );
            used += estimate_tokens(&line);
            if used > max_tokens {
                ctx.push_str("[…more excerpts omitted…]\n");
                break 'items;
            }
            ctx.push_str(&line);
        }
    }
    format!(
        "You answer questions about the user's library of transcribed audio recordings. {lang}\n\
         You are given excerpts retrieved by search; each line starts with a citation key like [R2#14] \
         (recording R2, segment 14).\n\
         Rules:\n\
         - Ground every claim in the excerpts and cite the keys you used in square brackets exactly as written, e.g. [R2#14].\n\
         - Mention which recording a point comes from when it helps.\n\
         - The user may attach images or voice messages; use them together with the excerpts.\n\
         - If the excerpts don't answer the question, say so and suggest what to search for.\n\
         - Transcripts come from speech recognition and may contain errors. Be concise; use markdown.\n\
         Excerpts:{ctx}",
        lang = language_instruction(ai_language),
    )
}

/// Rewrites `[R2#14]` / `[R2#14-16]` aliases to `[rec:<uuid>#14]` in a token stream.
pub struct CitationRewriter {
    ids: Vec<String>,
    pending: String,
    re: Regex,
}

impl CitationRewriter {
    pub fn new(ids: Vec<String>) -> Self {
        Self {
            ids,
            pending: String::new(),
            re: Regex::new(r"\[R(\d+)#(\d+)(?:\s*-\s*(?:R\d+#)?(\d+))?\]").unwrap(),
        }
    }

    fn rewrite(&self, s: &str) -> String {
        self.re
            .replace_all(s, |c: &regex::Captures| {
                let idx: usize = c[1].parse().unwrap_or(0);
                match self.ids.get(idx.wrapping_sub(1)) {
                    Some(id) => match c.get(3) {
                        Some(end) => format!("[rec:{id}#{}-{}]", &c[2], end.as_str()),
                        None => format!("[rec:{id}#{}]", &c[2]),
                    },
                    None => c[0].to_string(),
                }
            })
            .into_owned()
    }

    /// Feed a delta, get the text that is safe to emit now.
    pub fn push(&mut self, delta: &str) -> String {
        self.pending.push_str(delta);
        // hold back an unterminated "[..." that could still become a citation
        let cut = match self.pending.rfind('[') {
            Some(i) if !self.pending[i..].contains(']') && self.pending.len() - i < 48 => i,
            _ => self.pending.len(),
        };
        let ready: String = self.pending.drain(..cut).collect();
        self.rewrite(&ready)
    }

    pub fn finish(&mut self) -> String {
        let rest = std::mem::take(&mut self.pending);
        self.rewrite(&rest)
    }

    pub fn rewrite_all(&self, s: &str) -> String {
        self.rewrite(s)
    }
}

pub const DOC_BRIDGE_DOC: &str = r#"The document runs inside a sandboxed iframe in the app with this API pre-defined (do not redefine it):
  window.WWTA.recording  -> { id, title, durationSec }
  window.WWTA.segments   -> [{ id, start, end, text }]   // full transcript; start/end in seconds
  window.WWTA.notes      -> [{ start, end, body, color }]   // the user's own notes (markdown bodies)
  window.WWTA.analysis   -> null or { title, summary, keyPoints[], chapters[{title,startSegment,endSegment,start,end,summary,tags[]}], tags[{name,description,segments[]}], actionItems[], questions[] }
  window.WWTA.seek(seconds)  // jump the app's audio player to that time and play
  window.WWTA.play() / window.WWTA.pause()
  window.WWTA.onTime(cb)     // cb(currentSeconds, isPlaying) is called as audio plays"#;

pub fn doc_system(ai_language: &str) -> String {
    format!(
        "You create small, beautiful, self-contained interactive HTML documents that help someone explore and \
         understand an audio recording (mind maps, timelines, flashcards, quizzes, minutes, glossaries, dashboards…). {lang}\n\n\
         {bridge}\n\n\
         Hard requirements:\n\
         - Output ONE complete HTML document (<!doctype html> … </html>) and nothing else (no commentary).\n\
         - Inline all CSS and JavaScript. NO external resources: no CDNs, fonts, images by URL, fetch/XHR, \
         iframes or forms — a strict Content-Security-Policy blocks them. Draw with HTML/CSS/inline SVG/canvas.\n\
         - Read transcript data from window.WWTA at runtime instead of pasting the whole transcript into the HTML; \
         embed only the content you write (titles, explanations, quiz questions…) and segment ids/times you reference.\n\
         - Anything tied to a moment in the audio must be clickable and call WWTA.seek(seconds).\n\
         - Persian text must render right-to-left (use dir=\"rtl\" or dir=\"auto\" on text containers). Use a system \
         font stack such as Vazirmatn, Tahoma, system-ui, sans-serif.\n\
         - Support light and dark mode via prefers-color-scheme, be responsive, and keep the file under ~60 KB.\n\
         - Make it polished: clear hierarchy, generous spacing, subtle colors, smooth but minimal animation.",
        lang = language_instruction(ai_language),
        bridge = DOC_BRIDGE_DOC,
    )
}

pub fn doc_user(
    prompt: &str,
    title: &str,
    transcript: &TranscriptText,
    analysis: Option<&Analysis>,
) -> String {
    let analysis_json = analysis
        .map(|a| serde_json::to_string(a).unwrap_or_default())
        .unwrap_or_else(|| "null".into());
    let label = if transcript.condensed {
        format!(
            "Transcript, condensed for this request (the full transcript is still available as WWTA.segments). \
             {CONDENSED_NOTE}"
        )
    } else {
        "Transcript (also available as WWTA.segments; \"[id] (time) text\")".into()
    };
    format!(
        "Request: {prompt}\n\nRecording: {title}\n\nAnalysis JSON (also available as WWTA.analysis):\n{analysis_json}\n\n\
         {label}:\n{}",
        transcript.text
    )
}

// ------------------------------------------------------------------ condensing & compaction

/// Ask the model to condense one transcript section (`level` 0) or a group of section
/// notes (`level` ≥ 1) into shorter notes that keep segment ids.
pub fn condense_messages(title: &str, input: &str, level: u32) -> Vec<Msg> {
    let system = "You condense automatic speech-recognition transcripts of Persian audio into dense notes. The \
                  notes stand in for the transcript later, when someone asks questions about the recording or \
                  wants a summary, so nothing important may be lost.";
    let user = if level == 0 {
        format!(
            "Condense this part of the transcript of \"{title}\". Keep every fact, name, number, date, claim, \
             argument, example, decision, task and question; drop filler, repetition and small talk. Write short \
             lines and end each with the segment ids it comes from in square brackets, e.g. [12] or [12-15]. Write \
             in the transcript's language. Aim for about one fifth of the original length. Output only the notes.\n\n\
             Transcript (\"[id] (time) text\"):\n{input}"
        )
    } else {
        format!(
            "These notes were condensed from consecutive parts of the transcript of \"{title}\"; each line cites \
             segment ids. Condense them to about one third of their length: merge related points, keep the important \
             facts, names, numbers and decisions, and end every line with the segment ids it covers in square \
             brackets. Keep the language. Output only the notes.\n\n{input}"
        )
    };
    vec![Msg::system(system), Msg::user(user)]
}

/// Ask the model to merge older chat messages into the running conversation summary.
pub fn summary_messages(previous: Option<&str>, conversation: &str, about: &str) -> Vec<Msg> {
    let system = format!(
        "You keep the memory of a conversation between a user and an assistant about {about}. The assistant will \
         continue the conversation with only your summary instead of these messages."
    );
    let previous = previous
        .filter(|p| !p.trim().is_empty())
        .map(|p| format!("Summary so far:\n{p}\n\n"))
        .unwrap_or_default();
    let user = format!(
        "{previous}Messages to add:\n{conversation}\n\n\
         Write one updated summary that merges both. Keep what the user asked and wants, the key facts and conclusions \
         of the answers with their citations exactly as written (e.g. [12], [12-15], [rec:…#12]), what attached images \
         and voice messages contained, the user's preferences and open questions. Drop pleasantries. Use compact bullet \
         points, at most about 400 words, in the conversation's language. Output only the summary."
    );
    vec![Msg::system(system), Msg::user(user)]
}

/// Ask a vision model to describe an image for a chat model that can't see it.
pub fn describe_image_messages(image: Part, question: &str) -> Vec<Msg> {
    let ask = if question.trim().is_empty() {
        String::new()
    } else {
        format!(" Their message: \"{}\".", question.trim())
    };
    vec![
        Msg::system(
            "You describe images for an assistant that cannot see them. Be precise and complete but concise, and \
             don't speculate.",
        ),
        Msg::user_parts(vec![
            Part::text(format!(
                "The user attached this image to a question about a transcribed audio recording.{ask} Describe \
                 everything in it that could matter: all visible text verbatim (in its original language), numbers, \
                 tables, charts, diagrams, people, objects and layout. Output only the description."
            )),
            image,
        ]),
    ]
}

/// Pull the HTML document out of a model response.
pub fn extract_html(s: &str) -> Result<String> {
    let body = if let Some(i) = s.find("```html") {
        let rest = &s[i + 7..];
        match rest.find("```") {
            Some(j) => &rest[..j],
            None => rest,
        }
    } else if let Some(i) = s.find("```") {
        let rest = &s[i + 3..];
        match rest.find("```") {
            Some(j) => &rest[..j],
            None => rest,
        }
    } else {
        s
    };
    let lower = body.to_ascii_lowercase();
    let start = lower
        .find("<!doctype")
        .or_else(|| lower.find("<html"))
        .unwrap_or(0);
    let end = lower.rfind("</html>").map(|i| i + 7).unwrap_or(body.len());
    let html = body[start..end].trim();
    if !html.to_ascii_lowercase().contains("<") {
        bail!("the model did not return an HTML document");
    }
    Ok(html.to_string())
}

pub fn html_title(html: &str) -> Option<String> {
    let re = Regex::new(r"(?is)<title[^>]*>(.*?)</title>").ok()?;
    let t = re.captures(html)?.get(1)?.as_str().trim().to_string();
    (!t.is_empty()).then(|| t.chars().take(100).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segs(n: u32) -> Vec<Segment> {
        (0..n)
            .map(|i| Segment {
                id: i,
                start: i as f64 * 10.0,
                end: i as f64 * 10.0 + 9.0,
                text: format!("جمله {i}"),
                words: vec![],
            })
            .collect()
    }

    #[test]
    fn parses_and_repairs_analysis() {
        let v: Value = serde_json::from_str(
            r##"{"title":"عنوان","summary":"خلاصه","keyPoints":["a"],
            "chapters":[{"title":"دوم","startSegment":"5","endSegment":3},{"title":"اول","startSegment":1,"endSegment":4},{"title":"","startSegment":7}],
            "tags":[{"name":"#اقتصاد","segments":[1,"2",99,2]}],"actionItems":[],"questions":["چرا؟"]}"##,
        )
        .unwrap();
        let a = parse_analysis(&v, &segs(10), "m", "fa");
        assert_eq!(a.chapters.len(), 2);
        assert_eq!(a.chapters[0].title, "اول");
        assert_eq!(a.chapters[0].start_segment, 0);
        assert_eq!(a.chapters[0].end_segment, 4);
        assert_eq!(a.chapters[1].end_segment, 9);
        assert_eq!(a.chapters[1].end, 99.0);
        assert_eq!(a.tags[0].name, "اقتصاد");
        assert_eq!(a.tags[0].segments, vec![1, 2]);
    }

    #[test]
    fn extracts_json_and_html() {
        assert_eq!(
            extract_json("Here:\n```json\n{\"a\":1}\n```").unwrap()["a"],
            1
        );
        let h = extract_html(
            "Sure!\n```html\n<!DOCTYPE html><html><head><title>T</title></head></html>\n```\nenjoy",
        )
        .unwrap();
        assert!(h.starts_with("<!DOCTYPE"));
        assert!(h.ends_with("</html>"));
        assert_eq!(html_title(&h).as_deref(), Some("T"));
    }

    #[test]
    fn citation_rewriter_handles_split_tokens() {
        let mut r = CitationRewriter::new(vec!["uuid-a".into(), "uuid-b".into()]);
        let mut out = String::new();
        for d in ["Budget is low [R", "2#1", "4] and [R1#3-5]", " done [x"] {
            out.push_str(&r.push(d));
        }
        out.push_str(&r.finish());
        assert_eq!(
            out,
            "Budget is low [rec:uuid-b#14] and [rec:uuid-a#3-5] done [x"
        );
    }

    #[test]
    fn parses_openrouter_usage() {
        let v: Value = serde_json::from_str(
            r#"{"usage":{"prompt_tokens":1200,"completion_tokens":300,"cost":0.00042}}"#,
        )
        .unwrap();
        let u = Usage::from_json(&v).unwrap();
        assert_eq!((u.prompt_tokens, u.completion_tokens), (1200, 300));
        assert_eq!(u.cost, Some(0.00042));
        assert!(Usage::from_json(&serde_json::json!({"choices": []})).is_none());
    }

    fn model(id: &str, modalities: &[&str], created: u64) -> LlmModelInfo {
        LlmModelInfo {
            id: id.into(),
            name: id.into(),
            context_length: Some(128_000),
            prompt_price: None,
            completion_price: None,
            input_modalities: modalities.iter().map(|m| m.to_string()).collect(),
            created: Some(created),
        }
    }

    #[test]
    fn picks_the_latest_luna_that_sees_images() {
        assert_eq!(latest_luna(&[]), LUNA_FALLBACK);
        let models = vec![
            model("openai/gpt-5.5-luna", &["text", "image"], 300),
            model("openai/gpt-6-luna", &["text", "image"], 200),
            model("openai/gpt-6.1-luna", &["text"], 400), // can't see images
            model("openai/gpt-6-luna-mini", &["text", "image"], 500),
            model("openai/gpt-7-luna:free", &["text", "image"], 600),
            model("someone/luna-7", &["text", "image"], 700),
        ];
        assert_eq!(latest_luna(&models), "openai/gpt-6-luna");
        // only variants available: newest version wins
        let variants = vec![
            model("openai/gpt-6-luna-mini", &["image"], 100),
            model("openai/gpt-6.2-luna-mini", &["image"], 50),
        ];
        assert_eq!(latest_luna(&variants), "openai/gpt-6.2-luna-mini");
        let none = vec![model("openai/gpt-6", &["text", "image"], 1)];
        assert_eq!(latest_luna(&none), LUNA_FALLBACK);
    }

    #[test]
    fn serializes_multimodal_messages_for_openrouter() {
        let m = Msg::user_parts(vec![
            Part::text("what is this?"),
            Part::image("image/png", b"\x89PNG"),
        ]);
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(
            v["content"][0],
            json!({"type": "text", "text": "what is this?"})
        );
        assert_eq!(v["content"][1]["type"], "image_url");
        assert_eq!(
            v["content"][1]["image_url"]["url"],
            "data:image/png;base64,iVBORw=="
        );
        let a = serde_json::to_value(Part::wav(b"RIFF")).unwrap();
        assert_eq!(
            a,
            json!({"type": "input_audio", "input_audio": {"data": "UklGRg==", "format": "wav"}})
        );
        // a lone text part stays a plain string
        let t = serde_json::to_value(Msg::user_parts(vec![Part::text("hi")])).unwrap();
        assert_eq!(t, json!({"role": "user", "content": "hi"}));
        assert!(m.tokens() > IMAGE_TOKENS);
    }

    #[test]
    fn library_context_respects_the_budget() {
        let items = vec![LibraryContextItem {
            recording_id: "a".into(),
            recording_title: "A".into(),
            segments: segs(200),
        }];
        let small = library_qa_system(&items, "auto", 200);
        assert!(small.contains("[R1#0]"));
        assert!(small.contains("omitted"));
        assert!(!small.contains("[R1#199]"));
        assert!(library_qa_system(&items, "auto", 100_000).contains("[R1#199]"));
    }

    #[test]
    fn transcript_lines_truncate() {
        let t = transcript_lines(&segs(3), 1000);
        assert!(t.starts_with("[0] (0:00) جمله 0"));
        assert!(transcript_lines(&segs(100), 50).contains("truncated"));
    }
}
