import { describe, expect, it } from "vitest";
import {
  activeNoteKey,
  compareNotes,
  groupNotesByRecording,
  layoutNotes,
  noteCoversSegment,
  notePreview,
  spanOfSegments,
  upsertNote,
} from "./notes";
import type { Note, NoteWithRecording } from "./types";

const segs = [
  { id: 0, start: 0.5, end: 3 },
  { id: 1, start: 3.2, end: 6 },
  { id: 2, start: 8, end: 10 },
  { id: 3, start: 10, end: 12.5 },
  { id: 4, start: 12.4, end: 15 },
];

let n = 0;
function note(start: number, end: number, extra: Partial<Note> = {}): Note {
  n++;
  return {
    id: `n${n}`,
    recordingId: "r1",
    start,
    end,
    body: `note ${n}`,
    color: null,
    createdAt: `2026-11-0${(n % 9) + 1}T10:00:00Z`,
    updatedAt: `2026-11-0${(n % 9) + 1}T10:00:00Z`,
    ...extra,
  };
}

describe("noteCoversSegment", () => {
  it("covers the sentence a note was made from", () => {
    expect(noteCoversSegment({ start: 3.2, end: 6 }, segs[1])).toBe(true);
  });
  it("ignores neighbours that merely touch or overlap trivially", () => {
    expect(noteCoversSegment({ start: 8, end: 10 }, segs[3])).toBe(false); // touches at 10
    expect(noteCoversSegment({ start: 10, end: 12.5 }, segs[4])).toBe(false); // 0.1 s overlap
    expect(noteCoversSegment({ start: 3.2, end: 6 }, segs[0])).toBe(false);
  });
  it("covers a sentence whose midpoint is inside the note", () => {
    expect(noteCoversSegment({ start: 9, end: 9.1 }, { start: 9, end: 9.15 })).toBe(true);
  });
  it("covers the sentence that contains a point-in-time note", () => {
    expect(noteCoversSegment({ start: 4, end: 4 }, segs[1])).toBe(true);
    expect(noteCoversSegment({ start: 6, end: 6 }, segs[1])).toBe(false);
    expect(noteCoversSegment({ start: 7, end: 7 }, segs[1])).toBe(false);
  });
});

describe("layoutNotes", () => {
  it("maps a range note onto every sentence it spans and puts its card after the last", () => {
    const a = note(3.2, 12.5);
    const l = layoutNotes(segs, [a]);
    expect([...l.bySegment.keys()]).toEqual([1, 2, 3]);
    expect(l.after.get(3)).toEqual([a]);
    expect(l.firstSegment.get(a.id)).toBe(1);
    expect([...l.badgeAt]).toEqual([1]);
  });

  it("anchors notes in silent gaps after the previous sentence, and early notes before the first", () => {
    const gap = note(6.5, 7.5);
    const early = note(0, 0.2);
    const l = layoutNotes(segs, [early, gap]);
    expect(l.bySegment.size).toBe(0);
    expect(l.after.get(1)).toEqual([gap]);
    expect(l.before).toEqual([early]);
    expect(l.firstSegment.get(early.id)).toBe(0);
  });

  it("shows a new badge wherever the set of notes changes", () => {
    const a = note(0.5, 6);
    const b = note(3.2, 10);
    const l = layoutNotes(segs, [a, b]);
    expect(l.bySegment.get(0)).toEqual([a]);
    expect(l.bySegment.get(1)).toEqual([a, b]);
    expect(l.bySegment.get(2)).toEqual([b]);
    expect([...l.badgeAt]).toEqual([0, 1, 2]);
    expect(l.after.get(1)).toEqual([a]);
    expect(l.after.get(2)).toEqual([b]);
  });

  it("handles an empty transcript", () => {
    const l = layoutNotes([], [note(1, 2)]);
    expect(l.bySegment.size + l.after.size + l.before.length).toBe(0);
  });
});

describe("spanOfSegments", () => {
  it("spans from the first selected sentence's start to the last one's end, in any order", () => {
    expect(spanOfSegments(segs, [3, 1])).toEqual({ start: 3.2, end: 12.5, firstId: 1, lastId: 3 });
    expect(spanOfSegments(segs, [2])).toEqual({ start: 8, end: 10, firstId: 2, lastId: 2 });
  });
  it("returns null when none of the ids exist", () => {
    expect(spanOfSegments(segs, [])).toBeNull();
    expect(spanOfSegments(segs, [42])).toBeNull();
  });
});

describe("sorting", () => {
  it("orders by start, then end, then creation", () => {
    const a = note(5, 9, { createdAt: "2026-01-02T00:00:00Z" });
    const b = note(5, 7);
    const c = note(1, 20);
    const d = note(5, 9, { createdAt: "2026-01-01T00:00:00Z" });
    expect([a, b, c, d].sort(compareNotes).map((x) => x.id)).toEqual([c.id, b.id, d.id, a.id]);
  });
  it("upsertNote replaces by id and keeps time order", () => {
    const a = note(1, 2);
    const b = note(5, 6);
    const moved = { ...a, start: 9, end: 10 };
    expect(upsertNote([a, b], moved)).toEqual([b, moved]);
    const c = note(3, 4);
    expect(upsertNote([a, b], c).map((x) => x.id)).toEqual([a.id, c.id, b.id]);
  });
});

describe("groupNotesByRecording", () => {
  const lib = (recordingId: string, recordingTitle: string, start: number, body: string): NoteWithRecording => ({
    ...note(start, start + 2, { recordingId, body }),
    recordingTitle,
  });
  const notes = [
    lib("new", "Planning meeting", 30, "Budget is **200M**"),
    lib("new", "Planning meeting", 4, "Search is hard"),
    lib("old", "درس حافظ", 12, "گوته و دیوان غربی‌شرقی"),
  ];

  it("keeps recording order and sorts notes by time", () => {
    const g = groupNotesByRecording(notes);
    expect(g.map((x) => x.recordingId)).toEqual(["new", "old"]);
    expect(g[0].notes.map((x) => x.start)).toEqual([4, 30]);
  });
  it("filters case-insensitively on body and title", () => {
    expect(groupNotesByRecording(notes, "budget").flatMap((g) => g.notes.map((x) => x.body))).toEqual(["Budget is **200M**"]);
    expect(groupNotesByRecording(notes, "PLANNING")[0].notes).toHaveLength(2);
    expect(groupNotesByRecording(notes, "گوته")[0].recordingId).toBe("old");
    expect(groupNotesByRecording(notes, "nothing like this")).toEqual([]);
  });
});

describe("activeNoteKey", () => {
  it("lists the notes playing at a time, giving point notes a short window", () => {
    const a = note(1, 5);
    const b = note(4, 4);
    expect(activeNoteKey([a, b], 4.2)).toBe(`${a.id}\n${b.id}`);
    expect(activeNoteKey([a, b], 5)).toBe("");
  });
});

describe("notePreview", () => {
  it("strips markdown down to readable text", () => {
    expect(notePreview("**Action items**\n- Reza: [prototype](https://x.dev) by Friday\n- see [rec:rec-1#12]")).toBe(
      "Action items Reza: prototype by Friday see #12",
    );
    expect(notePreview("# Title\n> quoted _emphasis_ and `code`")).toBe("Title quoted emphasis and code");
  });
  it("truncates long text", () => {
    expect(notePreview("a".repeat(200), 10)).toBe(`${"a".repeat(9)}…`);
  });
});
