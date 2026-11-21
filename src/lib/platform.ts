// Thin wrappers over Tauri plugins with browser fallbacks for the mock mode.

import { convertFileSrc } from "@tauri-apps/api/core";
import { isTauri } from "./api";
import type { ExportFormat } from "./types";

export const AUDIO_EXTENSIONS = [
  "mp3", "m4a", "aac", "wav", "flac", "ogg", "opus", "oga", "webm",
  "mp4", "mkv", "mov", "wma", "amr", "3gp",
];

export const isMac =
  typeof navigator !== "undefined" && /Mac|iPhone|iPad/i.test(navigator.platform || navigator.userAgent);

/** Label for the platform's primary modifier key. */
export const MOD = isMac ? "⌘" : "Ctrl";

/** URL the webview can load for a file on disk (or a mock blob/data URL). */
export function mediaUrl(path: string | null | undefined): string | null {
  if (!path) return null;
  if (/^(blob:|data:|https?:)/i.test(path)) return path;
  if (!isTauri()) return null;
  return convertFileSrc(path);
}

export function newRequestId(): string {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") return crypto.randomUUID();
  return `req-${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
}

export function fileName(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

/** Browser-only file picker for the mock. Must be called synchronously from a click. */
function browserPick(accept: string, multiple: boolean, dir: string): Promise<string[]> {
  return new Promise((resolve) => {
    const input = document.createElement("input");
    input.type = "file";
    input.multiple = multiple;
    if (accept) input.accept = accept;
    input.style.display = "none";
    const done = (paths: string[]) => {
      input.remove();
      resolve(paths);
    };
    input.addEventListener("change", () => {
      done(Array.from(input.files ?? []).map((f) => `${dir}/${f.name}`));
    });
    input.addEventListener("cancel", () => done([]));
    document.body.appendChild(input);
    input.click();
  });
}

/** Open the system picker for audio/video files. Returns absolute paths ([] on cancel). */
export async function pickAudioFiles(): Promise<string[]> {
  if (!isTauri()) {
    return browserPick(AUDIO_EXTENSIONS.map((e) => `.${e}`).join(","), true, "/home/you/Recordings");
  }
  const { open } = await import("@tauri-apps/plugin-dialog");
  const res = await open({
    multiple: true,
    title: "Import audio or video",
    filters: [{ name: "Audio & video", extensions: AUDIO_EXTENSIONS }],
  });
  return res ?? [];
}

/** Pick model files downloaded by hand. */
export async function pickModelFiles(): Promise<string[]> {
  if (!isTauri()) return browserPick("", true, "/home/you/Downloads");
  const { open } = await import("@tauri-apps/plugin-dialog");
  const res = await open({ multiple: true, title: "Select the downloaded model files" });
  return res ?? [];
}

const FORMAT_NAMES: Record<ExportFormat, string> = {
  txt: "Plain text",
  srt: "SubRip subtitles",
  vtt: "WebVTT subtitles",
  json: "JSON",
  md: "Markdown",
};

/** Ask where to save an export. Returns null when cancelled. */
export async function pickSavePath(baseName: string, format: ExportFormat): Promise<string | null> {
  const safe = baseName.replace(/[\\/:*?"<>|]+/g, " ").trim() || "transcript";
  const defaultPath = `${safe}.${format}`;
  if (!isTauri()) return `/home/you/Downloads/${defaultPath}`;
  const { save } = await import("@tauri-apps/plugin-dialog");
  return save({
    title: "Export transcript",
    defaultPath,
    filters: [{ name: FORMAT_NAMES[format], extensions: [format] }],
  });
}

/** Open a URL in the system browser. */
export async function openExternal(url: string): Promise<void> {
  if (!/^https?:\/\//i.test(url)) throw new Error(`Refusing to open non-web URL: ${url}`);
  if (!isTauri()) {
    window.open(url, "_blank", "noopener,noreferrer");
    return;
  }
  const { openUrl } = await import("@tauri-apps/plugin-opener");
  await openUrl(url);
}

export type DropEvent = { type: "enter" | "leave" } | { type: "drop"; paths: string[] };

/**
 * Listen for files dragged onto the window. In Tauri this uses the webview's native
 * drag & drop (real absolute paths); in the browser mock, HTML5 events with fake paths.
 */
export async function listenFileDrop(cb: (e: DropEvent) => void): Promise<() => void> {
  if (isTauri()) {
    const { getCurrentWebview } = await import("@tauri-apps/api/webview");
    return getCurrentWebview().onDragDropEvent((event) => {
      const p = event.payload;
      if (p.type === "enter") cb({ type: "enter" });
      else if (p.type === "leave") cb({ type: "leave" });
      else if (p.type === "drop") cb({ type: "drop", paths: p.paths });
    });
  }
  let depth = 0;
  const hasFiles = (e: DragEvent) => Array.from(e.dataTransfer?.types ?? []).includes("Files");
  const onEnter = (e: DragEvent) => {
    if (!hasFiles(e)) return;
    e.preventDefault();
    if (depth++ === 0) cb({ type: "enter" });
  };
  const onOver = (e: DragEvent) => {
    if (hasFiles(e)) e.preventDefault();
  };
  const onLeave = (e: DragEvent) => {
    if (!hasFiles(e)) return;
    if (--depth <= 0) {
      depth = 0;
      cb({ type: "leave" });
    }
  };
  const onDrop = (e: DragEvent) => {
    if (!hasFiles(e)) return;
    e.preventDefault();
    depth = 0;
    const paths = Array.from(e.dataTransfer?.files ?? []).map((f) => `/home/you/Recordings/${f.name}`);
    cb({ type: "drop", paths });
  };
  window.addEventListener("dragenter", onEnter);
  window.addEventListener("dragover", onOver);
  window.addEventListener("dragleave", onLeave);
  window.addEventListener("drop", onDrop);
  return () => {
    window.removeEventListener("dragenter", onEnter);
    window.removeEventListener("dragover", onOver);
    window.removeEventListener("dragleave", onLeave);
    window.removeEventListener("drop", onDrop);
  };
}

/** True when keyboard focus is in a text field (shortcuts like Space must not fire). */
export function isTypingTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.isContentEditable) return true;
  const tag = target.tagName;
  if (tag === "TEXTAREA" || tag === "SELECT") return true;
  if (tag === "INPUT") {
    const type = (target as HTMLInputElement).type;
    return !["button", "checkbox", "radio", "range", "submit", "reset", "color", "file"].includes(type);
  }
  return false;
}
