import { useMemo, useState } from "react";
import clsx from "clsx";
import { ArrowUpDown, Boxes, Clock, FileAudio, LayoutGrid, List, Plus, Search, Upload } from "lucide-react";
import type { Recording } from "../lib/types";
import { formatDate, formatDuration, pluralize } from "../lib/format";
import { normalizeForMatch } from "../lib/text";
import { useApp } from "../store/app";
import { useLibrary } from "../store/library";
import { hasReadyModel, useModels } from "../store/models";
import { Button } from "../components/ui/Button";
import { Menu } from "../components/ui/Menu";
import { ProgressBar, Segmented, Spinner, inputClass } from "../components/ui/misc";
import { StatusBadge, isRunning, overallProgress } from "../components/RecordingStatus";
import { importViaDialog } from "../components/Sidebar";

type Sort = "newest" | "oldest" | "title" | "longest";
const SORTS: { value: Sort; label: string }[] = [
  { value: "newest", label: "Newest first" },
  { value: "oldest", label: "Oldest first" },
  { value: "title", label: "Title" },
  { value: "longest", label: "Longest first" },
];

function sortRecordings(list: Recording[], sort: Sort): Recording[] {
  const out = list.slice();
  switch (sort) {
    case "newest":
      return out.sort((a, b) => b.createdAt.localeCompare(a.createdAt));
    case "oldest":
      return out.sort((a, b) => a.createdAt.localeCompare(b.createdAt));
    case "title":
      return out.sort((a, b) => a.title.localeCompare(b.title, undefined, { sensitivity: "base" }));
    case "longest":
      return out.sort((a, b) => (b.durationSec ?? 0) - (a.durationSec ?? 0));
  }
}

function TagChips({ tags, max = 3 }: { tags: string[]; max?: number }) {
  if (!tags.length) return null;
  return (
    <div className="flex flex-wrap gap-1">
      {tags.slice(0, max).map((t) => (
        <span key={t} className="rounded-md bg-sunken px-1.5 py-0.5 text-[11.5px] text-muted" dir="auto">
          {t}
        </span>
      ))}
      {tags.length > max && <span className="px-1 py-0.5 text-[11.5px] text-faint">+{tags.length - max}</span>}
    </div>
  );
}

function RecordingCard({ r, layout }: { r: Recording; layout: "grid" | "list" }) {
  const openRecording = useApp((s) => s.openRecording);
  const progress = useLibrary((s) => s.progress[r.id]);
  const pct = overallProgress(progress);
  const running = isRunning(r);

  if (layout === "list") {
    return (
      <button
        type="button"
        onClick={() => openRecording(r.id)}
        className="group flex w-full items-center gap-4 border-b border-line px-4 py-3 text-left transition-colors last:border-b-0 hover:bg-hover/60"
      >
        <FileAudio className="size-4 shrink-0 text-faint" />
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <span className="truncate text-[13.5px] font-medium" dir="auto">
              {r.title}
            </span>
            <StatusBadge recording={r} progress={progress} />
          </div>
          {r.blurb && (
            <div className="mt-0.5 truncate text-[12.5px] text-muted" dir="auto">
              {r.blurb}
            </div>
          )}
          {running && <ProgressBar value={pct} className="mt-1.5 max-w-xs" label="Transcription progress" />}
        </div>
        <div className="hidden w-48 shrink-0 justify-end lg:flex">
          <TagChips tags={r.tags} max={2} />
        </div>
        <span className="w-16 shrink-0 text-right text-[12.5px] text-muted tabular-nums">{formatDuration(r.durationSec)}</span>
        <span className="w-28 shrink-0 text-right text-[12.5px] text-faint">{formatDate(r.createdAt)}</span>
      </button>
    );
  }

  return (
    <button
      type="button"
      onClick={() => openRecording(r.id)}
      className="group flex h-full flex-col rounded-xl border border-line bg-surface p-4 text-left shadow-card transition-[border-color,transform] hover:-translate-y-px hover:border-line-strong"
    >
      <div className="line-clamp-2 text-[14px] leading-snug font-semibold" dir="auto">
        {r.title}
      </div>
      <div className="mt-1.5 flex items-center gap-2 text-[12px] whitespace-nowrap text-faint">
        <span className="flex items-center gap-1 tabular-nums">
          <Clock className="size-3" />
          {formatDuration(r.durationSec)}
        </span>
        <span>·</span>
        <span className="truncate">{formatDate(r.createdAt)}</span>
        <span className="ml-auto">
          <StatusBadge recording={r} progress={progress} />
        </span>
      </div>
      {running && <ProgressBar value={pct} className="mt-3" label="Transcription progress" />}
      <div className="mt-3 line-clamp-3 min-h-[3lh] text-[12.5px] leading-relaxed text-muted" dir="auto">
        {r.blurb ??
          (r.status === "ready"
            ? `${pluralize(r.segmentCount, "sentence")} transcribed.`
            : r.status === "new"
              ? "Not transcribed yet — open it to start."
              : r.status === "needsDecode"
                ? "This format needs decoding in the app before transcription."
                : r.status === "error"
                  ? (r.error ?? "Something went wrong.")
                  : "Transcribing…")}
      </div>
      <div className="mt-auto pt-3">
        <TagChips tags={r.tags} />
      </div>
    </button>
  );
}

