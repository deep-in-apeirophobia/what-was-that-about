// Word-level differences between the recognized text of a segment and its AI-fixed text,
// used to highlight the corrections and to keep word timings on the right words.

import { wordRanges } from "./segments";
import type { Segment } from "./types";

export interface FixPiece {
  /** A slice of the fixed text; "" where words were removed. */
  text: string;
  /** What speech recognition had here: null for unchanged text, "" for added words. */
  original: string | null;
  /** Only punctuation changed here; shown more lightly than word changes. */
  minor?: true;
}

interface Token {
  text: string;
  start: number;
  end: number;
  word: boolean;
}

// Words (letters, marks, digits, ZWNJ/ZWJ) and runs of punctuation; whitespace separates.
const TOKEN_RE = /[\p{L}\p{M}\p{N}\u200c\u200d_]+|[^\s\p{L}\p{M}\p{N}\u200c\u200d_]+/gu;
const WORD_RE = /^[\p{L}\p{M}\p{N}\u200c\u200d_]/u;
/** Above this many alignment cells, the whole sentence counts as one change. */
const MAX_CELLS = 40_000;

function tokenize(text: string): Token[] {
  return [...text.matchAll(TOKEN_RE)].map((m) => ({
    text: m[0],
    start: m.index,
    end: m.index + m[0].length,
    word: WORD_RE.test(m[0]),
  }));
}

/** Misheard words usually share letters: one contains the other, or half the letters match. */
function similar(x: string, y: string): boolean {
  const [short, long] = [x, y].map((t) => t.replaceAll("\u200c", "")).sort((p, q) => p.length - q.length);
  if (long.includes(short)) return true;
  const pool = [...long];
  let common = 0;
  for (const ch of short) {
    const k = pool.indexOf(ch);
    if (k >= 0) {
      common++;
      pool.splice(k, 1);
    }
  }
  return common * 2 >= long.length;
}

// Edit costs. Replacing a token is cheaper than removing one and adding another, so a
// corrected word lines up with the word it replaced; replacing it with a similar word is
// cheapest, so `می خوایم` → `می‌خوایم` pairs `می` with `می‌خوایم`, not some other word.
const ADD_OR_REMOVE = 2;
const REPLACE_SIMILAR = 2;
const REPLACE = 3;

/** Pairs of matching token indices (old, new) from a cheapest token edit script. */
function matchedPairs(a: Token[], b: Token[]): [number, number][] {
  const n = a.length;
  const m = b.length;
  const w = m + 1;
  const replace = (i: number, j: number) => (similar(a[i].text, b[j].text) ? REPLACE_SIMILAR : REPLACE);
  // cost[i * w + j]: cost of turning a[i..] into b[j..]
  const cost = new Uint32Array((n + 1) * w);
  for (let i = n; i >= 0; i--) {
    for (let j = m; j >= 0; j--) {
      const k = i * w + j;
      if (i === n || j === m) cost[k] = (n - i + (m - j)) * ADD_OR_REMOVE;
      else if (a[i].text === b[j].text) cost[k] = cost[k + w + 1];
      else cost[k] = Math.min(cost[k + w + 1] + replace(i, j), cost[k + w] + ADD_OR_REMOVE, cost[k + 1] + ADD_OR_REMOVE);
    }
  }
  const pairs: [number, number][] = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (a[i].text === b[j].text) {
      pairs.push([i++, j++]);
      continue;
    }
    const k = i * w + j;
    // On ties remove, then add, before replacing: that keeps more tokens matched.
    if (cost[k + w] + ADD_OR_REMOVE === cost[k]) i++;
    else if (cost[k + 1] + ADD_OR_REMOVE === cost[k]) j++;
    else {
      i++;
      j++;
    }
  }
  return pairs;
}

