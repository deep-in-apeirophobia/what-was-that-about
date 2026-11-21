import { useCallback, type ReactNode } from "react";
import { MessagesSquare } from "lucide-react";
import type { Citation } from "../lib/citations";
import { useApp } from "../store/app";
import { useLibrary } from "../store/library";
import { hasApiKey, useSettings } from "../store/settings";
import { toast } from "../store/toasts";
import { ChatPanel } from "../components/Chat";
import { EmptyState } from "../components/ui/misc";
import { NoKeyCta } from "../components/recording/AiCta";

const SUGGESTIONS = [
  "What decisions were made across my meetings?",
  "Which recordings mention deadlines?",
  "Summarize everything about the budget.",
  "کجاها دربارهٔ حریم خصوصی صحبت شده؟",
];

export function AskLibraryView() {
  const recordings = useLibrary((s) => s.recordings);
  const settings = useSettings((s) => s.settings);
  const openRecording = useApp((s) => s.openRecording);
  const transcribed = recordings.filter((r) => r.status === "ready" && r.segmentCount > 0).length;

  const onCite = useCallback(
    (c: Citation) => {
      if (!c.recordingId) return;
      if (!useLibrary.getState().recordings.some((r) => r.id === c.recordingId)) {
        toast.info("That recording isn't in your library anymore");
        return;
      }
      openRecording(c.recordingId, { segmentId: c.start, play: true });
    },
    [openRecording],
  );

  const renderCite = useCallback(
    (c: Citation, label: ReactNode) => {
      const title = recordings.find((r) => r.id === c.recordingId)?.title;
      return (
        <>
          {title && (
            <span className="mr-1 inline-block max-w-[180px] truncate align-bottom font-medium" dir="auto">
              {title}
            </span>
          )}
          <span className="opacity-80">#{label}</span>
        </>
      );
    },
    [recordings],
  );

  return (
    <div className="flex h-full flex-col">
      <header className="border-b border-line px-6 py-3.5">
        <h1 className="text-[17px] font-semibold">Ask your library</h1>
        <p className="text-[12.5px] text-muted">
          Questions are answered from all {transcribed} transcribed recording{transcribed === 1 ? "" : "s"}, with links to the exact moments.
        </p>
      </header>
      <div className="mx-auto flex min-h-0 w-full max-w-3xl flex-1 flex-col">
        <ChatPanel
          scope={null}
          onCite={onCite}
          renderCite={renderCite}
          suggestions={SUGGESTIONS}
          placeholder="Ask across all your recordings…"
          blocked={
            !hasApiKey(settings) ? (
              <NoKeyCta what="ask questions across your library" compact />
            ) : transcribed === 0 ? (
              <div className="py-1 text-center text-[12.5px] text-muted">Transcribe a recording first, then ask away.</div>
            ) : undefined
          }
          empty={
            <EmptyState icon={<MessagesSquare />} title="What was that about?" className="pt-16">
              Ask about anything said in any recording — “when did we decide on the budget?” — and jump to the answer. Attach
              images or voice messages when it helps.
            </EmptyState>
          }
        />
      </div>
    </div>
  );
}
