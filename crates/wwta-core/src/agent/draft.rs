//! The in-memory doc file, bounded reads, and forgiving single-span edits.

use std::sync::LazyLock;

use regex::Regex;

/// Soft limit the docs guide asks for; a warning past this.
pub const SOFT_MAX_BYTES: usize = 60_000;
/// Edits and writes that would make the draft bigger than this are refused.
pub const HARD_MAX_BYTES: usize = 200_000;
/// Lines per `read_draft` call.
pub const READ_LINES: usize = 200;
/// Bytes per `read_draft` call.
pub const READ_CHARS: usize = 20_000;
const LINE_CHARS: usize = 2_000;
const NO_DRAFT: &str = "there is no draft yet. Write the whole file first with write_draft.";

#[derive(Debug, Default)]
pub struct Draft {
    html: String,
    /// Changed since it was loaded (a revision starts clean).
    pub dirty: bool,
    pub finished: bool,
    pub writes: u32,
    pub edits: u32,
    pub failed_edits: u32,
    failed_in_a_row: u32,
    pub reads: u32,
    pub finish_attempts: u32,
}

#[derive(Debug, Default)]
pub struct Check {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

fn kb(bytes: usize) -> usize {
    (bytes + 512) / 1024
}

fn too_big(bytes: usize) -> String {
    format!("that would make the draft {} KB; keep docs under about 60 KB (don't embed the transcript; read it from WWTA.segments).", kb(bytes))
}

fn numbered(n: usize, line: &str) -> String {
    let text: String = line.chars().take(LINE_CHARS).collect();
    let cut = if text.len() < line.len() { "…" } else { "" };
    format!("{n}: {text}{cut}\n")
}

impl Draft {
    /// Load an existing doc for revision, without marking it changed.
    pub fn load(html: &str) -> Self {
        Self {
            html: html.replace("\r\n", "\n"),
            ..Self::default()
        }
    }

    /// Whether a file has been written or loaded.
    pub fn is_empty(&self) -> bool {
        self.html.is_empty()
    }

    /// The current file, without line numbers.
    pub fn html(&self) -> &str {
        &self.html
    }

    /// Line count and rounded size in KB.
    pub fn summary(&self) -> String {
        format!(
            "{} lines, {} KB",
            self.html.lines().count(),
            kb(self.html.len())
        )
    }

    /// Save the whole file, accepting incomplete skeletons while they are being built.
    pub fn write(&mut self, html: &str) -> Result<String, String> {
        let normalized = html.replace("\r\n", "\n");
        let mut html = normalized.trim();
        if let Some((fence, rest)) = html.split_once('\n') {
            if matches!(fence.trim(), "```html" | "```") {
                if let Some(body) = rest.strip_suffix("\n```") {
                    html = body.trim();
                }
            }
        }
        if html.is_empty() {
            return Err("`html` is empty.".into());
        }
        if !html.contains('<') {
            return Err(
                "`html` doesn't look like an HTML document; start with <!doctype html>.".into(),
            );
        }
        if html.len() > HARD_MAX_BYTES {
            return Err(too_big(html.len()));
        }
        let previous = (!self.is_empty()).then(|| self.summary());
        self.html = html.to_string();
        self.writes += 1;
        self.dirty = true;
        self.finished = false;
        self.failed_in_a_row = 0;
        let mut out = match previous {
            Some(previous) => format!("Draft replaced (was {previous}): now {}.", self.summary()),
            None => format!("Draft saved: {}.", self.summary()),
        };
        out.push_str(" Add content with edit_draft; call finish_doc when it's complete.");
        let check = self.check();
        if !check.errors.is_empty() {
            out.push_str(&format!("\nNote: {}", check.errors.join("; ")));
        }
        Ok(out)
    }

    /// Read a bounded, numbered window of 1-based inclusive lines.
    pub fn read(&mut self, from: usize, to: usize) -> Result<String, String> {
        if self.is_empty() {
            return Err(NO_DRAFT.into());
        }
        let lines: Vec<&str> = self.html.lines().collect();
        let from = from.max(1);
        if from > lines.len() {
            return Err(format!("the draft has {} lines.", lines.len()));
        }
        let limit = from.saturating_add(READ_LINES - 1);
        let to = if to < from { limit } else { to.min(limit) }.min(lines.len());
        let mut body = String::new();
        let mut end = from;
        for n in from..=to {
            let line = numbered(n, lines[n - 1]);
            // Leave room for the header and continuation footer.
            if body.len() + line.len() + 200 > READ_CHARS {
                break;
            }
            body.push_str(&line);
            end = n;
        }
        self.reads += 1;
        let footer = if end == lines.len() {
            format!("(End of the draft: {} lines.)", lines.len())
        } else {
            format!("(Continue with read_draft from {}.)", end + 1)
        };
        Ok(format!(
            "Lines {from}–{end} of {} ({} KB):\n{body}{footer}",
            lines.len(),
            kb(self.html.len())
        ))
    }

