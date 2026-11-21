import clsx from "clsx";
import { AlertCircle, Loader2 } from "lucide-react";
import type { Recording, TranscriptionStage } from "../lib/types";
import type { ProgressInfo } from "../store/library";
import { Badge } from "./ui/misc";

export const STAGE_LABELS: Record<TranscriptionStage, string> = {
  queued: "Queued",
  decoding: "Decoding audio",
  loadingModel: "Loading model",
  transcribing: "Transcribing",
  saving: "Saving",
  done: "Done",
  error: "Error",
  cancelled: "Cancelled",
};

export function isRunning(r: Pick<Recording, "status">): boolean {
  return r.status === "queued" || r.status === "transcribing";
}

/** Overall 0..1 progress for a running transcription (stage-weighted). */
export function overallProgress(p: ProgressInfo | undefined): number | null {
  if (!p) return null;
  switch (p.stage) {
    case "queued":
      return 0;
    case "decoding":
      return 0.02 + p.progress * 0.06;
    case "loadingModel":
      return 0.08 + p.progress * 0.04;
    case "transcribing":
      return 0.12 + p.progress * 0.85;
    case "saving":
      return 0.98;
    default:
      return null;
  }
}

export function StatusDot({ recording, className }: { recording: Recording; className?: string }) {
  const { status } = recording;
  if (status === "transcribing" || status === "queued")
    return <Loader2 className={clsx("size-3 shrink-0 animate-spin text-accent", className)} aria-label="Transcribing" />;
  const color =
    status === "ready"
      ? "bg-ok"
      : status === "error"
        ? "bg-danger"
        : status === "needsDecode"
          ? "bg-warn"
          : "bg-line-strong";
  const label =
    status === "ready" ? "Transcribed" : status === "error" ? "Error" : status === "needsDecode" ? "Needs decoding" : "Not transcribed";
  return <span className={clsx("inline-block size-2 shrink-0 rounded-full", color, className)} role="img" aria-label={label} title={label} />;
}

export function StatusBadge({ recording, progress }: { recording: Recording; progress?: ProgressInfo }) {
  switch (recording.status) {
    case "new":
      return <Badge>Not transcribed</Badge>;
    case "needsDecode":
      return <Badge tone="warn">Needs decoding</Badge>;
    case "queued":
      return (
        <Badge tone="accent">
          <Loader2 className="size-3 animate-spin" /> Queued
        </Badge>
      );
    case "transcribing": {
      const pct = overallProgress(progress);
      return (
        <Badge tone="accent">
          <Loader2 className="size-3 animate-spin" />
          {progress ? STAGE_LABELS[progress.stage] : "Transcribing"}
          {pct != null && progress?.stage === "transcribing" ? ` ${Math.round(pct * 100)}%` : ""}
        </Badge>
      );
    }
    case "error":
      return (
        <Badge tone="danger" title={recording.error ?? undefined}>
          <AlertCircle className="size-3" /> Failed
        </Badge>
      );
    case "ready":
      return null;
  }
}
