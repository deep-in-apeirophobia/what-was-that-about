import { create } from "zustand";
import { errorMessage } from "../lib/errors";

export type ToastKind = "success" | "error" | "info";

export interface Toast {
  id: number;
  kind: ToastKind;
  title: string;
  message?: string;
  action?: { label: string; onClick: () => void };
}

interface ToastState {
  toasts: Toast[];
  push: (t: Omit<Toast, "id">, timeoutMs?: number) => number;
  dismiss: (id: number) => void;
}

let nextId = 1;

export const useToasts = create<ToastState>((set, get) => ({
  toasts: [],
  push: (t, timeoutMs) => {
    const id = nextId++;
    // Identical errors in a burst collapse into one toast.
    const dup = get().toasts.find((x) => x.kind === t.kind && x.title === t.title && x.message === t.message);
    if (dup) return dup.id;
    set((s) => ({ toasts: [...s.toasts.slice(-4), { ...t, id }] }));
    const ms = timeoutMs ?? (t.kind === "error" ? 8000 : 4000);
    if (ms > 0) setTimeout(() => get().dismiss(id), ms);
    return id;
  },
  dismiss: (id) => set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })),
}));

export const toast = {
  success: (title: string, message?: string, action?: Toast["action"]) =>
    useToasts.getState().push({ kind: "success", title, message, action }),
  info: (title: string, message?: string, action?: Toast["action"]) =>
    useToasts.getState().push({ kind: "info", title, message, action }),
  error: (title: string, error?: unknown, action?: Toast["action"]) =>
    useToasts.getState().push({
      kind: "error",
      title,
      message: error === undefined ? undefined : errorMessage(error),
      action,
    }),
};

/** Run an API call; on failure show a readable toast and resolve to undefined. */
export async function attempt<T>(title: string, fn: () => Promise<T>): Promise<T | undefined> {
  try {
    return await fn();
  } catch (e) {
    console.error(title, e);
    toast.error(title, e);
    return undefined;
  }
}
