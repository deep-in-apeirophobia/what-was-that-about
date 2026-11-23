//! The tool-using agent behind Ask and Docs. Instead of getting whole transcripts pasted into
//! the prompt, the model looks things up with a few simple tools ([`tools`]) and, for docs,
//! loads a design guide ([`skills`]). The context is kept under a token budget: old tool
//! output is dropped first, then earlier conversation is summarized.

pub mod draft;
pub mod skills;
pub mod tools;

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{anyhow, bail, Result};
use regex::Regex;
use serde_json::Value;

use crate::llm::{language_instruction, ChatRequest, Client, Content, Msg, Part, ToolCall, Usage};
use crate::text::fmt_time;

/// Compact the context once it is estimated to be larger than this many tokens.
pub const CONTEXT_BUDGET: usize = 256_000;
/// Tool rounds per request; after that the model has to answer with what it found.
const MAX_ROUNDS: u32 = 24;
/// Keep the two newest tool rounds during doc runs, so each read survives its next edit.
pub const DOC_KEEP_ROUNDS: usize = 2;
const PRUNE_MIN_CHARS: usize = 400;
const PRUNE_ARGS_CHARS: usize = 1_000;
const KEEP_ROUNDS_ON_FOLD: usize = 2;
/// Earlier messages kept word for word when the rest of the conversation is summarized.
const KEEP_RECENT: usize = 4;
/// A single tool result is cut beyond this (the tools page long transcripts themselves).
const MAX_TOOL_CHARS: usize = 200_000;
/// Tokens per character before the first reply tells us better (Persian runs ~2.5 chars/token).
const DEFAULT_RATIO: f64 = 0.4;
/// The budget never shrinks below this after context-length errors.
pub const MIN_BUDGET: usize = 8_000;
/// Output tokens allowed for one summary (reasoning models spend some of it thinking).
const SUMMARY_TOKENS: u32 = 6_000;

const SUMMARY_PROMPT: &str = "You compress a conversation so it can go on without the original \
messages. Write a summary of at most about 600 words that keeps: what the user asked and wants, \
the answers given with their citations exactly as written (like [12] or [R3#14]), the facts and \
names that came up, and open questions. Write it in the language of the conversation. Reply with \
the summary only.";

/// Put after a tool result that was cut to fit the context.
pub(crate) const CUT_NOTE: &str =
    "\n[…the rest was cut to fit the context; ask for a smaller part.]";

const ANSWER_NOW: &str = "You have used all your tool calls. Answer now with what you found; \
say what you couldn't check.";

/// What the agent needs from the app: the tools it may call, and where streamed text goes.
pub trait Host: Send {
    /// OpenAI-format function definitions offered to the model.
    fn tools(&self) -> Vec<Value>;
    /// Run one tool call; the returned text is what the model sees.
    fn run_tool(&mut self, call: &ToolCall) -> impl Future<Output = String> + Send;
    /// Answer text as it streams.
    fn on_text(&mut self, delta: &str);
    /// The text streamed since the last call was the model thinking aloud before calling
    /// tools, not the answer: discard it.
    fn on_reset(&mut self);
    /// A tool call ended the task: stop without asking for an answer.
    fn finished(&self) -> bool {
        false
    }
    /// Continue after an early answer; the host limits how often.
    fn follow_up(&mut self, _answer: &str) -> Option<String> {
        None
    }
    /// The result of `call_id` was cut to fit the context before the model saw all of it;
    /// `kept` is what it will see.
    fn on_cut(&mut self, _call_id: &str, _kept: &str) {}
}

/// What the model sees: the system prompt (plus a summary of older conversation), earlier
/// messages, and the current turn (the request, then tool calls and their results).
pub struct Context {
    system: String,
    pub summary: Option<String>,
    history: Vec<Msg>,
    turn: Vec<Msg>,
    /// How many of the initial history messages have been folded into `summary`.
    pub folded: usize,
    pub budget: usize,
    /// Prune output older than the newest N rounds before each request (docs); None keeps
    /// output until the context overflows (chat).
    pub keep_rounds: Option<usize>,
    /// Tool results cut since the host last heard about it: (call id, text kept).
    cut: Vec<(String, String)>,
    /// Estimated tokens per character, calibrated from the usage the API reports.
    ratio: f64,
}

fn content_chars(c: &Content) -> usize {
    match c {
        Content::Text(t) => t.chars().count(),
        Content::Parts(parts) => parts
            .iter()
            .map(|p| match p {
                Part::Text { text } => text.chars().count(),
                // images and audio cost tokens by their size, not by their base64 length
                media => (media.tokens() as f64 / DEFAULT_RATIO) as usize,
            })
            .sum(),
    }
}

/// Length of a tool result (always plain text).
fn text_len(c: &Content) -> usize {
    match c {
        Content::Text(t) => t.len(),
        Content::Parts(_) => 0,
    }
}

fn msg_chars(m: &Msg) -> usize {
    let calls: usize = m
        .tool_calls
        .iter()
        .map(|c| c.name.len() + c.arguments.chars().count() + 16)
        .sum();
    let reasoning: usize = m
        .reasoning_details
        .iter()
        .map(|r| r.to_string().len())
        .sum();
    content_chars(&m.content) + calls + reasoning + 8
}

impl Context {
    pub fn new(
        system: impl Into<String>,
        summary: Option<String>,
        history: Vec<Msg>,
        request: Msg,
    ) -> Self {
        Self {
            system: system.into(),
            summary,
            history,
            turn: vec![request],
            folded: 0,
            budget: CONTEXT_BUDGET,
            keep_rounds: None,
            cut: vec![],
            ratio: DEFAULT_RATIO,
        }
    }

    pub fn messages(&self) -> Vec<Msg> {
        let mut system = self.system.clone();
        if let Some(s) = &self.summary {
            system.push_str("\n\nSummary of the earlier conversation:\n");
            system.push_str(s);
        }
        std::iter::once(Msg::system(system))
            .chain(self.history.iter().cloned())
            .chain(self.turn.iter().cloned())
            .collect()
    }

    fn tokens_of(&self, chars: usize) -> usize {
        (chars as f64 * self.ratio).ceil() as usize
    }

    fn history_chars(&self) -> usize {
        let summary = self.summary.as_deref().map_or(0, |s| s.chars().count());
        summary + self.history.iter().map(msg_chars).sum::<usize>()
    }

    /// Estimated size of the whole context in tokens.
    pub fn tokens(&self) -> usize {
        let chars = self.system.chars().count()
            + self.history_chars()
            + self.turn.iter().map(msg_chars).sum::<usize>();
        self.tokens_of(chars)
    }

    fn history_tokens(&self) -> usize {
        self.tokens_of(self.history_chars())
    }

    /// Learn the tokens-per-character ratio from a request of `chars` characters.
    fn calibrate(&mut self, prompt_tokens: u64, chars: usize) {
        // small prompts are dominated by the tool definitions
        if chars >= 20_000 && prompt_tokens > 0 {
            self.ratio = (prompt_tokens as f64 / chars as f64).clamp(0.1, 1.5);
        }
    }

    /// Replace the output of all but the latest tool round with a short note.
    fn drop_old_tool_output(&mut self) {
        let Some(latest) = self.turn.iter().rposition(|m| !m.tool_calls.is_empty()) else {
            return;
        };
        for i in 0..latest {
            if self.turn[i].role != "tool" || text_len(&self.turn[i].content) <= 400 {
                continue;
            }
            let id = self.turn[i].tool_call_id.clone().unwrap_or_default();
            let what = self.turn[..i]
                .iter()
                .flat_map(|m| &m.tool_calls)
                .find(|c| c.id == id)
                .map(|c| format!("{}({})", c.name, clip(&c.arguments, 120)))
                .unwrap_or_else(|| "a tool".into());
            self.turn[i].content = Content::Text(format!(
                "[The output of {what} was removed to save space. Call the tool again if you still need it.]"
            ));
        }
    }

