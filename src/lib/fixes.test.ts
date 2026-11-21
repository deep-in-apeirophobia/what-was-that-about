import { describe, expect, it } from "vitest";
import { diffFix, type FixPiece } from "./fixes";

const changes = (pieces: FixPiece[]) => pieces.filter((p) => p.original !== null).map((p) => [p.original, p.text]);
const joined = (pieces: FixPiece[]) => pieces.map((p) => p.text).join("");

describe("diffFix", () => {
  it("marks a replaced word with what was heard", () => {
    const fixed = "امروز درباره‌ی بودجه حرف می‌زنیم";
    const p = diffFix("امروز درباره‌ی بوجه حرف می‌زنیم", fixed);
    expect(changes(p)).toEqual([["بوجه", "بودجه"]]);
    expect(joined(p)).toBe(fixed);
  });

  it("groups neighbouring changes and handles split or joined words", () => {
    const p = diffFix("ما می خوایم که که بریم", "ما می‌خوایم که بریم");
    expect(changes(p)).toEqual([
      ["می خوایم", "می‌خوایم"],
      ["که", ""],
    ]);
    expect(joined(p)).toBe("ما می‌خوایم که بریم");
  });

  it("marks added words, and keeps punctuation out of the highlight", () => {
    const p = diffFix("سلام خوبی", "سلام، خوبید؟ ممنون");
    expect(changes(p)).toEqual([["خوبی", "خوبید؟ ممنون"]]);
    const q = diffFix("سلام خوبی", "سلام، خوبید؟");
    expect(changes(q)).toEqual([["خوبی", "خوبید"]]);
    expect(joined(q)).toBe("سلام، خوبید؟");
  });

  it("marks punctuation-only changes as minor and ignores spacing", () => {
    const p = diffFix("سلام  به همه. خب", "سلام، به همه خب؟");
    expect(p.filter((x) => x.original !== null)).toEqual([
      { text: "،", original: "", minor: true },
      { text: "", original: ".", minor: true },
      { text: "؟", original: "", minor: true },
    ]);
    expect(joined(p)).toBe("سلام، به همه خب؟");
    expect(diffFix("سلام  به همه", "سلام به همه")).toEqual([{ text: "سلام به همه", original: null }]);
  });

  it("puts removed words at the start or end of the sentence", () => {
    expect(diffFix("خب سلام", "سلام")).toEqual([
      { text: "", original: "خب" },
      { text: "سلام", original: null },
    ]);
    expect(diffFix("سلام دیگه", "سلام")).toEqual([
      { text: "سلام", original: null },
      { text: "", original: "دیگه" },
    ]);
  });

  it("treats Arabic letter variants as corrections", () => {
    expect(changes(diffFix("يک کتاب", "یک کتاب"))).toEqual([["يک", "یک"]]);
  });

  it("falls back to one change for very long texts", () => {
    const long = Array.from({ length: 600 }, (_, i) => `w${i}`).join(" ");
    expect(diffFix(long, `${long} x`)).toEqual([{ text: `${long} x`, original: long }]);
  });
});
