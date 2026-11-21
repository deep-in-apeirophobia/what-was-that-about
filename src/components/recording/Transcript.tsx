import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ChevronDown, ChevronUp, Crosshair, Search, StickyNote, Tag as TagIcon, X } from "lucide-react";
import { api } from "../../lib/api";
import { formatTime, pluralize } from "../../lib/format";
import { layoutNotes, spanOfSegments } from "../../lib/notes";
import { findActiveIndex } from "../../lib/segments";
import { segmentIdsInRange } from "../../lib/selection";
import { detectDirection, findMatches } from "../../lib/text";
import type { Chapter, Note, Segment } from "../../lib/types";
import { MOD } from "../../lib/platform";
import { usePlayer } from "../../store/player";
import { FONT_SIZES, useRecording } from "../../store/recording";
import { attempt, toast } from "../../store/toasts";
import { IconButton } from "../ui/Button";
import { NoteComposer } from "../notes/NoteComposer";
import { NoteCard } from "../notes/NoteCard";
import { NO_RANGES, SegmentRow, type SegmentHandlers } from "./SegmentRow";
import { copyMomentLink, goToNote, seekToSegment } from "./actions";
import { DecodePanel, TranscribeCta, TranscriptionErrorCard, TranscriptionProgressCard } from "./StatusPanels";

function FindBar({
  query,
  setQuery,
  count,
  index,
  step,
  onClose,
}: {
  query: string;
  setQuery: (q: string) => void;
  count: number;
  index: number;
  step: (d: 1 | -1) => void;
  onClose: () => void;
}) {
  const ref = useRef<HTMLInputElement>(null);
  const focusNonce = useRecording((s) => s.findOpen);
  useEffect(() => {
    ref.current?.focus();
    ref.current?.select();
  }, [focusNonce]);
  return (
    <div className="flex items-center gap-2 border-b border-line bg-surface px-4 py-2" role="search">
      <Search className="size-3.5 shrink-0 text-faint" />
      <input
        ref={ref}
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            step(e.shiftKey ? -1 : 1);
          } else if (e.key === "Escape") {
            e.preventDefault();
            e.stopPropagation();
            onClose();
          }
        }}
        placeholder="Find in transcript…"
        aria-label="Find in transcript"
        dir="auto"
        className="h-7 min-w-0 flex-1 bg-transparent font-fa text-[13.5px] outline-none placeholder:font-sans placeholder:text-faint"
      />
      <span className="shrink-0 text-[12px] text-muted tabular-nums" aria-live="polite">
        {query.trim() ? (count ? `${index + 1} / ${count}` : "No matches") : ""}
      </span>
      <IconButton label="Previous match (Shift+Enter)" size="xs" onClick={() => step(-1)} disabled={!count}>
        <ChevronUp className="size-4" />
      </IconButton>
      <IconButton label="Next match (Enter)" size="xs" onClick={() => step(1)} disabled={!count}>
        <ChevronDown className="size-4" />
      </IconButton>
      <IconButton label="Close find (Esc)" size="xs" onClick={onClose}>
        <X className="size-4" />
      </IconButton>
    </div>
  );
}

function TagBar({ name, ids, onClose }: { name: string; ids: number[]; onClose: () => void }) {
  const [pos, setPos] = useState(-1);
  useEffect(() => setPos(-1), [name]);
  const step = (d: 1 | -1) => {
    if (!ids.length) return;
    const next = pos < 0 ? (d === 1 ? 0 : ids.length - 1) : (pos + d + ids.length) % ids.length;
    setPos(next);
    seekToSegment(ids[next], true);
  };
  return (
    <div className="flex items-center gap-2 border-b border-line bg-ok-soft/60 px-4 py-2 text-[12.5px]">
      <TagIcon className="size-3.5 shrink-0 text-ok" />
      <span className="text-muted">Topic</span>
      <span className="truncate font-medium" dir="auto">
        {name}
      </span>
      <span className="text-muted tabular-nums">
        · {pos >= 0 ? `${pos + 1} of ${ids.length}` : pluralize(ids.length, "moment")}
      </span>
      <div className="ml-auto flex items-center gap-1">
        <IconButton label="Previous moment" size="xs" onClick={() => step(-1)} disabled={!ids.length}>
          <ChevronUp className="size-4" />
        </IconButton>
        <IconButton label="Next moment" size="xs" onClick={() => step(1)} disabled={!ids.length}>
          <ChevronDown className="size-4" />
        </IconButton>
        <IconButton label="Clear topic" size="xs" onClick={onClose}>
          <X className="size-4" />
        </IconButton>
      </div>
    </div>
  );
}

