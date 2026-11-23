//! The tools the agent can call. Kept few and simple for small models: every argument is
//! required, results are plain text, recordings and docs get short ids (R3, D2) instead of
//! UUIDs, and arguments are parsed leniently ("R3", "3" and "[R3]" all work).

use regex::Regex;
use serde_json::{json, Value};

use super::draft::{check_html, Draft, READ_LINES};
use super::{clip, skills};
use crate::db::Db;
use crate::llm::{extract_html, ToolCall};
use crate::search;
use crate::text::fmt_time;
use crate::types::SearchHit;

/// Transcript text per read: about 10–15 minutes of Persian speech (~10k tokens). Longer
/// requests are cut at a segment boundary and say where to continue.
pub const WINDOW_CHARS: usize = 24_000;
const DOC_CHARS: usize = 80_000;
/// Recordings per `list_recordings` page.
const PAGE_SIZE: usize = 100;
const SEARCH_LIMIT: usize = 20;

#[derive(Debug, Clone, PartialEq)]
pub enum Scope {
    /// Library chat: every recording, addressed by alias (R1, R2…).
    Library,
    /// One recording (its chat, or doc generation); tools take no recording argument.
    Recording(String),
}

pub struct ToolBox {
    scope: Scope,
    /// Offer `load_skill` and draft tools (doc generation).
    docs: bool,
    /// Recording UUIDs by alias: R1 is `recordings[0]`. Assigned on first sight.
    recordings: Vec<String>,
    /// Doc UUIDs by alias: D1 is `doc_aliases[0]`.
    doc_aliases: Vec<String>,
    /// The doc being built (doc mode only).
    draft: Draft,
    /// Segment ranges returned by transcript reads.
    read_ranges: Vec<(u32, u32)>,
    /// Last segment id, cached for coverage and statistics.
    transcript_last: Option<u32>,
    /// Draft changed since the last transcript read.
    edited_since_read: bool,
    nudges: u32,
}

/// A completed draft, or a valid HTML answer from a model that ignored the tools.
pub struct DocOutput {
    pub html: String,
    pub source: &'static str,
}

/// Counts and flags only, safe for telemetry.
#[derive(Debug, Default, Clone, Copy)]
pub struct DocStats {
    pub writes: u32,
    pub edits: u32,
    pub failed_edits: u32,
    pub reads: u32,
    pub finish_attempts: u32,
    pub finished: bool,
    /// Share of the transcript the model read (0–100).
    pub transcript_read_pct: u8,
}

const MAX_NUDGES: u32 = 2;

/// A tool failed in a way the model can fix (bad id, missing argument…).
type ToolResult = std::result::Result<String, String>;

impl ToolBox {
    pub fn new(scope: Scope, docs: bool) -> Self {
        Self {
            scope,
            docs,
            recordings: vec![],
            doc_aliases: vec![],
            draft: Draft::default(),
            read_ranges: vec![],
            transcript_last: None,
            edited_since_read: false,
            nudges: 0,
        }
    }

    /// Load the existing doc for an in-place revision.
    pub fn set_draft(&mut self, html: &str) {
        self.draft = Draft::load(html);
    }

    /// The current in-memory doc file.
    pub fn draft(&self) -> &Draft {
        &self.draft
    }

    /// A successful finish call ends a doc run.
    pub fn finished(&self) -> bool {
        self.docs && self.draft.finished
    }

    /// Remind a model that answered early to finish its draft, at most twice.
    pub fn follow_up(&mut self, answer: &str) -> Option<String> {
        if !self.docs || self.finished() || self.nudges >= MAX_NUDGES {
            return None;
        }
        if !self.draft.dirty
            && extract_html(answer).is_ok_and(|html| check_html(&html).errors.is_empty())
        {
            return None;
        }
        self.nudges += 1;
        Some(if self.draft.is_empty() {
            "You haven't written the doc yet. Write the complete file with write_draft (start from the skeleton), add the content with edit_draft, then call finish_doc.".into()
        } else if !self.draft.dirty {
            "You haven't changed the doc yet. Read the part to change with read_draft, change it with edit_draft, then call finish_doc.".into()
        } else {
            let check = self.draft.check();
            let problems = check
                .errors
                .into_iter()
                .chain(check.warnings)
                .collect::<Vec<_>>();
            if problems.is_empty() {
                "The doc isn't finished. If it's complete, call finish_doc; otherwise keep adding to it with edit_draft.".into()
            } else {
                format!("The doc isn't finished. It has these problems:\n- {}\nFix them with edit_draft, then call finish_doc.", problems.join("\n- "))
            }
        })
    }

    /// Select a complete draft or a valid fallback answer; never save a truncated file.
    pub fn doc_result(&self, answer: &str) -> anyhow::Result<DocOutput> {
        let check = self.draft.check();
        if self.draft.dirty && check.errors.is_empty() {
            return Ok(DocOutput {
                html: self.draft.html().to_string(),
                source: "draft",
            });
        }
        if let Ok(html) = extract_html(answer) {
            if check_html(&html).errors.is_empty() {
                return Ok(DocOutput {
                    html,
                    source: "answer",
                });
            }
        }
        if self.draft.dirty {
            anyhow::bail!("the doc is incomplete: {}", check.errors.join("; "));
        }
        if !self.draft.is_empty() {
            anyhow::bail!("the model made no changes to the doc");
        }
        anyhow::bail!("the model didn't write a doc")
    }

    /// Counts from this run, without any recording or doc content.
    pub fn doc_stats(&self) -> DocStats {
        let covered: u64 = self
            .merged_ranges()
            .iter()
            .map(|&(a, b)| u64::from(b) - u64::from(a) + 1)
            .sum();
        let total = self.transcript_last.map_or(0, |last| u64::from(last) + 1);
        DocStats {
            writes: self.draft.writes,
            edits: self.draft.edits,
            failed_edits: self.draft.failed_edits,
            reads: self.draft.reads,
            finish_attempts: self.draft.finish_attempts,
            finished: self.finished(),
            transcript_read_pct: (covered * 100).checked_div(total).unwrap_or(0).min(100) as u8,
        }
    }

    fn merged_ranges(&self) -> Vec<(u32, u32)> {
        let mut ranges = self.read_ranges.clone();
        ranges.sort_unstable();
        let mut merged: Vec<(u32, u32)> = vec![];
        for (from, to) in ranges {
            if let Some(last) = merged.last_mut() {
                if from <= last.1.saturating_add(1) {
                    last.1 = last.1.max(to);
                    continue;
                }
            }
            merged.push((from, to));
        }
        merged
    }

