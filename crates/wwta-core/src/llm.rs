//! OpenRouter client (streaming chat completions with tool calls, embeddings, model list),
//! the analysis prompt and citation rewriting. Q&A and docs run as agents, see [`crate::agent`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use futures_util::StreamExt;
use regex::Regex;
use serde::Serialize;
use serde_json::{json, Value};

use crate::text::fmt_time;
use crate::types::{Analysis, Chapter, LlmModelInfo, Segment, Tag};

pub const BASE_URL: &str = "https://openrouter.ai/api/v1";

/// One chat message in OpenAI format. Assistant messages may carry tool calls; `tool`
/// messages answer one of them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Msg {
    pub role: String,
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub tool_call_id: Option<String>,
}

impl Msg {
    fn new(role: &str, c: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: c.into(),
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

    fn finish(mut self) -> Result<Reply> {
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
        if self.reply.content.is_empty() && calls.is_empty() {
            bail!("the model returned an empty answer");
        }
        Ok(self.reply)
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
            "reasoning": { "exclude": true },
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
        reply.finish()
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
    let v: Value = http
        .get(format!("{BASE_URL}/models"))
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
        // Ask and Docs are tool-using agents
        .filter(|m| {
            let params = &m["supported_parameters"];
            params
                .as_array()
                .is_none_or(|a| a.iter().any(|x| x == "tools"))
        })
        .map(|m| LlmModelInfo {
            id: m["id"].as_str().unwrap_or_default().to_string(),
            name: m["name"].as_str().unwrap_or_default().to_string(),
            context_length: m["context_length"].as_u64(),
            prompt_price: price(&m["pricing"]["prompt"]),
            completion_price: price(&m["pricing"]["completion"]),
        })
        .filter(|m| !m.id.is_empty())
        .collect();
    models.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(models)
}

// ------------------------------------------------------------------ prompts

pub fn language_instruction(ai_language: &str) -> &'static str {
    match ai_language {
        "en" => "Write every piece of prose you produce (titles, summaries, tags, answers) in English, even though the transcript is in another language.",
        "fa" => "Write every piece of prose you produce (titles, summaries, tags, answers) in Persian (فارسی).",
        _ => "Write every piece of prose you produce in the same language as the transcript (usually Persian/فارسی), unless the user writes to you in another language.",
    }
}

/// Transcript lines with stable ids: `[12] (03:21) text`.
pub fn transcript_lines(segs: &[Segment], max_chars: usize) -> String {
    let mut out = String::new();
    for s in segs {
        let line = format!("[{}] ({}) {}\n", s.id, fmt_time(s.start), s.text.trim());
        if out.len() + line.len() > max_chars {
            out.push_str("[…transcript truncated…]\n");
            break;
        }
        out.push_str(&line);
    }
    out
}

const MAX_TRANSCRIPT_CHARS: usize = 900_000;

pub fn analysis_messages(title: &str, segs: &[Segment], ai_language: &str) -> Vec<Msg> {
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
         and reusable across recordings (prefer general topics over one-off phrases).",
        lang = language_instruction(ai_language)
    );
    let user = format!(
        "Recording: {title}\nSegments: 0..{last}\n\nTranscript:\n{}",
        transcript_lines(segs, MAX_TRANSCRIPT_CHARS)
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
        let r = b.finish().unwrap();
        assert_eq!(text, "Let me look.");
        assert_eq!(r.tool_calls.len(), 2);
        assert_eq!(r.tool_calls[0].name, "search");
        assert_eq!(r.tool_calls[0].arguments, r#"{"query":"بودجه"}"#);
        assert_eq!(r.tool_calls[1].arguments, "{}");
        assert_eq!(r.usage.unwrap().prompt_tokens, 10);
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

    #[test]
    fn transcript_lines_truncate() {
        let t = transcript_lines(&segs(3), 1000);
        assert!(t.starts_with("[0] (0:00) جمله 0"));
        assert!(transcript_lines(&segs(100), 50).contains("truncated"));
    }
}
