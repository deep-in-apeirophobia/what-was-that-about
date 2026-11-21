//! Persian text helpers: search normalization, spoken-number ITN, segment splitting.

use crate::types::{Segment, Word};

/// Normalize text for matching: unify Arabic/Persian letter variants, drop diacritics,
/// tatweel and ZWNJ, map Persian/Arabic digits to ASCII, lowercase Latin.
pub fn normalize_for_search(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            'ي' | 'ى' | 'ئ' => out.push('ی'),
            'ك' => out.push('ک'),
            'ة' => out.push('ه'),
            'أ' | 'إ' | 'ٱ' | 'آ' => out.push('ا'),
            'ؤ' => out.push('و'),
            '\u{064B}'..='\u{065F}'
            | '\u{0670}'
            | '\u{0640}'
            | '\u{200C}'
            | '\u{200D}'
            | '\u{200F}'
            | '\u{200E}' => {}
            '۰'..='۹' => out.push(char::from(b'0' + (c as u32 - '۰' as u32) as u8)),
            '٠'..='٩' => out.push(char::from(b'0' + (c as u32 - '٠' as u32) as u8)),
            _ => {
                for l in c.to_lowercase() {
                    out.push(l);
                }
            }
        }
    }
    out
}

/// Normalize and return, for every char of the normalized string, the UTF-16 offset of the
/// source char it came from (plus one trailing entry for the end). Used to map match
/// positions back to highlight ranges in the original text.
pub fn normalize_with_map(s: &str) -> (Vec<char>, Vec<u32>) {
    let mut chars = Vec::new();
    let mut map = Vec::new();
    let mut u16_pos = 0u32;
    for c in s.chars() {
        let n = normalize_for_search(c.encode_utf8(&mut [0u8; 4]));
        for nc in n.chars() {
            chars.push(nc);
            map.push(u16_pos);
        }
        u16_pos += c.len_utf16() as u32;
    }
    map.push(u16_pos);
    (chars, map)
}

/// Find all (case/diacritic-insensitive) occurrences of each term in `text`, returning
/// merged UTF-16 ranges into the original text.
pub fn find_highlights(text: &str, terms: &[String]) -> Vec<(u32, u32)> {
    let (chars, map) = normalize_with_map(text);
    let mut ranges = Vec::new();
    for t in terms {
        let tc: Vec<char> = normalize_for_search(t).chars().collect();
        if tc.is_empty() || tc.len() > chars.len() {
            continue;
        }
        let mut i = 0;
        while i + tc.len() <= chars.len() {
            if chars[i..i + tc.len()] == tc[..] {
                let start = map[i];
                let end = map[i + tc.len()].max(start + 1);
                // extend end to the next source char boundary
                let end = map.iter().copied().find(|&m| m >= end).unwrap_or(end);
                ranges.push((start, end));
                i += tc.len();
            } else {
                i += 1;
            }
        }
    }
    ranges.sort();
    let mut merged: Vec<(u32, u32)> = Vec::new();
    for r in ranges {
        if let Some(last) = merged.last_mut() {
            if r.0 <= last.1 {
                last.1 = last.1.max(r.1);
                continue;
            }
        }
        merged.push(r);
    }
    merged
}

pub fn search_terms(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(|t| t.trim_matches(|c: char| c.is_ascii_punctuation() || "،؛؟«»".contains(c)))
        .filter(|t| !t.is_empty())
        .map(String::from)
        .collect()
}

// ---- Persian inverse text normalization (port of Shenava's persian_itn.py)

fn unit(w: &str) -> Option<u64> {
    Some(match w {
        "صفر" => 0,
        "یک" => 1,
        "دو" => 2,
        "سه" => 3,
        "چهار" => 4,
        "پنج" => 5,
        "شش" | "شیش" => 6,
        "هفت" => 7,
        "هشت" => 8,
        "نه" => 9,
        "ده" => 10,
        "یازده" => 11,
        "دوازده" => 12,
        "سیزده" => 13,
        "چهارده" => 14,
        "پانزده" | "پونزده" => 15,
        "شانزده" | "شونزده" => 16,
        "هفده" | "هیفده" => 17,
        "هجده" | "هیجده" => 18,
        "نوزده" => 19,
        "بیست" => 20,
        "سی" => 30,
        "چهل" => 40,
        "پنجاه" => 50,
        "شصت" => 60,
        "هفتاد" => 70,
        "هشتاد" => 80,
        "نود" => 90,
        "صد" | "یکصد" => 100,
        "دویست" => 200,
        "سیصد" => 300,
        "چهارصد" => 400,
        "پانصد" | "پونصد" => 500,
        "ششصد" | "شیشصد" => 600,
        "هفتصد" => 700,
        "هشتصد" => 800,
        "نهصد" => 900,
        _ => return None,
    })
}

