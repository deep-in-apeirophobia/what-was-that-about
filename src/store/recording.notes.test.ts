import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Note, RecordingDetail } from "../lib/types";

vi.mock("../lib/api", () => ({
  api: {
    addNote: vi.fn(),
    updateNote: vi.fn(),
    deleteNote: vi.fn(),
  },
}));

const { api } = await import("../lib/api");
const { useRecording } = await import("./recording");
const { useToasts } = await import("./toasts");

const mocked = api as unknown as Record<"addNote" | "updateNote" | "deleteNote", ReturnType<typeof vi.fn>>;

function deferred<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const base: Note = {
  id: "n1",
  recordingId: "r1",
  start: 10,
  end: 12,
  body: "first",
  color: null,
  createdAt: "2026-11-01T10:00:00Z",
  updatedAt: "2026-11-01T10:00:00Z",
};

function open(notes: Note[] = [base]) {
  useRecording.setState({
    id: "r1",
    detail: { notes, segments: [], docs: [], threads: [], analysis: null } as unknown as RecordingDetail,
    noteDraft: null,
    noteEditor: null,
  });
}

const notes = () => useRecording.getState().detail!.notes;
const toastTitles = () => useToasts.getState().toasts.map((t) => t.title);

beforeEach(() => {
  vi.clearAllMocks();
  useToasts.setState({ toasts: [] });
  open();
});

describe("addNote", () => {
  it("shows the note at once, then swaps in the saved one", async () => {
    const d = deferred<Note>();
    mocked.addNote.mockReturnValue(d.promise);
    const p = useRecording.getState().addNote({ start: 2, end: 4, body: "new", color: "red" });
    expect(notes().map((n) => n.body)).toEqual(["new", "first"]);
    expect(notes()[0].id).toMatch(/^pending:/);
    d.resolve({ ...base, id: "n2", start: 2, end: 4, body: "new", color: "red" });
    await p;
    expect(notes().map((n) => n.id)).toEqual(["n2", "n1"]);
    expect(mocked.addNote).toHaveBeenCalledWith("r1", 2, 4, "new", "red");
  });

  it("removes the note and reopens the composer with the text when saving fails", async () => {
    mocked.addNote.mockRejectedValue(new Error("disk full"));
    useRecording.getState().startNote({ start: 2, end: 4, anchorSegmentId: 0 });
    await useRecording.getState().saveDraft("keep me", "green");
    expect(notes().map((n) => n.id)).toEqual(["n1"]);
    expect(useRecording.getState().noteDraft).toMatchObject({ start: 2, end: 4, body: "keep me", color: "green" });
    expect(toastTitles()).toContain("Couldn't save the note");
  });
});

describe("updateNote", () => {
  it("applies the edit optimistically and reverts on failure", async () => {
    const d = deferred<Note>();
    mocked.updateNote.mockReturnValue(d.promise);
    const p = useRecording.getState().updateNote("n1", { start: 10, end: 12, body: "edited", color: "blue" });
    expect(notes()[0]).toMatchObject({ body: "edited", color: "blue" });
    d.reject(new Error("nope"));
    expect(await p).toBe(false);
    expect(notes()[0]).toMatchObject({ body: "first", color: null });
    expect(toastTitles()).toContain("Couldn't save the note");
  });
});

describe("deleteNote", () => {
  it("removes the note and offers undo", async () => {
    mocked.deleteNote.mockResolvedValue(undefined);
    await useRecording.getState().deleteNote("n1");
    expect(notes()).toEqual([]);
    const t = useToasts.getState().toasts.find((x) => x.title === "Note deleted");
    expect(t?.action?.label).toBe("Undo");
    mocked.addNote.mockResolvedValue({ ...base, id: "n3" });
    t!.action!.onClick();
    await vi.waitFor(() => expect(notes().map((n) => n.id)).toEqual(["n3"]));
  });

  it("puts the note back when deleting fails", async () => {
    mocked.deleteNote.mockRejectedValue(new Error("locked"));
    await useRecording.getState().deleteNote("n1");
    expect(notes().map((n) => n.id)).toEqual(["n1"]);
    expect(toastTitles()).toContain("Couldn't delete the note");
  });

  it("ignores results for a recording that is no longer open", async () => {
    const d = deferred<Note>();
    mocked.addNote.mockReturnValue(d.promise);
    const p = useRecording.getState().addNote({ start: 1, end: 2, body: "late", color: null });
    open([]);
    useRecording.setState({ id: "r2" });
    d.resolve({ ...base, id: "late" });
    await p;
    expect(notes()).toEqual([]);
  });
});