export function LibraryView() {
  const recordings = useLibrary((s) => s.recordings);
  const loaded = useLibrary((s) => s.loaded);
  const models = useModels((s) => s.models);
  const modelsLoaded = useModels((s) => s.loaded);
  const navigate = useApp((s) => s.navigate);
  const [filter, setFilter] = useState("");
  const [sort, setSort] = useState<Sort>("newest");
  const [layout, setLayout] = useState<"grid" | "list">(() =>
    localStorage.getItem("wwta.libraryLayout") === "list" ? "list" : "grid",
  );

  const visible = useMemo(() => {
    const q = normalizeForMatch(filter.trim());
    const list = q
      ? recordings.filter((r) =>
          normalizeForMatch([r.title, r.blurb ?? "", ...r.tags, r.sourcePath].join(" ")).includes(q),
        )
      : recordings;
    return sortRecordings(list, sort);
  }, [recordings, filter, sort]);

  const setLayoutPersist = (l: "grid" | "list") => {
    setLayout(l);
    try {
      localStorage.setItem("wwta.libraryLayout", l);
    } catch {
      /* ignore */
    }
  };

  const needsModel = modelsLoaded && !hasReadyModel(models);

  return (
    <div className="flex h-full flex-col">
      <header className="flex flex-wrap items-center gap-3 border-b border-line px-6 py-3.5">
        <div className="mr-auto">
          <h1 className="text-[17px] font-semibold">Library</h1>
          <p className="text-[12.5px] text-muted">
            {loaded ? `${pluralize(recordings.length, "recording")}` : "Loading…"}
          </p>
        </div>
        {recordings.length > 0 && (
          <>
            <div className="relative w-64">
              <Search className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-faint" />
              <input
                value={filter}
                onChange={(e) => setFilter(e.target.value)}
                placeholder="Filter by title, tag…"
                aria-label="Filter recordings"
                dir="auto"
                className={clsx(inputClass, "pl-8")}
              />
            </div>
            <Menu
              align="end"
              items={SORTS.map((s) => ({ key: s.value, label: s.label, checked: s.value === sort, onSelect: () => setSort(s.value) }))}
              trigger={(p) => (
                <Button {...p} icon={<ArrowUpDown className="size-3.5" />}>
                  {SORTS.find((s) => s.value === sort)?.label}
                </Button>
              )}
            />
            <Segmented
              label="Layout"
              value={layout}
              onChange={setLayoutPersist}
              options={[
                { value: "grid", label: <LayoutGrid className="size-3.5" aria-label="Grid" /> },
                { value: "list", label: <List className="size-3.5" aria-label="List" /> },
              ]}
            />
          </>
        )}
        <Button variant="primary" icon={<Plus className="size-3.5" />} onClick={() => void importViaDialog()}>
          Import
        </Button>
      </header>

      <div className="min-h-0 flex-1 overflow-y-auto">
        {needsModel && (
          <div className="mx-6 mt-5 flex items-center gap-3 rounded-xl border border-warn/30 bg-warn-soft px-4 py-3">
            <Boxes className="size-4 shrink-0 text-warn" />
            <div className="min-w-0 flex-1 text-[13px]">
              <span className="font-medium">No speech model installed yet.</span>{" "}
              <span className="text-muted">You can import recordings now; install a model to transcribe them.</span>
            </div>
            <Button size="sm" onClick={() => navigate({ name: "models" })}>
              Set up models
            </Button>
          </div>
        )}

        {!loaded ? (
          <div className="flex justify-center py-20">
            <Spinner />
          </div>
        ) : recordings.length === 0 ? (
          <div className="p-6">
            <button
              type="button"
              onClick={() => void importViaDialog()}
              className="flex h-[min(420px,60vh)] w-full flex-col items-center justify-center rounded-2xl border-2 border-dashed border-line-strong bg-surface/50 text-center transition-colors hover:border-accent hover:bg-accent-soft/30"
            >
              <div className="flex size-14 items-center justify-center rounded-2xl bg-accent-soft text-accent">
                <Upload className="size-6" />
              </div>
              <div className="mt-4 text-[16px] font-semibold">Drop audio or video files here</div>
              <div className="mt-1 max-w-sm text-[13px] text-muted">
                or click to choose files. Meetings, lectures, podcasts, voice notes — MP3, M4A, WAV, OPUS, MP4, MKV and more.
              </div>
              <span className="mt-5 inline-flex h-9 items-center gap-2 rounded-lg bg-accent px-4 text-[13px] font-medium text-accent-fg shadow-sm">
                <Plus className="size-4" /> Import audio
              </span>
            </button>
          </div>
        ) : visible.length === 0 ? (
          <div className="py-16 text-center text-[13px] text-muted">
            Nothing matches “<span dir="auto">{filter}</span>”.
          </div>
        ) : layout === "grid" ? (
          <div className="grid grid-cols-[repeat(auto-fill,minmax(270px,1fr))] gap-4 p-6">
            {visible.map((r) => (
              <RecordingCard key={r.id} r={r} layout="grid" />
            ))}
          </div>
        ) : (
          <div className="m-6 overflow-hidden rounded-xl border border-line bg-surface shadow-card">
            {visible.map((r) => (
              <RecordingCard key={r.id} r={r} layout="list" />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
