//! The tools the agent can call. Kept few and simple for small models: every argument is
//! required, results are plain text, recordings and docs get short ids (R3, D2) instead of
//! UUIDs, and arguments are parsed leniently ("R3", "3" and "[R3]" all work).

use regex::Regex;
use serde_json::{json, Value};

use super::{clip, skills};
use crate::db::Db;
use crate::llm::ToolCall;
use crate::search;
use crate::text::fmt_time;
use crate::types::{SearchHit, Segment};

/// Transcript text per tool result; longer transcripts are paged.
const TRANSCRIPT_CHARS: usize = 120_000;
const DOC_CHARS: usize = 80_000;
const LIST_LIMIT: usize = 150;
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
    /// Offer `load_skill` (doc generation).
    skills: bool,
    /// Recording UUIDs by alias: R1 is `recordings[0]`. Assigned on first sight.
    recordings: Vec<String>,
    /// Doc UUIDs by alias: D1 is `docs[0]`.
    docs: Vec<String>,
}

/// A tool failed in a way the model can fix (bad id, missing argument…).
type ToolResult = std::result::Result<String, String>;

impl ToolBox {
    pub fn new(scope: Scope, skills: bool) -> Self {
        Self {
            scope,
            skills,
            recordings: vec![],
            docs: vec![],
        }
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
        let n = match self.docs.iter().position(|d| d == id) {
            Some(i) => i + 1,
            None => {
                self.docs.push(id.to_string());
                self.docs.len()
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
                "List all recordings in the library with their ids (like R3), dates, lengths and short descriptions.",
                &[],
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
        if self.skills {
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
            "list_recordings" => "Browsing the library".into(),
            "search" => format!(
                "Searching for “{}”",
                arg_str(&args, &["query", "q", "text"]).unwrap_or_default()
            ),
            "get_overview" => format!("Reading the overview{of}"),
            "read_transcript" => format!("Reading the transcript{of}"),
            "read_transcript_section" => match (arg_u32(&args, &["from"]), arg_u32(&args, &["to"]))
            {
                (Some(a), Some(b)) => format!("Reading segments {a}–{b}{of}"),
                _ => format!("Reading the transcript{of}"),
            },
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
            "list_recordings" => self.list_recordings(db),
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
                let to = arg_u32(args, &["to", "end", "to_segment"]).unwrap_or(from + 40);
                self.transcript(db, &id, Some((from.min(to), from.max(to))))
            }
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

    fn list_recordings(&mut self, db: &Db) -> ToolResult {
        let recs = db.list_recordings().map_err(|e| e.to_string())?;
        if recs.is_empty() {
            return Ok("The library is empty.".into());
        }
        let mut out = format!("{} recordings, newest first:\n", recs.len());
        for r in recs.iter().take(LIST_LIMIT) {
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
        if recs.len() > LIST_LIMIT {
            out.push_str(&format!(
                "\n\n…and {} older recordings. Use search to find them.",
                recs.len() - LIST_LIMIT
            ));
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

    /// The whole transcript, or segments `from..=to`, cut at [`TRANSCRIPT_CHARS`].
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
        let picked: Vec<&Segment> = segs.iter().filter(|s| s.id >= from && s.id <= to).collect();
        let mut out = format!("Segments {from}–{to} of 0–{last}:\n");
        for s in picked {
            let line = format!(
                "[{prefix}{}] ({}) {}\n",
                s.id,
                fmt_time(s.start),
                s.text.trim()
            );
            if out.len() + line.len() > TRANSCRIPT_CHARS {
                out.push_str(&format!(
                    "[Stopped at segment {} to save space. Continue with read_transcript_section from {}.]\n",
                    s.id, s.id
                ));
                break;
            }
            out.push_str(&line);
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
        let id = match n.ok().and_then(|n| self.docs.get(n.wrapping_sub(1))) {
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
    use crate::types::{now_iso, Analysis, Chapter, Doc, Recording, RecordingStatus};

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
    fn long_transcripts_are_paged() {
        let mut db = Db::open_in_memory().unwrap();
        db.insert_recording(&rec("x", "long")).unwrap();
        let line = "کلمه ".repeat(400);
        let segs: Vec<Segment> = (0..200).map(|i| seg(i, &line)).collect();
        db.replace_segments("x", "m", &segs).unwrap();
        let mut t = ToolBox::new(Scope::Recording("x".into()), false);
        let out = t.run(&db, &call("read_transcript", json!({})), vec![]);
        assert!(out.len() <= TRANSCRIPT_CHARS + 200);
        assert!(
            out.contains("Continue with read_transcript_section from"),
            "{}",
            &out[out.len() - 200..]
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
