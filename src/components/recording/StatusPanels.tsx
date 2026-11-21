import { useCallback, useEffect, useState } from "react";
import clsx from "clsx";
import { AlertCircle, AudioLines, Check, FileWarning, Loader2, X } from "lucide-react";
import { api, isTauri } from "../../lib/api";
import { decodeForTranscription, TRANSCRIPTION_SAMPLE_RATE, type DecodeStage } from "../../lib/audio";
import { errorMessage } from "../../lib/errors";
import { mediaUrl } from "../../lib/platform";
import type { Recording } from "../../lib/types";
import { useLibrary } from "../../store/library";
import { useModels } from "../../store/models";
import { useRecording } from "../../store/recording";
import { Button } from "../ui/Button";
import { ProgressBar } from "../ui/misc";
import { STAGE_LABELS, overallProgress } from "../RecordingStatus";
import { cancelTranscription } from "./actions";
import { TranscribeButton } from "./TranscribeControls";

export function TranscribeCta({ recording }: { recording: Recording }) {
  const models = useModels((s) => s.models);
  const anyReady = models.some((m) => m.status === "ready");
  return (
    <div className="mx-auto mt-10 max-w-md rounded-2xl border border-line bg-surface p-6 text-center shadow-card">
      <div className="mx-auto flex size-12 items-center justify-center rounded-2xl bg-accent-soft text-accent">
        <AudioLines className="size-6" />
      </div>
      <h2 className="mt-4 text-[15px] font-semibold">Ready to transcribe</h2>
      <p className="mt-1 text-[13px] text-muted">
        {anyReady
          ? "Turn this recording into a searchable transcript. It runs on this computer — nothing is uploaded."
          : "Install a speech model first. It downloads once and then works offline."}
      </p>
      <div className="mt-5 flex justify-center">
        <TranscribeButton recording={recording} size="lg" variant="primary" />
      </div>
    </div>
  );
}

export function TranscriptionProgressCard({ recording }: { recording: Recording }) {
  const progress = useLibrary((s) => s.progress[recording.id]);
  const segmentCount = useRecording((s) => s.detail?.segments.length ?? 0);
  const [cancelling, setCancelling] = useState(false);
  const pct = overallProgress(progress);
  const stage = progress ? STAGE_LABELS[progress.stage] : recording.status === "queued" ? "Queued" : "Starting";
  return (
    <div className="sticky top-0 z-10 mx-4 mt-3 mb-2 rounded-xl border border-accent/25 bg-surface/95 p-3.5 shadow-card backdrop-blur" role="status">
      <div className="flex items-center gap-3">
        <Loader2 className="size-4 shrink-0 animate-spin text-accent" />
        <div className="min-w-0 flex-1">
          <div className="flex items-baseline gap-2 text-[13px]">
            <span className="font-medium">{stage}…</span>
            {progress?.message && <span className="truncate text-[12px] text-muted tabular-nums">{progress.message}</span>}
          </div>
          <ProgressBar value={pct} className="mt-2" label="Transcription progress" />
          <div className="mt-1.5 text-[11.5px] text-faint">
            {segmentCount > 0 ? `${segmentCount} sentences so far — you can read and play them already.` : "Sentences appear here as they're recognized."}
          </div>
        </div>
        <Button
          size="sm"
          icon={<X className="size-3.5" />}
          loading={cancelling}
          onClick={async () => {
            setCancelling(true);
            await cancelTranscription(recording.id);
            setCancelling(false);
          }}
        >
          Cancel
        </Button>
      </div>
    </div>
  );
}

export function TranscriptionErrorCard({ recording }: { recording: Recording }) {
  return (
    <div className="mx-auto mt-10 max-w-md rounded-2xl border border-danger/30 bg-surface p-6 text-center shadow-card" role="alert">
      <div className="mx-auto flex size-12 items-center justify-center rounded-2xl bg-danger-soft text-danger">
        <AlertCircle className="size-6" />
      </div>
      <h2 className="mt-4 text-[15px] font-semibold">Transcription failed</h2>
      <p className="mt-1 text-[13px] break-words text-muted">{recording.error ?? "Something went wrong."}</p>
      <div className="mt-5 flex justify-center">
        <TranscribeButton recording={recording} variant="primary" />
      </div>
    </div>
  );
}

