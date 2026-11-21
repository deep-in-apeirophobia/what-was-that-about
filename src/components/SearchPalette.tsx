import { useEffect, useMemo, useRef, useState } from "react";
import clsx from "clsx";
import { CornerDownLeft, FileAudio, Search, Sparkles, StickyNote, Tag, Type } from "lucide-react";
import { api } from "../lib/api";
import { errorMessage } from "../lib/errors";
import { formatDuration, formatTime } from "../lib/format";
import { snippetAround } from "../lib/text";
import type { SearchHit } from "../lib/types";
import { useApp } from "../store/app";
import { useLibrary } from "../store/library";
import { Dialog } from "./ui/Dialog";
import { Badge, Kbd, Spinner } from "./ui/misc";
import { HighlightedText } from "./HighlightedText";
import { StatusDot } from "./RecordingStatus";

const KIND: Record<SearchHit["kind"], { label: string; icon: React.ReactNode; tone: "neutral" | "accent" | "ok" | "warn" }> = {
  lexical: { label: "Match", icon: <Type className="size-3" />, tone: "neutral" },
  semantic: { label: "Similar", icon: <Sparkles className="size-3" />, tone: "accent" },
  tag: { label: "Topic", icon: <Tag className="size-3" />, tone: "ok" },
  note: { label: "Note", icon: <StickyNote className="size-3" />, tone: "warn" },
};

type Row =
  | { kind: "hit"; hit: SearchHit }
  | { kind: "recording"; id: string; title: string; meta: string };

