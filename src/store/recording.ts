import { create } from "zustand";
import { api } from "../lib/api";
import { errorMessage } from "../lib/errors";
import { PENDING_NOTE_PREFIX, compareNotes, notePreview, upsertNote } from "../lib/notes";
import { mergeSegments } from "../lib/segments";
import type {
  Analysis,
  ChatMessage,
  Doc,
  Note,
  NoteColor,
  Recording,
  RecordingDetail,
  SearchHit,
  Segment,
  Waveform,
} from "../lib/types";
import { attempt, toast } from "./toasts";

export type PanelTab = "overview" | "notes" | "topics" | "ask" | "docs" | "related";

const LS_FONT = "wwta.transcriptFontSize";
const LS_HIGHLIGHT_FIXES = "wwta.highlightFixes";
export const FONT_SIZES = [15, 17, 19, 21, 24, 28] as const;

function readHighlightFixes(): boolean {
  try {
    return localStorage.getItem(LS_HIGHLIGHT_FIXES) !== "0";
  } catch {
    return true;
  }
}

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

/** A note being written. */
export interface NoteDraft {
  start: number;
  end: number;
  /** Transcript segment the composer appears after; null = in the Notes tab. */
  anchorSegmentId: number | null;
  /** Restored text after a failed save. */
  body?: string;
  color?: NoteColor | null;
}

/** A note being edited, and where its editor is shown. */
export interface NoteEditor {
  id: string;
  where: "transcript" | "panel";
  /** Restored text after a failed save. */
  body?: string;
  color?: NoteColor | null;
}

export type NoteFields = Pick<Note, "start" | "end" | "body" | "color">;

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
  /** Mark the words an AI fix changed in the transcript. */
  highlightFixes: boolean;
  pendingAsk: PendingAsk | null;
  /** In-flight doc generation (new doc) for the Docs tab. */
  pendingDoc: { requestId: string; prompt: string } | null;
  /** Ask the transcript to scroll to (and briefly flash) a segment. */
  reveal: { segmentId: number; nonce: number } | null;
  noteDraft: NoteDraft | null;
  noteEditor: NoteEditor | null;
  /** Ask the transcript to scroll to (and briefly flash) a note. */
  noteReveal: { noteId: string; nonce: number } | null;

  open: (id: string) => Promise<void>;
  close: () => void;
  reload: () => Promise<void>;
  loadWaveform: () => Promise<void>;
  /** A recording row changed (event or command result). Reloads details when needed. */
  applyRecording: (r: Recording) => void;
  appendSegments: (recordingId: string, segs: Segment[]) => void;
  replaceSegment: (seg: Segment) => void;
  /** Replace sentences of the open recording that are already shown (AI fixes, reverts).
   *  The backend drops the semantic index when that happens, so it's marked as gone. */
  updateSegments: (recordingId: string, segs: Segment[]) => void;
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
  setHighlightFixes: (on: boolean) => void;
  setPendingAsk: (p: PendingAsk | null) => void;
  setPendingDoc: (p: { requestId: string; prompt: string } | null) => void;
  revealSegment: (segmentId: number) => void;

  /** Open the composer for a new note (closes any other editor). */
  startNote: (draft: NoteDraft) => void;
  cancelNote: () => void;
  /** Close the composer and save; reopens it with the text if saving fails. */
  saveDraft: (body: string, color: NoteColor | null) => Promise<void>;
  editNote: (editor: NoteEditor | null) => void;
  /** Close the editor and save; reopens it with the text if saving fails. */
  saveEdit: (id: string, fields: NoteFields) => Promise<void>;
  /** Optimistic add: the note shows at once and is replaced by the saved one. */
  addNote: (fields: NoteFields) => Promise<Note | null>;
  updateNote: (id: string, fields: NoteFields) => Promise<boolean>;
  /** Optimistic delete with an "Undo" toast. */
  deleteNote: (id: string) => Promise<void>;
  revealNote: (noteId: string) => void;
}

