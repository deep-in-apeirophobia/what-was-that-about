// Typed wrappers around the Tauri commands exposed by src-tauri/src/commands.rs.
// When the UI runs outside Tauri (plain `pnpm dev` in a browser), calls are
// routed to an in-memory mock so the interface can be developed without the backend.

import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen as tauriListen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AnalysisPart,
  ChatMessage,
  Doc,
  ExportFormat,
  LlmModelInfo,
  LlmStreamEvent,
  ModelInfo,
  ModelProgress,
  Recording,
  RecordingDetail,
  SearchHit,
  Segment,
  Settings,
  SystemInfo,
  TagSummary,
  TranscriptionProgress,
  Waveform,
  Analysis,
  Note,
  NoteColor,
  NoteWithRecording,
} from "./types";

export const isTauri = (): boolean =>
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

type Handler = (args: Record<string, unknown>) => unknown | Promise<unknown>;
let mockHandlers: Record<string, Handler> | null = null;
let mockEmitter: ((event: string, cb: (payload: unknown) => void) => () => void) | null = null;

async function loadMock() {
  if (!mockHandlers) {
    const m = await import("./mock");
    mockHandlers = m.handlers;
    mockEmitter = m.subscribe;
  }
}

async function invoke<T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> {
  if (isTauri()) return tauriInvoke<T>(cmd, args);
  await loadMock();
  const h = mockHandlers![cmd];
  if (!h) throw new Error(`mock: no handler for ${cmd}`);
  return (await h(args)) as T;
}

export async function listen<T>(event: string, cb: (payload: T) => void): Promise<UnlistenFn> {
  if (isTauri()) return tauriListen<T>(event, (e) => cb(e.payload));
  await loadMock();
  return mockEmitter!(event, cb as (p: unknown) => void);
}

export const EVENTS = {
  modelProgress: "model://progress",
  transcriptionProgress: "transcription://progress",
  llmStream: "llm://stream",
  /** Payload: Recording — emitted whenever a recording row changes. */
  recordingUpdated: "library://recording-updated",
  /** Payload: string (recording id) */
  recordingDeleted: "library://recording-deleted",
} as const;

export const onModelProgress = (cb: (p: ModelProgress) => void) => listen(EVENTS.modelProgress, cb);
export const onTranscriptionProgress = (cb: (p: TranscriptionProgress) => void) =>
  listen(EVENTS.transcriptionProgress, cb);
export const onLlmStream = (cb: (p: LlmStreamEvent) => void) => listen(EVENTS.llmStream, cb);
export const onRecordingUpdated = (cb: (r: Recording) => void) => listen(EVENTS.recordingUpdated, cb);
export const onRecordingDeleted = (cb: (id: string) => void) => listen(EVENTS.recordingDeleted, cb);

