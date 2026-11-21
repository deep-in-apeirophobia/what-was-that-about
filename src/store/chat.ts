import { create } from "zustand";
import { api } from "../lib/api";
import { errorMessage } from "../lib/errors";
import { newRequestId } from "../lib/platform";
import type { ChatMessage } from "../lib/types";
import { useStreams } from "./streams";
import { attempt, toast } from "./toasts";

export interface PendingQuestion {
  requestId: string;
  question: string;
}

/** Ask the backend to stop an in-flight LLM request. */
export function stopLlm(requestId: string): void {
  void attempt("Couldn't stop the request", () => api.cancelLlm(requestId));
}

export function isCancellation(e: unknown): boolean {
  return /cancel/i.test(errorMessage(e));
}

/**
 * Ask a question and stream the answer. Resolves with the stored assistant message,
 * or null when it failed / was cancelled (a toast explains failures).
 */
export async function askStreaming(
  recordingId: string | null,
  question: string,
  onStart: (p: PendingQuestion) => void,
): Promise<ChatMessage | null> {
  const requestId = newRequestId();
  const streams = useStreams.getState();
  streams.start(requestId);
  onStart({ requestId, question });
  try {
    return await api.ask(recordingId, question, requestId);
  } catch (e) {
    if (isCancellation(e)) toast.info("Stopped");
    else toast.error("Couldn't get an answer", e);
    return null;
  } finally {
    useStreams.getState().clear(requestId);
  }
}

export function localUserMessage(recordingId: string | null, question: string): ChatMessage {
  return {
    id: `local-${newRequestId()}`,
    recordingId,
    role: "user",
    content: question,
    createdAt: new Date().toISOString(),
    model: null,
  };
}

interface LibraryChatState {
  messages: ChatMessage[];
  loaded: boolean;
  pending: PendingQuestion | null;
  load: () => Promise<void>;
  ask: (question: string) => Promise<boolean>;
  clear: () => Promise<void>;
}

export const useLibraryChat = create<LibraryChatState>((set, get) => ({
  messages: [],
  loaded: false,
  pending: null,

  load: async () => {
    const messages = await attempt("Couldn't load the library chat", () => api.listLibraryChat());
    set({ messages: messages ?? get().messages, loaded: true });
  },

  ask: async (question) => {
    if (get().pending) return false;
    const msg = await askStreaming(null, question, (pending) => set({ pending }));
    set((s) => ({
      pending: null,
      messages: msg ? [...s.messages, localUserMessage(null, question), msg] : s.messages,
    }));
    return !!msg;
  },

  clear: async () => {
    const ok = await attempt("Couldn't clear the chat", async () => {
      await api.clearChat(null);
      return true;
    });
    if (ok) set({ messages: [] });
  },
}));
