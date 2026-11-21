// Citation parsing for AI answers.
//
// Answers reference transcript segments as `[12]`, `[12-15]` or `[3, 7]` inside a
// recording chat, and as `[rec:<recordingId>#12]` in library-wide chats. Before
// rendering markdown we rewrite each citation into a link with a private `#cite/`
// href that the markdown renderer turns into a clickable chip.

export interface Citation {
  /** null = the recording the chat belongs to. */
  recordingId: string | null;
  start: number;
  end: number;
}

const PART = String.raw`(?:rec:[^\]#\s,،;]+#)?\d+(?:\s*[-–]\s*\d+)?`;
const CITATION_RE = new RegExp(String.raw`\[(${PART}(?:\s*[,،;]\s*${PART})*)\](?![(\[])`, "g");
const PART_RE = /^(?:rec:([^\]#\s,\u060C;]+)#)?(\d+)(?:\s*[-\u2013]\s*(\d+))?$/;

/** Parse the inside of one bracket, e.g. `"12-15"`, `"3, 7"`, `"rec:abc#4"`. */
export function parseCitationGroup(inner: string): Citation[] | null {
  const parts = inner.split(/[,،;]/).map((p) => p.trim());
  const out: Citation[] = [];
  for (const part of parts) {
    const m = PART_RE.exec(part);
    if (!m) return null;
    const a = Number(m[2]);
    const b = m[3] !== undefined ? Number(m[3]) : a;
    out.push({ recordingId: m[1] ?? null, start: Math.min(a, b), end: Math.max(a, b) });
  }
  return out.length ? out : null;
}

export function citationHref(c: Citation): string {
  return `#cite/${encodeURIComponent(c.recordingId ?? "")}/${c.start}/${c.end}`;
}

export function citationLabel(c: Citation): string {
  return c.start === c.end ? String(c.start) : `${c.start}–${c.end}`;
}

/** Inverse of `citationHref`. Returns null for any other href. */
export function parseCiteHref(href: string | undefined | null): Citation | null {
  if (!href) return null;
  const m = /^#cite\/([^/]*)\/(\d+)\/(\d+)$/.exec(href);
  if (!m) return null;
  let recordingId: string | null = null;
  try {
    recordingId = m[1] ? decodeURIComponent(m[1]) : null;
  } catch {
    return null;
  }
  return { recordingId, start: Number(m[2]), end: Number(m[3]) };
}

function linkifyPlain(text: string): string {
  return text.replace(CITATION_RE, (whole, inner: string) => {
    const cites = parseCitationGroup(inner);
    if (!cites) return whole;
    return cites.map((c) => `[${citationLabel(c)}](${citationHref(c)})`).join(" ");
  });
}

/** Rewrite citations in markdown into `#cite/` links, leaving code spans and fences alone. */
export function linkifyCitations(markdown: string): string {
  // Split into code (fenced blocks and inline spans) and prose; only touch prose.
  const CODE_RE = /(```[\s\S]*?(?:```|$)|~~~[\s\S]*?(?:~~~|$)|`[^`\n]*`)/g;
  let out = "";
  let last = 0;
  for (const m of markdown.matchAll(CODE_RE)) {
    const idx = m.index ?? 0;
    out += linkifyPlain(markdown.slice(last, idx)) + m[0];
    last = idx + m[0].length;
  }
  return out + linkifyPlain(markdown.slice(last));
}

/** All citations in a piece of text, in order of appearance. */
export function extractCitations(markdown: string): Citation[] {
  const out: Citation[] = [];
  for (const m of markdown.matchAll(CITATION_RE)) {
    const cites = parseCitationGroup(m[1]);
    if (cites) out.push(...cites);
  }
  return out;
}
