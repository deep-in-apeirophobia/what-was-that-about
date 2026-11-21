import { describe, expect, it } from "vitest";
import {
  findActiveIndex,
  findContainingIndex,
  indexOfSegmentId,
  mergeSegments,
  nextSegmentTime,
  previousSegmentTime,
  wordRanges,
} from "./segments";

const segs = [
  { id: 0, start: 0.5, end: 3 },
  { id: 1, start: 3.2, end: 6 },
  { id: 2, start: 8, end: 10 },
  { id: 3, start: 10, end: 12.5 },
];

describe("findActiveIndex", () => {
  it("returns -1 before the first segment and for empty lists", () => {
    expect(findActiveIndex(segs, 0)).toBe(-1);
    expect(findActiveIndex([], 5)).toBe(-1);
    expect(findActiveIndex(segs, Number.NaN)).toBe(-1);
  });
  it("finds the segment being spoken", () => {
    expect(findActiveIndex(segs, 0.5)).toBe(0);
    expect(findActiveIndex(segs, 2.9)).toBe(0);
    expect(findActiveIndex(segs, 3.2)).toBe(1);
    expect(findActiveIndex(segs, 10)).toBe(3);
  });
  it("keeps the previous segment active in gaps and after the end", () => {
    expect(findActiveIndex(segs, 7)).toBe(1);
    expect(findActiveIndex(segs, 999)).toBe(3);
  });
  it("matches a linear scan on a large list", () => {
    const many = Array.from({ length: 1000 }, (_, i) => ({ start: i * 2, end: i * 2 + 1.5 }));
    for (const t of [0, 1.7, 2, 999.9, 1500, 1998.4, 5000]) {
      let expected = -1;
      many.forEach((s, i) => {
        if (s.start <= t) expected = i;
      });
      expect(findActiveIndex(many, t)).toBe(expected);
    }
  });
});

describe("findContainingIndex", () => {
  it("returns -1 in gaps", () => {
    expect(findContainingIndex(segs, 7)).toBe(-1);
    expect(findContainingIndex(segs, 5)).toBe(1);
    expect(findContainingIndex(segs, 12.5)).toBe(-1);
  });
});

describe("indexOfSegmentId", () => {
  it("uses the fast path and falls back to search", () => {
    expect(indexOfSegmentId(segs, 2)).toBe(2);
    expect(indexOfSegmentId([{ id: 5 }, { id: 9 }], 9)).toBe(1);
    expect(indexOfSegmentId(segs, 42)).toBe(-1);
  });
});

describe("previous/next sentence", () => {
  it("restarts the current sentence when well into it", () => {
    expect(previousSegmentTime(segs, 5)).toBe(3.2);
  });
  it("goes to the previous sentence near the start", () => {
    expect(previousSegmentTime(segs, 3.5)).toBe(0.5);
    expect(previousSegmentTime(segs, 0.6)).toBe(0.5);
    expect(previousSegmentTime(segs, 0.1)).toBe(0);
  });
  it("finds the next sentence", () => {
    expect(nextSegmentTime(segs, 0)).toBe(0.5);
    expect(nextSegmentTime(segs, 4)).toBe(8);
    expect(nextSegmentTime(segs, 11)).toBeNull();
  });
});

describe("mergeSegments", () => {
  it("adds and replaces by id, sorted by start", () => {
    const merged = mergeSegments(segs.slice(0, 2), [
      { id: 2, start: 8, end: 10 },
      { id: 1, start: 3.2, end: 6.5 },
    ]);
    expect(merged.map((s) => s.id)).toEqual([0, 1, 2]);
    expect(merged[1].end).toBe(6.5);
  });
  it("returns the same array when nothing is incoming", () => {
    const list = segs.slice();
    expect(mergeSegments(list, [])).toBe(list);
  });
});

describe("wordRanges", () => {
  it("locates words in order", () => {
    const text = "سلام به همه، ممنون.";
    const ranges = wordRanges(text, [{ text: "سلام" }, { text: " به" }, { text: "همه،" }, { text: "ممنون." }]);
    expect(ranges.map((r) => (r ? text.slice(r[0], r[1]) : null))).toEqual(["سلام", "به", "همه،", "ممنون."]);
  });
  it("maps tokens it cannot find to null without losing its place", () => {
    const text = "hello brave new world";
    const ranges = wordRanges(text, [{ text: "hello" }, { text: "xyz" }, { text: "new" }, { text: "" }]);
    expect(ranges).toEqual([[0, 5], null, [12, 15], null]);
  });
  it("matches across Arabic/Persian letter variants", () => {
    expect(wordRanges("كتاب", [{ text: "کتاب" }])).toEqual([[0, 4]]);
  });
});
