//! Library search: diacritic-insensitive substring search, tag search, semantic search over
//! embeddings and a lexical "related sections" fallback that works offline.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use rusqlite::params_from_iter;

use crate::db::{Db, EmbeddingRow};
use crate::text::{find_highlights, normalize_for_search, search_terms};
use crate::types::{HitKind, SearchHit, Segment, TagSummary};

fn like_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('%');
    for c in s.chars() {
        if matches!(c, '%' | '_' | '\\') {
            o.push('\\');
        }
        o.push(c);
    }
    o.push('%');
    o
}

pub fn lexical(
    db: &Db,
    query: &str,
    recording_id: Option<&str>,
    limit: usize,
) -> Result<Vec<SearchHit>> {
    let terms = search_terms(query);
    if terms.is_empty() {
        return Ok(vec![]);
    }
    let mut sql = String::from(
        "SELECT s.recording_id, r.title, s.idx, s.start, s.end, s.text, s.norm FROM segments s \
         JOIN recordings r ON r.id = s.recording_id WHERE 1=1",
    );
    let mut args: Vec<String> = Vec::new();
    for t in &terms {
        args.push(like_escape(&normalize_for_search(t)));
        sql.push_str(&format!(" AND s.norm LIKE ?{} ESCAPE '\\'", args.len()));
    }
    if let Some(id) = recording_id {
        args.push(id.to_string());
        sql.push_str(&format!(" AND s.recording_id = ?{}", args.len()));
    }
    sql.push_str(&format!(
        " ORDER BY r.created_at DESC, s.idx LIMIT {}",
        limit.max(1) * 4
    ));
    let mut st = db.conn.prepare(&sql)?;
    let phrase = normalize_for_search(&terms.join(" "));
    let rows = st.query_map(params_from_iter(args.iter()), |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, f64>(3)?,
            r.get::<_, f64>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, String>(6)?,
        ))
    })?;
    let mut hits = Vec::new();
    for row in rows {
        let (rid, title, idx, start, end, text, norm) = row?;
        let exact = norm.contains(&phrase);
        let whole_word = norm.split_whitespace().any(|w| w == phrase);
        let score = 0.6 + if exact { 0.25 } else { 0.0 } + if whole_word { 0.15 } else { 0.0 };
        hits.push(SearchHit {
            recording_id: rid,
            recording_title: title,
            segment_id: idx as u32,
            start,
            end,
            highlights: find_highlights(&text, &terms),
            text,
            score,
            kind: HitKind::Lexical,
        });
    }
    hits.sort_by(|a, b| b.score.total_cmp(&a.score));
    hits.truncate(limit);
    Ok(hits)
}

pub fn list_tags(db: &Db) -> Result<Vec<TagSummary>> {
    let mut by_name: HashMap<String, (String, HashSet<String>, u32)> = HashMap::new();
    for (rid, _title, tag, segs) in db.all_tags()? {
        let e = by_name
            .entry(normalize_for_search(&tag))
            .or_insert_with(|| (tag.clone(), HashSet::new(), 0));
        e.1.insert(rid);
        e.2 += segs.len() as u32;
    }
    let mut v: Vec<TagSummary> = by_name
        .into_values()
        .map(|(name, recs, n)| TagSummary {
            name,
            recording_count: recs.len() as u32,
            segment_count: n,
        })
        .collect();
    v.sort_by(|a, b| {
        b.recording_count
            .cmp(&a.recording_count)
            .then(b.segment_count.cmp(&a.segment_count))
            .then(a.name.cmp(&b.name))
    });
    Ok(v)
}

fn hits_for_segments(
    db: &Db,
    rid: &str,
    title: &str,
    segs: &[u32],
    kind: HitKind,
    score: f32,
    out: &mut Vec<SearchHit>,
) -> Result<()> {
    let all = db.segments(rid)?;
    for &sid in segs {
        if let Some(s) = all.get(sid as usize) {
            out.push(SearchHit {
                recording_id: rid.into(),
                recording_title: title.into(),
                segment_id: s.id,
                start: s.start,
                end: s.end,
                text: s.text.clone(),
                score,
                kind,
                highlights: vec![],
            });
        }
    }
    Ok(())
}

