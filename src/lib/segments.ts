// Time ↔ segment lookups used by the player and transcript.

import { normalizeForMatch } from "./text";

interface Timed {
  start: number;
  end: number;
}

/**
 * Index of the item being spoken at `time`: the last item whose start is <= time.
 * While in a silent gap the previous item stays active, which keeps the highlight
 * steady. Returns -1 before the first item or for an empty list. Items must be
 * sorted by start. O(log n).
 */
export function findActiveIndex(items: readonly Timed[], time: number): number {
  if (!items.length || !(time >= items[0].start)) return -1;
  let lo = 0;
  let hi = items.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (items[mid].start <= time) lo = mid;
    else hi = mid - 1;
  }
  return lo;
}

/**
 * Like `findActiveIndex` but only returns an index when `time` is inside the item
 * (used for word-level karaoke, where gaps should not highlight anything).
 */
export function findContainingIndex(items: readonly Timed[], time: number): number {
  const i = findActiveIndex(items, time);
  if (i < 0) return -1;
  return time < items[i].end ? i : -1;
}

/** Position of the segment with the given id. Ids are normally the array index. */
export function indexOfSegmentId(items: readonly { id: number }[], id: number): number {
  if (items[id]?.id === id) return id;
  return items.findIndex((s) => s.id === id);
}

/**
 * Where "previous sentence" should jump: the start of the current sentence when
 * we're more than `restartThreshold` seconds into it, otherwise the one before.
 */
export function previousSegmentTime(
  items: readonly Timed[],
  time: number,
  restartThreshold = 1.5,
): number {
  const i = findActiveIndex(items, time);
  if (i < 0) return 0;
  if (time - items[i].start > restartThreshold) return items[i].start;
  return i > 0 ? items[i - 1].start : items[0].start;
}

/** Start of the next sentence after `time`, or null at the end. */
export function nextSegmentTime(items: readonly Timed[], time: number): number | null {
  const i = findActiveIndex(items, time);
  const next = items[i + 1];
  return next ? next.start : null;
}

/** Merge live segments into an existing list by id, keeping start order. */
export function mergeSegments<T extends { id: number; start: number }>(
  existing: readonly T[],
  incoming: readonly T[],
): T[] {
  if (!incoming.length) return existing as T[];
  const byId = new Map<number, T>();
  for (const s of existing) byId.set(s.id, s);
  for (const s of incoming) byId.set(s.id, s);
  return [...byId.values()].sort((a, b) => a.start - b.start || a.id - b.id);
}

/**
 * Character ranges of each word inside the segment text, so the active word can be
 * highlighted without re-rendering the text from word tokens (which may differ in
 * spacing or punctuation). Words that can't be located map to null.
 */
export function wordRanges(text: string, words: readonly { text: string }[]): ([number, number] | null)[] {
  const hay = normalizeForMatch(text);
  let cursor = 0;
  return words.map((w) => {
    const needle = normalizeForMatch(w.text.trim());
    if (!needle) return null;
    const i = hay.indexOf(needle, cursor);
    // Don't let one unmatched token make us skip far ahead.
    if (i < 0 || i - cursor > 24) return null;
    cursor = i + needle.length;
    return [i, i + needle.length];
  });
}
