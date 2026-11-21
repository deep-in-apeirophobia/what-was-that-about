import { describe, expect, it } from "vitest";
import {
  containsPersian,
  detectDirection,
  findMatches,
  normalizeForMatch,
  snippetAround,
  splitByRanges,
} from "./text";

describe("detectDirection", () => {
  it("detects Persian as rtl", () => {
    expect(detectDirection("سلام دنیا")).toBe("rtl");
  });
  it("uses the first strong character", () => {
    expect(detectDirection("123 hello سلام")).toBe("ltr");
    expect(detectDirection("  «سلام» hello")).toBe("rtl");
  });
  it("defaults to ltr without strong characters", () => {
    expect(detectDirection("12:30 — !")).toBe("ltr");
    expect(detectDirection("")).toBe("ltr");
  });
  it("detects Persian content", () => {
    expect(containsPersian("abc")).toBe(false);
    expect(containsPersian("abc کتاب")).toBe(true);
  });
});

describe("normalizeForMatch", () => {
  it("keeps the length and unifies Arabic/Persian letters and digits", () => {
    const input = "ArabicYeh: علي كتاب ۱۲۳ ١٢٣";
    const out = normalizeForMatch(input);
    expect(out.length).toBe(input.length);
    expect(out).toBe("arabicyeh: علی کتاب 123 123");
  });
});

describe("findMatches", () => {
  it("finds all occurrences case-insensitively", () => {
    expect(findMatches("Hello hello HELLO", "hello")).toEqual([
      [0, 5],
      [6, 11],
      [12, 17],
    ]);
  });
  it("matches Persian across Arabic letter variants", () => {
    expect(findMatches("كتاب خوب", "کتاب")).toEqual([[0, 4]]);
  });
  it("returns nothing for empty queries", () => {
    expect(findMatches("abc", "  ")).toEqual([]);
  });
});

describe("splitByRanges", () => {
  it("splits plain and highlighted pieces", () => {
    expect(splitByRanges("hello world", [[6, 11]])).toEqual([
      { text: "hello ", highlighted: false, rangeIndex: -1 },
      { text: "world", highlighted: true, rangeIndex: 0 },
    ]);
  });
  it("merges overlapping ranges and clamps out-of-range values", () => {
    const pieces = splitByRanges("abcdefgh", [
      [5, 99],
      [1, 3],
      [2, 4],
    ]);
    expect(pieces.map((p) => [p.text, p.highlighted])).toEqual([
      ["a", false],
      ["bcd", true],
      ["e", false],
      ["fgh", true],
    ]);
    expect(pieces[1].rangeIndex).toBe(1);
  });
  it("returns one plain piece without ranges", () => {
    expect(splitByRanges("abc", [])).toEqual([{ text: "abc", highlighted: false, rangeIndex: -1 }]);
    expect(splitByRanges("", [])).toEqual([{ text: "", highlighted: false, rangeIndex: -1 }]);
  });
});

describe("snippetAround", () => {
  it("returns short text unchanged", () => {
    expect(snippetAround("short", [[0, 2]])).toEqual({ text: "short", ranges: [[0, 2]] });
  });
  it("centres long text on the first match and shifts ranges", () => {
    const text = "x".repeat(300) + "MATCH" + "y".repeat(300);
    const { text: snip, ranges } = snippetAround(text, [[300, 305]], 60);
    expect(snip.startsWith("…")).toBe(true);
    expect(snip.endsWith("…")).toBe(true);
    const [s, e] = ranges[0];
    expect(snip.slice(s, e)).toBe("MATCH");
  });
});
