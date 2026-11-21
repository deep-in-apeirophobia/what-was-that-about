import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AskResult, ChatAttachment, ChatMessage, ChatThread } from "../lib/types";

vi.mock("../lib/api", () => ({
  isTauri: () => false,
  api: {
    listChatThreads: vi.fn(),
    listChatMessages: vi.fn(),
    ask: vi.fn(),
    renameChatThread: vi.fn(),
    deleteChatThread: vi.fn(),
    addChatAudio: vi.fn(),
    addChatImage: vi.fn(),
    addChatAttachmentFile: vi.fn(),
    discardChatAttachment: vi.fn(),
    cancelLlm: vi.fn(),
  },
}));

const { api } = await import("../lib/api");
const { useChats, EMPTY_CHAT } = await import("./chat");
const { useStreams } = await import("./streams");

const mocked = api as unknown as Record<keyof typeof api, ReturnType<typeof vi.fn>>;

function deferred<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const thread = (id: string, updatedAt: string, extra: Partial<ChatThread> = {}): ChatThread => ({
  id,
  recordingId: "r1",
  title: id,
  createdAt: updatedAt,
  updatedAt,
  messageCount: 2,
  summary: null,
  compactedCount: 0,
  ...extra,
});

const message = (threadId: string, role: "user" | "assistant", content: string, attachments: ChatAttachment[] = []): ChatMessage => ({
  id: `${threadId}-${role}-${content}`,
  recordingId: "r1",
  threadId,
  role,
  content,
  createdAt: "2026-11-20T10:00:00Z",
  model: role === "assistant" ? "m" : null,
  attachments,
  contextNote: null,
});

const voice: ChatAttachment = {
  id: "att-1",
  kind: "audio",
  name: "Voice message",
  mime: "audio/wav",
  sizeBytes: 64_000,
  durationSec: 2,
  path: "/data/attachments/att-1.wav",
  text: null,
  textModel: null,
};

const state = () => useChats.getState().scopes["r1"] ?? EMPTY_CHAT;

beforeEach(() => {
  vi.clearAllMocks();
  useChats.setState({ scopes: {} });
  useStreams.setState({ streams: {} });
  mocked.listChatThreads.mockResolvedValue([thread("t2", "2026-11-20T10:00:00Z"), thread("t1", "2026-11-19T10:00:00Z")]);
  mocked.listChatMessages.mockImplementation(async (id: string) => [message(id, "user", "q"), message(id, "assistant", "a")]);
});

