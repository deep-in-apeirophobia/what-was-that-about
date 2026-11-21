import { useState } from "react";
import clsx from "clsx";
import { CheckSquare, RefreshCw, Sparkles } from "lucide-react";
import { formatTime } from "../../lib/format";
import { findActiveIndex } from "../../lib/segments";
import type { Chapter } from "../../lib/types";
import { usePlayer } from "../../store/player";
import { useRecording } from "../../store/recording";
import { hasApiKey, useSettings } from "../../store/settings";
import { Button } from "../ui/Button";
import { Markdown } from "../ui/Markdown";
import { NoKeyCta, PanelSection } from "./AiCta";
import { analyzeRecording } from "./actions";

function ChapterList({ chapters }: { chapters: Chapter[] }) {
  const active = usePlayer((s) => findActiveIndex(chapters, s.time));
  return (
    <ol className="space-y-px">
      {chapters.map((c, i) => (
        <li key={`${c.startSegment}-${i}`}>
          <button
            type="button"
            onClick={() => usePlayer.getState().seek(c.start, true)}
            className={clsx(
              "flex w-full items-start gap-2.5 rounded-lg px-2 py-1.5 text-left transition-colors",
              i === active ? "bg-active-row" : "hover:bg-hover",
            )}
          >
            <span className={clsx("mt-px w-10 shrink-0 text-[12px] tabular-nums", i === active ? "font-medium text-accent" : "text-faint")}>
              {formatTime(c.start)}
            </span>
            <span className="min-w-0 flex-1">
              <span className="block text-[13px] font-medium" dir="auto">
                {c.title}
              </span>
              {c.summary && (
                <span className="mt-0.5 line-clamp-2 block text-[12px] text-muted" dir="auto">
                  {c.summary}
                </span>
              )}
            </span>
          </button>
        </li>
      ))}
    </ol>
  );
}

export function OverviewTab() {
  const detail = useRecording((s) => s.detail)!;
  const settings = useSettings((s) => s.settings);
  const [busy, setBusy] = useState(false);
  const { analysis, recording, segments } = detail;
  const keyOk = hasApiKey(settings);
  const canAnalyze = recording.status === "ready" && segments.length > 0;

  const run = async () => {
    setBusy(true);
    await analyzeRecording(recording.id);
    setBusy(false);
  };

  if (!analysis) {
    if (!keyOk) return <NoKeyCta what="get a summary, chapters and topics" />;
    return (
      <div className="mx-4 my-6 rounded-xl border border-dashed border-line-strong p-5 text-center">
        <div className="mx-auto flex size-10 items-center justify-center rounded-xl bg-accent-soft text-accent">
          <Sparkles className="size-5" />
        </div>
        <div className="mt-3 text-[13.5px] font-semibold">No AI overview yet</div>
        <p className="mx-auto mt-1 max-w-xs text-[12.5px] text-muted">
          {canAnalyze
            ? "Get a summary, key points, action items, chapters and topics for this recording."
            : "Once the transcript is ready you can get a summary, chapters and topics."}
        </p>
        <Button className="mt-4" variant="primary" icon={<Sparkles className="size-3.5" />} loading={busy} disabled={!canAnalyze} onClick={() => void run()}>
          {busy ? "Analyzing…" : "Analyze with AI"}
        </Button>
      </div>
    );
  }

  return (
    <div>
      <PanelSection
        title="Summary"
        action={
          keyOk && canAnalyze ? (
            <Button size="sm" variant="ghost" icon={<RefreshCw className="size-3" />} loading={busy} onClick={() => void run()}>
              Refresh
            </Button>
          ) : undefined
        }
      >
        {analysis.title && (
          <div className="mb-1.5 text-[14px] font-semibold" dir="auto">
            {analysis.title}
          </div>
        )}
        <Markdown content={analysis.summary} className="font-fa text-[13px] text-fg/90" />
      </PanelSection>

      {analysis.keyPoints.length > 0 && (
        <PanelSection title="Key points">
          <ul className="space-y-1.5">
            {analysis.keyPoints.map((p, i) => (
              <li key={i} className="flex gap-2 font-fa text-[13px] leading-relaxed" dir="auto">
                <span className="mt-[0.6em] size-1 shrink-0 rounded-full bg-accent" />
                <span>{p}</span>
              </li>
            ))}
          </ul>
        </PanelSection>
      )}

      {analysis.actionItems.length > 0 && (
        <PanelSection title="Action items">
          <ul className="space-y-1.5">
            {analysis.actionItems.map((p, i) => (
              <li key={i} className="flex gap-2 font-fa text-[13px] leading-relaxed" dir="auto">
                <CheckSquare className="mt-0.5 size-3.5 shrink-0 text-ok" />
                <span>{p}</span>
              </li>
            ))}
          </ul>
        </PanelSection>
      )}

      {analysis.chapters.length > 0 && (
        <PanelSection title="Chapters">
          <ChapterList chapters={analysis.chapters} />
        </PanelSection>
      )}

      <div className="px-4 py-3 text-[11px] text-faint">
        Generated by {analysis.model} · {new Date(analysis.createdAt).toLocaleDateString()}
      </div>
    </div>
  );
}