/** Drop leading and trailing punctuation tokens, so a highlight covers just the words. */
function trimPunctuation(tokens: Token[]): Token[] {
  let s = 0;
  let e = tokens.length;
  while (s < e && !tokens[s].word) s++;
  while (e > s && !tokens[e - 1].word) e--;
  return tokens.slice(s, e);
}

/**
 * Split `fixed` into unchanged and corrected pieces, each corrected piece carrying the
 * recognized text it replaced. The pieces' texts join back into `fixed`. Word changes are
 * marked without the punctuation around them; punctuation-only changes are marked `minor`,
 * and spacing changes not at all.
 */
export function diffFix(original: string, fixed: string): FixPiece[] {
  const a = tokenize(original);
  const b = tokenize(fixed);
  if (a.length * b.length > MAX_CELLS) return [{ text: fixed, original }];
  const pairs = matchedPairs(a, b);
  pairs.push([a.length, b.length]); // sentinel: the change after the last match

  const out: FixPiece[] = [];
  let pos = 0; // in `fixed`
  const plainUpTo = (end: number) => {
    if (end > pos) out.push({ text: fixed.slice(pos, end), original: null });
    pos = Math.max(pos, end);
  };
  let ai = 0;
  let bi = 0;
  for (const [am, bm] of pairs) {
    let removed = trimPunctuation(a.slice(ai, am));
    let added = trimPunctuation(b.slice(bi, bm));
    const minor = !removed.length && !added.length;
    if (minor) {
      removed = a.slice(ai, am);
      added = b.slice(bi, bm);
    }
    if (removed.length || added.length) {
      const was = removed.length ? original.slice(removed[0].start, removed[removed.length - 1].end) : "";
      let text = "";
      if (added.length) {
        plainUpTo(added[0].start);
        text = fixed.slice(added[0].start, added[added.length - 1].end);
      } else {
        // Removed text: mark the spot after the previous unchanged token.
        plainUpTo(bi > 0 ? b[bi - 1].end : 0);
      }
      out.push(minor ? { text, original: was, minor } : { text, original: was });
      pos += text.length;
    }
    ai = am + 1;
    bi = bm + 1;
  }
  plainUpTo(fixed.length);
  return out;
}

/**
 * Where each timed word sits in the segment's text, like `wordRanges`. The timings belong
 * to the recognized words, so for an AI-fixed segment they are found in the recognized
 * text and carried over to the fixed text: a corrected word lights up when the word it
 * replaced was spoken, and removed words light up nothing.
 */
export function segmentWordRanges(seg: Pick<Segment, "text" | "words" | "originalText">): ([number, number] | null)[] {
  const original = seg.originalText;
  if (original == null) return wordRanges(seg.text, seg.words);
  const a = tokenize(original);
  const b = tokenize(seg.text);
  if (a.length * b.length > MAX_CELLS) return wordRanges(seg.text, seg.words);
  // Where each recognized token ended up in the fixed text (null: removed).
  const to: ([number, number] | null)[] = a.map(() => null);
  const pairs = matchedPairs(a, b);
  pairs.push([a.length, b.length]);
  let ai = 0;
  let bi = 0;
  for (const [am, bm] of pairs) {
    const added = trimPunctuation(b.slice(bi, bm));
    // Only words take the replacement's place; removed punctuation maps to nothing.
    for (let i = ai; i < am; i++) {
      if (added.length && a[i].word) to[i] = [added[0].start, added[added.length - 1].end];
    }
    if (am < a.length) to[am] = [b[bm].start, b[bm].end];
    ai = am + 1;
    bi = bm + 1;
  }
  return wordRanges(original, seg.words).map((r) => {
    if (!r) return null;
    let start = Infinity;
    let end = -1;
    a.forEach((t, i) => {
      const m = to[i];
      if (m && t.start < r[1] && t.end > r[0]) {
        start = Math.min(start, m[0]);
        end = Math.max(end, m[1]);
      }
    });
    return end >= 0 ? [start, end] : null;
  });
}
