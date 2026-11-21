import { useState } from "react";
import { Network, Sparkles } from "lucide-react";
import { formatTime } from "../../lib/format";
import type { SearchHit } from "../../lib/types";
import { useApp } from "../../store/app";
import { useRecording } from "../../store/recording";
import { hasApiKey, useSettings } from "../../store/settings";
import { Button } from "../ui/Button";
import { Badge, EmptyState, Spinner } from "../ui/misc";
import { PanelSection } from "./AiCta";
import { enableSemanticSearch, seekToSegment } from "./actions";

export function SemanticSearchOffer() {
  const detail = useRecording((s) => s.detail)!;
  const settings = useSettings((s) => s.settings);
  const [busy, setBusy] = useState(false);
  if (detail.embeddingsIndexed || detail.recording.status !== "ready" || !detail.segments.length) return null;
  const keyOk = hasApiKey(settings);
  return (
    <div className="mx-4 my-3 flex items-start gap-3 rounded-lg border border-accent/20 bg-accent-soft/40 px-3 py-2.5">
      <Sparkles className="mt-0.5 size-4 shrink-0 text-accent" />
      <div className="min-w-0 flex-1 text-[12.5px]">
        <div className="font-medium">Enable semantic search</div>
        <div className="mt-0.5 text-muted">
          {keyOk
            ? "Find moments by meaning, not just exact words. Takes a few seconds."
            : "Needs an OpenRouter key (for embeddings). Until then, matching uses shared words."}
        </div>
      </div>
      <Button
        size="sm"
        loading={busy}
        disabled={!keyOk}
        onClick={async () => {
          setBusy(true);
          const ok = await enableSemanticSearch(detail.recording.id);
          setBusy(false);
          const rel = useRecording.getState().related;
          if (ok && rel) {
            const seg = useRecording.getState().detail?.segments.find((s) => s.id === rel.segmentId);
            if (seg) void useRecording.getState().findRelated(seg);
          }
        }}
      >
        Enable
      </Button>
    </div>
  );
}

function HitRow({ hit, currentId }: { hit: SearchHit; currentId: string }) {
  const openRecording = useApp((s) => s.openRecording);
  const same = hit.recordingId === currentId;
  return (
    <button
      type="button"
      onClick={() => (same ? seekToSegment(hit.segmentId, true) : openRecording(hit.recordingId, { segmentId: hit.segmentId, play: true }))}
      className="flex w-full flex-col gap-1 rounded-lg px-2 py-2 text-left hover:bg-hover"
    >
      <span className="flex items-center gap-2 text-[11.5px]">
        <span className="font-medium text-accent tabular-nums">{formatTime(hit.start)}</span>
        <span className="min-w-0 flex-1 truncate text-faint" dir="auto">
          {same ? "This recording" : hit.recordingTitle}
        </span>
        <Badge tone={hit.kind === "semantic" ? "accent" : "neutral"}>{Math.round(hit.score * 100)}%</Badge>
      </span>
      <span className="font-fa text-[13px] leading-relaxed" dir="auto">
        {hit.text}
      </span>
    </button>
  );
}

export function RelatedTab() {
  const related = useRecording((s) => s.related);
  const id = useRecording((s) => s.id)!;

  if (!related) {
    return (
      <>
        <SemanticSearchOffer />
        <EmptyState icon={<Network />} title="Find related moments">
          Hover a sentence in the transcript and choose <b className="font-medium text-fg">Find related</b> to see similar moments across your whole
          library.
        </EmptyState>
      </>
    );
  }

  return (
    <div>
      <SemanticSearchOffer />
      <PanelSection title="Related to">
        <button
          type="button"
          onClick={() => seekToSegment(related.segmentId, true)}
          className="w-full rounded-lg border border-line bg-sunken/60 px-3 py-2 text-left font-fa text-[13px] leading-relaxed hover:border-line-strong"
          dir="auto"
        >
          {related.text}
        </button>
      </PanelSection>
      <PanelSection title="Similar moments">
        {related.hits == null ? (
          <div className="flex justify-center py-6">
            <Spinner />
          </div>
        ) : related.error ? (
          <div className="rounded-lg bg-danger-soft px-3 py-2 text-[12.5px] text-danger">{related.error}</div>
        ) : related.hits.length === 0 ? (
          <div className="py-4 text-center text-[12.5px] text-muted">Nothing similar was found.</div>
        ) : (
          <div className="-mx-2 space-y-0.5">
            {related.hits.map((h) => (
              <HitRow key={`${h.recordingId}-${h.segmentId}`} hit={h} currentId={id} />
            ))}
          </div>
        )}
      </PanelSection>
    </div>
  );
}
