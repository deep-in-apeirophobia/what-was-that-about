import { useEffect, useMemo, useState } from "react";
import clsx from "clsx";
import { FileAudio, Play, Search, Tags } from "lucide-react";
import { api } from "../lib/api";
import { errorMessage } from "../lib/errors";
import { formatTime, pluralize } from "../lib/format";
import { normalizeForMatch } from "../lib/text";
import type { SearchHit, TagSummary } from "../lib/types";
import { useApp } from "../store/app";
import { useLibrary } from "../store/library";
import { EmptyState, Segmented, Spinner, inputClass } from "../components/ui/misc";

type Layout = "cloud" | "list";

export function TopicsView({ initialTag }: { initialTag?: string }) {
  const openRecording = useApp((s) => s.openRecording);
  const recordings = useLibrary((s) => s.recordings);
  const [tags, setTags] = useState<TagSummary[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<string | null>(initialTag ?? null);
  const [hits, setHits] = useState<SearchHit[] | null>(null);
  const [hitsError, setHitsError] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  const [layout, setLayout] = useState<Layout>("cloud");

  // Tags change whenever an analysis finishes; refetch when the library changes.
  const analysisKey = recordings.map((r) => `${r.id}:${r.hasAnalysis ? r.tags.join(",") : ""}`).join("|");
  useEffect(() => {
    let alive = true;
    api
      .listTags()
      .then((t) => alive && (setTags(t), setError(null)))
      .catch((e) => alive && setError(errorMessage(e)));
    return () => {
      alive = false;
    };
  }, [analysisKey]);

  useEffect(() => {
    if (!selected) {
      setHits(null);
      return;
    }
    let alive = true;
    setHits(null);
    setHitsError(null);
    api
      .segmentsForTag(selected)
      .then((h) => alive && setHits(h))
      .catch((e) => {
        if (!alive) return;
        setHits([]);
        setHitsError(errorMessage(e));
      });
    return () => {
      alive = false;
    };
  }, [selected]);

  const visible = useMemo(() => {
    const q = normalizeForMatch(filter.trim());
    return (tags ?? []).filter((t) => !q || normalizeForMatch(t.name).includes(q));
  }, [tags, filter]);

  const max = Math.max(1, ...(tags ?? []).map((t) => t.segmentCount));

  const groups = useMemo(() => {
    const map = new Map<string, { title: string; hits: SearchHit[] }>();
    for (const h of hits ?? []) {
      const g = map.get(h.recordingId) ?? { title: h.recordingTitle, hits: [] };
      g.hits.push(h);
      map.set(h.recordingId, g);
    }
    for (const g of map.values()) g.hits.sort((a, b) => a.start - b.start);
    return [...map.entries()];
  }, [hits]);

  return (
    <div className="flex h-full flex-col">
      <header className="flex items-center gap-3 border-b border-line px-6 py-3.5">
        <div className="mr-auto">
          <h1 className="text-[17px] font-semibold">Topics</h1>
          <p className="text-[12.5px] text-muted">What your recordings talk about, across the whole library.</p>
        </div>
        {tags && tags.length > 0 && (
          <>
            <div className="relative w-56">
              <Search className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-faint" />
              <input
                value={filter}
                onChange={(e) => setFilter(e.target.value)}
                placeholder="Filter topics…"
                aria-label="Filter topics"
                dir="auto"
                className={clsx(inputClass, "pl-8")}
              />
            </div>
            <Segmented
              label="Layout"
              value={layout}
              onChange={setLayout}
              options={[
                { value: "cloud", label: "Cloud" },
                { value: "list", label: "List" },
              ]}
            />
          </>
        )}
      </header>

      <div className="flex min-h-0 flex-1">
        <div className="min-h-0 w-[min(460px,45%)] shrink-0 overflow-y-auto border-r border-line p-5">
          {error ? (
            <div className="rounded-lg bg-danger-soft px-3 py-2 text-[13px] text-danger">Couldn't load topics: {error}</div>
          ) : !tags ? (
            <div className="flex justify-center py-10">
              <Spinner />
            </div>
          ) : tags.length === 0 ? (
            <EmptyState icon={<Tags />} title="No topics yet">
              Topics appear after a recording is analyzed with AI. Open a transcribed recording and click “Analyze with AI”.
            </EmptyState>
          ) : layout === "cloud" ? (
            <div className="flex flex-wrap items-baseline gap-x-2 gap-y-2.5">
              {visible.map((t) => {
                const weight = Math.log(1 + t.segmentCount) / Math.log(1 + max);
                return (
                  <button
                    key={t.name}
                    type="button"
                    aria-pressed={selected === t.name}
                    onClick={() => setSelected(t.name)}
                    style={{ fontSize: `${12.5 + weight * 9}px` }}
                    className={clsx(
                      "rounded-lg px-2 py-0.5 font-medium transition-colors",
                      selected === t.name ? "bg-accent text-accent-fg" : "text-fg/80 hover:bg-hover hover:text-fg",
                    )}
                    title={`${pluralize(t.segmentCount, "moment")} in ${pluralize(t.recordingCount, "recording")}`}
                    dir="auto"
                  >
                    {t.name}
                  </button>
                );
              })}
            </div>
          ) : (
            <ul className="space-y-px">
              {visible.map((t) => (
                <li key={t.name}>
                  <button
                    type="button"
                    aria-pressed={selected === t.name}
                    onClick={() => setSelected(t.name)}
                    className={clsx(
                      "flex w-full items-center gap-3 rounded-lg px-3 py-2 text-left text-[13px]",
                      selected === t.name ? "bg-accent-soft text-accent" : "hover:bg-hover",
                    )}
                  >
                    <span className="min-w-0 flex-1 truncate font-medium" dir="auto">
                      {t.name}
                    </span>
                    <span className="text-[12px] text-faint tabular-nums">{pluralize(t.recordingCount, "recording")}</span>
                    <span className="w-20 text-right text-[12px] text-muted tabular-nums">{pluralize(t.segmentCount, "moment")}</span>
                  </button>
                </li>
              ))}
            </ul>
          )}
          {tags && tags.length > 0 && visible.length === 0 && (
            <div className="py-8 text-center text-[13px] text-muted">No topic matches “{filter}”.</div>
          )}
        </div>

        <div className="min-h-0 min-w-0 flex-1 overflow-y-auto p-5">
          {!selected ? (
            <EmptyState icon={<Tags />} title="Pick a topic" className="h-full">
              See every moment it comes up, in every recording, and jump straight there.
            </EmptyState>
          ) : hits == null ? (
            <div className="flex justify-center py-10">
              <Spinner />
            </div>
          ) : (
            <div className="mx-auto max-w-3xl">
              <h2 className="text-[16px] font-semibold" dir="auto">
                {selected}
              </h2>
              <p className="mt-0.5 text-[12.5px] text-muted">
                {pluralize(hits.length, "moment")} in {pluralize(groups.length, "recording")}
              </p>
              {hitsError && <div className="mt-3 rounded-lg bg-danger-soft px-3 py-2 text-[13px] text-danger">{hitsError}</div>}
              <div className="mt-4 space-y-4">
                {groups.map(([recId, g]) => (
                  <section key={recId} className="overflow-hidden rounded-xl border border-line bg-surface shadow-card">
                    <button
                      type="button"
                      onClick={() => openRecording(recId)}
                      className="flex w-full items-center gap-2 border-b border-line bg-sunken/50 px-4 py-2.5 text-left text-[13px] font-medium hover:bg-hover"
                    >
                      <FileAudio className="size-4 text-faint" />
                      <span className="min-w-0 flex-1 truncate" dir="auto">
                        {g.title}
                      </span>
                      <span className="text-[12px] font-normal text-faint">{pluralize(g.hits.length, "moment")}</span>
                    </button>
                    <ul className="divide-y divide-line">
                      {g.hits.map((h) => (
                        <li key={h.segmentId}>
                          <button
                            type="button"
                            onClick={() => openRecording(recId, { segmentId: h.segmentId, play: true })}
                            className="group flex w-full items-start gap-3 px-4 py-2.5 text-left hover:bg-hover/60"
                            aria-label={`Play from ${formatTime(h.start)}`}
                          >
                            <span className="mt-0.5 flex size-6 shrink-0 items-center justify-center rounded-full bg-accent-soft text-accent group-hover:bg-accent group-hover:text-accent-fg">
                              <Play className="size-3 translate-x-px" fill="currentColor" />
                            </span>
                            <span className="mt-1 w-11 shrink-0 text-[12px] text-accent tabular-nums">{formatTime(h.start)}</span>
                            <span className="min-w-0 flex-1 font-fa text-[13.5px] leading-relaxed" dir="auto">
                              {h.text}
                            </span>
                          </button>
                        </li>
                      ))}
                    </ul>
                  </section>
                ))}
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
