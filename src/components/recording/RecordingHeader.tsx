import { useEffect, useRef, useState } from "react";
import { ChevronRight, Download, Ellipsis, Sparkles, Trash2, Undo2, X } from "lucide-react";
import { api } from "../../lib/api";
import { formatDate, formatDuration, pluralize } from "../../lib/format";
import { fileName } from "../../lib/platform";
import type { ExportFormat, RecordingDetail } from "../../lib/types";
import { useApp } from "../../store/app";
import { useLibrary } from "../../store/library";
import { useModels } from "../../store/models";
import { useRecording } from "../../store/recording";
import { hasApiKey, useSettings } from "../../store/settings";
import { attempt, toast } from "../../store/toasts";
import { Button, IconButton } from "../ui/Button";
import { ConfirmDialog } from "../ui/Dialog";
import { Menu } from "../ui/Menu";
import { StatusBadge, isRunning } from "../RecordingStatus";
import { analyzeRecording, cancelTranscription, enableSemanticSearch, exportTranscript, revertTranscriptFixes } from "./actions";
import { TranscribeButton } from "./TranscribeControls";

const EXPORTS: { format: ExportFormat; label: string; hint: string }[] = [
  { format: "txt", label: "Plain text", hint: ".txt" },
  { format: "md", label: "Markdown with chapters", hint: ".md" },
  { format: "srt", label: "Subtitles (SRT)", hint: ".srt" },
  { format: "vtt", label: "Subtitles (WebVTT)", hint: ".vtt" },
  { format: "json", label: "JSON with word timings", hint: ".json" },
];

function EditableTitle({ id, title }: { id: string; title: string }) {
  const [editing, setEditing] = useState(false);
  const [value, setValue] = useState(title);
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (!editing) setValue(title);
  }, [title, editing]);
  useEffect(() => {
    if (editing) ref.current?.select();
  }, [editing]);

  const commit = async () => {
    setEditing(false);
    const t = value.trim();
    if (!t || t === title) {
      setValue(title);
      return;
    }
    const updated = await attempt("Couldn't rename", () => api.renameRecording(id, t));
    if (updated) {
      useLibrary.getState().upsert(updated);
      useRecording.getState().applyRecording(updated);
    } else setValue(title);
  };

  if (editing) {
    return (
      <input
        ref={ref}
        value={value}
        dir="auto"
        aria-label="Recording title"
        onChange={(e) => setValue(e.target.value)}
        onBlur={() => void commit()}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            void commit();
          } else if (e.key === "Escape") {
            e.preventDefault();
            e.stopPropagation();
            setValue(title);
            setEditing(false);
          }
        }}
        className="-mx-1.5 w-full min-w-0 rounded-md border border-accent bg-surface px-1.5 text-[18px] font-semibold outline-none ring-2 ring-accent/20"
      />
    );
  }
  return (
    <button
      type="button"
      onClick={() => setEditing(true)}
      title="Click to rename"
      className="-mx-1.5 max-w-full min-w-0 self-start truncate rounded-md px-1.5 text-start text-[18px] font-semibold hover:bg-hover"
      dir="auto"
    >
      {title}
    </button>
  );
}

