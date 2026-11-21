import { describe, expect, it } from "vitest";
import {
  citationHref,
  extractCitations,
  linkifyCitations,
  parseCitationGroup,
  parseCiteHref,
} from "./citations";

describe("parseCitationGroup", () => {
  it("parses single ids and ranges", () => {
    expect(parseCitationGroup("12")).toEqual([{ recordingId: null, start: 12, end: 12 }]);
    expect(parseCitationGroup("12-15")).toEqual([{ recordingId: null, start: 12, end: 15 }]);
    expect(parseCitationGroup("15 – 12")).toEqual([{ recordingId: null, start: 12, end: 15 }]);
  });
  it("parses lists, including the Persian comma", () => {
    expect(parseCitationGroup("3, 7")).toEqual([
      { recordingId: null, start: 3, end: 3 },
      { recordingId: null, start: 7, end: 7 },
    ]);
    expect(parseCitationGroup("3، 7")).toHaveLength(2);
  });
  it("parses library citations", () => {
    expect(parseCitationGroup("rec:abc-123#12")).toEqual([
      { recordingId: "abc-123", start: 12, end: 12 },
    ]);
    expect(parseCitationGroup("rec:a#1-2, rec:b#9")).toEqual([
      { recordingId: "a", start: 1, end: 2 },
      { recordingId: "b", start: 9, end: 9 },
    ]);
  });
  it("rejects non-citations", () => {
    expect(parseCitationGroup("x")).toBeNull();
    expect(parseCitationGroup("12, foo")).toBeNull();
  });
});

describe("linkifyCitations", () => {
  it("rewrites citations into cite links", () => {
    expect(linkifyCitations("See [12] and [3-5].")).toBe(
      "See [12](#cite//12/12) and [3–5](#cite//3/5).",
    );
  });
  it("splits lists into separate chips", () => {
    expect(linkifyCitations("[1, 4]")).toBe("[1](#cite//1/1) [4](#cite//4/4)");
  });
  it("handles library citations", () => {
    expect(linkifyCitations("as said [rec:r1#7]")).toBe("as said [7](#cite/r1/7/7)");
  });
  it("leaves real markdown links, footnotes and code alone", () => {
    expect(linkifyCitations("[12](https://x.y)")).toBe("[12](https://x.y)");
    expect(linkifyCitations("text[^1]")).toBe("text[^1]");
    expect(linkifyCitations("`arr[0]` and\n```\nx[1]\n```\n[2]")).toBe(
      "`arr[0]` and\n```\nx[1]\n```\n[2](#cite//2/2)",
    );
  });
  it("does not touch incomplete citations while streaming", () => {
    expect(linkifyCitations("partial [1")).toBe("partial [1");
  });
});

describe("parseCiteHref", () => {
  it("round-trips hrefs", () => {
    const c = { recordingId: "rec id/1", start: 4, end: 9 };
    expect(parseCiteHref(citationHref(c))).toEqual(c);
    expect(parseCiteHref(citationHref({ recordingId: null, start: 1, end: 1 }))).toEqual({
      recordingId: null,
      start: 1,
      end: 1,
    });
  });
  it("rejects other hrefs", () => {
    expect(parseCiteHref("https://example.com")).toBeNull();
    expect(parseCiteHref(undefined)).toBeNull();
    expect(parseCiteHref("#cite/x/1")).toBeNull();
  });
});

describe("extractCitations", () => {
  it("collects every citation in order", () => {
    expect(extractCitations("a [2] b [rec:z#3-4]").map((c) => c.start)).toEqual([2, 3]);
  });
});
