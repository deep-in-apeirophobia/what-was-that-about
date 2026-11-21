import { create } from "zustand";
import { api } from "../lib/api";
import type { ModelInfo, ModelProgress } from "../lib/types";
import { attempt, toast } from "./toasts";

interface ModelsState {
  models: ModelInfo[];
  loaded: boolean;
  progress: Record<string, ModelProgress>;
  refresh: () => Promise<void>;
  applyProgress: (p: ModelProgress) => void;
  install: (id: string) => Promise<void>;
  cancel: (id: string) => Promise<void>;
  remove: (id: string) => Promise<void>;
  importFiles: (id: string, paths: string[]) => Promise<void>;
}

const nameOf = (models: ModelInfo[], id: string) => models.find((m) => m.id === id)?.name ?? id;

export const useModels = create<ModelsState>((set, get) => ({
  models: [],
  loaded: false,
  progress: {},

  refresh: async () => {
    const models = await attempt("Couldn't load speech models", () => api.listModels());
    set({ models: models ?? get().models, loaded: true });
  },

  applyProgress: (p) => {
    const finished = p.stage === "done" || p.stage === "error" || p.stage === "cancelled";
    set((s) => {
      const progress = { ...s.progress };
      if (finished) delete progress[p.modelId];
      else progress[p.modelId] = p;
      const status: ModelInfo["status"] | null =
        p.stage === "downloading" ? "downloading" : p.stage === "converting" ? "converting" : null;
      const models = status
        ? s.models.map((m) => (m.id === p.modelId && m.status !== status ? { ...m, status } : m))
        : s.models;
      return { progress, models };
    });
    if (!finished) return;
    const name = nameOf(get().models, p.modelId);
    if (p.stage === "done") toast.success(`${name} is ready`, "You can transcribe recordings with it now.");
    if (p.stage === "error") toast.error(`Couldn't install ${name}`, p.message ?? "Unknown error");
    void get().refresh();
  },

  install: async (id) => {
    set((s) => ({
      models: s.models.map((m) => (m.id === id ? { ...m, status: "downloading", error: null } : m)),
    }));
    const ok = await attempt(`Couldn't start installing ${nameOf(get().models, id)}`, async () => {
      await api.installModel(id);
      return true;
    });
    if (!ok) void get().refresh();
  },

  cancel: async (id) => {
    await attempt("Couldn't cancel the install", () => api.cancelModelInstall(id));
  },

  remove: async (id) => {
    const ok = await attempt("Couldn't delete the model", async () => {
      await api.deleteModel(id);
      return true;
    });
    if (ok) toast.success(`${nameOf(get().models, id)} removed`);
    await get().refresh();
  },

  importFiles: async (id, paths) => {
    if (!paths.length) return;
    const ok = await attempt("Couldn't import the model files", async () => {
      await api.importModelFiles(id, paths);
      return true;
    });
    if (ok) toast.info("Importing model files…", "Progress is shown on the model card.");
    await get().refresh();
  },
}));

export const hasReadyModel = (models: ModelInfo[]) => models.some((m) => m.status === "ready");
