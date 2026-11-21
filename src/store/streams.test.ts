import { describe, expect, it } from "vitest";
import type { LlmStreamEvent } from "../lib/types";
import { useStreams } from "./streams";

const ev = (e: Partial<LlmStreamEvent>) =>
  useStreams.getState().apply({ requestId: "r", delta: null, done: false, error: null, ...e });

describe("useStreams agent progress", () => {
  it("shows activity until answer text arrives, and drops text before tool calls", () => {
    useStreams.getState().start("r");
    ev({ delta: "Let me check." });
    ev({ reset: true });
    ev({ activity: "Reading the transcript" });
    expect(useStreams.getState().streams.r).toMatchObject({ text: "", activity: "Reading the transcript" });
    ev({ delta: "The answer" });
    ev({ delta: " [12]." });
    expect(useStreams.getState().streams.r).toMatchObject({ text: "The answer [12].", activity: null });
    ev({ done: true });
    expect(useStreams.getState().streams.r.done).toBe(true);
  });

  it("ignores requests it didn't start", () => {
    useStreams.getState().clear("r");
    ev({ activity: "Searching" });
    expect(useStreams.getState().streams.r).toBeUndefined();
  });
});
