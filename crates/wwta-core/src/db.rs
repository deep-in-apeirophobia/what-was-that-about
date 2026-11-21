//! SQLite-backed library: recordings, transcripts, AI analysis, chats, docs and embeddings.

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::text::normalize_for_search;
use crate::types::{
    now_iso, Analysis, ChatMessage, Doc, Recording, RecordingStatus, Segment, Word,
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
    blurb TEXT
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

    pub fn set_title(&self, id: &str, title: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE recordings SET title = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, title, now_iso()],
        )?;
        Ok(())
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
