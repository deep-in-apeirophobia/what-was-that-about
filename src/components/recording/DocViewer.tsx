import { useEffect, useMemo, useRef, useState } from "react";
import { Pause, Play, Trash2, Wand2, X } from "lucide-react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { api, isTauri } from "../../lib/api";
import { bridgeDataFrom, buildDocSrcdoc, parseBridgeMessage } from "../../lib/docBridge";
import { formatBytes, formatTime } from "../../lib/format";
import { newRequestId } from "../../lib/platform";
import type { Doc } from "../../lib/types";
import { isCancellation, stopLlm } from "../../store/chat";
import { usePlayer } from "../../store/player";
import { useRecording } from "../../store/recording";
import { useStreams } from "../../store/streams";
import { attempt, toast } from "../../store/toasts";
import { Button, IconButton } from "../ui/Button";
import { ConfirmDialog, Dialog } from "../ui/Dialog";
import { inputClass } from "../ui/misc";

/**
 * Untrusted AI-generated HTML in a sandboxed iframe (scripts only — no same-origin,
 * popups, forms or navigation), talking to the player through validated postMessage.
 */
export function DocFrame({ doc, title, onEscape }: { doc: Doc; title: string; onEscape?: () => void }) {
  const detail = useRecording((s) => s.detail)!;
  const frame = useRef<HTMLIFrameElement>(null);
  const escapeRef = useRef(onEscape);
  escapeRef.current = onEscape;
  const { id, title: recTitle, durationSec: duration } = detail.recording;
  // Depend on the fields the doc sees, so unrelated recording updates don't reload the frame.
  // In the desktop app the backend serves the doc (with the bridge injected) from its own
  // `wwta-doc:` origin and CSP; srcdoc would inherit the app's CSP, which blocks inline scripts.
  const src = isTauri() ? `${convertFileSrc(doc.id, "wwta-doc")}?v=${encodeURIComponent(doc.updatedAt)}` : null;
  const srcdoc = useMemo(
    () =>
      src ? undefined : buildDocSrcdoc(
        doc.html,
        bridgeDataFrom({ ...detail.recording, id, title: recTitle, durationSec: duration }, detail.segments, detail.analysis),
      ),
    [src, doc.html, id, recTitle, duration, detail.segments, detail.analysis],
  );

  useEffect(() => {
    const onMessage = (ev: MessageEvent) => {
      const win = frame.current?.contentWindow;
      if (!win || ev.source !== win) return;
      const cmd = parseBridgeMessage(ev.data, duration ?? usePlayer.getState().duration);
      if (!cmd) return;
      const player = usePlayer.getState();
      if (cmd.type === "wwta:seek") player.seek(cmd.time);
      else if (cmd.type === "wwta:play") player.play();
      else if (cmd.type === "wwta:pause") player.pause();
      else escapeRef.current?.();
    };
    window.addEventListener("message", onMessage);

    let lastTime = -1;
    let lastPlaying: boolean | null = null;
    const post = () => {
      const win = frame.current?.contentWindow;
      if (!win) return;
      const { time, playing } = usePlayer.getState();
      lastTime = time;
      lastPlaying = playing;
      // Opaque-origin frame: "*" is the only target that works; the payload is not sensitive.
      win.postMessage({ type: "wwta:time", time, playing }, "*");
    };
    const unsub = usePlayer.subscribe((s) => {
      if (s.playing !== lastPlaying || Math.abs(s.time - lastTime) >= 0.25) post();
    });
    const el = frame.current;
    el?.addEventListener("load", post);
    return () => {
      window.removeEventListener("message", onMessage);
      unsub();
      el?.removeEventListener("load", post);
    };
  }, [duration]);

  return (
    <iframe
      ref={frame}
      title={title}
      sandbox="allow-scripts"
      referrerPolicy="no-referrer"
      {...(src ? { src } : { srcDoc: srcdoc })}
      // Inside an iframe, prefers-color-scheme follows the frame's color-scheme, so docs
      // match the app's theme rather than the OS setting.
      style={{ colorScheme: document.documentElement.classList.contains("dark") ? "dark" : "light" }}
      className="h-full w-full border-0 bg-white"
    />
  );
}