pub fn segments_for_tag(db: &Db, tag: &str) -> Result<Vec<SearchHit>> {
    let norm = normalize_for_search(tag);
    let mut out = Vec::new();
    for (rid, title, t, segs) in db.all_tags()? {
        if normalize_for_search(&t) == norm {
            hits_for_segments(db, &rid, &title, &segs, HitKind::Tag, 1.0, &mut out)?;
        }
    }
    Ok(out)
}

/// Tags whose name contains the query: return their first segment per recording.
pub fn tag_hits(db: &Db, query: &str, recording_id: Option<&str>) -> Result<Vec<SearchHit>> {
    let q = normalize_for_search(query.trim());
    if q.chars().count() < 2 {
        return Ok(vec![]);
    }
    let mut out = Vec::new();
    for (rid, title, t, segs) in db.all_tags()? {
        if recording_id.is_some_and(|r| r != rid) {
            continue;
        }
        if normalize_for_search(&t).contains(&q) {
            let first: Vec<u32> = segs.iter().copied().min().into_iter().collect();
            let before = out.len();
            hits_for_segments(db, &rid, &title, &first, HitKind::Tag, 0.7, &mut out)?;
            for h in &mut out[before..] {
                h.text = format!("#{t} — {}", h.text);
            }
        }
    }
    Ok(out)
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let (mut dot, mut na, mut nb) = (0f32, 0f32, 0f32);
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}

/// Group consecutive segments into chunks of roughly `target_chars` for embedding.
pub fn chunk_segments(segs: &[Segment], target_chars: usize) -> Vec<(u32, u32, String)> {
    let mut out = Vec::new();
    let mut cur: Vec<&Segment> = Vec::new();
    let mut len = 0;
    for s in segs {
        cur.push(s);
        len += s.text.chars().count() + 1;
        if len >= target_chars || cur.len() >= 6 {
            out.push((
                cur[0].id,
                cur[cur.len() - 1].id,
                cur.iter()
                    .map(|s| s.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" "),
            ));
            cur.clear();
            len = 0;
        }
    }
    if !cur.is_empty() {
        out.push((
            cur[0].id,
            cur[cur.len() - 1].id,
            cur.iter()
                .map(|s| s.text.as_str())
                .collect::<Vec<_>>()
                .join(" "),
        ));
    }
    out
}

/// Rank stored embedding chunks against a query vector.
pub fn semantic_hits(
    db: &Db,
    rows: &[EmbeddingRow],
    query: &[f32],
    exclude: Option<(&str, u32)>,
    limit: usize,
    min_score: f32,
) -> Result<Vec<SearchHit>> {
    let mut scored: Vec<(f32, &EmbeddingRow)> = rows
        .iter()
        .filter(|r| match exclude {
            Some((rid, seg)) => {
                !(r.recording_id == rid && r.start_seg <= seg + 6 && r.end_seg + 6 >= seg)
            }
            None => true,
        })
        .map(|r| (cosine(query, &r.vector), r))
        .filter(|(s, _)| *s >= min_score)
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored.truncate(limit);
    let mut titles: HashMap<String, String> = HashMap::new();
    let mut seg_cache: HashMap<String, Vec<Segment>> = HashMap::new();
    let mut out = Vec::new();
    for (score, r) in scored {
        if !titles.contains_key(&r.recording_id) {
            let t = db
                .get_recording(&r.recording_id)?
                .map(|r| r.title)
                .unwrap_or_default();
            titles.insert(r.recording_id.clone(), t);
            seg_cache.insert(r.recording_id.clone(), db.segments(&r.recording_id)?);
        }
        let segs = &seg_cache[&r.recording_id];
        let span: Vec<&Segment> = segs
            .iter()
            .filter(|s| s.id >= r.start_seg && s.id <= r.end_seg)
            .collect();
        let Some(first) = span.first() else { continue };
        out.push(SearchHit {
            recording_id: r.recording_id.clone(),
            recording_title: titles[&r.recording_id].clone(),
            segment_id: first.id,
            start: first.start,
            end: span.last().unwrap().end,
            text: span
                .iter()
                .map(|s| s.text.as_str())
                .collect::<Vec<_>>()
                .join(" "),
            score,
            kind: HitKind::Semantic,
            highlights: vec![],
        });
    }
    Ok(out)
}

