//! OpenRouter client (streaming chat completions with tool calls, embeddings, model list) and
//! the prompts behind analysis, transcript condensing and fixing, chat compaction and image
//! descriptions, plus citation rewriting. Q&A and docs run as agents, see [`crate::agent`].

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

/// One chat message in OpenAI format. Assistant messages may carry tool calls; `tool`
/// messages answer one of them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Msg {
    pub role: String,
    pub content: Content,
    pub tool_calls: Vec<ToolCall>,
    pub tool_call_id: Option<String>,
    /// The model's reasoning blocks (possibly encrypted) from a tool-calling turn. Some models
    /// (e.g. Gemini 3) need them sent back unchanged with the tool results.
    pub reasoning_details: Vec<Value>,
}

/// Message content: plain text, or parts mixing text with images and audio.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Content {
    Text(String),
    Parts(Vec<Part>),
}

impl Default for Content {
    fn default() -> Self {
        Content::Text(String::new())
    }
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

    /// The text, leaving out images and audio.
    pub fn text(&self) -> String {
        match self {
            Content::Text(t) => t.clone(),
            Content::Parts(p) => p
                .iter()
                .filter_map(|p| match p {
                    Part::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Content::Text(t) => t.is_empty(),
            Content::Parts(p) => p.is_empty(),
        }
    }
}

impl Msg {
    fn new(role: &str, c: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: Content::Text(c.into()),
            ..Default::default()
        }
    }
    pub fn system(c: impl Into<String>) -> Self {
        Self::new("system", c)
    }
    pub fn user(c: impl Into<String>) -> Self {
        Self::new("user", c)
    }
    pub fn assistant(c: impl Into<String>) -> Self {
        Self::new("assistant", c)
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
            ..Default::default()
        }
    }
    pub fn tool_calls(c: impl Into<String>, calls: Vec<ToolCall>) -> Self {
        Self {
            tool_calls: calls,
            ..Self::assistant(c)
        }
    }
    pub fn tool_result(call_id: &str, c: impl Into<String>) -> Self {
        Self {
            tool_call_id: Some(call_id.into()),
            ..Self::new("tool", c)
        }
    }
    pub fn tokens(&self) -> usize {
        self.content.tokens() + 4
    }
}

impl Serialize for Msg {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        let mut v = json!({ "role": self.role, "content": self.content });
        if !self.tool_calls.is_empty() {
            // some providers reject an empty text block next to tool calls
            if self.content.is_empty() {
                v["content"] = Value::Null;
            }
            v["tool_calls"] = self
                .tool_calls
                .iter()
                .map(|c| {
                    json!({
                        "id": c.id,
                        "type": "function",
                        "function": { "name": c.name, "arguments": c.arguments },
                    })
                })
                .collect();
        }
        if let Some(id) = &self.tool_call_id {
            v["tool_call_id"] = id.as_str().into();
        }
        if !self.reasoning_details.is_empty() {
            v["reasoning_details"] = self.reasoning_details.clone().into();
        }
        v.serialize(s)
    }
}

/// A function call requested by the model. `arguments` is the raw JSON text.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
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

pub struct ChatRequest<'a> {
    pub model: &'a str,
    pub messages: &'a [Msg],
    /// OpenAI-format function definitions; empty for a plain completion.
    pub tools: &'a [Value],
    /// e.g. "none" to make the model answer instead of calling tools.
    pub tool_choice: Option<&'a str>,
    pub max_tokens: u32,
}

/// One streamed model reply.
#[derive(Debug, Clone, Default)]
pub struct Reply {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Option<Usage>,
    /// Reasoning blocks to replay with the tool results (never shown).
    pub reasoning_details: Vec<Value>,
}

/// Add one streamed reasoning-detail chunk: pieces of the same block share an `index`, and
/// their text fields are concatenated in order.
fn merge_reasoning(acc: &mut Vec<Value>, chunk: &Value) {
    let index = chunk["index"].as_u64();
    let Some(block) = index.and_then(|i| acc.iter_mut().find(|b| b["index"].as_u64() == Some(i)))
    else {
        acc.push(chunk.clone());
        return;
    };
    for (k, v) in chunk.as_object().into_iter().flatten() {
        match (k.as_str(), v, &mut block[k]) {
            (_, Value::Null, _) => {}
            ("text" | "summary" | "data" | "signature", Value::String(add), Value::String(cur)) => {
                cur.push_str(add)
            }
            (_, v, slot) if slot.is_null() => *slot = v.clone(),
            _ => {}
        }
    }
}

