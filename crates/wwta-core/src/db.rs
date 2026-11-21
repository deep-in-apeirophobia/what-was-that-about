//! SQLite-backed library: recordings, transcripts, AI analysis, chats, docs and embeddings.

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::text::normalize_for_search;
use crate::types::{
    now_iso, Analysis, ChatMessage, Doc, Note, NoteWithRecording, Recording, RecordingStatus,
    Segment, Word, NOTE_COLORS,
};

/// (recording_id, title, tag, segment ids), as returned by [`Db::all_tags`].
pub type TagRow = (String, String, String, Vec<u32>);

pub struct Db {
    pub conn: Connection,
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS recordings (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    source_path TEXT NOT NULL,
    audio_path TEXT NOT NULL,
    playback_wav_path TEXT,
    duration_sec REAL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    status TEXT NOT NULL,
    model_id TEXT,
    error TEXT,
    tags_json TEXT NOT NULL DEFAULT '[]',
    blurb TEXT,
    title_custom INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS segments (
    recording_id TEXT NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
    idx INTEGER NOT NULL,
    start REAL NOT NULL,
    end REAL NOT NULL,
    text TEXT NOT NULL,
    norm TEXT NOT NULL,
    words TEXT NOT NULL DEFAULT '[]',
    original_text TEXT,
    PRIMARY KEY (recording_id, idx)
);
CREATE TABLE IF NOT EXISTS analyses (
    recording_id TEXT PRIMARY KEY REFERENCES recordings(id) ON DELETE CASCADE,
    json TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS analysis_tags (
    recording_id TEXT NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
    tag TEXT NOT NULL,
    norm TEXT NOT NULL,
    segments TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS analysis_tags_norm ON analysis_tags(norm);
CREATE TABLE IF NOT EXISTS chat_messages (
    id TEXT PRIMARY KEY,
    recording_id TEXT REFERENCES recordings(id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    created_at TEXT NOT NULL,
    model TEXT
);
CREATE TABLE IF NOT EXISTS docs (
    id TEXT PRIMARY KEY,
    recording_id TEXT NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    prompt TEXT NOT NULL,
    html TEXT NOT NULL,
    model TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS notes (
    id TEXT PRIMARY KEY,
    recording_id TEXT NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
    start REAL NOT NULL,
    end REAL NOT NULL,
    body TEXT NOT NULL,
    norm TEXT NOT NULL,
    color TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS notes_recording ON notes(recording_id, start);
CREATE TABLE IF NOT EXISTS embeddings (
    recording_id TEXT NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
    chunk INTEGER NOT NULL,
    start_seg INTEGER NOT NULL,
    end_seg INTEGER NOT NULL,
    model TEXT NOT NULL,
    vector BLOB NOT NULL,
    PRIMARY KEY (recording_id, chunk)
);
"#;

#[derive(Debug, Clone)]
pub struct EmbeddingRow {
    pub recording_id: String,
    pub chunk: u32,
    pub start_seg: u32,
    pub end_seg: u32,
    pub vector: Vec<f32>,
}

fn recording_from_row(r: &Row) -> rusqlite::Result<Recording> {
    let tags_json: String = r.get("tags_json")?;
    Ok(Recording {
        id: r.get("id")?,
        title: r.get("title")?,
        source_path: r.get("source_path")?,
        audio_path: r.get("audio_path")?,
        playback_wav_path: r.get("playback_wav_path")?,
        duration_sec: r.get("duration_sec")?,
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
        status: RecordingStatus::parse(&r.get::<_, String>("status")?),
        model_id: r.get("model_id")?,
        error: r.get("error")?,
        segment_count: r.get::<_, i64>("segment_count")? as u32,
        has_analysis: r.get::<_, i64>("has_analysis")? != 0,
        tags: serde_json::from_str(&tags_json).unwrap_or_default(),
        blurb: r.get("blurb")?,
    })
}

const RECORDING_SELECT: &str = "SELECT r.*, \
    (SELECT COUNT(*) FROM segments s WHERE s.recording_id = r.id) AS segment_count, \
    EXISTS(SELECT 1 FROM analyses a WHERE a.recording_id = r.id) AS has_analysis \
    FROM recordings r";

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(SCHEMA)?;
        // migrations for libraries created by earlier builds
        let has_title_custom: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('recordings') WHERE name = 'title_custom'")?
            .exists([])?;
        if !has_title_custom {
            conn.execute_batch(
                "ALTER TABLE recordings ADD COLUMN title_custom INTEGER NOT NULL DEFAULT 0",
            )?;
        }
        let has_original_text: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('segments') WHERE name = 'original_text'")?
            .exists([])?;
        if !has_original_text {
            conn.execute_batch("ALTER TABLE segments ADD COLUMN original_text TEXT")?;
        }
        Ok(Self { conn })
    }

    /// After a crash/quit mid-job, put queued/transcribing recordings back to a resting state.
    pub fn reset_interrupted(&self) -> Result<()> {
        self.conn.execute(
            "UPDATE recordings SET status = CASE WHEN EXISTS(SELECT 1 FROM segments s WHERE s.recording_id = recordings.id) \
             THEN 'ready' ELSE 'new' END WHERE status IN ('queued', 'transcribing')",
            [],
        )?;
        Ok(())
    }

    // ---- recordings

    pub fn insert_recording(&self, r: &Recording) -> Result<()> {
        self.conn.execute(
            "INSERT INTO recordings (id, title, source_path, audio_path, playback_wav_path, duration_sec, \
             created_at, updated_at, status, model_id, error) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                r.id,
                r.title,
                r.source_path,
                r.audio_path,
                r.playback_wav_path,
                r.duration_sec,
                r.created_at,
                r.updated_at,
                r.status.as_str(),
                r.model_id,
                r.error
            ],
        )?;
        Ok(())
    }

    pub fn list_recordings(&self) -> Result<Vec<Recording>> {
        let mut st = self
            .conn
            .prepare(&format!("{RECORDING_SELECT} ORDER BY r.created_at DESC"))?;
        let rows = st.query_map([], recording_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn get_recording(&self, id: &str) -> Result<Option<Recording>> {
        Ok(self
            .conn
            .query_row(
                &format!("{RECORDING_SELECT} WHERE r.id = ?1"),
                [id],
                recording_from_row,
            )
            .optional()?)
    }

    pub fn recording(&self, id: &str) -> Result<Recording> {
        self.get_recording(id)?
            .ok_or_else(|| anyhow::anyhow!("recording {id} not found"))
    }

    pub fn set_status(&self, id: &str, status: RecordingStatus, error: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE recordings SET status = ?2, error = ?3, updated_at = ?4 WHERE id = ?1",
            params![id, status.as_str(), error, now_iso()],
        )?;
        Ok(())
    }

    /// Title chosen by the user (AI analysis won't replace it).
    pub fn set_title(&self, id: &str, title: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE recordings SET title = ?2, title_custom = 1, updated_at = ?3 WHERE id = ?1",
            params![id, title, now_iso()],
        )?;
        Ok(())
    }

    /// Replace the title with an AI suggestion unless the user named the recording.
    pub fn set_suggested_title(&self, id: &str, title: &str) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE recordings SET title = ?2, updated_at = ?3 WHERE id = ?1 AND title_custom = 0",
            params![id, title, now_iso()],
        )?;
        Ok(n > 0)
    }

    pub fn set_duration(&self, id: &str, secs: f64) -> Result<()> {
        self.conn.execute(
            "UPDATE recordings SET duration_sec = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, secs, now_iso()],
        )?;
        Ok(())
    }

    pub fn set_playback_wav(&self, id: &str, path: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE recordings SET playback_wav_path = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, path, now_iso()],
        )?;
        Ok(())
    }