export function RecordingHeader({ detail }: { detail: RecordingDetail }) {
  const { recording, analysis } = detail;
  const navigate = useApp((s) => s.navigate);
  const progress = useLibrary((s) => s.progress[recording.id]);
  const fixing = useLibrary((s) => !!s.fixing[recording.id]);
  const models = useModels((s) => s.models);
  const settings = useSettings((s) => s.settings);
  const [analyzing, setAnalyzing] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [confirmRestore, setConfirmRestore] = useState(false);
  const [restoring, setRestoring] = useState(false);
  const fixedCount = detail.segments.filter((s) => s.originalText != null).length;
  const running = isRunning(recording);
  const ready = recording.status === "ready" && detail.segments.length > 0;
  const keyOk = hasApiKey(settings);
  const modelName = recording.modelId ? (models.find((m) => m.id === recording.modelId)?.name ?? recording.modelId) : null;

  const onDelete = async () => {
    setDeleting(true);
    const ok = await attempt("Couldn't delete the recording", async () => {
      await api.deleteRecording(recording.id);
      return true;
    });
    setDeleting(false);
    setConfirmDelete(false);
    if (ok) {
      useLibrary.getState().remove(recording.id);
      navigate({ name: "library" });
      toast.success("Recording deleted", recording.title);
    }
  };

  return (
    <header className="shrink-0 border-b border-line px-5 pt-3 pb-3">
      <nav aria-label="Breadcrumb" className="mb-1 flex items-center gap-1 text-[12px] text-faint">
        <button type="button" onClick={() => navigate({ name: "library" })} className="hover:text-fg">
          Library
        </button>
        <ChevronRight className="size-3" />
        <span className="truncate" title={recording.sourcePath}>
          {fileName(recording.sourcePath)}
        </span>
      </nav>
      <div className="flex items-center gap-3">
        <div className="flex min-w-0 flex-1 flex-col">
          <EditableTitle id={recording.id} title={recording.title} />
          <div className="mt-0.5 flex flex-wrap items-center gap-x-2 gap-y-1 text-[12.5px] text-muted">
            <span className="tabular-nums">{formatDuration(recording.durationSec)}</span>
            <span className="text-faint">·</span>
            <span>{formatDate(recording.createdAt)}</span>
            {modelName && (
              <>
                <span className="text-faint">·</span>
                <span title="Speech model used">{modelName}</span>
              </>
            )}
            {analysis && (
              <>
                <span className="text-faint">·</span>
                <span className="flex items-center gap-1" title="AI analysis">
                  <Sparkles className="size-3 text-accent" />
                  {analysis.model.split("/").pop()}
                </span>
              </>
            )}
            <StatusBadge recording={recording} progress={progress} />
          </div>
        </div>

        <div className="flex shrink-0 items-center gap-2">
          {running ? (
            <Button icon={<X className="size-3.5" />} onClick={() => void cancelTranscription(recording.id)}>
              Cancel transcription
            </Button>
          ) : recording.status !== "needsDecode" && recording.status !== "new" ? (
            <TranscribeButton recording={recording} />
          ) : null}
          {ready && (
            <Button
              variant={analysis ? "secondary" : "primary"}
              icon={<Sparkles className="size-3.5" />}
              loading={analyzing}
              disabled={!keyOk}
              title={keyOk ? "Summary, chapters and topics" : "Add an OpenRouter key in Settings first"}
              onClick={async () => {
                setAnalyzing(true);
                await analyzeRecording(recording.id);
                setAnalyzing(false);
              }}
            >
              {analysis ? "Re-analyze" : "Analyze with AI"}
            </Button>
          )}
          <Menu
            align="end"
            items={[
              { heading: "Export transcript" },
              ...EXPORTS.map((e) => ({
                key: e.format,
                label: e.label,
                hint: e.hint,
                disabled: !ready,
                onSelect: () => void exportTranscript(recording, e.format),
              })),
            ]}
            trigger={(p) => (
              <Button {...p} icon={<Download className="size-3.5" />} disabled={!ready}>
                Export
              </Button>
            )}
          />
          <Menu
            align="end"
            items={[
              {
                key: "semantic",
                label: detail.embeddingsIndexed ? "Semantic search enabled" : "Enable semantic search",
                icon: <Sparkles />,
                disabled: detail.embeddingsIndexed || !ready || !keyOk,
                onSelect: () => void enableSemanticSearch(recording.id),
              },
              {
                key: "restore",
                label: "Restore original transcript…",
                icon: <Undo2 />,
                disabled: !fixedCount || fixing,
                onSelect: () => setConfirmRestore(true),
              },
              "separator",
              {
                key: "delete",
                label: "Delete recording…",
                icon: <Trash2 />,
                danger: true,
                onSelect: () => setConfirmDelete(true),
              },
            ]}
            trigger={(p) => (
              <IconButton {...p} label="More actions" size="md" variant="secondary">
                <Ellipsis className="size-4" />
              </IconButton>
            )}
          />
        </div>
      </div>
      <ConfirmDialog
        open={confirmDelete}
        title="Delete this recording?"
        message={
          <>
            “<span dir="auto">{recording.title}</span>” and its transcript, analysis, chat and docs will be removed from the library.
            {settingsCopyNote(settings?.copyIntoLibrary)}
          </>
        }
        confirmLabel="Delete"
        danger
        busy={deleting}
        onCancel={() => setConfirmDelete(false)}
        onConfirm={() => void onDelete()}
      />
      <ConfirmDialog
        open={confirmRestore}
        title="Restore the original transcript?"
        message={`The AI fixes in ${pluralize(fixedCount, "sentence")} are undone and the text speech recognition produced comes back, including where you edited a fixed sentence afterwards.`}
        confirmLabel="Restore"
        busy={restoring}
        onCancel={() => setConfirmRestore(false)}
        onConfirm={async () => {
          setRestoring(true);
          await revertTranscriptFixes(recording.id, null);
          setRestoring(false);
          setConfirmRestore(false);
        }}
      />
    </header>
  );
}

function settingsCopyNote(copied: boolean | undefined): string {
  return copied ? " The copy inside the library is deleted; your original file is untouched." : " Your original file is untouched.";
}