/// Assembles a [`Reply`] from streamed chunks. Tool calls arrive in pieces keyed by `index`
/// (the name first, then the arguments a few characters at a time).
#[derive(Default)]
struct ReplyBuilder {
    reply: Reply,
}

impl ReplyBuilder {
    /// Apply one chunk; returns the answer text it added, if any.
    fn apply(&mut self, v: &Value) -> Result<Option<String>> {
        if let Some(err) = v["error"]["message"].as_str() {
            bail!("OpenRouter: {err}");
        }
        // the final chunk carries token usage and cost
        if let Some(u) = Usage::from_json(v) {
            self.reply.usage = Some(u);
        }
        let delta = &v["choices"][0]["delta"];
        for d in delta["reasoning_details"].as_array().into_iter().flatten() {
            merge_reasoning(&mut self.reply.reasoning_details, d);
        }
        let calls = &mut self.reply.tool_calls;
        for tc in delta["tool_calls"].as_array().into_iter().flatten() {
            let id = tc["id"].as_str().filter(|s| !s.is_empty());
            let idx = match (tc["index"].as_u64(), id) {
                (Some(i), _) => i as usize,
                // providers that omit the index send each call whole, with its own id
                (None, Some(id)) => calls.iter().position(|c| c.id == id).unwrap_or(calls.len()),
                (None, None) => calls.len().saturating_sub(1),
            };
            if calls.len() <= idx {
                calls.resize_with(idx + 1, ToolCall::default);
            }
            let c = &mut calls[idx];
            if let Some(id) = id {
                c.id = id.to_string();
            }
            if let Some(n) = tc["function"]["name"].as_str() {
                c.name.push_str(n);
            }
            if let Some(a) = tc["function"]["arguments"].as_str() {
                c.arguments.push_str(a);
            }
        }
        match delta["content"].as_str() {
            Some(d) if !d.is_empty() => {
                self.reply.content.push_str(d);
                Ok(Some(d.to_string()))
            }
            _ => Ok(None),
        }
    }

    fn finish(mut self) -> Reply {
        let calls = &mut self.reply.tool_calls;
        calls.retain(|c| !c.name.is_empty());
        for (i, c) in calls.iter_mut().enumerate() {
            if c.id.is_empty() {
                c.id = format!("call_{i}");
            }
            if c.arguments.trim().is_empty() {
                c.arguments = "{}".into();
            }
        }
        self.reply
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
        let base = std::env::var("WWTA_OPENROUTER_BASE").unwrap_or_else(|_| BASE_URL.into());
        Self::with_base(key, &base)
    }

    pub fn with_base(key: &str, base: &str) -> Result<Self> {
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
            base: base.trim_end_matches('/').to_string(),
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

    /// Streaming chat completion, optionally with tools; `on_delta` receives answer text as
    /// it arrives. Returns the full text and any tool calls the model made.
    pub async fn chat(
        &self,
        req: &ChatRequest<'_>,
        cancel: &AtomicBool,
        on_delta: &mut (dyn FnMut(&str) + Send),
    ) -> Result<Reply> {
        let mut body = json!({
            "model": req.model,
            "messages": req.messages,
            "max_tokens": req.max_tokens,
            "temperature": 0.3,
            "stream": true,
            // no `reasoning.exclude`: tool turns must send the reasoning blocks back, and
            // hiding them doesn't save tokens (only `delta.content` is shown)
            "usage": { "include": true },
        });
        if !req.tools.is_empty() {
            body["tools"] = req.tools.into();
            if let Some(choice) = req.tool_choice {
                body["tool_choice"] = choice.into();
            }
        }
        let resp = Self::check(
            self.req(reqwest::Method::POST, "/chat/completions")
                .json(&body)
                .send()
                .await?,
        )
        .await?;
        let mut stream = resp.bytes_stream();
        let mut buf: Vec<u8> = Vec::new();
        let mut reply = ReplyBuilder::default();
        'outer: while let Some(chunk) = stream.next().await {
            if cancel.load(Ordering::Relaxed) {
                bail!("cancelled");
            }
            buf.extend_from_slice(&chunk?);
            while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = buf.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&line);
                let Some(data) = line.trim().strip_prefix("data:") else {
                    continue;
                };
                let data = data.trim();
                if data == "[DONE]" {
                    break 'outer;
                }
                let Ok(v) = serde_json::from_str::<Value>(data) else {
                    continue;
                };
                if let Some(d) = reply.apply(&v)? {
                    on_delta(&d);
                }
            }
        }
        Ok(reply.finish())
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
            tools: m["supported_parameters"]
                .as_array()
                .is_none_or(|a| a.iter().any(|x| x == "tools")),
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
/// context — condensed notes per section.
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

