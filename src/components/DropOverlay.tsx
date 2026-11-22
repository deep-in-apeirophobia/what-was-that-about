import { FileAudio, Paperclip } from "lucide-react";
import { useApp } from "../store/app";

export function DropOverlay() {
  const dragging = useApp((s) => s.dragging);
  const overChat = useApp((s) => s.dragOverChat);
  if (!dragging) return null;
  return (
    <div className="pointer-events-none fixed inset-0 z-[70] flex items-center justify-center bg-canvas/80 p-8 backdrop-blur-sm animate-fade-in">
      <div className="flex h-full w-full flex-col items-center justify-center rounded-3xl border-2 border-dashed border-accent bg-accent-soft/40">
        <div className="flex size-16 items-center justify-center rounded-2xl bg-accent text-accent-fg shadow-card">
          {overChat ? <Paperclip className="size-8" /> : <FileAudio className="size-8" />}
        </div>
        <div className="mt-4 text-lg font-semibold">{overChat ? "Drop to attach to your question" : "Drop to import"}</div>
        <div className="mt-1 text-[13px] text-muted">
          {overChat
            ? "Images (PNG, JPEG, WebP, GIF) or audio for a voice message"
            : "Audio or video files — MP3, M4A, WAV, OPUS, MP4, MKV and more"}
        </div>
      </div>
    </div>
  );
}
