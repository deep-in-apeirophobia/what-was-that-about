//! Fitting work into a model's context window: token estimates, splitting long transcripts
//! into sections for condensing, picking verbatim excerpts, and deciding which chat messages
//! to fold into a conversation summary.

use std::ops::Range;

use sha2::{Digest, Sha256};

use crate::types::Segment;

/// Context window assumed when OpenRouter doesn't report one.
pub const DEFAULT_CONTEXT_TOKENS: u64 = 128_000;

/// Length of one transcript section that gets condensed on its own. Time-based (not
/// token-based) so editing a sentence never moves section boundaries and cached digests
/// stay valid for the rest of the recording.
pub const SECTION_SECS: f64 = 600.0;

/// Sections denser than this (in estimated tokens) are split further.
pub const MAX_SECTION_TOKENS: usize = 12_000;

/// Digests of this many consecutive sections are condensed together at the next level.
pub const SECTIONS_PER_GROUP: usize = 6;

/// Rough token count, erring high. Persian/Arabic script costs about one token per
/// 1.5–2 characters in current tokenizers; Latin text, digits and brackets one per 3–4.
pub fn estimate_tokens(s: &str) -> usize {
    let (mut ascii, mut other) = (0usize, 0usize);
    for c in s.chars() {
        if c.is_ascii() {
            ascii += 1;
        } else {
            other += 1;
        }
    }
    ascii.div_ceil(3) + (other * 2).div_ceil(3)
}

/// Tokens available for the prompt once `answer_tokens` are reserved for the reply, with a
/// margin for estimation error.
pub fn input_budget(context_length: Option<u64>, answer_tokens: usize) -> usize {
    let ctx = context_length
        .filter(|&c| c > 0)
        .unwrap_or(DEFAULT_CONTEXT_TOKENS) as usize;
    ctx.saturating_sub(answer_tokens) * 85 / 100
}

/// Stable fingerprint of a condensing input, so cached digests are redone after edits.
pub fn fingerprint(s: &str) -> String {
    let h = Sha256::digest(s.as_bytes());
    h.iter().take(12).map(|b| format!("{b:02x}")).collect()
}

/// Split a transcript into consecutive sections of [`SECTION_SECS`] of audio (by segment
/// start time), splitting any section whose lines exceed `max_tokens`. Returns index ranges
/// into `segs`; `line_tokens[i]` is the token estimate of segment `i`'s transcript line.
pub fn sections(segs: &[Segment], line_tokens: &[usize], max_tokens: usize) -> Vec<Range<usize>> {
    let mut windows: Vec<Range<usize>> = Vec::new();
    let mut start = 0;
    for i in 1..segs.len() {
        let a = (segs[start].start / SECTION_SECS).floor();
        let b = (segs[i].start / SECTION_SECS).floor();
        if b > a {
            windows.push(start..i);
            start = i;
        }
    }
    if !segs.is_empty() {
        windows.push(start..segs.len());
    }
    let max_tokens = max_tokens.max(1);
    let mut out = Vec::with_capacity(windows.len());
    for w in windows {
        let total: usize = line_tokens[w.clone()].iter().sum();
        if total <= max_tokens || w.len() < 2 {
            out.push(w);
            continue;
        }
        // split evenly by tokens
        let parts = total.div_ceil(max_tokens);
        let per = total.div_ceil(parts);
        let (mut s, mut acc) = (w.start, 0usize);
        for i in w.clone() {
            if acc > 0 && acc + line_tokens[i] > per {
                out.push(s..i);
                s = i;
                acc = 0;
            }
            acc += line_tokens[i];
        }
        out.push(s..w.end);
    }
    out
}

/// Pick transcript lines to quote verbatim: each hit (index into the transcript, best first)
/// with `radius` neighbours on both sides, until `budget` tokens are used. Returns sorted
/// indices.
pub fn select_excerpts(
    line_tokens: &[usize],
    hits: &[usize],
    radius: usize,
    budget: usize,
) -> Vec<usize> {
    let n = line_tokens.len();
    let mut taken = vec![false; n];
    let mut used = 0usize;
    'hits: for &h in hits {
        if h >= n {
            continue;
        }
        // the hit itself first, then its neighbours outwards
        let mut order = vec![h];
        for d in 1..=radius {
            if h >= d {
                order.push(h - d);
            }
            if h + d < n {
                order.push(h + d);
            }
        }
        for i in order {
            if taken[i] {
                continue;
            }
            if used + line_tokens[i] > budget {
                break 'hits;
            }
            taken[i] = true;
            used += line_tokens[i];
        }
    }
    (0..n).filter(|&i| taken[i]).collect()
}

/// Number of tokens a message would cost in the prompt.
#[derive(Debug, Clone, Copy)]
pub struct MsgCost {
    pub is_user: bool,
    pub tokens: usize,
}

/// How many of the oldest messages to fold into the conversation summary so the rest fit in
/// `budget` tokens. Folds down to about half the budget (so it doesn't happen every turn),
/// keeps at least `keep_last` messages, and never leaves an answer without its question.
/// Returns 0 when everything fits.
pub fn messages_to_fold(msgs: &[MsgCost], budget: usize, keep_last: usize) -> usize {
    let total: usize = msgs.iter().map(|m| m.tokens).sum();
    if total <= budget {
        return 0;
    }
    let max_fold = msgs.len().saturating_sub(keep_last);
    let target = budget / 2;
    let (mut n, mut rest) = (0usize, total);
    while n < max_fold && rest > target {
        rest -= msgs[n].tokens;
        n += 1;
    }
    // the kept history must start with a question
    while n < msgs.len() && !msgs[n].is_user {
        n += 1;
    }
    n
}

