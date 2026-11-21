// Recording-level actions shared by the header, transcript and side panel.

import { api } from "../../lib/api";
import type { Citation } from "../../lib/citations";
import { pluralize } from "../../lib/format";
import type { ExportFormat, Note, Recording } from "../../lib/types";
import { indexOfSegmentId } from "../../lib/segments";
import { pickSavePath } from "../../lib/platform";
import { useApp } from "../../store/app";
import { isCancellation } from "../../store/chat";
import { useLibrary } from "../../store/library";
import { usePlayer } from "../../store/player";
import { useRecording } from "../../store/recording";
import { attempt, toast } from "../../store/toasts";

/** Seek to a segment of the open recording and scroll it into view. */
export function seekToSegment(segmentId: number, play = true): void {
  const segments = useRecording.getState().detail?.segments ?? [];
  const i = indexOfSegmentId(segments, segmentId);
  if (i < 0) {
    toast.info("That part of the transcript isn't available", `Segment ${segmentId} doesn't exist (anymore).`);
    return;
  }
  usePlayer.getState().seek(segments[i].start + 0.001, play);
  useRecording.getState().revealSegment(segmentId);
}

/** Handle a citation chip: local ones seek here, library ones open the recording. */
export function followCitation(c: Citation): void {
  const currentId = useRecording.getState().id;
  if (c.recordingId && c.recordingId !== currentId) {
    if (!useLibrary.getState().recordings.some((r) => r.id === c.recordingId)) {
      toast.info("That recording isn't in your library anymore");
      return;
    }
    useApp.getState().openRecording(c.recordingId, { segmentId: c.start, play: true });
    return;
  }
  seekToSegment(c.start, true);
}

/** Play a note of the open recording from its start and scroll the transcript to it. */
export function goToNote(note: Pick<Note, "id" | "start">, play = true): void {
  usePlayer.getState().seek(note.start + 0.001, play);
  useRecording.getState().revealNote(note.id);
}

/**
 * Handle a citation chip inside a note, wherever the note is shown. `[12]` refers to
 * the note's own recording; `[rec:<id>#12]` to any recording.
 */
export function followNoteCitation(c: Citation, noteRecordingId: string): void {
  const target = c.recordingId ?? noteRecordingId;
  const route = useApp.getState().route;
  if (route.name === "recording" && route.id === target && useRecording.getState().id === target) {
    seekToSegment(c.start, true);
    return;
  }
  if (!useLibrary.getState().recordings.some((r) => r.id === target)) {
    toast.info("That recording isn't in your library anymore");
    return;
  }
  useApp.getState().openRecording(target, { segmentId: c.start, play: true });
}

/** Copy a `[rec:<id>#<segment>]` link that notes (in any recording) render as a chip. */
export function copyMomentLink(recordingId: string, segmentId: number): void {
  navigator.clipboard
    .writeText(`[rec:${recordingId}#${segmentId}]`)
    .then(() => toast.success("Link copied — paste it into a note"))
    .catch((e) => toast.error("Couldn't copy", e));
}

export async function startTranscription(id: string, modelId: string | null): Promise<void> {
  await attempt("Couldn't start transcription", () => api.transcribe(id, modelId));
}

export async function cancelTranscription(id: string): Promise<void> {
  await attempt("Couldn't cancel transcription", () => api.cancelTranscription(id));
}

export async function analyzeRecording(id: string): Promise<boolean> {
  const analysis = await attempt("AI analysis failed", () => api.analyze(id));
  if (!analysis) return false;
  const rec = useRecording.getState();
  if (rec.id === id) {
    rec.setAnalysis(analysis);
    void rec.reload();
  }
  toast.success("Analysis ready", "Summary, chapters and topics were added.");
  return true;
}

/** Correct recognition errors with the AI model. Sentences update as batches finish. */
export async function fixTranscript(id: string): Promise<void> {
  const lib = useLibrary.getState();
  if (lib.fixing[id]) return;
  lib.setFixing(id, { done: 0, total: 0 });
  try {
    const s = await api.fixTranscript(id);
    if (s.changed) {
      toast.success("Transcript fixed", `${pluralize(s.changed, "sentence")} corrected. Hover a highlighted word to see what was heard.`);
    } else if (!s.failed) {
      toast.info("Nothing to fix", "The model didn't find recognition errors.");
    }
    if (s.failed) toast.error(`${s.failed} of ${s.batches} parts couldn't be fixed`, s.error ?? undefined);
  } catch (e) {
    if (isCancellation(e)) toast.info("Stopped fixing", "Corrections made so far were kept.");
    else toast.error("Couldn't fix the transcript", e);
  } finally {
    useLibrary.getState().setFixing(id, null);
    // Progress events can race a (re)load of the recording; settle on what was saved.
    if (useRecording.getState().id === id) void useRecording.getState().reload();
  }
}

export async function cancelFixTranscript(id: string): Promise<void> {
  await attempt("Couldn't stop fixing", () => api.cancelFixTranscript(id));
}

/** Put back the recognized text of AI-fixed sentences (`segmentIds` null = all). */
export async function revertTranscriptFixes(id: string, segmentIds: number[] | null): Promise<boolean> {
  const reverted = await attempt("Couldn't restore the original text", () => api.revertTranscriptFixes(id, segmentIds));
  if (!reverted) return false;
  useRecording.getState().updateSegments(id, reverted);
  toast.success(segmentIds ? "Original text restored" : `Original text restored in ${pluralize(reverted.length, "sentence")}`);
  return true;
}

export async function enableSemanticSearch(id: string): Promise<boolean> {
  const ok = await attempt("Couldn't build the semantic index", async () => {
    await api.indexEmbeddings(id);
    return true;
  });
  if (!ok) return false;
  if (useRecording.getState().id === id) useRecording.getState().setEmbeddingsIndexed(true);
  toast.success("Semantic search enabled", "Search and “find related” now match meaning, not just words.");
  return true;
}

export async function exportTranscript(recording: Recording, format: ExportFormat): Promise<void> {
  const path = await attempt("Couldn't open the save dialog", () => pickSavePath(recording.title, format));
  if (!path) return;
  const ok = await attempt("Export failed", async () => {
    await api.exportTranscript(recording.id, format, path);
    return true;
  });
  if (ok) toast.success("Transcript exported", path);
}
