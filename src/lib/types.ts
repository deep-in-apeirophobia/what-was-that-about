// Shared data contract between the Rust backend (src-tauri) and the UI.
// Field names are camelCase on the wire (serde `rename_all = "camelCase"`).
// Keep this file in sync with `crates/wwta-core/src/types.rs`.

export type EngineKind = "whisper" | "shenava";

export type ModelStatus =
  | "notInstalled"
  | "downloading"
  | "converting"
  | "ready"
  | "error";

/** A speech-to-text model the app knows how to install and run. */
export interface ModelInfo {
  id: string; // "taraneh-wlarge" | "shenava-koochik" | ...
  name: string;
  engine: EngineKind;
  description: string;
  license: string;
  /** Short hint shown as a badge, e.g. "Best accuracy, GPU" */
  recommendedFor: string;
  /** Approximate bytes downloaded when installing. */
  downloadBytes: number;
  /** Approximate bytes on disk once installed. */
  diskBytes: number;
  homepage: string;
  status: ModelStatus;
  /** Set when status is "error". */
  error: string | null;
  /** Absolute directory holding the model files once installed. */
  installDir: string;
  /** Files the user can download by hand and then import with `importModelFiles`. */
  manualFiles: ManualFile[];
  /** Human-readable instructions for manual install (markdown). */
  manualInstructions: string;
}

export interface ManualFile {
  name: string; // file name expected by the importer, e.g. "model.onnx"
  url: string; // direct download URL
  sizeBytes: number | null;
  required: boolean;
}

export type ModelProgressStage =
  | "downloading"
  | "converting"
  | "verifying"
  | "done"
  | "error"
  | "cancelled";

/** Event `model://progress` */
export interface ModelProgress {
  modelId: string;
  stage: ModelProgressStage;
  /** File currently being processed (download or conversion step). */
  file: string | null;
  doneBytes: number;
  totalBytes: number;
  message: string | null;
}

export type RecordingStatus =
  | "new" // imported, not transcribed yet
  | "needsDecode" // backend could not decode the format; UI should try `decodeInBrowser`
  | "queued"
  | "transcribing"
  | "ready"
  | "error";

export interface Recording {
  id: string;
  title: string;
  /** Original path the user imported. */
  sourcePath: string;
  /** File the UI should play (inside the library when copied). */
  audioPath: string;
  /** Lossless 16-bit WAV fallback for playback, created on demand. */
  playbackWavPath: string | null;
  durationSec: number | null;
  createdAt: string; // ISO 8601
  updatedAt: string; // ISO 8601
  status: RecordingStatus;
  modelId: string | null;
  error: string | null;
  segmentCount: number;
  hasAnalysis: boolean;
  /** Top tags from the AI analysis, for library chips. */
  tags: string[];
  /** Short AI summary (first paragraph), or null. */
  blurb: string | null;
}

export interface Word {
  start: number; // seconds
  end: number; // seconds
  text: string;
}

export interface Segment {
  /** 0-based index within the recording; stable id used for citations. */
  id: number;
  start: number; // seconds
  end: number; // seconds
  text: string;
  /** Word timings when the engine provides them. */
  words: Word[];
  /** What speech recognition produced, when an AI fix changed `text`. */
  originalText?: string | null;
}

export interface Chapter {
  title: string;
  startSegment: number;
  endSegment: number; // inclusive
  start: number; // seconds (filled by backend from segments)
  end: number; // seconds
  summary: string;
  tags: string[];
}

export interface Tag {
  name: string;
  description: string;
  /** Segment ids where this topic is discussed. */
  segments: number[];
}

export interface Analysis {
  title: string | null;
  summary: string; // markdown
  keyPoints: string[];
  chapters: Chapter[];
  tags: Tag[];
  actionItems: string[];
  /** Suggested follow-up questions the user can click in the chat. */
  questions: string[];
  model: string;
  language: string;
  createdAt: string;
}

