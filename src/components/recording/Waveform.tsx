import { useEffect, useRef } from "react";
import WaveSurfer from "wavesurfer.js";
import RegionsPlugin from "wavesurfer.js/dist/plugins/regions.esm.js";
import type { Chapter, Waveform as WaveformData } from "../../lib/types";
import { usePlayer } from "../../store/player";
import { useApp } from "../../store/app";

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

function cssVar(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

type Regions = ReturnType<typeof RegionsPlugin.create>;

function drawChapters(regions: Regions, chapters: readonly Chapter[]): void {
  regions.clearRegions();
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
}: {
  media: HTMLAudioElement | null;
  data: WaveformData;
  chapters: readonly Chapter[];
}) {
  const container = useRef<HTMLDivElement>(null);
  const ws = useRef<WaveSurfer | null>(null);
  const regions = useRef<Regions | null>(null);
  const chaptersRef = useRef(chapters);
  chaptersRef.current = chapters;
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
    const draw = () => drawChapters(plugin, chaptersRef.current);
    instance.on("decode", draw);
    instance.on("error", (e) => console.warn("waveform:", e));
    return () => {
      instance.destroy();
      ws.current = null;
      regions.current = null;
    };
  }, [media, data]);

  // Redraw chapter bands when the analysis changes.
  useEffect(() => {
    if (regions.current && ws.current && ws.current.getDuration() > 0) drawChapters(regions.current, chapters);
  }, [chapters]);

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
