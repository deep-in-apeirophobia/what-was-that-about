import { useEffect } from "react";
import { AlertCircle, ArrowLeft } from "lucide-react";
import { isTypingTarget } from "../lib/platform";
import { indexOfSegmentId, nextSegmentTime, previousSegmentTime } from "../lib/segments";
import { useApp } from "../store/app";
import { usePlayer } from "../store/player";
import { useRecording } from "../store/recording";
import { Button } from "../components/ui/Button";
import { isOverlayOpen } from "../components/ui/Dialog";
import { Spinner } from "../components/ui/misc";
import { PlayerBar } from "../components/recording/PlayerBar";
import { RecordingHeader } from "../components/recording/RecordingHeader";
import { SidePanel } from "../components/recording/SidePanel";
import { Transcript } from "../components/recording/Transcript";

/** Space, arrows and Ctrl/Cmd+F while a recording is open. */
function usePlayerShortcuts() {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented || isOverlayOpen()) return;
      const mod = e.metaKey || e.ctrlKey;
      if (mod && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "f") {
        const rec = useRecording.getState();
        if (!rec.detail?.segments.length) return;
        e.preventDefault();
        // Toggle off/on so the find input re-focuses even if already open.
        rec.setFindOpen(false);
        requestAnimationFrame(() => useRecording.getState().setFindOpen(true));
        return;
      }
      if (mod || e.altKey || isTypingTarget(e.target)) return;
      const player = usePlayer.getState();
      const segments = useRecording.getState().detail?.segments ?? [];
      if (e.key === " " || e.code === "Space") {
        // Space means play/pause, even on a focused button (preventDefault stops the click).
        // Widgets where Space has its own meaning keep it.
        const t = e.target as HTMLElement | null;
        if (t?.closest("[role=menuitem], [role=menuitemcheckbox], [role=switch], [role=radio], [role=option], [role=checkbox]")) return;
        e.preventDefault();
        player.toggle();
      } else if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
        const t = e.target as HTMLElement | null;
        if (t?.closest("[role=tablist], [role=radiogroup], [role=menu]")) return;
        e.preventDefault();
        const forward = e.key === "ArrowRight";
        if (e.shiftKey) {
          if (forward) {
            const next = nextSegmentTime(segments, player.time);
            if (next != null) player.seek(next);
          } else player.seek(previousSegmentTime(segments, player.time));
        } else player.skip(forward ? 5 : -5);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
}

/** Apply seeks requested before/while navigating here (search, citations, topics). */
function usePendingSeek(id: string, ready: boolean) {
  const pending = useApp((s) => s.pendingSeek);
  useEffect(() => {
    if (!ready || !pending || pending.recordingId !== id) return;
    const req = useApp.getState().takeSeek(id);
    if (!req) return;
    const segments = useRecording.getState().detail?.segments ?? [];
    let time = req.time;
    if (req.segmentId != null) {
      const i = indexOfSegmentId(segments, req.segmentId);
      if (i >= 0) time = segments[i].start + 0.001;
      useRecording.getState().revealSegment(req.segmentId);
    }
    if (time != null || req.noteId) {
      // Wait a frame so the transcript rows exist before scrolling/highlighting.
      requestAnimationFrame(() => {
        if (time != null) usePlayer.getState().seek(time, req.play);
        if (req.noteId) useRecording.getState().revealNote(req.noteId);
      });
    }
  }, [id, ready, pending]);
}

export function RecordingView({ id }: { id: string }) {
  const open = useRecording((s) => s.open);
  const detail = useRecording((s) => (s.id === id ? s.detail : null));
  const loading = useRecording((s) => s.loading);
  const error = useRecording((s) => s.error);
  const waveform = useRecording((s) => s.waveform);
  const waveformError = useRecording((s) => s.waveformError);
  const navigate = useApp((s) => s.navigate);

  useEffect(() => {
    void open(id);
  }, [id, open]);

  usePlayerShortcuts();
  usePendingSeek(id, !!detail);

  if (!detail) {
    if (error && !loading) {
      return (
        <div className="flex h-full flex-col items-center justify-center gap-3 p-8 text-center">
          <AlertCircle className="size-8 text-danger" />
          <div className="text-[15px] font-semibold">Couldn't open this recording</div>
          <div className="max-w-md text-[13px] text-muted">{error}</div>
          <div className="mt-2 flex gap-2">
            <Button icon={<ArrowLeft className="size-3.5" />} onClick={() => navigate({ name: "library" })}>
              Back to library
            </Button>
            <Button variant="primary" onClick={() => void useRecording.getState().reload()}>
              Try again
            </Button>
          </div>
        </div>
      );
    }
    return (
      <div className="flex h-full items-center justify-center">
        <Spinner />
      </div>
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      <RecordingHeader detail={detail} />
      <div className="flex min-h-0 flex-1">
        <section className="flex min-w-0 flex-1 flex-col" aria-label="Transcript">
          <Transcript />
        </section>
        <SidePanel />
      </div>
      <PlayerBar
        recording={detail.recording}
        segments={detail.segments}
        waveform={waveform}
        waveformError={waveformError}
        chapters={detail.analysis?.chapters ?? []}
        notes={detail.notes}
      />
    </div>
  );
}
