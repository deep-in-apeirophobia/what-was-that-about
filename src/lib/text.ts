// Text helpers: direction detection, Persian-aware normalization, match finding and
// splitting text into highlighted / plain pieces.

const RTL_RE = /[\u0590-\u08FF\uFB1D-\uFDFF\uFE70-\uFEFF]/;
const LTR_RE = /[A-Za-z\u00C0-\u024F\u0370-\u03FF\u0400-\u04FF]/;
const PERSIAN_RE = /[\u0600-\u06FF\uFB50-\uFDFF\uFE70-\uFEFF]/;

/** Direction of the first strongly-directional character; `"ltr"` when there is none. */
export function detectDirection(text: string): "rtl" | "ltr" {
  for (const ch of text) {
    if (RTL_RE.test(ch)) return "rtl";
    if (LTR_RE.test(ch)) return "ltr";
  }
  return "ltr";
}

export function containsPersian(text: string): boolean {
  return PERSIAN_RE.test(text);
}

/**
 * Normalize for matching without changing string length (indices stay valid):
 * lower-case, Arabic yeh/kaf → Persian, Arabic/Persian digits → ASCII.
 */
export function normalizeForMatch(text: string): string {
  let out = "";
  for (const ch of text) {
    const code = ch.charCodeAt(0);
    if (ch === "ي" || ch === "ى") out += "ی";
    else if (ch === "ك") out += "ک";
    else if (ch === "ة") out += "ه";
    else if (code >= 0x06f0 && code <= 0x06f9) out += String.fromCharCode(code - 0x06f0 + 48);
    else if (code >= 0x0660 && code <= 0x0669) out += String.fromCharCode(code - 0x0660 + 48);
    else {
      const lower = ch.toLowerCase();
      // Only accept lower-casing that keeps the length (e.g. not "İ" → "i̇").
      out += lower.length === ch.length ? lower : ch;
    }
  }
  return out;
}

/** All non-overlapping occurrences of `query` in `text` as [start, end) ranges. */
export function findMatches(text: string, query: string): [number, number][] {
  const q = normalizeForMatch(query.trim());
  if (!q) return [];
  const hay = normalizeForMatch(text);
  const out: [number, number][] = [];
  let from = 0;
  for (;;) {
    const i = hay.indexOf(q, from);
    if (i < 0) break;
    out.push([i, i + q.length]);
    from = i + q.length;
  }
  return out;
}

export interface TextPiece {
  text: string;
  highlighted: boolean;
  /** Index of the highlight range this piece belongs to (-1 for plain text). */
  rangeIndex: number;
}

/**
 * Split `text` into plain and highlighted pieces. Ranges are clamped, sorted and
 * overlapping ranges merged (the merged range keeps the lowest original index).
 */
export function splitByRanges(text: string, ranges: readonly (readonly [number, number])[]): TextPiece[] {
  const clean = ranges
    .map(([s, e], i) => ({
      s: Math.max(0, Math.min(text.length, Math.floor(s))),
      e: Math.max(0, Math.min(text.length, Math.floor(e))),
      i,
    }))
    .filter((r) => r.e > r.s)
    .sort((a, b) => a.s - b.s || b.e - a.e);
  const merged: { s: number; e: number; i: number }[] = [];
  for (const r of clean) {
    const last = merged[merged.length - 1];
    if (last && r.s <= last.e) {
      last.e = Math.max(last.e, r.e);
      last.i = Math.min(last.i, r.i);
    } else merged.push({ ...r });
  }
  const out: TextPiece[] = [];
  let pos = 0;
  for (const r of merged) {
    if (r.s > pos) out.push({ text: text.slice(pos, r.s), highlighted: false, rangeIndex: -1 });
    out.push({ text: text.slice(r.s, r.e), highlighted: true, rangeIndex: r.i });
    pos = r.e;
  }
  if (pos < text.length || out.length === 0)
    out.push({ text: text.slice(pos), highlighted: false, rangeIndex: -1 });
  return out;
}

/** Truncate around the first highlight so search snippets show the match. */
export function snippetAround(
  text: string,
  ranges: readonly (readonly [number, number])[],
  maxLen = 160,
): { text: string; ranges: [number, number][] } {
  if (text.length <= maxLen) return { text, ranges: ranges.map(([s, e]) => [s, e]) };
  const first = ranges.length ? Math.min(...ranges.map((r) => r[0])) : 0;
  let start = Math.max(0, first - Math.floor(maxLen / 3));
  const end = Math.min(text.length, start + maxLen);
  start = Math.max(0, end - maxLen);
  const prefix = start > 0 ? "…" : "";
  const suffix = end < text.length ? "…" : "";
  const shift = prefix.length - start;
  return {
    text: prefix + text.slice(start, end) + suffix,
    ranges: ranges
      .map(([s, e]) => [Math.max(s, start) + shift, Math.min(e, end) + shift] as [number, number])
      .filter(([s, e]) => e > s),
  };
}