    /// Replace one unique span, accepting copied line numbers and common text differences.
    pub fn edit(&mut self, old: &str, new: &str) -> Result<String, String> {
        let old = strip_numbers(old);
        let new = strip_numbers(new);
        let result = self.edit_inner(&old, &new);
        if let Err(mut error) = result {
            self.failed_edits += 1;
            self.failed_in_a_row += 1;
            if self.failed_in_a_row >= 2 {
                error.push_str("\nTip: read the lines you want to change with read_draft and copy them exactly.");
            }
            return Err(error);
        }
        result
    }

    fn edit_inner(&mut self, old: &str, new: &str) -> Result<String, String> {
        if self.is_empty() {
            return Err(NO_DRAFT.into());
        }
        if old.trim().is_empty() {
            return Err(
                "`old` is empty. Copy a few unique lines from the draft to replace.".into(),
            );
        }
        if old == new {
            return Err("`old` and `new` are the same.".into());
        }
        let (start, end) = find_unique(&self.html, old).map_err(|error| match error {
            EditError::NotFound => {
                let mut out = "could not find `old` in the draft. It must match the draft text exactly (copy it from read_draft, without the line numbers).".to_string();
                let hint = closest_lines(&self.html, old);
                if !hint.is_empty() {
                    out.push_str(&format!("\nThe closest lines in the draft:\n{hint}"));
                }
                if !new.trim().is_empty() && self.html.contains(new) {
                    out.push_str("\n`new` is already in the draft, so this edit may already be applied.");
                }
                out
            }
            EditError::Several(lines) => {
                let mut shown = lines.iter().take(5).map(usize::to_string).collect::<Vec<_>>();
                if lines.len() > 5 {
                    shown.push("…".into());
                }
                format!("`old` appears {} times in the draft (lines {}). Include more surrounding lines so it matches only once.", lines.len(), shown.join(", "))
            }
        })?;
        let size = self.html.len() - (end - start) + new.len();
        if size > HARD_MAX_BYTES {
            return Err(too_big(size));
        }
        let first = line_at(&self.html, start);
        let last = line_at(&self.html, end.saturating_sub(1));
        self.html.replace_range(start..end, new);
        self.edits += 1;
        self.dirty = true;
        self.finished = false;
        self.failed_in_a_row = 0;
        Ok(format!(
            "Edited the draft: lines {first}–{last} replaced by {} lines. Draft: {}.",
            new.lines().count(),
            self.summary()
        ))
    }

    /// Check completeness and common sandbox or script mistakes.
    pub fn check(&self) -> Check {
        check_html(&self.html)
    }

