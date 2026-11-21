import { create } from "zustand";
import { api } from "../lib/api";
import type { FixProgress, Recording, TranscriptionProgress } from "../lib/types";
import { pluralize } from "../lib/format";
import { attempt, toast } from "./toasts";
import { useApp } from "./app";

/** Last progress per recording, without the (potentially large) newSegments payload. */
export type ProgressInfo = Omit<TranscriptionProgress, "newSegments">;

/** A running AI transcript fix: batches finished so far (total is 0 until the first one). */
export interface FixingInfo {
  done: number;
  total: number;
}

interface LibraryState {
  recordings: Recording[];
  loaded: boolean;
  progress: Record<string, ProgressInfo>;
  /** AI transcript fixes started from this window, by recording id. */
  fixing: Record<string, FixingInfo>;
  refresh: () => Promise<void>;
  upsert: (r: Recording) => void;
  remove: (id: string) => void;
  applyProgress: (p: TranscriptionProgress) => void;
  setFixing: (id: string, info: FixingInfo | null) => void;
  applyFixProgress: (p: FixProgress) => void;
  importPaths: (paths: string[]) => Promise<Recording[]>;
  importLink: (url: string) => Promise<Recording | null>;
  insertIfMissing: (r: Recording) => void;
}

const byNewest = (a: Recording, b: Recording) => b.createdAt.localeCompare(a.createdAt);

export const useLibrary = create<LibraryState>((set, get) => ({
  recordings: [],
  loaded: false,
  progress: {},
  fixing: {},

  refresh: async () => {
    const list = await attempt("Couldn't load your library", () => api.listRecordings());
    set({ recordings: (list ?? get().recordings).slice().sort(byNewest), loaded: true });
  },

  upsert: (r) =>
    set((s) => {
      const i = s.recordings.findIndex((x) => x.id === r.id);
      const recordings = i >= 0 ? s.recordings.map((x) => (x.id === r.id ? r : x)) : [r, ...s.recordings].sort(byNewest);
      // Drop stale progress once a recording leaves the running states.
      const progress = { ...s.progress };
      if (r.status !== "queued" && r.status !== "transcribing") delete progress[r.id];
      return { recordings, progress };
    }),

  // Import commands return a snapshot taken before background work starts; status events
  // for the same recording can arrive first and are newer, so don't overwrite them.
  insertIfMissing: (r) => {
    if (!get().recordings.some((x) => x.id === r.id)) get().upsert(r);
  },

  remove: (id) =>
    set((s) => {
      const progress = { ...s.progress };
      delete progress[id];
      return { recordings: s.recordings.filter((x) => x.id !== id), progress };
    }),

  applyProgress: (p) => {
    const { newSegments: _ignored, ...info } = p;
    void _ignored;
    set((s) => {
      const progress = { ...s.progress };
      if (p.stage === "done" || p.stage === "cancelled" || p.stage === "error") delete progress[p.recordingId];
      else progress[p.recordingId] = info;
      return { progress };
    });
    if (p.stage === "error") {
      const title = get().recordings.find((r) => r.id === p.recordingId)?.title ?? "A recording";
      toast.error(`Transcription failed: ${title}`, p.message ?? "Unknown error");
    }
  },

  setFixing: (id, info) =>
    set((s) => {
      const fixing = { ...s.fixing };
      if (info) fixing[id] = info;
      else delete fixing[id];
      return { fixing };
    }),

  applyFixProgress: (p) => {
    if (get().fixing[p.recordingId]) get().setFixing(p.recordingId, { done: p.done, total: p.total });
  },

  importLink: async (url) => {
    const rec = await attempt("Couldn't import the link", () => api.importLink(url));
    if (!rec) return null;
    get().insertIfMissing(rec);
    toast.success("Downloading the recording…", "It will be transcribed when the download finishes.", {
      label: "Open",
      onClick: () => useApp.getState().openRecording(rec.id),
    });
    return rec;
  },

  importPaths: async (paths) => {
    if (!paths.length) return [];
    const added = await attempt("Import failed", () => api.importFiles(paths));
    if (!added?.length) return [];
    for (const r of added) get().insertIfMissing(r);
    if (added.length === 1) {
      toast.success("Imported", added[0].title, {
        label: "Open",
        onClick: () => useApp.getState().openRecording(added[0].id),
      });
    } else {
      toast.success(`Imported ${pluralize(added.length, "file")}`);
    }
    const skipped = paths.length - added.length;
    if (skipped > 0) toast.info(`${pluralize(skipped, "file")} skipped`, "Unsupported or unreadable files were ignored.");
    return added;
  },
}));