    fn coverage(&self) -> String {
        let Some(last) = self.transcript_last else {
            return "Transcript read so far: no transcript.".into();
        };
        let ranges = self.merged_ranges();
        if ranges.is_empty() {
            return format!("Transcript read so far: nothing of 0–{last}.");
        }
        if ranges == [(0, last)] {
            return format!("Transcript read so far: all of 0–{last}.");
        }
        let show = |&(a, b): &(u32, u32)| {
            if a == b {
                a.to_string()
            } else {
                format!("{a}–{b}")
            }
        };
        let mut unread = vec![];
        let mut from = 0u64;
        for &(a, b) in &ranges {
            if u64::from(a) > from {
                unread.push((from as u32, a - 1));
            }
            from = u64::from(b) + 1;
        }
        if from <= u64::from(last) {
            unread.push((from as u32, last));
        }
        let mut missing = unread.iter().take(3).map(show).collect::<Vec<_>>();
        if unread.len() > 3 {
            missing.push("…".into());
        }
        format!(
            "Transcript read so far: {} ({}% of 0–{last}). Unread: {}.",
            ranges.iter().map(show).collect::<Vec<_>>().join(", "),
            self.doc_stats().transcript_read_pct,
            missing.join(", ")
        )
    }

    /// Recording UUIDs by alias, for [`crate::llm::rewrite_citations`].
    pub fn recording_ids(&self) -> &[String] {
        &self.recordings
    }

    fn library(&self) -> bool {
        self.scope == Scope::Library
    }

    /// The alias (R3) for a recording, assigning the next one on first sight.
    pub fn alias(&mut self, id: &str) -> String {
        let n = match self.recordings.iter().position(|r| r == id) {
            Some(i) => i + 1,
            None => {
                self.recordings.push(id.to_string());
                self.recordings.len()
            }
        };
        format!("R{n}")
    }

    fn doc_alias(&mut self, id: &str) -> String {
        let n = match self.doc_aliases.iter().position(|d| d == id) {
            Some(i) => i + 1,
            None => {
                self.doc_aliases.push(id.to_string());
                self.doc_aliases.len()
            }
        };
        format!("D{n}")
    }

    /// Turn stored `[rec:<uuid>#14]` citations back into aliases (`[R2#14]`), so earlier
    /// answers read the same way as tool results.
    pub fn aliasify(&mut self, text: &str) -> String {
        static RE: std::sync::LazyLock<Regex> =
            std::sync::LazyLock::new(|| Regex::new(r"rec:([^\]#\s,،;]+)#").unwrap());
        if !self.library() {
            return text.to_string();
        }
        RE.replace_all(text, |c: &regex::Captures| {
            format!("{}#", self.alias(&c[1]))
        })
        .into_owned()
    }

    /// A segment reference in the citation format of this scope: `[14]` / `[R2#14-20]`.
    fn cite(&mut self, rid: &str, from: u32, to: u32) -> String {
        let range = if to > from {
            format!("{from}-{to}")
        } else {
            from.to_string()
        };
        if self.library() {
            format!("[{}#{range}]", self.alias(rid))
        } else {
            format!("[{range}]")
        }
    }

