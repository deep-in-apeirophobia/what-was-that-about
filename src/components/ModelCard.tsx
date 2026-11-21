import { useState } from "react";
import clsx from "clsx";
import {
  AlertCircle,
  CheckCircle2,
  ChevronDown,
  Cpu,
  Download,
  ExternalLink,
  FolderInput,
  Gpu,
  Trash2,
  X,
} from "lucide-react";
import type { ModelInfo, ModelProgress } from "../lib/types";
import { formatByteProgress, formatBytes } from "../lib/format";
import { openExternal, pickModelFiles } from "../lib/platform";
import { useModels } from "../store/models";
import { attempt, toast } from "../store/toasts";
import { Button } from "./ui/Button";
import { ConfirmDialog } from "./ui/Dialog";
import { Badge, ProgressBar } from "./ui/misc";
import { Markdown } from "./ui/Markdown";

export function progressText(p: ModelProgress): string {
  const bytes = p.totalBytes > 0 ? ` ${formatByteProgress(p.doneBytes, p.totalBytes)}` : "";
  switch (p.stage) {
    case "downloading":
      return `Downloading ${p.file ?? "files"}…${bytes}`;
    case "converting":
      return p.message ?? `Converting${p.file ? ` ${p.file}` : ""}…`;
    case "verifying":
      return p.message ?? "Verifying files…";
    default:
      return p.message ?? "";
  }
}

export function ModelProgressView({ progress }: { progress: ModelProgress | undefined }) {
  const value =
    progress && progress.totalBytes > 0 && progress.stage !== "verifying" ? progress.doneBytes / progress.totalBytes : null;
  return (
    <div className="space-y-1.5">
      <ProgressBar value={value} label="Install progress" />
      <div className="flex items-center justify-between gap-3 text-[12px] text-muted">
        <span className="truncate">{progress ? progressText(progress) : "Starting…"}</span>
        {value != null && <span className="shrink-0 tabular-nums">{Math.round(value * 100)}%</span>}
      </div>
    </div>
  );
}

export function ManualInstall({ model }: { model: ModelInfo }) {
  const importFiles = useModels((s) => s.importFiles);
  const busy = model.status === "downloading" || model.status === "converting";
  const onImport = async () => {
    const paths = await attempt("Couldn't open the file picker", () => pickModelFiles());
    if (paths?.length) await importFiles(model.id, paths);
  };
  return (
    <div className="space-y-3 rounded-lg border border-line bg-sunken/50 p-3.5">
      {model.manualInstructions && <Markdown content={model.manualInstructions} className="text-[13px] text-muted" />}
      <ul className="divide-y divide-line overflow-hidden rounded-lg border border-line bg-surface">
        {model.manualFiles.map((f) => (
          <li key={f.name} className="flex items-center gap-3 px-3 py-2 text-[12.5px]">
            <span className="min-w-0 flex-1 truncate font-mono text-[12px]">{f.name}</span>
            {!f.required && <Badge>optional</Badge>}
            <span className="shrink-0 text-faint tabular-nums">{formatBytes(f.sizeBytes)}</span>
            <button
              type="button"
              onClick={() => openExternal(f.url).catch((e) => toast.error("Couldn't open the link", e))}
              className="inline-flex shrink-0 items-center gap-1 rounded-md px-1.5 py-0.5 font-medium text-accent hover:bg-accent-soft"
            >
              Download <ExternalLink className="size-3" />
            </button>
          </li>
        ))}
      </ul>
      <div className="flex items-center justify-between gap-3">
        <span className="text-[12px] text-faint">Select all the files at once — the app checks and copies them.</span>
        <Button size="sm" icon={<FolderInput className="size-3.5" />} onClick={() => void onImport()} disabled={busy}>
          Import files…
        </Button>
      </div>
    </div>
  );
}

