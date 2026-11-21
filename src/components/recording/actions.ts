// Recording-level actions shared by the header, transcript and side panel.

import { api } from "../../lib/api";
import type { Citation } from "../../lib/citations";
import type { ExportFormat, Recording } from "../../lib/types";
import { indexOfSegmentId } from "../../lib/segments";
import { pickSavePath } from "../../lib/platform";
import { useApp } from "../../store/app";
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
