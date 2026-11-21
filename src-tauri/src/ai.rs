//! AI features on top of OpenRouter: analysis, embeddings, and the tool-using agent behind
//! Q&A and interactive docs.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};
use wwta_core::agent::tools::{Scope, ToolBox};
use wwta_core::agent::{self, add_usage, Agent, Context, Host, RecordingFacts, Stats};
use wwta_core::llm::{self, rewrite_citations, CitationRewriter, Msg, ToolCall};
use wwta_core::search;
use wwta_core::types::{now_iso, Analysis, ChatMessage, Doc, LlmStreamEvent, Recording, SearchHit};

use crate::events;
use crate::state::AppState;
use crate::telemetry;

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

fn client(app: &AppHandle) -> Result<(llm::Client, wwta_core::types::Settings)> {
    let s = app.state::<AppState>().settings();
    Ok((llm::Client::new(&s.openrouter_api_key)?, s))
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

    fn send(&self, ev: LlmStreamEvent) {
        let _ = self.app.emit(events::LLM_STREAM, ev);
    }

    fn event(&self) -> LlmStreamEvent {
        LlmStreamEvent {
            request_id: self.id.clone(),
            delta: None,
            done: false,
            error: None,
            activity: None,
            reset: false,
        }
    }

    fn delta(&self, text: String) {
        self.send(LlmStreamEvent {
            delta: Some(text),
            ..self.event()
        });
    }

    /// Show what the agent is doing (e.g. which transcript it is reading).
    fn activity(&self, label: String) {
        self.send(LlmStreamEvent {
            activity: Some(label),
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

pub async fn analyze(app: &AppHandle, id: &str) -> Result<Analysis> {
    let started = std::time::Instant::now();
    let model = app.state::<AppState>().settings().llm_model;
    let mut usage = None;
    let r = analyze_inner(app, id, &mut usage).await;
    let extra = match &r {
        Ok(a) => serde_json::json!({ "chapters": a.chapters.len(), "tags": a.tags.len() }),
        Err(_) => serde_json::json!({}),
    };
    track_ai(app, "analyze", &model, started, &r, usage, extra);
    r
}

async fn analyze_inner(
    app: &AppHandle,
    id: &str,
    usage: &mut Option<llm::Usage>,
) -> Result<Analysis> {
    let (client, settings) = client(app)?;
    let state = app.state::<AppState>();
    let (rec, segs) = {
        let db = state.db.lock().unwrap();
        (db.recording(id)?, db.segments(id)?)
    };
    if segs.is_empty() {
        bail!("transcribe the recording before analyzing it");
    }
    let messages = llm::analysis_messages(&rec.title, &segs, &settings.ai_language);
    let model = settings.llm_model.clone();
    let (raw, u) = match client.complete(&model, &messages, true, 16_000).await {
        Ok(r) => r,
        // some providers reject response_format; retry without it
        Err(e) if e.to_string().contains("response_format") => {
            client.complete(&model, &messages, false, 16_000).await?
        }
        Err(e) => return Err(e),
    };
    add_usage(usage, u);
    let value = match llm::extract_json(&raw) {
        Ok(v) => v,
        Err(first) => {
            log::warn!("analysis JSON invalid ({first:#}); asking the model to repair it");
            let mut repair = messages.clone();
            repair.push(Msg::assistant(raw));
            repair.push(Msg::user(
                "That was not valid JSON. Reply again with only the corrected JSON object.",
            ));
            let (fixed, u) = client.complete(&model, &repair, false, 16_000).await?;
            add_usage(usage, u);
            llm::extract_json(&fixed)?
        }
    };
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
    let (client, settings) = client(app)?;
    let state = app.state::<AppState>();
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
    state
        .db
        .lock()
        .unwrap()
        .replace_embeddings(id, &settings.embedding_model, &rows)?;
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

fn store_chat(
    app: &AppHandle,
    recording_id: Option<&str>,
    role: &str,
    content: &str,
    model: Option<&str>,
) -> Result<ChatMessage> {
    let m = ChatMessage {
        id: uuid::Uuid::new_v4().to_string(),
        recording_id: recording_id.map(String::from),
        role: role.into(),
        content: content.into(),
        created_at: now_iso(),
        model: model.map(String::from),
    };
    app.state::<AppState>().db.lock().unwrap().add_chat(&m)?;
    Ok(m)
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
        self.job.activity(label);
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

fn stats_props(stats: Stats) -> serde_json::Value {
    serde_json::json!({
        "requests": stats.requests,
        "tool_calls": stats.tool_calls,
        "compactions": stats.compactions,
    })
}

pub async fn ask(
    app: &AppHandle,
    recording_id: Option<String>,
    question: String,
    request_id: String,
) -> Result<ChatMessage> {
    let job = LlmJob::start(app, &request_id);
    let started = std::time::Instant::now();
    let model = app.state::<AppState>().settings().llm_model;
    let kind = if recording_id.is_some() {
        "ask"
    } else {
        "ask_library"
    };
    let mut usage = None;
    let mut stats = Stats::default();
    let r = ask_inner(app, recording_id, question, &job, &mut usage, &mut stats).await;
    job.finish(&r);
    track_ai(app, kind, &model, started, &r, usage, stats_props(stats));
    r
}

async fn ask_inner(
    app: &AppHandle,
    recording_id: Option<String>,
    question: String,
    job: &LlmJob,
    usage: &mut Option<llm::Usage>,
    stats: &mut Stats,
) -> Result<ChatMessage> {
    let (client, settings) = client(app)?;
    let state = app.state::<AppState>();
    let question = question.trim().to_string();
    if question.is_empty() {
        bail!("empty question");
    }
    let rid = recording_id.as_deref();
    let (system, summary, prior) = {
        let db = state.db.lock().unwrap();
        let system = match rid {
            Some(id) => {
                let rec = db.recording(id)?;
                if rec.segment_count == 0 {
                    bail!("transcribe the recording before asking about it");
                }
                let notes = db.notes(id)?.len();
                agent::recording_chat_system(&facts(&rec, notes), &settings.ai_language)
            }
            None => {
                let n = db.list_recordings()?.len();
                if n == 0 {
                    bail!("your library is empty; import and transcribe some recordings first");
                }
                agent::library_chat_system(n, &settings.ai_language)
            }
        };
        // messages already folded into a summary are replaced by it
        let chat = db.chat(rid)?;
        let (summary, prior) = match db.chat_summary(rid)? {
            Some((through, text)) => match chat.iter().position(|m| m.id == through) {
                Some(i) => (Some(text), chat[i + 1..].to_vec()),
                None => (None, chat),
            },
            None => (None, chat),
        };
        (system, summary, prior)
    };
    let mut host = ToolHost::new(app, job, rid, false);
    // stored answers cite `[rec:<uuid>#14]`; show the model the aliases its tools use
    let summary = summary.map(|s| host.tools.aliasify(&s));
    let history: Vec<Msg> = prior
        .iter()
        .map(|m| {
            let text = host.tools.aliasify(&m.content);
            if m.role == "user" {
                Msg::user(text)
            } else {
                Msg::assistant(text)
            }
        })
        .collect();
    store_chat(app, rid, "user", &question, None)?;

    let ctx = Context::new(system, summary, history, Msg::user(question));
    let mut agent = Agent::new(&client, &settings.llm_model, 4_000, &job.cancel, ctx);
    let result = agent.run(&mut host).await;
    *usage = agent.usage;
    *stats = agent.stats;
    // remember what was summarized so later questions don't summarize it again
    if let (Some(summary), Some(last)) = (
        &agent.ctx.summary,
        agent.ctx.folded.checked_sub(1).and_then(|i| prior.get(i)),
    ) {
        let summary = rewrite_citations(summary, host.tools.recording_ids());
        state
            .db
            .lock()
            .unwrap()
            .save_chat_summary(rid, &last.id, &summary)?;
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
    store_chat(app, rid, "assistant", &answer, Some(&settings.llm_model))
}

/// Let the doc agent work on one recording and return the HTML it wrote.
async fn write_doc(
    app: &AppHandle,
    job: &LlmJob,
    recording_id: &str,
    request: String,
    usage: &mut Option<llm::Usage>,
    stats: &mut Stats,
) -> Result<String> {
    let (client, settings) = client(app)?;
    let state = app.state::<AppState>();
    let system = {
        let db = state.db.lock().unwrap();
        let rec = db.recording(recording_id)?;
        if rec.segment_count == 0 {
            bail!("transcribe the recording before generating docs");
        }
        let notes = db.notes(recording_id)?.len();
        agent::doc_system(&facts(&rec, notes), &settings.ai_language)
    };
    let ctx = Context::new(system, None, vec![], Msg::user(request));
    let mut host = ToolHost::new(app, job, Some(recording_id), true);
    let mut agent = Agent::new(&client, &settings.llm_model, 32_000, &job.cancel, ctx);
    let result = agent.run(&mut host).await;
    *usage = agent.usage;
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
    let mut usage = None;
    let mut stats = Stats::default();
    let r = async {
        let request = agent::doc_request(&prompt, skill.as_deref());
        let html = write_doc(app, &job, &id, request, &mut usage, &mut stats).await?;
        let now = now_iso();
        let doc = Doc {
            id: uuid::Uuid::new_v4().to_string(),
            recording_id: id.clone(),
            title: llm::html_title(&html).unwrap_or_else(|| prompt.chars().take(60).collect()),
            prompt: prompt.clone(),
            html,
            model: model.clone(),
            created_at: now.clone(),
            updated_at: now,
        };
        app.state::<AppState>().db.lock().unwrap().save_doc(&doc)?;
        Ok(doc)
    }
    .await;
    job.finish(&r);
    let mut props = stats_props(stats);
    // only the built-in template names, never free text
    if let Some(s) = skill.as_deref().and_then(agent::skills::get) {
        props["skill"] = s.name.as_str().into();
    }
    track_ai(app, "generate_doc", &model, started, &r, usage, props);
    r
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
    let mut usage = None;
    let mut stats = Stats::default();
    let r = async {
        let doc = app
            .state::<AppState>()
            .db
            .lock()
            .unwrap()
            .doc(&doc_id)
            .map_err(|_| anyhow!("doc not found"))?;
        let request = agent::doc_revision(&doc.prompt, &doc.html, &instruction);
        let html = write_doc(
            app,
            &job,
            &doc.recording_id,
            request,
            &mut usage,
            &mut stats,
        )
        .await?;
        let updated = Doc {
            title: llm::html_title(&html).unwrap_or(doc.title.clone()),
            html,
            model: model.clone(),
            updated_at: now_iso(),
            ..doc
        };
        app.state::<AppState>()
            .db
            .lock()
            .unwrap()
            .save_doc(&updated)?;
        Ok(updated)
    }
    .await;
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