const STOPWORDS: &[&str] = &[
    "و",
    "در",
    "به",
    "از",
    "که",
    "این",
    "را",
    "با",
    "است",
    "برای",
    "آن",
    "یک",
    "هم",
    "تا",
    "می",
    "ها",
    "های",
    "شود",
    "کرد",
    "بود",
    "شد",
    "کنیم",
    "کنند",
    "کند",
    "ما",
    "من",
    "تو",
    "او",
    "شما",
    "آنها",
    "اما",
    "یا",
    "اگر",
    "پس",
    "چون",
    "هر",
    "همه",
    "بر",
    "دیگر",
    "خود",
    "نیز",
    "باید",
    "دارد",
    "داریم",
    "هست",
    "نیست",
    "بله",
    "نه",
    "خب",
    "خوب",
    "حالا",
    "یعنی",
    "اینکه",
    "چه",
    "چی",
    "رو",
    "هم",
    "دیگه",
    "اون",
    "این",
    "ای",
    "بی",
    "تر",
    "ترین",
    "کردن",
    "شدن",
    "بودن",
    "میشه",
    "هستند",
    "بودند",
    "the",
    "a",
    "an",
    "and",
    "of",
    "to",
    "in",
    "is",
    "it",
    "that",
];

fn terms_of(text: &str) -> Vec<String> {
    let stop: HashSet<&str> = STOPWORDS.iter().copied().collect();
    normalize_for_search(text)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 3 && !stop.contains(w))
        .map(String::from)
        .collect()
}

