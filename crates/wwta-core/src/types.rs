//! Wire types shared with the UI. Keep in sync with `src/lib/types.ts`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EngineKind {
    Whisper,
    Shenava,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ModelStatus {
    NotInstalled,
    Downloading,
    Converting,
    Ready,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManualFile {
    pub name: String,
    pub url: String,
    pub size_bytes: Option<u64>,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub engine: EngineKind,
    pub description: String,
    pub license: String,
    pub recommended_for: String,
    pub download_bytes: u64,
    pub disk_bytes: u64,
    pub homepage: String,
    pub status: ModelStatus,
    pub error: Option<String>,
    pub install_dir: String,
    pub manual_files: Vec<ManualFile>,
    pub manual_instructions: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ModelProgressStage {
    Downloading,
    Converting,
    Verifying,
    Done,
    Error,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelProgress {
    pub model_id: String,
    pub stage: ModelProgressStage,
    pub file: Option<String>,
    pub done_bytes: u64,
    pub total_bytes: u64,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecordingStatus {
    New,
    NeedsDecode,
    Queued,
    Transcribing,
    Ready,
    Error,
}

impl RecordingStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::NeedsDecode => "needsDecode",
            Self::Queued => "queued",
            Self::Transcribing => "transcribing",
            Self::Ready => "ready",
            Self::Error => "error",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "needsDecode" => Self::NeedsDecode,
            "queued" => Self::Queued,
            "transcribing" => Self::Transcribing,
            "ready" => Self::Ready,
            "error" => Self::Error,
            _ => Self::New,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Recording {
    pub id: String,
    pub title: String,
    pub source_path: String,
    pub audio_path: String,
    pub playback_wav_path: Option<String>,
    pub duration_sec: Option<f64>,
    pub created_at: String,
    pub updated_at: String,
    pub status: RecordingStatus,
    pub model_id: Option<String>,
    pub error: Option<String>,
    pub segment_count: u32,
    pub has_analysis: bool,
    pub tags: Vec<String>,
    pub blurb: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Word {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    pub id: u32,
    pub start: f64,
    pub end: f64,
    pub text: String,
    #[serde(default)]
    pub words: Vec<Word>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Chapter {
    pub title: String,
    pub start_segment: u32,
    pub end_segment: u32,
    #[serde(default)]
    pub start: f64,
    #[serde(default)]
    pub end: f64,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Tag {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub segments: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Analysis {
    pub title: Option<String>,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub key_points: Vec<String>,
    #[serde(default)]
    pub chapters: Vec<Chapter>,
    #[serde(default)]
    pub tags: Vec<Tag>,
    #[serde(default)]
    pub action_items: Vec<String>,
    #[serde(default)]
    pub questions: Vec<String>,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub id: String,
    pub recording_id: Option<String>,
    #[serde(default)]
    pub thread_id: String,
    pub role: String,
    pub content: String,
    pub created_at: String,
    pub model: Option<String>,
    /// Images and voice clips sent with a user message.
    #[serde(default)]
    pub attachments: Vec<ChatAttachment>,
    /// Set on answers whose context had to be reduced, e.g. "condensed transcript".
    #[serde(default)]
    pub context_note: Option<String>,
}

/// An image or voice clip attached to a chat message. Files live in the app's data dir.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatAttachment {
    pub id: String,
    /// "image" | "audio"
    pub kind: String,
    pub name: String,
    pub mime: String,
    pub size_bytes: u64,
    pub duration_sec: Option<f64>,
    pub path: String,
    /// Text stand-in for models that can't take this input: the transcript of a voice clip or a
    /// description of an image. Filled the first time such a model needs it.
    pub text: Option<String>,
    /// Model that produced `text` (a speech model id or an OpenRouter model id).
    pub text_model: Option<String>,
}

/// Result of attaching a file from disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum AttachmentUpload {
    Ready {
        attachment: ChatAttachment,
    },
    /// The webview has to convert the file first (decode audio the native decoder can't
    /// read, shrink a large image) and send the result as raw bytes.
    Convert {
        path: String,
        kind: String,
    },
}

/// One conversation in the Ask panel (per recording) or Ask your library (`recording_id` None).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatThread {
    pub id: String,
    pub recording_id: Option<String>,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub message_count: u32,
    /// Summary of the first `compacted_count` messages; the model gets it instead of them.
    pub summary: Option<String>,
    pub compacted_count: u32,
}

/// Result of a question: the (possibly new) thread and the two stored messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskResult {
    pub thread: ChatThread,
    pub question: ChatMessage,
    pub answer: ChatMessage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Doc {
    pub id: String,
    pub recording_id: String,
    pub title: String,
    pub prompt: String,
    pub html: String,
    pub model: String,
    pub created_at: String,
    pub updated_at: String,
}

/// A user note/comment on a stretch of a recording, anchored by time so it survives
/// re-transcription and transcript edits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub id: String,
    pub recording_id: String,
    pub start: f64,
    pub end: f64,
    /// Markdown.
    pub body: String,
    /// "yellow" | "green" | "blue" | "purple" | "red"
    pub color: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

pub const NOTE_COLORS: &[&str] = &["yellow", "green", "blue", "purple", "red"];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteWithRecording {
    #[serde(flatten)]
    pub note: Note,
    pub recording_title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingDetail {
    pub recording: Recording,
    pub segments: Vec<Segment>,
    pub notes: Vec<Note>,
    pub analysis: Option<Analysis>,
    pub docs: Vec<Doc>,
    /// Ask conversations about this recording, most recent first.
    pub threads: Vec<ChatThread>,
    pub embeddings_indexed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Waveform {
    pub peaks: Vec<f32>,
    pub peaks_per_second: u32,
    pub duration_sec: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TranscriptionStage {
    Queued,
    Decoding,
    LoadingModel,
    Transcribing,
    Saving,
    Done,
    Error,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptionProgress {
    pub recording_id: String,
    pub stage: TranscriptionStage,
    pub progress: f32,
    pub message: Option<String>,
    pub new_segments: Vec<Segment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HitKind {
    Lexical,
    Semantic,
    Tag,
    Note,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub recording_id: String,
    pub recording_title: String,
    pub segment_id: u32,
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub score: f32,
    pub kind: HitKind,
    /// UTF-16 code unit ranges, matching JavaScript string indices.
    pub highlights: Vec<(u32, u32)>,
    /// Set for note hits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagSummary {
    pub name: String,
    pub recording_count: u32,
    pub segment_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub openrouter_api_key: String,
    pub llm_model: String,
    pub embedding_model: String,
    /// "auto" | "en" | "fa"
    pub ai_language: String,
    pub default_model_id: String,
    pub use_gpu: bool,
    pub threads: u32,
    pub copy_into_library: bool,
    /// "system" | "light" | "dark"
    pub theme: String,
    /// "q8_0" | "f16"
    pub whisper_quant: String,
    pub keep_source_weights: bool,
    pub persian_digits: bool,
    pub precise_word_timings: bool,
    pub auto_analyze: bool,
    /// Anonymous usage statistics (PostHog). On by default, opt-out.
    pub analytics_enabled: bool,
    /// Crash and error reports (Sentry). On by default, opt-out.
    pub crash_reports_enabled: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            openrouter_api_key: String::new(),
            llm_model: "deepseek/deepseek-v4-flash".into(),
            embedding_model: "baai/bge-m3".into(),
            ai_language: "auto".into(),
            default_model_id: "shenava-koochik".into(),
            use_gpu: true,
            threads: 0,
            copy_into_library: true,
            theme: "system".into(),
            whisper_quant: "q8_0".into(),
            keep_source_weights: false,
            persian_digits: true,
            precise_word_timings: true,
            auto_analyze: false,
            analytics_enabled: true,
            crash_reports_enabled: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuDevice {
    pub name: String,
    pub description: String,
    pub memory_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Engines {
    pub whisper: bool,
    pub shenava: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemInfo {
    pub os: String,
    pub arch: String,
    pub app_version: String,
    pub cpu_cores: u32,
    pub total_memory_bytes: u64,
    pub gpu_backend: String,
    pub gpu_available: bool,
    pub gpu_devices: Vec<GpuDevice>,
    pub data_dir: String,
    pub engines: Engines,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmModelInfo {
    pub id: String,
    pub name: String,
    pub context_length: Option<u64>,
    pub prompt_price: Option<f64>,
    pub completion_price: Option<f64>,
    /// "text", "image", "audio", "file"… as reported by OpenRouter.
    #[serde(default)]
    pub input_modalities: Vec<String>,
    /// Unix seconds when OpenRouter added the model.
    #[serde(default)]
    pub created: Option<u64>,
}

impl LlmModelInfo {
    pub fn accepts(&self, modality: &str) -> bool {
        self.input_modalities.iter().any(|m| m == modality)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmStreamEvent {
    pub request_id: String,
    pub delta: Option<String>,
    pub done: bool,
    pub error: Option<String>,
    /// What the request is doing before the answer streams ("Transcribing the voice message…").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}
