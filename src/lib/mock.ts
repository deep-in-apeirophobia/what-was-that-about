// In-memory backend used when the UI runs in a plain browser (`pnpm dev`).
// Mirrors the Tauri commands in api.ts closely enough to exercise every screen:
// model installs, transcription with live segments, AI analysis, streaming chat,
// interactive docs, notes, search, tags and related segments.

import type {
  Analysis,
  ChatMessage,
  Doc,
  ExportFormat,
  ModelInfo,
  ModelProgress,
  Note,
  NoteColor,
  NoteWithRecording,
  Recording,
  SearchHit,
  Segment,
  Settings,
  TagSummary,
  TranscriptionProgress,
  Waveform,
} from "./types";
import { NOTE_COLORS } from "./types";
import {
  HAFEZ_LINES,
  MEETING_LINES,
  MOCK_LLM_MODELS,
  MOCK_SYSTEM,
  PODCAST_LINES,
  buildPeaks,
  buildSegments,
  docKindFromPrompt,
  docTitle,
  hafezAnalysis,
  initialModels,
  meetingAnalysis,
  mockDocHtml,
  podcastAnalysis,
} from "./mockData";
import { containsPersian, findMatches, normalizeForMatch } from "./text";
import { formatTime } from "./format";
import { compareNotes } from "./notes";
import { findActiveIndex } from "./segments";

const EV = {
  model: "model://progress",
  transcription: "transcription://progress",
  llm: "llm://stream",
  updated: "library://recording-updated",
  deleted: "library://recording-deleted",
};

// ---- event bus ----------------------------------------------------------------------

type Listener = (payload: unknown) => void;
const listeners = new Map<string, Set<Listener>>();

export function subscribe(event: string, cb: (payload: unknown) => void): () => void {
  let set = listeners.get(event);
  if (!set) listeners.set(event, (set = new Set()));
  set.add(cb);
  return () => {
    set.delete(cb);
  };
}

function emit(event: string, payload: unknown): void {
  const set = listeners.get(event);
  if (!set) return;
  for (const cb of [...set]) setTimeout(() => cb(payload), 0);
}

// ---- helpers ------------------------------------------------------------------------

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));
const nowIso = () => new Date().toISOString();
const ago = (minutes: number) => new Date(Date.now() - minutes * 60_000).toISOString();
let counter = 0;
const uid = (prefix: string) => `${prefix}-${Date.now().toString(36)}${(counter++).toString(36)}`;

function str(args: Record<string, unknown>, key: string): string {
  const v = args[key];
  if (typeof v !== "string") throw new Error(`mock: expected string argument "${key}"`);
  return v;
}
function optStr(args: Record<string, unknown>, key: string): string | null {
  const v = args[key];
  return typeof v === "string" ? v : null;
}
function num(args: Record<string, unknown>, key: string, fallback?: number): number {
  const v = args[key];
  if (typeof v === "number" && Number.isFinite(v)) return v;
  if (fallback !== undefined) return fallback;
  throw new Error(`mock: expected number argument "${key}"`);
}
function strArray(args: Record<string, unknown>, key: string): string[] {
  const v = args[key];
  if (!Array.isArray(v) || !v.every((x) => typeof x === "string")) throw new Error(`mock: expected string[] "${key}"`);
  return v;
}

/** 8 kHz 8-bit mono silence, so the <audio> element behaves like the real thing. */
function silentWav(seconds: number): Blob {
  const rate = 8000;
  const n = Math.ceil(seconds * rate);
  const buf = new ArrayBuffer(44 + n);
  const v = new DataView(buf);
  const ascii = (o: number, s: string) => [...s].forEach((c, i) => v.setUint8(o + i, c.charCodeAt(0)));
  ascii(0, "RIFF");
  v.setUint32(4, 36 + n, true);
  ascii(8, "WAVE");
  ascii(12, "fmt ");
  v.setUint32(16, 16, true);
  v.setUint16(20, 1, true);
  v.setUint16(22, 1, true);
  v.setUint32(24, rate, true);
  v.setUint32(28, rate, true);
  v.setUint16(32, 1, true);
  v.setUint16(34, 8, true);
  ascii(36, "data");
  v.setUint32(40, n, true);
  new Uint8Array(buf, 44).fill(128);
  return new Blob([buf], { type: "audio/wav" });
}

// ---- state --------------------------------------------------------------------------

type Bank = "meeting" | "hafez" | "podcast";

interface RecState {
  rec: Recording;
  /** The "audio content": the full transcript this recording will produce. */
  bank: Bank;
  full: Segment[];
  segments: Segment[];
  analysis: Analysis | null;
  docs: Doc[];
  chat: ChatMessage[];
  notes: Note[];
  embeddingsIndexed: boolean;
  job: { cancelled: boolean } | null;
  audioUrl: string | null;
}

const BANKS: Record<Bank, { lines: string[]; seed: number }> = {
  meeting: { lines: MEETING_LINES, seed: 11 },
  hafez: { lines: HAFEZ_LINES, seed: 23 },
  podcast: { lines: PODCAST_LINES, seed: 37 },
};

function bankSegments(bank: Bank): Segment[] {
  return buildSegments(BANKS[bank].lines, BANKS[bank].seed);
}

function bankAnalysis(bank: Bank, segments: Segment[]): Analysis {
  const at = nowIso();
  if (bank === "meeting") return meetingAnalysis(segments, at);
  if (bank === "hafez") return hafezAnalysis(segments, at);
  return podcastAnalysis(segments, at);
}

function durationOf(segments: Segment[]): number {
  return segments.length ? Math.ceil(segments[segments.length - 1].end + 1.5) : 0;
}

