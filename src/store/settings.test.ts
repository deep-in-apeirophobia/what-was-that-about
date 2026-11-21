import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Settings } from "../lib/types";

const saveSettings = vi.fn<(s: Settings) => Promise<Settings>>();
vi.mock("../lib/api", () => ({ api: { saveSettings: (s: Settings) => saveSettings(s) } }));

const { useSettings } = await import("./settings");
const { useToasts } = await import("./toasts");

const base: Settings = {
  openrouterApiKey: "",
  llmModel: "deepseek/deepseek-v4-flash",
  embeddingModel: "baai/bge-m3",
  aiLanguage: "auto",
  defaultModelId: "taraneh-wlarge",
  useGpu: true,
  threads: 0,
  copyIntoLibrary: true,
  theme: "system",
  whisperQuant: "q8_0",
  keepSourceWeights: false,
  persianDigits: true,
  preciseWordTimings: false,
  autoAnalyze: true,
  analyticsEnabled: true,
  crashReportsEnabled: true,
};

describe("useSettings.patch", () => {
  beforeEach(() => {
    saveSettings.mockReset();
    useSettings.setState({ settings: { ...base } });
    useToasts.setState({ toasts: [] });
  });

  it("applies changes optimistically and keeps both of two quick edits", async () => {
    saveSettings.mockImplementation(async (s) => {
      await new Promise((r) => setTimeout(r, 5));
      return s;
    });
    const a = useSettings.getState().patch({ theme: "dark" });
    expect(useSettings.getState().settings?.theme).toBe("dark");
    const b = useSettings.getState().patch({ threads: 4 });
    await Promise.all([a, b]);
    // Saves are serialized and each sends the latest local state.
    expect(saveSettings).toHaveBeenCalledTimes(2);
    expect(saveSettings.mock.calls[1][0]).toMatchObject({ theme: "dark", threads: 4 });
    expect(useSettings.getState().settings).toMatchObject({ theme: "dark", threads: 4 });
  });

  it("reverts only the failed keys and shows a toast", async () => {
    saveSettings.mockRejectedValueOnce("disk is read-only");
    const ok = await useSettings.getState().patch({ autoAnalyze: false });
    expect(ok).toBe(false);
    expect(useSettings.getState().settings?.autoAnalyze).toBe(true);
    const toasts = useToasts.getState().toasts;
    expect(toasts).toHaveLength(1);
    expect(toasts[0]).toMatchObject({ kind: "error", message: "disk is read-only" });
  });

  it("does nothing before settings are loaded", async () => {
    useSettings.setState({ settings: null });
    expect(await useSettings.getState().patch({ theme: "dark" })).toBe(false);
    expect(saveSettings).not.toHaveBeenCalled();
  });
});
