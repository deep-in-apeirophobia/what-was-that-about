import { beforeEach, describe, expect, it, vi } from "vitest";
import type { FixSummary, RecordingDetail, Segment } from "../../lib/types";

vi.mock("../../lib/api", () => ({
  api: {
    fixTranscript: vi.fn(),
    revertTranscriptFixes: vi.fn(),
  },
}));

const { api } = await import("../../lib/api");
const { fixTranscript, revertTranscriptFixes } = await import("./actions");
const { useLibrary } = await import("../../store/library");
const { useRecording } = await import("../../store/recording");
const { useToasts } = await import("../../store/toasts");

const mocked = api as unknown as Record<"fixTranscript" | "revertTranscriptFixes", ReturnType<typeof vi.fn>>;

const seg = (id: number, text: string, originalText?: string): Segment => ({
  id,
  start: id,
  end: id + 1,
  text,
  words: [],
  originalText,
});

function open(segments: Segment[]) {
  useRecording.setState({
    id: "r1",
    detail: { segments, notes: [], docs: [], chat: [], analysis: null } as unknown as RecordingDetail,
  });
}

const texts = () => useRecording.getState().detail!.segments.map((s) => s.text);
const toastTitles = () => useToasts.getState().toasts.map((t) => t.title);

beforeEach(() => {
  vi.clearAllMocks();
  useToasts.setState({ toasts: [] });
  useLibrary.setState({ fixing: {} });
  open([seg(0, "سلام"), seg(1, "خداحافز")]);
});

describe("fixTranscript", () => {
  it("tracks progress, applies changed sentences and reports the result", async () => {
    let finish!: (s: FixSummary) => void;
    mocked.fixTranscript.mockReturnValue(new Promise<FixSummary>((r) => (finish = r)));
    const run = fixTranscript("r1");
    expect(useLibrary.getState().fixing.r1).toEqual({ done: 0, total: 0 });

    // what the backend event handler does for `transcript://fix-progress`
    useLibrary.getState().applyFixProgress({ recordingId: "r1", done: 1, total: 2, changed: [] });
    useRecording.getState().updateSegments("r1", [seg(1, "خداحافظ", "خداحافز"), seg(7, "unknown")]);
    expect(useLibrary.getState().fixing.r1).toEqual({ done: 1, total: 2 });
    expect(texts()).toEqual(["سلام", "خداحافظ"]);

    finish({ changed: 1, batches: 2, failed: 0, error: null });
    await run;
    expect(useLibrary.getState().fixing.r1).toBeUndefined();
    expect(toastTitles()).toEqual(["Transcript fixed"]);
  });

  it("ignores progress for fixes this window didn't start", () => {
    useLibrary.getState().applyFixProgress({ recordingId: "r2", done: 1, total: 2, changed: [] });
    expect(useLibrary.getState().fixing).toEqual({});
  });

  it("reports failed parts, and a stop as information rather than an error", async () => {
    mocked.fixTranscript.mockResolvedValueOnce({ changed: 0, batches: 3, failed: 1, error: "OpenRouter rate limit reached (429)" });
    await fixTranscript("r1");
    expect(toastTitles()).toEqual(["1 of 3 parts couldn't be fixed"]);

    useToasts.setState({ toasts: [] });
    mocked.fixTranscript.mockRejectedValueOnce("cancelled");
    await fixTranscript("r1");
    expect(toastTitles()).toEqual(["Stopped fixing"]);
    expect(useLibrary.getState().fixing).toEqual({});
  });
});

describe("revertTranscriptFixes", () => {
  it("puts the returned sentences back in the transcript", async () => {
    open([seg(0, "سلام"), seg(1, "خداحافظ", "خداحافز")]);
    mocked.revertTranscriptFixes.mockResolvedValueOnce([seg(1, "خداحافز")]);
    expect(await revertTranscriptFixes("r1", [1])).toBe(true);
    expect(mocked.revertTranscriptFixes).toHaveBeenCalledWith("r1", [1]);
    expect(texts()).toEqual(["سلام", "خداحافز"]);
    expect(useRecording.getState().detail!.segments[1].originalText).toBeUndefined();
  });

  it("leaves another recording's transcript alone", async () => {
    mocked.revertTranscriptFixes.mockResolvedValueOnce([seg(1, "x")]);
    await revertTranscriptFixes("r2", null);
    expect(texts()).toEqual(["سلام", "خداحافز"]);
  });
});
