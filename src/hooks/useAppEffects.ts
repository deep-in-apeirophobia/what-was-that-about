import { useEffect } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";
import {
  onLlmStream,
  onModelProgress,
  onRecordingDeleted,
  onRecordingUpdated,
  onTranscriptionProgress,
} from "../lib/api";
import { listenFileDrop, isTypingTarget } from "../lib/platform";
import { useApp } from "../store/app";
import { useChats } from "../store/chat";
import { useLibrary } from "../store/library";
import { useModels } from "../store/models";
import { useRecording } from "../store/recording";
import { useSettings } from "../store/settings";
import { useStreams } from "../store/streams";
import { toast } from "../store/toasts";
import { isOverlayOpen } from "../components/ui/Dialog";
import { chatScopeAt } from "../components/Chat";

/** Collect async unlisten functions and dispose them safely on unmount. */
function useSubscriptions(setup: () => Promise<UnlistenFn>[]): void {
  useEffect(() => {
    let disposed = false;
    const unsubs: UnlistenFn[] = [];
    for (const p of setup()) {
      p.then((u) => (disposed ? u() : unsubs.push(u))).catch((e) => {
        console.error("event subscription failed", e);
      });
    }
    return () => {
      disposed = true;
      unsubs.forEach((u) => u());
    };
  }, []);
}

/** Wire backend events into the stores. Mounted once by <App>. */
export function useBackendEvents(): void {
  useSubscriptions(() => [
    onRecordingUpdated((r) => {
      const prev = useLibrary.getState().recordings.find((x) => x.id === r.id);
      useLibrary.getState().upsert(r);
      useRecording.getState().applyRecording(r);
      const route = useApp.getState().route;
      const viewing = route.name === "recording" && route.id === r.id;
      if (prev && prev.status !== "ready" && r.status === "ready" && (prev.status === "transcribing" || prev.status === "queued") && !viewing) {
        toast.success("Transcript ready", r.title, {
          label: "Open",
          onClick: () => useApp.getState().openRecording(r.id),
        });
      }
    }),
    onRecordingDeleted((id) => {
      useLibrary.getState().remove(id);
      const route = useApp.getState().route;
      if (route.name === "recording" && route.id === id) {
        useApp.getState().navigate({ name: "library" });
        useRecording.getState().close();
      }
    }),
    onTranscriptionProgress((p) => {
      useLibrary.getState().applyProgress(p);
      if (p.newSegments?.length) useRecording.getState().appendSegments(p.recordingId, p.newSegments);
    }),
    onModelProgress((p) => useModels.getState().applyProgress(p)),
    onLlmStream((e) => useStreams.getState().apply(e)),
  ]);
}

/** Initial data load. */
export function useInitialLoad(): void {
  useEffect(() => {
    void useSettings.getState().load();
    void useSettings.getState().loadSystem();
    void useModels.getState().refresh();
    void useLibrary.getState().refresh();
  }, []);
}

/** Apply settings.theme ("system" follows the OS) to <html>. */
export function useTheme(): void {
  const theme = useSettings((s) => s.settings?.theme ?? "system");
  useEffect(() => {
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const apply = () => {
      const dark = theme === "dark" || (theme === "system" && mq.matches);
      document.documentElement.classList.toggle("dark", dark);
      useApp.getState().setDark(dark);
    };
    apply();
    mq.addEventListener("change", apply);
    return () => mq.removeEventListener("change", apply);
  }, [theme]);
}

/** Whole-window file drag & drop → import, or attach when dropped on an Ask chat. */
export function useFileDrop(): void {
  useSubscriptions(() => [
    listenFileDrop((e) => {
      const app = useApp.getState();
      if (e.type === "enter") app.setDragging(true);
      else if (e.type === "leave") app.setDragging(false);
      else if (e.type === "over") app.setDragOverChat(chatScopeAt(e.x, e.y) !== undefined);
      else if (e.type === "drop") {
        app.setDragging(false);
        const scope = chatScopeAt(e.x, e.y);
        if (scope === undefined) void useLibrary.getState().importPaths(e.paths);
        else if (e.files.length) void useChats.getState().attachFiles(scope, e.files);
        else void useChats.getState().attachPaths(scope, e.paths);
      }
    }),
  ]);
}

/** App-wide shortcuts: Ctrl/Cmd+K search, ? help. Player keys live in the recording view. */
export function useGlobalShortcuts(): void {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = e.metaKey || e.ctrlKey;
      if (mod && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "k") {
        e.preventDefault();
        const app = useApp.getState();
        app.setSearchOpen(!app.searchOpen);
        return;
      }
      if (e.key === "?" && !mod && !isTypingTarget(e.target) && !isOverlayOpen()) {
        e.preventDefault();
        useApp.getState().setShortcutsOpen(true);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
}