/// Shorten text to roughly `max_tokens`, keeping the start and the end.
pub fn clip_middle(s: &str, max_tokens: usize) -> String {
    let t = estimate_tokens(s);
    if t <= max_tokens {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let keep = (chars.len() * max_tokens / t.max(1)).max(2);
    let head = keep * 2 / 3;
    let tail = keep - head;
    let mut out: String = chars[..head].iter().collect();
    out.push_str("\n[…]\n");
    out.extend(&chars[chars.len() - tail..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(id: u32, start: f64, text: &str) -> Segment {
        Segment {
            id,
            start,
            end: start + 5.0,
            text: text.into(),
            words: vec![],
        }
    }

    #[test]
    fn estimates_persian_higher_than_latin() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcdef"), 2);
        // 6 Persian letters ≈ 4 tokens
        assert_eq!(estimate_tokens("سلامبر"), 4);
        // a one-hour transcript (~50k Persian chars + line prefixes) lands near 40k tokens
        let line = format!("[123] (12:34) {}\n", "س".repeat(50));
        let hour: String = line.repeat(1000);
        let t = estimate_tokens(&hour);
        assert!((35_000..45_000).contains(&t), "{t}");
    }

    #[test]
    fn budget_reserves_answer_and_margin() {
        assert_eq!(input_budget(Some(100_000), 4_000), 81_600);
        assert_eq!(input_budget(None, 0), 108_800);
        assert_eq!(input_budget(Some(0), 0), 108_800);
        assert_eq!(input_budget(Some(1_000), 4_000), 0);
    }

    #[test]
    fn sections_follow_time_and_split_dense_windows() {
        let segs: Vec<Segment> = (0..10).map(|i| seg(i, i as f64 * 200.0, "x")).collect();
        let toks = vec![10; 10];
        // starts 0,200,400 | 600,800,1000 | 1200,1400,1600 | 1800
        assert_eq!(sections(&segs, &toks, 1_000), vec![0..3, 3..6, 6..9, 9..10]);
        // a dense window splits evenly
        let dense: Vec<Segment> = (0..6).map(|i| seg(i, i as f64, "x")).collect();
        assert_eq!(sections(&dense, &[10; 6], 25), vec![0..2, 2..4, 4..6]);
        assert!(sections(&[], &[], 10).is_empty());
    }

    #[test]
    fn sections_survive_text_edits() {
        let segs: Vec<Segment> = (0..30).map(|i| seg(i, i as f64 * 60.0, "x")).collect();
        let before = sections(&segs, &[50; 30], MAX_SECTION_TOKENS);
        let mut toks = vec![50; 30];
        toks[4] = 400; // one sentence got much longer
        assert_eq!(sections(&segs, &toks, MAX_SECTION_TOKENS), before);
    }

    #[test]
    fn excerpts_take_hits_with_neighbours_until_full() {
        let toks = vec![10; 20];
        assert_eq!(select_excerpts(&toks, &[5], 1, 1_000), vec![4, 5, 6]);
        // the second hit only partly fits: its centre comes first
        assert_eq!(select_excerpts(&toks, &[5, 15], 1, 40), vec![4, 5, 6, 15]);
        // overlapping neighbourhoods aren't double counted; out-of-range hits are ignored
        assert_eq!(select_excerpts(&toks, &[0, 1, 99], 1, 1_000), vec![0, 1, 2]);
        assert!(select_excerpts(&toks, &[3], 2, 5).is_empty());
    }

    #[test]
    fn folds_oldest_messages_in_pairs() {
        let m = |is_user, tokens| MsgCost { is_user, tokens };
        let msgs = vec![
            m(true, 100),
            m(false, 400),
            m(true, 100),
            m(false, 400),
            m(true, 100),
            m(false, 400),
        ];
        assert_eq!(messages_to_fold(&msgs, 2_000, 2), 0);
        // over budget: fold until ≤ half the budget remains, ending on a question boundary
        assert_eq!(messages_to_fold(&msgs, 1_200, 2), 4);
        // never folds the last `keep_last` messages
        assert_eq!(messages_to_fold(&msgs, 10, 2), 4);
        // an orphan answer at the cut is folded with its question
        let odd = vec![m(true, 500), m(false, 500), m(false, 10), m(true, 10)];
        assert_eq!(messages_to_fold(&odd, 600, 1), 3);
    }

    #[test]
    fn clips_long_text_in_the_middle() {
        assert_eq!(clip_middle("short", 100), "short");
        let long = "a".repeat(3_000);
        let c = clip_middle(&long, 100);
        assert!(c.contains("[…]"));
        assert!(estimate_tokens(&c) <= 110);
    }

    #[test]
    fn fingerprints_are_stable() {
        assert_eq!(fingerprint("abc"), fingerprint("abc"));
        assert_ne!(fingerprint("abc"), fingerprint("abd"));
        assert_eq!(fingerprint("abc").len(), 24);
    }
}
