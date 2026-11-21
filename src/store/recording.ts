import { create } from "zustand";
import { api } from "../lib/api";
import { errorMessage } from "../lib/errors";
import { mergeSegments } from "../lib/segments";
import type {
  Analysis,
  ChatMessage,
  Doc,
  Recording,
  RecordingDetail,
  SearchHit,
  Segment,
  Waveform,
} from "../lib/types";

export type PanelTab = "overview" | "topics" | "ask" | "docs" | "related";

const LS_FONT = "wwta.transcriptFontSize";
export const FONT_SIZES = [15, 17, 19, 21, 24, 28] as const;

function readFont(): number {
  try {
    const v = Number(localStorage.getItem(LS_FONT));
    return (FONT_SIZES as readonly number[]).includes(v) ? v : 19;
  } catch {
    return 19;
  }
}

export interface RelatedState {
  segmentId: number;
  text: string;
  hits: SearchHit[] | null;
  error: string | null;
}

export interface PendingAsk {
  requestId: string;
  question: string;
}

interface RecordingState {
  id: string | null;
  detail: RecordingDetail | null;
  loading: boolean;
  error: string | null;
  waveform: Waveform | null;
  waveformError: string | null;
  tab: PanelTab;
  selectedTag: string | null;
  related: RelatedState | null;
  findOpen: boolean;
  fontSize: number;
  pendingAsk: PendingAsk | null;
  /** In-flight doc generation (new doc) for the Docs tab. */
  pendingDoc: { requestId: string; prompt: string } | null;
  /** Ask the transcript to scroll to (and briefly flash) a segment. */
  reveal: { segmentId: number; nonce: number } | null;

  open: (id: string) => Promise<void>;
  close: () => void;
  reload: () => Promise<void>;
  loadWaveform: () => Promise<void>;
  /** A recording row changed (event or command result). Reloads details when needed. */
  applyRecording: (r: Recording) => void;
  appendSegments: (recordingId: string, segs: Segment[]) => void;
  replaceSegment: (seg: Segment) => void;
  setAnalysis: (a: Analysis) => void;
  setEmbeddingsIndexed: (v: boolean) => void;
  upsertDoc: (doc: Doc) => void;
  removeDoc: (docId: string) => void;
  appendChat: (...msgs: ChatMessage[]) => void;
  clearChat: () => void;
  setTab: (tab: PanelTab) => void;
  selectTag: (tag: string | null) => void;
  findRelated: (seg: Segment) => Promise<void>;
  setFindOpen: (open: boolean) => void;
  setFontSize: (size: number) => void;
  setPendingAsk: (p: PendingAsk | null) => void;
  setPendingDoc: (p: { requestId: string; prompt: string } | null) => void;
  revealSegment: (segmentId: number) => void;
}

let revealNonce = 0;

