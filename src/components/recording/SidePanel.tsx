import clsx from "clsx";
import { useRecording, type PanelTab } from "../../store/recording";
import { AskTab } from "./AskTab";
import { DocsTab } from "./DocsTab";
import { OverviewTab } from "./OverviewTab";
import { RelatedTab } from "./RelatedTab";
import { TopicsTab } from "./TopicsTab";

const TABS: { id: PanelTab; label: string }[] = [
  { id: "overview", label: "Overview" },
  { id: "topics", label: "Topics" },
  { id: "ask", label: "Ask" },
  { id: "docs", label: "Docs" },
  { id: "related", label: "Related" },
];

export function SidePanel() {
  const tab = useRecording((s) => s.tab);
  const setTab = useRecording((s) => s.setTab);
  const docCount = useRecording((s) => s.detail?.docs.length ?? 0);
  const chatCount = useRecording((s) => s.detail?.chat.length ?? 0);
  const hasRelated = useRecording((s) => !!s.related);

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key !== "ArrowRight" && e.key !== "ArrowLeft") return;
    e.preventDefault();
    e.stopPropagation();
    const i = TABS.findIndex((t) => t.id === tab);
    const next = TABS[(i + (e.key === "ArrowRight" ? 1 : -1) + TABS.length) % TABS.length];
    setTab(next.id);
    document.getElementById(`panel-tab-${next.id}`)?.focus();
  };

  return (
    <aside className="flex w-[400px] shrink-0 flex-col border-l border-line bg-surface/60 xl:w-[440px]" aria-label="Recording details">
      <div role="tablist" aria-label="Panels" className="flex h-10 shrink-0 items-stretch gap-0.5 border-b border-line px-2" onKeyDown={onKeyDown}>
        {TABS.map((t) => {
          const selected = t.id === tab;
          const count = t.id === "docs" ? docCount : t.id === "ask" ? Math.ceil(chatCount / 2) : 0;
          return (
            <button
              key={t.id}
              id={`panel-tab-${t.id}`}
              type="button"
              role="tab"
              aria-selected={selected}
              aria-controls={`panel-${t.id}`}
              tabIndex={selected ? 0 : -1}
              onClick={() => setTab(t.id)}
              className={clsx(
                "relative flex items-center gap-1.5 px-2.5 text-[12.5px] font-medium transition-colors",
                selected ? "text-fg" : "text-muted hover:text-fg",
              )}
            >
              {t.label}
              {count > 0 && <span className="rounded bg-sunken px-1 text-[10.5px] text-faint tabular-nums">{count}</span>}
              {t.id === "related" && hasRelated && !selected && <span className="size-1.5 rounded-full bg-accent" />}
              {selected && <span className="absolute inset-x-2 -bottom-px h-0.5 rounded-full bg-accent" />}
            </button>
          );
        })}
      </div>
      <div
        id={`panel-${tab}`}
        role="tabpanel"
        aria-labelledby={`panel-tab-${tab}`}
        className={clsx("min-h-0 flex-1", tab === "ask" ? "flex flex-col" : "overflow-y-auto")}
      >
        {tab === "overview" && <OverviewTab />}
        {tab === "topics" && <TopicsTab />}
        {tab === "ask" && <AskTab />}
        {tab === "docs" && <DocsTab />}
        {tab === "related" && <RelatedTab />}
      </div>
    </aside>
  );
}
