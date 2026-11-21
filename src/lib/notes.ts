// User notes are anchored by time, not segment ids. These helpers map them onto the
// transcript (which sentences a note covers, where its card goes), sort and group
// them, and turn a selection of sentences back into a time range.

import { findActiveIndex } from "./segments";
import { normalizeForMatch } from "./text";
import type { Note, NoteColor, NoteWithRecording } from "./types";

interface Timed {
  start: number;
  end: number;
}

/** Overlap (seconds) above which a sentence counts as part of a note. */
export const MIN_NOTE_OVERLAP = 0.15;

/** Ids of optimistic notes that the backend hasn't confirmed yet start with this. */
export const PENDING_NOTE_PREFIX = "pending:";

export const isPendingNote = (note: Pick<Note, "id">): boolean => note.id.startsWith(PENDING_NOTE_PREFIX);

/**
 * True when the sentence belongs to the note: their ranges overlap by more than a
 * trivial amount, the sentence's midpoint is inside the note, or the note (e.g. a
 * point in time) lies inside the sentence.
 */
export function noteCoversSegment(note: Timed, seg: Timed): boolean {
  const overlap = Math.min(note.end, seg.end) - Math.max(note.start, seg.start);
  if (overlap > MIN_NOTE_OVERLAP) return true;
  const mid = (seg.start + seg.end) / 2;
  if (mid >= note.start && mid <= note.end) return true;
  return note.start >= seg.start && note.end <= seg.end && note.start < seg.end;
}

export interface NoteLayout {
  /** Notes covering each segment id (segments without notes are absent). */
  bySegment: Map<number, Note[]>;
  /** Notes whose card goes right after this segment: their last covered one, or the
   *  sentence before them when they cover none (a note in a silent gap). */
  after: Map<number, Note[]>;
  /** Notes that come before the first segment. */
  before: Note[];
  /** Where each note starts in the transcript (first covered or anchor segment id). */
  firstSegment: Map<string, number>;
  /** Segments that begin a run of rows with the same notes (where the count badge goes). */
  badgeAt: Set<number>;
}

function push<K, V>(map: Map<K, V[]>, key: K, value: V): void {
  const list = map.get(key);
  if (list) list.push(value);
  else map.set(key, [value]);
}

/** Place notes in a transcript. `segments` must be sorted by start; `notes` order is kept. */
export function layoutNotes(segments: readonly (Timed & { id: number })[], notes: readonly Note[]): NoteLayout {
  const layout: NoteLayout = {
    bySegment: new Map(),
    after: new Map(),
    before: [],
    firstSegment: new Map(),
    badgeAt: new Set(),
  };
  if (!segments.length) return layout;
  for (const note of notes) {
    let first = -1;
    let last = -1;
    for (let i = 0; i < segments.length; i++) {
      const seg = segments[i];
      // Sorted by start: nothing later can reach back into the note.
      if (seg.start > note.end) break;
      if (!noteCoversSegment(note, seg)) continue;
      if (first < 0) first = i;
      last = i;
      push(layout.bySegment, seg.id, note);
    }
    if (last < 0) {
      const anchor = findActiveIndex(segments, note.start);
      if (anchor < 0) {
        layout.before.push(note);
        layout.firstSegment.set(note.id, segments[0].id);
      } else {
        push(layout.after, segments[anchor].id, note);
        layout.firstSegment.set(note.id, segments[anchor].id);
      }
      continue;
    }
    push(layout.after, segments[last].id, note);
    layout.firstSegment.set(note.id, segments[first].id);
  }
  let prevKey = "";
  for (const seg of segments) {
    const key = layout.bySegment.get(seg.id)?.map((n) => n.id).join("\n") ?? "";
    if (key && key !== prevKey) layout.badgeAt.add(seg.id);
    prevKey = key;
  }
  return layout;
}

export interface SegmentSpan {
  start: number;
  end: number;
  firstId: number;
  lastId: number;
}