let revealNonce = 0;
/** Bumped per `reload`, so a slower, older response can't overwrite a newer one. */
let reloadSeq = 0;
let pendingNoteSeq = 0;

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
  highlightFixes: readHighlightFixes(),
  pendingAsk: null,
  pendingDoc: null,
  reveal: null,
  noteDraft: null,
  noteEditor: null,
  noteReveal: null,

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
      noteDraft: null,
      noteEditor: null,
      noteReveal: null,
      tab: get().tab === "related" ? "overview" : get().tab,
    });
    await get().reload();
    void get().loadWaveform();
  },

  close: () => set({ id: null, detail: null, waveform: null }),

  reload: async () => {
    const id = get().id;
    if (!id) return;
    const seq = ++reloadSeq;
    const current = () => get().id === id && seq === reloadSeq;
    try {
      const detail = await api.getRecording(id);
      if (!current()) return;
      const notes = [...(detail.notes ?? [])].sort(compareNotes);
      set({ detail: { ...detail, notes }, loading: false, error: null });
    } catch (e) {
      if (!current()) return;
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

  updateSegments: (recordingId, segs) => {
    const { id, detail } = get();
    if (id !== recordingId || !detail || !segs.length) return;
    const byId = new Map(segs.map((s) => [s.id, s]));
    if (!detail.segments.some((s) => byId.has(s.id))) return;
    set({ detail: { ...detail, segments: detail.segments.map((s) => byId.get(s.id) ?? s), embeddingsIndexed: false } });
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
  setHighlightFixes: (highlightFixes) => {
    try {
      localStorage.setItem(LS_HIGHLIGHT_FIXES, highlightFixes ? "1" : "0");
    } catch {
      /* ignore */
    }
    set({ highlightFixes });
  },
  setPendingAsk: (pendingAsk) => set({ pendingAsk }),
  setPendingDoc: (pendingDoc) => set({ pendingDoc }),
  revealSegment: (segmentId) => set({ reveal: { segmentId, nonce: ++revealNonce } }),

  // ---- notes

  startNote: (noteDraft) => set({ noteDraft, noteEditor: null }),
  cancelNote: () => set({ noteDraft: null }),

  saveDraft: async (body, color) => {
    const { noteDraft: draft, id } = get();
    if (!draft || !id) return;
    set({ noteDraft: null });
    const saved = await get().addNote({ start: draft.start, end: draft.end, body, color });
    if (!saved && get().id === id && !get().noteDraft && !get().noteEditor) {
      set({ noteDraft: { ...draft, body, color } });
    }
  },

  editNote: (noteEditor) => set({ noteEditor, noteDraft: null }),

  saveEdit: async (noteId, fields) => {
    const { noteEditor: editor, id } = get();
    if (!id) return;
    if (editor?.id === noteId) set({ noteEditor: null });
    const ok = await get().updateNote(noteId, fields);
    const still = get().detail?.notes.some((n) => n.id === noteId);
    if (!ok && editor && still && get().id === id && !get().noteDraft && !get().noteEditor) {
      set({ noteEditor: { ...editor, body: fields.body, color: fields.color } });
    }
  },

  addNote: async (fields) => {
    const recordingId = get().id;
    if (!recordingId || !get().detail) return null;
    const now = new Date().toISOString();
    const pending: Note = { ...fields, id: `${PENDING_NOTE_PREFIX}${++pendingNoteSeq}`, recordingId, createdAt: now, updatedAt: now };
    mutateNotes(recordingId, (list) => upsertNote(list, pending));
    try {
      const saved = await api.addNote(recordingId, fields.start, fields.end, fields.body, fields.color);
      mutateNotes(recordingId, (list) => upsertNote(list.filter((n) => n.id !== pending.id), saved));
      return saved;
    } catch (e) {
      mutateNotes(recordingId, (list) => list.filter((n) => n.id !== pending.id));
      toast.error("Couldn't save the note", e);
      return null;
    }
  },

  updateNote: async (noteId, fields) => {
    const recordingId = get().id;
    const prev = get().detail?.notes.find((n) => n.id === noteId);
    if (!recordingId || !prev) return false;
    const exists = (list: Note[]) => list.some((n) => n.id === noteId);
    mutateNotes(recordingId, (list) => upsertNote(list, { ...prev, ...fields, updatedAt: new Date().toISOString() }));
    try {
      const saved = await api.updateNote({ id: noteId, ...fields });
      mutateNotes(recordingId, (list) => (exists(list) ? upsertNote(list, saved) : list));
      return true;
    } catch (e) {
      mutateNotes(recordingId, (list) => (exists(list) ? upsertNote(list, prev) : list));
      toast.error("Couldn't save the note", e);
      return false;
    }
  },

  deleteNote: async (noteId) => {
    const recordingId = get().id;
    const prev = get().detail?.notes.find((n) => n.id === noteId);
    if (!recordingId || !prev) return;
    mutateNotes(recordingId, (list) => list.filter((n) => n.id !== noteId));
    if (get().noteEditor?.id === noteId) set({ noteEditor: null });
    try {
      await api.deleteNote(noteId);
    } catch (e) {
      mutateNotes(recordingId, (list) => upsertNote(list, prev));
      toast.error("Couldn't delete the note", e);
      return;
    }
    toast.success("Note deleted", notePreview(prev.body, 80), {
      label: "Undo",
      onClick: () => void restoreNote(prev),
    });
  },

  revealNote: (noteId) => set({ noteReveal: { noteId, nonce: ++revealNonce } }),
}));

/** Apply `fn` to the open recording's notes, if `recordingId` is still the one open. */
function mutateNotes(recordingId: string, fn: (notes: Note[]) => Note[]): void {
  const { id, detail } = useRecording.getState();
  if (id !== recordingId || !detail) return;
  useRecording.setState({ detail: { ...detail, notes: fn(detail.notes) } });
}

/** Undo a delete: add the note again (it gets a new id). */
async function restoreNote(note: Note): Promise<void> {
  const fields: NoteFields = { start: note.start, end: note.end, body: note.body, color: note.color };
  if (useRecording.getState().id === note.recordingId) {
    await useRecording.getState().addNote(fields);
    return;
  }
  await attempt("Couldn't restore the note", () => api.addNote(note.recordingId, fields.start, fields.end, fields.body, fields.color));
}