    pub fn definitions(&self) -> Vec<Value> {
        let lib = self.library();
        let rec = if lib { "a" } else { "this" };
        let mut defs = vec![];
        if lib {
            defs.push(def(
                "list_recordings",
                "List the recordings in the library, newest first, with their ids (like R3), dates, lengths and short descriptions.",
                &[(
                    "page",
                    "integer",
                    "1 for the newest 100 recordings, 2 for the next 100, and so on.",
                )],
            ));
        }
        let search = if lib {
            "Find where words or a topic come up in all transcripts and the user's notes. Returns matching lines, best first."
        } else {
            "Find where words or a topic come up in this recording's transcript and the user's notes. Returns matching lines, best first."
        };
        defs.push(def(
            "search",
            search,
            &[("query", "string", "Words or a short phrase to look for.")],
        ));
        let recording = ("recording", "string", "Recording id like R3.");
        let with_rec = |extra: &[(&'static str, &'static str, &'static str)]| {
            let mut p = if lib { vec![recording] } else { vec![] };
            p.extend_from_slice(extra);
            p
        };
        defs.push(def(
            "get_overview",
            &format!("Get the AI summary, key points, chapters with their segment ranges, topics and action items of {rec} recording, and the docs generated for it."),
            &with_rec(&[]),
        ));
        defs.push(def(
            "read_transcript",
            &format!("Read the whole transcript of {rec} recording. Each line is [segment] (time) text. Very long transcripts are cut; continue with read_transcript_section."),
            &with_rec(&[]),
        ));
        defs.push(def(
            "read_transcript_section",
            &format!("Read part of the transcript of {rec} recording: segments `from` to `to`, inclusive. Use it around a search result or for one chapter."),
            &with_rec(&[
                ("from", "integer", "First segment id."),
                ("to", "integer", "Last segment id."),
            ]),
        ));
        defs.push(def(
            "get_notes",
            &format!("Read the notes the user wrote on {rec} recording, with the segments each note is attached to."),
            &with_rec(&[]),
        ));
        defs.push(def(
            "read_doc",
            "Read the HTML of a generated doc. Doc ids like D2 come from get_overview.",
            &[("doc", "string", "Doc id like D2.")],
        ));
        if self.docs {
            let names: Vec<&str> = skills::loadable().map(|s| s.name.as_str()).collect();
            defs.push(json!({
                "type": "function",
                "function": {
                    "name": "load_skill",
                    "description": "Load the design guide for a kind of doc. Load the closest one before you write a doc.",
                    "parameters": {
                        "type": "object",
                        "properties": { "name": { "type": "string", "enum": names, "description": "Guide name." } },
                        "required": ["name"],
                    },
                },
            }));
            defs.push(def(
                "write_draft",
                "Save the doc file. Call it once with the complete HTML (skeleton, styles, the script that renders the content, and an empty data list ending in a marker comment), then grow it with edit_draft. Calling it again replaces the whole file.",
                &[("html", "string", "The whole HTML file.")],
            ));
            defs.push(def(
                "read_draft",
                "Read part of the doc file with line numbers: lines `from` to `to` (1-based, inclusive), at most 200 lines per call.",
                &[("from", "integer", "First line."), ("to", "integer", "Last line.")],
            ));
            defs.push(def(
                "edit_draft",
                "Change the doc file: replace `old` with `new`. `old` is a few lines copied exactly from the draft (without line numbers) and must occur only once. To add items to a list, use its end-marker comment as `old` and put the new items before that same comment in `new`.",
                &[("old", "string", "Exact text to replace, copied from the draft."), ("new", "string", "The replacement text.")],
            ));
            defs.push(def(
                "finish_doc",
                "Finish: check the draft and save it as the doc. Call it when the content is complete.",
                &[],
            ));
        }
        defs
    }

    /// The search text, when `call` is a search (the host adds semantic hits for it).
    pub fn search_query(call: &ToolCall) -> Option<String> {
        let args = parse_args(&call.arguments).ok()?;
        (call.name == "search")
            .then(|| arg_str(&args, &["query", "q", "text"]))
            .flatten()
    }

    /// A short description of a call for the UI, e.g. `Reading the transcript of “Title”`.
    pub fn label(&mut self, db: &Db, call: &ToolCall) -> String {
        let args = parse_args(&call.arguments).unwrap_or(Value::Null);
        let of = match (self.library(), self.recording(db, &args)) {
            (true, Ok(id)) => db
                .get_recording(&id)
                .ok()
                .flatten()
                .map(|r| format!(" of “{}”", r.title))
                .unwrap_or_default(),
            _ => String::new(),
        };
        match call.name.as_str() {
            "list_recordings" => match arg_u32(&args, &["page"]) {
                Some(p) if p > 1 => format!("Browsing the library (page {p})"),
                _ => "Browsing the library".into(),
            },
            "search" => format!(
                "Searching for “{}”",
                arg_str(&args, &["query", "q", "text"]).unwrap_or_default()
            ),
            "get_overview" => format!("Reading the overview{of}"),
            "read_transcript" => format!("Reading the transcript{of}"),
            "read_transcript_section" => match (
                arg_u32(&args, &["from", "start", "from_segment"]),
                arg_u32(&args, &["to", "end", "to_segment"]),
            ) {
                (Some(a), Some(b)) => format!("Reading segments {a}–{b}{of}"),
                (Some(a), None) => format!("Reading segments from {a}{of}"),
                _ => format!("Reading the transcript{of}"),
            },
            "write_draft" => "Writing the draft".into(),
            "edit_draft" if self.draft.is_empty() => "Editing the draft".into(),
            "edit_draft" => format!(
                "Adding to the draft ({} so far)",
                self.draft.summary().split_once(", ").unwrap().1
            ),
            "read_draft" if args.is_object() => {
                let (from, to) = draft_window(&args);
                format!("Reading the draft (lines {from}–{to})")
            }
            "read_draft" => "Reading the draft".into(),
            "finish_doc" => "Checking the doc".into(),
            "get_notes" => format!("Reading notes{of}"),
            "read_doc" => "Reading a doc".into(),
            "load_skill" => format!(
                "Loading the {} guide",
                arg_str(&args, &["name", "skill"]).unwrap_or_default()
            ),
            other => format!("Running {other}"),
        }
    }

    /// Run a call. Problems the model can fix come back as text, not as errors.
    pub fn run(&mut self, db: &Db, call: &ToolCall, semantic: Vec<SearchHit>) -> String {
        if self.docs && self.transcript_last.is_none() {
            if let Scope::Recording(id) = &self.scope {
                self.transcript_last = db.segments(id).ok().and_then(|s| s.last().map(|s| s.id));
            }
        }
        let out = match parse_args(&call.arguments) {
            Ok(args) => self.dispatch(db, &call.name, &args, semantic),
            Err(e) => Err(e),
        };
        match out {
            Ok(s) => s,
            Err(e) => format!("Error: {e}"),
        }
    }

    fn dispatch(
        &mut self,
        db: &Db,
        name: &str,
        args: &Value,
        semantic: Vec<SearchHit>,
    ) -> ToolResult {
        let known = self.definitions();
        let offered = known.iter().any(|d| d["function"]["name"] == name);
        if !offered {
            let names: Vec<&str> = known
                .iter()
                .filter_map(|d| d["function"]["name"].as_str())
                .collect();
            return Err(format!(
                "unknown tool `{name}`. Available: {}.",
                names.join(", ")
            ));
        }
        match name {
            "list_recordings" => {
                let page = arg_u32(args, &["page"]).unwrap_or(1).max(1) as usize;
                self.list_recordings(db, page)
            }
            "search" => self.search(db, args, semantic),
            "get_overview" => self.overview(db, args),
            "read_transcript" => {
                let id = self.recording(db, args)?;
                self.transcript(db, &id, None)
            }
            "read_transcript_section" => {
                let id = self.recording(db, args)?;
                let from = arg_u32(args, &["from", "start", "from_segment"])
                    .ok_or("`from` (a segment id) is required")?;
                let to = arg_u32(args, &["to", "end", "to_segment"]).unwrap_or(u32::MAX);
                self.transcript(db, &id, Some((from.min(to), from.max(to))))
            }
            "write_draft" => {
                let html = arg_text(args, &["html", "content", "text", "file"])
                    .ok_or("`html` is required")?;
                let out = self.draft.write(&html)?;
                self.edited_since_read = true;
                Ok(format!("{out}\n{}", self.coverage()))
            }
            "read_draft" => {
                let (from, to) = draft_window(args);
                self.draft.read(from, to)
            }
            "edit_draft" => {
                let old = arg_text(
                    args,
                    &[
                        "old",
                        "old_text",
                        "oldText",
                        "old_string",
                        "oldString",
                        "search",
                    ],
                )
                .ok_or("`old` is required")?;
                let new = arg_text(
                    args,
                    &[
                        "new",
                        "new_text",
                        "newText",
                        "new_string",
                        "newString",
                        "replace",
                        "replacement",
                    ],
                )
                .ok_or("`new` is required")?;
                let out = self.draft.edit(&old, &new)?;
                self.edited_since_read = true;
                Ok(format!("{out}\n{}", self.coverage()))
            }
            "finish_doc" => self.draft.finish(),
            "get_notes" => self.notes(db, args),
            "read_doc" => self.read_doc(db, args),
            "load_skill" => {
                let name = arg_str(args, &["name", "skill"]).ok_or("`name` is required")?;
                match skills::get(&name) {
                    Some(s) => Ok(s.body.clone()),
                    None => Err(format!(
                        "no guide named “{name}”. Available: {}.",
                        skills::loadable()
                            .map(|s| s.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )),
                }
            }
            _ => unreachable!("checked against the definitions"),
        }
    }

    /// The recording a call is about: the scope's own, or the one named by the argument.
    fn recording(&mut self, db: &Db, args: &Value) -> ToolResult {
        if let Scope::Recording(id) = &self.scope {
            return Ok(id.clone());
        }
        let raw = arg_str(args, &["recording", "recording_id", "id"])
            .ok_or("`recording` is required (an id like R3 from list_recordings or search)")?;
        let r = raw.trim().trim_matches(['[', ']', '"', '\'', ' ']);
        let r = r.split('#').next().unwrap_or(r);
        let n = r.strip_prefix(['R', 'r']).unwrap_or(r).parse::<usize>();
        if let Some(id) = n.ok().and_then(|n| self.recordings.get(n.wrapping_sub(1))) {
            return Ok(id.clone());
        }
        // a UUID or an exact title also work
        let recs = db.list_recordings().map_err(|e| e.to_string())?;
        if let Some(rec) = recs
            .iter()
            .find(|x| x.id == r || x.title.trim().eq_ignore_ascii_case(r))
        {
            self.alias(&rec.id);
            return Ok(rec.id.clone());
        }
        Err(format!(
            "unknown recording “{raw}”. Use an id like R3 from list_recordings or search."
        ))
    }

    fn list_recordings(&mut self, db: &Db, page: usize) -> ToolResult {
        let recs = db.list_recordings().map_err(|e| e.to_string())?;
        if recs.is_empty() {
            return Ok("The library is empty.".into());
        }
        let pages = recs.len().div_ceil(PAGE_SIZE);
        if page > pages {
            return Err(format!(
                "there are only {pages} pages ({} recordings).",
                recs.len()
            ));
        }
        let mut out = format!(
            "{} recordings, newest first. Page {page} of {pages}:\n",
            recs.len()
        );
        for r in recs.iter().skip((page - 1) * PAGE_SIZE).take(PAGE_SIZE) {
            let alias = self.alias(&r.id);
            let date = r.created_at.get(..10).unwrap_or(&r.created_at);
            let length = r.duration_sec.map(fmt_time).unwrap_or_else(|| "?".into());
            let state = if r.segment_count == 0 {
                "not transcribed yet".to_string()
            } else {
                format!("{} segments", r.segment_count)
            };
            out.push_str(&format!(
                "\n{alias} “{}” · {date} · {length} · {state}",
                r.title
            ));
            if !r.tags.is_empty() {
                out.push_str(&format!(" · topics: {}", r.tags.join(", ")));
            }
            if let Some(b) = r.blurb.as_deref().filter(|b| !b.is_empty()) {
                out.push_str(&format!("\n   {b}"));
            }
        }
        if page < pages {
            out.push_str(&format!("\n\nOlder recordings are on page {}.", page + 1));
        }
        Ok(out)
    }

    fn search(&mut self, db: &Db, args: &Value, semantic: Vec<SearchHit>) -> ToolResult {
        let q = arg_str(args, &["query", "q", "text"]).ok_or("`query` is required")?;
        let rid = match &self.scope {
            Scope::Recording(id) => Some(id.clone()),
            Scope::Library => None,
        };
        let rid = rid.as_deref();
        let err = |e: anyhow::Error| e.to_string();
        let mut lists = vec![semantic, search::lexical(db, &q, rid, 12).map_err(err)?];
        // also match single words, ranked below lines that contain the whole query
        let words: Vec<&str> = q
            .split_whitespace()
            .filter(|w| w.chars().count() >= 3)
            .collect();
        if words.len() > 1 {
            for w in words.iter().take(8) {
                let mut hits = search::lexical(db, w, rid, 5).map_err(err)?;
                hits.iter_mut().for_each(|h| h.score *= 0.8);
                lists.push(hits);
            }
        }
        lists.push(search::tag_hits(db, &q, rid).map_err(err)?);
        lists.push(search::note_hits(db, &q, rid).map_err(err)?);
        let hits = search::merge_hits(lists, SEARCH_LIMIT);
        if hits.is_empty() {
            return Ok(format!(
                "No matches for “{q}”. Try other words (a synonym, a shorter form of the word), \
                 or read the overview or transcript instead."
            ));
        }
        let mut out = format!("{} results for “{q}”, best first:\n", hits.len());
        for h in &hits {
            let cite = self.cite(&h.recording_id, h.segment_id, h.segment_id);
            let title = if self.library() {
                format!(" “{}”", h.recording_title)
            } else {
                String::new()
            };
            let text = clip(h.text.trim(), 400);
            if h.note_id.is_some() {
                out.push_str(&format!(
                    "\n{cite} ({}–{}){title} user's note: {text}",
                    fmt_time(h.start),
                    fmt_time(h.end)
                ));
            } else {
                out.push_str(&format!("\n{cite} ({}){title} {text}", fmt_time(h.start)));
            }
        }
        Ok(out)
    }

    fn overview(&mut self, db: &Db, args: &Value) -> ToolResult {
        let id = self.recording(db, args)?;
        let err = |e: anyhow::Error| e.to_string();
        let rec = db.recording(&id).map_err(err)?;
        let analysis = db.analysis(&id).map_err(err)?;
        let notes = db.notes(&id).map_err(err)?;
        let docs = db.docs(&id).map_err(err)?;
        let mut out = if self.library() {
            format!("{} “{}”\n", self.alias(&id), rec.title)
        } else {
            format!("“{}”\n", rec.title)
        };
        let length = rec.duration_sec.map(fmt_time).unwrap_or_else(|| "?".into());
        let date = rec.created_at.get(..10).unwrap_or(&rec.created_at);
        if rec.segment_count == 0 {
            out.push_str(&format!("Added {date} · {length} · not transcribed yet.\n"));
        } else {
            out.push_str(&format!(
                "Added {date} · {length} · segments 0–{}\n",
                rec.segment_count - 1
            ));
        }
        match &analysis {
            Some(a) => {
                out.push_str(&format!("\nSummary:\n{}\n", a.summary.trim()));
                if !a.key_points.is_empty() {
                    out.push_str("\nKey points:\n");
                    for p in &a.key_points {
                        out.push_str(&format!("- {p}\n"));
                    }
                }
                if !a.chapters.is_empty() {
                    out.push_str("\nChapters:\n");
                    for c in &a.chapters {
                        let cite = self.cite(&id, c.start_segment, c.end_segment);
                        out.push_str(&format!(
                            "- {cite} ({}–{}) {}: {}\n",
                            fmt_time(c.start),
                            fmt_time(c.end),
                            c.title,
                            c.summary
                        ));
                    }
                }
                if !a.tags.is_empty() {
                    out.push_str("\nTopics:\n");
                    for t in &a.tags {
                        let segs: Vec<String> =
                            t.segments.iter().take(8).map(u32::to_string).collect();
                        let more = if t.segments.len() > 8 { "…" } else { "" };
                        out.push_str(&format!(
                            "- {} (segments {}{more})\n",
                            t.name,
                            segs.join(", ")
                        ));
                    }
                }
                if !a.action_items.is_empty() {
                    out.push_str("\nAction items:\n");
                    for i in &a.action_items {
                        out.push_str(&format!("- {i}\n"));
                    }
                }
            }
            None => out.push_str(
                "\nNo AI summary yet (the recording hasn't been analyzed). Search or read the transcript instead.\n",
            ),
        }
        out.push_str(&match notes.len() {
            0 => "\nThe user has no notes on it.\n".to_string(),
            n => format!("\nThe user has {n} notes on it (read them with get_notes).\n"),
        });
        if !docs.is_empty() {
            out.push_str("\nGenerated docs:\n");
            for d in &docs {
                let alias = self.doc_alias(&d.id);
                out.push_str(&format!(
                    "- {alias} “{}” (asked for: {})\n",
                    d.title,
                    clip(&d.prompt, 160)
                ));
            }
        }
        Ok(out)
    }

    /// The whole transcript, or segments `from..=to`, cut at [`WINDOW_CHARS`].
    fn transcript(&mut self, db: &Db, id: &str, range: Option<(u32, u32)>) -> ToolResult {
        let segs = db.segments(id).map_err(|e| e.to_string())?;
        let Some(last) = segs.last().map(|s| s.id) else {
            return Ok("This recording has no transcript yet.".into());
        };
        let (from, to) = range.unwrap_or((0, last));
        if from > last {
            return Err(format!(
                "segment {from} doesn't exist; the transcript has segments 0–{last}."
            ));
        }
        let to = to.min(last);
        let prefix = if self.library() {
            format!("{}#", self.alias(id))
        } else {
            String::new()
        };
        let mut body = String::new();
        let mut returned = None;
        let mut next = None;
        for s in segs.iter().filter(|s| s.id >= from && s.id <= to) {
            let line = format!(
                "[{prefix}{}] ({}) {}\n",
                s.id,
                fmt_time(s.start),
                s.text.trim()
            );
            if returned.is_some() && body.len() + line.len() > WINDOW_CHARS - 200 {
                next = Some(s.id);
                break;
            }
            if line.len() > WINDOW_CHARS - 200 {
                body.push_str(&clip(&line, WINDOW_CHARS - 200));
                body.push('\n');
            } else {
                body.push_str(&line);
            }
            returned = Some(s.id);
        }
        let end = returned.unwrap_or(from);
        let mut out = format!("Segments {from}–{end} of 0–{last}:\n{body}");
        if let Some(next) = next {
            out.push_str(&format!("[Stopped at segment {next} to keep this part small. Continue with read_transcript_section from {next}.]"));
        } else if end == last {
            out.push_str("(End of the transcript.)");
        }
        if self.docs {
            if let Some(end) = returned {
                self.read_ranges.push((from, end));
            }
            if !self.draft.is_empty() && !self.edited_since_read {
                out.push_str("\nReminder: nothing was added to the draft since the last part you read. Put what you found into the draft before reading on; earlier parts are removed from your context.");
            }
            self.edited_since_read = false;
            out.push_str(&format!("\n{}", self.coverage()));
        }
        Ok(out)
    }

    fn notes(&mut self, db: &Db, args: &Value) -> ToolResult {
        let id = self.recording(db, args)?;
        let notes = db.notes(&id).map_err(|e| e.to_string())?;
        if notes.is_empty() {
            return Ok("The user has no notes on this recording.".into());
        }
        let segs = db.segments(&id).map_err(|e| e.to_string())?;
        let mut out = format!("{} notes by the user, in order:\n", notes.len());
        for n in &notes {
            // segments the note's time range overlaps
            let first = segs.iter().find(|s| s.end > n.start + 0.05);
            let last = segs.iter().rev().find(|s| s.start < n.end - 0.05);
            let at = match (first, last) {
                (Some(a), Some(b)) => self.cite(&id, a.id, b.id.max(a.id)) + " ",
                _ => String::new(),
            };
            out.push_str(&format!(
                "\n- {at}({}–{}) {}\n",
                fmt_time(n.start),
                fmt_time(n.end),
                n.body.trim()
            ));
        }
        Ok(out)
    }

    fn read_doc(&mut self, db: &Db, args: &Value) -> ToolResult {
        let raw = arg_str(args, &["doc", "doc_id", "id"]).ok_or("`doc` is required")?;
        let r = raw.trim().trim_matches(['[', ']', '"', '\'', ' ']);
        let n = r.strip_prefix(['D', 'd']).unwrap_or(r).parse::<usize>();
        let id = match n.ok().and_then(|n| self.doc_aliases.get(n.wrapping_sub(1))) {
            Some(id) => id.clone(),
            None => r.to_string(),
        };
        let doc = db
            .doc(&id)
            .map_err(|_| format!("unknown doc “{raw}”. Doc ids like D2 come from get_overview."))?;
        if let Scope::Recording(rid) = &self.scope {
            if &doc.recording_id != rid {
                return Err(format!("doc “{raw}” belongs to another recording."));
            }
        }
        Ok(format!(
            "“{}” (asked for: {})\n\n{}",
            doc.title,
            doc.prompt,
            clip(&doc.html, DOC_CHARS)
        ))
    }
}

/// A function definition whose parameters are all required.
fn def(name: &str, description: &str, params: &[(&str, &str, &str)]) -> Value {
    let props: serde_json::Map<String, Value> = params
        .iter()
        .map(|(n, t, d)| (n.to_string(), json!({ "type": t, "description": d })))
        .collect();
    let required: Vec<&str> = params.iter().map(|p| p.0).collect();
    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": { "type": "object", "properties": props, "required": required },
        },
    })
}