export interface ChatMessage {
  id: string;
  /** null = library-wide conversation */
  recordingId: string | null;
  threadId: string;
  role: "user" | "assistant";
  /** Markdown. Citations look like [12] or [12-15] (segment ids), or
   *  [rec:<recordingId>#12] in library-wide chats. May be empty when only files were sent. */
  content: string;
  createdAt: string;
  model: string | null;
  /** Images and voice clips sent with a user message. */
  attachments: ChatAttachment[];
  /** Set on answers whose context had to be reduced (e.g. a condensed transcript). */
  contextNote: string | null;
}

/** An image or voice clip attached to a chat message (stored in the app's data dir). */
export interface ChatAttachment {
  id: string;
  kind: "image" | "audio";
  name: string;
  mime: string;
  sizeBytes: number;
  durationSec: number | null;
  path: string;
  /** Text stand-in for models that can't take the file: a voice transcript or an image
   *  description, made the first time such a model needs it. */
  text: string | null;
  /** Model that produced `text` (a speech model id or an OpenRouter model id). */
  textModel: string | null;
}

/** One conversation in an Ask view. */
export interface ChatThread {
  id: string;
  /** null = Ask your library */
  recordingId: string | null;
  title: string;
  createdAt: string;
  updatedAt: string;
  messageCount: number;
  /** Summary of the first `compactedCount` messages, which the model gets instead of them. */
  summary: string | null;
  compactedCount: number;
}

export interface AskResult {
  thread: ChatThread;
  question: ChatMessage;
  answer: ChatMessage;
}

/** Result of attaching a file from disk. "convert": the webview must decode the audio /
 *  shrink the image itself and upload the result (the path is readable via mediaUrl). */
export type AttachmentUpload =
  | { status: "ready"; attachment: ChatAttachment }
  | { status: "convert"; path: string; kind: "image" | "audio" };

export interface Doc {
  id: string;
  recordingId: string;
  title: string;
  prompt: string;
  /** Full self-contained HTML document produced by the LLM. */
  html: string;
  model: string;
  createdAt: string;
  updatedAt: string;
}

/** Palette keys for note colors (the UI maps them to theme colors). */
export const NOTE_COLORS = ["yellow", "green", "blue", "purple", "red"] as const;
export type NoteColor = (typeof NOTE_COLORS)[number];

/** A user note/comment attached to a stretch of a recording. Anchored by time (not
 *  segment ids) so it survives re-transcription and transcript edits. */
export interface Note {
  id: string;
  recordingId: string;
  start: number; // seconds
  end: number; // seconds
  /** Markdown. Plain URLs and [text](url) become links; [rec:<id>#12], [12] chips work like in chat. */
  body: string;
  color: NoteColor | null;
  createdAt: string;
  updatedAt: string;
}

export interface NoteWithRecording extends Note {
  recordingTitle: string;
}

export interface RecordingDetail {
  recording: Recording;
  segments: Segment[];
  /** User notes, sorted by start time. */
  notes: Note[];
  analysis: Analysis | null;
  docs: Doc[];
  /** Ask conversations about this recording, most recent first. */
  threads: ChatThread[];
  /** True when semantic (embedding) search is available for this recording. */
  embeddingsIndexed: boolean;
}

export interface Waveform {
  /** Normalized peak values 0..1, `peaksPerSecond` per second of audio. */
  peaks: number[];
  peaksPerSecond: number;
  durationSec: number;
}

export type TranscriptionStage =
  | "queued"
  | "decoding"
  | "loadingModel"
  | "transcribing"
  | "saving"
  | "done"
  | "error"
  | "cancelled";

/** Event `transcription://progress` */
export interface TranscriptionProgress {
  recordingId: string;
  stage: TranscriptionStage;
  /** 0..1 for the current stage */
  progress: number;
  message: string | null;
  /** Newly finalized segments since the last event (live preview). */
  newSegments: Segment[];
}