let settings: Settings = {
  openrouterApiKey: "",
  llmModel: "deepseek/deepseek-v4-flash",
  embeddingModel: "baai/bge-m3",
  aiLanguage: "auto",
  defaultModelId: "taraneh-wlarge",
  useGpu: true,
  threads: 0,
  copyIntoLibrary: true,
  theme: "system",
  whisperQuant: "q8_0",
  keepSourceWeights: false,
  persianDigits: true,
  preciseWordTimings: false,
  autoAnalyze: true,
  analyticsEnabled: true,
  crashReportsEnabled: true,
};

const models: ModelInfo[] = initialModels(MOCK_SYSTEM.dataDir);
const installs = new Map<string, { cancelled: boolean }>();
const cancelledRequests = new Set<string>();
const recs = new Map<string, RecState>();
let libraryChat: ChatMessage[] = [];

function makeRecording(partial: Partial<Recording> & Pick<Recording, "id" | "title" | "sourcePath">): Recording {
  return {
    audioPath: partial.sourcePath,
    playbackWavPath: null,
    durationSec: null,
    createdAt: nowIso(),
    updatedAt: nowIso(),
    status: "new",
    modelId: null,
    error: null,
    segmentCount: 0,
    hasAnalysis: false,
    tags: [],
    blurb: null,
    ...partial,
  };
}

function blurbOf(a: Analysis | null): string | null {
  if (!a) return null;
  const first = a.summary.split(/\n\s*\n/)[0] ?? "";
  return first.replace(/\*\*/g, "").trim() || null;
}

function syncSummary(s: RecState): void {
  s.rec.segmentCount = s.segments.length;
  s.rec.hasAnalysis = !!s.analysis;
  s.rec.tags = s.analysis ? s.analysis.tags.slice(0, 4).map((t) => t.name) : [];
  s.rec.blurb = blurbOf(s.analysis);
}

/** Lazily attach a playable (silent) audio URL once the duration is known. */
function withAudio(s: RecState): Recording {
  if (s.rec.durationSec && !s.audioUrl && typeof URL.createObjectURL === "function") {
    s.audioUrl = URL.createObjectURL(silentWav(s.rec.durationSec));
  }
  if (s.audioUrl) s.rec.audioPath = s.audioUrl;
  return { ...s.rec, tags: [...s.rec.tags] };
}

function touch(s: RecState): void {
  s.rec.updatedAt = nowIso();
  syncSummary(s);
  emit(EV.updated, withAudio(s));
}