fn parse_args(raw: &str) -> std::result::Result<Value, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(json!({}));
    }
    match serde_json::from_str::<Value>(raw) {
        Ok(v) if v.is_object() => Ok(v),
        // some models double-encode the arguments
        Ok(Value::String(s)) => parse_args(&s),
        _ => Err(format!(
            "the arguments were not a JSON object: {}",
            clip(raw, 200)
        )),
    }
}

fn draft_window(args: &Value) -> (usize, usize) {
    let from = arg_u32(args, &["from", "start", "offset"])
        .unwrap_or(1)
        .max(1) as usize;
    let limit = from.saturating_add(READ_LINES - 1);
    let to = arg_u32(args, &["to", "end"]).map_or(limit, |n| n as usize);
    (from, if to < from { limit } else { to.min(limit) })
}

// Draft replacements must preserve indentation and allow an empty replacement (deletion).
fn arg_text(args: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| match &args[*k] {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

fn arg_str(args: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| match &args[*k] {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

fn arg_u32(args: &Value, keys: &[&str]) -> Option<u32> {
    keys.iter().find_map(|k| {
        let v = &args[*k];
        v.as_u64()
            .map(|x| x as u32)
            .or_else(|| v.as_f64().map(|x| x.max(0.0) as u32))
            .or_else(|| {
                v.as_str()
                    .and_then(|s| s.trim().trim_matches(['[', ']']).parse().ok())
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{now_iso, Analysis, Chapter, Doc, Recording, RecordingStatus, Segment};

    fn rec(id: &str, title: &str) -> Recording {
        Recording {
            id: id.into(),
            title: title.into(),
            source_path: String::new(),
            audio_path: String::new(),
            playback_wav_path: None,
            duration_sec: Some(125.0),
            created_at: now_iso(),
            updated_at: now_iso(),
            status: RecordingStatus::Ready,
            model_id: None,
            error: None,
            segment_count: 0,
            has_analysis: false,
            tags: vec![],
            blurb: None,
        }
    }

    fn seg(i: u32, t: &str) -> Segment {
        Segment {
            id: i,
            start: i as f64 * 10.0,
            end: i as f64 * 10.0 + 9.0,
            text: t.into(),
            words: vec![],
            original_text: None,
        }
    }

    fn call(name: &str, args: Value) -> ToolCall {
        ToolCall {
            id: "c".into(),
            name: name.into(),
            arguments: args.to_string(),
        }
    }

    fn library() -> Db {
        let mut db = Db::open_in_memory().unwrap();
        db.insert_recording(&rec("uuid-a", "جلسه بودجه")).unwrap();
        db.insert_recording(&rec("uuid-b", "پادکست")).unwrap();
        db.replace_segments(
            "uuid-a",
            "m",
            &[
                seg(0, "سلام به جلسه خوش آمدید"),
                seg(1, "بودجه سال آینده کم است"),
                seg(2, "باید هزینه‌ها را کم کنیم"),
            ],
        )
        .unwrap();
        db.replace_segments("uuid-b", "m", &[seg(0, "امروز درباره موسیقی حرف می‌زنیم")])
            .unwrap();
        db.save_analysis(
            "uuid-a",
            &Analysis {
                summary: "درباره بودجه".into(),
                chapters: vec![Chapter {
                    title: "بودجه".into(),
                    start_segment: 0,
                    end_segment: 2,
                    end: 29.0,
                    ..Default::default()
                }],
                ..Default::default()
            },
        )
        .unwrap();
        db.add_note("uuid-a", 10.0, 25.0, "مهم: کاهش هزینه", None)
            .unwrap();
        db.save_doc(&Doc {
            id: "doc-1".into(),
            recording_id: "uuid-a".into(),
            title: "Quiz".into(),
            prompt: "a quiz".into(),
            html: "<html>quiz</html>".into(),
            model: "m".into(),
            created_at: now_iso(),
            updated_at: now_iso(),
        })
        .unwrap();
        db
    }

    #[test]
    fn library_tools_use_aliases() {
        let db = library();
        let mut t = ToolBox::new(Scope::Library, false);
        let list = t.run(&db, &call("list_recordings", json!({})), vec![]);
        // newest first; both were inserted in the same second, so just check both appear
        assert!(list.contains("R1 “") && list.contains("R2 “"), "{list}");
        let a = t.alias("uuid-a");
        let found = t.run(&db, &call("search", json!({ "query": "بودجه" })), vec![]);
        assert!(found.contains(&format!("[{a}#1]")), "{found}");
        let section = t.run(
            &db,
            &call(
                "read_transcript_section",
                json!({ "recording": a, "from": "1", "to": 5 }),
            ),
            vec![],
        );
        assert!(
            section.contains(&format!("[{a}#1] (0:10) بودجه")),
            "{section}"
        );
        assert!(section.contains(&format!("[{a}#2]")), "{section}");
        assert!(!section.contains(&format!("[{a}#0]")), "{section}");
        let notes = t.run(
            &db,
            &call("get_notes", json!({ "recording": format!("[{a}]") })),
            vec![],
        );
        assert!(
            notes.contains(&format!("[{a}#1-2] (0:10–0:25) مهم")),
            "{notes}"
        );
        let bad = t.run(
            &db,
            &call("get_notes", json!({ "recording": "R9" })),
            vec![],
        );
        assert!(bad.starts_with("Error: unknown recording"), "{bad}");
    }

    #[test]
    fn recording_tools_need_no_recording_argument() {
        let db = library();
        let mut t = ToolBox::new(Scope::Recording("uuid-a".into()), true);
        let names: Vec<String> = t
            .definitions()
            .iter()
            .map(|d| d["function"]["name"].as_str().unwrap().to_string())
            .collect();
        assert!(!names.contains(&"list_recordings".to_string()));
        assert!(names.contains(&"load_skill".to_string()));
        assert!(t
            .definitions()
            .iter()
            .all(|d| d["function"]["parameters"]["properties"]
                .get("recording")
                .is_none()));
        let overview = t.run(&db, &call("get_overview", json!({})), vec![]);
        assert!(overview.contains("- [0-2] (0:00–0:29) بودجه"), "{overview}");
        assert!(overview.contains("D1 “Quiz”"), "{overview}");
        let doc = t.run(&db, &call("read_doc", json!({ "doc": "D1" })), vec![]);
        assert!(doc.contains("<html>quiz</html>"), "{doc}");
        let full = t.run(&db, &call("read_transcript", json!({})), vec![]);
        assert!(
            full.starts_with("Segments 0–2 of 0–2:\n[0] (0:00) سلام"),
            "{full}"
        );
        let skill = t.run(&db, &call("load_skill", json!({ "name": "quiz" })), vec![]);
        assert!(skill.starts_with("# Quiz"), "{skill}");
        let unknown = t.run(&db, &call("list_recordings", json!({})), vec![]);
        assert!(unknown.starts_with("Error: unknown tool"), "{unknown}");
        let broken = t.run(
            &db,
            &ToolCall {
                id: "c".into(),
                name: "search".into(),
                arguments: "{query:".into(),
            },
            vec![],
        );
        assert!(
            broken.starts_with("Error: the arguments were not"),
            "{broken}"
        );
    }

    #[test]
    fn lists_large_libraries_in_pages() {
        let db = Db::open_in_memory().unwrap();
        for i in 0..105 {
            let mut r = rec(&format!("id-{i:03}"), &format!("rec {i}"));
            r.created_at = format!("2026-01-01T00:{:02}:{:02}Z", i / 60, i % 60);
            db.insert_recording(&r).unwrap();
        }
        let mut t = ToolBox::new(Scope::Library, false);
        let first = t.run(&db, &call("list_recordings", json!({ "page": 1 })), vec![]);
        assert!(
            first.starts_with("105 recordings, newest first. Page 1 of 2:"),
            "{first}"
        );
        assert!(
            first.contains("R1 “rec 104”") && first.ends_with("Older recordings are on page 2.")
        );
        let second = t.run(
            &db,
            &call("list_recordings", json!({ "page": "2" })),
            vec![],
        );
        assert!(
            second.contains("R101 “rec 4”") && second.contains("R105 “rec 0”"),
            "{second}"
        );
        assert!(!second.contains("Older recordings"));
        let past = t.run(&db, &call("list_recordings", json!({ "page": 3 })), vec![]);
        assert!(past.starts_with("Error: there are only 2 pages"), "{past}");
    }

    #[test]
    fn long_transcripts_are_paged() {
        let mut db = Db::open_in_memory().unwrap();
        db.insert_recording(&rec("x", "long")).unwrap();
        let line = "کلمه ".repeat(400);
        let segs: Vec<Segment> = (0..200).map(|i| seg(i, &line)).collect();
        db.replace_segments("x", "m", &segs).unwrap();
        let mut t = ToolBox::new(Scope::Recording("x".into()), false);
        let out = t.run(&db, &call("read_transcript", json!({})), vec![]);
        assert!(out.len() <= WINDOW_CHARS + 200);
        let end = out
            .split_once('–')
            .unwrap()
            .1
            .split(' ')
            .next()
            .unwrap()
            .parse::<u32>()
            .unwrap();
        assert!(out.starts_with(&format!("Segments 0–{end} of 0–199:")));
        assert!(out.contains(&format!(
            "Continue with read_transcript_section from {}.",
            end + 1
        )));
        assert!(
            out.contains("Continue with read_transcript_section from"),
            "{}",
            &out[out.len() - 200..]
        );
    }

    #[test]
    fn a_segment_bigger_than_a_window_is_still_readable() {
        let mut db = library();
        db.replace_segments(
            "uuid-a",
            "m",
            &[seg(0, &"ش".repeat(40_000)), seg(1, "next")],
        )
        .unwrap();
        let mut t = ToolBox::new(Scope::Recording("uuid-a".into()), false);
        let out = t.run(&db, &call("read_transcript", json!({})), vec![]);
        assert!(out.len() <= WINDOW_CHARS + 200);
        assert!(out.starts_with("Segments 0–0 of 0–1:"));
        assert!(out.contains("…\n"));
        assert!(!out.contains("……"));
        assert!(out.contains("Continue with read_transcript_section from 1."));
        db.replace_segments("uuid-a", "m", &[seg(0, &"ش".repeat(40_000))])
            .unwrap();
        let out = t.run(&db, &call("read_transcript", json!({})), vec![]);
        assert!(out.ends_with("(End of the transcript.)"));
    }

    #[test]
    fn section_without_to_reads_to_the_end() {
        let db = library();
        let mut t = ToolBox::new(Scope::Recording("uuid-a".into()), false);
        let out = t.run(
            &db,
            &call("read_transcript_section", json!({"from": 1})),
            vec![],
        );
        assert!(out.starts_with("Segments 1–2 of 0–2:"));
        assert!(out.ends_with("(End of the transcript.)"));
        let out = t.run(
            &db,
            &call("read_transcript_section", json!({"from": 0, "to": 1})),
            vec![],
        );
        assert!(out.starts_with("Segments 0–1 of 0–2:"));
        assert!(!out.contains("End of the transcript"));
    }

    const SKELETON: &str = "<!doctype html>\n<html>\n<script>const ITEMS = [\n  // end: items\n];\nconst segments = WWTA.segments;</script>\n</html>";

    #[test]
    fn draft_tools_build_a_doc() {
        let db = library();
        let mut t = ToolBox::new(Scope::Recording("uuid-a".into()), true);
        let out = t.run(&db, &call("write_draft", json!({"html": SKELETON})), vec![]);
        assert!(out.starts_with("Draft saved:"));
        assert!(out.ends_with("Transcript read so far: nothing of 0–2."));
        let out = t.run(&db, &call("read_transcript", json!({})), vec![]);
        assert!(out.ends_with("Transcript read so far: all of 0–2."));
        let out = t.run(
            &db,
            &call(
                "edit_draft",
                json!({"old": "  // end: items", "new": "  {seg: 1},\n  // end: items"}),
            ),
            vec![],
        );
        assert!(out.starts_with("Edited the draft:"));
        assert!(t.draft().html().contains("  {seg: 1},\n  // end: items"));
        let out = t.run(
            &db,
            &call("read_draft", json!({"from": 1, "to": 50})),
            vec![],
        );
        assert!(out.contains("1: <!doctype html>"));
        assert!(!out.contains("Transcript read so far"));
        let out = t.run(&db, &call("finish_doc", json!({})), vec![]);
        assert!(out.starts_with("Doc finished:"), "{out}");
        assert!(t.finished());
        let stats = t.doc_stats();
        assert_eq!(
            (
                stats.writes,
                stats.edits,
                stats.reads,
                stats.finish_attempts
            ),
            (1, 1, 1, 1)
        );
        assert_eq!(stats.transcript_read_pct, 100);
        assert!(stats.finished);
        assert_eq!(t.doc_result("<html>other</html>").unwrap().source, "draft");
    }

    #[test]
    fn draft_tools_are_not_offered_in_chat() {
        let db = library();
        for scope in [Scope::Recording("uuid-a".into()), Scope::Library] {
            let mut t = ToolBox::new(scope, false);
            for name in ["write_draft", "read_draft", "edit_draft", "finish_doc"] {
                assert!(!t
                    .definitions()
                    .iter()
                    .any(|d| d["function"]["name"] == name));
                assert!(t
                    .run(&db, &call(name, json!({})), vec![])
                    .starts_with("Error: unknown tool"));
            }
            assert!(t.follow_up("ok").is_none());
        }
    }

    #[test]
    fn reminds_to_write_before_reading_on() {
        let db = library();
        let mut t = ToolBox::new(Scope::Recording("uuid-a".into()), true);
        t.run(&db, &call("write_draft", json!({"html": SKELETON})), vec![]);
        let out = t.run(
            &db,
            &call("read_transcript_section", json!({"from": 0, "to": 0})),
            vec![],
        );
        assert!(!out.contains("Reminder:"));
        assert!(out.ends_with("Transcript read so far: 0 (33% of 0–2). Unread: 1–2."));
        let out = t.run(
            &db,
            &call("read_transcript_section", json!({"from": 1, "to": 1})),
            vec![],
        );
        assert!(out.contains("Reminder: nothing was added"));
        assert!(out.ends_with("Transcript read so far: 0–1 (66% of 0–2). Unread: 2."));
        t.run(
            &db,
            &call(
                "edit_draft",
                json!({"old": "  // end: items", "new": "  1,\n  // end: items"}),
            ),
            vec![],
        );
        let out = t.run(
            &db,
            &call("read_transcript_section", json!({"from": 2, "to": 2})),
            vec![],
        );
        assert!(!out.contains("Reminder:"));
        assert_eq!(t.doc_stats().transcript_read_pct, 100);
    }

    #[test]
    fn coverage_merges_overlaps_and_limits_unread_ranges() {
        let mut t = ToolBox::new(Scope::Recording("uuid-a".into()), true);
        t.transcript_last = Some(20);
        t.read_ranges = vec![(4, 5), (2, 4), (9, 9), (7, 7), (11, 11)];
        assert_eq!(t.merged_ranges(), [(2, 5), (7, 7), (9, 9), (11, 11)]);
        assert_eq!(
            t.coverage(),
            "Transcript read so far: 2–5, 7, 9, 11 (33% of 0–20). Unread: 0–1, 6, 8, …."
        );
    }

    #[test]
    fn follow_up_and_doc_result() {
        let mut t = ToolBox::new(Scope::Recording("uuid-a".into()), true);
        assert!(t
            .doc_result("ok")
            .err()
            .unwrap()
            .to_string()
            .contains("didn't write a doc"));
        assert!(t
            .follow_up("ok")
            .unwrap()
            .starts_with("You haven't written"));
        assert!(t.follow_up("ok").is_some());
        assert!(t.follow_up("ok").is_none());
        let mut t = ToolBox::new(Scope::Recording("uuid-a".into()), true);
        let answer = format!("```html\n{SKELETON}\n```");
        assert!(t.follow_up(&answer).is_none());
        assert_eq!(t.doc_result(&answer).unwrap().source, "answer");
        assert_eq!(t.nudges, 0);
        t.draft.write("<!doctype html><html>").unwrap();
        assert!(t
            .doc_result("done")
            .err()
            .unwrap()
            .to_string()
            .contains("incomplete"));
        assert!(t
            .follow_up("done")
            .unwrap()
            .contains("It has these problems:"));
        assert_eq!(t.doc_result(&answer).unwrap().source, "answer");
        t.set_draft(SKELETON);
        assert!(t
            .doc_result("done")
            .err()
            .unwrap()
            .to_string()
            .contains("made no changes"));
        assert!(t.follow_up("done").unwrap().contains("haven't changed"));
    }

    #[test]
    fn labels_for_draft_tools() {
        let db = library();
        let mut t = ToolBox::new(Scope::Recording("uuid-a".into()), true);
        assert_eq!(
            t.label(&db, &call("write_draft", json!({}))),
            "Writing the draft"
        );
        assert_eq!(
            t.label(&db, &call("edit_draft", json!({}))),
            "Editing the draft"
        );
        t.set_draft(&format!(
            "{SKELETON}{}",
            " ".repeat(18 * 1024 - SKELETON.len())
        ));
        assert_eq!(
            t.label(&db, &call("edit_draft", json!({}))),
            "Adding to the draft (18 KB so far)"
        );
        assert_eq!(
            t.label(&db, &call("read_draft", json!({"from": 1, "to": 500}))),
            "Reading the draft (lines 1–200)"
        );
        assert_eq!(
            t.label(&db, &call("read_draft", json!({"start": 3}))),
            "Reading the draft (lines 3–202)"
        );
        assert_eq!(
            t.label(&db, &call("finish_doc", json!({}))),
            "Checking the doc"
        );
        assert_eq!(
            t.label(&db, &call("read_transcript_section", json!({"from": 212}))),
            "Reading segments from 212"
        );
        assert_eq!(
            t.label(
                &db,
                &call("read_transcript_section", json!({"start": 212, "end": 430}))
            ),
            "Reading segments 212–430"
        );
    }

    #[test]
    fn draft_arguments_preserve_spaces_allow_deletion_and_accept_aliases() {
        let db = library();
        let mut t = ToolBox::new(Scope::Recording("uuid-a".into()), true);
        assert!(t
            .run(&db, &call("write_draft", json!({"file": SKELETON})), vec![])
            .starts_with("Draft saved:"));
        assert!(t
            .run(
                &db,
                &call(
                    "edit_draft",
                    json!({"oldString": "  // end: items", "replacement": ""})
                ),
                vec![]
            )
            .starts_with("Edited the draft:"));
        assert!(!t.draft().html().contains("// end:"));
        assert!(t
            .run(
                &db,
                &call("read_draft", json!({"offset": 2, "end": 2})),
                vec![]
            )
            .contains("2: <html>"));
        assert_eq!(
            t.run(&db, &call("edit_draft", json!({"old": "x"})), vec![]),
            "Error: `new` is required"
        );
    }

    #[test]
    fn aliasify_and_labels() {
        let db = library();
        let mut t = ToolBox::new(Scope::Library, false);
        assert_eq!(
            t.aliasify("see [rec:uuid-b#3, rec:uuid-a#1-2]"),
            "see [R1#3, R2#1-2]"
        );
        assert_eq!(t.recording_ids(), ["uuid-b", "uuid-a"]);
        let label = t.label(&db, &call("read_transcript", json!({ "recording": "R2" })));
        assert_eq!(label, "Reading the transcript of “جلسه بودجه”");
        assert_eq!(
            ToolBox::search_query(&call("search", json!({ "query": "x" }))).as_deref(),
            Some("x")
        );
    }
}
