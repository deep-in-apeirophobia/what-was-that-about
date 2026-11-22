import { Fragment, memo, useEffect, useMemo, useRef, useState } from "react";
import clsx from "clsx";
import { Check, Copy, Link, Network, Pencil, Play, StickyNote, Undo2, X } from "lucide-react";
import { diffFix, segmentWordRanges, type FixPiece } from "../../lib/fixes";
import { formatTime, pluralize } from "../../lib/format";
import { noteColor } from "../../lib/notes";
import { findContainingIndex } from "../../lib/segments";
import { splitByRanges } from "../../lib/text";
import type { Note, Segment } from "../../lib/types";
import { usePlayer } from "../../store/player";
import { textareaClass } from "../ui/misc";

export const NO_RANGES: [number, number][] = [];

export interface SegmentHandlers {
  play: (seg: Segment) => void;
  copy: (seg: Segment) => void;
  related: (seg: Segment) => void;
  save: (seg: Segment, text: string) => Promise<boolean>;
  addNote: (seg: Segment) => void;
  copyLink: (seg: Segment) => void;
  showNote: (note: Note) => void;
  /** Put back the recognized text of an AI-fixed sentence. */
  restoreOriginal: (seg: Segment) => void;
}

/** Active-row text with the word being spoken highlighted (only this row re-renders per word). */
function KaraokeText({ seg }: { seg: Segment }) {
  const ranges = useMemo(() => segmentWordRanges(seg), [seg]);
  const wi = usePlayer((s) => findContainingIndex(seg.words, s.time));
  const r = wi >= 0 ? ranges[wi] : null;
  if (!r) return <>{seg.text}</>;
  return (
    <>
      {seg.text.slice(0, r[0])}
      <span className="word-active">{seg.text.slice(r[0], r[1])}</span>
      {seg.text.slice(r[1])}
    </>
  );
}

/** `text` (starting at `offset` in the sentence) with the part inside `word` highlighted. */
function withActiveWord(text: string, offset: number, word: [number, number] | null): React.ReactNode {
  const s = word ? Math.max(word[0] - offset, 0) : 0;
  const e = word ? Math.min(word[1] - offset, text.length) : 0;
  if (e <= s) return text;
  return (
    <>
      {text.slice(0, s)}
      <span className="word-active">{text.slice(s, e)}</span>
      {text.slice(e)}
    </>
  );
}

/** Sentence text with AI corrections marked; the transcript shows what was heard on hover. */
function FixText({ pieces, word = null }: { pieces: FixPiece[]; word?: [number, number] | null }) {
  let offset = 0;
  return (
    <>
      {pieces.map((p, i) => {
        const at = offset;
        offset += p.text.length;
        if (p.original === null) return <Fragment key={i}>{withActiveWord(p.text, at, word)}</Fragment>;
        return (
          <mark key={i} className={clsx(p.text ? "fix-hit" : "fix-removed", p.minor && "fix-minor")} data-fix-original={p.original}>
            {withActiveWord(p.text, at, word)}
          </mark>
        );
      })}
    </>
  );
}

/** `FixText` on the active row, following the spoken word like `KaraokeText`. */
function FixKaraokeText({ seg, pieces }: { seg: Segment; pieces: FixPiece[] }) {
  const ranges = useMemo(() => segmentWordRanges(seg), [seg]);
  const wi = usePlayer((s) => findContainingIndex(seg.words, s.time));
  return <FixText pieces={pieces} word={wi >= 0 ? ranges[wi] : null} />;
}

function FindText({ text, ranges, current }: { text: string; ranges: [number, number][]; current: number }) {
  const pieces = useMemo(() => splitByRanges(text, ranges), [text, ranges]);
  return (
    <>
      {pieces.map((p, i) =>
        p.highlighted ? (
          <mark key={i} className="find-hit" data-current={p.rangeIndex === current ? "true" : undefined}>
            {p.text}
          </mark>
        ) : (
          <span key={i}>{p.text}</span>
        ),
      )}
    </>
  );
}

