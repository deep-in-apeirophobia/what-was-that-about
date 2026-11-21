import { create } from "zustand";
import { api } from "../lib/api";
import { MAX_ATTACHMENTS, attachmentKind, prepareImage } from "../lib/attachments";
import { TRANSCRIPTION_SAMPLE_RATE, decodeBytesForTranscription, decodeForTranscription, resampleTo16k } from "../lib/audio";
import { errorMessage } from "../lib/errors";
import { fileName, mediaUrl, newRequestId } from "../lib/platform";
import type { ChatAttachment, ChatMessage, ChatThread } from "../lib/types";
import { useStreams } from "./streams";
import { attempt, toast } from "./toasts";

/** Ask the backend to stop an in-flight LLM request. */
export function stopLlm(requestId: string): void {
  void attempt("Couldn't stop the request", () => api.cancelLlm(requestId));
}

export function isCancellation(e: unknown): boolean {
  return /cancel/i.test(errorMessage(e));
}

/** Where a conversation lives: a recording id, or null for Ask your library. */
export type ChatScope = string | null;

export const scopeKey = (scope: ChatScope): string => scope ?? "library";

/** A file on its way into the message being written. */
export interface DraftAttachment {
  key: string;
  name: string;
  kind: "image" | "audio";
  /** The uploaded attachment; null while it uploads. */
  attachment: ChatAttachment | null;
}

export interface PendingQuestion {
  requestId: string;
  /** null = the question starts a new thread */
  threadId: string | null;
  question: string;
  attachments: ChatAttachment[];
  /** The drafts that were sent; back in the composer if asking fails. */
  drafts: DraftAttachment[];
}

export interface ScopeChat {
  threads: ChatThread[];
  loaded: boolean;
  /** Open conversation; null = a new one that starts with the next question. */
  activeId: string | null;
  /** Loaded messages per thread id. */
  messages: Record<string, ChatMessage[]>;
  /** Threads whose messages failed to load. */
  loadErrors: Record<string, string>;
  pending: PendingQuestion | null;
  drafts: DraftAttachment[];
  /** Bumped by every change to the thread list, so slower list fetches don't undo it. */
  rev: number;
}

export const EMPTY_CHAT: ScopeChat = {
  threads: [],
  loaded: false,
  activeId: null,
  messages: {},
  loadErrors: {},
  pending: null,
  drafts: [],
  rev: 0,
};

interface ChatsState {
  scopes: Record<string, ScopeChat>;
  /** Load the scope's threads and open the most recent one (first time only). */
  load: (scope: ChatScope) => Promise<void>;
  /** Open a thread, or a new conversation with null. */
  open: (scope: ChatScope, threadId: string | null) => Promise<void>;
  /** Try loading a thread's messages again after a failure. */
  reloadMessages: (scope: ChatScope, threadId: string) => Promise<void>;
  /** Send the question with the ready attachments; resolves true once answered. */
  ask: (scope: ChatScope, question: string) => Promise<boolean>;
  rename: (scope: ChatScope, threadId: string, title: string) => Promise<boolean>;
  remove: (scope: ChatScope, threadId: string) => Promise<void>;
  /** Attach files from disk (picker or drop in the app). */
  attachPaths: (scope: ChatScope, paths: string[]) => Promise<void>;
  /** Attach in-memory files (paste, or picker/drop in the browser mock). */
  attachFiles: (scope: ChatScope, files: File[]) => Promise<void>;
  /** Attach a voice recording (mono samples at any rate). */
  attachVoice: (scope: ChatScope, samples: Float32Array, sampleRate: number) => Promise<void>;
  discardDraft: (scope: ChatScope, key: string) => void;
}

let draftSeq = 0;

function byRecent(a: ChatThread, b: ChatThread): number {
  return b.updatedAt.localeCompare(a.updatedAt);
}

