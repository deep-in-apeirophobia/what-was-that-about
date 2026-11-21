import { Sidebar } from "./components/Sidebar";
import { Toasts } from "./components/Toasts";
import { DropOverlay } from "./components/DropOverlay";
import { SearchPalette } from "./components/SearchPalette";
import { ShortcutsHelp } from "./components/ShortcutsHelp";
import { Onboarding } from "./components/Onboarding";
import { LibraryView } from "./views/LibraryView";
import { RecordingView } from "./views/RecordingView";
import { TopicsView } from "./views/TopicsView";
import { AskLibraryView } from "./views/AskLibraryView";
import { ModelsView } from "./views/ModelsView";
import { SettingsView } from "./views/SettingsView";
import { useApp } from "./store/app";
import {
  useBackendEvents,
  useFileDrop,
  useGlobalShortcuts,
  useInitialLoad,
  useTheme,
} from "./hooks/useAppEffects";

function CurrentView() {
  const route = useApp((s) => s.route);
  switch (route.name) {
    case "library":
      return <LibraryView />;
    case "recording":
      return <RecordingView key={route.id} id={route.id} />;
    case "topics":
      return <TopicsView initialTag={route.tag} />;
    case "ask":
      return <AskLibraryView />;
    case "models":
      return <ModelsView />;
    case "settings":
      return <SettingsView />;
  }
}

export default function App() {
  useInitialLoad();
  useBackendEvents();
  useTheme();
  useFileDrop();
  useGlobalShortcuts();

  return (
    <div className="flex h-full bg-canvas text-fg">
      <Sidebar />
      <main className="flex min-w-0 flex-1 flex-col">
        <CurrentView />
      </main>
      <SearchPalette />
      <ShortcutsHelp />
      <Onboarding />
      <DropOverlay />
      <Toasts />
    </div>
  );
}
