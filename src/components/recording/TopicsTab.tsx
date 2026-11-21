import clsx from "clsx";
import { Play, Tags } from "lucide-react";
import { formatTime } from "../../lib/format";
import { indexOfSegmentId } from "../../lib/segments";
import { useRecording } from "../../store/recording";
import { hasApiKey, useSettings } from "../../store/settings";
import { EmptyState } from "../ui/misc";
import { NoKeyCta, PanelSection } from "./AiCta";
import { seekToSegment } from "./actions";

export function TopicsTab() {
  const detail = useRecording((s) => s.detail)!;
  const selectedTag = useRecording((s) => s.selectedTag);
  const selectTag = useRecording((s) => s.selectTag);
  const settings = useSettings((s) => s.settings);
  const tags = detail.analysis?.tags ?? [];

  if (!detail.analysis) {
    if (!hasApiKey(settings)) return <NoKeyCta what="discover the topics discussed" />;
    return (
      <EmptyState icon={<Tags />} title="No topics yet">
        Run “Analyze with AI” from the Overview tab to tag the topics discussed in this recording.
      </EmptyState>
    );
  }
  if (!tags.length) return <EmptyState icon={<Tags />} title="No topics were found" />;

  const tag = tags.find((t) => t.name === selectedTag);
  const sorted = [...tags].sort((a, b) => b.segments.length - a.segments.length);

  return (
    <div>
      <PanelSection title="Topics">
        <div className="flex flex-wrap gap-1.5">
          {sorted.map((t) => {
            const on = t.name === selectedTag;
            return (
              <button
                key={t.name}
                type="button"
                aria-pressed={on}
                onClick={() => selectTag(on ? null : t.name)}
                title={t.description}
                className={clsx(
                  "inline-flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-[12.5px] transition-colors",
                  on ? "border-ok bg-ok-soft text-ok" : "border-line bg-surface text-muted hover:border-line-strong hover:text-fg",
                )}
              >
                <span dir="auto">{t.name}</span>
                <span className={clsx("text-[11px] tabular-nums", on ? "text-ok" : "text-faint")}>{t.segments.length}</span>
              </button>
            );
          })}
        </div>
        {!tag && <p className="mt-3 text-[12px] text-faint">Pick a topic to highlight where it's discussed in the transcript.</p>}
      </PanelSection>

      {tag && (
        <PanelSection title={`Where “${tag.name}” comes up`}>
          {tag.description && (
            <p className="mb-2.5 text-[12.5px] text-muted" dir="auto">
              {tag.description}
            </p>
          )}
          <ul className="space-y-1">
            {tag.segments.map((id) => {
              const i = indexOfSegmentId(detail.segments, id);
              const seg = detail.segments[i];
              if (!seg) return null;
              return (
                <li key={id}>
                  <button
                    type="button"
                    onClick={() => seekToSegment(id, true)}
                    className="group flex w-full items-start gap-2.5 rounded-lg px-2 py-1.5 text-left hover:bg-hover"
                  >
                    <span className="mt-0.5 flex w-12 shrink-0 items-center gap-1 text-[12px] text-accent tabular-nums">
                      <Play className="size-3 opacity-60 group-hover:opacity-100" fill="currentColor" />
                      {formatTime(seg.start)}
                    </span>
                    <span className="min-w-0 flex-1 font-fa text-[13px] leading-relaxed" dir="auto">
                      {seg.text}
                    </span>
                  </button>
                </li>
              );
            })}
          </ul>
        </PanelSection>
      )}
    </div>
  );
}