function seed(): void {
  // 1. A fully processed meeting with analysis, a doc and a chat.
  const meetingFull = bankSegments("meeting");
  const meeting: RecState = {
    rec: makeRecording({
      id: "rec-meeting",
      title: "جلسهٔ برنامه‌ریزی محصول — پاییز",
      sourcePath: "/home/you/Recordings/meeting-2026-11-18.m4a",
      durationSec: durationOf(meetingFull),
      createdAt: ago(60 * 50),
      updatedAt: ago(60 * 49),
      status: "ready",
      modelId: "taraneh-wlarge",
    }),
    bank: "meeting",
    full: meetingFull,
    segments: meetingFull,
    analysis: meetingAnalysis(meetingFull, ago(60 * 49)),
    docs: [],
    chat: [],
    notes: [],
    embeddingsIndexed: true,
    job: null,
    audioUrl: null,
  };
  meeting.docs.push({
    id: "doc-timeline",
    recordingId: meeting.rec.id,
    title: "Interactive timeline",
    prompt: "Create an interactive timeline of the chapters. Clicking a chapter plays it.",
    html: mockDocHtml("timeline"),
    model: settings.llmModel,
    createdAt: ago(60 * 48),
    updatedAt: ago(60 * 48),
  });
  meeting.chat.push(
    {
      id: "msg-1",
      recordingId: meeting.rec.id,
      role: "user",
      content: "What were the main findings from the user interviews?",
      createdAt: ago(60 * 47),
      model: null,
    },
    {
      id: "msg-2",
      recordingId: meeting.rec.id,
      role: "assistant",
      content:
        "Three themes came up in the interviews:\n\n" +
        "1. **Search** — users find it hard to search long recordings and forget who said what [4-5]. One user re-listens for about three hours a week [6].\n" +
        "2. **Accuracy** — Persian speech-to-text is still a concern, especially with overlapping speakers [7-8].\n" +
        "3. **Privacy** — many don't want to upload files to foreign servers, which makes on-device processing an advantage [9, 10].\n\n" +
        "Sara added that automatic summaries and action items were very appealing [12-13], though some worried about LLM costs [14].",
      createdAt: ago(60 * 47),
      model: settings.llmModel,
    },
  );

  // Notes: one with a URL, one linking to a moment in the lecture, one range note.
  const span = (from: number, to: number) => ({ start: meetingFull[from].start, end: meetingFull[to].end });
  meeting.notes.push(
    {
      id: "note-search",
      recordingId: meeting.rec.id,
      ...span(4, 6),
      body:
        "Strongest signal from the interviews: people can't **find things** in long recordings.\n\n" +
        "Background on meaning-based search: https://en.wikipedia.org/wiki/Semantic_search",
      color: "yellow",
      createdAt: ago(60 * 46),
      updatedAt: ago(60 * 46),
    },
    {
      id: "note-summaries",
      recordingId: meeting.rec.id,
      ...span(12, 13),
      body: "خلاصه‌سازی خودکار رو با درس حافظ امتحان کنیم — متن ادبی محک خوبیه [rec:rec-hafez#4]",
      color: "green",
      createdAt: ago(60 * 45),
      updatedAt: ago(60 * 30),
    },
    {
      id: "note-actions",
      recordingId: meeting.rec.id,
      ...span(36, 38),
      body:
        "**Action items**\n- رضا: نمونهٔ اولیهٔ جستجوی معنایی تا آخر هفته\n- سارا: پنج جلسهٔ تست کاربری\n- Me: update the roadmap doc [38]",
      color: "purple",
      createdAt: ago(60 * 44),
      updatedAt: ago(60 * 44),
    },
  );

  // 2. A Persian lecture with a Persian analysis.
  const hafezFull = bankSegments("hafez");
  const hafez: RecState = {
    rec: makeRecording({
      id: "rec-hafez",
      title: "درس‌گفتار ادبیات — حافظ",
      sourcePath: "/home/you/Recordings/lecture-hafez.mp3",
      durationSec: durationOf(hafezFull),
      createdAt: ago(60 * 24 * 5),
      updatedAt: ago(60 * 24 * 5),
      status: "ready",
      modelId: "shenava-koochik",
    }),
    bank: "hafez",
    full: hafezFull,
    segments: hafezFull,
    analysis: hafezAnalysis(hafezFull, ago(60 * 24 * 5)),
    docs: [],
    chat: [],
    notes: [
      {
        id: "note-goethe",
        recordingId: "rec-hafez",
        start: hafezFull[11].start,
        end: hafezFull[11].end,
        body: "Read Goethe's *West–östlicher Divan* before the next session.",
        color: "blue",
        createdAt: ago(60 * 24 * 4),
        updatedAt: ago(60 * 24 * 4),
      },
    ],
    embeddingsIndexed: false,
    job: null,
    audioUrl: null,
  };

  // 3. Imported but not transcribed yet.
  const podcastFull = bankSegments("podcast");
  const podcast: RecState = {
    rec: makeRecording({
      id: "rec-podcast",
      title: "Podcast — Zehn-e Aram, episode 12",
      sourcePath: "/home/you/Downloads/zehn-e-aram-12.opus",
      durationSec: durationOf(podcastFull),
      createdAt: ago(55),
      updatedAt: ago(55),
      status: "new",
    }),
    bank: "podcast",
    full: podcastFull,
    segments: [],
    analysis: null,
    docs: [],
    chat: [],
    notes: [],
    embeddingsIndexed: false,
    job: null,
    audioUrl: null,
  };

  // 4. A format the backend can't decode: exercises the in-browser decode path.
  const memo: RecState = {
    rec: makeRecording({
      id: "rec-memo",
      title: "voice-memo",
      sourcePath: "/home/you/Recordings/voice-memo.amr",
      createdAt: ago(8),
      updatedAt: ago(8),
      status: "needsDecode",
    }),
    bank: "podcast",
    full: podcastFull,
    segments: [],
    analysis: null,
    docs: [],
    chat: [],
    notes: [],
    embeddingsIndexed: false,
    job: null,
    audioUrl: null,
  };

  for (const s of [meeting, hafez, podcast, memo]) {
    syncSummary(s);
    recs.set(s.rec.id, s);
  }

  libraryChat = [
    {
      id: "lmsg-1",
      recordingId: null,
      role: "user",
      content: "Which recordings talk about privacy or cost?",
      createdAt: ago(60 * 20),
      model: null,
    },
    {
      id: "lmsg-2",
      recordingId: null,
      role: "assistant",
      content:
        "Only the product planning meeting covers these topics:\n\n" +
        "- **Privacy**: users don't want to upload recordings to foreign servers, so on-device processing is an advantage [rec:rec-meeting#9-10].\n" +
        "- **Cost**: with cheaper models an AI summary costs less than a cent per meeting [rec:rec-meeting#30], and using a personal key keeps costs transparent [rec:rec-meeting#31].\n\n" +
        "The Hafez lecture doesn't mention either; it focuses on ghazals and ambiguity (*iham*) [rec:rec-hafez#4].",
      createdAt: ago(60 * 20),
      model: settings.llmModel,
    },
  ];
}

seed();

function getRec(id: string): RecState {
  const s = recs.get(id);
  if (!s) throw new Error("Recording not found. It may have been deleted.");
  return s;
}

function getModel(id: string): ModelInfo {
  const m = models.find((x) => x.id === id);
  if (!m) throw new Error(`Unknown model "${id}".`);
  return m;
}

function requireKey(): void {
  if (!settings.openrouterApiKey.trim()) {
    throw new Error("No OpenRouter API key. Add one in Settings to use AI features.");
  }
}

// ---- model installs -----------------------------------------------------------------

function progress(p: ModelProgress): void {
  emit(EV.model, p);
}

