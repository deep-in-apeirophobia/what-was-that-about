import type { ReactNode } from "react";
import clsx from "clsx";
import {
  AudioLines,
  Boxes,
  CircleHelp,
  Library,
  MessagesSquare,
  PanelLeftClose,
  PanelLeftOpen,
  Plus,
  Search,
  Settings,
  Tags,
} from "lucide-react";
import { useApp, type Route } from "../store/app";
import { useLibrary } from "../store/library";
import { hasReadyModel, useModels } from "../store/models";
import { isTauri } from "../lib/api";
import { MOD, pickAudioFiles } from "../lib/platform";
import { attempt } from "../store/toasts";
import { IconButton } from "./ui/Button";
import { Kbd } from "./ui/misc";
import { StatusDot, overallProgress } from "./RecordingStatus";

export async function importViaDialog(): Promise<void> {
  const paths = await attempt("Couldn't open the file picker", () => pickAudioFiles());
  if (paths?.length) await useLibrary.getState().importPaths(paths);
}

function NavItem({
  icon,
  label,
  route,
  collapsed,
  badge,
}: {
  icon: ReactNode;
  label: string;
  route: Route;
  collapsed: boolean;
  badge?: ReactNode;
}) {
  const current = useApp((s) => s.route);
  const navigate = useApp((s) => s.navigate);
  const active = current.name === route.name;
  return (
    <button
      type="button"
      onClick={() => navigate(route)}
      aria-current={active ? "page" : undefined}
      title={collapsed ? label : undefined}
      aria-label={collapsed ? label : undefined}
      className={clsx(
        "group flex h-8 w-full items-center gap-2.5 rounded-lg text-[13px] font-medium transition-colors",
        collapsed ? "justify-center" : "px-2.5",
        active ? "bg-hover text-fg" : "text-muted hover:bg-hover hover:text-fg",
      )}
    >
      <span className={clsx("shrink-0 [&>svg]:size-4", active ? "text-accent" : "text-faint group-hover:text-muted")}>{icon}</span>
      {!collapsed && <span className="flex-1 truncate text-left">{label}</span>}
      {!collapsed && badge}
    </button>
  );
}

function RecentList() {
  const recordings = useLibrary((s) => s.recordings);
  const progress = useLibrary((s) => s.progress);
  const route = useApp((s) => s.route);
  const openRecording = useApp((s) => s.openRecording);
  const recent = recordings.slice(0, 12);
  if (!recent.length) return null;
  return (
    <div className="mt-5 min-h-0 flex-1 overflow-y-auto">
      <div className="px-2.5 pb-1.5 text-[11px] font-medium uppercase tracking-wide text-faint">Recent</div>
      <ul className="space-y-px">
        {recent.map((r) => {
          const active = route.name === "recording" && route.id === r.id;
          const pct = overallProgress(progress[r.id]);
          return (
            <li key={r.id}>
              <button
                type="button"
                onClick={() => openRecording(r.id)}
                aria-current={active ? "page" : undefined}
                className={clsx(
                  "flex h-8 w-full items-center gap-2.5 rounded-lg px-2.5 text-[13px] transition-colors",
                  active ? "bg-hover text-fg" : "text-muted hover:bg-hover hover:text-fg",
                )}
              >
                <StatusDot recording={r} />
                <span className="min-w-0 flex-1 truncate text-left" dir="auto">
                  {r.title}
                </span>
                {pct != null && <span className="shrink-0 text-[11px] tabular-nums text-faint">{Math.round(pct * 100)}%</span>}
              </button>
            </li>
          );
        })}
      </ul>
    </div>
  );
}