    /// Prune old, large read results and call arguments, counting each replacement once.
    fn prune_rounds(&mut self, keep: usize) -> u32 {
        let rounds: Vec<_> = self
            .turn
            .iter()
            .enumerate()
            .filter(|(_, m)| !m.tool_calls.is_empty())
            .map(|(i, _)| i)
            .collect();
        let old = rounds.len().saturating_sub(keep);
        let mut pruned = 0;
        for (n, &start) in rounds.iter().take(old).enumerate() {
            let end = rounds.get(n + 1).copied().unwrap_or(self.turn.len());
            let calls = self.turn[start].tool_calls.clone();
            for m in &mut self.turn[start + 1..end] {
                if m.role != "tool" || text_len(&m.content) <= PRUNE_MIN_CHARS {
                    continue;
                }
                let Some(call) = calls
                    .iter()
                    .find(|c| Some(&c.id) == m.tool_call_id.as_ref())
                else {
                    continue;
                };
                if matches!(
                    call.name.as_str(),
                    "read_transcript"
                        | "read_transcript_section"
                        | "read_draft"
                        | "read_doc"
                        | "search"
                        | "get_notes"
                        | "list_recordings"
                ) {
                    m.content = Content::Text(format!("[The output of {}({}) was removed to save space. Call the tool again if you still need it.]", call.name, clip(&call.arguments, 120)));
                    pruned += 1;
                }
            }
            for call in &mut self.turn[start].tool_calls {
                pruned += u32::from(stub_arguments(call));
            }
        }
        pruned
    }

    /// Replace the large arguments of every draft write and edit made so far, the newest
    /// included: the draft holds what they wrote (overflow only).
    fn prune_draft_arguments(&mut self) {
        for call in self.turn.iter_mut().flat_map(|m| &mut m.tool_calls) {
            if matches!(call.name.as_str(), "write_draft" | "edit_draft") {
                stub_arguments(call);
            }
        }
    }

    /// Remove whole old tool rounds and replace them with one cumulative note.
    fn fold_old_rounds(&mut self, keep: usize) -> usize {
        let rounds: Vec<_> = self
            .turn
            .iter()
            .enumerate()
            .filter(|(_, m)| !m.tool_calls.is_empty())
            .map(|(i, _)| i)
            .collect();
        let removed = rounds.len().saturating_sub(keep);
        if removed == 0 {
            return 0;
        }
        let end = rounds.get(removed).copied().unwrap_or(self.turn.len());
        let previous = self
            .turn
            .get(1)
            .filter(|m| m.role == "user")
            .map(|m| m.content.text())
            .filter(|s| s.starts_with('[') && s.contains("earlier tool rounds were removed"))
            .and_then(|s| {
                s.split_whitespace()
                    .next()?
                    .trim_start_matches('[')
                    .parse::<usize>()
                    .ok()
            })
            .unwrap_or(0);
        self.turn.drain(1..end);
        self.turn.insert(1, Msg::user(format!("[{} earlier tool rounds were removed to save space. Call a tool again if you need its output.]", previous + removed)));
        removed
    }

    /// Halve the largest tool outputs until the context fits `target` tokens.
    fn shrink_tool_output(&mut self, target: usize) {
        while self.tokens() > target {
            let Some(m) = self
                .turn
                .iter_mut()
                .filter(|m| m.role == "tool")
                .max_by_key(|m| text_len(&m.content))
            else {
                break;
            };
            let Content::Text(text) = &m.content else {
                break;
            };
            if text.len() < 4_000 {
                break;
            }
            let kept = format!("{}{CUT_NOTE}", clip(text, text.len() / 2));
            let id = m.tool_call_id.clone().unwrap_or_default();
            self.cut.retain(|(cut, _)| *cut != id);
            self.cut.push((id, kept.clone()));
            m.content = Content::Text(kept);
        }
    }
}

/// Replace tool-call arguments longer than [`PRUNE_ARGS_CHARS`] with a short, valid JSON
/// note. Returns whether it did.
fn stub_arguments(call: &mut ToolCall) -> bool {
    if call.arguments.len() <= PRUNE_ARGS_CHARS {
        return false;
    }
    let size = (call.arguments.len() + 512) / 1024;
    call.arguments = serde_json::json!({"removed": format!("the arguments ({size} KB) were removed to save space; the draft holds the result")}).to_string();
    true
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Stats {
    /// Model requests, not counting summaries.
    pub requests: u32,
    pub tool_calls: u32,
    pub compactions: u32,
    pub nudges: u32,
    pub pruned: u32,
}

pub struct Agent<'a> {
    client: &'a Client,
    model: &'a str,
    max_tokens: u32,
    /// Tool rounds before the final request with tools disabled.
    pub max_rounds: u32,
    cancel: &'a AtomicBool,
    pub ctx: Context,
    /// Summed over every request, including summaries.
    pub usage: Option<Usage>,
    pub stats: Stats,
}

impl<'a> Agent<'a> {
    pub fn new(
        client: &'a Client,
        model: &'a str,
        max_tokens: u32,
        cancel: &'a AtomicBool,
        ctx: Context,
    ) -> Self {
        Self {
            client,
            model,
            max_tokens,
            max_rounds: MAX_ROUNDS,
            cancel,
            ctx,
            usage: None,
            stats: Stats::default(),
        }
    }

    fn check_cancel(&self) -> Result<()> {
        if self.cancel.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        Ok(())
    }

    /// Let the model work until it answers; returns the answer text.
    pub async fn run(&mut self, host: &mut impl Host) -> Result<String> {
        let tools = host.tools();
        // leave most of the budget for this turn's tool output
        if self.ctx.history_tokens() > self.ctx.budget / 4 {
            self.compact().await?;
        }
        let mut rounds = 0;
        let mut retried = false;
        loop {
            self.check_cancel()?;
            let last = rounds >= self.max_rounds;
            if last {
                self.ctx.turn.push(Msg::user(ANSWER_NOW));
            }
            if let Some(keep) = self.ctx.keep_rounds {
                self.stats.pruned += self.ctx.prune_rounds(keep);
            }
            if self.ctx.tokens() > self.ctx.budget {
                self.compact().await?;
            }
            for (id, kept) in std::mem::take(&mut self.ctx.cut) {
                host.on_cut(&id, &kept);
            }
            let messages = self.ctx.messages();
            let chars: usize = messages.iter().map(msg_chars).sum();
            let req = ChatRequest {
                model: self.model,
                messages: &messages,
                tools: &tools,
                tool_choice: last.then_some("none"),
                max_tokens: self.max_tokens,
            };
            let reply = match self
                .client
                .chat(&req, self.cancel, &mut |d: &str| host.on_text(d))
                .await
            {
                Ok(r) => r,
                Err(e) if !retried && is_context_overflow(&e) => {
                    log::warn!("context overflow ({e:#}); compacting and retrying");
                    retried = true;
                    host.on_reset();
                    if last {
                        self.ctx.turn.pop();
                    }
                    self.shrink_budget(&e, self.max_tokens);
                    self.compact().await?;
                    continue;
                }
                Err(e) => return Err(explain_error(e, self.model)),
            };
            retried = false;
            self.stats.requests += 1;
            add_usage(&mut self.usage, reply.usage);
            if let Some(u) = reply.usage {
                self.ctx.calibrate(u.prompt_tokens, chars);
            }
            if reply.tool_calls.is_empty() || last {
                if !last {
                    if let Some(message) = host.follow_up(&reply.content) {
                        host.on_reset();
                        self.stats.nudges += 1;
                        // some providers reject an assistant message with no content
                        if !reply.content.trim().is_empty() {
                            self.ctx.turn.push(Msg::assistant(reply.content));
                        }
                        self.ctx.turn.push(Msg::user(message));
                        continue;
                    }
                }
                return Ok(reply.content);
            }
            host.on_reset();
            rounds += 1;
            self.ctx.turn.push(Msg {
                reasoning_details: reply.reasoning_details,
                ..Msg::tool_calls(reply.content.clone(), reply.tool_calls.clone())
            });
            for call in &reply.tool_calls {
                self.check_cancel()?;
                self.stats.tool_calls += 1;
                let out = host.run_tool(call).await;
                self.ctx
                    .turn
                    .push(Msg::tool_result(&call.id, clip(&out, MAX_TOOL_CHARS)));
            }
            if host.finished() {
                return Ok(reply.content);
            }
        }
    }

