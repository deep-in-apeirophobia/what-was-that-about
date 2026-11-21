import { create } from "zustand";

export type Route =
  | { name: "library" }
  | { name: "recording"; id: string }
  | { name: "topics"; tag?: string }
  | { name: "ask" }
  | { name: "models" }
  | { name: "settings" };

export interface SeekRequest {
  recordingId: string;
  segmentId?: number;
  time?: number;
  play?: boolean;
  nonce: number;
}

const LS = {
  sidebar: "wwta.sidebarCollapsed",
  onboarding: "wwta.onboardingDismissed",
};

function readBool(key: string): boolean {
  try {
    return localStorage.getItem(key) === "1";
  } catch {
    return false;
  }
}
function writeBool(key: string, v: boolean): void {
  try {
    localStorage.setItem(key, v ? "1" : "0");
  } catch {
    /* storage unavailable */
  }
}

interface AppState {
  route: Route;
  sidebarCollapsed: boolean;
  searchOpen: boolean;
  shortcutsOpen: boolean;
  onboardingDismissed: boolean;
  /** Seek to apply once the target recording's view is ready. */
  pendingSeek: SeekRequest | null;
  dragging: boolean;
  /** Resolved theme (settings.theme with "system" applied). */
  dark: boolean;

  navigate: (route: Route) => void;
  openRecording: (id: string, seek?: Omit<SeekRequest, "recordingId" | "nonce">) => void;
  takeSeek: (recordingId: string) => SeekRequest | null;
  toggleSidebar: () => void;
  setSearchOpen: (open: boolean) => void;
  setShortcutsOpen: (open: boolean) => void;
  dismissOnboarding: () => void;
  reopenOnboarding: () => void;
  setDragging: (v: boolean) => void;
  setDark: (v: boolean) => void;
}

let nonce = 0;

export const useApp = create<AppState>((set, get) => ({
  route: { name: "library" },
  sidebarCollapsed: readBool(LS.sidebar),
  searchOpen: false,
  shortcutsOpen: false,
  onboardingDismissed: readBool(LS.onboarding),
  pendingSeek: null,
  dragging: false,
  dark: false,

  navigate: (route) => set({ route }),
  openRecording: (id, seek) =>
    set({
      route: { name: "recording", id },
      pendingSeek: seek ? { ...seek, recordingId: id, nonce: ++nonce } : get().pendingSeek,
    }),
  takeSeek: (recordingId) => {
    const s = get().pendingSeek;
    if (!s || s.recordingId !== recordingId) return null;
    set({ pendingSeek: null });
    return s;
  },
  toggleSidebar: () =>
    set((s) => {
      writeBool(LS.sidebar, !s.sidebarCollapsed);
      return { sidebarCollapsed: !s.sidebarCollapsed };
    }),
  setSearchOpen: (searchOpen) => set({ searchOpen }),
  setShortcutsOpen: (shortcutsOpen) => set({ shortcutsOpen }),
  dismissOnboarding: () => {
    writeBool(LS.onboarding, true);
    set({ onboardingDismissed: true });
  },
  reopenOnboarding: () => {
    writeBool(LS.onboarding, false);
    set({ onboardingDismissed: false });
  },
  setDragging: (dragging) => set({ dragging }),
  setDark: (dark) => set({ dark }),
}));