    pub fn delete_recording(&self, id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM recordings WHERE id = ?1", [id])?;
        Ok(())
    }

    // ---- segments

    pub fn replace_segments(&mut self, id: &str, model_id: &str, segs: &[Segment]) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM segments WHERE recording_id = ?1", [id])?;
        tx.execute("DELETE FROM embeddings WHERE recording_id = ?1", [id])?;
        // A new transcript renumbers segments, so the old analysis (chapters/tags point at
        // segment ids) no longer fits; docs and chat history are kept.
        tx.execute("DELETE FROM analyses WHERE recording_id = ?1", [id])?;
        tx.execute("DELETE FROM analysis_tags WHERE recording_id = ?1", [id])?;
        tx.execute(
            "UPDATE recordings SET tags_json = '[]', blurb = NULL WHERE id = ?1",
            [id],
        )?;
        {
            let mut st = tx.prepare(
                "INSERT INTO segments (recording_id, idx, start, end, text, norm, words) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            )?;
            for s in segs {
                st.execute(params![
                    id,
                    s.id,
                    s.start,
                    s.end,
                    s.text,
                    normalize_for_search(&s.text),
                    serde_json::to_string(&s.words)?
                ])?;
            }
        }
        tx.execute(
            "UPDATE recordings SET model_id = ?2, status = 'ready', error = NULL, updated_at = ?3 WHERE id = ?1",
            params![id, model_id, now_iso()],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn segments(&self, id: &str) -> Result<Vec<Segment>> {
        let mut st = self.conn.prepare(
            "SELECT idx, start, end, text, words, original_text FROM segments WHERE recording_id = ?1 ORDER BY idx",
        )?;
        let rows = st.query_map([id], |r| {
            let words: String = r.get(4)?;
            Ok(Segment {
                id: r.get::<_, i64>(0)? as u32,
                start: r.get(1)?,
                end: r.get(2)?,
                text: r.get(3)?,
                words: serde_json::from_str::<Vec<Word>>(&words).unwrap_or_default(),
                original_text: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    fn segments_with_ids(&self, id: &str, ids: &[u32]) -> Result<Vec<Segment>> {
        Ok(self
            .segments(id)?
            .into_iter()
            .filter(|s| ids.contains(&s.id))
            .collect())
    }

    pub fn update_segment_text(&self, id: &str, seg: u32, text: &str) -> Result<Segment> {
        // Edited text invalidates word timings; keep the segment's overall span. The AI-fix
        // original is kept, unless the edit restores it.
        self.conn.execute(
            "UPDATE segments SET text = ?3, norm = ?4, words = '[]', \
             original_text = NULLIF(original_text, ?3) WHERE recording_id = ?1 AND idx = ?2",
            params![id, seg, text, normalize_for_search(text)],
        )?;
        self.conn.execute(
            "DELETE FROM embeddings WHERE recording_id = ?1 AND start_seg <= ?2 AND end_seg >= ?2",
            params![id, seg],
        )?;
        self.segments(id)?
            .into_iter()
            .find(|s| s.id == seg)
            .ok_or_else(|| anyhow::anyhow!("segment not found"))
    }

    /// Apply AI corrections as `(segment, text it was corrected from, corrected text)`. A
    /// segment whose text changed in the meantime (edited by the user) is skipped. The
    /// recognizer's text is kept in `original_text`, and word timings stay so the karaoke
    /// highlight still finds the unchanged words. Returns the segments that changed.
    pub fn apply_fixes(
        &mut self,
        id: &str,
        fixes: &[(u32, String, String)],
    ) -> Result<Vec<Segment>> {
        let tx = self.conn.transaction()?;
        let mut changed = Vec::new();
        for (seg, old, new) in fixes {
            if old == new {
                continue;
            }
            let n = tx.execute(
                "UPDATE segments SET original_text = NULLIF(COALESCE(original_text, text), ?4), \
                 text = ?4, norm = ?5 WHERE recording_id = ?1 AND idx = ?2 AND text = ?3",
                params![id, seg, old, new, normalize_for_search(new)],
            )?;
            if n > 0 {
                tx.execute(
                    "DELETE FROM embeddings WHERE recording_id = ?1 AND start_seg <= ?2 AND end_seg >= ?2",
                    params![id, seg],
                )?;
                changed.push(*seg);
            }
        }
        tx.commit()?;
        if changed.is_empty() {
            return Ok(vec![]);
        }
        self.segments_with_ids(id, &changed)
    }

    /// Put back the recognizer's text for AI-fixed segments (`only` = these ids, else all).
    /// Returns the segments that changed.
    pub fn revert_fixes(&mut self, id: &str, only: Option<&[u32]>) -> Result<Vec<Segment>> {
        let fixed: Vec<(u32, String)> = {
            let mut st = self.conn.prepare(
                "SELECT idx, original_text FROM segments WHERE recording_id = ?1 AND original_text IS NOT NULL",
            )?;
            let rows = st.query_map([id], |r| Ok((r.get::<_, i64>(0)? as u32, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
                .into_iter()
                .filter(|(seg, _)| only.is_none_or(|ids| ids.contains(seg)))
                .collect()
        };
        if fixed.is_empty() {
            return Ok(vec![]);
        }
        let tx = self.conn.transaction()?;
        for (seg, original) in &fixed {
            tx.execute(
                "UPDATE segments SET text = ?3, norm = ?4, original_text = NULL WHERE recording_id = ?1 AND idx = ?2",
                params![id, seg, original, normalize_for_search(original)],
            )?;
            tx.execute(
                "DELETE FROM embeddings WHERE recording_id = ?1 AND start_seg <= ?2 AND end_seg >= ?2",
                params![id, seg],
            )?;
        }
        tx.commit()?;
        let ids: Vec<u32> = fixed.iter().map(|(seg, _)| *seg).collect();
        self.segments_with_ids(id, &ids)
    }

    // ---- analysis

    pub fn save_analysis(&mut self, id: &str, a: &Analysis) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT OR REPLACE INTO analyses (recording_id, json, created_at) VALUES (?1, ?2, ?3)",
            params![id, serde_json::to_string(a)?, a.created_at],
        )?;
        tx.execute("DELETE FROM analysis_tags WHERE recording_id = ?1", [id])?;
        for t in &a.tags {
            tx.execute(
                "INSERT INTO analysis_tags (recording_id, tag, norm, segments) VALUES (?1,?2,?3,?4)",
                params![id, t.name, normalize_for_search(&t.name), serde_json::to_string(&t.segments)?],
            )?;
        }
        let mut tags: Vec<&crate::types::Tag> = a.tags.iter().collect();
        tags.sort_by_key(|t| std::cmp::Reverse(t.segments.len()));
        let top: Vec<&str> = tags.iter().take(6).map(|t| t.name.as_str()).collect();
        let blurb = a
            .summary
            .split("\n\n")
            .map(|p| p.trim().trim_start_matches(['#', '-', '*', ' ']))
            .find(|p| !p.is_empty())
            .map(|p| {
                let mut s: String = p.chars().take(240).collect();
                if p.chars().count() > 240 {
                    s.push('…');
                }
                s
            });
        tx.execute(
            "UPDATE recordings SET tags_json = ?2, blurb = ?3, updated_at = ?4 WHERE id = ?1",
            params![id, serde_json::to_string(&top)?, blurb, now_iso()],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn analysis(&self, id: &str) -> Result<Option<Analysis>> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT json FROM analyses WHERE recording_id = ?1",
                [id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(json.and_then(|j| serde_json::from_str(&j).ok()))
    }

    /// (recording_id, title, tag, segment ids) for every tag in the library.
    pub fn all_tags(&self) -> Result<Vec<TagRow>> {
        let mut st = self.conn.prepare(
            "SELECT t.recording_id, r.title, t.tag, t.segments FROM analysis_tags t \
             JOIN recordings r ON r.id = t.recording_id ORDER BY r.created_at DESC",
        )?;
        let rows = st.query_map([], |r| {
            let segs: String = r.get(3)?;
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                serde_json::from_str(&segs).unwrap_or_default(),
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // ---- chat

    pub fn add_chat(&self, m: &ChatMessage) -> Result<()> {
        self.conn.execute(
            "INSERT INTO chat_messages (id, recording_id, role, content, created_at, model) VALUES (?1,?2,?3,?4,?5,?6)",
            params![m.id, m.recording_id, m.role, m.content, m.created_at, m.model],
        )?;
        Ok(())
    }

    pub fn chat(&self, recording_id: Option<&str>) -> Result<Vec<ChatMessage>> {
        let map = |r: &Row| -> rusqlite::Result<ChatMessage> {
            Ok(ChatMessage {
                id: r.get(0)?,
                recording_id: r.get(1)?,
                role: r.get(2)?,
                content: r.get(3)?,
                created_at: r.get(4)?,
                model: r.get(5)?,
            })
        };
        let sql = "SELECT id, recording_id, role, content, created_at, model FROM chat_messages";
        let rows: Vec<ChatMessage> = match recording_id {
            Some(id) => {
                let mut st = self.conn.prepare(&format!(
                    "{sql} WHERE recording_id = ?1 ORDER BY created_at, rowid"
                ))?;
                let v = st.query_map([id], map)?.collect::<rusqlite::Result<_>>()?;
                v
            }
            None => {
                let mut st = self.conn.prepare(&format!(
                    "{sql} WHERE recording_id IS NULL ORDER BY created_at, rowid"
                ))?;
                let v = st.query_map([], map)?.collect::<rusqlite::Result<_>>()?;
                v
            }
        };
        Ok(rows)
    }

    pub fn clear_chat(&self, recording_id: Option<&str>) -> Result<()> {
        match recording_id {
            Some(id) => self
                .conn
                .execute("DELETE FROM chat_messages WHERE recording_id = ?1", [id])?,
            None => self
                .conn
                .execute("DELETE FROM chat_messages WHERE recording_id IS NULL", [])?,
        };
        Ok(())
    }

    // ---- docs

    pub fn save_doc(&self, d: &Doc) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO docs (id, recording_id, title, prompt, html, model, created_at, updated_at) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![d.id, d.recording_id, d.title, d.prompt, d.html, d.model, d.created_at, d.updated_at],
        )?;
        Ok(())
    }

    fn doc_from_row(r: &Row) -> rusqlite::Result<Doc> {
        Ok(Doc {
            id: r.get(0)?,
            recording_id: r.get(1)?,
            title: r.get(2)?,
            prompt: r.get(3)?,
            html: r.get(4)?,
            model: r.get(5)?,
            created_at: r.get(6)?,
            updated_at: r.get(7)?,
        })
    }

    pub fn docs(&self, recording_id: &str) -> Result<Vec<Doc>> {
        let mut st = self.conn.prepare(
            "SELECT id, recording_id, title, prompt, html, model, created_at, updated_at FROM docs \
             WHERE recording_id = ?1 ORDER BY created_at DESC",
        )?;
        let rows = st.query_map([recording_id], Self::doc_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn doc(&self, id: &str) -> Result<Doc> {
        Ok(self.conn.query_row(
            "SELECT id, recording_id, title, prompt, html, model, created_at, updated_at FROM docs WHERE id = ?1",
            [id],
            Self::doc_from_row,
        )?)
    }

    pub fn delete_doc(&self, id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM docs WHERE id = ?1", [id])?;
        Ok(())
    }

    // ---- notes

    fn note_from_row(r: &Row) -> rusqlite::Result<Note> {
        Ok(Note {
            id: r.get("id")?,
            recording_id: r.get("recording_id")?,
            start: r.get("start")?,
            end: r.get("end")?,
            body: r.get("body")?,
            color: r.get("color")?,
            created_at: r.get("created_at")?,
            updated_at: r.get("updated_at")?,
        })
    }

    fn clean_note(start: f64, end: f64, color: Option<&str>) -> Result<(f64, f64, Option<String>)> {
        if !start.is_finite() || !end.is_finite() || start < 0.0 {
            anyhow::bail!("invalid note time range");
        }
        let (a, b) = if end < start {
            (end, start)
        } else {
            (start, end)
        };
        let color = color.filter(|c| NOTE_COLORS.contains(c)).map(String::from);
        Ok((a, b, color))
    }

    pub fn add_note(
        &self,
        recording_id: &str,
        start: f64,
        end: f64,
        body: &str,
        color: Option<&str>,
    ) -> Result<Note> {
        let (start, end, color) = Self::clean_note(start, end, color)?;
        let now = now_iso();
        let note = Note {
            id: uuid::Uuid::new_v4().to_string(),
            recording_id: recording_id.to_string(),
            start,
            end,
            body: body.trim().to_string(),
            color,
            created_at: now.clone(),
            updated_at: now,
        };
        self.conn.execute(
            "INSERT INTO notes (id, recording_id, start, end, body, norm, color, created_at, updated_at) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                note.id,
                note.recording_id,
                note.start,
                note.end,
                note.body,
                normalize_for_search(&note.body),
                note.color,
                note.created_at,
                note.updated_at
            ],
        )?;
        Ok(note)
    }

    pub fn update_note(
        &self,
        id: &str,
        start: f64,
        end: f64,
        body: &str,
        color: Option<&str>,
    ) -> Result<Note> {
        let (start, end, color) = Self::clean_note(start, end, color)?;
        let n = self.conn.execute(
            "UPDATE notes SET start = ?2, end = ?3, body = ?4, norm = ?5, color = ?6, updated_at = ?7 WHERE id = ?1",
            params![id, start, end, body.trim(), normalize_for_search(body), color, now_iso()],
        )?;
        if n == 0 {
            anyhow::bail!("note not found");
        }
        Ok(self.conn.query_row(
            "SELECT * FROM notes WHERE id = ?1",
            [id],
            Self::note_from_row,
        )?)
    }

    pub fn delete_note(&self, id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM notes WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn notes(&self, recording_id: &str) -> Result<Vec<Note>> {
        let mut st = self
            .conn
            .prepare("SELECT * FROM notes WHERE recording_id = ?1 ORDER BY start, created_at")?;
        let rows = st.query_map([recording_id], Self::note_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn all_notes(&self) -> Result<Vec<NoteWithRecording>> {
        let mut st = self.conn.prepare(
            "SELECT n.*, r.title AS recording_title FROM notes n JOIN recordings r ON r.id = n.recording_id \
             ORDER BY r.created_at DESC, n.start, n.created_at",
        )?;
        let rows = st.query_map([], |r| {
            Ok(NoteWithRecording {
                note: Self::note_from_row(r)?,
                recording_title: r.get("recording_title")?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Notes whose normalized body contains every term (already normalized, LIKE-escaped).
    pub fn search_notes(
        &self,
        like_terms: &[String],
        recording_id: Option<&str>,
    ) -> Result<Vec<NoteWithRecording>> {
        let mut sql = String::from(
            "SELECT n.*, r.title AS recording_title FROM notes n JOIN recordings r ON r.id = n.recording_id WHERE 1=1",
        );
        let mut args: Vec<String> = Vec::new();
        for t in like_terms {
            args.push(t.clone());
            sql.push_str(&format!(" AND n.norm LIKE ?{} ESCAPE '\\'", args.len()));
        }
        if let Some(id) = recording_id {
            args.push(id.to_string());
            sql.push_str(&format!(" AND n.recording_id = ?{}", args.len()));
        }
        sql.push_str(" ORDER BY r.created_at DESC, n.start LIMIT 200");
        let mut st = self.conn.prepare(&sql)?;
        let rows = st.query_map(rusqlite::params_from_iter(args.iter()), |r| {
            Ok(NoteWithRecording {
                note: Self::note_from_row(r)?,
                recording_title: r.get("recording_title")?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // ---- embeddings

    pub fn replace_embeddings(
        &mut self,
        id: &str,
        model: &str,
        rows: &[EmbeddingRow],
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM embeddings WHERE recording_id = ?1", [id])?;
        {
            let mut st = tx.prepare(
                "INSERT INTO embeddings (recording_id, chunk, start_seg, end_seg, model, vector) VALUES (?1,?2,?3,?4,?5,?6)",
            )?;
            for r in rows {
                let bytes: Vec<u8> = r.vector.iter().flat_map(|v| v.to_le_bytes()).collect();
                st.execute(params![id, r.chunk, r.start_seg, r.end_seg, model, bytes])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn has_embeddings(&self, id: &str, model: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM embeddings WHERE recording_id = ?1 AND model = ?2)",
            params![id, model],
            |r| r.get::<_, i64>(0),
        )? != 0)
    }

    pub fn embeddings(&self, model: &str, recording_id: Option<&str>) -> Result<Vec<EmbeddingRow>> {
        let mut st = self.conn.prepare(
            "SELECT recording_id, chunk, start_seg, end_seg, vector FROM embeddings \
             WHERE model = ?1 AND (?2 IS NULL OR recording_id = ?2)",
        )?;
        let rows = st.query_map(params![model, recording_id], |r| {
            let blob: Vec<u8> = r.get(4)?;
            Ok(EmbeddingRow {
                recording_id: r.get(0)?,
                chunk: r.get::<_, i64>(1)? as u32,
                start_seg: r.get::<_, i64>(2)? as u32,
                end_seg: r.get::<_, i64>(3)? as u32,
                vector: blob
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| f32::from_le_bytes(*c))
                    .collect(),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Tag;

    pub fn sample_recording(id: &str) -> Recording {
        Recording {
            id: id.into(),
            title: "جلسه".into(),
            source_path: "/tmp/a.mp3".into(),
            audio_path: "/tmp/a.mp3".into(),
            playback_wav_path: None,
            duration_sec: Some(10.0),
            created_at: now_iso(),
            updated_at: now_iso(),
            status: RecordingStatus::New,
            model_id: None,
            error: None,
            segment_count: 0,
            has_analysis: false,
            tags: vec![],
            blurb: None,
        }
    }

    #[test]
    fn roundtrip_recording_segments_analysis() {
        let mut db = Db::open_in_memory().unwrap();
        db.insert_recording(&sample_recording("r1")).unwrap();
        let segs = vec![
            Segment {
                id: 0,
                start: 0.0,
                end: 1.0,
                text: "سلام".into(),
                words: vec![],
                original_text: None,
            },
            Segment {
                id: 1,
                start: 1.0,
                end: 2.0,
                text: "خداحافظ".into(),
                words: vec![],
                original_text: None,
            },
        ];
        db.replace_segments("r1", "shenava-koochik", &segs).unwrap();
        let r = db.recording("r1").unwrap();
        assert_eq!(r.segment_count, 2);
        assert_eq!(r.status, RecordingStatus::Ready);
        assert_eq!(db.segments("r1").unwrap(), segs);

        let a = Analysis {
            summary: "خلاصه\n\nادامه".into(),
            tags: vec![Tag {
                name: "خوشامد".into(),
                description: String::new(),
                segments: vec![0],
            }],
            created_at: now_iso(),
            ..Default::default()
        };
        db.save_analysis("r1", &a).unwrap();
        let r = db.recording("r1").unwrap();
        assert!(r.has_analysis);
        assert_eq!(r.tags, vec!["خوشامد".to_string()]);
        assert_eq!(r.blurb.as_deref(), Some("خلاصه"));
        assert_eq!(db.all_tags().unwrap().len(), 1);

        assert!(db.set_suggested_title("r1", "AI title").unwrap());
        db.set_title("r1", "My title").unwrap();
        assert!(!db.set_suggested_title("r1", "Another AI title").unwrap());
        assert_eq!(db.recording("r1").unwrap().title, "My title");

        let s = db.update_segment_text("r1", 1, "بدرود").unwrap();
        assert_eq!(s.text, "بدرود");

        // re-transcribing drops the analysis that referenced the old segment ids
        db.replace_segments("r1", "taraneh-wlarge", &segs).unwrap();
        let r = db.recording("r1").unwrap();
        assert!(!r.has_analysis);
        assert!(r.tags.is_empty());
        assert!(db.all_tags().unwrap().is_empty());

        db.delete_recording("r1").unwrap();
        assert!(db.get_recording("r1").unwrap().is_none());
        assert!(db.segments("r1").unwrap().is_empty());
    }

    #[test]
    fn ai_fixes_keep_the_original() {
        let mut db = Db::open_in_memory().unwrap();
        db.insert_recording(&sample_recording("r1")).unwrap();
        let word = |t: &str| Word {
            start: 0.0,
            end: 1.0,
            text: t.into(),
        };
        let seg = |id: u32, text: &str| Segment {
            id,
            start: id as f64,
            end: id as f64 + 1.0,
            text: text.into(),
            words: vec![word(text)],
            original_text: None,
        };
        db.replace_segments(
            "r1",
            "m",
            &[seg(0, "سلام"), seg(1, "خداحافز"), seg(2, "متن")],
        )
        .unwrap();
        let changed = db
            .apply_fixes(
                "r1",
                &[
                    (1, "خداحافز".into(), "خداحافظ".into()),
                    (2, "edited meanwhile".into(), "x".into()),
                    (0, "سلام".into(), "سلام".into()),
                ],
            )
            .unwrap();
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].text, "خداحافظ");
        assert_eq!(changed[0].original_text.as_deref(), Some("خداحافز"));
        assert_eq!(changed[0].words.len(), 1);
        // search sees the fixed text
        let hits = crate::search::lexical(&db, "خداحافظ", Some("r1"), 10).unwrap();
        assert_eq!(hits.len(), 1);

        // a second fix keeps the recognizer's text, not the first fix
        db.apply_fixes("r1", &[(1, "خداحافظ".into(), "خداحافظ!".into())])
            .unwrap();
        let s = &db.segments("r1").unwrap()[1];
        assert_eq!(s.original_text.as_deref(), Some("خداحافز"));
        // a manual edit keeps it too, and restoring the original text drops it
        let s = db.update_segment_text("r1", 1, "خدانگهدار").unwrap();
        assert_eq!(s.original_text.as_deref(), Some("خداحافز"));
        let s = db.update_segment_text("r1", 1, "خداحافز").unwrap();
        assert_eq!(s.original_text, None);

        db.apply_fixes(
            "r1",
            &[
                (0, "سلام".into(), "سلام،".into()),
                (1, "خداحافز".into(), "خداحافظ".into()),
            ],
        )
        .unwrap();
        let reverted = db.revert_fixes("r1", Some(&[0])).unwrap();
        assert_eq!(reverted.len(), 1);
        assert_eq!(
            (
                reverted[0].text.as_str(),
                reverted[0].original_text.as_deref()
            ),
            ("سلام", None)
        );
        assert_eq!(db.revert_fixes("r1", None).unwrap().len(), 1);
        assert!(db
            .segments("r1")
            .unwrap()
            .iter()
            .all(|s| s.original_text.is_none()));
        assert_eq!(db.segments("r1").unwrap()[1].text, "خداحافز");

        // re-transcribing starts over
        db.apply_fixes("r1", &[(2, "متن".into(), "متن.".into())])
            .unwrap();
        db.replace_segments("r1", "m", &[seg(0, "سلام")]).unwrap();
        assert_eq!(db.segments("r1").unwrap()[0].original_text, None);
    }

    #[test]
    fn notes_crud_and_cascade() {
        let db = Db::open_in_memory().unwrap();
        db.insert_recording(&sample_recording("r1")).unwrap();
        let n = db
            .add_note(
                "r1",
                12.0,
                5.0,
                "  مهم: https://example.com  ",
                Some("blue"),
            )
            .unwrap();
        assert_eq!((n.start, n.end), (5.0, 12.0)); // swapped into order
        assert_eq!(n.body, "مهم: https://example.com");
        let bad = db
            .add_note("r1", 1.0, 2.0, "x", Some("chartreuse"))
            .unwrap();
        assert_eq!(bad.color, None);
        assert!(db.add_note("r1", f64::NAN, 2.0, "x", None).is_err());
        let u = db.update_note(&n.id, 6.0, 8.0, "مهم‌تر", None).unwrap();
        assert_eq!(
            (u.start, u.body.as_str(), u.color.as_deref()),
            (6.0, "مهم‌تر", None)
        );
        assert_eq!(db.notes("r1").unwrap().len(), 2);
        assert_eq!(db.notes("r1").unwrap()[0].id, bad.id); // sorted by start
        let found = db.search_notes(&["%مهمتر%".to_string()], None).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].recording_title, "جلسه");
        assert_eq!(db.all_notes().unwrap().len(), 2);
        db.delete_note(&bad.id).unwrap();
        assert_eq!(db.notes("r1").unwrap().len(), 1);
        db.delete_recording("r1").unwrap();
        assert!(db.all_notes().unwrap().is_empty());
    }

    #[test]
    fn chat_scopes() {
        let db = Db::open_in_memory().unwrap();
        db.insert_recording(&sample_recording("r1")).unwrap();
        for (rid, content) in [(Some("r1"), "a"), (None, "b")] {
            db.add_chat(&ChatMessage {
                id: uuid::Uuid::new_v4().to_string(),
                recording_id: rid.map(String::from),
                role: "user".into(),
                content: content.into(),
                created_at: now_iso(),
                model: None,
            })
            .unwrap();
        }
        assert_eq!(db.chat(Some("r1")).unwrap().len(), 1);
        assert_eq!(db.chat(None).unwrap()[0].content, "b");
        db.clear_chat(None).unwrap();
        assert!(db.chat(None).unwrap().is_empty());
        assert_eq!(db.chat(Some("r1")).unwrap().len(), 1);
    }
}