    /// After a context-length error, fit the budget to the model (its stated limit minus the
    /// output tokens asked for). Returns false when the budget can't shrink any further.
    fn shrink_budget(&mut self, e: &anyhow::Error, output_tokens: u32) -> bool {
        let lower = match overflow_limit(e) {
            Some(limit) => limit.saturating_sub(output_tokens as usize) * 9 / 10,
            None => self.ctx.tokens().min(self.ctx.budget) * 2 / 3,
        };
        let budget = lower.max(MIN_BUDGET).min(self.ctx.budget);
        let shrunk = budget < self.ctx.budget;
        self.ctx.budget = budget;
        shrunk
    }

    /// Bring the context under half the budget: drop old tool output, summarize earlier
    /// conversation when it takes more than a quarter of the budget, and as a last resort
    /// cut the newest tool output.
    async fn compact(&mut self) -> Result<()> {
        self.stats.compactions += 1;
        let target = self.ctx.budget / 2;
        self.ctx.drop_old_tool_output();
        self.ctx.prune_draft_arguments();
        if self.ctx.tokens() > target {
            self.ctx.fold_old_rounds(KEEP_ROUNDS_ON_FOLD);
        }
        if self.ctx.history_tokens() > self.ctx.budget / 4 {
            self.summarize_history().await?;
        }
        self.ctx.shrink_tool_output(target);
        log::info!(
            "compacted the context to ~{} tokens (budget {})",
            self.ctx.tokens(),
            self.ctx.budget
        );
        Ok(())
    }

    /// Fold all but the last few history messages (and any earlier summary) into a summary,
    /// in batches small enough for the model's context.
    async fn summarize_history(&mut self) -> Result<()> {
        let len = self.ctx.history.len();
        let mut left = if len > KEEP_RECENT {
            len - KEEP_RECENT
        } else {
            len
        };
        while left > 0 {
            self.check_cancel()?;
            // bytes of conversation per summary request: a third of the budget
            let batch = (self.ctx.budget as f64 / 3.0 / self.ctx.ratio) as usize;
            let mut text = String::new();
            if let Some(s) = &self.ctx.summary {
                text.push_str(&format!(
                    "Summary of the conversation before this:\n{s}\n\n"
                ));
            }
            let mut take = 0;
            for m in &self.ctx.history[..left] {
                let content = m.content.text();
                let body = content.trim();
                if take > 0 && text.len() + body.len() + 16 > batch {
                    break;
                }
                let who = if m.role == "user" {
                    "User"
                } else {
                    "Assistant"
                };
                // the earlier summary rides on top of the batch; only a message bigger than a
                // whole batch is cut, since no request could hold it
                text.push_str(&format!("{who}: {}\n\n", clip(body, batch)));
                take += 1;
            }
            let msgs = [Msg::system(SUMMARY_PROMPT), Msg::user(text)];
            let (summary, u) = match self
                .client
                .complete(self.model, &msgs, false, SUMMARY_TOKENS)
                .await
            {
                Ok(r) => r,
                // the model's context is smaller than the budget assumed: retry smaller
                Err(e) if is_context_overflow(&e) && self.shrink_budget(&e, SUMMARY_TOKENS) => {
                    log::warn!("summary too large for the model ({e:#}); retrying smaller");
                    continue;
                }
                Err(e) => return Err(explain_error(e, self.model)),
            };
            add_usage(&mut self.usage, u);
            // never trade real history for an empty summary
            let summary = summary.trim();
            if summary.is_empty() {
                bail!("the model returned an empty summary of the earlier conversation; try again or pick another model");
            }
            self.ctx.summary = Some(summary.to_string());
            self.ctx.history.drain(..take);
            self.ctx.folded += take;
            left -= take;
        }
        Ok(())
    }
}

pub fn add_usage(acc: &mut Option<Usage>, u: Option<Usage>) {
    if let Some(u) = u {
        let a = acc.get_or_insert_with(Usage::default);
        a.prompt_tokens += u.prompt_tokens;
        a.completion_tokens += u.completion_tokens;
        a.cost = match (a.cost, u.cost) {
            (Some(x), Some(y)) => Some(x + y),
            (x, y) => x.or(y),
        };
    }
}

fn is_context_overflow(e: &anyhow::Error) -> bool {
    let m = format!("{e:#}").to_lowercase();
    [
        "context length",
        "context_length",
        "context window",
        "maximum context",
        "too many tokens",
        "prompt is too long",
        "input is too long",
    ]
    .iter()
    .any(|p| m.contains(p))
}

/// The model's context size, when the error message states it.
fn overflow_limit(e: &anyhow::Error) -> Option<usize> {
    static RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"(?i)(?:maximum context length is|context length of|context window of|maximum of)\s*([\d,]+)")
            .unwrap()
    });
    let m = format!("{e:#}");
    RE.captures(&m)?[1].replace(',', "").parse().ok()
}

fn explain_error(e: anyhow::Error, model: &str) -> anyhow::Error {
    let m = format!("{e:#}").to_lowercase();
    if m.contains("tool use") || m.contains("tool_use") || m.contains("support tools") {
        anyhow!("{model} can't use tools, which Ask and Docs need. Pick another model in Settings. ({e:#})")
    } else {
        e
    }
}

/// At most `max` bytes of `s`, cut at a character boundary.
pub(crate) fn clip(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

// ------------------------------------------------------------------ prompts

/// What the prompts say about the recording a chat or doc is about.
pub struct RecordingFacts<'a> {
    pub title: &'a str,
    pub duration_sec: Option<f64>,
    pub segment_count: u32,
    pub has_analysis: bool,
    pub notes: usize,
}

impl RecordingFacts<'_> {
    fn describe(&self) -> String {
        let length = self
            .duration_sec
            .map(fmt_time)
            .unwrap_or_else(|| "?".into());
        format!(
            "“{}” ({length}, segments 0–{})",
            self.title,
            self.segment_count.saturating_sub(1)
        )
    }
}

pub fn recording_chat_system(rec: &RecordingFacts, ai_language: &str) -> String {
    let analysis = if rec.has_analysis {
        "It has an AI summary and chapters (get_overview)."
    } else {
        "It hasn't been analyzed, so there is no summary: search or read the transcript."
    };
    let notes = match rec.notes {
        0 => String::new(),
        n => format!(" The user wrote {n} notes on it (get_notes)."),
    };
    format!(
        "You answer questions about one audio recording, {desc}. You read its automatic transcript \
         with tools. {lang}\n\n\
         How to work:\n\
         - Look things up before you answer. get_overview gives the big picture; search finds where \
         something is said; read_transcript_section reads a range of segments and read_transcript \
         reads from the start. Long transcripts come in parts of about 15 minutes, and each part \
         says where to continue; get_notes shows the user's own notes.\n\
         - {analysis}{notes}\n\
         - The user may attach images or voice messages; use them together with the transcript.\n\
         - Don't guess. If the recording doesn't say, tell the user.\n\n\
         How to answer:\n\
         - After each claim, cite the segments it comes from in square brackets, like [12] or [12-15]. \
         Citations become links that play the audio, so only use segment ids you saw.\n\
         - The transcript comes from speech recognition, so some words are wrong; read for the meaning.\n\
         - Be concise. Use markdown: short paragraphs or bullet points.",
        desc = rec.describe(),
        lang = language_instruction(ai_language),
    )
}