function EditBox({ seg, onDone, save }: { seg: Segment; onDone: () => void; save: SegmentHandlers["save"] }) {
  const [text, setText] = useState(seg.text);
  const [busy, setBusy] = useState(false);
  const ref = useRef<HTMLTextAreaElement>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.focus();
    el.setSelectionRange(el.value.length, el.value.length);
  }, []);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight + 2}px`;
  }, [text]);
  const commit = async () => {
    const t = text.trim();
    if (!t || t === seg.text) return onDone();
    setBusy(true);
    const ok = await save(seg, t);
    setBusy(false);
    if (ok) onDone();
  };
  return (
    <div className="flex-1">
      <textarea
        ref={ref}
        value={text}
        dir="auto"
        disabled={busy}
        aria-label="Edit sentence"
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            void commit();
          } else if (e.key === "Escape") {
            e.preventDefault();
            e.stopPropagation();
            onDone();
          }
        }}
        className={clsx(textareaClass, "font-fa leading-[1.9]")}
        style={{ fontSize: "inherit" }}
      />
      <div className="mt-1.5 flex items-center gap-2 text-[11.5px] text-faint">
        <button
          type="button"
          onClick={() => void commit()}
          className="inline-flex items-center gap-1 rounded-md bg-accent px-2 py-1 font-medium text-accent-fg"
        >
          <Check className="size-3" /> Save
        </button>
        <button type="button" onClick={onDone} className="inline-flex items-center gap-1 rounded-md px-2 py-1 hover:bg-hover">
          <X className="size-3" /> Cancel
        </button>
        <span>Enter to save · Shift+Enter for a new line · Esc to cancel</span>
      </div>
    </div>
  );
}

export interface SegmentRowProps {
  seg: Segment;
  index: number;
  active: boolean;
  tagged: boolean;
  flash: boolean;
  findRanges: [number, number][];
  currentFind: number;
  editing: boolean;
  setEditing: (id: number | null) => void;
  handlers: SegmentHandlers;
  /** Notes covering this sentence. */
  notes?: Note[];
  /** Show the note-count badge (first row of a run with the same notes). */
  noteBadge?: boolean;
  /** Mark the words an AI fix changed. */
  highlightFixes: boolean;
}

/** Colored bars in the margin, one per note (up to three). */
function NoteMarkers({ notes }: { notes: Note[] }) {
  return (
    <span aria-hidden className="pointer-events-none absolute inset-y-0.5 -start-2.5 flex gap-[2px]">
      {notes.slice(0, 3).map((n) => (
        <span key={n.id} className="w-[3px] rounded-full opacity-80" style={{ background: noteColor(n.color) }} />
      ))}
    </span>
  );
}

function SegmentRowImpl({
  seg,
  index,
  active,
  tagged,
  flash,
  findRanges,
  currentFind,
  editing,
  setEditing,
  handlers,
  notes,
  noteBadge,
  highlightFixes,
}: SegmentRowProps) {
  const hasWords = seg.words.length > 0;
  const fixed = seg.originalText != null;
  const fixPieces = useMemo(
    () => (highlightFixes && seg.originalText != null ? diffFix(seg.originalText, seg.text) : null),
    [highlightFixes, seg.originalText, seg.text],
  );
  const clickText = () => {
    // Don't hijack a text selection.
    const sel = window.getSelection();
    if (sel && !sel.isCollapsed && sel.toString().trim()) return;
    handlers.play(seg);
  };

  return (
    <div
      data-seg-index={index}
      data-seg-id={seg.id}
      className={clsx(
        "group relative flex gap-3 rounded-xl px-3 py-1.5 transition-colors duration-200",
        active ? "bg-active-row" : tagged ? "bg-ok-soft/50 hover:bg-ok-soft/80" : "hover:bg-hover/50",
        tagged && "shadow-[inset_3px_0_0_var(--ok)] rtl:shadow-[inset_-3px_0_0_var(--ok)]",
        flash && "ring-2 ring-accent/50",
      )}
    >
      {notes && <NoteMarkers notes={notes} />}
      <div className="mt-[0.55em] flex h-fit w-11 shrink-0 flex-col items-start gap-1">
        <button
          type="button"
          onClick={() => handlers.play(seg)}
          className={clsx(
            "text-start text-[11.5px] tabular-nums transition-colors",
            active ? "font-medium text-accent" : "text-faint hover:text-accent",
          )}
          aria-label={`Play from ${formatTime(seg.start)}`}
          dir="ltr"
        >
          {formatTime(seg.start)}
        </button>
        {notes && noteBadge && (
          <button
            type="button"
            onClick={() => handlers.showNote(notes[0])}
            aria-label={`${pluralize(notes.length, "note")} on this part — show`}
            title={pluralize(notes.length, "note")}
            className="-ms-1 inline-flex h-4 items-center gap-0.5 rounded px-1 text-[10.5px] font-semibold tabular-nums transition-colors hover:bg-hover"
            style={{ color: noteColor(notes[0].color) }}
            dir="ltr"
          >
            <StickyNote className="size-3" aria-hidden />
            {notes.length}
          </button>
        )}
      </div>

      {editing ? (
        <EditBox seg={seg} save={handlers.save} onDone={() => setEditing(null)} />
      ) : (
        <div
          role="button"
          tabIndex={0}
          dir="auto"
          data-seg-text
          onClick={clickText}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              handlers.play(seg);
            }
          }}
          className={clsx(
            "min-w-0 flex-1 cursor-pointer font-fa leading-[1.9] outline-none focus-visible:rounded-md focus-visible:ring-2 focus-visible:ring-accent/40",
            active ? "text-fg" : "text-fg/85",
          )}
        >
          {findRanges.length ? (
            <FindText text={seg.text} ranges={findRanges} current={currentFind} />
          ) : fixPieces ? (
            active && hasWords ? <FixKaraokeText seg={seg} pieces={fixPieces} /> : <FixText pieces={fixPieces} />
          ) : active && hasWords ? (
            <KaraokeText seg={seg} />
          ) : (
            seg.text
          )}
        </div>
      )}

      {!editing && (
        <div
          className="pointer-events-none absolute -top-3 end-3 z-[1] flex items-center gap-0.5 rounded-lg border border-line bg-surface p-0.5 opacity-0 shadow-card transition-opacity group-focus-within:pointer-events-auto group-focus-within:opacity-100 group-hover:pointer-events-auto group-hover:opacity-100"
        >
          <RowAction label="Play from here" onClick={() => handlers.play(seg)}>
            <Play className="size-3.5" />
          </RowAction>
          <RowAction label="Copy sentence" onClick={() => handlers.copy(seg)}>
            <Copy className="size-3.5" />
          </RowAction>
          <RowAction label="Find related moments" onClick={() => handlers.related(seg)}>
            <Network className="size-3.5" />
          </RowAction>
          <RowAction label="Add note" onClick={() => handlers.addNote(seg)}>
            <StickyNote className="size-3.5" />
          </RowAction>
          <RowAction label="Copy link to this moment" onClick={() => handlers.copyLink(seg)}>
            <Link className="size-3.5" />
          </RowAction>
          <RowAction label="Edit text" onClick={() => setEditing(seg.id)}>
            <Pencil className="size-3.5" />
          </RowAction>
          {fixed && (
            <RowAction label="Restore the original text (undo the AI fix)" onClick={() => handlers.restoreOriginal(seg)}>
              <Undo2 className="size-3.5" />
            </RowAction>
          )}
        </div>
      )}
    </div>
  );
}

function RowAction({ label, onClick, children }: { label: string; onClick: () => void; children: React.ReactNode }) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      onClick={onClick}
      className="inline-flex size-6 items-center justify-center rounded-md text-muted hover:bg-hover hover:text-fg"
    >
      {children}
    </button>
  );
}

export const SegmentRow = memo(SegmentRowImpl);