/// Rewrites `[R2#14]`, `[R2#14-16]` and groups like `[R2#14, R3#5]` to `[rec:<uuid>#14]`
/// citations in a token stream. `R<n>` is an alias for `ids[n - 1]` (see the agent's tools).
#[derive(Default)]
pub struct CitationRewriter {
    pending: String,
}

static GROUP_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    let part = r"R\d+#\d+(?:\s*[-–]\s*(?:R\d+#)?\d+)?";
    Regex::new(&format!(r"\[({part}(?:\s*[,،;]\s*{part})*)\]")).unwrap()
});
static PART_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r"R(\d+)#(\d+)(?:\s*([-–])\s*(?:R\d+#)?(\d+))?").unwrap()
});

/// Rewrite every aliased citation in `s`; groups with an unknown alias stay as they are.
pub fn rewrite_citations(s: &str, ids: &[String]) -> String {
    GROUP_RE
        .replace_all(s, |g: &regex::Captures| {
            let mut ok = true;
            let inner = PART_RE.replace_all(&g[1], |c: &regex::Captures| {
                let idx: usize = c[1].parse().unwrap_or(0);
                match ids.get(idx.wrapping_sub(1)) {
                    Some(id) => match c.get(4) {
                        Some(end) => format!("rec:{id}#{}-{}", &c[2], end.as_str()),
                        None => format!("rec:{id}#{}", &c[2]),
                    },
                    None => {
                        ok = false;
                        c[0].to_string()
                    }
                }
            });
            if ok {
                format!("[{inner}]")
            } else {
                g[0].to_string()
            }
        })
        .into_owned()
}

impl CitationRewriter {
    /// Feed a delta, get the text that is safe to emit now.
    pub fn push(&mut self, delta: &str, ids: &[String]) -> String {
        self.pending.push_str(delta);
        // hold back an unterminated "[..." that could still become a citation
        let cut = match self.pending.rfind('[') {
            Some(i) if !self.pending[i..].contains(']') && self.pending.len() - i < 200 => i,
            _ => self.pending.len(),
        };
        let ready: String = self.pending.drain(..cut).collect();
        rewrite_citations(&ready, ids)
    }

    pub fn finish(&mut self, ids: &[String]) -> String {
        let rest = std::mem::take(&mut self.pending);
        rewrite_citations(&rest, ids)
    }

    /// Drop held-back text (the model's words before a tool call are discarded).
    pub fn reset(&mut self) {
        self.pending.clear();
    }
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
    // the closing tag after the opening one: an answer may mention `</html>` before it
    let end = lower[start..]
        .rfind("</html>")
        .map(|i| start + i + 7)
        .unwrap_or(body.len());
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

// ------------------------------------------------------------------ transcript fixing

/// Transcript text per fix request. The model answers with the changed segments only, so
/// the reply stays well under `FIX_MAX_TOKENS` even when most of a batch needs fixing.
const FIX_BATCH_CHARS: usize = 5_000;
const FIX_BATCH_SEGMENTS: usize = 80;
/// Read-only segments shown on each side of a batch.
const FIX_CONTEXT_SEGMENTS: usize = 3;
pub const FIX_MAX_TOKENS: u32 = 12_000;

/// Split a transcript into consecutive batches (index ranges into `segs`) for fixing.
pub fn fix_batches(segs: &[Segment]) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut chars = 0;
    for (i, s) in segs.iter().enumerate() {
        let len = s.text.chars().count();
        if i > start && (chars + len > FIX_BATCH_CHARS || i - start >= FIX_BATCH_SEGMENTS) {
            out.push(start..i);
            start = i;
            chars = 0;
        }
        chars += len;
    }
    if start < segs.len() {
        out.push(start..segs.len());
    }
    out
}