export function Sidebar() {
  const collapsed = useApp((s) => s.sidebarCollapsed);
  const toggle = useApp((s) => s.toggleSidebar);
  const setSearchOpen = useApp((s) => s.setSearchOpen);
  const setShortcutsOpen = useApp((s) => s.setShortcutsOpen);
  const models = useModels((s) => s.models);
  const modelsLoaded = useModels((s) => s.loaded);
  const installing = useModels((s) => Object.keys(s.progress).length > 0);

  const modelsBadge = installing ? (
    <span className="size-1.5 animate-pulse rounded-full bg-accent" aria-label="Installing" />
  ) : modelsLoaded && !hasReadyModel(models) ? (
    <span className="rounded bg-warn-soft px-1.5 text-[10.5px] font-semibold text-warn">Setup</span>
  ) : null;

  return (
    <aside
      className={clsx(
        "flex h-full shrink-0 flex-col border-r border-line bg-sunken/60 py-3 transition-[width] duration-200",
        collapsed ? "w-[60px] px-2" : "w-[248px] px-3",
      )}
    >
      <div className={clsx("mb-4 flex items-center", collapsed ? "flex-col gap-2" : "gap-2 px-1")}>
        <div className="flex size-7 shrink-0 items-center justify-center rounded-lg bg-accent text-accent-fg shadow-sm">
          <AudioLines className="size-4" />
        </div>
        {!collapsed && (
          <div className="min-w-0 flex-1 leading-tight">
            <div className="truncate text-[13.5px] font-semibold">What Was That About</div>
            {!isTauri() && <div className="text-[11px] text-faint">Browser preview (mock)</div>}
          </div>
        )}
        <IconButton label={collapsed ? "Expand sidebar" : "Collapse sidebar"} onClick={toggle} size="xs">
          {collapsed ? <PanelLeftOpen className="size-4" /> : <PanelLeftClose className="size-4" />}
        </IconButton>
      </div>

      <button
        type="button"
        onClick={() => void importViaDialog()}
        title={collapsed ? "Import audio" : undefined}
        aria-label={collapsed ? "Import audio" : undefined}
        className={clsx(
          "flex h-9 items-center justify-center gap-2 rounded-lg bg-accent text-[13px] font-medium text-accent-fg shadow-sm transition-colors hover:bg-accent-hover",
          collapsed ? "w-full" : "w-full px-3",
        )}
      >
        <Plus className="size-4" />
        {!collapsed && "Import audio"}
      </button>

      <button
        type="button"
        onClick={() => setSearchOpen(true)}
        title={collapsed ? `Search (${MOD}+K)` : undefined}
        aria-label={collapsed ? "Search" : undefined}
        className={clsx(
          "mt-2 flex h-8 items-center gap-2 rounded-lg border border-line bg-surface text-[13px] text-faint transition-colors hover:border-line-strong hover:text-muted",
          collapsed ? "justify-center" : "px-2.5",
        )}
      >
        <Search className="size-3.5 shrink-0" />
        {!collapsed && (
          <>
            <span className="flex-1 text-left">Search everything…</span>
            <Kbd>{MOD}K</Kbd>
          </>
        )}
      </button>

      <nav className="mt-4 space-y-px" aria-label="Main">
        <NavItem icon={<Library />} label="Library" route={{ name: "library" }} collapsed={collapsed} />
        <NavItem icon={<Tags />} label="Topics" route={{ name: "topics" }} collapsed={collapsed} />
        <NavItem icon={<MessagesSquare />} label="Ask library" route={{ name: "ask" }} collapsed={collapsed} />
        <NavItem icon={<Boxes />} label="Models" route={{ name: "models" }} collapsed={collapsed} badge={modelsBadge} />
        <NavItem icon={<Settings />} label="Settings" route={{ name: "settings" }} collapsed={collapsed} />
      </nav>

      {!collapsed ? <RecentList /> : <div className="flex-1" />}

      <div className={clsx("mt-2 flex", collapsed ? "justify-center" : "justify-end")}>
        <IconButton label="Keyboard shortcuts (?)" onClick={() => setShortcutsOpen(true)} size="sm">
          <CircleHelp className="size-4" />
        </IconButton>
      </div>
    </aside>
  );
}
