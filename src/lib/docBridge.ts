// The bridge between AI-generated docs (untrusted HTML rendered in a sandboxed
// iframe) and the app's player.
//
// Security model:
// - The iframe uses `sandbox="allow-scripts"` only (no allow-same-origin), so the doc
//   runs in an opaque origin and cannot touch the app's DOM, storage or IPC.
// - A CSP <meta> injected at the very top of <head> blocks all network access.
// - The doc talks to the app only through postMessage; the app validates the sender
//   (`event.source === iframe.contentWindow`) and the payload before acting.
// - Data is embedded with `<`, U+2028 and U+2029 escaped so transcript text such as
//   `</script>` cannot terminate the prelude script.

import type { Analysis, Recording, Segment } from "./types";

export const DOC_CSP =
  "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data: blob:; font-src data:; media-src data: blob:";

export interface DocBridgeData {
  recording: { id: string; title: string; durationSec: number | null };
  segments: { id: number; start: number; end: number; text: string }[];
  analysis: Analysis | null;
}

export function bridgeDataFrom(
  recording: Recording,
  segments: readonly Segment[],
  analysis: Analysis | null,
): DocBridgeData {
  return {
    recording: { id: recording.id, title: recording.title, durationSec: recording.durationSec },
    segments: segments.map((s) => ({ id: s.id, start: s.start, end: s.end, text: s.text })),
    analysis,
  };
}

/** JSON that is safe to embed inside an inline <script> element. */
export function serializeForScript(value: unknown): string {
  return (JSON.stringify(value) ?? "null")
    .replace(/</g, "\\u003c")
    .replace(/>/g, "\\u003e")
    .replace(/&/g, "\\u0026")
    .replace(/\u2028/g, "\\u2028")
    .replace(/\u2029/g, "\\u2029");
}

// Runs inside the iframe. Kept ES5-ish and dependency-free.
const BRIDGE_SCRIPT = `
(function () {
  var data = __WWTA_DATA__;
  var listeners = [];
  function send(msg) { try { window.parent.postMessage(msg, "*"); } catch (e) {} }
  // Keyboard focus inside the frame never reaches the app; forward Escape so the
  // viewer can still be closed from the keyboard.
  window.addEventListener("keydown", function (ev) {
    if (ev.key === "Escape") send({ type: "wwta:escape" });
  });
  window.addEventListener("message", function (ev) {
    if (ev.source !== window.parent) return;
    var d = ev.data;
    if (!d || d.type !== "wwta:time" || typeof d.time !== "number" || !isFinite(d.time)) return;
    var playing = d.playing === true;
    for (var i = 0; i < listeners.length; i++) {
      try { listeners[i](d.time, playing); } catch (e) { if (window.console) console.error(e); }
    }
  });
  window.WWTA = Object.freeze({
    recording: data.recording,
    segments: data.segments,
    analysis: data.analysis,
    seek: function (seconds) {
      var t = Number(seconds);
      if (isFinite(t) && t >= 0) send({ type: "wwta:seek", time: t });
    },
    play: function () { send({ type: "wwta:play" }); },
    pause: function () { send({ type: "wwta:pause" }); },
    onTime: function (cb) {
      if (typeof cb !== "function") return function () {};
      listeners.push(cb);
      return function () {
        var i = listeners.indexOf(cb);
        if (i >= 0) listeners.splice(i, 1);
      };
    }
  });
})();
`;

/** The `<meta>` CSP + bridge `<script>` placed at the top of every doc's <head>. */
export function buildDocPrelude(data: DocBridgeData): string {
  const script = BRIDGE_SCRIPT.replace("__WWTA_DATA__", () => serializeForScript(data));
  return (
    `<meta http-equiv="Content-Security-Policy" content="${DOC_CSP}">` + `<script>${script}</script>`
  );
}

const HEAD_OPEN_RE = /<head(?=[\s>/])[^>]*>/i;
const LEADING_RE = /^(?:\s|<!--[\s\S]*?-->|<!doctype[^>]*>|<html(?=[\s>])[^>]*>)*/i;

/**
 * Insert `prelude` so that it is the first thing the parser sees inside <head>.
 * - `<head>` present and nothing but doctype/<html>/comments before it → right after `<head>`.
 * - `<head>` missing → a new `<head>` holding the prelude after doctype/<html>.
 * - Content before `<head>` (malformed docs) → the prelude goes before that content so the
 *   CSP still applies to everything; the parser places it in the implied head.
 */
export function injectPrelude(html: string, prelude: string): string {
  const leading = LEADING_RE.exec(html)?.[0] ?? "";
  const head = HEAD_OPEN_RE.exec(html);
  if (head && head.index === leading.length) {
    const at = head.index + head[0].length;
    return html.slice(0, at) + prelude + html.slice(at);
  }
  const at = leading.length;
  const insert = head ? prelude : `<head>${prelude}</head>`;
  return html.slice(0, at) + insert + html.slice(at);
}

export function buildDocSrcdoc(html: string, data: DocBridgeData): string {
  return injectPrelude(html, buildDocPrelude(data));
}

// ---- parent side ----------------------------------------------------------------

export type BridgeCommand =
  | { type: "wwta:seek"; time: number }
  | { type: "wwta:play" }
  | { type: "wwta:pause" }
  /** Sent by the prelude itself when Escape is pressed inside the doc. */
  | { type: "wwta:escape" };

/** Upper bound for seek targets when the duration is unknown (24 h). */
const MAX_SEEK = 24 * 3600;

/**
 * Validate a message received from a doc iframe. Returns null for anything that is
 * not exactly one of the bridge commands. `durationSec` (when known) bounds seeks.
 */
export function parseBridgeMessage(data: unknown, durationSec?: number | null): BridgeCommand | null {
  if (typeof data !== "object" || data === null || Array.isArray(data)) return null;
  const msg = data as Record<string, unknown>;
  switch (msg.type) {
    case "wwta:play":
      return { type: "wwta:play" };
    case "wwta:pause":
      return { type: "wwta:pause" };
    case "wwta:escape":
      return { type: "wwta:escape" };
    case "wwta:seek": {
      const t = msg.time;
      if (typeof t !== "number" || !Number.isFinite(t) || t < 0) return null;
      const max = durationSec && durationSec > 0 ? durationSec : MAX_SEEK;
      if (t > max + 1) return null;
      return { type: "wwta:seek", time: Math.min(t, max) };
    }
    default:
      return null;
  }
}
