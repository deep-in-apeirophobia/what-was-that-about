//! SQLite-backed library: recordings, transcripts, AI analysis, chat threads, docs, notes,
//! embeddings and condensed-transcript digests.

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::text::normalize_for_search;
use crate::types::{
    now_iso, Analysis, ChatAttachment, ChatMessage, ChatThread, Doc, Note, NoteWithRecording,
    Recording, RecordingStatus, Segment, Word, NOTE_COLORS,
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
CREATE TABLE IF NOT EXISTS chat_threads (
    id TEXT PRIMARY KEY,
    recording_id TEXT REFERENCES recordings(id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    summary TEXT,
    compacted_count INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS chat_threads_recording ON chat_threads(recording_id, updated_at);
CREATE TABLE IF NOT EXISTS chat_messages (
    id TEXT PRIMARY KEY,
    recording_id TEXT REFERENCES recordings(id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    created_at TEXT NOT NULL,
    model TEXT,
    thread_id TEXT REFERENCES chat_threads(id) ON DELETE CASCADE,
    context_note TEXT
);
CREATE TABLE IF NOT EXISTS chat_attachments (
    id TEXT PRIMARY KEY,
    message_id TEXT REFERENCES chat_messages(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    name TEXT NOT NULL,
    mime TEXT NOT NULL,
    path TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    duration_sec REAL,
    text TEXT,
    text_model TEXT,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS chat_attachments_message ON chat_attachments(message_id);
CREATE TABLE IF NOT EXISTS transcript_digests (
    recording_id TEXT NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
    level INTEGER NOT NULL,
    start_seg INTEGER NOT NULL,
    end_seg INTEGER NOT NULL,
    fingerprint TEXT NOT NULL,
    text TEXT NOT NULL,
    model TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (recording_id, level, start_seg, end_seg)
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
        let has_thread_id: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('chat_messages') WHERE name = 'thread_id'")?
            .exists([])?;
        if !has_thread_id {
            conn.execute_batch(
                "ALTER TABLE chat_messages ADD COLUMN thread_id TEXT REFERENCES chat_threads(id) ON DELETE CASCADE; \
                 ALTER TABLE chat_messages ADD COLUMN context_note TEXT;",
            )?;
        }
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS chat_messages_thread ON chat_messages(thread_id, created_at)",
        )?;
        let db = Self { conn };
        db.adopt_threadless_messages()?;
        Ok(db)
    }

    /// Chats from builds without threads become one thread per recording (and one for the
    /// library), titled after their first question.
    fn adopt_threadless_messages(&self) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        let scopes: Vec<Option<String>> = {
            let mut st = tx.prepare(
                "SELECT DISTINCT recording_id FROM chat_messages WHERE thread_id IS NULL",
            )?;
            let v = st
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            v
        };
        for rid in scopes {
            let (first, created, updated): (Option<String>, String, String) = tx.query_row(
                "SELECT (SELECT content FROM chat_messages WHERE thread_id IS NULL AND recording_id IS ?1 \
                 AND role = 'user' ORDER BY created_at, rowid LIMIT 1), MIN(created_at), MAX(created_at) \
                 FROM chat_messages WHERE thread_id IS NULL AND recording_id IS ?1",
                [&rid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
            let id = uuid::Uuid::new_v4().to_string();
            tx.execute(
                "INSERT INTO chat_threads (id, recording_id, title, created_at, updated_at) VALUES (?1,?2,?3,?4,?5)",
                params![id, rid, thread_title(first.as_deref().unwrap_or(""), &[]), created, updated],
            )?;
            tx.execute(
                "UPDATE chat_messages SET thread_id = ?1 WHERE thread_id IS NULL AND recording_id IS ?2",
                params![id, rid],
            )?;
        }
        tx.commit()?;
        Ok(())
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
        tx.execute(
            "DELETE FROM transcript_digests WHERE recording_id = ?1",
            [id],
        )?;
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
            "SELECT idx, start, end, text, words FROM segments WHERE recording_id = ?1 ORDER BY idx",
        )?;
        let rows = st.query_map([id], |r| {
            let words: String = r.get(4)?;
            Ok(Segment {
                id: r.get::<_, i64>(0)? as u32,
                start: r.get(1)?,
                end: r.get(2)?,
                text: r.get(3)?,
                words: serde_json::from_str::<Vec<Word>>(&words).unwrap_or_default(),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn update_segment_text(&self, id: &str, seg: u32, text: &str) -> Result<Segment> {
        // Edited text invalidates word timings; keep the segment's overall span.
        self.conn.execute(
            "UPDATE segments SET text = ?3, norm = ?4, words = '[]' WHERE recording_id = ?1 AND idx = ?2",
            params![id, seg, text, normalize_for_search(text)],
        )?;
        self.conn.execute(
            "DELETE FROM embeddings WHERE recording_id = ?1 AND start_seg <= ?2 AND end_seg >= ?2",
            params![id, seg],
        )?;
        self.conn.execute(
            "DELETE FROM transcript_digests WHERE recording_id = ?1 AND start_seg <= ?2 AND end_seg >= ?2",
            params![id, seg],
        )?;
        self.segments(id)?
            .into_iter()
            .find(|s| s.id == seg)
            .ok_or_else(|| anyhow::anyhow!("segment not found"))
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

    // ---- chat threads

    fn thread_from_row(r: &Row) -> rusqlite::Result<ChatThread> {
        Ok(ChatThread {
            id: r.get("id")?,
            recording_id: r.get("recording_id")?,
            title: r.get("title")?,
            created_at: r.get("created_at")?,
            updated_at: r.get("updated_at")?,
            message_count: r.get::<_, i64>("message_count")? as u32,
            summary: r.get("summary")?,
            compacted_count: r.get::<_, i64>("compacted_count")? as u32,
        })
    }

    const THREAD_SELECT: &'static str = "SELECT t.*, \
        (SELECT COUNT(*) FROM chat_messages m WHERE m.thread_id = t.id) AS message_count \
        FROM chat_threads t";

    pub fn create_thread(&self, recording_id: Option<&str>, title: &str) -> Result<ChatThread> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = now_iso();
        self.conn.execute(
            "INSERT INTO chat_threads (id, recording_id, title, created_at, updated_at) VALUES (?1,?2,?3,?4,?4)",
            params![id, recording_id, title, now],
        )?;
        self.thread(&id)
    }

    pub fn thread(&self, id: &str) -> Result<ChatThread> {
        self.conn
            .query_row(
                &format!("{} WHERE t.id = ?1", Self::THREAD_SELECT),
                [id],
                Self::thread_from_row,
            )
            .optional()?
            .ok_or_else(|| anyhow::anyhow!("this conversation no longer exists"))
    }

    /// Threads of one recording (or the library when `None`), most recently active first.
    pub fn threads(&self, recording_id: Option<&str>) -> Result<Vec<ChatThread>> {
        let mut st = self.conn.prepare(&format!(
            "{} WHERE t.recording_id IS ?1 ORDER BY t.updated_at DESC, t.rowid DESC",
            Self::THREAD_SELECT
        ))?;
        let rows = st.query_map([recording_id], Self::thread_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn rename_thread(&self, id: &str, title: &str) -> Result<ChatThread> {
        let title = title.trim();
        if title.is_empty() {
            anyhow::bail!("a conversation needs a name");
        }
        let title: String = title.chars().take(120).collect();
        self.conn.execute(
            "UPDATE chat_threads SET title = ?2 WHERE id = ?1",
            params![id, title],
        )?;
        self.thread(id)
    }

    /// Deletes a thread with its messages; returns the attachment files to remove.
    pub fn delete_thread(&self, id: &str) -> Result<Vec<String>> {
        let paths = self.thread_attachment_paths("m.thread_id = ?1", id)?;
        self.conn
            .execute("DELETE FROM chat_threads WHERE id = ?1", [id])?;
        Ok(paths)
    }

    /// Attachment files of every conversation about a recording (to delete with it).
    pub fn recording_attachment_paths(&self, recording_id: &str) -> Result<Vec<String>> {
        self.thread_attachment_paths("m.recording_id = ?1", recording_id)
    }

    fn thread_attachment_paths(&self, cond: &str, arg: &str) -> Result<Vec<String>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT a.path FROM chat_attachments a JOIN chat_messages m ON m.id = a.message_id WHERE {cond}"
        ))?;
        let v = st
            .query_map([arg], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    pub fn set_thread_summary(&self, id: &str, summary: &str, compacted_count: u32) -> Result<()> {
        self.conn.execute(
            "UPDATE chat_threads SET summary = ?2, compacted_count = ?3 WHERE id = ?1",
            params![id, summary, compacted_count],
        )?;
        Ok(())
    }

    /// Store a message (linking its attachments) and mark the thread as active.
    pub fn add_chat(&self, m: &ChatMessage) -> Result<()> {
        self.conn.execute(
            "INSERT INTO chat_messages (id, recording_id, role, content, created_at, model, thread_id, context_note) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                m.id,
                m.recording_id,
                m.role,
                m.content,
                m.created_at,
                m.model,
                m.thread_id,
                m.context_note
            ],
        )?;
        for a in &m.attachments {
            self.conn.execute(
                "UPDATE chat_attachments SET message_id = ?2 WHERE id = ?1",
                params![a.id, m.id],
            )?;
        }
        self.conn.execute(
            "UPDATE chat_threads SET updated_at = ?2 WHERE id = ?1",
            params![m.thread_id, now_iso()],
        )?;
        Ok(())
    }

    /// Messages of a thread in order, with their attachments.
    pub fn thread_messages(&self, thread_id: &str) -> Result<Vec<ChatMessage>> {
        let mut atts: std::collections::HashMap<String, Vec<ChatAttachment>> =
            std::collections::HashMap::new();
        {
            let mut st = self.conn.prepare(
                "SELECT a.*, a.message_id AS mid FROM chat_attachments a JOIN chat_messages m ON m.id = a.message_id \
                 WHERE m.thread_id = ?1 ORDER BY a.created_at, a.rowid",
            )?;
            let rows = st.query_map([thread_id], |r| {
                Ok((r.get::<_, String>("mid")?, Self::attachment_from_row(r)?))
            })?;
            for row in rows {
                let (mid, a) = row?;
                atts.entry(mid).or_default().push(a);
            }
        }
        let mut st = self.conn.prepare(
            "SELECT id, recording_id, role, content, created_at, model, thread_id, context_note \
             FROM chat_messages WHERE thread_id = ?1 ORDER BY created_at, rowid",
        )?;
        let rows = st.query_map([thread_id], |r| {
            Ok(ChatMessage {
                id: r.get(0)?,
                recording_id: r.get(1)?,
                role: r.get(2)?,
                content: r.get(3)?,
                created_at: r.get(4)?,
                model: r.get(5)?,
                thread_id: r.get(6)?,
                context_note: r.get(7)?,
                attachments: vec![],
            })
        })?;
        let mut out: Vec<ChatMessage> = rows.collect::<rusqlite::Result<_>>()?;
        for m in &mut out {
            if let Some(a) = atts.remove(&m.id) {
                m.attachments = a;
            }
        }
        Ok(out)
    }

    /// Deletes every thread of a recording (or of the library); returns attachment files.
    pub fn clear_chat(&self, recording_id: Option<&str>) -> Result<Vec<String>> {
        let mut paths = Vec::new();
        for t in self.threads(recording_id)? {
            paths.extend(self.delete_thread(&t.id)?);
        }
        Ok(paths)
    }

    // ---- chat attachments

    fn attachment_from_row(r: &Row) -> rusqlite::Result<ChatAttachment> {
        Ok(ChatAttachment {
            id: r.get("id")?,
            kind: r.get("kind")?,
            name: r.get("name")?,
            mime: r.get("mime")?,
            size_bytes: r.get::<_, i64>("size_bytes")? as u64,
            duration_sec: r.get("duration_sec")?,
            path: r.get("path")?,
            text: r.get("text")?,
            text_model: r.get("text_model")?,
        })
    }

    /// Register an uploaded attachment; it stays pending until a message links it.
    pub fn add_attachment(&self, a: &ChatAttachment) -> Result<()> {
        self.conn.execute(
            "INSERT INTO chat_attachments (id, message_id, kind, name, mime, path, size_bytes, duration_sec, text, \
             text_model, created_at) VALUES (?1, NULL, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                a.id,
                a.kind,
                a.name,
                a.mime,
                a.path,
                a.size_bytes as i64,
                a.duration_sec,
                a.text,
                a.text_model,
                now_iso()
            ],
        )?;
        Ok(())
    }

    /// A pending attachment (not yet sent with a message).
    pub fn pending_attachment(&self, id: &str) -> Result<ChatAttachment> {
        self.conn
            .query_row(
                "SELECT * FROM chat_attachments WHERE id = ?1 AND message_id IS NULL",
                [id],
                Self::attachment_from_row,
            )
            .optional()?
            .ok_or_else(|| anyhow::anyhow!("the attachment is gone; attach the file again"))
    }

    pub fn set_attachment_text(&self, id: &str, text: &str, model: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE chat_attachments SET text = ?2, text_model = ?3 WHERE id = ?1",
            params![id, text, model],
        )?;
        Ok(())
    }

    /// Removes a pending attachment; returns its file path.
    pub fn discard_attachment(&self, id: &str) -> Result<Option<String>> {
        let path: Option<String> = self
            .conn
            .query_row(
                "SELECT path FROM chat_attachments WHERE id = ?1 AND message_id IS NULL",
                [id],
                |r| r.get(0),
            )
            .optional()?;
        self.conn.execute(
            "DELETE FROM chat_attachments WHERE id = ?1 AND message_id IS NULL",
            [id],
        )?;
        Ok(path)
    }

    /// Forget attachments never sent (pending ones don't survive a restart) and return the
    /// files that are still referenced.
    pub fn prune_attachments(&self) -> Result<std::collections::HashSet<String>> {
        self.conn
            .execute("DELETE FROM chat_attachments WHERE message_id IS NULL", [])?;
        let mut st = self.conn.prepare("SELECT path FROM chat_attachments")?;
        let v = st
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    // ---- condensed transcript digests

    /// Cached digest of a transcript stretch, if it was made from the same input.
    pub fn digest(
        &self,
        recording_id: &str,
        level: u32,
        start_seg: u32,
        end_seg: u32,
        fingerprint: &str,
    ) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT text FROM transcript_digests WHERE recording_id = ?1 AND level = ?2 AND start_seg = ?3 \
                 AND end_seg = ?4 AND fingerprint = ?5",
                params![recording_id, level, start_seg, end_seg, fingerprint],
                |r| r.get(0),
            )
            .optional()?)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn save_digest(
        &self,
        recording_id: &str,
        level: u32,
        start_seg: u32,
        end_seg: u32,
        fingerprint: &str,
        text: &str,
        model: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO transcript_digests (recording_id, level, start_seg, end_seg, fingerprint, text, \
             model, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![recording_id, level, start_seg, end_seg, fingerprint, text, model, now_iso()],
        )?;
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

/// Title for a new thread: the first line of the question, or what was attached.
pub fn thread_title(question: &str, attachments: &[ChatAttachment]) -> String {
    let line = question.lines().map(str::trim).find(|l| !l.is_empty());
    match line {
        Some(l) => {
            let mut t: String = l.chars().take(60).collect();
            if l.chars().count() > 60 {
                t.push('…');
            }
            t
        }
        None => match attachments.first().map(|a| a.kind.as_str()) {
            Some("audio") => "Voice message".into(),
            Some("image") => "Image".into(),
            _ => "Conversation".into(),
        },
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
            },
            Segment {
                id: 1,
                start: 1.0,
                end: 2.0,
                text: "خداحافظ".into(),
                words: vec![],
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

    fn msg(thread: &ChatThread, role: &str, content: &str) -> ChatMessage {
        ChatMessage {
            id: uuid::Uuid::new_v4().to_string(),
            recording_id: thread.recording_id.clone(),
            thread_id: thread.id.clone(),
            role: role.into(),
            content: content.into(),
            created_at: now_iso(),
            model: None,
            attachments: vec![],
            context_note: None,
        }
    }

    fn attachment(id: &str, kind: &str) -> ChatAttachment {
        ChatAttachment {
            id: id.into(),
            kind: kind.into(),
            name: format!("{id}.bin"),
            mime: "application/octet-stream".into(),
            size_bytes: 3,
            duration_sec: None,
            path: format!("/tmp/{id}.bin"),
            text: None,
            text_model: None,
        }
    }

    #[test]
    fn threads_scope_messages_and_attachments() {
        let db = Db::open_in_memory().unwrap();
        db.insert_recording(&sample_recording("r1")).unwrap();
        let t1 = db.create_thread(Some("r1"), "first").unwrap();
        let t2 = db.create_thread(Some("r1"), "second").unwrap();
        let lib = db.create_thread(None, "library").unwrap();
        db.add_attachment(&attachment("img", "image")).unwrap();
        let mut q = msg(&t1, "user", "what is this?");
        q.attachments = vec![db.pending_attachment("img").unwrap()];
        db.add_chat(&q).unwrap();
        db.add_chat(&msg(&t1, "assistant", "a chart")).unwrap();
        db.add_chat(&msg(&lib, "user", "b")).unwrap();
        // the attachment is linked now, so it's no longer pending
        assert!(db.pending_attachment("img").is_err());
        db.set_attachment_text("img", "a bar chart", "openai/gpt-6-luna")
            .unwrap();

        let m = db.thread_messages(&t1.id).unwrap();
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].attachments.len(), 1);
        assert_eq!(m[0].attachments[0].text.as_deref(), Some("a bar chart"));
        assert!(m[1].attachments.is_empty());
        let threads = db.threads(Some("r1")).unwrap();
        assert_eq!(threads.len(), 2);
        assert_eq!(
            threads
                .iter()
                .find(|t| t.id == t1.id)
                .unwrap()
                .message_count,
            2
        );
        assert_eq!(db.threads(None).unwrap().len(), 1);

        db.set_thread_summary(&t1.id, "- asked about a chart", 2)
            .unwrap();
        let t = db.thread(&t1.id).unwrap();
        assert_eq!(
            (t.summary.as_deref(), t.compacted_count),
            (Some("- asked about a chart"), 2)
        );
        assert_eq!(
            db.rename_thread(&t2.id, "  renamed ").unwrap().title,
            "renamed"
        );
        assert!(db.rename_thread(&t2.id, " ").is_err());

        assert_eq!(
            db.delete_thread(&t1.id).unwrap(),
            vec!["/tmp/img.bin".to_string()]
        );
        assert!(db.thread(&t1.id).is_err());
        assert!(db.prune_attachments().unwrap().is_empty());
        db.clear_chat(None).unwrap();
        assert!(db.threads(None).unwrap().is_empty());
        // deleting the recording takes its threads with it
        db.delete_recording("r1").unwrap();
        assert!(db.threads(Some("r1")).unwrap().is_empty());
    }

    #[test]
    fn pending_attachments_can_be_discarded_and_are_pruned() {
        let db = Db::open_in_memory().unwrap();
        db.add_attachment(&attachment("a", "audio")).unwrap();
        db.add_attachment(&attachment("b", "image")).unwrap();
        assert_eq!(
            db.discard_attachment("a").unwrap().as_deref(),
            Some("/tmp/a.bin")
        );
        assert!(db.pending_attachment("a").is_err());
        assert!(db.prune_attachments().unwrap().is_empty());
        assert!(db.pending_attachment("b").is_err());
    }

    #[test]
    fn migrates_threadless_chats_into_threads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.sqlite");
        {
            // a library from a build before threads existed
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE recordings (id TEXT PRIMARY KEY, title TEXT NOT NULL, source_path TEXT NOT NULL, \
                 audio_path TEXT NOT NULL, playback_wav_path TEXT, duration_sec REAL, created_at TEXT NOT NULL, \
                 updated_at TEXT NOT NULL, status TEXT NOT NULL, model_id TEXT, error TEXT, \
                 tags_json TEXT NOT NULL DEFAULT '[]', blurb TEXT, title_custom INTEGER NOT NULL DEFAULT 0); \
                 CREATE TABLE chat_messages (id TEXT PRIMARY KEY, recording_id TEXT REFERENCES recordings(id) \
                 ON DELETE CASCADE, role TEXT NOT NULL, content TEXT NOT NULL, created_at TEXT NOT NULL, model TEXT); \
                 INSERT INTO recordings (id, title, source_path, audio_path, created_at, updated_at, status) \
                 VALUES ('r1', 't', '', '', '2026-01-01', '2026-01-01', 'ready'); \
                 INSERT INTO chat_messages VALUES ('m1', 'r1', 'user', 'چه گفت؟', '2026-01-01T10:00:00Z', NULL); \
                 INSERT INTO chat_messages VALUES ('m2', 'r1', 'assistant', 'این [3]', '2026-01-01T10:00:05Z', 'x'); \
                 INSERT INTO chat_messages VALUES ('m3', NULL, 'user', 'library?', '2026-01-02T10:00:00Z', NULL);",
            )
            .unwrap();
        }
        let db = Db::open(&path).unwrap();
        let threads = db.threads(Some("r1")).unwrap();
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].title, "چه گفت؟");
        assert_eq!(threads[0].message_count, 2);
        assert_eq!(threads[0].updated_at, "2026-01-01T10:00:05Z");
        let msgs = db.thread_messages(&threads[0].id).unwrap();
        assert_eq!(msgs[1].content, "این [3]");
        assert_eq!(db.threads(None).unwrap()[0].title, "library?");
        drop(db);
        // opening again doesn't duplicate anything
        let db = Db::open(&path).unwrap();
        assert_eq!(db.threads(Some("r1")).unwrap().len(), 1);
    }

    #[test]
    fn digests_are_keyed_by_input_and_dropped_on_edits() {
        let mut db = Db::open_in_memory().unwrap();
        db.insert_recording(&sample_recording("r1")).unwrap();
        let segs: Vec<Segment> = (0..3)
            .map(|i| Segment {
                id: i,
                start: i as f64,
                end: i as f64 + 1.0,
                text: format!("s{i}"),
                words: vec![],
            })
            .collect();
        db.replace_segments("r1", "m", &segs).unwrap();
        db.save_digest("r1", 0, 0, 1, "fp", "notes [0]", "model")
            .unwrap();
        db.save_digest("r1", 0, 2, 2, "fp2", "notes [2]", "model")
            .unwrap();
        assert_eq!(
            db.digest("r1", 0, 0, 1, "fp").unwrap().as_deref(),
            Some("notes [0]")
        );
        assert!(db.digest("r1", 0, 0, 1, "other").unwrap().is_none());
        db.update_segment_text("r1", 1, "edited").unwrap();
        assert!(db.digest("r1", 0, 0, 1, "fp").unwrap().is_none());
        assert!(db.digest("r1", 0, 2, 2, "fp2").unwrap().is_some());
        db.replace_segments("r1", "m", &segs).unwrap();
        assert!(db.digest("r1", 0, 2, 2, "fp2").unwrap().is_none());
    }

    #[test]
    fn titles_threads_from_questions_or_attachments() {
        assert_eq!(
            thread_title("\n  What was decided?\nmore", &[]),
            "What was decided?"
        );
        assert_eq!(
            thread_title("", &[attachment("a", "audio")]),
            "Voice message"
        );
        assert_eq!(thread_title(" ", &[attachment("a", "image")]), "Image");
        assert_eq!(thread_title("", &[]), "Conversation");
        let long = "x".repeat(80);
        assert_eq!(thread_title(&long, &[]).chars().count(), 61);
    }
}
