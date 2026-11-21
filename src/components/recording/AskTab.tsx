import { MessagesSquare } from "lucide-react";
import { useRecording } from "../../store/recording";
import { hasApiKey, useSettings } from "../../store/settings";
import { askStreaming, localUserMessage } from "../../store/chat";
import { api } from "../../lib/api";
import { attempt } from "../../store/toasts";
import { ChatThread } from "../Chat";
import { EmptyState } from "../ui/misc";
import { NoKeyCta } from "./AiCta";
import { followCitation } from "./actions";

export function AskTab() {
  const detail = useRecording((s) => s.detail)!;
  const pending = useRecording((s) => s.pendingAsk);
  const settings = useSettings((s) => s.settings);
  const { recording, chat, analysis, segments } = detail;
  const keyOk = hasApiKey(settings);
  const hasText = segments.length > 0;

  const onAsk = async (question: string) => {
    const id = recording.id;
    const msg = await askStreaming(id, question, (p) => useRecording.getState().setPendingAsk(p));
    const rec = useRecording.getState();
    if (rec.id === id) {
      rec.setPendingAsk(null);
      if (msg) rec.appendChat(localUserMessage(id, question), msg);
    }
    return !!msg;
  };

  const onClear = async () => {
    const ok = await attempt("Couldn't clear the chat", async () => {
      await api.clearChat(recording.id);
      return true;
    });
    if (ok) useRecording.getState().clearChat();
  };

  return (
    <ChatThread
      messages={chat}
      pending={pending}
      onAsk={onAsk}
      onClear={onClear}
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
          Answers cite the exact sentences they're based on — click a citation to hear it.
        </EmptyState>
      }
    />
  );
}
