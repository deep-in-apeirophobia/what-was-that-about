import { create } from "zustand";
import type { LlmStreamEvent } from "../lib/types";

/** Accumulated text of in-flight LLM requests (chat answers and docs), keyed by requestId. */
export interface StreamState {
  text: string;
  done: boolean;
  error: string | null;
}

interface StreamsState {
  streams: Record<string, StreamState>;
  start: (requestId: string) => void;
  apply: (e: LlmStreamEvent) => void;
  clear: (requestId: string) => void;
}

export const useStreams = create<StreamsState>((set) => ({
  streams: {},
  start: (requestId) =>
    set((s) => ({ streams: { ...s.streams, [requestId]: { text: "", done: false, error: null } } })),
  apply: (e) =>
    set((s) => {
      const cur = s.streams[e.requestId];
      // Ignore events for requests this window didn't start (or already cleared).
      if (!cur) return s;
      return {
        streams: {
          ...s.streams,
          [e.requestId]: {
            text: cur.text + (e.delta ?? ""),
            done: cur.done || e.done,
            error: e.error ?? cur.error,
          },
        },
      };
    }),
  clear: (requestId) =>
    set((s) => {
      if (!(requestId in s.streams)) return s;
      const streams = { ...s.streams };
      delete streams[requestId];
      return { streams };
    }),
}));