export const useRecording = create<RecordingState>((set, get) => ({
  id: null,
  detail: null,
  loading: false,
  error: null,
  waveform: null,
  waveformError: null,
  tab: "overview",
  selectedTag: null,
  related: null,
  findOpen: false,
  fontSize: readFont(),
  pendingAsk: null,
  pendingDoc: null,
  reveal: null,

  open: async (id) => {
    if (get().id === id && get().detail) return;
    set({
      id,
      detail: null,
      loading: true,
      error: null,
      waveform: null,
      waveformError: null,
      selectedTag: null,
      related: null,
      findOpen: false,
      pendingAsk: null,
      pendingDoc: null,
      reveal: null,
      tab: get().tab === "related" ? "overview" : get().tab,
    });
    await get().reload();
    void get().loadWaveform();
  },

  close: () => set({ id: null, detail: null, waveform: null }),

  reload: async () => {
    const id = get().id;
    if (!id) return;
    try {
      const detail = await api.getRecording(id);
      if (get().id !== id) return;
      set({ detail, loading: false, error: null });
    } catch (e) {
      if (get().id !== id) return;
      set({ loading: false, error: errorMessage(e) });
    }
  },

  loadWaveform: async () => {
    const id = get().id;
    const rec = get().detail?.recording;
    if (!id || !rec || rec.status === "needsDecode") return;
    try {
      const waveform = await api.getWaveform(id);
      if (get().id === id) set({ waveform, waveformError: null });
    } catch (e) {
      if (get().id === id) set({ waveform: null, waveformError: errorMessage(e) });
    }
  },

  applyRecording: (r) => {
    const { id, detail } = get();
    if (id !== r.id || !detail) return;
    const prev = detail.recording;
    set({ detail: { ...detail, recording: r } });
    const becameReady = prev.status !== "ready" && r.status === "ready";
    const restarted = (r.status === "queued" || r.status === "transcribing") && prev.status !== r.status && prev.status !== "queued";
    const analysisChanged = prev.hasAnalysis !== r.hasAnalysis;
    const segmentsChanged = r.status === "ready" && prev.segmentCount !== r.segmentCount;
    if (restarted) {
      set((s) => (s.detail ? { detail: { ...s.detail, segments: [], analysis: null }, selectedTag: null } : s));
    }
    if (becameReady || analysisChanged || segmentsChanged || (prev.status === "needsDecode" && r.status !== "needsDecode")) {
      void get().reload();
    }
    if (prev.durationSec !== r.durationSec || (prev.status === "needsDecode" && r.status !== "needsDecode")) {
      void get().loadWaveform();
    }
  },

  appendSegments: (recordingId, segs) => {
    const { id, detail } = get();
    if (id !== recordingId || !detail || !segs.length) return;
    set({ detail: { ...detail, segments: mergeSegments(detail.segments, segs) } });
  },

  replaceSegment: (seg) => {
    const { detail } = get();
    if (!detail) return;
    set({ detail: { ...detail, segments: detail.segments.map((s) => (s.id === seg.id ? seg : s)) } });
  },

  setAnalysis: (analysis) => {
    const { detail } = get();
    if (detail) set({ detail: { ...detail, analysis } });
  },

  setEmbeddingsIndexed: (embeddingsIndexed) => {
    const { detail } = get();
    if (detail) set({ detail: { ...detail, embeddingsIndexed } });
  },

  upsertDoc: (doc) => {
    const { detail } = get();
    if (!detail || doc.recordingId !== detail.recording.id) return;
    const exists = detail.docs.some((d) => d.id === doc.id);
    set({
      detail: { ...detail, docs: exists ? detail.docs.map((d) => (d.id === doc.id ? doc : d)) : [doc, ...detail.docs] },
    });
  },

  removeDoc: (docId) => {
    const { detail } = get();
    if (detail) set({ detail: { ...detail, docs: detail.docs.filter((d) => d.id !== docId) } });
  },

  appendChat: (...msgs) => {
    const { detail } = get();
    if (detail) set({ detail: { ...detail, chat: [...detail.chat, ...msgs] } });
  },

  clearChat: () => {
    const { detail } = get();
    if (detail) set({ detail: { ...detail, chat: [] } });
  },

  setTab: (tab) => set({ tab }),
  selectTag: (selectedTag) => set({ selectedTag }),

  findRelated: async (seg) => {
    const id = get().id;
    if (!id) return;
    set({ related: { segmentId: seg.id, text: seg.text, hits: null, error: null }, tab: "related" });
    try {
      const hits = await api.relatedSegments(id, seg.id, 12);
      if (get().related?.segmentId === seg.id) set({ related: { segmentId: seg.id, text: seg.text, hits, error: null } });
    } catch (e) {
      if (get().related?.segmentId === seg.id)
        set({ related: { segmentId: seg.id, text: seg.text, hits: [], error: errorMessage(e) } });
    }
  },

  setFindOpen: (findOpen) => set({ findOpen }),
  setFontSize: (fontSize) => {
    try {
      localStorage.setItem(LS_FONT, String(fontSize));
    } catch {
      /* ignore */
    }
    set({ fontSize });
  },
  setPendingAsk: (pendingAsk) => set({ pendingAsk }),
  setPendingDoc: (pendingDoc) => set({ pendingDoc }),
  revealSegment: (segmentId) => set({ reveal: { segmentId, nonce: ++revealNonce } }),
}));
