import { create } from "zustand";
import { toast } from "./toasts";

// One <audio> element (owned by the Player component) drives everything that
// depends on playback time. Components subscribe with narrow selectors (e.g. the
// active segment index) so a time tick only re-renders what actually changed.

const LS_RATE = "wwta.playbackRate";
const LS_VOLUME = "wwta.volume";
const LS_FOLLOW = "wwta.follow";

function readNumber(key: string, fallback: number): number {
  try {
    const v = Number(localStorage.getItem(key));
    return localStorage.getItem(key) !== null && Number.isFinite(v) ? v : fallback;
  } catch {
    return fallback;
  }
}
function write(key: string, v: string): void {
  try {
    localStorage.setItem(key, v);
  } catch {
    /* ignore */
  }
}

export const RATES = [0.75, 1, 1.25, 1.5, 1.75, 2] as const;

interface PlayerState {
  el: HTMLAudioElement | null;
  time: number;
  duration: number;
  playing: boolean;
  rate: number;
  volume: number;
  muted: boolean;
  follow: boolean;
  attach: (el: HTMLAudioElement | null) => void;
  seek: (time: number, play?: boolean) => void;
  play: () => void;
  pause: () => void;
  toggle: () => void;
  skip: (delta: number) => void;
  setRate: (rate: number) => void;
  setVolume: (volume: number) => void;
  toggleMute: () => void;
  setFollow: (follow: boolean) => void;
  reset: () => void;
}

function clampTime(t: number, duration: number): number {
  if (!Number.isFinite(t)) return 0;
  const max = duration > 0 ? duration : Number.POSITIVE_INFINITY;
  return Math.max(0, Math.min(t, max));
}

export const usePlayer = create<PlayerState>((set, get) => ({
  el: null,
  time: 0,
  duration: 0,
  playing: false,
  rate: readNumber(LS_RATE, 1),
  volume: readNumber(LS_VOLUME, 1),
  muted: false,
  follow: readNumber(LS_FOLLOW, 1) === 1,

  attach: (el) => {
    if (el) {
      const { rate, volume, muted } = get();
      el.defaultPlaybackRate = rate;
      el.playbackRate = rate;
      el.volume = volume;
      el.muted = muted;
    }
    set({ el });
  },

  seek: (time, play) => {
    const { el, duration } = get();
    const t = clampTime(time, duration || el?.duration || 0);
    if (el) {
      try {
        el.currentTime = t;
      } catch {
        /* not seekable yet; the time is remembered below */
      }
    }
    set({ time: t });
    if (play) get().play();
  },

  play: () => {
    const { el } = get();
    // currentSrc is filled in asynchronously after `src` changes, so check both.
    if (!el || !(el.currentSrc || el.getAttribute("src"))) {
      toast.info("Audio isn't available", "The transcript still works — playback needs a readable audio file.");
      return;
    }
    el.play().catch((e: unknown) => {
      if (e instanceof DOMException && e.name === "AbortError") return;
      toast.error("Couldn't start playback", e);
    });
  },

  pause: () => get().el?.pause(),

  toggle: () => {
    const { el } = get();
    if (el && !el.paused) el.pause();
    else get().play();
  },

  skip: (delta) => {
    const { el, time } = get();
    get().seek((el ? el.currentTime : time) + delta);
  },

  setRate: (rate) => {
    const { el } = get();
    if (el) {
      el.defaultPlaybackRate = rate;
      el.playbackRate = rate;
    }
    write(LS_RATE, String(rate));
    set({ rate });
  },

  setVolume: (volume) => {
    const v = Math.max(0, Math.min(1, volume));
    const { el } = get();
    if (el) {
      el.volume = v;
      if (v > 0 && el.muted) el.muted = false;
    }
    write(LS_VOLUME, String(v));
    set({ volume: v, muted: v > 0 ? false : get().muted });
  },

  toggleMute: () => {
    const { el, muted } = get();
    if (el) el.muted = !muted;
    set({ muted: !muted });
  },

  setFollow: (follow) => {
    write(LS_FOLLOW, follow ? "1" : "0");
    set({ follow });
  },

  reset: () => set({ time: 0, duration: 0, playing: false }),
}));