pub fn library_chat_system(recordings: usize, ai_language: &str) -> String {
    format!(
        "You answer questions about the user's library of {recordings} transcribed audio recordings. \
         You read them with tools. {lang}\n\n\
         How to work:\n\
         - search finds where a word or topic comes up in all recordings. list_recordings shows \
         what is in the library. get_overview, read_transcript_section, read_transcript and get_notes \
         read one recording.\n\
         - Recordings have short ids like R3. Transcript lines look like [R3#14] (recording R3, segment 14).\n\
         - Search before you answer, and read around the best results when you need more context. \
         If a search finds nothing useful, try other words.\n\
         - The user may attach images or voice messages; use them together with what you find.\n\
         - Don't guess. If the library doesn't say, tell the user and suggest what to search for.\n\n\
         How to answer:\n\
         - After each claim, cite the lines it comes from in exactly this form: [R3#14] or [R3#14-16]. \
         Citations become links that play the audio.\n\
         - Say which recording a point comes from (by its title) when it helps.\n\
         - Transcripts come from speech recognition, so some words are wrong; read for the meaning.\n\
         - Be concise. Use markdown: short paragraphs or bullet points.",
        lang = language_instruction(ai_language),
    )
}

pub fn doc_system(rec: &RecordingFacts, ai_language: &str) -> String {
    let guides: String = skills::loadable()
        .map(|s| format!("- {}: {}\n", s.name, s.description))
        .collect();
    format!(
        r#"You make small, polished, interactive HTML documents that help someone explore an audio
recording: mind maps, timelines, flashcards, quizzes, minutes, glossaries, tables… This one is
about {desc}. {lang}

You build the doc as a file with tools. Only the draft file counts: text in your answers is not
kept.

How to work:
1. Load the design guide closest to the request with load_skill (skip this when a guide is
   already included with the request).
2. get_overview for the big picture: chapters, topics, how many notes the user wrote.
3. write_draft with the complete file right away: the skeleton from the rules below, the
   styles, the script that renders the content, and the data list empty except for a marker
   comment at its end (like `// end: items`). It must already be a working doc with no items.
4. Read the transcript part by part: read_transcript first, then read_transcript_section from
   the segment each part tells you to continue from. After each part, add what you found to the
   data list with edit_draft (`old` = the marker comment, `new` = the new items followed by the
   same comment). Do this before reading the next part: earlier parts are removed from your
   context, and the draft is your only memory. get_notes when the user's notes matter.
5. When every part is read (each result says what is still unread), fix anything that needs it
   and call finish_doc. If it reports problems, fix them with edit_draft and call it again.

Edits: `old` is a few lines copied exactly from the draft, without line numbers, and must occur
only once. Don't read the draft after an edit; the confirmation tells you it applied. If you are
told you have used all your tool calls, answer with one sentence: the draft as it is will be used.

Guides you can load:
{guides}
{base}"#,
        desc = rec.describe(),
        lang = language_instruction(ai_language),
        base = skills::base(),
    )
}

/// The request for a new doc; `skill` names a guide to include right away (a template).
pub fn doc_request(prompt: &str, skill: Option<&str>) -> String {
    match skill.and_then(skills::get) {
        Some(s) => format!(
            "Make this doc: {prompt}\n\nFollow this guide (already loaded, no need for load_skill):\n\n{}",
            s.body
        ),
        None => format!("Make this doc: {prompt}"),
    }
}

