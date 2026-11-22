//! AI features on top of OpenRouter: analysis, transcript fixes, embeddings, and the
//! tool-using agent behind Ask threads (with image and voice attachments and history
//! compaction) and interactive docs.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, Result};
use futures_util::{StreamExt, TryStreamExt};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};
use wwta_core::agent::tools::{Scope, ToolBox};
use wwta_core::agent::{self, add_usage, Agent, Context, Host, RecordingFacts, Stats};
use wwta_core::context::{self, MsgCost};
use wwta_core::db::thread_title;
use wwta_core::llm::{
    self, rewrite_citations, CitationRewriter, Msg, Part, ToolCall, TranscriptText,
};
use wwta_core::search;
use wwta_core::types::{
    now_iso, Analysis, AskResult, ChatAttachment, ChatMessage, ChatThread, Doc, FixProgress,
    FixSummary, LlmModelInfo, LlmStreamEvent, Recording, SearchHit, Segment,
};

use crate::events;
use crate::state::AppState;
use crate::telemetry;

/// Reply budget for chat answers.
const ANSWER_TOKENS: usize = 4_000;
/// Reply budget for the analysis JSON.
const ANALYSIS_TOKENS: usize = 16_000;
/// Reply budget for generated docs.
const DOC_TOKENS: usize = 32_000;
/// Most verbatim chat history sent with a question; older turns are summarized.
const HISTORY_MAX_TOKENS: usize = 24_000;
/// Share (percent) of the prompt budget the verbatim history may take.
const HISTORY_SHARE: usize = 30;
/// Messages always sent verbatim (the last question and answer).
const KEEP_LAST_MESSAGES: usize = 2;
/// Voice clips longer than this are transcribed locally even for models that take audio.
const MAX_NATIVE_AUDIO_SECS: f64 = 300.0;
/// Images of earlier turns re-sent to vision models; older ones go as descriptions.
const MAX_HISTORY_IMAGES: usize = 6;
/// Voice clips of earlier turns re-sent to audio models.
const MAX_HISTORY_AUDIO: usize = 2;
const MAX_ATTACHMENTS: usize = 8;
/// Parallel requests while condensing a long transcript.
const CONDENSE_CONCURRENCY: usize = 4;

/// Record one OpenRouter request for analytics (kind, model, latency, tokens, cost — never
/// prompts or answers) and report unexpected failures to Sentry.
fn track_ai<T>(
    app: &AppHandle,
    kind: &str,
    model: &str,
    started: std::time::Instant,
    result: &Result<T>,
    usage: Option<llm::Usage>,
    extra: serde_json::Value,
) {
    let mut props = serde_json::json!({
        "kind": kind,
        "model": model,
        "success": result.is_ok(),
        "latency_ms": started.elapsed().as_millis() as u64,
    });
    if let Some(u) = usage {
        props["prompt_tokens"] = u.prompt_tokens.into();
        props["completion_tokens"] = u.completion_tokens.into();
        if let Some(c) = u.cost {
            props["cost_usd"] = c.into();
        }
    }
    if let Err(e) = result {
        let msg = format!("{e:#}");
        log::warn!("AI request ({kind}, {model}) failed: {msg}");
        let ek = telemetry::error_kind(&msg);
        props["error_kind"] = ek.into();
        if !matches!(
            ek,
            "cancelled" | "auth" | "credits" | "rate_limited" | "network"
        ) {
            telemetry::report_error("ai_request", &msg, &[("ai_kind", kind), ("model", model)]);
        }
    }
    if let (serde_json::Value::Object(p), serde_json::Value::Object(x)) = (&mut props, extra) {
        p.extend(x);
    }
    app.state::<AppState>()
        .analytics
        .capture("ai request", props);
}

struct LlmJob {
    app: AppHandle,
    id: String,
    cancel: Arc<AtomicBool>,
}

impl LlmJob {
    fn start(app: &AppHandle, id: &str) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        app.state::<AppState>()
            .llm_jobs
            .lock()
            .unwrap()
            .insert(id.to_string(), cancel.clone());
        Self {
            app: app.clone(),
            id: id.to_string(),
            cancel,
        }
    }

    /// Like `start`, but `None` when a job with this id is already running.
    fn try_start(app: &AppHandle, id: &str) -> Option<Self> {
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let state = app.state::<AppState>();
            let mut jobs = state.llm_jobs.lock().unwrap();
            if jobs.contains_key(id) {
                return None;
            }
            jobs.insert(id.to_string(), cancel.clone());
        }
        Some(Self {
            app: app.clone(),
            id: id.to_string(),
            cancel,
        })
    }

    fn send(&self, ev: LlmStreamEvent) {
        let _ = self.app.emit(events::LLM_STREAM, ev);
    }

    fn event(&self) -> LlmStreamEvent {
        LlmStreamEvent {
            request_id: self.id.clone(),
            delta: None,
            done: false,
            error: None,
            status: None,
            reset: false,
        }
    }

    fn delta(&self, text: String) {
        self.send(LlmStreamEvent {
            delta: Some(text),
            ..self.event()
        });
    }

    /// Tell the UI what the request is busy with before the answer streams (preparing
    /// attachments, summarizing, or what the agent is looking up).
    fn status(&self, status: String) {
        self.send(LlmStreamEvent {
            status: Some(status),
            ..self.event()
        });
    }

    /// Clear the text streamed so far: it was the model's preface to tool calls.
    fn reset(&self) {
        self.send(LlmStreamEvent {
            reset: true,
            ..self.event()
        });
    }

    fn finish<T>(&self, r: &Result<T>) {
        self.send(LlmStreamEvent {
            done: true,
            error: r.as_ref().err().map(|e| format!("{e:#}")),
            ..self.event()
        });
    }
}

