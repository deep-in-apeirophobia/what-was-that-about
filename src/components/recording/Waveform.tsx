import { useEffect, useRef } from "react";
import WaveSurfer from "wavesurfer.js";
import RegionsPlugin from "wavesurfer.js/dist/plugins/regions.esm.js";
import { formatTimeRange } from "../../lib/format";
import { noteColor, notePreview } from "../../lib/notes";
import type { Chapter, Note, Waveform as WaveformData } from "../../lib/types";
import { usePlayer } from "../../store/player";
import { useApp } from "../../store/app";
import { goToNote } from "./actions";

const CHAPTER_TINTS = [
  "rgba(91, 91, 214, 0.10)",
  "rgba(36, 133, 90, 0.10)",
  "rgba(178, 107, 0, 0.10)",
  "rgba(14, 116, 144, 0.11)",
  "rgba(204, 61, 51, 0.08)",
];

const LABEL_STYLE = [
  'font: 500 10.5px/1.5 "Inter Variable", "Vazirmatn", system-ui, sans-serif',
  "padding: 0 6px",
  "margin: 2px",
  "border-radius: 4px",
  "background: var(--surface)",
  "color: var(--muted)",
  "box-shadow: 0 0 0 1px var(--line)",
  "white-space: nowrap",
  "overflow: hidden",
  "text-overflow: ellipsis",
  "max-width: calc(100% - 4px)",
  "width: fit-content",
  "pointer-events: auto",
  "cursor: pointer",
].join(";");

// A dot at the bottom of the waveform (chapter labels sit at the top), centered on the
// 2px marker line. Region content is positioned against the line's padding box.
const PIN_STYLE = [
  "position: absolute",
  "bottom: 1px",
  "left: -6px",
  "width: 10px",
  "height: 10px",
  "border-radius: 50%",
  "box-shadow: 0 0 0 1.5px var(--surface), 0 1px 2px rgb(0 0 0 / 0.25)",
  "pointer-events: auto",
  "cursor: pointer",
].join(";");

function cssVar(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

type Regions = ReturnType<typeof RegionsPlugin.create>;
type Region = ReturnType<Regions["addRegion"]>;

/** Replace the previous chapter bands with new ones; returns the regions it added. */
function drawChapters(regions: Regions, chapters: readonly Chapter[], previous: readonly Region[]): Region[] {
  previous.forEach((r) => r.remove());
  const out: Region[] = [];
  chapters.forEach((c, i) => {
    if (!(c.end > c.start)) return;
    // Wavesurfer renders inside a shadow root, so page CSS doesn't reach these.
    const label = document.createElement("div");
    label.style.cssText = LABEL_STYLE;
    label.textContent = c.title;
    label.title = c.title;
    label.dir = "auto";
    label.addEventListener("click", (e) => {
      e.stopPropagation();
      usePlayer.getState().seek(c.start);
    });
    const region = regions.addRegion({
      start: c.start,
      end: c.end,
      color: CHAPTER_TINTS[i % CHAPTER_TINTS.length],
      content: label,
      drag: false,
      resize: false,
    });
    // Let clicks on the band reach the waveform (seek); only the label is clickable.
    if (region.element) region.element.style.pointerEvents = "none";
    out.push(region);
  });
  return out;
}

/** Replace the previous note pins with new ones; returns the regions it added. */
function drawNotes(regions: Regions, notes: readonly Note[], previous: readonly Region[]): Region[] {
  previous.forEach((r) => r.remove());
  return notes.map((n) => {
    const color = noteColor(n.color);
    const pin = document.createElement("div");
    pin.style.cssText = `${PIN_STYLE};background:${color}`;
    pin.title = `${formatTimeRange(n.start, n.end)} · ${notePreview(n.body, 90)}`;
    // Keep wavesurfer's drag-to-seek from starting under the pin.
    pin.addEventListener("pointerdown", (e) => e.stopPropagation());
    pin.addEventListener("click", (e) => {
      e.stopPropagation();
      goToNote(n);
    });
    const region = regions.addRegion({
      start: n.start,
      end: n.start,
      color: `color-mix(in srgb, ${color} 60%, transparent)`,
      content: pin,
      drag: false,
      resize: false,
      avoidOverlapping: false,
    });
    if (region.element) region.element.style.pointerEvents = "none";
    return region;
  });
}

/**
 * Waveform drawn from backend peaks (wavesurfer never fetches or decodes the file),
 * bound to the shared <audio> element so clicks/drags seek it directly.
 */
export function Waveform({
  media,
  data,
  chapters,
  notes,
}: {
  media: HTMLAudioElement | null;
  data: WaveformData;
  chapters: readonly Chapter[];
  notes: readonly Note[];
}) {
  const container = useRef<HTMLDivElement>(null);
  const ws = useRef<WaveSurfer | null>(null);
  const regions = useRef<Regions | null>(null);
  const chapterRegions = useRef<Region[]>([]);
  const noteRegions = useRef<Region[]>([]);
  const chaptersRef = useRef(chapters);
  chaptersRef.current = chapters;
  const notesRef = useRef(notes);
  notesRef.current = notes;
  const dark = useApp((s) => s.dark);

  useEffect(() => {
    if (!media || !container.current) return;
    const plugin = RegionsPlugin.create();
    const instance = WaveSurfer.create({
      container: container.current,
      media,
      peaks: [data.peaks],
      duration: data.durationSec,
      height: 44,
      barWidth: 2,
      barGap: 1,
      barRadius: 2,
      cursorWidth: 2,
      normalize: true,
      dragToSeek: true,
      hideScrollbar: true,
      autoScroll: false,
      waveColor: cssVar("--wave"),
      progressColor: cssVar("--wave-progress"),
      cursorColor: cssVar("--accent"),
      plugins: [plugin],
    });
    ws.current = instance;
    regions.current = plugin;
    instance.on("interaction", (t) => usePlayer.setState({ time: t }));
    const draw = () => {
      chapterRegions.current = drawChapters(plugin, chaptersRef.current, chapterRegions.current);
      noteRegions.current = drawNotes(plugin, notesRef.current, noteRegions.current);
    };
    instance.on("decode", draw);
    instance.on("error", (e) => console.warn("waveform:", e));
    return () => {
      instance.destroy();
      ws.current = null;
      regions.current = null;
      chapterRegions.current = [];
      noteRegions.current = [];
    };
  }, [media, data]);

  // Redraw chapter bands when the analysis changes.
  useEffect(() => {
    if (regions.current && ws.current && ws.current.getDuration() > 0)
      chapterRegions.current = drawChapters(regions.current, chapters, chapterRegions.current);
  }, [chapters]);

  // Redraw note pins when notes are added, moved, recolored or deleted.
  useEffect(() => {
    if (regions.current && ws.current && ws.current.getDuration() > 0)
      noteRegions.current = drawNotes(regions.current, notes, noteRegions.current);
  }, [notes]);

  // Recolor on theme change.
  useEffect(() => {
    const id = requestAnimationFrame(() =>
      ws.current?.setOptions({
        waveColor: cssVar("--wave"),
        progressColor: cssVar("--wave-progress"),
        cursorColor: cssVar("--accent"),
      }),
    );
    return () => cancelAnimationFrame(id);
  }, [dark]);

  // Mouse-only affordance; keyboard users seek with the player controls and arrow keys.
  return <div ref={container} className="w-full" aria-hidden="true" />;
}
