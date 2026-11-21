import { useEffect, useMemo, useState } from "react";
import clsx from "clsx";
import { FileAudio, Search, StickyNote } from "lucide-react";
import { api } from "../lib/api";
import { errorMessage } from "../lib/errors";
import { formatDate, pluralize } from "../lib/format";
import { groupNotesByRecording, noteColor } from "../lib/notes";
import type { NoteWithRecording } from "../lib/types";
import { useApp } from "../store/app";
import { NoteBody, NoteTimeChip } from "../components/notes/NoteCard";
import { EmptyState, Spinner, inputClass } from "../components/ui/misc";

export function NotesView() {
  const openRecording = useApp((s) => s.openRecording);
  const [notes, setNotes] = useState<NoteWithRecording[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = useState("");

  useEffect(() => {
    let alive = true;
    api
      .listAllNotes()
      .then((n) => alive && (setNotes(n), setError(null)))
      .catch((e) => alive && setError(errorMessage(e)));
    return () => {
      alive = false;
    };
  }, []);

  const groups = useMemo(() => groupNotesByRecording(notes ?? [], filter), [notes, filter]);
  const shown = groups.reduce((n, g) => n + g.notes.length, 0);

  return (
    <div className="flex h-full flex-col">
      <header className="flex items-center gap-3 border-b border-line px-6 py-3.5">
        <div className="mr-auto">
          <h1 className="text-[17px] font-semibold">Notes</h1>
          <p className="text-[12.5px] text-muted">Everything you've written on your recordings, with a jump to each moment.</p>
        </div>
        {notes && notes.length > 0 && (
          <div className="relative w-64">
            <Search className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-faint" />
            <input
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
              placeholder="Filter notes…"
              aria-label="Filter notes"
              dir="auto"
              className={clsx(inputClass, "pl-8")}
            />
          </div>
        )}
      </header>

      <div className="min-h-0 flex-1 overflow-y-auto p-6">
        {error ? (
          <div className="mx-auto max-w-3xl rounded-lg bg-danger-soft px-3 py-2 text-[13px] text-danger">Couldn't load notes: {error}</div>
        ) : !notes ? (
          <div className="flex justify-center py-10">
            <Spinner />
          </div>
        ) : notes.length === 0 ? (
          <EmptyState icon={<StickyNote />} title="No notes yet" className="pt-16">
            Open a recording, select some of the transcript and click <b className="font-medium text-fg">Add note</b> — or hover a sentence and use
            the note button. Your notes from every recording collect here.
          </EmptyState>
        ) : groups.length === 0 ? (
          <div className="py-10 text-center text-[13px] text-muted">
            No note matches “<span dir="auto">{filter}</span>”.
          </div>
        ) : (
          <div className="mx-auto max-w-3xl">
            <p className="mb-4 text-[12.5px] text-muted">
              {pluralize(shown, "note")} in {pluralize(groups.length, "recording")}
            </p>
            <div className="space-y-4">
              {groups.map((g) => (
                <section key={g.recordingId} className="overflow-hidden rounded-xl border border-line bg-surface shadow-card">
                  <button
                    type="button"
                    onClick={() => openRecording(g.recordingId)}
                    className="flex w-full items-center gap-2 border-b border-line bg-sunken/50 px-4 py-2.5 text-left text-[13px] font-medium hover:bg-hover"
                  >
                    <FileAudio className="size-4 text-faint" />
                    <span className="min-w-0 flex-1 truncate" dir="auto">
                      {g.title}
                    </span>
                    <span className="text-[12px] font-normal text-faint">{pluralize(g.notes.length, "note")}</span>
                  </button>
                  <ul className="divide-y divide-line">
                    {g.notes.map((n) => (
                      <li key={n.id} className="relative flex gap-3 px-4 py-3">
                        <span aria-hidden className="absolute inset-y-3 left-1.5 w-[3px] rounded-full" style={{ background: noteColor(n.color) }} />
                        <div className="w-24 shrink-0 pt-0.5">
                          <NoteTimeChip note={n} onClick={() => openRecording(g.recordingId, { time: n.start + 0.001, noteId: n.id })} />
                          <div className="mt-1 text-[11px] text-faint">{formatDate(n.createdAt)}</div>
                        </div>
                        <NoteBody note={n} className="min-w-0 flex-1" />
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
  );
}