async function runInstall(model: ModelInfo, files: { name: string; size: number }[]): Promise<void> {
  const job = { cancelled: false };
  installs.set(model.id, job);
  const total = files.reduce((a, f) => a + f.size, 0);
  let done = 0;
  const cancelled = () => {
    if (!job.cancelled) return false;
    model.status = "notInstalled";
    installs.delete(model.id);
    progress({ modelId: model.id, stage: "cancelled", file: null, doneBytes: done, totalBytes: total, message: "Cancelled" });
    return true;
  };
  try {
    model.status = "downloading";
    model.error = null;
    for (const f of files) {
      const steps = Math.max(4, Math.round((f.size / total) * 40));
      for (let i = 1; i <= steps; i++) {
        await sleep(130);
        if (cancelled()) return;
        const fileDone = (f.size * i) / steps;
        progress({
          modelId: model.id,
          stage: "downloading",
          file: f.name,
          doneBytes: Math.round(done + fileDone),
          totalBytes: total,
          message: null,
        });
      }
      done += f.size;
    }
    if (model.engine === "whisper") {
      model.status = "converting";
      for (let i = 1; i <= 16; i++) {
        await sleep(150);
        if (cancelled()) return;
        progress({
          modelId: model.id,
          stage: "converting",
          file: "ggml-model-q8_0.bin",
          doneBytes: Math.round((model.diskBytes * i) / 16),
          totalBytes: model.diskBytes,
          message: `Converting for whisper.cpp (${settings.whisperQuant})…`,
        });
      }
    }
    progress({ modelId: model.id, stage: "verifying", file: null, doneBytes: total, totalBytes: total, message: "Verifying checksums…" });
    await sleep(500);
    if (cancelled()) return;
    model.status = "ready";
    installs.delete(model.id);
    progress({ modelId: model.id, stage: "done", file: null, doneBytes: total, totalBytes: total, message: null });
  } catch (e) {
    model.status = "error";
    model.error = String(e);
    installs.delete(model.id);
    progress({ modelId: model.id, stage: "error", file: null, doneBytes: done, totalBytes: total, message: model.error });
  }
}

// ---- transcription ------------------------------------------------------------------

function tprogress(p: TranscriptionProgress): void {
  emit(EV.transcription, p);
}

async function runTranscription(s: RecState, model: ModelInfo): Promise<void> {
  const job = { cancelled: false };
  s.job = job;
  const id = s.rec.id;
  const stop = () => {
    if (!job.cancelled) return false;
    s.job = null;
    s.segments = [];
    s.rec.status = "new";
    tprogress({ recordingId: id, stage: "cancelled", progress: 0, message: "Cancelled", newSegments: [] });
    touch(s);
    return true;
  };
  tprogress({ recordingId: id, stage: "queued", progress: 0, message: null, newSegments: [] });
  await sleep(500);
  if (stop()) return;
  s.rec.status = "transcribing";
  touch(s);
  for (let i = 1; i <= 5; i++) {
    tprogress({ recordingId: id, stage: "loadingModel", progress: i / 5, message: `Loading ${model.name}…`, newSegments: [] });
    await sleep(180);
    if (stop()) return;
  }
  const duration = s.rec.durationSec ?? durationOf(s.full);
  for (let i = 0; i < s.full.length; ) {
    const batch = s.full.slice(i, i + 1 + (i % 2));
    i += batch.length;
    await sleep(420);
    if (stop()) return;
    s.segments = [...s.segments, ...batch];
    s.rec.segmentCount = s.segments.length;
    const at = batch[batch.length - 1].end;
    tprogress({
      recordingId: id,
      stage: "transcribing",
      progress: Math.min(1, at / duration),
      message: `${formatTime(at)} / ${formatTime(duration)}`,
      newSegments: batch,
    });
  }
  tprogress({ recordingId: id, stage: "saving", progress: 1, message: "Saving transcript…", newSegments: [] });
  await sleep(300);
  if (stop()) return;
  s.job = null;
  s.rec.status = "ready";
  tprogress({ recordingId: id, stage: "done", progress: 1, message: null, newSegments: [] });
  touch(s);

  if (settings.autoAnalyze && settings.openrouterApiKey.trim()) {
    await sleep(1600);
    if (!recs.has(id) || s.rec.status !== "ready") return;
    s.analysis = bankAnalysis(s.bank, s.segments);
    touch(s);
  }
}

// ---- search -------------------------------------------------------------------------

