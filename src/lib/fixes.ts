// Word-level differences between the recognized text of a segment and its AI-fixed text,
// used to highlight the corrections.

export interface FixPiece {
  /** A slice of the fixed text; "" where words were removed. */
  text: string;
  /** What speech recognition had here: null for unchanged text, "" for added words. */
  original: string | null;
}

interface Token {
  text: string;
  start: number;
  end: number;
  word: boolean;
}

// Words (letters, marks, digits, ZWNJ/ZWJ) and runs of punctuation; whitespace separates.
const TOKEN_RE = /[\p{L}\p{M}\p{N}‌‍_]+|[^\s\p{L}\p{M}\p{N}‌‍_]+/gu;
const WORD_RE = /^[\p{L}\p{M}\p{N}‌‍_]/u;
/** Above this many LCS cells, the whole sentence counts as one change. */
const MAX_CELLS = 250_000;

function tokenize(text: string): Token[] {
  return [...text.matchAll(TOKEN_RE)].map((m) => ({
    text: m[0],
    start: m.index,
    end: m.index + m[0].length,
    word: WORD_RE.test(m[0]),
  }));
}

/** Pairs of matching token indices (old, new) of a longest common subsequence. */
function lcsPairs(a: Token[], b: Token[]): [number, number][] {
  const n = a.length;
  const m = b.length;
  const w = m + 1;
  const len = new Uint32Array((n + 1) * w);
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      len[i * w + j] = a[i].text === b[j].text ? len[(i + 1) * w + j + 1] + 1 : Math.max(len[(i + 1) * w + j], len[i * w + j + 1]);
    }
  }
  const pairs: [number, number][] = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (a[i].text === b[j].text) pairs.push([i++, j++]);
    // On ties skip the added token first, so a replacement and a removal stay separate.
    else if (len[(i + 1) * w + j] > len[i * w + j + 1]) i++;
    else j++;
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
 * recognized text it replaced. The pieces' texts join back into `fixed`. Changes that only
 * touch punctuation or spacing are left unmarked.
 */
export function diffFix(original: string, fixed: string): FixPiece[] {
  const a = tokenize(original);
  const b = tokenize(fixed);
  if (a.length * b.length > MAX_CELLS) return [{ text: fixed, original }];
  const pairs = lcsPairs(a, b);
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
    const removed = trimPunctuation(a.slice(ai, am));
    const added = trimPunctuation(b.slice(bi, bm));
    if (removed.length || added.length) {
      const was = removed.length ? original.slice(removed[0].start, removed[removed.length - 1].end) : "";
      if (added.length) {
        const start = added[0].start;
        const end = added[added.length - 1].end;
        plainUpTo(start);
        out.push({ text: fixed.slice(start, end), original: was });
        pos = end;
      } else {
        // Removed words: mark the spot after the previous unchanged word.
        plainUpTo(bi > 0 ? b[bi - 1].end : 0);
        out.push({ text: "", original: was });
      }
    }
    ai = am + 1;
    bi = bm + 1;
  }
  plainUpTo(fixed.length);
  return out;
}
