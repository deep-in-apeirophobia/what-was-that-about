import { MessagesSquare } from "lucide-react";
import { useRecording } from "../../store/recording";
import { hasApiKey, useSettings } from "../../store/settings";
import { ChatPanel } from "../Chat";
import { EmptyState } from "../ui/misc";
import { NoKeyCta } from "./AiCta";
import { followCitation } from "./actions";

export function AskTab() {
  const detail = useRecording((s) => s.detail)!;
  const settings = useSettings((s) => s.settings);
  const { recording, analysis, segments } = detail;
  const keyOk = hasApiKey(settings);
  const hasText = segments.length > 0;

  return (
    <ChatPanel
      key={recording.id}
      scope={recording.id}
      onCite={followCitation}
      suggestions={analysis?.questions ?? []}
      placeholder="Ask about this recording…"
      blocked={
        !keyOk ? (
          <NoKeyCta what="ask questions about this recording" compact />
        ) : !hasText ? (
          <div className="py-1 text-center text-[12.5px] text-muted">You can ask questions once there's a transcript.</div>
        ) : undefined
      }
      empty={
        <EmptyState icon={<MessagesSquare />} title="Ask anything about this recording">
          Answers cite the exact sentences they're based on — click a citation to hear it. You can attach images or voice
          messages too.
        </EmptyState>
      }
    />
  );
}