export function SearchPalette() {
  const open = useApp((s) => s.searchOpen);
  const setOpen = useApp((s) => s.setSearchOpen);
  const openRecording = useApp((s) => s.openRecording);
  const recordings = useLibrary((s) => s.recordings);
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [sel, setSel] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const reqId = useRef(0);

  useEffect(() => {
    if (open) {
      void api.trackUi("search opened");
      setSel(0);
      requestAnimationFrame(() => inputRef.current?.select());
    }
  }, [open]);

  useEffect(() => {
    const q = query.trim();
    if (!q) {
      setHits([]);
      setLoading(false);
      setError(null);
      return;
    }
    setLoading(true);
    const id = ++reqId.current;
    const t = setTimeout(() => {
      api
        .search(q, null)
        .then((res) => {
          if (id !== reqId.current) return;
          setHits(res);
          setError(null);
          setSel(0);
        })
        .catch((e) => {
          if (id !== reqId.current) return;
          setHits([]);
          setError(errorMessage(e));
        })
        .finally(() => {
          if (id === reqId.current) setLoading(false);
        });
    }, 180);
    return () => clearTimeout(t);
  }, [query]);

  // Group by recording, keeping the best-scoring recording first.
  const groups = useMemo(() => {
    const map = new Map<string, { title: string; hits: SearchHit[] }>();
    for (const h of hits) {
      const g = map.get(h.recordingId) ?? { title: h.recordingTitle, hits: [] };
      g.hits.push(h);
      map.set(h.recordingId, g);
    }
    return [...map.entries()].map(([id, g]) => ({ id, ...g }));
  }, [hits]);

  const titleMatches = useMemo(() => {
    const q = query.trim().toLowerCase();
    const list = q ? recordings.filter((r) => r.title.toLowerCase().includes(q)) : recordings;
    return list.slice(0, q ? 4 : 6);
  }, [recordings, query]);

  const rows: Row[] = useMemo(() => {
    const out: Row[] = titleMatches.map((r) => ({
      kind: "recording",
      id: r.id,
      title: r.title,
      meta: formatDuration(r.durationSec),
    }));
    for (const g of groups) for (const h of g.hits) out.push({ kind: "hit", hit: h });
    return out;
  }, [groups, titleMatches]);

  const choose = (row: Row | undefined) => {
    if (!row) return;
    setOpen(false);
    if (row.kind === "recording") openRecording(row.id);
    else if (row.hit.kind === "note")
      openRecording(row.hit.recordingId, { time: row.hit.start, noteId: row.hit.noteId ?? undefined });
    else openRecording(row.hit.recordingId, { segmentId: row.hit.segmentId, time: row.hit.start });
  };

  useEffect(() => {
    listRef.current?.querySelector(`[data-row="${sel}"]`)?.scrollIntoView({ block: "nearest" });
  }, [sel]);

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setSel((s) => Math.min(rows.length - 1, s + 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setSel((s) => Math.max(0, s - 1));
    } else if (e.key === "Enter") {
      e.preventDefault();
      choose(rows[sel]);
    }
  };

  let rowIndex = -1;
  const recordingRows = rows.filter((r) => r.kind === "recording");
  const q = query.trim();

  return (
    <Dialog open={open} onClose={() => setOpen(false)} bare size="lg" top initialFocus={inputRef}>
      <div className="flex flex-col" onKeyDown={onKeyDown}>
        <div className="flex items-center gap-2.5 border-b border-line px-4">
          <Search className="size-4 shrink-0 text-faint" />
          <input
            ref={inputRef}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search transcripts, notes, topics and recordings…"
            aria-label="Search"
            dir="auto"
            role="combobox"
            aria-expanded={rows.length > 0}
            aria-controls="search-results"
            aria-activedescendant={rows.length ? `search-row-${sel}` : undefined}
            className="h-12 min-w-0 flex-1 bg-transparent text-[14.5px] outline-none placeholder:text-faint"
          />
          {loading && <Spinner />}
          <Kbd>Esc</Kbd>
        </div>

        <div ref={listRef} id="search-results" role="listbox" className="max-h-[56vh] overflow-y-auto p-1.5">
          {recordingRows.length > 0 && (
            <div className="px-2.5 pt-2 pb-1 text-[11px] font-medium uppercase tracking-wide text-faint">
              {q ? "Recordings" : "Jump to"}
            </div>
          )}
          {rows.map((row) => {
            if (row.kind !== "recording") return null;
            rowIndex++;
            const i = rowIndex;
            const rec = recordings.find((r) => r.id === row.id);
            return (
              <button
                key={`r-${row.id}`}
                id={`search-row-${i}`}
                data-row={i}
                type="button"
                role="option"
                aria-selected={sel === i}
                onMouseMove={() => setSel(i)}
                onClick={() => choose(row)}
                className={clsx(
                  "flex w-full items-center gap-2.5 rounded-lg px-2.5 py-2 text-left text-[13px]",
                  sel === i ? "bg-hover" : "",
                )}
              >
                <FileAudio className="size-4 shrink-0 text-faint" />
                <span className="min-w-0 flex-1 truncate" dir="auto">
                  {row.title}
                </span>
                {rec && <StatusDot recording={rec} />}
                <span className="shrink-0 text-[12px] text-faint tabular-nums">{row.meta}</span>
              </button>
            );
          })}

          {groups.map((g) => (
            <div key={g.id} className="mt-1">
              <div className="truncate px-2.5 pt-2.5 pb-1 text-[11px] font-medium uppercase tracking-wide text-faint" dir="auto">
                {g.title}
              </div>
              {g.hits.map((h) => {
                rowIndex++;
                const i = rowIndex;
                const snip = snippetAround(h.text, h.highlights, 180);
                const k = KIND[h.kind] ?? KIND.lexical;
                return (
                  <button
                    key={`${h.recordingId}-${h.segmentId}-${h.kind}-${h.noteId ?? ""}`}
                    id={`search-row-${i}`}
                    data-row={i}
                    type="button"
                    role="option"
                    aria-selected={sel === i}
                    onMouseMove={() => setSel(i)}
                    onClick={() => choose({ kind: "hit", hit: h })}
                    className={clsx(
                      "flex w-full items-start gap-3 rounded-lg px-2.5 py-2 text-left",
                      sel === i ? "bg-hover" : "",
                    )}
                  >
                    <span className="mt-0.5 w-11 shrink-0 text-[12px] font-medium text-accent tabular-nums">
                      {formatTime(h.start)}
                    </span>
                    <span
                      className={clsx("min-w-0 flex-1 font-fa text-[13.5px] leading-relaxed", h.kind === "note" && "whitespace-pre-line")}
                      dir="auto"
                    >
                      <HighlightedText text={snip.text} ranges={snip.ranges} />
                    </span>
                    <Badge tone={k.tone} className="mt-0.5 shrink-0">
                      {k.icon}
                      {k.label}
                    </Badge>
                    {sel === i && <CornerDownLeft className="mt-1 size-3.5 shrink-0 text-faint" />}
                  </button>
                );
              })}
            </div>
          ))}

          {q && !loading && !error && hits.length === 0 && (
            <div className="px-4 py-10 text-center text-[13px] text-muted">
              No matches for “<span dir="auto">{q}</span>”.
              <div className="mt-1 text-[12px] text-faint">Enable semantic search on a recording to also find related wording.</div>
            </div>
          )}
          {error && <div className="px-4 py-8 text-center text-[13px] text-danger">Search failed: {error}</div>}
          {!q && recordings.length === 0 && (
            <div className="px-4 py-10 text-center text-[13px] text-muted">Import a recording to start searching.</div>
          )}
        </div>

        <div className="flex items-center gap-4 border-t border-line bg-sunken/50 px-4 py-2 text-[11.5px] text-faint">
          <span className="flex items-center gap-1">
            <Kbd>↑</Kbd>
            <Kbd>↓</Kbd> navigate
          </span>
          <span className="flex items-center gap-1">
            <Kbd>↵</Kbd> open at this moment
          </span>
        </div>
      </div>
    </Dialog>
  );
}