function tokens(text: string): string[] {
  return normalizeForMatch(text)
    .split(/[\s.,،؛;:!?؟«»"'()\-–—]+/)
    .filter((t) => t.length > 2);
}

function hit(s: RecState, seg: Segment, score: number, kind: SearchHit["kind"], highlights: [number, number][]): SearchHit {
  return {
    recordingId: s.rec.id,
    recordingTitle: s.rec.title,
    segmentId: seg.id,
    start: seg.start,
    end: seg.end,
    text: seg.text,
    score,
    kind,
    highlights,
  };
}

function noteHits(s: RecState, q: string): SearchHit[] {
  const out: SearchHit[] = [];
  for (const n of s.notes) {
    const m = findMatches(n.body, q);
    if (!m.length) continue;
    const seg = s.segments[Math.max(0, findActiveIndex(s.segments, n.start))];
    out.push({
      recordingId: s.rec.id,
      recordingTitle: s.rec.title,
      segmentId: seg?.id ?? 0,
      start: n.start,
      end: n.end,
      text: n.body,
      score: Math.min(1, 0.7 + m.length * 0.1),
      kind: "note",
      noteId: n.id,
      highlights: m,
    });
  }
  return out;
}

function searchLibrary(query: string, recordingId: string | null, limit: number): SearchHit[] {
  const q = query.trim();
  if (!q) return [];
  const scope = [...recs.values()].filter((s) => (recordingId ? s.rec.id === recordingId : true));
  const out: SearchHit[] = [];
  const seen = new Set<string>();
  const qTokens = new Set(tokens(q));
  for (const s of scope) {
    out.push(...noteHits(s, q));
    for (const seg of s.segments) {
      const m = findMatches(seg.text, q);
      if (m.length) {
        out.push(hit(s, seg, Math.min(1, 0.6 + m.length * 0.2), "lexical", m));
        seen.add(`${s.rec.id}#${seg.id}`);
      }
    }
    for (const tag of s.analysis?.tags ?? []) {
      if (!normalizeForMatch(tag.name).includes(normalizeForMatch(q))) continue;
      for (const id of tag.segments.slice(0, 5)) {
        const seg = s.segments[id];
        const key = `${s.rec.id}#${id}`;
        if (!seg || seen.has(key)) continue;
        seen.add(key);
        out.push(hit(s, seg, 0.55, "tag", []));
      }
    }
    if (s.embeddingsIndexed && qTokens.size) {
      for (const seg of s.segments) {
        const key = `${s.rec.id}#${seg.id}`;
        if (seen.has(key)) continue;
        const overlap = tokens(seg.text).filter((t) => [...qTokens].some((qt) => t.startsWith(qt) || qt.startsWith(t)));
        if (!overlap.length) continue;
        seen.add(key);
        out.push(hit(s, seg, 0.4 + Math.min(0.4, overlap.length * 0.1), "semantic", []));
      }
    }
  }
  return out.sort((a, b) => b.score - a.score).slice(0, limit);
}

function related(id: string, segmentId: number, limit: number): SearchHit[] {
  const src = getRec(id);
  const seg = src.segments.find((x) => x.id === segmentId);
  if (!seg) throw new Error("Segment not found.");
  const base = new Set(tokens(seg.text));
  const out: SearchHit[] = [];
  for (const s of recs.values()) {
    for (const other of s.segments) {
      if (s === src && other.id === segmentId) continue;
      const t = new Set(tokens(other.text));
      const inter = [...t].filter((x) => base.has(x)).length;
      const union = new Set([...t, ...base]).size || 1;
      let score = inter / union;
      if (s === src && Math.abs(other.id - segmentId) === 1) score += 0.08;
      if (score <= 0.02) continue;
      out.push(hit(s, other, Math.min(1, score + 0.3), src.embeddingsIndexed ? "semantic" : "lexical", []));
    }
  }
  return out.sort((a, b) => b.score - a.score).slice(0, limit);
}

// ---- notes --------------------------------------------------------------------------

function noteFields(args: Record<string, unknown>): Pick<Note, "start" | "end" | "body" | "color"> {
  const start = num(args, "start");
  const end = num(args, "end");
  const body = str(args, "body").trim();
  const color = (args.color ?? null) as NoteColor | null;
  if (!body) throw new Error("A note can't be empty.");
  if (start < 0 || end < start) throw new Error("The note's time range is invalid.");
  if (color !== null && !NOTE_COLORS.includes(color)) throw new Error(`Unknown note color "${String(color)}".`);
  // Lets the browser preview exercise the failure paths: put [fail] in a note.
  if (body.includes("[fail]")) throw new Error("Simulated failure (mock): the note contains [fail].");
  return { start, end, body, color };
}

function findNote(id: string): { s: RecState; note: Note } {
  for (const s of recs.values()) {
    const note = s.notes.find((n) => n.id === id);
    if (note) return { s, note };
  }
  throw new Error("Note not found. It may have been deleted.");
}

// ---- LLM streaming ------------------------------------------------------------------

async function stream(requestId: string, chunks: string[], delay: number): Promise<void> {
  for (const delta of chunks) {
    if (cancelledRequests.has(requestId)) {
      cancelledRequests.delete(requestId);
      emit(EV.llm, { requestId, delta: null, done: true, error: "Cancelled" });
      throw new Error("Cancelled");
    }
    emit(EV.llm, { requestId, delta, done: false, error: null });
    await sleep(delay);
  }
  emit(EV.llm, { requestId, delta: null, done: true, error: null });
}

/** Mimic the agent's tool steps before the answer streams. */
async function agentSteps(requestId: string, labels: string[]): Promise<void> {
  for (const activity of labels) {
    if (cancelledRequests.has(requestId)) return;
    emit(EV.llm, { requestId, delta: null, done: false, error: null, activity });
    await sleep(450);
  }
}

function wordChunks(text: string): string[] {
  return text.match(/\S+\s*|\s+/g) ?? [text];
}

function sizedChunks(text: string, size: number): string[] {
  const out: string[] = [];
  for (let i = 0; i < text.length; i += size) out.push(text.slice(i, i + size));
  return out;
}

function bestSegments(segments: Segment[], question: string, n: number): Segment[] {
  const q = new Set(tokens(question));
  const scored = segments
    .map((s) => ({ s, score: tokens(s.text).filter((t) => [...q].some((x) => t.startsWith(x) || x.startsWith(t))).length }))
    .filter((x) => x.score > 0)
    .sort((a, b) => b.score - a.score)
    .slice(0, n)
    .map((x) => x.s);
  return (scored.length ? scored : segments.filter((_, i) => i % Math.max(1, Math.floor(segments.length / n)) === 0).slice(0, n)).sort(
    (a, b) => a.id - b.id,
  );
}

function shorten(text: string, words = 10): string {
  const w = text.split(/\s+/);
  return w.length > words ? `${w.slice(0, words).join(" ")}…` : text;
}

/** Segments of the first tag whose name appears in the question (lets English questions find Persian text). */
function tagSegments(states: RecState[], question: string): Segment[] {
  const q = normalizeForMatch(question);
  for (const s of states) {
    const tag = s.analysis?.tags.find((t) => q.includes(normalizeForMatch(t.name)));
    if (tag) return tag.segments.slice(0, 3).flatMap((id) => (s.segments[id] ? [s.segments[id]] : []));
  }
  return [];
}

function composeAnswer(segments: Segment[], question: string, cite: (s: Segment) => string, preferred: Segment[] = []): string {
  const picks = preferred.length ? preferred : bestSegments(segments, question, 3);
  if (!picks.length) return "I couldn't find anything about that in the transcript.";
  if (containsPersian(question)) {
    return (
      "بر اساس متن:\n\n" +
      picks.map((s, i) => `${i + 1}. در ${formatTime(s.start)} گفته می‌شود: «${shorten(s.text)}» ${cite(s)}`).join("\n") +
      "\n\nبرای شنیدن هر بخش روی شمارهٔ ارجاع کلیک کنید."
    );
  }
  return (
    "Here's what the transcript says about that:\n\n" +
    picks.map((s, i) => `${i + 1}. Around **${formatTime(s.start)}**: “${shorten(s.text)}” ${cite(s)}`).join("\n") +
    "\n\n_Click a citation to hear that moment._"
  );
}

// ---- handlers -----------------------------------------------------------------------

type Handler = (args: Record<string, unknown>) => unknown | Promise<unknown>;

const UNDECODABLE = new Set(["amr", "wma", "3gp"]);
const SUPPORTED = new Set(["mp3", "m4a", "aac", "wav", "flac", "ogg", "opus", "oga", "webm", "mp4", "mkv", "mov", "wma", "amr", "3gp"]);

export const handlers: Record<string, Handler> = {
  // ---- system & settings
  get_system_info: async () => {
    await sleep(120);
    return { ...MOCK_SYSTEM };
  },
  get_settings: async () => ({ ...settings }),
  save_settings: async (args) => {
    const next = args.settings as Settings;
    if (next.threads < 0 || next.threads > 256) throw new Error("Threads must be between 0 and 256.");
    settings = { ...next };
    await sleep(120);
    return { ...settings };
  },

  // ---- speech models
  list_models: async () => models.map((m) => ({ ...m, manualFiles: m.manualFiles.map((f) => ({ ...f })) })),
  install_model: async (args) => {
    const m = getModel(str(args, "modelId"));
    if (installs.has(m.id)) throw new Error(`${m.name} is already being installed.`);
    if (m.status === "ready") throw new Error(`${m.name} is already installed.`);
    void runInstall(
      m,
      m.manualFiles.map((f) => ({ name: f.name, size: f.sizeBytes ?? 1000 })),
    );
  },
  cancel_model_install: async (args) => {
    const job = installs.get(str(args, "modelId"));
    if (job) job.cancelled = true;
  },
  import_model_files: async (args) => {
    const m = getModel(str(args, "modelId"));
    const paths = strArray(args, "paths");
    if (!paths.length) throw new Error("No files selected.");
    if (installs.has(m.id)) throw new Error(`${m.name} is already being installed.`);
    void runInstall(m, [{ name: "importing files", size: 1 }]);
  },
  delete_model: async (args) => {
    const m = getModel(str(args, "modelId"));
    if (installs.has(m.id)) throw new Error("Cancel the install first.");
    m.status = "notInstalled";
    m.error = null;
  },

  // ---- library
  list_recordings: async () => {
    await sleep(80);
    return [...recs.values()].map(withAudio).sort((a, b) => b.createdAt.localeCompare(a.createdAt));
  },
  get_recording: async (args) => {
    const s = getRec(str(args, "id"));
    await sleep(120);
    return {
      recording: withAudio(s),
      segments: s.segments.map((x) => ({ ...x })),
      notes: s.notes.map((n) => ({ ...n })).sort(compareNotes),
      analysis: s.analysis,
      docs: [...s.docs],
      chat: [...s.chat],
      embeddingsIndexed: s.embeddingsIndexed,
    };
  },
  track_ui: () => undefined,

  import_link: async (args) => {
    const url = String(args.url ?? "");
    const m = /^https?:\/\/[^/]+\/([^/?#]+)/i.exec(url);
    if (!m) throw new Error("not a valid link");
    const [rec] = (await handlers.import_files({ paths: [`/home/you/Downloads/${m[1]}.mp3`] })) as Recording[];
    return rec;
  },

  import_files: async (args) => {
    const paths = strArray(args, "paths");
    const out: Recording[] = [];
    const rejected: string[] = [];
    for (const path of paths) {
      const name = path.split(/[\\/]/).pop() ?? path;
      const ext = name.includes(".") ? name.split(".").pop()!.toLowerCase() : "";
      if (!SUPPORTED.has(ext)) {
        rejected.push(name);
        continue;
      }
      const full = bankSegments("podcast");
      const needsDecode = UNDECODABLE.has(ext);
      const s: RecState = {
        rec: makeRecording({
          id: uid("rec"),
          title: name.replace(/\.[^.]+$/, ""),
          sourcePath: path,
          status: needsDecode ? "needsDecode" : "new",
          durationSec: needsDecode ? null : durationOf(full),
        }),
        bank: "podcast",
        full,
        segments: [],
        analysis: null,
        docs: [],
        chat: [],
        notes: [],
        embeddingsIndexed: false,
        job: null,
        audioUrl: null,
      };
      recs.set(s.rec.id, s);
      const r = withAudio(s);
      out.push(r);
      emit(EV.updated, r);
    }
    await sleep(200);
    if (rejected.length && !out.length) throw new Error(`Unsupported file type: ${rejected.join(", ")}`);
    return out;
  },
  submit_decoded_audio: async (args) => {
    const s = getRec(str(args, "id"));
    await sleep(500);
    s.rec.status = "new";
    s.rec.durationSec = durationOf(s.full);
    touch(s);
    return withAudio(s);
  },
  rename_recording: async (args) => {
    const s = getRec(str(args, "id"));
    const title = str(args, "title").trim();
    if (!title) throw new Error("The title can't be empty.");
    s.rec.title = title;
    touch(s);
    return withAudio(s);
  },
  delete_recording: async (args) => {
    const id = str(args, "id");
    const s = getRec(id);
    if (s.job) s.job.cancelled = true;
    recs.delete(id);
    if (s.audioUrl) URL.revokeObjectURL(s.audioUrl);
    emit(EV.deleted, id);
  },
  transcribe: async (args) => {
    const s = getRec(str(args, "id"));
    const modelId = optStr(args, "modelId") ?? settings.defaultModelId;
    if (!models.some((m) => m.status === "ready")) {
      throw new Error("No speech model is installed yet. Install one from the Models page.");
    }
    const model = getModel(modelId);
    if (model.status !== "ready") throw new Error(`${model.name} isn't installed. Pick another model or install it first.`);
    if (s.rec.status === "needsDecode") throw new Error("This file has to be decoded before it can be transcribed.");
    if (s.job) throw new Error("This recording is already being transcribed.");
    s.segments = [];
    s.analysis = null;
    s.embeddingsIndexed = false;
    s.rec.modelId = model.id;
    s.rec.status = "queued";
    s.rec.error = null;
    touch(s);
    void runTranscription(s, model);
  },
  cancel_transcription: async (args) => {
    const s = getRec(str(args, "id"));
    if (s.job) s.job.cancelled = true;
  },
  update_segment_text: async (args) => {
    const s = getRec(str(args, "id"));
    const segmentId = num(args, "segmentId");
    const text = str(args, "text").trim();
    if (!text) throw new Error("A sentence can't be empty.");
    const seg = s.segments.find((x) => x.id === segmentId);
    if (!seg) throw new Error("Segment not found.");
    const sameCount = text.split(/\s+/).length === seg.words.length;
    const updated: Segment = {
      ...seg,
      text,
      words: sameCount ? seg.words.map((w, i) => ({ ...w, text: text.split(/\s+/)[i] })) : [],
    };
    s.segments = s.segments.map((x) => (x.id === segmentId ? updated : x));
    touch(s);
    return updated;
  },
  get_waveform: async (args) => {
    const s = getRec(str(args, "id"));
    if (!s.rec.durationSec) throw new Error("The waveform is available once the audio has been decoded.");
    await sleep(150);
    const perSecond = 20;
    const wf: Waveform = {
      peaks: buildPeaks(s.full, s.rec.durationSec, perSecond, s.rec.id.length * 7 + 3),
      peaksPerSecond: perSecond,
      durationSec: s.rec.durationSec,
    };
    return wf;
  },
  prepare_playback_wav: async (args) => {
    const s = getRec(str(args, "id"));
    if (!s.rec.durationSec) throw new Error("Playback isn't available until the audio has been decoded.");
    await sleep(300);
    withAudio(s);
    if (!s.audioUrl) throw new Error("Could not create a playable copy.");
    s.rec.playbackWavPath = s.audioUrl;
    return s.audioUrl;
  },
  export_transcript: async (args) => {
    const s = getRec(str(args, "id"));
    const format = str(args, "format") as ExportFormat;
    str(args, "path");
    if (!s.segments.length) throw new Error("There's no transcript to export yet.");
    if (!["txt", "srt", "vtt", "json", "md"].includes(format)) throw new Error(`Unknown format ${format}`);
    await sleep(250);
  },

  // ---- notes
  add_note: async (args) => {
    const s = getRec(str(args, "recordingId"));
    const fields = noteFields(args);
    await sleep(220);
    const at = nowIso();
    const note: Note = { id: uid("note"), recordingId: s.rec.id, ...fields, createdAt: at, updatedAt: at };
    s.notes.push(note);
    return { ...note };
  },
  update_note: async (args) => {
    const { s, note } = findNote(str(args, "id"));
    const fields = noteFields(args);
    await sleep(220);
    const updated: Note = { ...note, ...fields, updatedAt: nowIso() };
    s.notes = s.notes.map((n) => (n.id === note.id ? updated : n));
    return { ...updated };
  },
  delete_note: async (args) => {
    const { s, note } = findNote(str(args, "id"));
    await sleep(150);
    s.notes = s.notes.filter((n) => n.id !== note.id);
  },
  list_all_notes: async () => {
    await sleep(120);
    const out: NoteWithRecording[] = [];
    const newestFirst = [...recs.values()].sort((a, b) => b.rec.createdAt.localeCompare(a.rec.createdAt));
    for (const s of newestFirst) {
      for (const n of [...s.notes].sort(compareNotes)) out.push({ ...n, recordingTitle: s.rec.title });
    }
    return out;
  },

  // ---- search
  search: async (args) => {
    await sleep(90);
    return searchLibrary(str(args, "query"), optStr(args, "recordingId"), num(args, "limit", 50));
  },
  related_segments: async (args) => {
    await sleep(250);
    return related(str(args, "id"), num(args, "segmentId"), num(args, "limit", 10));
  },
  list_tags: async () => {
    const map = new Map<string, TagSummary>();
    for (const s of recs.values()) {
      for (const t of s.analysis?.tags ?? []) {
        const cur = map.get(t.name) ?? { name: t.name, recordingCount: 0, segmentCount: 0 };
        cur.recordingCount += 1;
        cur.segmentCount += t.segments.length;
        map.set(t.name, cur);
      }
    }
    return [...map.values()].sort((a, b) => b.segmentCount - a.segmentCount);
  },
  segments_for_tag: async (args) => {
    const tag = str(args, "tag");
    const out: SearchHit[] = [];
    for (const s of recs.values()) {
      const t = s.analysis?.tags.find((x) => x.name === tag);
      if (!t) continue;
      for (const id of t.segments) {
        const seg = s.segments[id];
        if (seg) out.push(hit(s, seg, 1, "tag", []));
      }
    }
    return out;
  },

  // ---- AI
  list_llm_models: async () => {
    await sleep(350);
    return MOCK_LLM_MODELS.map((m) => ({ ...m }));
  },
  test_api_key: async (args) => {
    const key = str(args, "apiKey").trim();
    await sleep(600);
    if (!key) throw new Error("Enter a key first.");
    if (key.length < 8) throw new Error("OpenRouter rejected this key (401 Unauthorized).");
    return "Key works — $4.82 of credit remaining (mock).";
  },
  analyze: async (args) => {
    requireKey();
    const s = getRec(str(args, "id"));
    if (!s.segments.length) throw new Error("Transcribe the recording before analyzing it.");
    await sleep(1800);
    s.analysis = bankAnalysis(s.bank, s.segments);
    s.analysis.model = settings.llmModel;
    touch(s);
    return s.analysis;
  },
  index_embeddings: async (args) => {
    requireKey();
    const s = getRec(str(args, "id"));
    if (!s.segments.length) throw new Error("Nothing to index yet.");
    await sleep(1400);
    s.embeddingsIndexed = true;
    touch(s);
  },
  ask: async (args) => {
    requireKey();
    const recordingId = optStr(args, "recordingId");
    const question = str(args, "question").trim();
    const requestId = str(args, "requestId");
    if (!question) throw new Error("Type a question first.");
    const userMsg: ChatMessage = { id: uid("msg"), recordingId, role: "user", content: question, createdAt: nowIso(), model: null };
    let answer: string;
    if (recordingId) {
      const s = getRec(recordingId);
      if (!s.segments.length) throw new Error("This recording has no transcript yet.");
      s.chat.push(userMsg);
      answer = composeAnswer(s.segments, question, (seg) => `[${seg.id}]`, tagSegments([s], question));
    } else {
      libraryChat.push(userMsg);
      const all = [...recs.values()].flatMap((s) => s.segments.map((seg) => ({ seg, s })));
      const preferred = tagSegments([...recs.values()], question);
      const picks = preferred.length ? preferred : bestSegments(all.map((x) => x.seg), question, 3);
      const owner = (seg: Segment) => all.find((x) => x.seg === seg)!.s;
      answer = composeAnswer(picks, question, (seg) => `[rec:${owner(seg).rec.id}#${seg.id}]`, picks);
    }
    await agentSteps(
      requestId,
      recordingId ? ["Reading the overview", `Searching for “${shorten(question, 4)}”`] : [`Searching for “${shorten(question, 4)}”`],
    );
    await stream(requestId, wordChunks(answer), 35);
    const msg: ChatMessage = {
      id: uid("msg"),
      recordingId,
      role: "assistant",
      content: answer,
      createdAt: nowIso(),
      model: settings.llmModel,
    };
    if (recordingId) recs.get(recordingId)?.chat.push(msg);
    else libraryChat.push(msg);
    return msg;
  },
  clear_chat: async (args) => {
    const recordingId = optStr(args, "recordingId");
    if (recordingId) getRec(recordingId).chat = [];
    else libraryChat = [];
  },
  list_library_chat: async () => [...libraryChat],
  generate_doc: async (args) => {
    requireKey();
    const s = getRec(str(args, "id"));
    const prompt = str(args, "prompt").trim();
    const requestId = str(args, "requestId");
    if (!prompt) throw new Error("Describe the doc you want first.");
    if (!s.segments.length) throw new Error("Transcribe the recording first.");
    const kind = docKindFromPrompt(prompt);
    const html = mockDocHtml(kind, 200 + Math.floor(Math.random() * 120));
    const skill = optStr(args, "skill");
    await agentSteps(requestId, [
      ...(skill ? [] : [`Loading the ${kind} guide`]),
      "Reading the overview",
      "Reading the transcript",
    ]);
    await stream(requestId, sizedChunks(html, 160), 18);
    const doc: Doc = {
      id: uid("doc"),
      recordingId: s.rec.id,
      title: docTitle(kind),
      prompt,
      html,
      model: settings.llmModel,
      createdAt: nowIso(),
      updatedAt: nowIso(),
    };
    s.docs.unshift(doc);
    return doc;
  },
  revise_doc: async (args) => {
    requireKey();
    const docId = str(args, "docId");
    const instruction = str(args, "instruction").trim();
    const requestId = str(args, "requestId");
    if (!instruction) throw new Error("Describe the change first.");
    const owner = [...recs.values()].find((s) => s.docs.some((d) => d.id === docId));
    const doc = owner?.docs.find((d) => d.id === docId);
    if (!owner || !doc) throw new Error("Doc not found.");
    const hue = Math.floor(Math.random() * 360);
    const html = doc.html.replace(/--accent: hsl\(\d+/, `--accent: hsl(${hue}`) + `\n<!-- revised: ${instruction.replace(/-->/g, "")} -->`;
    await sleep(500);
    await stream(requestId, sizedChunks(html, 200), 14);
    const updated: Doc = { ...doc, html, updatedAt: nowIso(), model: settings.llmModel };
    owner.docs = owner.docs.map((d) => (d.id === docId ? updated : d));
    return updated;
  },
  delete_doc: async (args) => {
    const docId = str(args, "docId");
    for (const s of recs.values()) s.docs = s.docs.filter((d) => d.id !== docId);
  },
  cancel_llm: async (args) => {
    cancelledRequests.add(str(args, "requestId"));
  },
};