    /// Finish a complete file; warnings block only the first attempt.
    pub fn finish(&mut self) -> Result<String, String> {
        self.finish_attempts += 1;
        let check = self.check();
        if !check.errors.is_empty() {
            return Err(format!("the draft isn't a complete doc yet:\n- {}\nFix it with edit_draft (or write_draft) and call finish_doc again.", check.errors.join("\n- ")));
        }
        if !check.warnings.is_empty() && self.finish_attempts == 1 {
            return Err(format!("check these before finishing:\n- {}\nFix them with edit_draft, or call finish_doc again to finish anyway.", check.warnings.join("\n- ")));
        }
        self.finished = true;
        Ok(format!("Doc finished: {}.", self.summary()))
    }
}

#[derive(Debug, PartialEq)]
enum EditError {
    NotFound,
    Several(Vec<usize>),
}

fn line_at(text: &str, byte: usize) -> usize {
    text.as_bytes()[..byte]
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
        + 1
}

fn strip_numbers(text: &str) -> String {
    static PREFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*\d+:\s?").unwrap());
    if !text.lines().any(|l| !l.trim().is_empty())
        || !text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .all(|l| PREFIX.is_match(l))
    {
        return text.to_string();
    }
    text.split('\n')
        .map(|l| PREFIX.replace(l, "").into_owned())
        .collect::<Vec<_>>()
        .join("\n")
}

fn normalize(line: &str, tier: u8) -> String {
    match tier {
        2 => line.trim_end().to_string(),
        3 => line.trim().to_string(),
        _ => {
            let mapped: String = line
                .chars()
                .filter_map(|c| {
                    Some(match c {
                        '\u{200c}' | '\u{200b}' | '\u{feff}' => return None,
                        '‘' | '’' => '\'',
                        '“' | '”' => '"',
                        '–' | '—' => '-',
                        'ي' => 'ی',
                        'ك' => 'ک',
                        '۰'..='۹' => char::from_u32('0' as u32 + c as u32 - '۰' as u32).unwrap(),
                        '٠'..='٩' => char::from_u32('0' as u32 + c as u32 - '٠' as u32).unwrap(),
                        _ => c,
                    })
                })
                .collect();
            mapped.split_whitespace().collect::<Vec<_>>().join(" ")
        }
    }
}

fn find_unique(text: &str, old: &str) -> Result<(usize, usize), EditError> {
    let old = strip_numbers(old);
    if old.trim().is_empty() {
        return Err(EditError::NotFound);
    }
    let decide = |matches: Vec<(usize, usize)>| {
        if matches.len() == 1 {
            Ok(matches[0])
        } else {
            Err(EditError::Several(
                matches.iter().map(|&(s, _)| line_at(text, s)).collect(),
            ))
        }
    };
    let exact: Vec<_> = text
        .match_indices(&old)
        .map(|(i, m)| (i, i + m.len()))
        .collect();
    if !exact.is_empty() {
        return decide(exact);
    }
    let mut offset = 0;
    let lines: Vec<_> = text
        .split_inclusive('\n')
        .map(|line| {
            let start = offset;
            offset += line.len();
            let line = line.strip_suffix('\n').unwrap_or(line);
            (start, start + line.len(), line)
        })
        .collect();
    for tier in 2..=4 {
        let mut wanted: Vec<_> = old.lines().map(|l| normalize(l, tier)).collect();
        while wanted.first().is_some_and(String::is_empty) {
            wanted.remove(0);
        }
        while wanted.last().is_some_and(String::is_empty) {
            wanted.pop();
        }
        if wanted.is_empty() {
            continue;
        }
        let normalized: Vec<_> = lines.iter().map(|l| normalize(l.2, tier)).collect();
        let matches = normalized
            .windows(wanted.len())
            .enumerate()
            .filter(|(_, window)| *window == wanted)
            .map(|(i, _)| (lines[i].0, lines[i + wanted.len() - 1].1))
            .collect::<Vec<_>>();
        if !matches.is_empty() {
            return decide(matches);
        }
    }
    Err(EditError::NotFound)
}

fn closest_lines(text: &str, old: &str) -> String {
    let Some(first) = old.lines().map(|l| normalize(l, 4)).find(|l| !l.is_empty()) else {
        return String::new();
    };
    let probe: String = first.chars().take(20).collect();
    let collect = |probe: &str| {
        text.lines()
            .enumerate()
            .filter(|(_, line)| normalize(line, 4).contains(probe))
            .take(6)
            .map(|(i, line)| numbered(i + 1, line))
            .collect::<String>()
    };
    let hint = collect(&probe);
    if !hint.is_empty() {
        return hint;
    }
    first
        .split_whitespace()
        .filter(|s| s.chars().count() >= 4)
        .max_by_key(|s| s.chars().count())
        .map(collect)
        .unwrap_or_default()
}

/// Check a draft or an HTML answer before it is saved.
pub fn check_html(html: &str) -> Check {
    let html = html.trim();
    let lower = html.to_ascii_lowercase();
    let mut check = Check::default();
    if html.is_empty() {
        check.errors.push("the draft is empty".into());
    }
    if !lower.starts_with("<!doctype") && !lower.contains("<html") {
        check
            .errors
            .push("it doesn't start with <!doctype html>".into());
    }
    if !lower.ends_with("</html>") {
        check
            .errors
            .push("it doesn't end with </html> (is the file cut off?)".into());
    }
    if html.len() > HARD_MAX_BYTES {
        check
            .errors
            .push(format!("it is {} KB; the limit is 200 KB", kb(html.len())));
    }
    if html.len() > SOFT_MAX_BYTES {
        check.warnings.push(format!("it is {} KB; keep docs under about 60 KB (don't embed the transcript; read it from WWTA.segments)", kb(html.len())));
    }
    if !html.contains("WWTA.") {
        check.warnings.push("it never uses the WWTA bridge (WWTA.segments, WWTA.seek), so nothing links to the audio".into());
    }
    for needle in [
        "<script src",
        "<link ",
        "<iframe",
        "<form",
        "@import",
        "url(http",
        "fetch(",
        "XMLHttpRequest",
        "localStorage",
        "sessionStorage",
        "navigator.clipboard",
        "window.open(",
    ] {
        if let Some((i, _)) = lower
            .lines()
            .enumerate()
            .find(|(_, l)| l.contains(&needle.to_ascii_lowercase()))
        {
            check.warnings.push(format!("line {}: `{needle}` — external resources, storage, clipboard and pop-ups are blocked in docs", i + 1));
        }
    }
    static SCRIPTS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?is)<script\b[^>]*>(.*?)</script\s*>").unwrap());
    for captures in SCRIPTS.captures_iter(html) {
        let (braces, brackets) = script_balance(&captures[1]);
        for (balance, open, close) in [(braces, '{', '}'), (brackets, '[', ']')] {
            if balance != 0 {
                let (more, less) = if balance > 0 {
                    (open, close)
                } else {
                    (close, open)
                };
                check.warnings.push(format!(
                    "the script has {} more \"{more}\" than \"{less}\"",
                    balance.unsigned_abs()
                ));
            }
        }
    }
    if html.contains("```") {
        check
            .warnings
            .push("it contains a markdown fence (```)".into());
    }
    check
}

