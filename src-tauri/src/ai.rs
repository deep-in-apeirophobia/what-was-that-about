//! AI features on top of OpenRouter: analysis, transcript fixes, embeddings, Ask threads
//! (with image and voice attachments, history compaction and condensed transcripts) and
//! interactive docs.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, Result};
use futures_util::{StreamExt, TryStreamExt};
use tauri::{AppHandle, Emitter, Manager};
use wwta_core::context::{self, MsgCost};
use wwta_core::db::thread_title;
use wwta_core::llm::{self, CitationRewriter, LibraryContextItem, Msg, Part, TranscriptText};
use wwta_core::search;
use wwta_core::types::{
    now_iso, Analysis, AskResult, ChatAttachment, ChatMessage, ChatThread, Doc, FixProgress,
    FixSummary, LlmModelInfo, LlmStreamEvent, SearchHit, Segment,
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
/// Most transcript a doc request sends, however large the model's context.
const DOC_TRANSCRIPT_MAX_TOKENS: usize = 150_000;
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

fn add_usage(acc: &mut Option<llm::Usage>, u: Option<llm::Usage>) {
    if let Some(u) = u {
        let a = acc.get_or_insert_with(llm::Usage::default);
        a.prompt_tokens += u.prompt_tokens;
        a.completion_tokens += u.completion_tokens;
        a.cost = match (a.cost, u.cost) {
            (Some(x), Some(y)) => Some(x + y),
            (x, y) => x.or(y),
        };
    }
}

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

    fn emit(&self, delta: Option<String>, done: bool, error: Option<String>) {
        let _ = self.app.emit(
            events::LLM_STREAM,
            LlmStreamEvent {
                request_id: self.id.clone(),
                delta,
                done,
                error,
                status: None,
            },
        );
    }

    /// Tell the UI what the request is busy with before the answer streams.
    fn status(&self, status: String) {
        let _ = self.app.emit(
            events::LLM_STREAM,
            LlmStreamEvent {
                request_id: self.id.clone(),
                delta: None,
                done: false,
                error: None,
                status: Some(status),
            },
        );
    }

    fn finish<T>(&self, r: &Result<T>) {
        match r {
            Ok(_) => self.emit(None, true, None),
            Err(e) => self.emit(None, true, Some(format!("{e:#}"))),
        }
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

/// Distinct words of a query worth a lexical search (long voice transcripts would otherwise
/// mean thousands of scans).
fn search_words(query: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    query
        .split_whitespace()
        .filter(|t| t.chars().count() >= 3)
        .filter(|t| seen.insert(wwta_core::text::normalize_for_search(t)))
        .take(16)
        .map(String::from)
        .collect()
}

/// Transcript indices most relevant to a question (semantic, word and tag matches), best first.
async fn question_hits(app: &AppHandle, rid: &str, query: &str, segs: &[Segment]) -> Vec<usize> {
    let semantic = semantic_search(app, query, Some(rid), 24).await;
    let (lexical, tags) = {
        let state = app.state::<AppState>();
        let db = state.db.lock().unwrap();
        let mut lex = Vec::new();
        for term in search_words(query) {
            lex.extend(search::lexical(&db, &term, Some(rid), 12).unwrap_or_default());
        }
        (
            lex,
            search::tag_hits(&db, query, Some(rid)).unwrap_or_default(),
        )
    };
    search::merge_hits(vec![semantic, lexical, tags], 80)
        .iter()
        .filter(|h| h.recording_id == rid)
        .filter_map(|h| segs.binary_search_by_key(&h.segment_id, |s| s.id).ok())
        .collect()
}

/// The transcript within `budget` tokens: every line when it fits; otherwise condensed
/// notes on the whole recording, plus verbatim excerpts picked for `query` when there is one.
async fn fit_transcript(
    ai: &Ai<'_>,
    caps: &Caps,
    rid: &str,
    title: &str,
    segs: &[Segment],
    budget: usize,
    query: Option<&str>,
) -> Result<TranscriptText> {
    let full = TranscriptText::full(segs);
    if context::estimate_tokens(&full.text) <= budget {
        return Ok(full);
    }
    let max_section =
        (context::input_budget(caps.context, 3_000) / 2).clamp(2_000, context::MAX_SECTION_TOKENS);
    let Some(query) = query.filter(|q| !q.trim().is_empty()) else {
        let notes = condensed_transcript(ai, rid, title, segs, budget, max_section).await?;
        return Ok(TranscriptText {
            text: notes,
            condensed: true,
        });
    };
    let notes = condensed_transcript(ai, rid, title, segs, budget / 2, max_section).await?;
    let room = budget.saturating_sub(context::estimate_tokens(&notes));
    let lines: Vec<String> = segs.iter().map(llm::transcript_line).collect();
    let toks: Vec<usize> = lines.iter().map(|l| context::estimate_tokens(l)).collect();
    let hits = question_hits(ai.app, rid, query, segs).await;
    let mut excerpts = String::new();
    let mut prev: Option<usize> = None;
    for i in context::select_excerpts(&toks, &hits, 2, room) {
        if prev.is_some_and(|p| p + 1 != i) {
            excerpts.push_str("…\n");
        }
        excerpts.push_str(&lines[i]);
        prev = Some(i);
    }
    if excerpts.is_empty() {
        excerpts.push_str("(no passages matched the question)\n");
    }
    Ok(TranscriptText {
        text: format!("{notes}\n\n## Verbatim excerpts for the question\n{excerpts}"),
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

/// Text to search the transcript or library with: the typed question, plus what voice
/// messages say (transcribing them now if nothing was typed).
async fn retrieval_query(
    ai: &Ai<'_>,
    caps: &Caps,
    question: &str,
    attachments: &mut [ChatAttachment],
) -> Result<String> {
    let mut q = question.to_string();
    for a in attachments.iter_mut().filter(|a| a.kind == "audio") {
        if a.text.is_none() && q.trim().is_empty() {
            if let Err(e) = transcribe_attachment(ai, a).await {
                ai.check_cancel()?;
                log::info!("voice message not transcribed for search: {e:#}");
            }
        }
        if let Some(t) = &a.text {
            q.push(' ');
            q.push_str(t);
        }
    }
    // nothing typed or said: search with what the images show
    if q.trim().is_empty() {
        for a in attachments.iter_mut().filter(|a| a.kind == "image") {
            if a.text.is_none() {
                if let Err(e) = describe_attachment(ai, caps, a, question).await {
                    ai.check_cancel()?;
                    log::info!("image not described for search: {e:#}");
                }
            }
            if let Some(t) = &a.text {
                q.push(' ');
                q.push_str(t);
            }
        }
    }
    Ok(q.trim().to_string())
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
    let transcript = fit_transcript(ai, &caps, id, &rec.title, &segs, room, None).await?;
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

/// What a question used, for usage statistics (counts and flags only).
#[derive(Default)]
struct AskStats {
    images: usize,
    voice: usize,
    condensed: bool,
    compacted: bool,
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
    track_ai(
        app,
        kind,
        &model,
        started,
        &r,
        usage,
        serde_json::json!({
            "images": stats.images,
            "voice_messages": stats.voice,
            "transcript_condensed": stats.condensed,
            "history_compacted": stats.compacted,
            "new_thread": new_thread,
        }),
    );
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
    let scope = match rid {
        Some(id) => {
            let db = state.db.lock().unwrap();
            let segs = db.segments(id)?;
            if segs.is_empty() {
                bail!("transcribe the recording before asking about it");
            }
            Some((db.recording(id)?, segs, db.analysis(id)?, db.notes(id)?))
        }
        None => None,
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
    // the transcript
    let budget = context::input_budget(caps.context, ANSWER_TOKENS);
    let history_budget = (budget * HISTORY_SHARE / 100).min(HISTORY_MAX_TOKENS);
    let about = match &scope {
        Some((rec, ..)) => format!("the recording \"{}\"", rec.title),
        None => "the user's library of transcribed recordings".into(),
    };
    let (summary, kept_from) = match &thread {
        Some(t) => {
            let (summary, from, compacted) =
                compact_history(ai, &caps, t, &mut prior, history_budget, &about).await?;
            stats.compacted = compacted;
            (summary, from)
        }
        None => (None, 0),
    };
    let kept = &mut prior[kept_from..];
    let plan = history_file_plan(&caps, kept, speech);
    let mut history = Vec::with_capacity(kept.len());
    for (m, files) in kept.iter_mut().zip(plan) {
        if m.role != "user" {
            history.push(Msg::assistant(m.content.clone()));
            continue;
        }
        let mut p = Vec::new();
        if !m.content.trim().is_empty() {
            p.push(Part::text(m.content.clone()));
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

    let summary_block = summary
        .as_deref()
        .map(|s| {
            format!(
                "\n\nSummary of the earlier part of this conversation (those messages aren't shown):\n{s}"
            )
        })
        .unwrap_or_default();
    let fixed = prompt_tokens(&history)
        + parts.iter().map(Part::tokens).sum::<usize>()
        + context::estimate_tokens(&summary_block)
        + 64;
    let (system, mut rewriter, condensed) = match scope {
        Some((rec, segs, analysis, notes)) => {
            let empty = TranscriptText {
                text: String::new(),
                condensed: true,
            };
            let overhead = context::estimate_tokens(&llm::qa_system(
                &rec.title,
                &empty,
                analysis.as_ref(),
                &notes,
                &settings.ai_language,
            ));
            let room = budget.saturating_sub(fixed + overhead).max(2_000);
            let fits = context::estimate_tokens(&TranscriptText::full(&segs).text) <= room;
            let query = if fits {
                question.clone()
            } else {
                retrieval_query(ai, &caps, &question, &mut attachments).await?
            };
            let transcript =
                fit_transcript(ai, &caps, &rec.id, &rec.title, &segs, room, Some(&query)).await?;
            let system = llm::qa_system(
                &rec.title,
                &transcript,
                analysis.as_ref(),
                &notes,
                &settings.ai_language,
            );
            (system, None, transcript.condensed)
        }
        None => {
            let query = retrieval_query(ai, &caps, &question, &mut attachments).await?;
            let room = budget.saturating_sub(fixed + 600).max(2_000);
            let (system, ids) = library_context(app, &query, room).await?;
            (system, Some(CitationRewriter::new(ids)), false)
        }
    };
    stats.condensed = condensed;
    let mut messages = vec![Msg::system(system + &summary_block)];
    messages.extend(history);
    messages.push(Msg::user_parts(parts));

    ai.check_cancel()?;
    let (answer, u) = ai
        .client
        .stream(
            &ai.model,
            &messages,
            ANSWER_TOKENS as u32,
            &job.cancel,
            &mut |d| {
                let out = match rewriter.as_mut() {
                    Some(r) => r.push(d),
                    None => d.to_string(),
                };
                if !out.is_empty() {
                    job.emit(Some(out), false, None);
                }
            },
        )
        .await?;
    ai.add_usage(u);
    let answer = match rewriter.as_mut() {
        Some(r) => {
            let tail = r.finish();
            if !tail.is_empty() {
                job.emit(Some(tail), false, None);
            }
            r.rewrite_all(&answer)
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
        context_note: condensed
            .then(|| "The transcript was condensed to fit this model's context.".into()),
    };
    db.add_chat(&answered)?;
    Ok(AskResult {
        thread: db.thread(&thread.id)?,
        question: asked,
        answer: answered,
    })
}

/// Retrieve excerpts from the whole library for a question (lexical + semantic + tags).
async fn library_context(
    app: &AppHandle,
    question: &str,
    max_tokens: usize,
) -> Result<(String, Vec<String>)> {
    let state = app.state::<AppState>();
    let settings = state.settings();
    let semantic = if question.trim().is_empty() {
        vec![]
    } else {
        semantic_search(app, question, None, 24).await
    };
    let (lexical, tags) = {
        let db = state.db.lock().unwrap();
        // match on individual meaningful words, not just the full phrase
        let mut lex = Vec::new();
        for term in search_words(question) {
            lex.extend(search::lexical(&db, &term, None, 8)?);
        }
        (lex, search::tag_hits(&db, question, None)?)
    };
    let hits = search::merge_hits(vec![semantic, lexical, tags], 40);
    if hits.is_empty() {
        let n = state.db.lock().unwrap().list_recordings()?.len();
        if n == 0 {
            bail!("your library is empty; import and transcribe some recordings first");
        }
    }
    // group hits by recording, with one segment of context on each side
    let mut by_rec: BTreeMap<String, (String, Vec<u32>)> = BTreeMap::new();
    for h in &hits {
        let e = by_rec
            .entry(h.recording_id.clone())
            .or_insert_with(|| (h.recording_title.clone(), vec![]));
        for s in h.segment_id.saturating_sub(1)..=h.segment_id + 1 {
            e.1.push(s);
        }
    }
    let mut items = Vec::new();
    let mut ids = Vec::new();
    {
        let db = state.db.lock().unwrap();
        for (rid, (title, mut wanted)) in by_rec {
            wanted.sort_unstable();
            wanted.dedup();
            let segs = db.segments(&rid)?;
            let picked: Vec<_> = segs
                .into_iter()
                .filter(|s| wanted.binary_search(&s.id).is_ok())
                .collect();
            ids.push(rid.clone());
            items.push(LibraryContextItem {
                recording_id: rid,
                recording_title: title,
                segments: picked,
            });
        }
    }
    Ok((
        llm::library_qa_system(&items, &settings.ai_language, max_tokens),
        ids,
    ))
}

/// The transcript for a doc request, within what the model's context leaves after `overhead`.
async fn doc_transcript(
    ai: &Ai<'_>,
    rid: &str,
    title: &str,
    segs: &[Segment],
    overhead: usize,
) -> Result<TranscriptText> {
    let caps = caps(ai.app, &ai.model).await;
    let room = context::input_budget(caps.context, DOC_TOKENS)
        .min(DOC_TRANSCRIPT_MAX_TOKENS)
        .saturating_sub(overhead)
        .max(2_000);
    fit_transcript(ai, &caps, rid, title, segs, room, None).await
}

fn doc_overhead(prompt: &str, title: &str, analysis: Option<&Analysis>, lang: &str) -> usize {
    let empty = TranscriptText {
        text: String::new(),
        condensed: true,
    };
    context::estimate_tokens(&llm::doc_system(lang))
        + context::estimate_tokens(&llm::doc_user(prompt, title, &empty, analysis))
}

pub async fn generate_doc(
    app: &AppHandle,
    id: String,
    prompt: String,
    request_id: String,
) -> Result<Doc> {
    let job = LlmJob::start(app, &request_id);
    let started = std::time::Instant::now();
    let model = app.state::<AppState>().settings().llm_model;
    let (r, usage) = match Ai::new(app, Some(&job)) {
        Ok(ai) => {
            let r = until_stopped(&job, generate_doc_inner(&ai, &job, &id, &prompt)).await;
            (r, ai.usage())
        }
        Err(e) => (Err(e), None),
    };
    job.finish(&r);
    track_ai(
        app,
        "generate_doc",
        &model,
        started,
        &r,
        usage,
        serde_json::json!({}),
    );
    r
}

async fn generate_doc_inner(ai: &Ai<'_>, job: &LlmJob, id: &str, prompt: &str) -> Result<Doc> {
    let state = ai.app.state::<AppState>();
    let settings = state.settings();
    let (rec, segs, analysis) = {
        let db = state.db.lock().unwrap();
        (db.recording(id)?, db.segments(id)?, db.analysis(id)?)
    };
    if segs.is_empty() {
        bail!("transcribe the recording before generating docs");
    }
    let overhead = doc_overhead(prompt, &rec.title, analysis.as_ref(), &settings.ai_language);
    let transcript = doc_transcript(ai, id, &rec.title, &segs, overhead).await?;
    let messages = vec![
        Msg::system(llm::doc_system(&settings.ai_language)),
        Msg::user(llm::doc_user(
            prompt,
            &rec.title,
            &transcript,
            analysis.as_ref(),
        )),
    ];
    ai.check_cancel()?;
    let (raw, u) = ai
        .client
        .stream(
            &ai.model,
            &messages,
            DOC_TOKENS as u32,
            &job.cancel,
            &mut |d| job.emit(Some(d.to_string()), false, None),
        )
        .await?;
    ai.add_usage(u);
    let html = llm::extract_html(&raw)?;
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
    state.db.lock().unwrap().save_doc(&doc)?;
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
    let (r, usage) = match Ai::new(app, Some(&job)) {
        Ok(ai) => {
            let r = until_stopped(&job, revise_doc_inner(&ai, &job, &doc_id, &instruction)).await;
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
        serde_json::json!({}),
    );
    r
}

async fn revise_doc_inner(
    ai: &Ai<'_>,
    job: &LlmJob,
    doc_id: &str,
    instruction: &str,
) -> Result<Doc> {
    let state = ai.app.state::<AppState>();
    let settings = state.settings();
    let (doc, rec, segs, analysis) = {
        let db = state.db.lock().unwrap();
        let doc = db.doc(doc_id).map_err(|_| anyhow!("doc not found"))?;
        let rid = doc.recording_id.clone();
        (
            doc,
            db.recording(&rid)?,
            db.segments(&rid)?,
            db.analysis(&rid)?,
        )
    };
    let revise = format!(
        "Revise the document: {instruction}\nReturn the complete updated HTML document only."
    );
    let overhead = doc_overhead(
        &doc.prompt,
        &rec.title,
        analysis.as_ref(),
        &settings.ai_language,
    ) + context::estimate_tokens(&doc.html)
        + context::estimate_tokens(&revise);
    let transcript = doc_transcript(ai, &rec.id, &rec.title, &segs, overhead).await?;
    let messages = vec![
        Msg::system(llm::doc_system(&settings.ai_language)),
        Msg::user(llm::doc_user(
            &doc.prompt,
            &rec.title,
            &transcript,
            analysis.as_ref(),
        )),
        Msg::assistant(doc.html.clone()),
        Msg::user(revise),
    ];
    ai.check_cancel()?;
    let (raw, u) = ai
        .client
        .stream(
            &ai.model,
            &messages,
            DOC_TOKENS as u32,
            &job.cancel,
            &mut |d| job.emit(Some(d.to_string()), false, None),
        )
        .await?;
    ai.add_usage(u);
    let html = llm::extract_html(&raw)?;
    let updated = Doc {
        title: llm::html_title(&html).unwrap_or(doc.title.clone()),
        html,
        model: ai.model.clone(),
        updated_at: now_iso(),
        ..doc
    };
    state.db.lock().unwrap().save_doc(&updated)?;
    Ok(updated)
}