function MiniTransport() {
  const playing = usePlayer((s) => s.playing);
  const time = usePlayer((s) => formatTime(s.time));
  return (
    <div className="flex items-center gap-1.5 rounded-lg border border-line px-1 py-0.5">
      <IconButton label={playing ? "Pause" : "Play"} size="xs" onClick={() => usePlayer.getState().toggle()}>
        {playing ? <Pause className="size-3.5" fill="currentColor" /> : <Play className="size-3.5" fill="currentColor" />}
      </IconButton>
      <span className="pr-1.5 text-[12px] text-muted tabular-nums">{time}</span>
    </div>
  );
}

export function DocViewer({ doc, onClose }: { doc: Doc; onClose: () => void }) {
  const [instruction, setInstruction] = useState("");
  const [revising, setRevising] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const streamLen = useStreams((s) => (revising ? (s.streams[revising]?.text.length ?? 0) : 0));

  const revise = async () => {
    const text = instruction.trim();
    if (!text || revising) return;
    const requestId = newRequestId();
    useStreams.getState().start(requestId);
    setRevising(requestId);
    try {
      const updated = await api.reviseDoc(doc.id, text, requestId);
      useRecording.getState().upsertDoc(updated);
      setInstruction("");
      toast.success("Doc updated");
    } catch (e) {
      if (isCancellation(e)) toast.info("Revision stopped");
      else toast.error("Couldn't revise the doc", e);
    } finally {
      useStreams.getState().clear(requestId);
      setRevising(null);
    }
  };

  const remove = async () => {
    const ok = await attempt("Couldn't delete the doc", async () => {
      await api.deleteDoc(doc.id);
      return true;
    });
    if (ok) {
      useRecording.getState().removeDoc(doc.id);
      toast.success("Doc deleted");
      onClose();
    }
  };

  return (
    <Dialog open onClose={onClose} size="full" bare>
      <div className="flex h-full flex-col">
        <div className="flex items-center gap-3 border-b border-line px-4 py-2.5">
          <div className="min-w-0">
            <h2 className="truncate text-[14px] font-semibold" dir="auto">
              {doc.title}
            </h2>
            <p className="truncate text-[11.5px] text-faint" dir="auto" title={doc.prompt}>
              {doc.prompt}
            </p>
          </div>
          <MiniTransport />
          <form
            className="ml-auto flex w-[min(460px,40vw)] items-center gap-2"
            onSubmit={(e) => {
              e.preventDefault();
              void revise();
            }}
          >
            <input
              value={instruction}
              onChange={(e) => setInstruction(e.target.value)}
              placeholder="Ask for a change… e.g. “add a dark theme”"
              aria-label="Revision instruction"
              dir="auto"
              disabled={!!revising}
              className={inputClass}
            />
            {revising ? (
              <Button onClick={() => stopLlm(revising)}>Stop</Button>
            ) : (
              <Button type="submit" variant="primary" icon={<Wand2 className="size-3.5" />} disabled={!instruction.trim()}>
                Revise
              </Button>
            )}
          </form>
          <IconButton label="Delete doc" variant="danger-ghost" onClick={() => setConfirmDelete(true)}>
            <Trash2 className="size-4" />
          </IconButton>
          <IconButton label="Close (Esc)" onClick={onClose}>
            <X className="size-4" />
          </IconButton>
        </div>
        <div className="relative min-h-0 flex-1">
          <DocFrame key={doc.updatedAt} doc={doc} title={doc.title} onEscape={onClose} />
          {revising && (
            <div className="absolute inset-x-0 top-0 flex justify-center p-3">
              <div className="flex items-center gap-2 rounded-full border border-line bg-surface px-3.5 py-1.5 text-[12.5px] shadow-pop">
                <Wand2 className="size-3.5 animate-pulse text-accent" />
                Revising… {streamLen > 0 && <span className="text-muted tabular-nums">{formatBytes(streamLen)}</span>}
              </div>
            </div>
          )}
        </div>
      </div>
      <ConfirmDialog
        open={confirmDelete}
        title="Delete this doc?"
        message="It will be removed from this recording. You can always generate a new one."
        confirmLabel="Delete doc"
        danger
        onCancel={() => setConfirmDelete(false)}
        onConfirm={() => {
          setConfirmDelete(false);
          void remove();
        }}
      />
    </Dialog>
  );
}