fn scale(w: &str) -> Option<u64> {
    Some(match w {
        "هزار" => 1_000,
        "میلیون" => 1_000_000,
        "میلیارد" => 1_000_000_000,
        _ => return None,
    })
}

fn is_num(w: &str) -> bool {
    unit(w).is_some() || scale(w).is_some()
}

fn to_persian_digits(n: u64) -> String {
    n.to_string()
        .chars()
        .map(|c| char::from_u32('۰' as u32 + c.to_digit(10).unwrap()).unwrap())
        .collect()
}

/// Convert spelled-out Persian numbers to Persian digits ("بیست و پنج" -> "۲۵").
/// Operates on whitespace-separated words, same rules as Shenava's reference script.
/// Returns, for each output word, the range of input words it covers.
pub fn itn_words(words: &[&str]) -> Vec<(String, std::ops::Range<usize>)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < words.len() {
        if is_num(words[i]) {
            let mut total = 0u64;
            let mut cur = 0u64;
            let mut j = i;
            let mut last_num = i;
            while j < words.len() && (is_num(words[j]) || words[j] == "و") {
                let w = words[j];
                if w == "و" {
                    // a trailing "و" is a conjunction, not part of the number
                    if j + 1 >= words.len() || !is_num(words[j + 1]) {
                        break;
                    }
                    j += 1;
                    continue;
                }
                if let Some(s) = scale(w) {
                    total += if cur > 0 { cur } else { 1 } * s;
                    cur = 0;
                } else {
                    cur += unit(w).unwrap_or(0);
                }
                last_num = j;
                j += 1;
            }
            total += cur;
            if last_num == i && words[i] == "یک" {
                // a lone "یک" is usually the article "a/one", not a number to digitize
                out.push((words[i].to_string(), i..i + 1));
            } else {
                out.push((to_persian_digits(total), i..last_num + 1));
            }
            i = last_num + 1;
        } else {
            out.push((words[i].to_string(), i..i + 1));
            i += 1;
        }
    }
    out
}

