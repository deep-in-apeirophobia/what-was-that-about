//! AI features on top of OpenRouter: analysis, embeddings, Q&A and interactive docs.

use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use anyhow::{anyhow, bail, Result};
use tauri::{AppHandle, Emitter, Manager};
use wwta_core::llm::{self, CitationRewriter, LibraryContextItem, Msg};
use wwta_core::search;
use wwta_core::types::{now_iso, Analysis, ChatMessage, Doc, LlmStreamEvent, SearchHit};

use crate::events;
use crate::state::AppState;
use crate::telemetry;

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

    fn emit(&self, delta: Option<String>, done: bool, error: Option<String>) {
        let _ = self.app.emit(
            events::LLM_STREAM,
            LlmStreamEvent {
                request_id: self.id.clone(),
                delta,
                done,
                error,
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

fn history(chat: &[ChatMessage], max: usize) -> Vec<Msg> {
    let start = chat.len().saturating_sub(max);
    chat[start..]
        .iter()
        .map(|m| Msg {
            role: m.role.clone(),
            content: m.content.clone(),
        })
        .collect()
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
    let r = ask_inner(app, recording_id, question, &job, &mut usage).await;
    job.finish(&r);
    track_ai(app, kind, &model, started, &r, usage, serde_json::json!({}));
    r
}

async fn ask_inner(
    app: &AppHandle,
    recording_id: Option<String>,
    question: String,
    job: &LlmJob,
    usage: &mut Option<llm::Usage>,
) -> Result<ChatMessage> {
    let (client, settings) = client(app)?;
    let state = app.state::<AppState>();
    let question = question.trim().to_string();
    if question.is_empty() {
        bail!("empty question");
    }
    let rid = recording_id.as_deref();
    let prior = state.db.lock().unwrap().chat(rid)?;
    let (system, rewriter) = match rid {
        Some(id) => {
            let (rec, segs, analysis, notes) = {
                let db = state.db.lock().unwrap();
                (
                    db.recording(id)?,
                    db.segments(id)?,
                    db.analysis(id)?,
                    db.notes(id)?,
                )
            };
            if segs.is_empty() {
                bail!("transcribe the recording before asking about it");
            }
            (
                llm::qa_system(
                    &rec.title,
                    &segs,
                    analysis.as_ref(),
                    &notes,
                    &settings.ai_language,
                ),
                None,
            )
        }
        None => {
            let (system, ids) = library_context(app, &question).await?;
            (system, Some(CitationRewriter::new(ids)))
        }
    };
    store_chat(app, rid, "user", &question, None)?;
    let mut messages = vec![Msg::system(system)];
    messages.extend(history(&prior, 12));
    messages.push(Msg::user(question));

    let mut rewriter = rewriter;
    let (answer, u) = client
        .stream(
            &settings.llm_model,
            &messages,
            4_000,
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
    *usage = u;
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
    store_chat(app, rid, "assistant", &answer, Some(&settings.llm_model))
}

/// Retrieve excerpts from the whole library for a question (lexical + semantic + tags).
async fn library_context(app: &AppHandle, question: &str) -> Result<(String, Vec<String>)> {
    let state = app.state::<AppState>();
    let settings = state.settings();
    let semantic = semantic_search(app, question, None, 24).await;
    let (lexical, tags) = {
        let db = state.db.lock().unwrap();
        // match on individual meaningful words, not just the full phrase
        let mut lex = Vec::new();
        for term in question
            .split_whitespace()
            .filter(|t| t.chars().count() >= 3)
        {
            lex.extend(search::lexical(&db, term, None, 8)?);
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
    Ok((llm::library_qa_system(&items, &settings.ai_language), ids))
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
    let mut usage = None;
    let r = async {
        let (client, settings) = client(app)?;
        let state = app.state::<AppState>();
        let (rec, segs, analysis) = {
            let db = state.db.lock().unwrap();
            (db.recording(&id)?, db.segments(&id)?, db.analysis(&id)?)
        };
        if segs.is_empty() {
            bail!("transcribe the recording before generating docs");
        }
        let messages = vec![
            Msg::system(llm::doc_system(&settings.ai_language)),
            Msg::user(llm::doc_user(&prompt, &rec.title, &segs, analysis.as_ref())),
        ];
        let (raw, u) = client
            .stream(
                &settings.llm_model,
                &messages,
                32_000,
                &job.cancel,
                &mut |d| job.emit(Some(d.to_string()), false, None),
            )
            .await?;
        usage = u;
        let html = llm::extract_html(&raw)?;
        let now = now_iso();
        let doc = Doc {
            id: uuid::Uuid::new_v4().to_string(),
            recording_id: id.clone(),
            title: llm::html_title(&html).unwrap_or_else(|| prompt.chars().take(60).collect()),
            prompt: prompt.clone(),
            html,
            model: settings.llm_model.clone(),
            created_at: now.clone(),
            updated_at: now,
        };
        state.db.lock().unwrap().save_doc(&doc)?;
        Ok(doc)
    }
    .await;
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
    let r = async {
        let (client, settings) = client(app)?;
        let state = app.state::<AppState>();
        let (doc, rec, segs, analysis) = {
            let db = state.db.lock().unwrap();
            let doc = db.doc(&doc_id).map_err(|_| anyhow!("doc not found"))?;
            let rid = doc.recording_id.clone();
            (doc, db.recording(&rid)?, db.segments(&rid)?, db.analysis(&rid)?)
        };
        let messages = vec![
            Msg::system(llm::doc_system(&settings.ai_language)),
            Msg::user(llm::doc_user(&doc.prompt, &rec.title, &segs, analysis.as_ref())),
            Msg::assistant(doc.html.clone()),
            Msg::user(format!(
                "Revise the document: {instruction}\nReturn the complete updated HTML document only."
            )),
        ];
        let (raw, u) = client
            .stream(&settings.llm_model, &messages, 32_000, &job.cancel, &mut |d| {
                job.emit(Some(d.to_string()), false, None)
            })
            .await?;
        usage = u;
        let html = llm::extract_html(&raw)?;
        let updated = Doc {
            title: llm::html_title(&html).unwrap_or(doc.title.clone()),
            html,
            model: settings.llm_model.clone(),
            updated_at: now_iso(),
            ..doc
        };
        state.db.lock().unwrap().save_doc(&updated)?;
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
        serde_json::json!({}),
    );
    r
}
