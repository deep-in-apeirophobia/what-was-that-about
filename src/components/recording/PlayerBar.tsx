import { useCallback, useEffect, useRef, useState } from "react";
import clsx from "clsx";
import {
  AlertCircle,
  Crosshair,
  Pause,
  Play,
  RotateCcw,
  RotateCw,
  SkipBack,
  SkipForward,
  Volume1,
  Volume2,
  VolumeX,
} from "lucide-react";
import { api } from "../../lib/api";
import { errorMessage } from "../../lib/errors";
import { formatTime } from "../../lib/format";
import { mediaUrl } from "../../lib/platform";
import { nextSegmentTime, previousSegmentTime } from "../../lib/segments";
import type { Chapter, Recording, Segment, Waveform as WaveformData } from "../../lib/types";
import { RATES, usePlayer } from "../../store/player";
import { IconButton } from "../ui/Button";
import { Menu } from "../ui/Menu";
import { Spinner } from "../ui/misc";
import { Waveform } from "./Waveform";

type Source = { url: string | null; kind: "original" | "fallback"; error: string | null; preparing: boolean };

/** Owns the single <audio> element and keeps the player store in sync with it. */
function useAudioElement(recording: Recording) {
  const ref = useRef<HTMLAudioElement>(null);
  const [el, setEl] = useState<HTMLAudioElement | null>(null);
  const [source, setSource] = useState<Source>(() => ({
    url: mediaUrl(recording.audioPath),
    kind: "original",
    error: null,
    preparing: false,
  }));
  const resumeAt = useRef<number | null>(null);
  const sourceRef = useRef(source);
  sourceRef.current = source;

  const tryFallback = useCallback(async () => {
    if (sourceRef.current.kind === "fallback" || sourceRef.current.preparing) {
      setSource((s) => ({ ...s, preparing: false, error: s.error ?? "This audio format can't be played in the app." }));
      return;
    }
    resumeAt.current = usePlayer.getState().time;
    setSource((s) => ({ ...s, preparing: true }));
    try {
      const path = await api.preparePlaybackWav(recording.id);
      const url = mediaUrl(path);
      if (!url) throw new Error("No playable file was produced.");
      setSource({ url, kind: "fallback", error: null, preparing: false });
    } catch (e) {
      setSource({ url: null, kind: "fallback", error: errorMessage(e), preparing: false });
    }
  }, [recording.id]);

  // New file (e.g. after in-browser decoding) → start over with the original source.
  useEffect(() => {
    const url = mediaUrl(recording.audioPath);
    setSource({ url, kind: "original", error: null, preparing: false });
    if (!url) void tryFallback();
  }, [recording.audioPath, tryFallback]);

  useEffect(() => {
    const audio = ref.current;
    if (!audio) return;
    setEl(audio);
    const store = usePlayer.getState();
    store.attach(audio);
    store.reset();
    const sync = () => usePlayer.setState({ time: audio.currentTime });
    const onMeta = () => {
      if (Number.isFinite(audio.duration)) usePlayer.setState({ duration: audio.duration });
      if (resumeAt.current != null) {
        audio.currentTime = resumeAt.current;
        resumeAt.current = null;
      }
    };
    const onPlay = () => usePlayer.setState({ playing: true });
    const onPause = () => usePlayer.setState({ playing: false, time: audio.currentTime });
    const onRate = () => usePlayer.setState({ rate: audio.playbackRate });
    const onVolume = () => usePlayer.setState({ volume: audio.volume, muted: audio.muted });
    const onError = () => {
      if (!audio.getAttribute("src")) return;
      void tryFallback();
    };
    audio.addEventListener("timeupdate", sync);
    audio.addEventListener("seeked", sync);
    audio.addEventListener("loadedmetadata", onMeta);
    audio.addEventListener("durationchange", onMeta);
    audio.addEventListener("play", onPlay);
    audio.addEventListener("pause", onPause);
    audio.addEventListener("ended", onPause);
    audio.addEventListener("ratechange", onRate);
    audio.addEventListener("volumechange", onVolume);
    audio.addEventListener("error", onError);
    return () => {
      audio.pause();
      audio.removeEventListener("timeupdate", sync);
      audio.removeEventListener("seeked", sync);
      audio.removeEventListener("loadedmetadata", onMeta);
      audio.removeEventListener("durationchange", onMeta);
      audio.removeEventListener("play", onPlay);
      audio.removeEventListener("pause", onPause);
      audio.removeEventListener("ended", onPause);
      audio.removeEventListener("ratechange", onRate);
      audio.removeEventListener("volumechange", onVolume);
      audio.removeEventListener("error", onError);
      if (usePlayer.getState().el === audio) usePlayer.getState().attach(null);
      usePlayer.getState().reset();
    };
  }, [tryFallback]);

  // Smooth time updates while playing (timeupdate only fires ~4×/s; karaoke needs more).
  const playing = usePlayer((s) => s.playing);
  useEffect(() => {
    const audio = ref.current;
    if (!playing || !audio) return;
    let raf = 0;
    const tick = () => {
      usePlayer.setState({ time: audio.currentTime });
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [playing]);

  return { ref, el, source };
}

function TimeDisplay({ total }: { total: number }) {
  const current = usePlayer((s) => formatTime(s.time, total >= 3600));
  return (
    <span className="text-[12px] text-muted tabular-nums">
      <span className="text-fg">{current}</span>
      <span className="mx-1 text-faint">/</span>
      {formatTime(total)}
    </span>
  );
}

function VolumeControl() {
  const volume = usePlayer((s) => s.volume);
  const muted = usePlayer((s) => s.muted);
  const setVolume = usePlayer((s) => s.setVolume);
  const toggleMute = usePlayer((s) => s.toggleMute);
  const Icon = muted || volume === 0 ? VolumeX : volume < 0.5 ? Volume1 : Volume2;
  return (
    <div className="group flex items-center gap-1">
      <IconButton label={muted ? "Unmute" : "Mute"} onClick={toggleMute}>
        <Icon className="size-4" />
      </IconButton>
      <input
        type="range"
        min={0}
        max={1}
        step={0.05}
        value={muted ? 0 : volume}
        onChange={(e) => setVolume(Number(e.target.value))}
        aria-label="Volume"
        className="h-1 w-20 cursor-pointer accent-[var(--accent)]"
      />
    </div>
  );
}

export function PlayerBar({
  recording,
  segments,
  waveform,
  waveformError,
  chapters,
}: {
  recording: Recording;
  segments: readonly Segment[];
  waveform: WaveformData | null;
  waveformError: string | null;
  chapters: readonly Chapter[];
}) {
  const { ref, el, source } = useAudioElement(recording);
  const playing = usePlayer((s) => s.playing);
  const duration = usePlayer((s) => s.duration);
  const rate = usePlayer((s) => s.rate);
  const follow = usePlayer((s) => s.follow);
  const { toggle, skip, seek, setRate, setFollow } = usePlayer.getState();
  const total = duration || recording.durationSec || waveform?.durationSec || 0;

  const prevSentence = () => seek(previousSegmentTime(segments, usePlayer.getState().time));
  const nextSentence = () => {
    const t = nextSegmentTime(segments, usePlayer.getState().time);
    if (t != null) seek(t);
  };

  return (
    <div className="shrink-0 border-t border-line bg-surface px-5 pt-2.5 pb-3">
      <audio ref={ref} src={source.url ?? undefined} preload="metadata" />
      <div className="relative min-h-[44px]">
        {waveform && el ? (
          <Waveform media={el} data={waveform} chapters={chapters} />
        ) : (
          <div className="flex h-[44px] items-center justify-center text-[12px] text-faint">
            {waveformError ? "Waveform unavailable" : recording.status === "needsDecode" ? "Waveform appears after decoding" : <Spinner className="size-3.5" />}
          </div>
        )}
      </div>

      <div className="mt-2 flex items-center gap-3">
        <div className="flex flex-1 items-center gap-2 min-w-0">
          <TimeDisplay total={total} />
          {recording.status === "needsDecode" && !source.url ? (
            <span className="truncate text-[12px] text-faint">Playback starts working once the file is decoded.</span>
          ) : (
            (source.error || source.preparing) && (
              <span
                className={clsx("flex min-w-0 items-center gap-1 text-[12px]", source.error ? "text-danger" : "text-muted")}
                role={source.error ? "alert" : "status"}
                title={source.error ?? undefined}
              >
                {source.error ? <AlertCircle className="size-3.5 shrink-0" /> : <Spinner className="size-3.5" />}
                <span className="truncate">{source.error ? `Can't play audio: ${source.error}` : "Preparing a playable copy…"}</span>
              </span>
            )
          )}
        </div>

        <div className="flex items-center gap-1">
          <IconButton label="Previous sentence (Shift+←)" onClick={prevSentence} disabled={!segments.length}>
            <SkipBack className="size-4" />
          </IconButton>
          <IconButton label="Back 5 seconds (←)" onClick={() => skip(-5)}>
            <RotateCcw className="size-4" />
          </IconButton>
          <IconButton label={playing ? "Pause (Space)" : "Play (Space)"} onClick={toggle} variant="primary" size="lg" disabled={!source.url}>
            {playing ? <Pause className="size-4.5" fill="currentColor" /> : <Play className="size-4.5 translate-x-px" fill="currentColor" />}
          </IconButton>
          <IconButton label="Forward 5 seconds (→)" onClick={() => skip(5)}>
            <RotateCw className="size-4" />
          </IconButton>
          <IconButton label="Next sentence (Shift+→)" onClick={nextSentence} disabled={!segments.length}>
            <SkipForward className="size-4" />
          </IconButton>
        </div>

        <div className="flex flex-1 items-center justify-end gap-2">
          <Menu
            side="top"
            align="end"
            items={RATES.map((r) => ({ key: String(r), label: `${r}×`, checked: r === rate, onSelect: () => setRate(r) }))}
            trigger={(p) => (
              <button
                {...p}
                type="button"
                aria-label={`Playback speed ${rate}×`}
                title="Playback speed"
                className="h-7 min-w-11 rounded-md px-2 text-[12px] font-semibold text-muted tabular-nums hover:bg-hover hover:text-fg"
              >
                {rate}×
              </button>
            )}
          />
          <VolumeControl />
          <IconButton
            label={follow ? "Follow along: on" : "Follow along: off"}
            active={follow}
            onClick={() => setFollow(!follow)}
          >
            <Crosshair className="size-4" />
          </IconButton>
        </div>
      </div>
    </div>
  );
}