/** Time span from the earliest to the latest of the given segments (any order). */
export function spanOfSegments(segments: readonly (Timed & { id: number })[], ids: readonly number[]): SegmentSpan | null {
  const wanted = new Set(ids);
  let first = -1;
  let last = -1;
  segments.forEach((seg, i) => {
    if (!wanted.has(seg.id)) return;
    if (first < 0) first = i;
    last = i;
  });
  if (first < 0) return null;
  return {
    start: segments[first].start,
    end: Math.max(segments[last].end, segments[first].start),
    firstId: segments[first].id,
    lastId: segments[last].id,
  };
}

/** Time order; ties by end, then creation. */
export function compareNotes(a: Note, b: Note): number {
  return a.start - b.start || a.end - b.end || a.createdAt.localeCompare(b.createdAt) || a.id.localeCompare(b.id);
}

/** Replace the note with the same id (or add it), keeping time order. */
export function upsertNote<T extends Note>(list: readonly T[], note: T): T[] {
  return [...list.filter((n) => n.id !== note.id), note].sort(compareNotes);
}

export interface NoteGroup {
  recordingId: string;
  title: string;
  notes: NoteWithRecording[];
}

/**
 * Group library notes by recording. Recordings keep the order they first appear in
 * (the backend sends newest recordings first); notes inside a group are in time order.
 * `query` filters on the note text and the recording title.
 */
export function groupNotesByRecording(notes: readonly NoteWithRecording[], query = ""): NoteGroup[] {
  const q = normalizeForMatch(query.trim());
  const groups = new Map<string, NoteGroup>();
  for (const note of notes) {
    if (q && !normalizeForMatch(note.body).includes(q) && !normalizeForMatch(note.recordingTitle).includes(q)) continue;
    let g = groups.get(note.recordingId);
    if (!g) groups.set(note.recordingId, (g = { recordingId: note.recordingId, title: note.recordingTitle, notes: [] }));
    g.notes.push(note);
  }
  const out = [...groups.values()];
  for (const g of out) g.notes.sort(compareNotes);
  return out;
}

/** Ids of the notes playing at `time`, as a stable string key (cheap store selector). */
export function activeNoteKey(notes: readonly (Timed & { id: string })[], time: number): string {
  let key = "";
  for (const n of notes) {
    if (time >= n.start && time < Math.max(n.end, n.start + 0.5)) key += (key ? "\n" : "") + n.id;
  }
  return key;
}

/** A one-line plain-text version of a markdown body (tooltips, toasts). */
export function notePreview(body: string, max = 120): string {
  const text = body
    .replace(/```[\s\S]*?(```|$)/g, " ")
    .replace(/!\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/\[([^\]]+)\]\((?:[^()]|\([^)]*\))*\)/g, "$1")
    .replace(/\[rec:[^\]#\s]+#(\d+(?:\s*[-–]\s*\d+)?)\]/g, "#$1")
    .replace(/^\s{0,3}(?:#{1,6}\s+|>\s?|[-*+]\s+|\d+[.)]\s+)/gm, "")
    .replace(/(\*\*|__|~~|`)/g, "")
    .replace(/(^|[\s(])[*_]([^*_\s][^*_]*?)[*_](?=[\s).,!?:;]|$)/g, "$1$2")
    .replace(/\s+/g, " ")
    .trim();
  return text.length > max ? `${text.slice(0, max - 1).trimEnd()}…` : text;
}

export const NOTE_COLOR_LABELS: Record<NoteColor, string> = {
  yellow: "Yellow",
  green: "Green",
  blue: "Blue",
  purple: "Purple",
  red: "Red",
};

/** CSS color for a note (theme variables from styles.css; null = the neutral default). */
export function noteColor(color: NoteColor | null): string {
  return color ? `var(--note-${color})` : "var(--note-default)";
}

/** `noteColor` mixed toward `base` — for tinted backgrounds and borders. */
export function noteTint(color: NoteColor | null, percent: number, base = "var(--surface)"): string {
  return `color-mix(in srgb, ${noteColor(color)} ${percent}%, ${base})`;
}