export function ModelCard({ model, recommended }: { model: ModelInfo; recommended?: boolean }) {
  const progress = useModels((s) => s.progress[model.id]);
  const install = useModels((s) => s.install);
  const cancel = useModels((s) => s.cancel);
  const remove = useModels((s) => s.remove);
  const [manualOpen, setManualOpen] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const installing = model.status === "downloading" || model.status === "converting" || !!progress;
  const ready = model.status === "ready" && !installing;

  return (
    <article
      className={clsx(
        "rounded-xl border bg-surface p-5 shadow-card transition-colors",
        ready ? "border-ok/40" : "border-line",
      )}
    >
      <div className="flex items-start gap-4">
        <div
          className={clsx(
            "flex size-10 shrink-0 items-center justify-center rounded-xl",
            model.engine === "whisper" ? "bg-accent-soft text-accent" : "bg-ok-soft text-ok",
          )}
        >
          {model.engine === "whisper" ? <Gpu className="size-5" /> : <Cpu className="size-5" />}
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <h3 className="text-[15px] font-semibold">{model.name}</h3>
            <Badge tone={model.engine === "whisper" ? "accent" : "ok"}>{model.recommendedFor}</Badge>
            {recommended && <Badge tone="warn">Recommended for this computer</Badge>}
          </div>
          <p className="mt-1 text-[13px] leading-relaxed text-muted">{model.description}</p>
          <dl className="mt-2.5 flex flex-wrap gap-x-5 gap-y-1 text-[12px] text-faint">
            <div>
              <dt className="inline">Engine: </dt>
              <dd className="inline text-muted">{model.engine === "whisper" ? "whisper.cpp" : "Shenava (ONNX)"}</dd>
            </div>
            <div>
              <dt className="inline">License: </dt>
              <dd className="inline text-muted">{model.license}</dd>
            </div>
            <div>
              <dt className="inline">Download: </dt>
              <dd className="inline text-muted">{formatBytes(model.downloadBytes)}</dd>
            </div>
            <div>
              <dt className="inline">On disk: </dt>
              <dd className="inline text-muted">{formatBytes(model.diskBytes)}</dd>
            </div>
            {model.homepage && (
              <button
                type="button"
                onClick={() => openExternal(model.homepage).catch((e) => toast.error("Couldn't open the link", e))}
                className="inline-flex items-center gap-1 text-accent hover:underline"
              >
                Model page <ExternalLink className="size-3" />
              </button>
            )}
          </dl>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          {ready && (
            <>
              <span className="flex items-center gap-1 text-[12.5px] font-medium text-ok">
                <CheckCircle2 className="size-4" /> Installed
              </span>
              <Button size="sm" variant="danger-ghost" icon={<Trash2 className="size-3.5" />} onClick={() => setConfirmDelete(true)}>
                Delete
              </Button>
            </>
          )}
          {installing && (
            <Button size="sm" icon={<X className="size-3.5" />} onClick={() => void cancel(model.id)}>
              Cancel
            </Button>
          )}
          {!ready && !installing && (
            <Button variant="primary" icon={<Download className="size-3.5" />} onClick={() => void install(model.id)}>
              {model.status === "error" ? "Retry install" : "Install"}
            </Button>
          )}
        </div>
      </div>

      {installing && (
        <div className="mt-4">
          <ModelProgressView progress={progress} />
        </div>
      )}

      {model.status === "error" && model.error && !installing && (
        <div className="mt-4 flex items-start gap-2 rounded-lg bg-danger-soft px-3 py-2 text-[12.5px] text-danger">
          <AlertCircle className="mt-0.5 size-3.5 shrink-0" />
          <span className="break-words">{model.error}</span>
        </div>
      )}

      {ready && model.installDir && (
        <div className="mt-3 truncate font-mono text-[11.5px] text-faint" title={model.installDir}>
          {model.installDir}
        </div>
      )}

      {!ready && (
        <div className="mt-4 border-t border-line pt-3">
          <button
            type="button"
            onClick={() => setManualOpen((o) => !o)}
            aria-expanded={manualOpen}
            className="flex items-center gap-1.5 text-[12.5px] font-medium text-muted hover:text-fg"
          >
            <ChevronDown className={clsx("size-3.5 transition-transform", manualOpen && "rotate-180")} />
            Manual install — I downloaded the files myself
          </button>
          {manualOpen && (
            <div className="mt-3">
              <ManualInstall model={model} />
            </div>
          )}
        </div>
      )}

      <ConfirmDialog
        open={confirmDelete}
        title={`Delete ${model.name}?`}
        message={`This frees ${formatBytes(model.diskBytes)}. Existing transcripts are kept; you can reinstall the model any time.`}
        confirmLabel="Delete model"
        danger
        onCancel={() => setConfirmDelete(false)}
        onConfirm={() => {
          setConfirmDelete(false);
          void remove(model.id);
        }}
      />
    </article>
  );
}