/// Resolves once the user stops the job.
async fn stopped(job: &LlmJob) {
    while !job.cancel.load(Ordering::Relaxed) {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

/// Run a job's work, giving up as soon as the user stops it (whatever it's waiting on).
async fn until_stopped<T>(
    job: &LlmJob,
    work: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    tokio::select! {
        r = work => r,
        _ = stopped(job) => Err(anyhow!("cancelled")),
    }
}

impl Drop for LlmJob {
    fn drop(&mut self) {
        self.app
            .state::<AppState>()
            .llm_jobs
            .lock()
            .unwrap()
            .remove(&self.id);
    }
}

/// A completion that must be one JSON object. Retries without `response_format` for
/// providers that reject it, and asks the model once to repair invalid JSON.
async fn complete_json(
    client: &llm::Client,
    model: &str,
    messages: &[Msg],
    max_tokens: u32,
    usage: &mut Option<llm::Usage>,
) -> Result<serde_json::Value> {
    let (raw, u) = match client.complete(model, messages, true, max_tokens).await {
        Ok(r) => r,
        Err(e) if e.to_string().contains("response_format") => {
            client.complete(model, messages, false, max_tokens).await?
        }
        Err(e) => return Err(e),
    };
    add_usage(usage, u);
    match llm::extract_json(&raw) {
        Ok(v) => Ok(v),
        Err(first) => {
            log::warn!("model JSON invalid ({first:#}); asking the model to repair it");
            let mut repair = messages.to_vec();
            repair.push(Msg::assistant(raw));
            repair.push(Msg::user(
                "That was not valid JSON. Reply again with only the corrected JSON object.",
            ));
            let (fixed, u) = client.complete(model, &repair, false, max_tokens).await?;
            add_usage(usage, u);
            llm::extract_json(&fixed)
        }
    }
}

/// One user-visible AI request: the client and chat model, the job to report progress to
/// (when the UI watches a stream) and the token usage of every call made along the way.
struct Ai<'a> {
    app: &'a AppHandle,
    client: llm::Client,
    model: String,
    job: Option<&'a LlmJob>,
    usage: Mutex<Option<llm::Usage>>,
}

impl<'a> Ai<'a> {
    fn new(app: &'a AppHandle, job: Option<&'a LlmJob>) -> Result<Self> {
        let s = app.state::<AppState>().settings();
        Ok(Self {
            app,
            client: llm::Client::new(&s.openrouter_api_key)?,
            model: s.llm_model,
            job,
            usage: Mutex::new(None),
        })
    }

    fn usage(&self) -> Option<llm::Usage> {
        *self.usage.lock().unwrap()
    }

    fn add_usage(&self, u: Option<llm::Usage>) {
        add_usage(&mut self.usage.lock().unwrap(), u);
    }

    fn status(&self, s: impl Into<String>) {
        if let Some(j) = self.job {
            j.status(s.into());
        }
    }

    fn check_cancel(&self) -> Result<()> {
        if self.job.is_some_and(|j| j.cancel.load(Ordering::Relaxed)) {
            bail!("cancelled");
        }
        Ok(())
    }

    async fn complete_with(
        &self,
        model: &str,
        messages: &[Msg],
        max_tokens: u32,
    ) -> Result<String> {
        self.check_cancel()?;
        let call = self.client.complete(model, messages, false, max_tokens);
        let (text, u) = match self.job {
            // Stop must not wait for a slow request to come back
            Some(job) => until_stopped(job, call).await?,
            None => call.await?,
        };
        self.add_usage(u);
        Ok(text.trim().to_string())
    }

    async fn complete(&self, messages: &[Msg], max_tokens: u32) -> Result<String> {
        self.complete_with(&self.model, messages, max_tokens).await
    }
}

/// OpenRouter's model list, cached for the session; None when it can't be fetched.
async fn model_list(app: &AppHandle) -> Option<Vec<LlmModelInfo>> {
    let state = app.state::<AppState>();
    let mut cache = state.llm_models.lock().await;
    if cache.is_none() {
        match llm::list_models().await {
            Ok(m) => *cache = Some(m),
            Err(e) => {
                log::info!("model list unavailable: {e:#}");
                return None;
            }
        }
    }
    cache.clone()
}

/// What the chat model accepts and how much fits in its context.
struct Caps {
    context: Option<u64>,
    image: bool,
    audio: bool,
    /// Describes images when the chat model can't see them.
    luna: String,
}

async fn caps(app: &AppHandle, model: &str) -> Caps {
    let list = model_list(app).await.unwrap_or_default();
    let info = list.iter().find(|m| m.id == model);
    Caps {
        context: info.and_then(|m| m.context_length),
        image: info.is_some_and(|m| m.accepts("image")),
        audio: info.is_some_and(|m| m.accepts("audio")),
        luna: llm::latest_luna(&list),
    }
}

fn prompt_tokens(messages: &[Msg]) -> usize {
    messages.iter().map(Msg::tokens).sum()
}

// ------------------------------------------------------------------ long transcripts

/// A stretch of transcript (or of lower-level notes) condensed on its own.
struct Section {
    first: u32,
    last: u32,
    start: f64,
    end: f64,
    input: String,
    text: String,
}

fn render_sections(sections: &[Section]) -> String {
    sections
        .iter()
        .map(|s| {
            format!(
                "{}{}\n",
                llm::section_heading(s.first, s.last, s.start, s.end),
                s.text.trim()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Notes on the whole recording within `budget` tokens: each ~10-minute section condensed
/// by the model (cached per recording), and groups of section notes condensed again while
/// they're still too long.
async fn condensed_transcript(
    ai: &Ai<'_>,
    rid: &str,
    title: &str,
    segs: &[Segment],
    budget: usize,
    max_section: usize,
) -> Result<String> {
    let lines: Vec<String> = segs.iter().map(llm::transcript_line).collect();
    let toks: Vec<usize> = lines.iter().map(|l| context::estimate_tokens(l)).collect();
    let mut sections: Vec<Section> = context::sections(segs, &toks, max_section)
        .into_iter()
        .map(|r| Section {
            first: segs[r.start].id,
            last: segs[r.end - 1].id,
            start: segs[r.start].start,
            end: segs[r.end - 1].end,
            input: lines[r].concat(),
            text: String::new(),
        })
        .collect();
    let mut level = 0u32;
    loop {
        condense_level(ai, rid, title, level, &mut sections).await?;
        let notes = render_sections(&sections);
        if context::estimate_tokens(&notes) <= budget {
            return Ok(notes);
        }
        if sections.len() <= 1 || level >= 3 {
            return Ok(context::clip_middle(&notes, budget));
        }
        level += 1;
        // each group's notes form one condensing request, so they must fit like a section
        let sizes: Vec<usize> = sections
            .iter()
            .map(|s| context::estimate_tokens(&s.text) + 16)
            .collect();
        sections = context::group_by_tokens(&sizes, max_section, context::SECTIONS_PER_GROUP)
            .into_iter()
            .map(|r| &sections[r])
            .map(|g| Section {
                first: g[0].first,
                last: g[g.len() - 1].last,
                start: g[0].start,
                end: g[g.len() - 1].end,
                input: render_sections(g),
                text: String::new(),
            })
            .collect();
    }
}

/// Fill in each section's notes from the cache, condensing the missing ones in parallel.
async fn condense_level(
    ai: &Ai<'_>,
    rid: &str,
    title: &str,
    level: u32,
    sections: &mut [Section],
) -> Result<()> {
    let state = ai.app.state::<AppState>();
    let mut todo = Vec::new();
    {
        let db = state.db.lock().unwrap();
        for (i, s) in sections.iter_mut().enumerate() {
            let fp = context::fingerprint(&s.input);
            match db.digest(rid, level, s.first, s.last, &fp)? {
                Some(t) => s.text = t,
                None => todo.push((i, fp, s.input.clone(), s.first, s.last)),
            }
        }
    }
    if todo.is_empty() {
        return Ok(());
    }
    let total = todo.len();
    let done = AtomicUsize::new(0);
    let progress = |n: usize| {
        ai.status(format!(
            "Condensing the long transcript for the model ({n}/{total})…"
        ))
    };
    progress(0);
    let state = &state;
    let (done, progress) = (&done, &progress);
    let results: Vec<(usize, String)> = futures_util::stream::iter(todo)
        .map(|(i, fp, input, first, last)| async move {
            let text = ai
                .complete(&llm::condense_messages(title, &input, level), 3_000)
                .await?;
            if text.is_empty() {
                bail!("the model returned empty notes while condensing the transcript");
            }
            state
                .db
                .lock()
                .unwrap()
                .save_digest(rid, level, first, last, &fp, &text, &ai.model)?;
            progress(done.fetch_add(1, Ordering::Relaxed) + 1);
            Ok((i, text))
        })
        .buffer_unordered(CONDENSE_CONCURRENCY)
        .try_collect()
        .await?;
    for (i, text) in results {
        sections[i].text = text;
    }
    Ok(())
}

/// The transcript within `budget` tokens: every line when it fits, otherwise condensed notes
/// on the whole recording.
async fn fit_transcript(
    ai: &Ai<'_>,
    caps: &Caps,
    rid: &str,
    title: &str,
    segs: &[Segment],
    budget: usize,
) -> Result<TranscriptText> {
    let full = TranscriptText::full(segs);
    if context::estimate_tokens(&full.text) <= budget {
        return Ok(full);
    }
    let max_section =
        (context::input_budget(caps.context, 3_000) / 2).clamp(2_000, context::MAX_SECTION_TOKENS);
    let notes = condensed_transcript(ai, rid, title, segs, budget, max_section).await?;
    Ok(TranscriptText {
        text: notes,
        condensed: true,
    })
}

// ------------------------------------------------------------------ attachments

fn speech_model_installed(app: &AppHandle) -> bool {
    let state = app.state::<AppState>();
    wwta_core::models::catalog()
        .iter()
        .any(|m| wwta_core::models::model_file(&state.paths.models, m.id).is_some())
}

async fn read_attachment(a: &ChatAttachment) -> Result<Vec<u8>> {
    tokio::fs::read(&a.path)
        .await
        .map_err(|e| anyhow!("the attached file \"{}\" is missing ({e})", a.name))
}

/// Fill in a voice clip's transcript using the local speech model.
async fn transcribe_attachment(ai: &Ai<'_>, a: &mut ChatAttachment) -> Result<()> {
    ai.status("Transcribing the voice message…");
    let app = ai.app.clone();
    let path = a.path.clone();
    let cancel = ai.job.map(|j| j.cancel.clone()).unwrap_or_default();
    let (text, model) = tokio::task::spawn_blocking(move || {
        let samples = wwta_core::audio::read_wav_16k(Path::new(&path))?;
        crate::worker::transcribe_clip(&app, &samples, &cancel)
    })
    .await??;
    ai.app
        .state::<AppState>()
        .db
        .lock()
        .unwrap()
        .set_attachment_text(&a.id, &text, model)?;
    a.text = Some(text);
    a.text_model = Some(model.into());
    Ok(())
}

/// Fill in an image's description using the latest Luna model.
async fn describe_attachment(
    ai: &Ai<'_>,
    caps: &Caps,
    a: &mut ChatAttachment,
    question: &str,
) -> Result<()> {
    ai.status(format!("Describing the image with {}…", caps.luna));
    let bytes = read_attachment(a).await?;
    let messages = llm::describe_image_messages(Part::image(&a.mime, &bytes), question);
    let text = ai.complete_with(&caps.luna, &messages, 1_500).await?;
    if text.is_empty() {
        bail!("{} returned an empty image description", caps.luna);
    }
    ai.app
        .state::<AppState>()
        .db
        .lock()
        .unwrap()
        .set_attachment_text(&a.id, &text, &caps.luna)?;
    a.text = Some(text);
    a.text_model = Some(caps.luna.clone());
    Ok(())
}

/// What a model gets instead of a file it can't take (or that was left out).
fn stand_in(a: &ChatAttachment) -> String {
    let text = a.text.as_deref().unwrap_or("").trim();
    let by = a.text_model.as_deref().unwrap_or("another model");
    match (a.kind.as_str(), text.is_empty()) {
        ("image", false) => format!(
            "[Attached image \"{}\". You can't see images, so here is a description by {by}:]\n{text}",
            a.name
        ),
        ("image", true) => format!("[Attached image \"{}\" (not available to you)]", a.name),
        (_, false) => format!("[Voice message from the user, transcribed:]\n{text}"),
        _ => "[Voice message from the user (not available as text)]".into(),
    }
}

/// Should the current message's attachment go to the model as a file?
fn send_file_now(caps: &Caps, a: &ChatAttachment, speech_installed: bool) -> bool {
    match a.kind.as_str() {
        "image" => caps.image,
        _ => {
            caps.audio
                && (a.duration_sec.unwrap_or(0.0) <= MAX_NATIVE_AUDIO_SECS || !speech_installed)
        }
    }
}

/// For earlier messages: which attachments are re-sent as files (the most recent few the
/// model accepts), per message and attachment.
fn history_file_plan(caps: &Caps, msgs: &[ChatMessage], speech_installed: bool) -> Vec<Vec<bool>> {
    let (mut images, mut audio) = (0usize, 0usize);
    let mut plan: Vec<Vec<bool>> = msgs
        .iter()
        .rev()
        .map(|m| {
            m.attachments
                .iter()
                .rev()
                .map(|a| {
                    if a.kind == "image" {
                        let ok = caps.image && images < MAX_HISTORY_IMAGES;
                        images += ok as usize;
                        ok
                    } else {
                        // same rule as for the current message: long clips go as text
                        // when a speech model can transcribe them
                        let short = a.duration_sec.unwrap_or(0.0) <= MAX_NATIVE_AUDIO_SECS;
                        let ok =
                            caps.audio && (short || !speech_installed) && audio < MAX_HISTORY_AUDIO;
                        audio += ok as usize;
                        ok
                    }
                })
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect()
        })
        .collect();
    plan.reverse();
    plan
}

/// Estimated prompt tokens of an earlier message under a file plan.
fn message_cost(m: &ChatMessage, files: &[bool]) -> usize {
    let mut t = context::estimate_tokens(&m.content) + 4;
    for (a, &file) in m.attachments.iter().zip(files) {
        t += match (file, a.kind.as_str()) {
            (true, "image") => llm::IMAGE_TOKENS,
            (true, _) => (a.duration_sec.unwrap_or(60.0) * 32.0) as usize,
            (false, _) => a
                .text
                .as_deref()
                .map(context::estimate_tokens)
                .unwrap_or(300),
        };
    }
    t
}

/// Prompt parts for one attachment: the file itself when `as_file`, otherwise its text
/// stand-in, created on first use (voice: local speech model; image: latest Luna).
async fn attachment_parts(
    ai: &Ai<'_>,
    caps: &Caps,
    a: &mut ChatAttachment,
    as_file: bool,
    question: &str,
) -> Result<Vec<Part>> {
    let image = a.kind == "image";
    if as_file {
        let bytes = read_attachment(a).await?;
        return Ok(if image {
            vec![
                Part::text(format!("[Attached image \"{}\"]", a.name)),
                Part::image(&a.mime, &bytes),
            ]
        } else {
            vec![
                Part::text("[Voice message from the user]"),
                Part::wav(&bytes),
            ]
        });
    }
    if a.text.is_none() {
        if image {
            describe_attachment(ai, caps, a, question).await?;
        } else {
            transcribe_attachment(ai, a).await?;
        }
    }
    Ok(vec![Part::text(stand_in(a))])
}

// ------------------------------------------------------------------ chat compaction

/// Earlier messages as plain text for the summarizer.
fn conversation_text(msgs: &[ChatMessage]) -> String {
    let mut out = String::new();
    for m in msgs {
        let who = if m.role == "user" {
            "User"
        } else {
            "Assistant"
        };
        let mut body = m.content.trim().to_string();
        for a in &m.attachments {
            body.push('\n');
            body.push_str(&stand_in(a));
        }
        out.push_str(&format!(
            "{who}: {}\n\n",
            context::clip_middle(&body, 3_000)
        ));
    }
    out
}

/// Fold a thread's oldest messages into its summary when the verbatim history would go over
/// `budget` tokens. Returns the summary and how many leading messages it replaces.
async fn compact_history(
    ai: &Ai<'_>,
    caps: &Caps,
    thread: &ChatThread,
    msgs: &mut [ChatMessage],
    budget: usize,
    about: &str,
) -> Result<(Option<String>, usize, bool)> {
    let start = (thread.compacted_count as usize).min(msgs.len());
    let hist = &mut msgs[start..];
    let plan = history_file_plan(caps, hist, speech_model_installed(ai.app));
    let costs: Vec<MsgCost> = hist
        .iter()
        .zip(&plan)
        .map(|(m, files)| MsgCost {
            is_user: m.role == "user",
            tokens: message_cost(m, files),
        })
        .collect();
    let fold = context::messages_to_fold(&costs, budget, KEEP_LAST_MESSAGES);
    if fold == 0 {
        return Ok((thread.summary.clone(), start, false));
    }
    // files the model saw directly have no text yet; the summary needs it to keep them
    let mut missing = false;
    for m in hist[..fold].iter_mut() {
        let asked = m.content.clone();
        for a in m.attachments.iter_mut().filter(|a| a.text.is_none()) {
            if let Err(e) = attachment_parts(ai, caps, a, false, &asked).await {
                ai.check_cancel()?;
                log::warn!("couldn't put an attachment into words for the summary: {e:#}");
                missing = true;
            }
        }
    }
    // Folding now would lose that file for good; wait for a later turn while the history
    // still fits comfortably (but not forever, if the describing model stays unavailable).
    let total: usize = costs.iter().map(|c| c.tokens).sum();
    if missing && total <= budget * 2 {
        return Ok((thread.summary.clone(), start, false));
    }
    ai.status("Summarizing earlier messages to fit the model's context…");
    let previous = thread.summary.as_deref();
    let room = context::input_budget(caps.context, 1_500)
        .saturating_sub(previous.map_or(0, context::estimate_tokens) + 1_000)
        .max(2_000);
    let convo = context::clip_middle(&conversation_text(&hist[..fold]), room);
    let summary = ai
        .complete(&llm::summary_messages(previous, &convo, about), 1_500)
        .await?;
    if summary.is_empty() {
        bail!("the model returned an empty summary of the conversation");
    }
    let count = start + fold;
    ai.app
        .state::<AppState>()
        .db
        .lock()
        .unwrap()
        .set_thread_summary(&thread.id, &summary, count as u32)?;
    Ok((Some(summary), count, true))
}

pub async fn analyze(app: &AppHandle, id: &str) -> Result<Analysis> {
    let started = std::time::Instant::now();
    let model = app.state::<AppState>().settings().llm_model;
    let (r, usage) = match Ai::new(app, None) {
        Ok(ai) => {
            let r = analyze_inner(&ai, id).await;
            (r, ai.usage())
        }
        Err(e) => (Err(e), None),
    };
    let extra = match &r {
        Ok(a) => serde_json::json!({ "chapters": a.chapters.len(), "tags": a.tags.len() }),
        Err(_) => serde_json::json!({}),
    };
    track_ai(app, "analyze", &model, started, &r, usage, extra);
    r
}

async fn analyze_inner(ai: &Ai<'_>, id: &str) -> Result<Analysis> {
    let app = ai.app;
    let state = app.state::<AppState>();
    let settings = state.settings();
    let (rec, segs) = {
        let db = state.db.lock().unwrap();
        (db.recording(id)?, db.segments(id)?)
    };
    if segs.is_empty() {
        bail!("transcribe the recording before analyzing it");
    }
    let caps = caps(app, &ai.model).await;
    let empty = TranscriptText {
        text: String::new(),
        condensed: true,
    };
    let overhead = prompt_tokens(&llm::analysis_messages(
        &rec.title,
        &segs,
        &empty,
        &settings.ai_language,
    ));
    let room = context::input_budget(caps.context, ANALYSIS_TOKENS)
        .saturating_sub(overhead)
        .max(2_000);
    let transcript = fit_transcript(ai, &caps, id, &rec.title, &segs, room).await?;
    let messages = llm::analysis_messages(&rec.title, &segs, &transcript, &settings.ai_language);
    let model = ai.model.clone();
    let mut usage = None;
    let value = complete_json(
        &ai.client,
        &model,
        &messages,
        ANALYSIS_TOKENS as u32,
        &mut usage,
    )
    .await;
    ai.add_usage(usage);
    let value = value?;
    let analysis = llm::parse_analysis(&value, &segs, &model, &settings.ai_language);
    {
        let mut db = state.db.lock().unwrap();
        db.save_analysis(id, &analysis)?;
        // use the AI title unless the user renamed the recording
        if let Some(t) = &analysis.title {
            db.set_suggested_title(id, t)?;
        }
        events::recording_updated(app, &db, id);
    }
    Ok(analysis)
}

/// Fix requests in flight at once.
const FIX_CONCURRENCY: usize = 4;

/// `llm_jobs` key of a recording's transcript fix; `cancel_llm` with it stops the fix.
pub fn fix_job_id(recording_id: &str) -> String {
    format!("fix:{recording_id}")
}

/// Correct recognition errors in the transcript, batch by batch. Each batch is saved and
/// sent to the UI as it finishes, so a failed or cancelled run keeps what was done.
pub async fn fix_transcript(app: &AppHandle, id: &str) -> Result<FixSummary> {
    let job = LlmJob::try_start(app, &fix_job_id(id))
        .ok_or_else(|| anyhow!("this transcript is already being fixed"))?;
    let started = std::time::Instant::now();
    let model = app.state::<AppState>().settings().llm_model;
    let (r, usage) = match Ai::new(app, None) {
        Ok(ai) => {
            let r = fix_transcript_inner(&ai, id, &job).await;
            (r, ai.usage())
        }
        Err(e) => (Err(e), None),
    };
    let extra = match &r {
        Ok(s) => {
            serde_json::json!({ "changed": s.changed, "batches": s.batches, "failed": s.failed })
        }
        Err(_) => serde_json::json!({}),
    };
    track_ai(app, "fix_transcript", &model, started, &r, usage, extra);
    r
}

async fn fix_transcript_inner(ai: &Ai<'_>, id: &str, job: &LlmJob) -> Result<FixSummary> {
    let app = ai.app;
    let state = app.state::<AppState>();
    let (rec, segs, analysis) = {
        let db = state.db.lock().unwrap();
        (db.recording(id)?, db.segments(id)?, db.analysis(id)?)
    };
    if segs.is_empty() {
        bail!("transcribe the recording before fixing it");
    }
    let summary = analysis.as_ref().map(|a| a.summary.as_str());
    let mut pending = llm::fix_batches(&segs).into_iter().map(|range| {
        let messages = llm::fix_messages(&rec.title, summary, &segs, range.clone());
        (range, messages)
    });
    let batches = pending.len() as u32;
    let mut running = tokio::task::JoinSet::new();
    let mut spawn_next = |running: &mut tokio::task::JoinSet<_>| {
        if let Some((range, messages)) = pending.next() {
            let (client, model) = (ai.client.clone(), ai.model.clone());
            running.spawn(async move {
                let mut usage = None;
                let r = complete_json(&client, &model, &messages, llm::FIX_MAX_TOKENS, &mut usage)
                    .await;
                (range, r, usage)
            });
        }
    };
    for _ in 0..FIX_CONCURRENCY {
        spawn_next(&mut running);
    }
    let (mut done, mut changed, mut failed) = (0u32, 0u32, 0u32);
    let mut first_error: Option<String> = None;
    loop {
        // Dropping `running` on cancel aborts the requests still in flight.
        let joined = tokio::select! {
            j = running.join_next() => j,
            _ = stopped(job) => bail!("cancelled"),
        };
        let Some(joined) = joined else { break };
        spawn_next(&mut running);
        let (range, r, u) = joined?;
        ai.add_usage(u);
        done += 1;
        let fixed = match r {
            Ok(v) => {
                let fixes = llm::parse_fixes(&v, &segs[range]);
                state.db.lock().unwrap().apply_fixes(id, &fixes)?
            }
            Err(e) => {
                log::warn!("transcript fix batch failed: {e:#}");
                failed += 1;
                first_error.get_or_insert_with(|| format!("{e:#}"));
                vec![]
            }
        };
        changed += fixed.len() as u32;
        let _ = app.emit(
            events::TRANSCRIPT_FIX,
            FixProgress {
                recording_id: id.to_string(),
                done,
                total: batches,
                changed: fixed,
            },
        );
    }
    if failed == batches {
        bail!(first_error.unwrap_or_default());
    }
    Ok(FixSummary {
        changed,
        batches,
        failed,
        error: first_error,
    })
}

pub async fn index_embeddings(app: &AppHandle, id: &str) -> Result<()> {
    let started = std::time::Instant::now();
    let model = app.state::<AppState>().settings().embedding_model;
    let mut inputs = 0usize;
    let r = index_embeddings_inner(app, id, &mut inputs).await;
    track_ai(
        app,
        "embed",
        &model,
        started,
        &r,
        None,
        serde_json::json!({ "inputs": inputs }),
    );
    r
}

async fn index_embeddings_inner(app: &AppHandle, id: &str, inputs: &mut usize) -> Result<()> {
    let state = app.state::<AppState>();
    let settings = state.settings();
    let client = llm::Client::new(&settings.openrouter_api_key)?;
    let segs = state.db.lock().unwrap().segments(id)?;
    if segs.is_empty() {
        bail!("transcribe the recording first");
    }
    let chunks = search::chunk_segments(&segs, 450);
    let texts: Vec<String> = chunks.iter().map(|c| c.2.clone()).collect();
    *inputs = texts.len();
    let vectors = client.embed(&settings.embedding_model, &texts).await?;
    let rows: Vec<wwta_core::db::EmbeddingRow> = chunks
        .iter()
        .zip(vectors)
        .enumerate()
        .map(|(i, (c, v))| wwta_core::db::EmbeddingRow {
            recording_id: id.to_string(),
            chunk: i as u32,
            start_seg: c.0,
            end_seg: c.1,
            vector: v,
        })
        .collect();
    let mut db = state.db.lock().unwrap();
    // An edit, AI fix or new transcript while the request ran would leave stale vectors.
    if db.segments(id)? != segs {
        bail!("the transcript changed while indexing; enable semantic search again");
    }
    db.replace_embeddings(id, &settings.embedding_model, &rows)?;
    Ok(())
}

/// Semantic hits for a free-text query; empty (not an error) when offline or not indexed.
pub async fn semantic_search(
    app: &AppHandle,
    query: &str,
    recording_id: Option<&str>,
    limit: usize,
) -> Vec<SearchHit> {
    let state = app.state::<AppState>();
    let settings = state.settings();
    if settings.openrouter_api_key.trim().is_empty() {
        return vec![];
    }
    let rows = match state
        .db
        .lock()
        .unwrap()
        .embeddings(&settings.embedding_model, recording_id)
    {
        Ok(r) if !r.is_empty() => r,
        _ => return vec![],
    };
    let Ok(client) = llm::Client::new(&settings.openrouter_api_key) else {
        return vec![];
    };
    let started = std::time::Instant::now();
    let embedded = client
        .embed(&settings.embedding_model, &[query.to_string()])
        .await;
    track_ai(
        app,
        "semantic_search",
        &settings.embedding_model,
        started,
        &embedded,
        None,
        serde_json::json!({}),
    );
    let q = match embedded {
        Ok(mut v) if !v.is_empty() => v.remove(0),
        Ok(_) => return vec![],
        Err(e) => {
            log::info!("semantic search unavailable: {e:#}");
            return vec![];
        }
    };
    let db = state.db.lock().unwrap();
    search::semantic_hits(&db, &rows, &q, None, limit, 0.45).unwrap_or_default()
}

/// Runs the agent's tools against the library and streams its text to the UI.
struct ToolHost<'a> {
    app: &'a AppHandle,
    job: &'a LlmJob,
    tools: ToolBox,
    /// Limits semantic search to one recording.
    recording_id: Option<String>,
    /// Library chat: turns `[R3#14]` aliases into `[rec:<uuid>#14]` while streaming.
    rewriter: Option<CitationRewriter>,
}

impl<'a> ToolHost<'a> {
    fn new(app: &'a AppHandle, job: &'a LlmJob, recording_id: Option<&str>, skills: bool) -> Self {
        let scope = match recording_id {
            Some(id) => Scope::Recording(id.to_string()),
            None => Scope::Library,
        };
        Self {
            app,
            job,
            tools: ToolBox::new(scope, skills),
            recording_id: recording_id.map(String::from),
            rewriter: recording_id.is_none().then(CitationRewriter::default),
        }
    }
}

impl Host for ToolHost<'_> {
    fn tools(&self) -> Vec<Value> {
        self.tools.definitions()
    }

    async fn run_tool(&mut self, call: &ToolCall) -> String {
        let state = self.app.state::<AppState>();
        let label = self.tools.label(&state.db.lock().unwrap(), call);
        self.job.status(format!("{label}…"));
        let semantic = match ToolBox::search_query(call) {
            Some(q) => semantic_search(self.app, &q, self.recording_id.as_deref(), 12).await,
            None => vec![],
        };
        let db = state.db.lock().unwrap();
        self.tools.run(&db, call, semantic)
    }

    fn on_text(&mut self, delta: &str) {
        let out = match self.rewriter.as_mut() {
            Some(r) => r.push(delta, self.tools.recording_ids()),
            None => delta.to_string(),
        };
        if !out.is_empty() {
            self.job.delta(out);
        }
    }

    fn on_reset(&mut self) {
        if let Some(r) = self.rewriter.as_mut() {
            r.reset();
        }
        self.job.reset();
    }
}

fn facts(rec: &Recording, notes: usize) -> RecordingFacts<'_> {
    RecordingFacts {
        title: &rec.title,
        duration_sec: rec.duration_sec,
        segment_count: rec.segment_count,
        has_analysis: rec.has_analysis,
        notes,
    }
}

/// The agent's token budget: what the model's context leaves after the reply, at most
/// [`agent::CONTEXT_BUDGET`].
fn agent_budget(caps: &Caps, answer_tokens: usize) -> usize {
    context::input_budget(caps.context, answer_tokens)
        .clamp(agent::MIN_BUDGET, agent::CONTEXT_BUDGET)
}

fn stats_props(stats: Stats) -> serde_json::Value {
    serde_json::json!({
        "requests": stats.requests,
        "tool_calls": stats.tool_calls,
        "compactions": stats.compactions,
    })
}

/// What a question used, for usage statistics (counts and flags only).
#[derive(Default)]
struct AskStats {
    images: usize,
    voice: usize,
    compacted: bool,
    agent: Stats,
}

pub async fn ask(
    app: &AppHandle,
    recording_id: Option<String>,
    thread_id: Option<String>,
    question: String,
    attachment_ids: Vec<String>,
    request_id: String,
) -> Result<AskResult> {
    let job = LlmJob::start(app, &request_id);
    let started = std::time::Instant::now();
    let model = app.state::<AppState>().settings().llm_model;
    let kind = if recording_id.is_some() {
        "ask"
    } else {
        "ask_library"
    };
    let new_thread = thread_id.is_none();
    let mut stats = AskStats::default();
    let (r, usage) = match Ai::new(app, Some(&job)) {
        Ok(ai) => {
            let work = ask_inner(
                &ai,
                &job,
                recording_id,
                thread_id,
                question,
                attachment_ids,
                &mut stats,
            );
            let r = until_stopped(&job, work).await;
            (r, ai.usage())
        }
        Err(e) => (Err(e), None),
    };
    job.finish(&r);
    let mut props = stats_props(stats.agent);
    props["images"] = stats.images.into();
    props["voice_messages"] = stats.voice.into();
    props["history_compacted"] = stats.compacted.into();
    props["new_thread"] = new_thread.into();
    track_ai(app, kind, &model, started, &r, usage, props);
    r
}

async fn ask_inner(
    ai: &Ai<'_>,
    job: &LlmJob,
    recording_id: Option<String>,
    thread_id: Option<String>,
    question: String,
    attachment_ids: Vec<String>,
    stats: &mut AskStats,
) -> Result<AskResult> {
    let app = ai.app;
    let state = app.state::<AppState>();
    let settings = state.settings();
    let asked_at = now_iso();
    let question = question.trim().to_string();
    let rid = recording_id.as_deref();
    if attachment_ids.len() > MAX_ATTACHMENTS {
        bail!("attach at most {MAX_ATTACHMENTS} files to one message");
    }
    let (thread, mut prior, mut attachments) = {
        let db = state.db.lock().unwrap();
        let thread = match thread_id.as_deref() {
            Some(id) => {
                let t = db.thread(id)?;
                if t.recording_id.as_deref() != rid {
                    bail!("this conversation belongs to another recording");
                }
                Some(t)
            }
            None => None,
        };
        let prior = match &thread {
            Some(t) => db.thread_messages(&t.id)?,
            None => vec![],
        };
        let atts = attachment_ids
            .iter()
            .map(|id| db.pending_attachment(id))
            .collect::<Result<Vec<_>>>()?;
        (thread, prior, atts)
    };
    if question.is_empty() && attachments.is_empty() {
        bail!("empty question");
    }
    stats.images = attachments.iter().filter(|a| a.kind == "image").count();
    stats.voice = attachments.len() - stats.images;
    let (system, about) = {
        let db = state.db.lock().unwrap();
        match rid {
            Some(id) => {
                let rec = db.recording(id)?;
                if rec.segment_count == 0 {
                    bail!("transcribe the recording before asking about it");
                }
                let notes = db.notes(id)?.len();
                (
                    agent::recording_chat_system(&facts(&rec, notes), &settings.ai_language),
                    format!("the recording \"{}\"", rec.title),
                )
            }
            None => {
                let n = db.list_recordings()?.len();
                if n == 0 {
                    bail!("your library is empty; import and transcribe some recordings first");
                }
                (
                    agent::library_chat_system(n, &settings.ai_language),
                    "the user's library of transcribed recordings".into(),
                )
            }
        }
    };
    let caps = caps(app, &ai.model).await;
    let speech = speech_model_installed(app);

    // the new message: files the model takes, text stand-ins for the rest
    let mut parts = Vec::new();
    if !question.is_empty() {
        parts.push(Part::text(question.clone()));
    }
    for a in attachments.iter_mut() {
        let as_file = send_file_now(&caps, a, speech);
        parts.extend(attachment_parts(ai, &caps, a, as_file, &question).await?);
    }

    // earlier messages, the oldest folded into the thread summary when they'd crowd out
    // what the agent reads with its tools
    let budget = agent_budget(&caps, ANSWER_TOKENS);
    let history_budget = (budget * HISTORY_SHARE / 100).min(HISTORY_MAX_TOKENS);
    let (summary, kept_from) = match &thread {
        Some(t) => {
            let (summary, from, compacted) =
                compact_history(ai, &caps, t, &mut prior, history_budget, &about).await?;
            stats.compacted = compacted;
            (summary, from)
        }
        None => (None, 0),
    };
    let mut host = ToolHost::new(app, job, rid, false);
    // stored answers cite `[rec:<uuid>#14]`; show the model the aliases its tools use
    let summary = summary.map(|s| host.tools.aliasify(&s));
    let kept = &mut prior[kept_from..];
    let plan = history_file_plan(&caps, kept, speech);
    let mut history = Vec::with_capacity(kept.len());
    for (m, files) in kept.iter_mut().zip(plan) {
        let content = host.tools.aliasify(&m.content);
        if m.role != "user" {
            history.push(Msg::assistant(content));
            continue;
        }
        let mut p = Vec::new();
        if !content.trim().is_empty() {
            p.push(Part::text(content));
        }
        let asked = m.content.clone();
        for (a, as_file) in m.attachments.iter_mut().zip(files) {
            match attachment_parts(ai, &caps, a, as_file, &asked).await {
                Ok(x) => p.extend(x),
                Err(e) => {
                    ai.check_cancel()?;
                    log::warn!("earlier attachment left out: {e:#}");
                    p.push(Part::text(stand_in(a)));
                }
            }
        }
        if p.is_empty() {
            p.push(Part::text("(empty message)"));
        }
        history.push(Msg::user_parts(p));
    }

    let mut ctx = Context::new(system, summary, history, Msg::user_parts(parts));
    ctx.budget = budget;
    ai.check_cancel()?;
    let mut agent = Agent::new(
        &ai.client,
        &ai.model,
        ANSWER_TOKENS as u32,
        &job.cancel,
        ctx,
    );
    let result = agent.run(&mut host).await;
    ai.add_usage(agent.usage);
    stats.agent = agent.stats;
    // the agent folded more of the history: remember it so later questions don't redo it
    if let (Some(t), Some(summary)) = (&thread, &agent.ctx.summary) {
        if agent.ctx.folded > 0 {
            let summary = rewrite_citations(summary, host.tools.recording_ids());
            let count = (kept_from + agent.ctx.folded) as u32;
            state
                .db
                .lock()
                .unwrap()
                .set_thread_summary(&t.id, &summary, count)?;
            stats.compacted = true;
        }
    }
    let answer = result?;
    let answer = match host.rewriter.as_mut() {
        Some(r) => {
            let tail = r.finish(host.tools.recording_ids());
            if !tail.is_empty() {
                job.delta(tail);
            }
            rewrite_citations(&answer, host.tools.recording_ids())
        }
        None => answer,
    };

    let db = state.db.lock().unwrap();
    // a thread deleted while the answer streamed: keep the answer in a new one
    let thread = match thread.filter(|t| db.thread(&t.id).is_ok()) {
        Some(t) => t,
        None => {
            let spoken = attachments
                .iter()
                .find(|a| a.kind == "audio")
                .and_then(|a| a.text.clone());
            let source = if question.is_empty() {
                spoken.unwrap_or_default()
            } else {
                question.clone()
            };
            db.create_thread(rid, &thread_title(&source, &attachments))?
        }
    };
    let asked = ChatMessage {
        id: uuid::Uuid::new_v4().to_string(),
        recording_id: recording_id.clone(),
        thread_id: thread.id.clone(),
        role: "user".into(),
        content: question,
        created_at: asked_at,
        model: None,
        attachments,
        context_note: None,
    };
    db.add_chat(&asked)?;
    let answered = ChatMessage {
        id: uuid::Uuid::new_v4().to_string(),
        recording_id,
        thread_id: thread.id.clone(),
        role: "assistant".into(),
        content: answer,
        created_at: now_iso(),
        model: Some(ai.model.clone()),
        attachments: vec![],
        context_note: None,
    };
    db.add_chat(&answered)?;
    Ok(AskResult {
        thread: db.thread(&thread.id)?,
        question: asked,
        answer: answered,
    })
}

/// Let the doc agent work on one recording and return the HTML it wrote.
async fn write_doc(
    ai: &Ai<'_>,
    job: &LlmJob,
    recording_id: &str,
    request: String,
    stats: &mut Stats,
) -> Result<String> {
    let state = ai.app.state::<AppState>();
    let settings = state.settings();
    let system = {
        let db = state.db.lock().unwrap();
        let rec = db.recording(recording_id)?;
        if rec.segment_count == 0 {
            bail!("transcribe the recording before generating docs");
        }
        let notes = db.notes(recording_id)?.len();
        agent::doc_system(&facts(&rec, notes), &settings.ai_language)
    };
    let caps = caps(ai.app, &ai.model).await;
    let mut ctx = Context::new(system, None, vec![], Msg::user(request));
    ctx.budget = agent_budget(&caps, DOC_TOKENS);
    let mut host = ToolHost::new(ai.app, job, Some(recording_id), true);
    let mut agent = Agent::new(&ai.client, &ai.model, DOC_TOKENS as u32, &job.cancel, ctx);
    let result = agent.run(&mut host).await;
    ai.add_usage(agent.usage);
    *stats = agent.stats;
    llm::extract_html(&result?)
}

pub async fn generate_doc(
    app: &AppHandle,
    id: String,
    prompt: String,
    skill: Option<String>,
    request_id: String,
) -> Result<Doc> {
    let job = LlmJob::start(app, &request_id);
    let started = std::time::Instant::now();
    let model = app.state::<AppState>().settings().llm_model;
    let mut stats = Stats::default();
    let (r, usage) = match Ai::new(app, Some(&job)) {
        Ok(ai) => {
            let work = generate_doc_inner(&ai, &job, &id, &prompt, skill.as_deref(), &mut stats);
            let r = until_stopped(&job, work).await;
            (r, ai.usage())
        }
        Err(e) => (Err(e), None),
    };
    job.finish(&r);
    let mut props = stats_props(stats);
    // only the built-in template names, never free text
    if let Some(s) = skill.as_deref().and_then(agent::skills::get) {
        props["skill"] = s.name.as_str().into();
    }
    track_ai(app, "generate_doc", &model, started, &r, usage, props);
    r
}

async fn generate_doc_inner(
    ai: &Ai<'_>,
    job: &LlmJob,
    id: &str,
    prompt: &str,
    skill: Option<&str>,
    stats: &mut Stats,
) -> Result<Doc> {
    let request = agent::doc_request(prompt, skill);
    let html = write_doc(ai, job, id, request, stats).await?;
    let now = now_iso();
    let doc = Doc {
        id: uuid::Uuid::new_v4().to_string(),
        recording_id: id.to_string(),
        title: llm::html_title(&html).unwrap_or_else(|| prompt.chars().take(60).collect()),
        prompt: prompt.to_string(),
        html,
        model: ai.model.clone(),
        created_at: now.clone(),
        updated_at: now,
    };
    ai.app
        .state::<AppState>()
        .db
        .lock()
        .unwrap()
        .save_doc(&doc)?;
    Ok(doc)
}

pub async fn revise_doc(
    app: &AppHandle,
    doc_id: String,
    instruction: String,
    request_id: String,
) -> Result<Doc> {
    let job = LlmJob::start(app, &request_id);
    let started = std::time::Instant::now();
    let model = app.state::<AppState>().settings().llm_model;
    let mut stats = Stats::default();
    let (r, usage) = match Ai::new(app, Some(&job)) {
        Ok(ai) => {
            let work = revise_doc_inner(&ai, &job, &doc_id, &instruction, &mut stats);
            let r = until_stopped(&job, work).await;
            (r, ai.usage())
        }
        Err(e) => (Err(e), None),
    };
    job.finish(&r);
    track_ai(
        app,
        "revise_doc",
        &model,
        started,
        &r,
        usage,
        stats_props(stats),
    );
    r
}

async fn revise_doc_inner(
    ai: &Ai<'_>,
    job: &LlmJob,
    doc_id: &str,
    instruction: &str,
    stats: &mut Stats,
) -> Result<Doc> {
    let doc = ai
        .app
        .state::<AppState>()
        .db
        .lock()
        .unwrap()
        .doc(doc_id)
        .map_err(|_| anyhow!("doc not found"))?;
    let request = agent::doc_revision(&doc.prompt, &doc.html, instruction);
    let html = write_doc(ai, job, &doc.recording_id, request, stats).await?;
    let updated = Doc {
        title: llm::html_title(&html).unwrap_or(doc.title.clone()),
        html,
        model: ai.model.clone(),
        updated_at: now_iso(),
        ..doc
    };
    ai.app
        .state::<AppState>()
        .db
        .lock()
        .unwrap()
        .save_doc(&updated)?;
    Ok(updated)
}