const FIX_SYSTEM: &str = "You proofread automatic speech recognition (ASR) transcripts of Persian (Farsi) audio. \
    Each line is one segment: `[id] text`. Fix recognition errors so each segment says what the speaker \
    actually said:\n\
    - misheard words: use the context to pick the word that was meant (usually one that sounds similar)\n\
    - words wrongly split or joined, missing or wrong half-spaces (ZWNJ, e.g. می\u{200c}شود), spelling mistakes\n\
    - Arabic letter forms (ي ك) instead of Persian (ی ک)\n\
    - missing or wrong punctuation (، . ؟ !)\n\
    Rules:\n\
    - Keep the speaker's words, word order and spoken style. Colloquial forms stay colloquial \
    (می\u{200c}خوام must not become می\u{200c}خواهم). \
    Never paraphrase, summarize, translate, censor, complete or reorder.\n\
    - Keep words from other languages, filler words and repetitions; keep numbers in the form they have \
    (digits stay digits, words stay words).\n\
    - Segments stay separate: never move words from one segment to another, merge or split segments.\n\
    - Only change what is clearly a recognition error; when unsure, leave the text as it is.\n\
    - Correct only the segments under \"Segments to correct\". Context lines are there to help you \
    understand; never return them.\n\
    Respond with ONE JSON object and nothing else: {\"fixes\": [{\"id\": 12, \"text\": \"the complete corrected \
    text of segment 12\"}]}. List only the segments you changed; reply {\"fixes\": []} if none need changes.";

fn id_lines(segs: &[Segment]) -> String {
    segs.iter()
        .map(|s| format!("[{}] {}\n", s.id, s.text.trim()))
        .collect()
}

/// The request for one batch: `segs[batch]` with a few read-only segments around it.
/// `summary` (from the AI analysis) helps the model with names and terms.
pub fn fix_messages(
    title: &str,
    summary: Option<&str>,
    segs: &[Segment],
    batch: std::ops::Range<usize>,
) -> Vec<Msg> {
    let mut user = format!("Recording: {title}\n");
    if let Some(s) = summary.map(str::trim).filter(|s| !s.is_empty()) {
        user.push_str(&format!(
            "\nSummary of the whole recording (for names and terms):\n{s}\n"
        ));
    }
    let before = &segs[batch.start.saturating_sub(FIX_CONTEXT_SEGMENTS)..batch.start];
    let after = &segs[batch.end..(batch.end + FIX_CONTEXT_SEGMENTS).min(segs.len())];
    if !before.is_empty() {
        user.push_str(&format!(
            "\nContext before (don't correct):\n{}",
            id_lines(before)
        ));
    }
    user.push_str(&format!(
        "\nSegments to correct:\n{}",
        id_lines(&segs[batch])
    ));
    if !after.is_empty() {
        user.push_str(&format!(
            "\nContext after (don't correct):\n{}",
            id_lines(after)
        ));
    }
    vec![Msg::system(FIX_SYSTEM), Msg::user(user)]
}

/// A correction is a small edit of the segment, not a rewrite or a dropped sentence.
fn plausible_fix(old: &str, new: &str) -> bool {
    let (a, b) = (old.chars().count(), new.chars().count());
    b > 0 && b <= a * 2 + 12 && b * 2 + 12 >= a
}