export const useChats = create<ChatsState>((set, get) => {
  const chat = (scope: ChatScope): ScopeChat => get().scopes[scopeKey(scope)] ?? EMPTY_CHAT;
  const patch = (scope: ChatScope, fn: (c: ScopeChat) => Partial<ScopeChat>) =>
    set((s) => {
      const key = scopeKey(scope);
      const cur = s.scopes[key] ?? EMPTY_CHAT;
      return { scopes: { ...s.scopes, [key]: { ...cur, ...fn(cur) } } };
    });

  const loadMessages = async (scope: ChatScope, threadId: string) => {
    const before = chat(scope).messages[threadId];
    try {
      const messages = await api.listChatMessages(threadId);
      patch(scope, (c) => {
        // an answer arrived (or another load finished) meanwhile: that copy is newer
        if (c.messages[threadId] !== before) return {};
        const loadErrors = { ...c.loadErrors };
        delete loadErrors[threadId];
        return { messages: { ...c.messages, [threadId]: messages }, loadErrors };
      });
    } catch (e) {
      patch(scope, (c) => ({ loadErrors: { ...c.loadErrors, [threadId]: errorMessage(e) } }));
    }
  };

  /** Upload one file and keep it as a draft; `upload` does the actual work. */
  const addDraft = async (
    scope: ChatScope,
    name: string,
    kind: "image" | "audio",
    upload: () => Promise<ChatAttachment>,
  ) => {
    if (chat(scope).drafts.length >= MAX_ATTACHMENTS) {
      toast.info(`You can attach up to ${MAX_ATTACHMENTS} files to one message`);
      return;
    }
    const key = `draft-${++draftSeq}`;
    patch(scope, (c) => ({ drafts: [...c.drafts, { key, name, kind, attachment: null }] }));
    try {
      const attachment = await upload();
      if (!chat(scope).drafts.some((d) => d.key === key)) {
        // removed while uploading
        void api.discardChatAttachment(attachment.id).catch(() => {});
        return;
      }
      patch(scope, (c) => ({ drafts: c.drafts.map((d) => (d.key === key ? { ...d, attachment } : d)) }));
    } catch (e) {
      patch(scope, (c) => ({ drafts: c.drafts.filter((d) => d.key !== key) }));
      toast.error(`Couldn't attach ${name}`, e);
    }
  };

  return {
    scopes: {},

    load: async (scope) => {
      const rev = chat(scope).rev;
      const threads = await attempt("Couldn't load conversations", () => api.listChatThreads(scope));
      if (!threads) {
        patch(scope, () => ({ loaded: true }));
        return;
      }
      // a question, rename or delete finished meanwhile: this list is out of date
      if (chat(scope).rev !== rev) {
        patch(scope, () => ({ loaded: true }));
        return;
      }
      const first = !chat(scope).loaded;
      patch(scope, (c) => {
        const keep = c.activeId && threads.some((t) => t.id === c.activeId);
        return { threads, loaded: true, activeId: first ? (threads[0]?.id ?? null) : keep ? c.activeId : null };
      });
      const active = chat(scope).activeId;
      if (active && !chat(scope).messages[active]) await loadMessages(scope, active);
    },

    open: async (scope, threadId) => {
      patch(scope, () => ({ activeId: threadId }));
      if (threadId && !chat(scope).messages[threadId]) await loadMessages(scope, threadId);
    },

    reloadMessages: (scope, threadId) => loadMessages(scope, threadId),

    ask: async (scope, question) => {
      const c = chat(scope);
      if (c.pending) return false;
      if (c.drafts.some((d) => !d.attachment)) {
        toast.info("Wait for the attachments to finish uploading");
        return false;
      }
      const drafts = c.drafts;
      const attachments = drafts.map((d) => d.attachment!);
      const q = question.trim();
      if (!q && !attachments.length) return false;
      const requestId = newRequestId();
      const threadId = c.activeId;
      useStreams.getState().start(requestId);
      // the sent files leave the composer (so they can't be removed mid-answer)
      patch(scope, () => ({ pending: { requestId, threadId, question: q, attachments, drafts }, drafts: [] }));
      try {
        const r = await api.ask(scope, threadId, q, attachments.map((a) => a.id), requestId);
        const id = r.thread.id;
        let reload = false;
        patch(scope, (cur) => {
          const known = cur.messages[id];
          // a thread whose messages never loaded is fetched whole instead of showing a stub
          reload = !known && threadId !== null;
          const messages = { ...cur.messages };
          if (known || threadId === null) messages[id] = [...(known ?? []), r.question, r.answer];
          return {
            pending: null,
            rev: cur.rev + 1,
            threads: [r.thread, ...cur.threads.filter((t) => t.id !== id && t.id !== threadId)].sort(byRecent),
            messages,
            // follow the thread unless the user moved elsewhere meanwhile
            activeId: cur.activeId === threadId ? id : cur.activeId,
          };
        });
        if (reload) await loadMessages(scope, id);
        return true;
      } catch (e) {
        patch(scope, (cur) => ({ pending: null, drafts: [...drafts, ...cur.drafts] }));
        if (isCancellation(e)) toast.info("Stopped");
        else toast.error("Couldn't get an answer", e);
        return false;
      } finally {
        useStreams.getState().clear(requestId);
      }
    },

    rename: async (scope, threadId, title) => {
      const t = await attempt("Couldn't rename the conversation", () => api.renameChatThread(threadId, title));
      if (!t) return false;
      patch(scope, (c) => ({ rev: c.rev + 1, threads: c.threads.map((x) => (x.id === t.id ? t : x)) }));
      return true;
    },

    remove: async (scope, threadId) => {
      const ok = await attempt("Couldn't delete the conversation", async () => {
        await api.deleteChatThread(threadId);
        return true;
      });
      if (!ok) return;
      patch(scope, (c) => {
        const messages = { ...c.messages };
        delete messages[threadId];
        return {
          rev: c.rev + 1,
          threads: c.threads.filter((t) => t.id !== threadId),
          messages,
          activeId: c.activeId === threadId ? null : c.activeId,
        };
      });
    },

    attachPaths: async (scope, paths) => {
      await Promise.all(
        paths.map((path) => {
          const name = fileName(path);
          const kind = attachmentKind(name);
          if (!kind) {
            toast.error(`Can't attach ${name}`, "Attach images (PNG, JPEG, WebP, GIF) or audio files.");
            return Promise.resolve();
          }
          return addDraft(scope, name, kind, async () => {
            const up = await api.addChatAttachmentFile(path);
            if (up.status === "ready") return up.attachment;
            const url = mediaUrl(up.path);
            if (!url) throw new Error("The file can't be read.");
            if (up.kind === "image") {
              const res = await fetch(url);
              if (!res.ok) throw new Error(`Couldn't read the image (HTTP ${res.status}).`);
              return api.addChatImage(await prepareImage(await res.blob()), name);
            }
            return api.addChatAudio(await decodeForTranscription(url), TRANSCRIPTION_SAMPLE_RATE, name);
          });
        }),
      );
    },

    attachFiles: async (scope, files) => {
      await Promise.all(
        files.map((file) => {
          const name = file.name || "Pasted image";
          const kind = attachmentKind(name, file.type);
          if (!kind) {
            toast.error(`Can't attach ${name}`, "Attach images (PNG, JPEG, WebP, GIF) or audio files.");
            return Promise.resolve();
          }
          return addDraft(scope, name, kind, async () => {
            if (kind === "image") return api.addChatImage(await prepareImage(file), name);
            const samples = await decodeBytesForTranscription(await file.arrayBuffer());
            return api.addChatAudio(samples, TRANSCRIPTION_SAMPLE_RATE, name);
          });
        }),
      );
    },

    attachVoice: async (scope, samples, sampleRate) => {
      await addDraft(scope, "Voice message", "audio", async () => {
        const at16k = await resampleTo16k(samples, sampleRate);
        return api.addChatAudio(at16k, TRANSCRIPTION_SAMPLE_RATE, "Voice message");
      });
    },

    discardDraft: (scope, key) => {
      const d = chat(scope).drafts.find((x) => x.key === key);
      patch(scope, (c) => ({ drafts: c.drafts.filter((x) => x.key !== key) }));
      if (d?.attachment) void api.discardChatAttachment(d.attachment.id).catch(() => {});
    },
  };
});

/** Reactive view of one scope's chat state. */
export function useScopeChat(scope: ChatScope): ScopeChat {
  return useChats((s) => s.scopes[scopeKey(scope)] ?? EMPTY_CHAT);
}
