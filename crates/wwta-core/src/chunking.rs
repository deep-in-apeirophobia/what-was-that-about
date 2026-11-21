//! Engine-independent helpers: grouping VAD speech regions into decoder chunks and
//! assembling word timings from sub-word tokens.

use crate::types::Word;

/// Merge sorted speech regions (sample ranges) into chunks of at most `max_len` samples,
/// then pad each chunk by `pad` samples without overlapping its neighbours.
pub fn merge_regions(
    regions: &[(usize, usize)],
    total: usize,
    max_len: usize,
    pad: usize,
) -> Vec<(usize, usize)> {
    let mut split: Vec<(usize, usize)> = Vec::new();
    for &(s, e) in regions {
        let e = e.min(total);
        if e <= s {
            continue;
        }
        if e - s <= max_len {
            split.push((s, e));
        } else {
            let n = (e - s).div_ceil(max_len);
            let step = (e - s).div_ceil(n);
            let mut a = s;
            while a < e {
                split.push((a, (a + step).min(e)));
                a += step;
            }
        }
    }
    let mut chunks: Vec<(usize, usize)> = Vec::new();
    for (s, e) in split {
        match chunks.last_mut() {
            Some(c) if e - c.0 <= max_len => c.1 = e,
            _ => chunks.push((s, e)),
        }
    }
    pad_regions(&mut chunks, total, pad);
    chunks
}

/// Pad sorted, non-overlapping ranges by up to `pad` on each side; neighbours split the
/// gap between them so padded ranges never overlap.
pub fn pad_regions(ranges: &mut [(usize, usize)], total: usize, pad: usize) {
    let orig = ranges.to_vec();
    let n = orig.len();
    for i in 0..n {
        let lo = if i == 0 {
            0
        } else {
            (orig[i - 1].1 + orig[i].0) / 2
        };
        let hi = if i + 1 == n {
            total
        } else {
            (orig[i].1 + orig[i + 1].0) / 2
        };
        ranges[i].0 = orig[i].0.saturating_sub(pad).max(lo);
        ranges[i].1 = (orig[i].1 + pad).min(hi).min(total);
    }
}

/// SentencePiece pieces ("▁" marks a word start) with per-token start times -> words.
pub fn words_from_pieces(
    tokens: &[String],
    times: &[f32],
    offset: f64,
    chunk_end: f64,
) -> Vec<Word> {
    let mut words: Vec<(String, f64)> = Vec::new();
    for (i, tok) in tokens.iter().enumerate() {
        let t = offset + times.get(i).copied().unwrap_or(0.0) as f64;
        let starts_word = tok.starts_with('\u{2581}') || tok.starts_with(' ');
        let piece = tok.trim_start_matches(['\u{2581}', ' ']);
        if starts_word || words.is_empty() {
            if piece.is_empty() && !starts_word {
                continue;
            }
            words.push((piece.to_string(), t));
        } else if let Some(last) = words.last_mut() {
            last.0.push_str(piece);
        }
    }
    let words: Vec<(String, f64)> = words
        .into_iter()
        .filter(|(w, _)| !w.trim().is_empty())
        .collect();
    let n = words.len();
    words
        .iter()
        .enumerate()
        .map(|(i, (w, start))| {
            let next = if i + 1 < n { words[i + 1].1 } else { chunk_end };
            // CTC only gives onsets; estimate the spoken length from the word's size (~14
            // chars/s) so real pauses show up as gaps between words.
            let est = 0.15 + 0.07 * w.chars().count() as f64;
            let end = next.min(start + est).max(start + 0.04);
            Word {
                start: *start,
                end,
                text: w.clone(),
            }
        })
        .collect()
}

/// Byte-level BPE tokens (Whisper) with start/end times -> words. A token whose bytes
/// begin with a space starts a new word; multi-byte UTF-8 may span tokens.
pub fn words_from_byte_tokens(tokens: &[(Vec<u8>, f64, f64)]) -> Vec<Word> {
    let mut acc: Vec<(Vec<u8>, f64, f64)> = Vec::new();
    for (bytes, s, e) in tokens {
        if bytes.is_empty() {
            continue;
        }
        if bytes[0] == b' ' || acc.is_empty() {
            acc.push((bytes.clone(), *s, *e));
        } else if let Some(last) = acc.last_mut() {
            last.0.extend_from_slice(bytes);
            last.2 = last.2.max(*e);
        }
    }
    acc.into_iter()
        .filter_map(|(b, s, e)| {
            let t = String::from_utf8_lossy(&b).trim().to_string();
            (!t.is_empty()).then_some(Word {
                start: s,
                end: e.max(s),
                text: t,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_and_pads() {
        let r = vec![(100, 200), (250, 400), (1000, 1100)];
        let c = merge_regions(&r, 2000, 500, 20);
        assert_eq!(c, vec![(80, 420), (980, 1120)]);
        // close neighbours split the gap instead of overlapping
        let c = merge_regions(&[(100, 200), (210, 300)], 400, 100, 50);
        assert_eq!(c, vec![(50, 205), (205, 350)]);
    }

    #[test]
    fn splits_overlong_regions() {
        let c = merge_regions(&[(0, 1000)], 1000, 300, 0);
        assert!(c.iter().all(|(s, e)| e - s <= 300));
        assert_eq!(c.first().unwrap().0, 0);
        assert_eq!(c.last().unwrap().1, 1000);
    }

    #[test]
    fn pieces_to_words() {
        let toks: Vec<String> = ["▁سل", "ام", "▁دن", "یا"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let w = words_from_pieces(&toks, &[0.0, 0.08, 0.4, 0.48], 10.0, 11.0);
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].text, "سلام");
        assert_eq!(w[0].start, 10.0);
        assert!((w[0].end - 10.4).abs() < 1e-6); // capped by the next onset
        assert_eq!(w[1].text, "دنیا");
        assert!(
            (w[1].end - (10.4 + 0.15 + 0.28)).abs() < 1e-6,
            "{}",
            w[1].end
        ); // estimated from length
    }

    #[test]
    fn byte_tokens_to_words() {
        let salam = "سلام".as_bytes();
        let toks = vec![
            ([b" ".as_slice(), &salam[..3]].concat(), 0.0, 0.2),
            (salam[3..].to_vec(), 0.2, 0.4),
            (b".".to_vec(), 0.4, 0.45),
            (" خوب".as_bytes().to_vec(), 0.5, 0.9),
        ];
        let w = words_from_byte_tokens(&toks);
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].text, "سلام.");
        assert_eq!(w[0].end, 0.45);
        assert_eq!(w[1].text, "خوب");
    }
}
