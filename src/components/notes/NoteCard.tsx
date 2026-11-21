import { useCallback, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import clsx from "clsx";
import { Pencil, Trash2 } from "lucide-react";
import type { Citation } from "../../lib/citations";
import { formatDate, formatTimeRange } from "../../lib/format";
import { isPendingNote, noteColor, noteTint } from "../../lib/notes";
import type { Note } from "../../lib/types";
import { useLibrary } from "../../store/library";
import { IconButton } from "../ui/Button";
import { Markdown } from "../ui/Markdown";
import { followNoteCitation } from "../recording/actions";

/** A note's markdown. Links open in the browser; `[12]` / `[rec:<id>#12]` become chips. */
export function NoteBody({ note, className }: { note: Pick<Note, "body" | "recordingId">; className?: string }) {
  const recordings = useLibrary((s) => s.recordings);
  const own = note.recordingId;
  const onCite = useCallback((c: Citation) => followNoteCitation(c, own), [own]);
  const renderCite = useCallback(
    (c: Citation, label: ReactNode) => {
      if (!c.recordingId || c.recordingId === own) return label;
      const title = recordings.find((r) => r.id === c.recordingId)?.title;
      return (
        <>
          {title && (
            <span className="mr-1 inline-block max-w-[180px] truncate align-bottom font-medium" dir="auto">
              {title}
            </span>
          )}
          <span className="opacity-80">#{label}</span>
        </>
      );
    },
    [recordings, own],
  );
  return <Markdown content={note.body} onCite={onCite} renderCite={renderCite} className={clsx("font-fa text-[13.5px]", className)} />;
}

/** Clickable "1:05–1:22" chip. */
export function NoteTimeChip({ note, onClick }: { note: Pick<Note, "start" | "end">; onClick: () => void }) {
  const range = formatTimeRange(note.start, note.end);
  return (
    <button
      type="button"
      dir="ltr"
      onClick={onClick}
      aria-label={`Play from ${range}`}
      title="Play from here"
      className="inline-flex h-5 shrink-0 items-center rounded-md bg-surface/70 px-1.5 text-[11.5px] font-semibold text-accent tabular-nums shadow-[0_0_0_1px_var(--line)] transition-colors hover:bg-accent-soft"
    >
      {range}
    </button>
  );
}

function NoteMeta({ note }: { note: Note }) {
  if (isPendingNote(note)) return <span dir="ltr">Saving…</span>;
  const edited = Date.parse(note.updatedAt) - Date.parse(note.createdAt) > 1000;
  const title = `Created ${new Date(note.createdAt).toLocaleString()}${edited ? `\nEdited ${new Date(note.updatedAt).toLocaleString()}` : ""}`;
  return (
    <span dir="ltr" title={title} className="truncate">
      {formatDate(note.createdAt)}
      {edited && ` · edited ${formatDate(note.updatedAt)}`}
    </span>
  );
}

function NoteActions({ note, onEdit, onDelete }: { note: Note; onEdit: () => void; onDelete: () => void }) {
  const pending = isPendingNote(note);
  return (
    <div className="ms-auto flex shrink-0 items-center opacity-0 transition-opacity group-focus-within/note:opacity-100 group-hover/note:opacity-100">
      <IconButton label="Edit note" size="xs" onClick={onEdit} disabled={pending}>
        <Pencil className="size-3.5" />
      </IconButton>
      <IconButton label="Delete note" size="xs" variant="danger-ghost" onClick={onDelete} disabled={pending}>
        <Trash2 className="size-3.5" />
      </IconButton>
    </div>
  );
}

export interface NoteCardProps {
  note: Note;
  flash?: boolean;
  /** Collapse long bodies behind "Show more" (side panel). */
  clamp?: boolean;
  /** Highlight as currently playing. */
  active?: boolean;
  onPlay: () => void;
  onEdit: () => void;
  onDelete: () => void;
  className?: string;
}

/** A note with its color, time chip, body, date and edit/delete actions. */
export function NoteCard({ note, flash, clamp, active, onPlay, onEdit, onDelete, className }: NoteCardProps) {
  return (
    <div
      data-note-id={note.id}
      data-note-ui
      className={clsx(
        "group/note relative rounded-lg border border-s-[3px] px-3 py-2 transition-[box-shadow,opacity]",
        flash && "ring-2 ring-accent/50",
        active && "shadow-[0_0_0_1px_var(--accent)]",
        isPendingNote(note) && "opacity-70",
        className,
      )}
      style={{
        background: noteTint(note.color, 9),
        borderColor: noteTint(note.color, 28, "var(--line)"),
        borderInlineStartColor: noteColor(note.color),
      }}
    >
      <div className="flex min-w-0 items-center gap-2 text-[11.5px] text-faint">
        <NoteTimeChip note={note} onClick={onPlay} />
        <NoteMeta note={note} />
        <NoteActions note={note} onEdit={onEdit} onDelete={onDelete} />
      </div>
      {clamp ? <ClampedBody note={note} /> : <NoteBody note={note} className="mt-1" />}
    </div>
  );
}

const CLAMP_FADE = "linear-gradient(to bottom, black 55%, transparent)";

function ClampedBody({ note }: { note: Note }) {
  const ref = useRef<HTMLDivElement>(null);
  const [expanded, setExpanded] = useState(false);
  const [overflows, setOverflows] = useState(false);
  useLayoutEffect(() => {
    const el = ref.current;
    if (el && !expanded) setOverflows(el.scrollHeight > el.clientHeight + 1);
  }, [note.body, expanded]);
  const fade = overflows && !expanded;
  return (
    <>
      <div
        ref={ref}
        className={clsx("mt-1", !expanded && "max-h-[7.5em] overflow-hidden")}
        style={fade ? { maskImage: CLAMP_FADE, WebkitMaskImage: CLAMP_FADE } : undefined}
      >
        <NoteBody note={note} />
      </div>
      {overflows && (
        <button type="button" onClick={() => setExpanded((v) => !v)} className="mt-0.5 text-[12px] font-medium text-accent hover:underline">
          {expanded ? "Show less" : "Show more"}
        </button>
      )}
    </>
  );
}
