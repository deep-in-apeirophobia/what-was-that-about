import { create } from "zustand";
import { api } from "../lib/api";
import type { Settings, SystemInfo } from "../lib/types";
import { attempt } from "./toasts";

interface SettingsState {
  settings: Settings | null;
  system: SystemInfo | null;
  load: () => Promise<void>;
  loadSystem: () => Promise<void>;
  /**
   * Apply a change optimistically and persist the whole settings object. Saves are
   * serialized so quick successive changes can't overwrite each other. On failure the
   * changed keys are reverted and a toast explains why. Resolves to success.
   */
  patch: (partial: Partial<Settings>) => Promise<boolean>;
}

let queue: Promise<unknown> = Promise.resolve();

export const useSettings = create<SettingsState>((set, get) => ({
  settings: null,
  system: null,

  load: async () => {
    const settings = await attempt("Couldn't load settings", () => api.getSettings());
    if (settings) set({ settings });
  },

  loadSystem: async () => {
    const system = await attempt("Couldn't read system information", () => api.getSystemInfo());
    if (system) set({ system });
  },

  patch: (partial) => {
    const before = get().settings;
    if (!before) return Promise.resolve(false);
    set({ settings: { ...before, ...partial } });
    const run = async () => {
      const toSave = get().settings!;
      const saved = await attempt("Couldn't save settings", () => api.saveSettings(toSave));
      if (saved) {
        // Only adopt the server copy if nothing changed locally in the meantime.
        if (get().settings === toSave) set({ settings: saved });
        return true;
      }
      const revert: Partial<Settings> = {};
      for (const k of Object.keys(partial) as (keyof Settings)[]) {
        (revert as Record<string, unknown>)[k] = before[k];
      }
      set({ settings: { ...get().settings!, ...revert } });
      return false;
    };
    const p = queue.then(run, run);
    queue = p;
    return p;
  },
}));

export const hasApiKey = (s: Settings | null) => !!s?.openrouterApiKey.trim();