/// The model's corrections for `batch` as `(segment id, current text, corrected text)`.
/// Drops ids outside the batch, empty texts, implausible rewrites and changes that only
/// touch whitespace (nothing to show for those).
pub fn parse_fixes(v: &Value, batch: &[Segment]) -> Vec<(u32, String, String)> {
    let echoed_id = Regex::new(r"^\[\d+\]\s*").unwrap();
    let items = v["fixes"].as_array().or_else(|| v.as_array());
    let mut out: Vec<(u32, String, String)> = Vec::new();
    for item in items.into_iter().flatten() {
        let (Some(id), Some(text)) = (as_u32(&item["id"]), item["text"].as_str()) else {
            continue;
        };
        let Some(seg) = batch.iter().find(|s| s.id == id) else {
            continue;
        };
        let new = echoed_id.replace(text.trim(), "").trim().to_string();
        let old = seg.text.trim();
        let same_words = new.split_whitespace().eq(old.split_whitespace());
        if same_words || !plausible_fix(old, &new) || out.iter().any(|f| f.0 == id) {
            continue;
        }
        out.push((id, seg.text.clone(), new));
    }
    out
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
                original_text: None,
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
        // a closing tag mentioned before the document doesn't panic
        let h = extract_html("I will add </html> at the end, after <!doctype html>.").unwrap();
        assert_eq!(h, "<!doctype html>.");
    }

    #[test]
    fn citation_rewriter_handles_split_tokens() {
        let ids = vec!["uuid-a".to_string(), "uuid-b".to_string()];
        let mut r = CitationRewriter::default();
        let mut out = String::new();
        for d in ["Budget is low [R", "2#1", "4] and [R1#3-5]", " done [x"] {
            out.push_str(&r.push(d, &ids));
        }
        out.push_str(&r.finish(&ids));
        assert_eq!(
            out,
            "Budget is low [rec:uuid-b#14] and [rec:uuid-a#3-5] done [x"
        );
    }

    #[test]
    fn rewrites_citation_groups_and_keeps_unknown_aliases() {
        let ids = vec!["a".to_string(), "b".to_string()];
        assert_eq!(
            rewrite_citations("x [R1#2, R2#7-R2#9] y [R2#1-3] z [R9#1] [12]", &ids),
            "x [rec:a#2, rec:b#7-9] y [rec:b#1-3] z [R9#1] [12]"
        );
    }

    #[test]
    fn assembles_streamed_tool_calls() {
        let mut b = ReplyBuilder::default();
        let chunks = [
            r#"{"choices":[{"delta":{"content":"Let me look."}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","type":"function","function":{"name":"sea","arguments":""}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"rch"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"query\":"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"بودجه\"}"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":1,"id":"c2","function":{"name":"get_notes"}}]}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":10,"completion_tokens":5}}"#,
        ];
        let mut text = String::new();
        for c in chunks {
            if let Some(d) = b.apply(&serde_json::from_str(c).unwrap()).unwrap() {
                text.push_str(&d);
            }
        }
        let r = b.finish();
        assert_eq!(text, "Let me look.");
        assert_eq!(r.tool_calls.len(), 2);
        assert_eq!(r.tool_calls[0].name, "search");
        assert_eq!(r.tool_calls[0].arguments, r#"{"query":"بودجه"}"#);
        assert_eq!(r.tool_calls[1].arguments, "{}");
        assert_eq!(r.usage.unwrap().prompt_tokens, 10);
    }

    #[test]
    fn collects_streamed_reasoning_blocks() {
        let mut b = ReplyBuilder::default();
        for c in [
            r#"{"choices":[{"delta":{"reasoning":"thi","reasoning_details":[{"type":"reasoning.text","text":"thi","index":0,"format":"anthropic-claude-v1"}]}}]}"#,
            r#"{"choices":[{"delta":{"reasoning_details":[{"type":"reasoning.text","text":"nk","signature":null,"index":0}]}}]}"#,
            r#"{"choices":[{"delta":{"reasoning_details":[{"type":"reasoning.text","signature":"sig","index":0}]}}]}"#,
            r#"{"choices":[{"delta":{"reasoning_details":[{"type":"reasoning.encrypted","data":"abc","index":1}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"get_notes","arguments":"{}"}}]}}]}"#,
        ] {
            assert_eq!(b.apply(&serde_json::from_str(c).unwrap()).unwrap(), None);
        }
        let r = b.finish();
        assert_eq!(
            r.reasoning_details,
            vec![
                serde_json::json!({"type":"reasoning.text","text":"think","signature":"sig","index":0,"format":"anthropic-claude-v1"}),
                serde_json::json!({"type":"reasoning.encrypted","data":"abc","index":1}),
            ]
        );
        let m = Msg {
            reasoning_details: r.reasoning_details,
            ..Msg::tool_calls("", r.tool_calls)
        };
        let v = serde_json::to_value(m).unwrap();
        assert_eq!(v["reasoning_details"][1]["data"], "abc");
    }

    #[test]
    fn serializes_tool_messages() {
        let call = ToolCall {
            id: "c1".into(),
            name: "search".into(),
            arguments: "{}".into(),
        };
        let a = serde_json::to_value(Msg::tool_calls("", vec![call])).unwrap();
        assert!(a["content"].is_null());
        assert_eq!(a["tool_calls"][0]["function"]["name"], "search");
        let t = serde_json::to_value(Msg::tool_result("c1", "ok")).unwrap();
        assert_eq!(t["role"], "tool");
        assert_eq!(t["tool_call_id"], "c1");
        assert!(a.get("reasoning_details").is_none());
        let u = serde_json::to_value(Msg::user("hi")).unwrap();
        assert_eq!(u, serde_json::json!({"role": "user", "content": "hi"}));
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
            tools: true,
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
    fn fix_batches_cover_the_transcript() {
        let s = segs(200);
        let b = fix_batches(&s);
        assert_eq!(b.first().unwrap().start, 0);
        assert_eq!(b.last().unwrap().end, 200);
        assert!(b.windows(2).all(|w| w[0].end == w[1].start));
        assert!(b.iter().all(|r| r.len() <= FIX_BATCH_SEGMENTS));
        assert_eq!(b.len(), 3);
        assert!(fix_batches(&[]).is_empty());

        let text = |m: &Msg| match &m.content {
            Content::Text(t) => t.clone(),
            Content::Parts(_) => panic!("fix prompts are plain text"),
        };
        let user = text(&fix_messages("t", Some("خلاصه"), &s, b[1].clone())[1]);
        assert!(user.contains("خلاصه"));
        assert!(user.contains(
            "Context before (don't correct):\n[77] جمله 77\n[78] جمله 78\n[79] جمله 79\n"
        ));
        assert!(user.contains("Segments to correct:\n[80] جمله 80\n"));
        assert!(user.contains("Context after (don't correct):\n[160] جمله 160\n"));
        assert!(!text(&fix_messages("t", None, &s, b[0].clone())[1]).contains("Context before"));
    }

    #[test]
    fn parses_and_filters_fixes() {
        let batch = segs(5);
        let v: Value = serde_json::from_str(
            r#"{"fixes":[
                {"id":1,"text":"[1] جمله یک"},
                {"id":"2","text":" جمله  2 "},
                {"id":3,"text":""},
                {"id":4,"text":"این جمله را مدل کاملاً بازنویسی کرده و خیلی طولانی‌تر شده است"},
                {"id":9,"text":"خارج از دسته"},
                {"id":1,"text":"تکراری"},
                {"text":"بدون شناسه"}
            ]}"#,
        )
        .unwrap();
        assert_eq!(
            parse_fixes(&v, &batch),
            vec![(1, "جمله 1".to_string(), "جمله یک".to_string())]
        );
        let bare: Value = serde_json::from_str(r#"[{"id":0,"text":"جمله ۰"}]"#).unwrap();
        assert_eq!(parse_fixes(&bare, &batch).len(), 1);
        assert!(parse_fixes(&serde_json::json!({"fixes": []}), &batch).is_empty());
    }

    #[test]
    fn transcript_lines_truncate() {
        let t = transcript_lines(&segs(3), 1000);
        assert!(t.starts_with("[0] (0:00) جمله 0"));
        assert!(transcript_lines(&segs(100), 50).contains("truncated"));
    }
}