/// TF-IDF overlap between one segment (with its neighbours) and every other segment.
pub fn lexical_related(db: &Db, rid: &str, seg_id: u32, limit: usize) -> Result<Vec<SearchHit>> {
    let segs = db.segments(rid)?;
    let lo = seg_id.saturating_sub(1) as usize;
    let hi = ((seg_id + 1) as usize).min(segs.len().saturating_sub(1));
    if segs.is_empty() || seg_id as usize >= segs.len() {
        return Ok(vec![]);
    }
    let query: HashSet<String> = segs[lo..=hi]
        .iter()
        .flat_map(|s| terms_of(&s.text))
        .collect();
    if query.is_empty() {
        return Ok(vec![]);
    }
    let mut st = db.conn.prepare(
        "SELECT s.recording_id, r.title, s.idx, s.start, s.end, s.text FROM segments s JOIN recordings r ON r.id = s.recording_id",
    )?;
    let rows: Vec<(String, String, u32, f64, f64, String)> = st
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get::<_, i64>(2)? as u32,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let docs: Vec<HashSet<String>> = rows
        .iter()
        .map(|r| terms_of(&r.5).into_iter().collect())
        .collect();
    let n = docs.len() as f32;
    let mut df: HashMap<&str, u32> = HashMap::new();
    for d in &docs {
        for t in d {
            if query.contains(t) {
                *df.entry(t.as_str()).or_default() += 1;
            }
        }
    }
    let idf = |t: &str| ((n + 1.0) / (*df.get(t).unwrap_or(&0) as f32 + 1.0)).ln() + 1.0;
    let qnorm: f32 = query.iter().map(|t| idf(t).powi(2)).sum::<f32>().sqrt();
    let mut scored: Vec<(f32, usize)> = docs
        .iter()
        .enumerate()
        .filter(|(i, _)| {
            let r = &rows[*i];
            !(r.0 == rid && (r.2 as i64 - seg_id as i64).abs() <= 2)
        })
        .filter_map(|(i, d)| {
            let shared: f32 = d
                .iter()
                .filter(|t| query.contains(*t))
                .map(|t| idf(t).powi(2))
                .sum();
            if shared == 0.0 {
                return None;
            }
            let dnorm: f32 = d.iter().map(|t| idf(t).powi(2)).sum::<f32>().sqrt();
            Some((shared / (qnorm * dnorm.max(1e-6)), i))
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored.truncate(limit);
    let qterms: Vec<String> = query.into_iter().collect();
    Ok(scored
        .into_iter()
        .map(|(score, i)| {
            let r = &rows[i];
            SearchHit {
                recording_id: r.0.clone(),
                recording_title: r.1.clone(),
                segment_id: r.2,
                start: r.3,
                end: r.4,
                highlights: find_highlights(&r.5, &qterms),
                text: r.5.clone(),
                score,
                kind: HitKind::Lexical,
            }
        })
        .collect())
}

/// Merge hit lists, dropping duplicates (same recording + segment), best score first.
pub fn merge_hits(lists: Vec<Vec<SearchHit>>, limit: usize) -> Vec<SearchHit> {
    let mut seen: HashSet<(String, u32)> = HashSet::new();
    let mut all: Vec<SearchHit> = lists.into_iter().flatten().collect();
    all.sort_by(|a, b| b.score.total_cmp(&a.score));
    all.retain(|h| seen.insert((h.recording_id.clone(), h.segment_id)));
    all.truncate(limit);
    all
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::types::{now_iso, Analysis, Recording, RecordingStatus, Tag};

    fn setup() -> Db {
        let mut db = Db::open_in_memory().unwrap();
        for (id, title) in [("a", "جلسه اول"), ("b", "پادکست")] {
            db.insert_recording(&Recording {
                id: id.into(),
                title: title.into(),
                source_path: String::new(),
                audio_path: String::new(),
                playback_wav_path: None,
                duration_sec: None,
                created_at: now_iso(),
                updated_at: now_iso(),
                status: RecordingStatus::New,
                model_id: None,
                error: None,
                segment_count: 0,
                has_analysis: false,
                tags: vec![],
                blurb: None,
            })
            .unwrap();
        }
        let seg = |i: u32, t: &str| Segment {
            id: i,
            start: i as f64,
            end: i as f64 + 1.0,
            text: t.into(),
            words: vec![],
        };
        db.replace_segments(
            "a",
            "m",
            &[
                seg(0, "امروز درباره هوش مصنوعی صحبت می‌کنیم"),
                seg(1, "بودجه پروژه کم است"),
                seg(2, "خداحافظ"),
            ],
        )
        .unwrap();
        db.replace_segments(
            "b",
            "m",
            &[seg(0, "مدل‌های هوش مصنوعی فارسی"), seg(1, "موسیقی سنتی")],
        )
        .unwrap();
        db.save_analysis(
            "a",
            &Analysis {
                tags: vec![Tag {
                    name: "هوش مصنوعی".into(),
                    description: String::new(),
                    segments: vec![0],
                }],
                ..Default::default()
            },
        )
        .unwrap();
        db.save_analysis(
            "b",
            &Analysis {
                tags: vec![Tag {
                    name: "هوش مصنوعی".into(),
                    description: String::new(),
                    segments: vec![0],
                }],
                ..Default::default()
            },
        )
        .unwrap();
        db
    }

    #[test]
    fn lexical_matches_normalized_and_highlights() {
        let db = setup();
        let hits = lexical(&db, "هوش مصنوعي", None, 10).unwrap();
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().all(|h| !h.highlights.is_empty()));
        let only_b = lexical(&db, "هوش", Some("b"), 10).unwrap();
        assert_eq!(only_b.len(), 1);
        assert!(lexical(&db, "100%", None, 10).unwrap().is_empty());
    }

    #[test]
    fn tags_aggregate_across_recordings() {
        let db = setup();
        let tags = list_tags(&db).unwrap();
        assert_eq!(tags[0].name, "هوش مصنوعی");
        assert_eq!(tags[0].recording_count, 2);
        assert_eq!(segments_for_tag(&db, "هوش مصنوعی").unwrap().len(), 2);
        assert_eq!(tag_hits(&db, "هوش", None).unwrap().len(), 2);
    }

    #[test]
    fn related_finds_other_recording() {
        let db = setup();
        let rel = lexical_related(&db, "a", 0, 5).unwrap();
        assert!(rel
            .iter()
            .any(|h| h.recording_id == "b" && h.segment_id == 0));
    }

    #[test]
    fn chunking_and_cosine() {
        let segs: Vec<Segment> = (0..10)
            .map(|i| Segment {
                id: i,
                start: 0.0,
                end: 0.0,
                text: "x".repeat(100),
                words: vec![],
            })
            .collect();
        let c = chunk_segments(&segs, 300);
        assert_eq!(c[0].0, 0);
        assert_eq!(c[0].1, 2);
        assert_eq!(c.last().unwrap().1, 9);
        assert!((cosine(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
        assert_eq!(cosine(&[1.0, 0.0], &[0.0, 1.0]), 0.0);
    }
}