describe("chat threads", () => {
  it("opens the most recent thread on first load and keeps the choice later", async () => {
    await useChats.getState().load("r1");
    expect(state().activeId).toBe("t2");
    expect(state().messages.t2).toHaveLength(2);
    await useChats.getState().open("r1", "t1");
    await useChats.getState().load("r1");
    expect(state().activeId).toBe("t1");
    await useChats.getState().open("r1", null);
    expect(state().activeId).toBeNull();
  });

  it("a question in a new conversation creates the thread and opens it", async () => {
    await useChats.getState().load("r1");
    await useChats.getState().open("r1", null);
    const created = thread("t3", "2026-11-21T10:00:00Z", { title: "Budget?" });
    mocked.ask.mockResolvedValue({
      thread: created,
      question: message("t3", "user", "Budget?"),
      answer: message("t3", "assistant", "Low [4]"),
    } satisfies AskResult);
    expect(await useChats.getState().ask("r1", " Budget? ")).toBe(true);
    expect(mocked.ask).toHaveBeenCalledWith("r1", null, "Budget?", [], expect.any(String));
    expect(state().activeId).toBe("t3");
    expect(state().threads.map((t) => t.id)).toEqual(["t3", "t2", "t1"]);
    expect(state().messages.t3.map((m) => m.content)).toEqual(["Budget?", "Low [4]"]);
    expect(state().pending).toBeNull();
  });

  it("doesn't pull the user back when they switched threads while waiting", async () => {
    await useChats.getState().load("r1");
    await useChats.getState().open("r1", null);
    const answer = deferred<AskResult>();
    mocked.ask.mockReturnValue(answer.promise);
    const asking = useChats.getState().ask("r1", "new question");
    expect(state().pending?.threadId).toBeNull();
    // a second question can't start while one is running
    expect(await useChats.getState().ask("r1", "again")).toBe(false);
    await useChats.getState().open("r1", "t1");
    answer.resolve({ thread: thread("t3", "2026-11-21T10:00:00Z"), question: message("t3", "user", "new question"), answer: message("t3", "assistant", "x") });
    await asking;
    expect(state().activeId).toBe("t1");
    expect(state().threads[0].id).toBe("t3");
  });

  it("keeps attachments when asking fails and sends them on retry", async () => {
    await useChats.getState().load("r1");
    mocked.addChatAudio.mockResolvedValue(voice);
    await useChats.getState().attachVoice("r1", new Float32Array(48_000), 48_000);
    expect(state().drafts).toHaveLength(1);
    expect(mocked.addChatAudio.mock.calls[0][1]).toBe(16_000); // resampled before upload
    mocked.ask.mockRejectedValueOnce("OpenRouter rate limit reached (429)");
    expect(await useChats.getState().ask("r1", "")).toBe(false);
    expect(state().drafts[0].attachment?.id).toBe("att-1");
    mocked.ask.mockResolvedValueOnce({
      thread: thread("t2", "2026-11-21T10:00:00Z", { messageCount: 4 }),
      question: message("t2", "user", "", [{ ...voice, text: "چه شد؟", textModel: "shenava-koochik" }]),
      answer: message("t2", "assistant", "a2"),
    });
    expect(await useChats.getState().ask("r1", "")).toBe(true);
    expect(mocked.ask).toHaveBeenLastCalledWith("r1", "t2", "", ["att-1"], expect.any(String));
    expect(state().drafts).toHaveLength(0);
    expect(state().messages.t2).toHaveLength(4);
  });

  it("waits for uploads and discards attachments removed mid-upload", async () => {
    await useChats.getState().load("r1");
    const upload = deferred<ChatAttachment>();
    mocked.addChatAudio.mockReturnValue(upload.promise);
    const attaching = useChats.getState().attachVoice("r1", new Float32Array(16_000), 16_000);
    await vi.waitFor(() => expect(state().drafts).toHaveLength(1));
    expect(await useChats.getState().ask("r1", "with the clip")).toBe(false);
    expect(mocked.ask).not.toHaveBeenCalled();
    useChats.getState().discardDraft("r1", state().drafts[0].key);
    upload.resolve(voice);
    await attaching;
    expect(state().drafts).toHaveLength(0);
    expect(mocked.discardChatAttachment).toHaveBeenCalledWith("att-1");
  });

  it("sent attachments leave the composer while the answer is pending", async () => {
    await useChats.getState().load("r1");
    mocked.addChatAudio.mockResolvedValue(voice);
    await useChats.getState().attachVoice("r1", new Float32Array(16_000), 16_000);
    const answer = deferred<AskResult>();
    mocked.ask.mockReturnValue(answer.promise);
    const asking = useChats.getState().ask("r1", "with a clip");
    expect(state().drafts).toHaveLength(0);
    expect(state().pending?.attachments.map((a) => a.id)).toEqual(["att-1"]);
    answer.reject("network down");
    await asking;
    expect(state().drafts.map((d) => d.attachment?.id)).toEqual(["att-1"]);
  });

  it("ignores a thread list fetched before a question finished", async () => {
    await useChats.getState().load("r1");
    await useChats.getState().open("r1", null);
    const stale = deferred<ChatThread[]>();
    mocked.listChatThreads.mockReturnValueOnce(stale.promise);
    const loading = useChats.getState().load("r1");
    mocked.ask.mockResolvedValue({ thread: thread("t3", "2026-11-21T10:00:00Z"), question: message("t3", "user", "q3"), answer: message("t3", "assistant", "a3") });
    await useChats.getState().ask("r1", "q3");
    stale.resolve([thread("t2", "2026-11-20T10:00:00Z"), thread("t1", "2026-11-19T10:00:00Z")]);
    await loading;
    expect(state().threads.map((t) => t.id)).toEqual(["t3", "t2", "t1"]);
    expect(state().activeId).toBe("t3");
  });

  it("recovers from a conversation that failed to load", async () => {
    mocked.listChatMessages.mockRejectedValueOnce("database is locked");
    await useChats.getState().load("r1");
    expect(state().loadErrors.t2).toBe("database is locked");
    expect(state().messages.t2).toBeUndefined();
    // asking anyway fetches the whole thread rather than showing only the new pair
    mocked.ask.mockResolvedValue({ thread: thread("t2", "2026-11-21T10:00:00Z", { messageCount: 4 }), question: message("t2", "user", "q2"), answer: message("t2", "assistant", "a2") });
    mocked.listChatMessages.mockResolvedValueOnce([message("t2", "user", "q"), message("t2", "assistant", "a"), message("t2", "user", "q2"), message("t2", "assistant", "a2")]);
    await useChats.getState().ask("r1", "q2");
    expect(state().messages.t2).toHaveLength(4);
    expect(state().loadErrors.t2).toBeUndefined();
  });

  it("rejects files that aren't images or audio", async () => {
    await useChats.getState().attachPaths("r1", ["/home/you/report.pdf"]);
    expect(mocked.addChatAttachmentFile).not.toHaveBeenCalled();
    expect(state().drafts).toHaveLength(0);
  });

  it("renames and deletes threads", async () => {
    await useChats.getState().load("r1");
    mocked.renameChatThread.mockResolvedValue(thread("t2", "2026-11-20T10:00:00Z", { title: "Renamed" }));
    expect(await useChats.getState().rename("r1", "t2", "Renamed")).toBe(true);
    expect(state().threads[0].title).toBe("Renamed");
    mocked.deleteChatThread.mockResolvedValue(undefined);
    await useChats.getState().remove("r1", "t2");
    expect(state().threads.map((t) => t.id)).toEqual(["t1"]);
    expect(state().activeId).toBeNull();
    expect(state().messages.t2).toBeUndefined();
  });
});