export const api = {
  // ---- system & settings
  getSystemInfo: () => invoke<SystemInfo>("get_system_info"),
  getSettings: () => invoke<Settings>("get_settings"),
  saveSettings: (settings: Settings) => invoke<Settings>("save_settings", { settings }),

  // ---- speech models
  listModels: () => invoke<ModelInfo[]>("list_models"),
  /** Starts a background install (download + conversion). Progress via `model://progress`. */
  installModel: (modelId: string) => invoke<void>("install_model", { modelId }),
  cancelModelInstall: (modelId: string) => invoke<void>("cancel_model_install", { modelId }),
  /** Install from files the user downloaded by hand. `paths` are absolute file paths.
   *  Runs conversion when needed; progress via `model://progress`. */
  importModelFiles: (modelId: string, paths: string[]) =>
    invoke<void>("import_model_files", { modelId, paths }),
  deleteModel: (modelId: string) => invoke<void>("delete_model", { modelId }),

  // ---- library
  listRecordings: () => invoke<Recording[]>("list_recordings"),
  getRecording: (id: string) => invoke<RecordingDetail>("get_recording", { id }),
  /** Import audio/video files. Returns the new recordings. */
  importFiles: (paths: string[]) => invoke<Recording[]>("import_files", { paths }),
  /** Import an Adobe Connect recording from its link; the source archive downloads in the
   *  background (progress via `transcription://progress`, stage "decoding"). */
  importLink: (url: string) => invoke<Recording>("import_link", { url }),
  /** For recordings in status "needsDecode": the UI decoded the file with Web Audio
   *  and sends mono float32 PCM. Body is sent as raw bytes for speed. */
  submitDecodedAudio: (id: string, samples: Float32Array, sampleRate: number) =>
    isTauri()
      ? tauriInvoke<Recording>("submit_decoded_audio", new Uint8Array(samples.buffer), {
          headers: { "x-recording-id": id, "x-sample-rate": String(sampleRate) },
        })
      : invoke<Recording>("submit_decoded_audio", { id, sampleRate, length: samples.length }),
  renameRecording: (id: string, title: string) => invoke<Recording>("rename_recording", { id, title }),
  deleteRecording: (id: string) => invoke<void>("delete_recording", { id }),
  /** Queue a transcription. `modelId` null = settings.defaultModelId. */
  transcribe: (id: string, modelId: string | null) => invoke<void>("transcribe", { id, modelId }),
  cancelTranscription: (id: string) => invoke<void>("cancel_transcription", { id }),
  updateSegmentText: (id: string, segmentId: number, text: string) =>
    invoke<Segment>("update_segment_text", { id, segmentId, text }),
  getWaveform: (id: string) => invoke<Waveform>("get_waveform", { id }),
  /** Writes a 16-bit WAV copy for webviews that cannot play the original format. */
  preparePlaybackWav: (id: string) => invoke<string>("prepare_playback_wav", { id }),
  exportTranscript: (id: string, format: ExportFormat, path: string) =>
    invoke<void>("export_transcript", { id, format, path }),

  // ---- notes
  addNote: (recordingId: string, start: number, end: number, body: string, color: NoteColor | null) =>
    invoke<Note>("add_note", { recordingId, start, end, body, color }),
  /** Saves all editable fields of the note. */
  updateNote: (note: Pick<Note, "id" | "start" | "end" | "body" | "color">) =>
    invoke<Note>("update_note", { id: note.id, start: note.start, end: note.end, body: note.body, color: note.color }),
  deleteNote: (id: string) => invoke<void>("delete_note", { id }),
  /** Every note in the library, newest recording first, then by time. */
  listAllNotes: () => invoke<NoteWithRecording[]>("list_all_notes"),

  // ---- search
  /** Lexical search (always available) merged with semantic search when embeddings exist.
   *  `recordingId` null = whole library. */
  search: (query: string, recordingId: string | null, limit = 50) =>
    invoke<SearchHit[]>("search", { query, recordingId, limit }),
  /** Segments similar to the given one, across the library (needs embeddings; falls back to lexical similarity). */
  relatedSegments: (id: string, segmentId: number, limit = 10) =>
    invoke<SearchHit[]>("related_segments", { id, segmentId, limit }),
  listTags: () => invoke<TagSummary[]>("list_tags"),
  segmentsForTag: (tag: string) => invoke<SearchHit[]>("segments_for_tag", { tag }),

  // ---- AI (OpenRouter)
  listLlmModels: () => invoke<LlmModelInfo[]>("list_llm_models"),
  /** Validates the key by calling OpenRouter. Returns credit info text. */
  testApiKey: (apiKey: string) => invoke<string>("test_api_key", { apiKey }),
  analyze: (id: string, parts: AnalysisPart[] = ["summary", "chapters", "tags"]) =>
    invoke<Analysis>("analyze", { id, parts }),
  /** Build the embedding index for semantic search. */
  indexEmbeddings: (id: string) => invoke<void>("index_embeddings", { id }),
  /** Ask a question. Streams tokens on `llm://stream` with the given requestId,
   *  resolves with the stored assistant message. `recordingId` null = ask the whole library. */
  ask: (recordingId: string | null, question: string, requestId: string) =>
    invoke<ChatMessage>("ask", { recordingId, question, requestId }),
  clearChat: (recordingId: string | null) => invoke<void>("clear_chat", { recordingId }),
  listLibraryChat: () => invoke<ChatMessage[]>("list_library_chat"),
  /** Generate an interactive HTML doc. Streams on `llm://stream`. */
  generateDoc: (id: string, prompt: string, requestId: string) =>
    invoke<Doc>("generate_doc", { id, prompt, requestId }),
  /** Revise an existing doc with an instruction. */
  reviseDoc: (docId: string, instruction: string, requestId: string) =>
    invoke<Doc>("revise_doc", { docId, instruction, requestId }),
  deleteDoc: (docId: string) => invoke<void>("delete_doc", { docId }),
  cancelLlm: (requestId: string) => invoke<void>("cancel_llm", { requestId }),
};

export type Api = typeof api;