fn script_balance(script: &str) -> (i32, i32) {
    let mut chars = script.chars().peekable();
    let mut quote = None;
    let mut escaped = false;
    let mut line_comment = false;
    let mut block_comment = false;
    let (mut braces, mut brackets) = (0, 0);
    while let Some(c) = chars.next() {
        if line_comment {
            if c == '\n' {
                line_comment = false;
            }
        } else if block_comment {
            if c == '*' && chars.peek() == Some(&'/') {
                chars.next();
                block_comment = false;
            }
        } else if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == q {
                quote = None;
            }
        } else if matches!(c, '\'' | '"' | '`') {
            quote = Some(c);
        } else if c == '/' && chars.peek() == Some(&'/') {
            chars.next();
            line_comment = true;
        } else if c == '/' && chars.peek() == Some(&'*') {
            chars.next();
            block_comment = true;
        } else {
            match c {
                '{' => braces += 1,
                '}' => braces -= 1,
                '[' => brackets += 1,
                ']' => brackets -= 1,
                _ => {}
            }
        }
    }
    (braces, brackets)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "<!doctype html>\n<html>\n<title>Quiz</title>\n<script>const x = WWTA.segments;</script>\n</html>";

    #[test]
    fn strips_fences_and_normalizes_newlines() {
        let mut d = Draft::default();
        assert!(d
            .write(&format!("```html\r\n{}\r\n```", DOC.replace('\n', "\r\n")))
            .unwrap()
            .starts_with("Draft saved:"));
        assert_eq!(d.html(), DOC);
        assert!(d.dirty);
        assert!(!Draft::load(&DOC.replace('\n', "\r\n")).dirty);
    }

    #[test]
    fn refuses_empty_and_huge_writes() {
        let mut d = Draft::default();
        assert_eq!(d.write(" ").unwrap_err(), "`html` is empty.");
        assert!(d.write("hello").unwrap_err().contains("doesn't look like"));
        assert!(d
            .write(&format!("<{}", "x".repeat(201_000)))
            .unwrap_err()
            .contains("keep docs under about 60 KB"));
        assert!(d.is_empty());
    }

    #[test]
    fn reads_in_numbered_windows() {
        let html = (1..=312)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut d = Draft::load(&html);
        let out = d.read(1, 500).unwrap();
        assert!(out.starts_with("Lines 1–200 of 312"));
        assert!(out.contains("200: line 200\n"));
        assert!(out.ends_with("(Continue with read_draft from 201.)"));
        assert!(d
            .read(301, 400)
            .unwrap()
            .ends_with("(End of the draft: 312 lines.)"));
        assert_eq!(d.read(400, 401).unwrap_err(), "the draft has 312 lines.");
        let mut long = Draft::load(&"ش".repeat(5_000));
        assert!(long.read(0, 0).unwrap().contains("…"));
        let mut many = Draft::load(&vec!["ش".repeat(2_000); 100].join("\n"));
        let out = many.read(1, 200).unwrap();
        assert!(out.len() <= READ_CHARS);
        assert!(out.ends_with("(Continue with read_draft from 5.)"));
        assert_eq!(d.reads, 2);
    }

    #[test]
    fn edits_exactly_once() {
        let mut d = Draft::load(DOC);
        assert!(d
            .edit("<title>Quiz</title>", "<title>New</title>\n<p>Hi</p>")
            .unwrap()
            .starts_with("Edited the draft: lines 3–3 replaced by 2 lines."));
        assert!(d.html().contains("<title>New</title>\n<p>Hi</p>"));
        assert_eq!(d.edits, 1);
        assert!(d.dirty);
    }

    #[test]
    fn matches_with_lenient_whitespace() {
        for old in [
            "  const A = 1;   \n  const B = 2; ",
            "const A = 1;\nconst B = 2;",
        ] {
            let mut d = Draft::load("start\n  const A = 1;\n  const B = 2;\nend");
            d.edit(old, "new").unwrap();
            assert_eq!(d.html(), "start\nnew\nend");
        }
        let mut d = Draft::load("start\n  می‌رود “كي” ۱۲ ٣ —\nend");
        d.edit("میرود \"کی\" 12 3 -", "new").unwrap();
        assert_eq!(d.html(), "start\nnew\nend");
        assert_eq!(
            find_unique("same\n same", "same"),
            Err(EditError::Several(vec![1, 2]))
        );
        assert_eq!(find_unique(" exact \nexact", " exact "), Ok((0, 7)));
    }

    #[test]
    fn strips_line_numbers_copied_from_read_draft() {
        let mut d = Draft::load("const A = 1;\nconst B = 2;");
        d.edit("12: const A = 1;\n13: const B = 2;", "12: const C = 3;")
            .unwrap();
        assert_eq!(d.html(), "const C = 3;");
    }

    #[test]
    fn rejects_ambiguous_and_missing_edits() {
        let mut d = Draft::load("// end: items\nconst memorable = 1;\n// end: items");
        assert!(d
            .edit("// end: items", "x")
            .unwrap_err()
            .contains("appears 2 times in the draft (lines 1, 3)"));
        let err = d
            .edit("const memorable = 2;", "const memorable = 1;")
            .unwrap_err();
        assert!(err.contains("could not find"));
        assert!(err.contains("2: const memorable = 1;"));
        assert!(err.contains("already in the draft"));
        assert!(err.contains("Tip: read the lines"));
        assert_eq!(d.failed_edits, 2);
        d.edit("const memorable = 1;", "hello").unwrap();
        assert!(!d.edit("missing", "bye").unwrap_err().contains("Tip:"));
    }

    #[test]
    fn checks_completeness() {
        assert!(!check_html("<!doctype html><html>").errors.is_empty());
        let c = check_html("<!doctype html>\n<html><script src='a'>const x = [;</script></html>");
        assert!(c.errors.is_empty());
        assert!(c
            .warnings
            .iter()
            .any(|s| s.contains("line 2: `<script src`")));
        assert!(c
            .warnings
            .iter()
            .any(|s| s.contains("1 more \"[\" than \"]\"")));
        assert!(c.warnings.iter().any(|s| s.contains("WWTA bridge")));
        assert!(
            check_html(&format!("<!doctype html>{}</html>", "x".repeat(70_000)))
                .warnings
                .iter()
                .any(|s| s.contains("keep docs under"))
        );
        let c = check_html("<html><script>let a = '[]{}'; let b = \"\\\"[\"; let c = `}`; // [\n/* { */ WWTA.seek(0);</script></html>");
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
    }

    #[test]
    fn finish_gates_on_errors_then_warnings() {
        let mut d = Draft::load("<html>");
        assert!(d.finish().unwrap_err().contains("isn't a complete doc"));
        let mut d = Draft::load("<html></html>");
        assert!(d.finish().unwrap_err().contains("check these"));
        assert!(d.finish().unwrap().starts_with("Doc finished:"));
        assert!(d.finished);
        d.edit("<html>", "<!doctype html><html>").unwrap();
        assert!(!d.finished);
        d.finish().unwrap();
        assert!(d.finished);
        assert_eq!(d.finish_attempts, 3);
    }
}