/// Revise a loaded draft without pasting its HTML into the request.
pub fn doc_revision(prompt: &str, summary: &str, instruction: &str) -> String {
    format!(
        r#"The doc made for the request “{prompt}” is loaded as the draft ({summary}). Change it: {instruction}
Read the parts you need with read_draft (start with lines 1–200 to see its structure), make the
change with edit_draft, keep everything else as it is, then call finish_doc. If the change needs
new content from the recording, read the transcript part by part as usual."#
    )
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    use serde_json::json;

    use super::tools::{Scope, ToolBox};
    use super::*;
    use crate::db::Db;
    use crate::types::{now_iso, Recording, RecordingStatus, Segment};

    /// A canned HTTP response from the fake OpenRouter.
    struct Canned {
        status: u16,
        content_type: &'static str,
        body: String,
    }

    fn sse(chunks: &[Value]) -> Canned {
        let mut body: String = chunks.iter().map(|c| format!("data: {c}\n\n")).collect();
        body.push_str("data: [DONE]\n\n");
        Canned {
            status: 200,
            content_type: "text/event-stream",
            body,
        }
    }

    fn answer(text: &str) -> Canned {
        sse(&[
            json!({ "choices": [{ "delta": { "content": text } }] }),
            json!({ "choices": [], "usage": { "prompt_tokens": 100, "completion_tokens": 10, "cost": 0.001 } }),
        ])
    }

    fn calls(text: &str, calls: &[(&str, Value)]) -> Canned {
        let mut chunks = vec![json!({ "choices": [{ "delta": { "content": text } }] })];
        for (i, (name, args)) in calls.iter().enumerate() {
            chunks.push(json!({ "choices": [{ "delta": { "tool_calls": [{
                "index": i, "id": format!("call{i}"), "type": "function",
                "function": { "name": name, "arguments": args.to_string() },
            }] } }] }));
        }
        sse(&chunks)
    }

    fn completion(text: &str) -> Canned {
        Canned {
            status: 200,
            content_type: "application/json",
            body: json!({ "choices": [{ "message": { "content": text } }] }).to_string(),
        }
    }

    /// Serve `script` in order on a local port; returns the base URL and the request bodies.
    fn fake_openrouter(script: Vec<Canned>) -> (String, Arc<Mutex<Vec<Value>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(vec![]));
        let log = seen.clone();
        std::thread::spawn(move || {
            for canned in script {
                let (mut stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut len = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap();
                    }
                    if line == "\r\n" {
                        break;
                    }
                }
                let mut body = vec![0; len];
                reader.read_exact(&mut body).unwrap();
                log.lock()
                    .unwrap()
                    .push(serde_json::from_slice(&body).unwrap());
                let head = format!(
                    "HTTP/1.1 {} X\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    canned.status,
                    canned.content_type,
                    canned.body.len()
                );
                stream.write_all(head.as_bytes()).unwrap();
                stream.write_all(canned.body.as_bytes()).unwrap();
            }
        });
        (base, seen)
    }

    struct TestHost {
        db: Db,
        tools: ToolBox,
        text: String,
        resets: usize,
    }

    impl Host for TestHost {
        fn tools(&self) -> Vec<Value> {
            self.tools.definitions()
        }
        async fn run_tool(&mut self, call: &ToolCall) -> String {
            self.tools.run(&self.db, call, vec![])
        }
        fn on_text(&mut self, d: &str) {
            self.text.push_str(d);
        }
        fn on_reset(&mut self) {
            self.text.clear();
            self.resets += 1;
        }
        fn finished(&self) -> bool {
            self.tools.finished()
        }
        fn follow_up(&mut self, answer: &str) -> Option<String> {
            self.tools.follow_up(answer)
        }
        fn on_cut(&mut self, call_id: &str, kept: &str) {
            self.tools.on_cut(call_id, kept)
        }
    }

    fn host(scope: Scope) -> TestHost {
        let mut db = Db::open_in_memory().unwrap();
        db.insert_recording(&Recording {
            id: "uuid-a".into(),
            title: "جلسه".into(),
            source_path: String::new(),
            audio_path: String::new(),
            playback_wav_path: None,
            duration_sec: Some(30.0),
            created_at: now_iso(),
            updated_at: now_iso(),
            status: RecordingStatus::Ready,
            model_id: None,
            error: None,
            segment_count: 0,
            has_analysis: false,
            tags: vec![],
            blurb: None,
        })
        .unwrap();
        let long = "متن طولانی ".repeat(3_000);
        let segs: Vec<Segment> = ["سلام", "بودجه سال آینده کم است", &long]
            .iter()
            .enumerate()
            .map(|(i, t)| Segment {
                id: i as u32,
                start: i as f64 * 10.0,
                end: i as f64 * 10.0 + 9.0,
                text: t.to_string(),
                words: vec![],
                original_text: None,
            })
            .collect();
        db.replace_segments("uuid-a", "m", &segs).unwrap();
        TestHost {
            db,
            tools: ToolBox::new(scope, false),
            text: String::new(),
            resets: 0,
        }
    }

    const SKELETON: &str = concat!(
        "<!doctype html>\n<html>\n<style>/* ",
        "padding padding padding padding padding padding padding padding padding padding ",
        "padding padding padding padding padding padding padding padding padding padding ",
        "padding padding padding padding padding padding padding padding padding padding ",
        "padding padding padding padding padding padding padding padding padding padding ",
        "padding padding padding padding padding padding padding padding padding padding ",
        "padding padding padding padding padding padding padding padding padding padding ",
        "padding padding padding padding padding padding padding padding padding padding ",
        "padding padding padding padding padding padding padding padding padding padding ",
        "padding padding padding padding padding padding padding padding padding padding ",
        "padding padding padding padding padding padding padding padding padding padding ",
        "padding padding padding padding padding padding padding padding padding padding ",
        "padding padding padding padding padding padding padding padding padding padding ",
        "padding padding padding padding padding padding padding padding padding padding ",
        "padding padding padding padding padding padding padding padding padding padding ",
        "*/</style>\n<script>const ITEMS = [\n  // end: items\n];\nconst segments = WWTA.segments;</script>\n</html>",
    );

    fn doc_host() -> TestHost {
        let mut h = host(Scope::Recording("uuid-a".into()));
        let mut segments = h.db.segments("uuid-a").unwrap();
        segments[1].text.push_str(&" حرف".repeat(120));
        h.db.replace_segments("uuid-a", "m", &segments).unwrap();
        h.tools = ToolBox::new(Scope::Recording("uuid-a".into()), true);
        h
    }

    fn doc_context(request: &str) -> Context {
        let mut ctx = Context::new("system", None, vec![], Msg::user(request));
        ctx.keep_rounds = Some(DOC_KEEP_ROUNDS);
        ctx
    }

    const DOC: &str = "<!doctype html>\n<html>\n<title>Quiz</title>\n<script>const x = WWTA.segments;</script>\n</html>";

    #[tokio::test]
    async fn builds_a_doc_step_by_step() {
        let (base, seen) = fake_openrouter(vec![
            calls("", &[("get_overview", json!({}))]),
            calls("", &[("read_transcript", json!({}))]),
            calls("", &[("write_draft", json!({"html": SKELETON}))]),
            calls("", &[("read_transcript_section", json!({"from": 2}))]),
            calls(
                "",
                &[(
                    "edit_draft",
                    json!({"old": "  // end: items", "new": "  { q: \"x\", seg: 1 },\n  // end: items"}),
                )],
            ),
            calls("", &[("finish_doc", json!({}))]),
        ]);
        let client = Client::with_base("key", &base).unwrap();
        let cancel = AtomicBool::new(false);
        let mut h = doc_host();
        let mut agent = Agent::new(&client, "m", 32_000, &cancel, doc_context("quiz"));
        assert_eq!(agent.run(&mut h).await.unwrap(), "");
        assert!(h.tools.finished());
        assert_eq!(agent.stats.requests, 6);
        assert!(agent.stats.pruned > 0);
        let result = h.tools.doc_result("").unwrap();
        assert_eq!(result.source, "draft");
        assert!(result.html.contains("{ q: \"x\", seg: 1 }"));
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 6);
        let messages = seen[4]["messages"].as_array().unwrap();
        let outputs = messages
            .iter()
            .filter(|m| m["role"] == "tool")
            .collect::<Vec<_>>();
        assert!(outputs[1]["content"]
            .as_str()
            .unwrap()
            .starts_with("[The output of read_transcript({}) was removed"));
        assert!(outputs[3]["content"]
            .as_str()
            .unwrap()
            .starts_with("Segments 2–2 of 0–2:"));
        let messages = seen[5]["messages"].as_array().unwrap();
        let write = messages
            .iter()
            .find(|m| m["tool_calls"][0]["function"]["name"] == "write_draft")
            .unwrap();
        let args: Value = serde_json::from_str(
            write["tool_calls"][0]["function"]["arguments"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert!(args["removed"]
            .as_str()
            .unwrap()
            .contains("the draft holds the result"));
        let messages = seen[2]["messages"].as_array().unwrap();
        assert!(messages.last().unwrap()["content"]
            .as_str()
            .unwrap()
            .ends_with("Transcript read so far: 0–1 (66% of 0–2). Unread: 2."));
    }

    #[tokio::test]
    async fn an_empty_early_answer_is_not_replayed() {
        let (base, seen) = fake_openrouter(vec![
            answer(""),
            calls("", &[("write_draft", json!({"html": SKELETON}))]),
            calls("", &[("finish_doc", json!({}))]),
        ]);
        let client = Client::with_base("key", &base).unwrap();
        let cancel = AtomicBool::new(false);
        let mut h = doc_host();
        let mut agent = Agent::new(&client, "m", 32_000, &cancel, doc_context("quiz"));
        agent.run(&mut h).await.unwrap();
        assert_eq!(agent.stats.nudges, 1);
        let seen = seen.lock().unwrap();
        let messages = seen[1]["messages"].as_array().unwrap();
        assert!(messages.iter().all(|m| m["role"] != "assistant"));
        assert!(messages.last().unwrap()["content"]
            .as_str()
            .unwrap()
            .starts_with("You haven't written the doc yet."));
    }

    #[tokio::test]
    async fn nudges_a_model_that_answers_too_early() {
        let (base, seen) = fake_openrouter(vec![
            calls("", &[("write_draft", json!({"html": SKELETON}))]),
            answer("Done."),
            calls("", &[("finish_doc", json!({}))]),
        ]);
        let client = Client::with_base("key", &base).unwrap();
        let cancel = AtomicBool::new(false);
        let mut h = doc_host();
        let mut agent = Agent::new(&client, "m", 32_000, &cancel, doc_context("quiz"));
        agent.run(&mut h).await.unwrap();
        assert_eq!((agent.stats.nudges, agent.stats.requests), (1, 3));
        assert_eq!(h.tools.doc_result("").unwrap().source, "draft");
        let seen = seen.lock().unwrap();
        let messages = seen[2]["messages"].as_array().unwrap();
        assert_eq!(messages[messages.len() - 2]["content"], "Done.");
        assert_eq!(messages[messages.len() - 2]["role"], "assistant");
        assert_eq!(messages.last().unwrap()["role"], "user");
        assert!(messages.last().unwrap()["content"]
            .as_str()
            .unwrap()
            .starts_with("The doc isn't finished."));
    }

    #[tokio::test]
    async fn accepts_html_dumped_in_the_answer() {
        let html = format!("```html\n{DOC}\n```");
        let (base, _) = fake_openrouter(vec![answer(&html)]);
        let client = Client::with_base("key", &base).unwrap();
        let cancel = AtomicBool::new(false);
        let mut h = doc_host();
        let mut agent = Agent::new(&client, "m", 32_000, &cancel, doc_context("quiz"));
        let answer = agent.run(&mut h).await.unwrap();
        assert_eq!(h.tools.doc_result(&answer).unwrap().source, "answer");
        assert_eq!(agent.stats.nudges, 0);
    }

    #[tokio::test]
    async fn takes_the_draft_at_the_round_limit() {
        let (base, seen) = fake_openrouter(vec![
            calls("", &[("write_draft", json!({"html": SKELETON}))]),
            calls(
                "",
                &[(
                    "edit_draft",
                    json!({"old": "  // end: items", "new": "  1,\n  // end: items"}),
                )],
            ),
            answer("I'd continue…"),
        ]);
        let client = Client::with_base("key", &base).unwrap();
        let cancel = AtomicBool::new(false);
        let mut h = doc_host();
        let mut agent = Agent::new(&client, "m", 32_000, &cancel, doc_context("quiz"));
        agent.max_rounds = 2;
        let answer = agent.run(&mut h).await.unwrap();
        let result = h.tools.doc_result(&answer).unwrap();
        assert_eq!(result.source, "draft");
        assert!(result.html.contains("  1,\n  // end: items"));
        let seen = seen.lock().unwrap();
        assert_eq!(seen[2]["tool_choice"], "none");
        assert_eq!(
            seen[2]["messages"].as_array().unwrap().last().unwrap()["content"],
            ANSWER_NOW
        );
        assert_eq!(agent.stats.nudges, 0);
    }

    #[tokio::test]
    async fn drops_draft_arguments_after_a_context_error() {
        let overflow = Canned {
            status: 400,
            content_type: "application/json",
            body: json!({ "error": { "message": "This endpoint's maximum context length is 30000 tokens." } }).to_string(),
        };
        let (base, seen) = fake_openrouter(vec![
            calls("", &[("write_draft", json!({"html": SKELETON}))]),
            overflow,
            calls("", &[("finish_doc", json!({}))]),
        ]);
        let client = Client::with_base("key", &base).unwrap();
        let cancel = AtomicBool::new(false);
        let mut h = doc_host();
        let mut agent = Agent::new(&client, "m", 32_000, &cancel, doc_context("quiz"));
        agent.run(&mut h).await.unwrap();
        assert_eq!(h.tools.doc_result("").unwrap().html, SKELETON);
        let seen = seen.lock().unwrap();
        let arguments = |i: usize| {
            seen[i]["messages"].as_array().unwrap()[2]["tool_calls"][0]["function"]["arguments"]
                .as_str()
                .unwrap()
                .to_string()
        };
        // the newest round is kept by the rolling prune, so only the retry drops it
        assert_eq!(arguments(1), json!({"html": SKELETON}).to_string());
        assert!(arguments(2).contains("the draft holds the result"));
    }

    #[tokio::test]
    async fn coverage_leaves_out_what_compaction_cut() {
        let (base, seen) = fake_openrouter(vec![
            calls("", &[("read_transcript_section", json!({"from": 2}))]),
            calls("", &[("write_draft", json!({"html": SKELETON}))]),
            calls("", &[("finish_doc", json!({}))]),
        ]);
        let client = Client::with_base("key", &base).unwrap();
        let cancel = AtomicBool::new(false);
        let mut h = doc_host();
        let mut ctx = doc_context("quiz");
        // segment 2 is one long line; this budget makes compaction cut it
        ctx.budget = 4_000;
        let mut agent = Agent::new(&client, "m", 32_000, &cancel, ctx);
        agent.run(&mut h).await.unwrap();
        let seen = seen.lock().unwrap();
        let tool = |i: usize, n: usize| {
            seen[i]["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|m| m["role"] == "tool")
                .nth(n)
                .unwrap()["content"]
                .as_str()
                .unwrap()
                .to_string()
        };
        assert!(tool(1, 0).ends_with(CUT_NOTE));
        assert!(tool(2, 1).ends_with("Transcript read so far: nothing of 0–2."));
        assert_eq!(h.tools.doc_stats().transcript_read_pct, 0);
    }

    #[tokio::test]
    async fn revises_in_place() {
        let (base, seen) = fake_openrouter(vec![
            calls("", &[("read_draft", json!({"from": 1, "to": 200}))]),
            calls(
                "",
                &[(
                    "edit_draft",
                    json!({"old": "<title>Quiz</title>", "new": "<title>Quiz 2</title>"}),
                )],
            ),
            calls("", &[("finish_doc", json!({}))]),
        ]);
        let client = Client::with_base("key", &base).unwrap();
        let cancel = AtomicBool::new(false);
        let mut h = doc_host();
        h.tools.set_draft(DOC);
        let mut agent = Agent::new(
            &client,
            "m",
            32_000,
            &cancel,
            doc_context(&doc_revision(
                "a quiz",
                &h.tools.draft().summary(),
                "Rename it to Quiz 2",
            )),
        );
        let answer = agent.run(&mut h).await.unwrap();
        assert_eq!(
            h.tools.doc_result(&answer).unwrap().html,
            DOC.replace("<title>Quiz</title>", "<title>Quiz 2</title>")
        );
        assert!(!seen.lock().unwrap()[0]["messages"]
            .to_string()
            .contains("<title>Quiz</title>"));
    }

    #[tokio::test]
    async fn folds_old_rounds_when_the_turn_overflows() {
        let mut script = (0..5)
            .map(|_| calls(&"x".repeat(3_000), &[("get_notes", json!({}))]))
            .collect::<Vec<_>>();
        script.push(answer("ok"));
        let (base, seen) = fake_openrouter(script);
        let client = Client::with_base("key", &base).unwrap();
        let cancel = AtomicBool::new(false);
        let mut h = host(Scope::Recording("uuid-a".into()));
        let mut ctx = Context::new("system", None, vec![], Msg::user("question"));
        ctx.budget = 6_000;
        let mut agent = Agent::new(&client, "m", 1_000, &cancel, ctx);
        assert_eq!(agent.run(&mut h).await.unwrap(), "ok");
        assert!(agent.ctx.tokens() <= 6_000);
        let seen = seen.lock().unwrap();
        let messages = seen.last().unwrap()["messages"].as_array().unwrap();
        let note = messages[2]["content"].as_str().unwrap();
        assert!(note.starts_with('[') && note.contains("earlier tool rounds were removed"));
        assert!(
            messages
                .iter()
                .filter(|m| m.get("tool_calls").is_some())
                .count()
                < 5
        );
    }

    #[test]
    fn pruning_preserves_guides_and_reasoning_and_is_idempotent() {
        let mut ctx = doc_context("question");
        for (i, name) in [
            "get_overview",
            "load_skill",
            "read_draft",
            "edit_draft",
            "get_notes",
        ]
        .iter()
        .enumerate()
        {
            ctx.turn.push(Msg {
                reasoning_details: vec![json!({"data": "opaque"})],
                ..Msg::tool_calls(
                    "",
                    vec![ToolCall {
                        id: i.to_string(),
                        name: name.to_string(),
                        arguments: json!({"text": "x".repeat(2_000)}).to_string(),
                    }],
                )
            });
            ctx.turn
                .push(Msg::tool_result(&i.to_string(), "x".repeat(2_000)));
        }
        assert_eq!(ctx.prune_rounds(2), 4);
        assert_eq!(ctx.prune_rounds(2), 0);
        assert_eq!(ctx.turn[2].content.text().len(), 2_000);
        assert_eq!(ctx.turn[4].content.text().len(), 2_000);
        assert!(ctx.turn[6]
            .content
            .text()
            .starts_with("[The output of read_draft("));
        assert_eq!(ctx.turn[5].reasoning_details[0]["data"], "opaque");
        assert_eq!(ctx.turn[8].content.text().len(), 2_000);
    }

    #[test]
    fn folding_accumulates_counts_and_removes_intervening_nudges() {
        let mut ctx = doc_context("question");
        for i in 0..5 {
            ctx.turn.push(Msg::tool_calls(
                "",
                vec![ToolCall {
                    id: i.to_string(),
                    name: "get_notes".into(),
                    arguments: "{}".into(),
                }],
            ));
            ctx.turn.push(Msg::tool_result(&i.to_string(), "notes"));
            if i == 1 {
                ctx.turn.push(Msg::assistant("Done."));
                ctx.turn.push(Msg::user("Continue."));
            }
        }
        assert_eq!(ctx.fold_old_rounds(3), 2);
        assert!(!ctx.turn.iter().any(|m| m.content.text() == "Continue."));
        assert_eq!(ctx.fold_old_rounds(2), 1);
        assert!(ctx.turn[1]
            .content
            .text()
            .starts_with("[3 earlier tool rounds"));
        assert_eq!(ctx.fold_old_rounds(2), 0);
        assert_eq!(ctx.turn[0].content.text(), "question");
        assert_eq!(
            ctx.turn.iter().filter(|m| !m.tool_calls.is_empty()).count(),
            2
        );
    }

    #[tokio::test]
    async fn retries_context_errors_again_after_a_successful_round() {
        let overflow = || Canned {
            status: 400,
            content_type: "application/json",
            body: json!({"error": {"message": "maximum context length is 30000 tokens"}})
                .to_string(),
        };
        let (base, _) = fake_openrouter(vec![
            overflow(),
            calls("", &[("get_notes", json!({}))]),
            overflow(),
            answer("ok"),
        ]);
        let client = Client::with_base("key", &base).unwrap();
        let cancel = AtomicBool::new(false);
        let mut h = host(Scope::Recording("uuid-a".into()));
        let mut agent = Agent::new(
            &client,
            "m",
            1_000,
            &cancel,
            Context::new("s", None, vec![], Msg::user("q")),
        );
        assert_eq!(agent.run(&mut h).await.unwrap(), "ok");
        assert_eq!(agent.stats.compactions, 2);
        assert_eq!(agent.stats.requests, 2);
    }

    #[tokio::test]
    async fn an_empty_answer_is_returned_not_an_error() {
        let (base, _) = fake_openrouter(vec![answer("")]);
        let client = Client::with_base("key", &base).unwrap();
        let cancel = AtomicBool::new(false);
        let mut h = host(Scope::Recording("uuid-a".into()));
        let mut agent = Agent::new(
            &client,
            "m",
            1_000,
            &cancel,
            Context::new("s", None, vec![], Msg::user("q")),
        );
        assert_eq!(agent.run(&mut h).await.unwrap(), "");
    }

    #[tokio::test]
    async fn runs_tools_until_the_model_answers() {
        let mut first = calls("Let me search.", &[("search", json!({ "query": "بودجه" }))]);
        let thought = json!({ "choices": [{ "delta": { "reasoning_details": [
            { "type": "reasoning.encrypted", "data": "opaque", "index": 0, "format": "google-gemini-v1" }
        ] } }] });
        first.body = format!("data: {thought}\n\n{}", first.body);
        let (base, seen) = fake_openrouter(vec![first, answer("The budget is low [R1#1].")]);
        let client = Client::with_base("key", &base).unwrap();
        let cancel = AtomicBool::new(false);
        let mut h = host(Scope::Library);
        let ctx = Context::new("system", None, vec![], Msg::user("budget?"));
        let mut agent = Agent::new(&client, "m", 1_000, &cancel, ctx);
        let out = agent.run(&mut h).await.unwrap();
        assert_eq!(out, "The budget is low [R1#1].");
        assert_eq!(h.text, out);
        assert_eq!(h.resets, 1);
        assert_eq!((agent.stats.requests, agent.stats.tool_calls), (2, 1));
        assert_eq!(agent.usage.unwrap().prompt_tokens, 100);
        let seen = seen.lock().unwrap();
        assert_eq!(seen[0]["tools"][0]["function"]["name"], "list_recordings");
        let msgs = seen[1]["messages"].as_array().unwrap();
        assert_eq!(msgs[2]["tool_calls"][0]["function"]["name"], "search");
        // reasoning blocks go back unchanged with the tool results
        assert_eq!(msgs[2]["reasoning_details"][0]["data"], "opaque");
        assert!(seen[0].get("reasoning").is_none());
        assert_eq!(msgs[3]["role"], "tool");
        assert!(msgs[3]["content"]
            .as_str()
            .unwrap()
            .contains("[R1#1] (0:10)"));
        assert_eq!(h.tools.recording_ids(), ["uuid-a"]);
    }

    #[tokio::test]
    async fn summarizes_history_and_cuts_oversized_tool_output() {
        let (base, seen) = fake_openrouter(vec![
            completion("They talked about the budget."),
            calls("", &[("read_transcript_section", json!({"from": 2}))]),
            answer("Done [1]."),
        ]);
        let client = Client::with_base("key", &base).unwrap();
        let cancel = AtomicBool::new(false);
        let mut h = host(Scope::Recording("uuid-a".into()));
        let history: Vec<Msg> = (0..6)
            .map(|i| {
                if i % 2 == 0 {
                    Msg::user(format!("question {i} {}", "x".repeat(1_500)))
                } else {
                    Msg::assistant(format!("answer {i}"))
                }
            })
            .collect();
        let mut ctx = Context::new("system", None, history, Msg::user("now?"));
        ctx.budget = 6_000;
        let mut agent = Agent::new(&client, "m", 1_000, &cancel, ctx);
        assert_eq!(agent.run(&mut h).await.unwrap(), "Done [1].");
        assert_eq!(agent.ctx.folded, 2);
        assert_eq!(
            agent.ctx.summary.as_deref(),
            Some("They talked about the budget.")
        );
        assert_eq!(agent.stats.compactions, 2);
        assert!(agent.ctx.tokens() <= 6_000);
        let seen = seen.lock().unwrap();
        // the summary request saw the folded messages only
        let summarized = seen[0]["messages"][1]["content"].as_str().unwrap();
        assert!(summarized.contains("question 0") && !summarized.contains("question 2"));
        // later requests carry the summary in the system prompt and only recent history
        let last = seen[2]["messages"].as_array().unwrap();
        assert!(last[0]["content"]
            .as_str()
            .unwrap()
            .contains("They talked about the budget."));
        assert!(last[1]["content"]
            .as_str()
            .unwrap()
            .starts_with("question 2"));
        // the transcript was too big for the budget, so its end was cut
        let out = last.iter().find(|m| m["role"] == "tool").unwrap()["content"]
            .as_str()
            .unwrap();
        assert!(
            out.starts_with("Segments 2–2 of 0–2:\n[2] (0:20) متن"),
            "{out}"
        );
        assert!(out.ends_with("ask for a smaller part.]"), "{out}");
    }

    #[tokio::test]
    async fn summarizes_long_history_in_batches() {
        let (base, seen) = fake_openrouter(vec![
            completion("s1"),
            completion("s2"),
            completion("s3"),
            completion("s4"),
            answer("ok"),
        ]);
        let client = Client::with_base("key", &base).unwrap();
        let cancel = AtomicBool::new(false);
        let mut h = host(Scope::Recording("uuid-a".into()));
        // 8 messages of 6 KB each; with a 12k budget each summary request takes one
        let history: Vec<Msg> = (0..8)
            .map(|i| Msg::user(format!("m{i} {}", "x".repeat(6_000))))
            .collect();
        let mut ctx = Context::new("system", None, history, Msg::user("now?"));
        ctx.budget = 12_000;
        let mut agent = Agent::new(&client, "m", 1_000, &cancel, ctx);
        assert_eq!(agent.run(&mut h).await.unwrap(), "ok");
        assert_eq!(agent.ctx.folded, 4);
        assert_eq!(agent.ctx.summary.as_deref(), Some("s4"));
        let seen = seen.lock().unwrap();
        for (i, req) in seen[..4].iter().enumerate() {
            let input = req["messages"][1]["content"].as_str().unwrap();
            assert!(
                input.len() <= 12_000 * 10 / 4 / 3 + 100,
                "request {i}: {}",
                input.len()
            );
            assert!(input.contains(&format!("User: m{i} ")), "request {i}");
        }
        // each batch builds on the previous summary
        assert!(seen[1]["messages"][1]["content"]
            .as_str()
            .unwrap()
            .starts_with("Summary of the conversation before this:\ns1"));
    }

    #[tokio::test]
    async fn shrinks_summary_batches_after_a_context_error() {
        let overflow = Canned {
            status: 400,
            content_type: "application/json",
            body: json!({ "error": { "message": "This endpoint's maximum context length is 30000 tokens." } }).to_string(),
        };
        let (base, seen) = fake_openrouter(vec![
            overflow,
            completion("s1"),
            completion("s2"),
            answer("ok"),
        ]);
        let client = Client::with_base("key", &base).unwrap();
        let cancel = AtomicBool::new(false);
        let mut h = host(Scope::Recording("uuid-a".into()));
        // two big old messages (over a quarter of the default budget), four short recent ones
        let mut history = vec![
            Msg::user(format!("big0 {}", "x".repeat(90_000))),
            Msg::assistant(format!("big1 {}", "y".repeat(90_000))),
        ];
        history.extend((0..4).map(|i| Msg::user(format!("short {i}"))));
        let ctx = Context::new("system", None, history, Msg::user("now?"));
        let mut agent = Agent::new(&client, "m", 1_000, &cancel, ctx);
        assert_eq!(agent.run(&mut h).await.unwrap(), "ok");
        assert_eq!(
            agent.ctx.budget,
            (30_000 - SUMMARY_TOKENS as usize) * 9 / 10
        );
        assert_eq!(agent.ctx.folded, 2);
        assert_eq!(agent.ctx.summary.as_deref(), Some("s2"));
        let seen = seen.lock().unwrap();
        let input = |i: usize| {
            seen[i]["messages"][1]["content"]
                .as_str()
                .unwrap()
                .to_string()
        };
        assert!(input(0).contains("big0") && input(0).contains("big1"));
        // after the error each big message gets its own, smaller request
        let batch = (agent.ctx.budget as f64 / 3.0 / DEFAULT_RATIO) as usize;
        assert!(input(1).starts_with("User: big0") && input(1).len() <= batch + 100);
        assert!(input(2).contains("Assistant: big1") && input(2).len() <= batch + 100);
    }

    #[tokio::test]
    async fn keeps_history_when_the_summary_is_empty() {
        let (base, _) = fake_openrouter(vec![completion("  ")]);
        let client = Client::with_base("key", &base).unwrap();
        let cancel = AtomicBool::new(false);
        let mut h = host(Scope::Recording("uuid-a".into()));
        let history: Vec<Msg> = (0..6)
            .map(|i| Msg::user(format!("m{i} {}", "x".repeat(3_000))))
            .collect();
        let mut ctx = Context::new("system", None, history, Msg::user("now?"));
        ctx.budget = 12_000;
        let mut agent = Agent::new(&client, "m", 1_000, &cancel, ctx);
        let err = agent.run(&mut h).await.unwrap_err().to_string();
        assert!(err.contains("empty summary"), "{err}");
        assert_eq!((agent.ctx.folded, agent.ctx.history.len()), (0, 6));
        assert!(agent.ctx.summary.is_none());
    }

    #[tokio::test]
    async fn an_earlier_summary_does_not_cut_the_next_message() {
        let (base, seen) = fake_openrouter(vec![completion("s2"), answer("ok")]);
        let client = Client::with_base("key", &base).unwrap();
        let cancel = AtomicBool::new(false);
        let mut h = host(Scope::Recording("uuid-a".into()));
        let mut history = vec![Msg::user(format!("m0 {}", "x".repeat(8_000)))];
        history.extend((0..4).map(|i| Msg::assistant(format!("short {i}"))));
        // 6 KB of earlier summary plus an 8 KB message, with a 10 KB batch
        let mut ctx = Context::new(
            "system",
            Some("s".repeat(6_000)),
            history,
            Msg::user("now?"),
        );
        ctx.budget = 12_000;
        let mut agent = Agent::new(&client, "m", 1_000, &cancel, ctx);
        assert_eq!(agent.run(&mut h).await.unwrap(), "ok");
        assert_eq!(agent.ctx.folded, 1);
        let seen = seen.lock().unwrap();
        let input = seen[0]["messages"][1]["content"].as_str().unwrap();
        assert!(input.contains(&"x".repeat(8_000)), "the message was cut");
    }

    #[test]
    fn counts_attachments_by_their_cost_not_their_base64() {
        let image = Msg::user_parts(vec![
            Part::text("what is this?"),
            Part::image("image/png", &vec![0u8; 3_000_000]),
        ]);
        let ctx = Context::new("s", None, vec![image], Msg::user("q"));
        let tokens = ctx.tokens();
        assert!(
            (crate::llm::IMAGE_TOKENS..crate::llm::IMAGE_TOKENS + 100).contains(&tokens),
            "{tokens}"
        );
    }

    #[test]
    fn drops_all_but_the_latest_tool_output() {
        let call = |id: &str, name: &str| ToolCall {
            id: id.into(),
            name: name.into(),
            arguments: "{}".into(),
        };
        let mut ctx = Context::new("s", None, vec![], Msg::user("q"));
        ctx.turn
            .push(Msg::tool_calls("", vec![call("a", "read_transcript")]));
        ctx.turn.push(Msg::tool_result("a", "x".repeat(5_000)));
        ctx.turn
            .push(Msg::tool_calls("", vec![call("b", "get_notes")]));
        ctx.turn.push(Msg::tool_result("b", "y".repeat(5_000)));
        ctx.drop_old_tool_output();
        assert_eq!(
            ctx.turn[2].content.text(),
            "[The output of read_transcript({}) was removed to save space. Call the tool again if you still need it.]"
        );
        assert_eq!(ctx.turn[4].content.text().len(), 5_000);
    }

    #[tokio::test]
    async fn retries_with_a_smaller_budget_after_a_context_error() {
        let overflow = Canned {
            status: 400,
            content_type: "application/json",
            body: json!({ "error": { "message": "This endpoint's maximum context length is 30000 tokens. However, you requested about 40000 tokens." } }).to_string(),
        };
        let (base, _) = fake_openrouter(vec![overflow, answer("ok")]);
        let client = Client::with_base("key", &base).unwrap();
        let cancel = AtomicBool::new(false);
        let mut h = host(Scope::Recording("uuid-a".into()));
        let ctx = Context::new("system", None, vec![], Msg::user("hi"));
        let mut agent = Agent::new(&client, "m", 2_000, &cancel, ctx);
        assert_eq!(agent.run(&mut h).await.unwrap(), "ok");
        assert_eq!(agent.ctx.budget, (30_000 - 2_000) * 9 / 10);
    }

    #[tokio::test]
    async fn forces_an_answer_after_too_many_rounds() {
        let mut script: Vec<Canned> = (0..3)
            .map(|_| calls("", &[("get_notes", json!({}))]))
            .collect();
        script.push(answer("Here is what I found."));
        let (base, seen) = fake_openrouter(script);
        let client = Client::with_base("key", &base).unwrap();
        let cancel = AtomicBool::new(false);
        let mut h = host(Scope::Recording("uuid-a".into()));
        let ctx = Context::new("system", None, vec![], Msg::user("hi"));
        let mut agent = Agent::new(&client, "m", 1_000, &cancel, ctx);
        agent.max_rounds = 3;
        assert_eq!(agent.run(&mut h).await.unwrap(), "Here is what I found.");
        let seen = seen.lock().unwrap();
        let last = seen.last().unwrap();
        assert_eq!(last["tool_choice"], "none");
        let msgs = last["messages"].as_array().unwrap();
        assert_eq!(msgs.last().unwrap()["content"], ANSWER_NOW);
    }

    #[test]
    fn explains_tool_and_context_errors() {
        let e = anyhow!("OpenRouter error 404: No endpoints found that support tool use.");
        assert!(explain_error(e, "x/y")
            .to_string()
            .starts_with("x/y can't use tools"));
        let e = anyhow!("This model's maximum context length is 131,072 tokens");
        assert!(is_context_overflow(&e));
        assert_eq!(overflow_limit(&e), Some(131_072));
        assert!(!is_context_overflow(&anyhow!("rate limited")));
    }

    #[test]
    fn doc_prompts_include_guides() {
        let facts = RecordingFacts {
            title: "t",
            duration_sec: Some(61.0),
            segment_count: 10,
            has_analysis: false,
            notes: 0,
        };
        let sys = doc_system(&facts, "fa");
        assert!(sys.contains("“t” (1:01, segments 0–9)"));
        assert!(sys.contains("- quiz: "));
        assert!(sys.contains("WWTA.onTime"));
        assert!(sys.contains("write_draft"));
        assert!(sys.contains("// end: items"));
        let revision = doc_revision("a quiz", "142 lines, 18 KB", "rename the title");
        assert!(revision.contains("loaded as the draft (142 lines, 18 KB)"));
        assert!(revision.contains("read_draft") && revision.contains("finish_doc"));
        assert!(!revision.contains("<html"));
        assert!(doc_request("x", Some("timeline")).contains("# Timeline"));
        assert_eq!(doc_request("x", None), "Make this doc: x");
    }
}