function ChapterHeading({ chapter }: { chapter: Chapter }) {
  return (
    <div className="px-3 pt-7 pb-1.5 first:pt-2" dir={detectDirection(chapter.title)}>
      <button
        type="button"
        onClick={() => usePlayer.getState().seek(chapter.start, true)}
        className="group flex w-full items-baseline gap-2 text-start"
        title="Play this chapter"
      >
        <span className="text-[15px] font-semibold group-hover:text-accent">{chapter.title}</span>
        <span className="shrink-0 text-[11.5px] text-faint tabular-nums" dir="ltr">
          {formatTime(chapter.start)}
        </span>
      </button>
      {chapter.summary && (
        <p className="mt-0.5 text-[12.5px] text-muted">{chapter.summary}</p>
      )}
    </div>
  );
}

/** Scroll so the span from `top` to `bottom` is visible: centered when it fits, else its start near the top. */
function scrollSpanIntoView(container: HTMLElement, top: Element, bottom: Element): void {
  const cr = container.getBoundingClientRect();
  const t = top.getBoundingClientRect().top;
  const b = bottom.getBoundingClientRect().bottom;
  const span = b - t;
  const offset = span < cr.height * 0.8 ? (cr.height - span) / 2 : cr.height * 0.12;
  container.scrollTo({ top: container.scrollTop + (t - cr.top) - offset, behavior: "smooth" });
}

/** Wraps note cards and composers so they line up with the sentence text column. */
function NoteSlot({ children }: { children: React.ReactNode }) {
  return <div className="ps-[4.25rem] pe-3 pt-1 pb-2">{children}</div>;
}

interface SelectionAction {
  ids: number[];
  x: number;
  y: number;
}