// ---- in-browser decoding for formats the backend can't read ------------------------

type DecodeState = { stage: DecodeStage | "idle" | "done"; error: string | null };
const running = new Set<string>();

const DECODE_STEPS: { id: DecodeStage; label: string }[] = [
  { id: "fetching", label: "Reading the file" },
  { id: "decoding", label: "Decoding with the system's codecs" },
  { id: "resampling", label: "Converting to 16 kHz mono" },
  { id: "uploading", label: "Handing audio to the transcriber" },
];

function useBrowserDecode(recording: Recording) {
  const [state, setState] = useState<DecodeState>({ stage: "idle", error: null });

  const run = useCallback(async () => {
    if (running.has(recording.id)) return;
    running.add(recording.id);
    setState({ stage: "fetching", error: null });
    try {
      let samples: Float32Array;
      if (isTauri()) {
        const url = mediaUrl(recording.sourcePath);
        if (!url) throw new Error("The original file can't be read.");
        samples = await decodeForTranscription(url, (stage) => setState({ stage, error: null }));
      } else {
        // Browser mock: there is no real file, so walk through the steps with silence.
        for (const s of ["decoding", "resampling"] as const) {
          await new Promise((r) => setTimeout(r, 450));
          setState({ stage: s, error: null });
        }
        samples = new Float32Array(TRANSCRIPTION_SAMPLE_RATE * 2);
      }
      setState({ stage: "uploading", error: null });
      const updated = await api.submitDecodedAudio(recording.id, samples, TRANSCRIPTION_SAMPLE_RATE);
      useLibrary.getState().upsert(updated);
      useRecording.getState().applyRecording(updated);
      setState({ stage: "done", error: null });
    } catch (e) {
      setState((s) => ({ stage: s.stage, error: errorMessage(e) }));
    } finally {
      running.delete(recording.id);
    }
  }, [recording.id, recording.sourcePath]);

  useEffect(() => {
    void run();
  }, [run]);

  return { state, retry: run };
}

export function DecodePanel({ recording }: { recording: Recording }) {
  const { state, retry } = useBrowserDecode(recording);
  const currentIdx = DECODE_STEPS.findIndex((s) => s.id === state.stage);
  return (
    <div className="mx-auto mt-10 max-w-md rounded-2xl border border-line bg-surface p-6 shadow-card">
      <div className="flex items-center gap-3">
        <div className="flex size-10 items-center justify-center rounded-xl bg-warn-soft text-warn">
          <FileWarning className="size-5" />
        </div>
        <div>
          <h2 className="text-[15px] font-semibold">Decoding this file in the app</h2>
          <p className="text-[12.5px] text-muted">The transcriber can't read this format directly, so the app converts it first.</p>
        </div>
      </div>
      <ol className="mt-5 space-y-2.5">
        {DECODE_STEPS.map((s, i) => {
          const done = state.stage === "done" || i < currentIdx;
          const active = i === currentIdx && !state.error;
          const failed = i === currentIdx && !!state.error;
          return (
            <li key={s.id} className="flex items-center gap-2.5 text-[13px]">
              <span
                className={clsx(
                  "flex size-5 shrink-0 items-center justify-center rounded-full",
                  done ? "bg-ok text-white" : failed ? "bg-danger text-white" : active ? "bg-accent-soft text-accent" : "bg-sunken text-faint",
                )}
              >
                {done ? (
                  <Check className="size-3" />
                ) : failed ? (
                  <X className="size-3" />
                ) : active ? (
                  <Loader2 className="size-3 animate-spin" />
                ) : (
                  <span className="text-[10px] font-semibold">{i + 1}</span>
                )}
              </span>
              <span className={clsx(done || active ? "text-fg" : "text-muted", failed && "text-danger")}>{s.label}</span>
            </li>
          );
        })}
      </ol>
      {state.error && (
        <div className="mt-4 rounded-lg bg-danger-soft px-3 py-2.5 text-[12.5px] text-danger" role="alert">
          <div className="font-medium">Couldn't decode this file</div>
          <div className="mt-0.5 break-words">{state.error}</div>
          <div className="mt-2.5">
            <Button size="sm" onClick={() => void retry()}>
              Try again
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}
