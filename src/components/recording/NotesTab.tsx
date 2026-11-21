import { useMemo } from "react";
import { Plus, StickyNote } from "lucide-react";
import { formatTime, pluralize } from "../../lib/format";
import { activeNoteKey } from "../../lib/notes";
import { findActiveIndex } from "../../lib/segments";
import { usePlayer } from "../../store/player";
import { useRecording } from "../../store/recording";
import { Button } from "../ui/Button";
import { EmptyState } from "../ui/misc";
import { NoteCard } from "../notes/NoteCard";
import { NoteComposer } from "../notes/NoteComposer";
import { goToNote } from "./actions";

/** New note at the playhead: on the current sentence when there's a transcript. */
function addNoteHere(): void {
  const { detail, startNote } = useRecording.getState();
  if (!detail) return;
  const t = usePlayer.getState().time;
  const segs = detail.segments;
  if (segs.length) {
    const seg = segs[Math.max(0, findActiveIndex(segs, t))];
    startNote({ start: seg.start, end: seg.end, anchorSegmentId: seg.id });
  } else {
    startNote({ start: t, end: t, anchorSegmentId: null });
  }
}

export function NotesTab() {
  const detail = useRecording((s) => s.detail)!;
  const draft = useRecording((s) => s.noteDraft);
  const editor = useRecording((s) => s.noteEditor);
  const now = usePlayer((s) => formatTime(s.time));
  const { notes, segments } = detail;
  const activeKey = usePlayer((s) => activeNoteKey(notes, s.time));
  const active = useMemo(() => new Set(activeKey.split("\n")), [activeKey]);
  const store = useRecording.getState();
  const panelDraft = draft && draft.anchorSegmentId == null ? draft : null;

  return (
    <div>
      <div className="flex items-center gap-2 border-b border-line px-4 py-2">
        <span className="text-[12px] text-muted">{notes.length ? pluralize(notes.length, "note") : "No notes yet"}</span>
        <Button size="sm" variant="ghost" className="ml-auto" icon={<Plus className="size-3.5" />} onClick={addNoteHere}>
          Add note at {now}
        </Button>
      </div>

      {panelDraft && (
        <div className="p-3">
          <NoteComposer
            start={panelDraft.start}
            end={panelDraft.end}
            initialBody={panelDraft.body}
            initialColor={panelDraft.color ?? null}
            onSave={(body, color) => void store.saveDraft(body, color)}
            onCancel={() => store.cancelNote()}
          />
        </div>
      )}

      {notes.length === 0 && !panelDraft ? (
        <EmptyState icon={<StickyNote />} title="Keep notes on this recording">
          Select text in the transcript and click <b className="font-medium text-fg">Add note</b>, or hover a sentence and pick the note
          button in its menu. Notes can hold links, and stay attached to their time even if you re-transcribe.
          {!segments.length && " Without a transcript, notes attach to the current playback time."}
        </EmptyState>
      ) : (
        <ul className="space-y-2 p-3">
          {notes.map((note) => (
            <li key={note.id}>
              {editor?.id === note.id && editor.where === "panel" ? (
                <NoteComposer
                  editing
                  start={note.start}
                  end={note.end}
                  initialBody={editor.body ?? note.body}
                  initialColor={editor.color !== undefined ? editor.color : note.color}
                  onSave={(body, color) => void store.saveEdit(note.id, { start: note.start, end: note.end, body, color })}
                  onCancel={() => store.editNote(null)}
                />
              ) : (
                <NoteCard
                  note={note}
                  clamp
                  active={active.has(note.id)}
                  onPlay={() => goToNote(note)}
                  onEdit={() => store.editNote({ id: note.id, where: "panel" })}
                  onDelete={() => void store.deleteNote(note.id)}
                />
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