export function Transcript() {
  const detail = useRecording((s) => s.detail);
  const fontSize = useRecording((s) => s.fontSize);
  const setFontSize = useRecording((s) => s.setFontSize);
  const findOpen = useRecording((s) => s.findOpen);
  const setFindOpen = useRecording((s) => s.setFindOpen);
  const selectedTag = useRecording((s) => s.selectedTag);
  const selectTag = useRecording((s) => s.selectTag);
  const reveal = useRecording((s) => s.reveal);
  const noteDraft = useRecording((s) => s.noteDraft);
  const noteEditor = useRecording((s) => s.noteEditor);
  const noteReveal = useRecording((s) => s.noteReveal);
  const follow = usePlayer((s) => s.follow);
  const playing = usePlayer((s) => s.playing);

  const recording = detail!.recording;
  const segments = detail!.segments;
  const analysis = detail!.analysis;
  const notes = detail!.notes;

  const activeIndex = usePlayer((s) => findActiveIndex(segments, s.time));
  const scrollRef = useRef<HTMLDivElement>(null);
  const [editingId, setEditingId] = useState<number | null>(null);
  const [suspended, setSuspended] = useState(false);
  const [flashId, setFlashId] = useState<number | null>(null);
  const [query, setQuery] = useState("");
  const [findIdx, setFindIdx] = useState(0);
  const contentRef = useRef<HTMLDivElement>(null);
  const [flashNoteId, setFlashNoteId] = useState<string | null>(null);
  const [selAction, setSelAction] = useState<SelectionAction | null>(null);

  const noteLayout = useMemo(() => layoutNotes(segments, notes), [segments, notes]);
  const noteLayoutRef = useRef(noteLayout);
  noteLayoutRef.current = noteLayout;
  // Typing a note while follow-along scrolls the composer away would be maddening. A ref, so
  // closing the composer doesn't itself trigger a jump back to the playhead.
  const composing = useRef(false);
  composing.current = (noteDraft != null && noteDraft.anchorSegmentId != null) || noteEditor?.where === "transcript";

  const hasText = segments.length > 0;
  const dir = useMemo(
    () => (hasText ? detectDirection(segments.slice(0, 8).map((s) => s.text).join(" ")) : "ltr"),
    // Direction only needs to be computed once, when text first appears.
    [hasText],
  );

  const chapterById = useMemo(() => {
    const m = new Map<number, Chapter>();
    for (const c of analysis?.chapters ?? []) m.set(c.startSegment, c);
    return m;
  }, [analysis]);

  const tag = analysis?.tags.find((t) => t.name === selectedTag) ?? null;
  const tagged = useMemo(() => new Set(tag?.segments ?? []), [tag]);
  const tagIds = useMemo(() => [...tagged].sort((a, b) => a - b), [tagged]);

  // ---- find in transcript
  const q = findOpen ? query.trim() : "";
  const matches = useMemo(() => {
    const perSeg = new Map<number, [number, number][]>();
    const flat: { segId: number; range: number }[] = [];
    if (!q) return { perSeg, flat };
    for (const seg of segments) {
      const m = findMatches(seg.text, q);
      if (!m.length) continue;
      perSeg.set(seg.id, m);
      m.forEach((_, range) => flat.push({ segId: seg.id, range }));
    }
    return { perSeg, flat };
  }, [segments, q]);

  useEffect(() => setFindIdx(0), [q]);
  const current = matches.flat[Math.min(findIdx, matches.flat.length - 1)];

  useEffect(() => {
    if (!current) return;
    scrollRef.current
      ?.querySelector(`[data-seg-id="${current.segId}"]`)
      ?.scrollIntoView({ block: "center", behavior: "smooth" });
  }, [current]);

  const stepFind = useCallback(
    (d: 1 | -1) => {
      const n = matches.flat.length;
      if (n) setFindIdx((i) => (i + d + n) % n);
    },
    [matches.flat.length],
  );

  // ---- follow along
  useEffect(() => {
    if (!follow || suspended || composing.current || activeIndex < 0) return;
    const c = scrollRef.current;
    const row = c?.querySelector<HTMLElement>(`[data-seg-index="${activeIndex}"]`);
    if (!c || !row) return;
    const cr = c.getBoundingClientRect();
    const rr = row.getBoundingClientRect();
    if (rr.top < cr.top + cr.height * 0.12 || rr.bottom > cr.top + cr.height * 0.72) {
      c.scrollTo({ top: c.scrollTop + (rr.top - cr.top) - cr.height * 0.3, behavior: "smooth" });
    }
  }, [activeIndex, follow, suspended]);

  const onUserScroll = () => {
    if (follow && playing && !suspended) setSuspended(true);
  };

  // ---- reveal requests (citations, search results, topics)
  useEffect(() => {
    if (!reveal) return;
    const row = scrollRef.current?.querySelector(`[data-seg-id="${reveal.segmentId}"]`);
    row?.scrollIntoView({ block: "center", behavior: "smooth" });
    setSuspended(false);
    setFlashId(reveal.segmentId);
    const t = setTimeout(() => setFlashId(null), 1400);
    return () => clearTimeout(t);
  }, [reveal]);

  // ---- note reveal requests (Notes tab, waveform pins, library Notes page)
  useEffect(() => {
    if (!noteReveal) return;
    const c = scrollRef.current;
    if (!c) return;
    const card = c.querySelector(`[data-note-id="${CSS.escape(noteReveal.noteId)}"]`);
    const firstId = noteLayoutRef.current.firstSegment.get(noteReveal.noteId);
    const first = firstId != null ? c.querySelector(`[data-seg-id="${firstId}"]`) : null;
    const top = first ?? card;
    if (top) scrollSpanIntoView(c, top, card ?? top);
    setSuspended(false);
    setFlashNoteId(noteReveal.noteId);
    const t = setTimeout(() => setFlashNoteId(null), 1400);
    return () => clearTimeout(t);
  }, [noteReveal]);

  // ---- "Add note" for a text selection
  const onContentMouseUp = (e: React.MouseEvent) => {
    if ((e.target as HTMLElement).closest("[data-note-ui], [data-selection-action]")) return;
    const clientX = e.clientX;
    // Let the browser settle the selection first.
    requestAnimationFrame(() => {
      const content = contentRef.current;
      const sel = window.getSelection();
      if (!content || !sel || sel.isCollapsed || !sel.rangeCount) return setSelAction(null);
      const range = sel.getRangeAt(0);
      if (!content.contains(range.commonAncestorContainer)) return setSelAction(null);
      const ids = segmentIdsInRange(range, content);
      if (!ids.length) return setSelAction(null);
      const rects = range.getClientRects();
      const last = rects.length ? rects[rects.length - 1] : range.getBoundingClientRect();
      const box = content.getBoundingClientRect();
      const x = Math.max(56, Math.min(box.width - 56, clientX - box.left));
      setSelAction({ ids, x, y: last.bottom - box.top + 6 });
    });
  };

  useEffect(() => {
    if (!selAction) return;
    const onChange = () => {
      const sel = window.getSelection();
      if (!sel || sel.isCollapsed) setSelAction(null);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setSelAction(null);
    };
    document.addEventListener("selectionchange", onChange);
    window.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("selectionchange", onChange);
      window.removeEventListener("keydown", onKey);
    };
  }, [selAction]);

  useEffect(() => setSelAction(null), [segments]);

  const addNoteFromSelection = () => {
    const span = selAction && spanOfSegments(segments, selAction.ids);
    setSelAction(null);
    window.getSelection()?.removeAllRanges();
    if (span) useRecording.getState().startNote({ start: span.start, end: span.end, anchorSegmentId: span.lastId });
  };

  const handlers = useMemo<SegmentHandlers>(
    () => ({
      play: (seg: Segment) => {
        setSuspended(false);
        usePlayer.getState().seek(seg.start + 0.001, true);
      },
      copy: (seg: Segment) => {
        const sel = window.getSelection()?.toString().trim();
        const text = sel || seg.text;
        navigator.clipboard
          .writeText(text)
          .then(() => toast.success("Copied to clipboard"))
          .catch((e) => toast.error("Couldn't copy", e));
      },
      related: (seg: Segment) => void useRecording.getState().findRelated(seg),
      save: async (seg: Segment, text: string) => {
        const id = useRecording.getState().id;
        if (!id) return false;
        const updated = await attempt("Couldn't save the edit", () => api.updateSegmentText(id, seg.id, text));
        if (!updated) return false;
        useRecording.getState().replaceSegment(updated);
        toast.success("Sentence updated");
        return true;
      },
      addNote: (seg: Segment) =>
        useRecording.getState().startNote({ start: seg.start, end: seg.end, anchorSegmentId: seg.id }),
      copyLink: (seg: Segment) => {
        const id = useRecording.getState().id;
        if (id) copyMomentLink(id, seg.id);
      },
      showNote: (note: Note) => useRecording.getState().revealNote(note.id),
    }),
    [],
  );

  const renderNote = (note: Note) => {
    const store = useRecording.getState();
    if (noteEditor?.id === note.id && noteEditor.where === "transcript") {
      return (
        <NoteSlot key={note.id}>
          <NoteComposer
            editing
            start={note.start}
            end={note.end}
            initialBody={noteEditor.body ?? note.body}
            initialColor={noteEditor.color !== undefined ? noteEditor.color : note.color}
            onSave={(body, color) => void store.saveEdit(note.id, { start: note.start, end: note.end, body, color })}
            onCancel={() => store.editNote(null)}
          />
        </NoteSlot>
      );
    }
    return (
      <NoteSlot key={note.id}>
        <NoteCard
          note={note}
          flash={flashNoteId === note.id}
          onPlay={() => goToNote(note)}
          onEdit={() => store.editNote({ id: note.id, where: "transcript" })}
          onDelete={() => void store.deleteNote(note.id)}
        />
      </NoteSlot>
    );
  };

  const draftComposer = noteDraft && noteDraft.anchorSegmentId != null && (
    <NoteSlot>
      <NoteComposer
        start={noteDraft.start}
        end={noteDraft.end}
        initialBody={noteDraft.body}
        initialColor={noteDraft.color ?? null}
        onSave={(body, color) => void useRecording.getState().saveDraft(body, color)}
        onCancel={() => useRecording.getState().cancelNote()}
      />
    </NoteSlot>
  );

  const fontIdx = FONT_SIZES.indexOf(fontSize as (typeof FONT_SIZES)[number]);
  const running = recording.status === "queued" || recording.status === "transcribing";

  return (
    <div className="relative flex min-h-0 flex-1 flex-col">
      <div className="flex h-10 shrink-0 items-center gap-2 border-b border-line px-4">
        <span className="text-[12.5px] font-medium">Transcript</span>
        {hasText && <span className="text-[12px] text-faint">{pluralize(segments.length, "sentence")}</span>}
        <div className="ml-auto flex items-center gap-0.5">
          <IconButton
            label={`Find in transcript (${MOD}+F)`}
            size="sm"
            active={findOpen}
            onClick={() => setFindOpen(!findOpen)}
            disabled={!hasText}
          >
            <Search className="size-3.5" />
          </IconButton>
          <div className="mx-1 h-4 w-px bg-line" />
          <IconButton
            label="Smaller text"
            size="sm"
            onClick={() => setFontSize(FONT_SIZES[Math.max(0, fontIdx - 1)])}
            disabled={fontIdx <= 0}
          >
            <span className="text-[11px] font-semibold">A−</span>
          </IconButton>
          <IconButton
            label="Larger text"
            size="sm"
            onClick={() => setFontSize(FONT_SIZES[Math.min(FONT_SIZES.length - 1, fontIdx + 1)])}
            disabled={fontIdx >= FONT_SIZES.length - 1}
          >
            <span className="text-[13px] font-semibold">A+</span>
          </IconButton>
        </div>
      </div>

      {findOpen && hasText && (
        <FindBar
          query={query}
          setQuery={setQuery}
          count={matches.flat.length}
          index={Math.min(findIdx, Math.max(0, matches.flat.length - 1))}
          step={stepFind}
          onClose={() => setFindOpen(false)}
        />
      )}
      {tag && <TagBar name={tag.name} ids={tagIds} onClose={() => selectTag(null)} />}

      <div
        ref={scrollRef}
        className="min-h-0 flex-1 overflow-y-auto"
        onWheel={onUserScroll}
        onTouchMove={onUserScroll}
      >
        {running && <TranscriptionProgressCard recording={recording} />}
        {recording.status === "new" && <TranscribeCta recording={recording} />}
        {recording.status === "needsDecode" && <DecodePanel recording={recording} />}
        {recording.status === "error" && !hasText && <TranscriptionErrorCard recording={recording} />}
        {recording.status === "ready" && !hasText && (
          <div className="py-16 text-center text-[13px] text-muted">No speech was detected in this recording.</div>
        )}

        {hasText && (
          <div
            ref={contentRef}
            dir={dir}
            className="relative mx-auto max-w-3xl px-4 pt-3 pb-28"
            style={{ fontSize }}
            onMouseUp={onContentMouseUp}
          >
            {noteLayout.before.map(renderNote)}
            {segments.map((seg, i) => {
              const chapter = chapterById.get(seg.id);
              const ranges = matches.perSeg.get(seg.id) ?? NO_RANGES;
              return (
                <Fragment key={seg.id}>
                  {chapter && <ChapterHeading chapter={chapter} />}
                  <SegmentRow
                    seg={seg}
                    index={i}
                    active={i === activeIndex}
                    tagged={tagged.has(seg.id)}
                    flash={flashId === seg.id}
                    findRanges={ranges}
                    currentFind={current && current.segId === seg.id ? current.range : -1}
                    editing={editingId === seg.id}
                    setEditing={setEditingId}
                    handlers={handlers}
                    notes={noteLayout.bySegment.get(seg.id)}
                    noteBadge={noteLayout.badgeAt.has(seg.id)}
                  />
                  {noteLayout.after.get(seg.id)?.map(renderNote)}
                  {noteDraft?.anchorSegmentId === seg.id && draftComposer}
                </Fragment>
              );
            })}
            {selAction && (
              <button
                type="button"
                data-selection-action
                dir="ltr"
                onMouseDown={(e) => e.preventDefault()}
                onClick={addNoteFromSelection}
                style={{ left: selAction.x, top: selAction.y }}
                className="absolute z-20 flex -translate-x-1/2 items-center gap-1.5 rounded-lg border border-line bg-surface px-2.5 py-1 font-sans text-[12.5px] font-medium text-fg shadow-pop animate-pop-in hover:bg-hover"
              >
                <StickyNote className="size-3.5 text-accent" aria-hidden />
                Add note
                {selAction.ids.length > 1 && <span className="text-faint">· {pluralize(selAction.ids.length, "sentence")}</span>}
              </button>
            )}
          </div>
        )}
      </div>

      {suspended && follow && (
        <button
          type="button"
          onClick={() => setSuspended(false)}
          className="absolute bottom-4 left-1/2 z-10 flex -translate-x-1/2 items-center gap-1.5 rounded-full border border-line bg-surface px-3.5 py-1.5 text-[12.5px] font-medium shadow-pop animate-slide-up hover:bg-hover"
        >
          <Crosshair className="size-3.5 text-accent" /> Back to the current sentence
        </button>
      )}
    </div>
  );
}
