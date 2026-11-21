import { describe, expect, it } from "vitest";
import {
  DOC_CSP,
  buildDocPrelude,
  buildDocSrcdoc,
  injectPrelude,
  parseBridgeMessage,
  serializeForScript,
  type DocBridgeData,
} from "./docBridge";

const data: DocBridgeData = {
  recording: { id: "r1", title: "Test </script><script>alert(1)</script>", durationSec: 120 },
  segments: [
    { id: 0, start: 0, end: 2, text: "سلام </SCRIPT ><img src=x onerror=alert(1)>" },
    { id: 1, start: 2, end: 4, text: "line\u2028sep\u2029para & <!-- -->" },
  ],
  analysis: null,
};

function parse(html: string): Document {
  return new DOMParser().parseFromString(html, "text/html");
}

describe("serializeForScript", () => {
  it("escapes characters that could break out of a script element", () => {
    const out = serializeForScript({ s: "</script>\u2028\u2029&>" });
    expect(out).not.toMatch(/[<>&\u2028\u2029]/);
    expect(JSON.parse(out)).toEqual({ s: "</script>\u2028\u2029&>" });
  });
  it("serializes undefined as null", () => {
    expect(serializeForScript(undefined)).toBe("null");
  });
});

describe("buildDocPrelude", () => {
  const prelude = buildDocPrelude(data);
  it("starts with the CSP meta", () => {
    expect(prelude.startsWith(`<meta http-equiv="Content-Security-Policy" content="${DOC_CSP}">`)).toBe(
      true,
    );
  });
  it("contains exactly one closing script tag even with hostile transcript text", () => {
    expect(prelude.match(/<\/script/gi)).toHaveLength(1);
    expect(prelude).not.toMatch(/<img/i);
  });
  it("embeds data that evaluates back to the original", () => {
    const m = /var data = (.*);\n/.exec(prelude);
    expect(m).not.toBeNull();
    expect(JSON.parse(m![1])).toEqual(data);
  });
  it("defines the documented API", () => {
    for (const name of ["seek", "play", "pause", "onTime", "recording", "segments", "analysis"]) {
      expect(prelude).toContain(`${name}:`);
    }
    expect(prelude).toContain('type: "wwta:seek"');
    expect(prelude).toContain('type: "wwta:play"');
    expect(prelude).toContain('type: "wwta:pause"');
    expect(prelude).toContain('"wwta:time"');
  });
});

describe("injectPrelude", () => {
  const P = "<!--P-->";
  it("inserts right after an existing <head>", () => {
    expect(injectPrelude("<!doctype html><html><head><title>x</title></head></html>", P)).toBe(
      "<!doctype html><html><head><!--P--><title>x</title></head></html>",
    );
  });
  it("handles attributes and casing on <head>", () => {
    expect(injectPrelude('<HTML lang="fa"><HEAD data-x="1"><title>x</title>', P)).toBe(
      '<HTML lang="fa"><HEAD data-x="1"><!--P--><title>x</title>',
    );
  });
  it("does not confuse <header> with <head>", () => {
    expect(injectPrelude("<html><body><header>hi</header></body></html>", P)).toBe(
      "<html><head><!--P--></head><body><header>hi</header></body></html>",
    );
  });
  it("creates a head for fragments", () => {
    expect(injectPrelude("<div>hello</div>", P)).toBe("<head><!--P--></head><div>hello</div>");
    expect(injectPrelude("<!DOCTYPE html>\n<div>x</div>", P)).toBe(
      "<!DOCTYPE html>\n<head><!--P--></head><div>x</div>",
    );
  });
  it("goes before content that precedes a late <head>", () => {
    expect(injectPrelude("<html><script>evil()</script><head></head>", P)).toBe(
      "<html><!--P--><script>evil()</script><head></head>",
    );
  });
});

describe("buildDocSrcdoc (parsed)", () => {
  const hostile =
    "<!doctype html><html><head><title>Doc</title></head><body><script>window.x=1</script></body></html>";

  it("makes the CSP meta the first element of <head>", () => {
    const doc = parse(buildDocSrcdoc(hostile, data));
    const first = doc.head.firstElementChild;
    expect(first?.tagName).toBe("META");
    expect(first?.getAttribute("http-equiv")).toBe("Content-Security-Policy");
    expect(first?.getAttribute("content")).toBe(DOC_CSP);
    expect(doc.head.children[1]?.tagName).toBe("SCRIPT");
  });

  it("keeps hostile transcript text inside the bridge script", () => {
    const doc = parse(buildDocSrcdoc(hostile, data));
    const scripts = [...doc.querySelectorAll("script")];
    expect(scripts).toHaveLength(2); // bridge + the doc's own script
    expect(doc.querySelectorAll("img")).toHaveLength(0);
    expect(scripts[0].textContent).toContain("WWTA");
  });

  it("puts the CSP first even for fragments and malformed docs", () => {
    for (const html of ["<p>hi</p>", "<script>a()</script><head></head><body></body>"]) {
      const doc = parse(buildDocSrcdoc(html, data));
      expect(doc.head.firstElementChild?.getAttribute("http-equiv")).toBe("Content-Security-Policy");
    }
  });
});

describe("parseBridgeMessage", () => {
  it("accepts valid commands", () => {
    expect(parseBridgeMessage({ type: "wwta:play" })).toEqual({ type: "wwta:play" });
    expect(parseBridgeMessage({ type: "wwta:pause", extra: 1 })).toEqual({ type: "wwta:pause" });
    expect(parseBridgeMessage({ type: "wwta:escape" })).toEqual({ type: "wwta:escape" });
    expect(parseBridgeMessage({ type: "wwta:seek", time: 12.5 }, 100)).toEqual({
      type: "wwta:seek",
      time: 12.5,
    });
  });
  it("clamps tiny overshoots to the duration", () => {
    expect(parseBridgeMessage({ type: "wwta:seek", time: 100.5 }, 100)).toEqual({
      type: "wwta:seek",
      time: 100,
    });
  });
  it("rejects malformed payloads", () => {
    const bad: unknown[] = [
      null,
      undefined,
      "wwta:play",
      42,
      [],
      { type: "wwta:seek" },
      { type: "wwta:seek", time: "12" },
      { type: "wwta:seek", time: -1 },
      { type: "wwta:seek", time: Number.NaN },
      { type: "wwta:seek", time: Number.POSITIVE_INFINITY },
      { type: "wwta:seek", time: 500 },
      { type: "wwta:navigate", url: "https://evil" },
      { type: "wwta:time", time: 1 },
    ];
    for (const b of bad) expect(parseBridgeMessage(b, 100)).toBeNull();
  });
  it("uses a 24h bound when the duration is unknown", () => {
    expect(parseBridgeMessage({ type: "wwta:seek", time: 3600 }, null)).not.toBeNull();
    expect(parseBridgeMessage({ type: "wwta:seek", time: 90_000 }, null)).toBeNull();
  });
});
