//! Transcript export formats.

use crate::text::fmt_time;
use crate::types::{Analysis, Note, Segment};

fn ts(secs: f64, sep: char) -> String {
    let ms = (secs.max(0.0) * 1000.0).round() as u64;
    format!(
        "{:02}:{:02}:{:02}{sep}{:03}",
        ms / 3_600_000,
        (ms / 60_000) % 60,
        (ms / 1000) % 60,
        ms % 1000
    )
}

pub fn srt(segs: &[Segment]) -> String {
    let mut o = String::new();
    for (i, s) in segs.iter().enumerate() {
        o.push_str(&format!(
            "{}\n{} --> {}\n{}\n\n",
            i + 1,
            ts(s.start, ','),
            ts(s.end, ','),
            s.text.trim()
        ));
    }
    o
}

pub fn vtt(segs: &[Segment]) -> String {
    let mut o = String::from("WEBVTT\n\n");
    for s in segs {
        o.push_str(&format!(
            "{} --> {}\n{}\n\n",
            ts(s.start, '.'),
            ts(s.end, '.'),
            s.text.trim()
        ));
    }
    o
}

pub fn txt(segs: &[Segment]) -> String {
    segs.iter()
        .map(|s| format!("[{}] {}", fmt_time(s.start), s.text.trim()))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

pub fn markdown(
    title: &str,
    segs: &[Segment],
    analysis: Option<&Analysis>,
    notes: &[Note],
) -> String {
    let mut o = format!("# {title}\n\n");
    if let Some(a) = analysis {
        if !a.summary.is_empty() {
            o.push_str(&format!("## Summary\n\n{}\n\n", a.summary));
        }
        if !a.key_points.is_empty() {
            o.push_str("## Key points\n\n");
            for k in &a.key_points {
                o.push_str(&format!("- {k}\n"));
            }
            o.push('\n');
        }
        if !a.action_items.is_empty() {
            o.push_str("## Action items\n\n");
            for k in &a.action_items {
                o.push_str(&format!("- [ ] {k}\n"));
            }
            o.push('\n');
        }
    }
    if !notes.is_empty() {
        o.push_str("## Notes\n\n");
        for n in notes {
            o.push_str(&format!(
                "- **{}–{}** {}\n",
                fmt_time(n.start),
                fmt_time(n.end),
                n.body.replace('\n', "\n  ")
            ));
        }
        o.push('\n');
    }
    o.push_str("## Transcript\n\n");
    let chapters = analysis.map(|a| a.chapters.as_slice()).unwrap_or(&[]);
    for s in segs {
        if let Some(c) = chapters.iter().find(|c| c.start_segment == s.id) {
            o.push_str(&format!("\n### {} ({})\n\n", c.title, fmt_time(c.start)));
        }
        o.push_str(&format!("**{}** {}\n\n", fmt_time(s.start), s.text.trim()));
    }
    o
}

pub fn json(title: &str, segs: &[Segment], analysis: Option<&Analysis>, notes: &[Note]) -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "title": title,
        "segments": segs,
        "analysis": analysis,
        "notes": notes,
    }))
    .unwrap_or_default()
}

pub fn render(
    format: &str,
    title: &str,
    segs: &[Segment],
    analysis: Option<&Analysis>,
    notes: &[Note],
) -> Option<String> {
    Some(match format {
        "srt" => srt(segs),
        "vtt" => vtt(segs),
        "txt" => txt(segs),
        "md" => markdown(title, segs, analysis, notes),
        "json" => json(title, segs, analysis, notes),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_in_markdown_and_json() {
        let segs = vec![Segment {
            id: 0,
            start: 1.5,
            end: 3.0,
            text: "سلام".into(),
            words: vec![],
            original_text: None,
        }];
        let note = Note {
            id: "n".into(),
            recording_id: "r".into(),
            start: 1.0,
            end: 65.0,
            body: "پیگیری".into(),
            color: None,
            created_at: String::new(),
            updated_at: String::new(),
        };
        let md = render("md", "t", &segs, None, std::slice::from_ref(&note)).unwrap();
        assert!(md.contains("## Notes") && md.contains("**0:01–1:05** پیگیری"));
        assert!(render("json", "t", &segs, None, &[note])
            .unwrap()
            .contains("\"notes\""));
    }

    #[test]
    fn subtitle_formats() {
        let segs = vec![Segment {
            id: 0,
            start: 1.5,
            end: 3725.25,
            text: "سلام".into(),
            words: vec![],
            original_text: None,
        }];
        assert_eq!(srt(&segs), "1\n00:00:01,500 --> 01:02:05,250\nسلام\n\n");
        assert!(vtt(&segs).starts_with("WEBVTT\n\n00:00:01.500 --> 01:02:05.250"));
        assert_eq!(txt(&segs), "[0:01] سلام\n");
        assert!(render("md", "t", &segs, None, &[])
            .unwrap()
            .contains("**0:01** سلام"));
        assert!(render("docx", "t", &segs, None, &[]).is_none());
    }
}