pub fn itn(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    itn_words(&words)
        .into_iter()
        .map(|(w, _)| w)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Apply ITN to a segment, keeping word timings aligned.
pub fn itn_segment(seg: &mut Segment) {
    if seg.words.is_empty() {
        seg.text = itn(&seg.text);
        return;
    }
    let texts: Vec<&str> = seg.words.iter().map(|w| w.text.as_str()).collect();
    let merged = itn_words(&texts);
    let words: Vec<Word> = merged
        .into_iter()
        .map(|(t, r)| Word {
            start: seg.words[r.start].start,
            end: seg.words[r.end - 1].end,
            text: t,
        })
        .collect();
    seg.text = words
        .iter()
        .map(|w| w.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    seg.words = words;
}

fn ends_sentence(word: &str) -> bool {
    word.ends_with(['.', '!', '?', '؟', '…', '۔'])
}

/// Split long segments into sentence-sized pieces using punctuation, falling back to
/// the largest pause between words. Segment ids are reassigned sequentially.
pub fn split_segments(segments: Vec<Segment>, max_secs: f64) -> Vec<Segment> {
    let mut out: Vec<Segment> = Vec::new();
    for seg in segments {
        if seg.words.len() < 2 {
            out.push(seg);
            continue;
        }
        // 1) punctuation
        let mut pieces: Vec<Vec<Word>> = vec![Vec::new()];
        for w in &seg.words {
            pieces.last_mut().unwrap().push(w.clone());
            if ends_sentence(&w.text) {
                pieces.push(Vec::new());
            }
        }
        pieces.retain(|p| !p.is_empty());
        // merge tiny fragments (< 1.2 s) into the previous piece
        let mut merged: Vec<Vec<Word>> = Vec::new();
        for p in pieces {
            let dur = p.last().unwrap().end - p[0].start;
            match merged.last_mut() {
                Some(prev) if dur < 1.2 || p.len() < 3 => prev.extend(p),
                _ => merged.push(p),
            }
        }
        // 2) long pieces: split at the biggest pause, recursively
        let mut final_pieces = Vec::new();
        let mut stack: Vec<Vec<Word>> = merged.into_iter().rev().collect();
        while let Some(p) = stack.pop() {
            let dur = p.last().unwrap().end - p[0].start;
            if dur <= max_secs || p.len() < 4 {
                final_pieces.push(p);
                continue;
            }
            // only consider cut points that leave >= 25% on each side
            let n = p.len();
            let lo = (n / 4).max(1);
            let hi = (n - n / 4).min(n - 1);
            let mut best = lo;
            let mut best_gap = f64::MIN;
            for k in lo..hi.max(lo + 1) {
                let gap = p[k].start - p[k - 1].end;
                if gap > best_gap {
                    best_gap = gap;
                    best = k;
                }
            }
            let (a, b) = p.split_at(best);
            stack.push(b.to_vec());
            stack.push(a.to_vec());
        }
        let n_pieces = final_pieces.len();
        for (k, p) in final_pieces.into_iter().enumerate() {
            let start = if k == 0 {
                seg.start.min(p[0].start)
            } else {
                p[0].start
            };
            let end = if k + 1 == n_pieces {
                seg.end.max(p.last().unwrap().end)
            } else {
                p.last().unwrap().end
            };
            out.push(Segment {
                id: 0,
                start,
                end,
                text: p
                    .iter()
                    .map(|w| w.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" "),
                words: p,
                original_text: None,
            });
        }
    }
    for (i, s) in out.iter_mut().enumerate() {
        s.id = i as u32;
    }
    out
}

/// Format seconds as H:MM:SS or M:SS.
pub fn fmt_time(secs: f64) -> String {
    let t = secs.max(0.0) as u64;
    let (h, m, s) = (t / 3600, (t % 3600) / 60, t % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_persian_variants() {
        assert_eq!(normalize_for_search("كتاب‌هاي"), "کتابهای");
        assert_eq!(normalize_for_search("۱۲۳ ABC"), "123 abc");
        assert_eq!(normalize_for_search("مَدرِسه"), "مدرسه");
    }

    #[test]
    fn highlights_map_back_to_original() {
        let text = "این كتاب‌ها خوب است";
        let h = find_highlights(text, &["کتابها".into()]);
        assert_eq!(h.len(), 1);
        let (s, e) = h[0];
        let utf16: Vec<u16> = text.encode_utf16().collect();
        let sub = String::from_utf16(&utf16[s as usize..e as usize]).unwrap();
        assert_eq!(sub, "كتاب‌ها");
    }

    #[test]
    fn itn_basic() {
        assert_eq!(itn("سال هزار و سیصد و نود و هشت"), "سال ۱۳۹۸");
        assert_eq!(itn("بیست و پنج نفر"), "۲۵ نفر");
        assert_eq!(itn("دو میلیون و پانصد هزار"), "۲۵۰۰۰۰۰");
        assert_eq!(itn("سه و نیم"), "۳ و نیم");
        assert_eq!(itn("سلام دنیا"), "سلام دنیا");
        assert_eq!(itn("یک بار دیگر"), "یک بار دیگر");
        assert_eq!(itn("یک میلیون"), "۱۰۰۰۰۰۰");
    }

    #[test]
    fn itn_keeps_word_timings() {
        let mut seg = Segment {
            id: 0,
            start: 0.0,
            end: 3.0,
            text: String::new(),
            words: vec![
                Word {
                    start: 0.0,
                    end: 0.5,
                    text: "بیست".into(),
                },
                Word {
                    start: 0.5,
                    end: 0.7,
                    text: "و".into(),
                },
                Word {
                    start: 0.7,
                    end: 1.0,
                    text: "پنج".into(),
                },
                Word {
                    start: 1.1,
                    end: 1.5,
                    text: "نفر".into(),
                },
            ],
            original_text: None,
        };
        itn_segment(&mut seg);
        assert_eq!(seg.text, "۲۵ نفر");
        assert_eq!(seg.words[0].start, 0.0);
        assert_eq!(seg.words[0].end, 1.0);
        assert_eq!(seg.words[1].text, "نفر");
    }

    fn w(start: f64, end: f64, t: &str) -> Word {
        Word {
            start,
            end,
            text: t.into(),
        }
    }

    #[test]
    fn splits_on_punctuation_and_pauses() {
        let seg = Segment {
            id: 0,
            start: 0.0,
            end: 6.0,
            text: String::new(),
            words: vec![
                w(0.0, 0.4, "سلام"),
                w(0.5, 0.9, "به"),
                w(1.0, 1.5, "همه."),
                w(2.0, 2.4, "امروز"),
                w(2.5, 2.9, "درباره"),
                w(3.0, 3.5, "مدل"),
                w(3.6, 4.0, "حرف"),
                w(4.1, 4.5, "می‌زنیم؟"),
            ],
            original_text: None,
        };
        let out = split_segments(vec![seg], 30.0);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].text, "سلام به همه.");
        assert_eq!(out[1].id, 1);
        assert_eq!(out[1].end, 6.0);

        let long = Segment {
            id: 0,
            start: 0.0,
            end: 20.0,
            text: String::new(),
            words: (0..20)
                .map(|i| {
                    let gap = if i == 10 { 1.0 } else { 0.0 };
                    w(i as f64 + gap * 0.0, i as f64 + 0.9 - gap * 0.5, "کلمه")
                })
                .collect(),
            original_text: None,
        };
        let out = split_segments(vec![long], 8.0);
        assert!(out.len() >= 3);
        assert!(out.iter().all(|s| s.end - s.start <= 10.5));
    }

    #[test]
    fn time_format() {
        assert_eq!(fmt_time(65.4), "1:05");
        assert_eq!(fmt_time(3725.0), "1:02:05");
    }
}