/** Event `transcript://fix-progress` — one batch of an AI transcript fix finished. */
export interface FixProgress {
  recordingId: string;
  /** Batches finished (including failed ones) out of `total`. */
  done: number;
  total: number;
  /** Segments this batch changed, with `originalText` set. */
  changed: Segment[];
}

export interface FixSummary {
  /** Sentences whose text changed. */
  changed: number;
  batches: number;
  /** Batches that failed; their sentences were left as they were. */
  failed: number;
  /** The first failure, when some batches failed. */
  error: string | null;
}

export interface SearchHit {
  recordingId: string;
  recordingTitle: string;
  segmentId: number;
  start: number;
  end: number;
  text: string;
  /** Higher is better. Lexical hits are 0..1, semantic hits are cosine similarity. */
  score: number;
  kind: "lexical" | "semantic" | "tag" | "note";
  /** Set for kind "note". */
  noteId?: string | null;
  /** Ranges inside `text` to highlight, as [startCharIndex, endCharIndex) pairs (UTF-16 indices). */
  highlights: [number, number][];
}

export interface TagSummary {
  name: string;
  recordingCount: number;
  segmentCount: number;
}

export type AiLanguage = "auto" | "en" | "fa";
export type Theme = "system" | "light" | "dark";
export type WhisperQuant = "q8_0" | "f16";

export interface Settings {
  openrouterApiKey: string;
  llmModel: string;
  embeddingModel: string;
  aiLanguage: AiLanguage;
  defaultModelId: string;
  useGpu: boolean;
  /** 0 = automatic */
  threads: number;
  copyIntoLibrary: boolean;
  theme: Theme;
  whisperQuant: WhisperQuant;
  /** Keep the original safetensors after converting Whisper models. */
  keepSourceWeights: boolean;
  /** Convert spoken numbers to digits in Shenava output. */
  persianDigits: boolean;
  /** Use Whisper DTW for precise word timings (slower). */
  preciseWordTimings: boolean;
  /** Automatically run AI analysis after a transcription finishes (needs API key). */
  autoAnalyze: boolean;
  /** Anonymous usage statistics (PostHog): counts, models, formats, durations. Never content. */
  analyticsEnabled: boolean;
  /** Crash and error reports (Sentry). Turning it on takes effect after a restart. */
  crashReportsEnabled: boolean;
}

export interface GpuDevice {
  name: string;
  description: string;
  memoryBytes: number | null;
}

export interface SystemInfo {
  os: string; // "windows" | "linux" | "macos"
  arch: string;
  appVersion: string;
  cpuCores: number;
  totalMemoryBytes: number;
  /** Compiled GPU backend for Whisper. */
  gpuBackend: "vulkan" | "metal" | "none";
  /** False when e.g. the Vulkan loader is missing on this machine. */
  gpuAvailable: boolean;
  gpuDevices: GpuDevice[];
  dataDir: string;
  engines: { whisper: boolean; shenava: boolean };
}

export interface LlmModelInfo {
  id: string;
  name: string;
  contextLength: number | null;
  /** USD per 1M tokens */
  promptPrice: number | null;
  completionPrice: number | null;
  /** "text", "image", "audio", "file"… */
  inputModalities: string[];
  /** Unix seconds when OpenRouter added the model. */
  created: number | null;
}

export type AnalysisPart = "summary" | "chapters" | "tags";

/** Event `llm://stream` — streamed tokens and agent progress for chat answers and docs. */
export interface LlmStreamEvent {
  requestId: string;
  delta: string | null;
  done: boolean;
  error: string | null;
  /** What the request is doing before the answer streams ("Transcribing the voice message…",
   *  "Reading the transcript…"). */
  status?: string | null;
  /** Discard the text streamed so far: it was the model's preface to tool calls. */
  reset?: boolean;
}

export type ExportFormat = "txt" | "srt" | "vtt" | "json" | "md";
