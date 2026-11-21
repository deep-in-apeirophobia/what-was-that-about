import { useEffect, useRef, useState, type ReactNode } from "react";
import clsx from "clsx";
import { ArrowUp, Eraser, Square } from "lucide-react";
import type { Citation } from "../lib/citations";
import type { ChatMessage } from "../lib/types";
import { useStreams } from "../store/streams";
import { stopLlm, type PendingQuestion } from "../store/chat";
import { Button, IconButton } from "./ui/Button";
import { ConfirmDialog } from "./ui/Dialog";
import { Markdown } from "./ui/Markdown";
import { textareaClass } from "./ui/misc";

function UserBubble({ content }: { content: string }) {
  return (
    <div className="flex justify-end">
      <div
        className="max-w-[85%] rounded-2xl rounded-br-md bg-accent-soft px-3.5 py-2 text-[13.5px] whitespace-pre-wrap text-fg"
        dir="auto"
      >
        {content}
      </div>
    </div>
  );
}

function AssistantMessage({
  content,
  streaming,
  onCite,
  renderCite,
  model,
}: {
  content: string;
  streaming?: boolean;
  onCite: (c: Citation) => void;
  renderCite?: (c: Citation, label: ReactNode) => ReactNode;
  model?: string | null;
}) {
  return (
    <div className="text-[13.5px]">
      {content ? (
        <Markdown content={content} onCite={onCite} renderCite={renderCite} className="font-fa" />
      ) : (
        <div className="flex items-center gap-1 py-1" aria-label="Thinking">
          {[0, 1, 2].map((i) => (
            <span key={i} className="size-1.5 animate-pulse rounded-full bg-faint" style={{ animationDelay: `${i * 150}ms` }} />
          ))}
        </div>
      )}
      {streaming && content && <span className="ml-0.5 inline-block h-3.5 w-1.5 animate-pulse bg-accent align-middle" />}
      {model && !streaming && <div className="mt-1.5 text-[11px] text-faint">{model}</div>}
    </div>
  );
}

export interface ChatThreadProps {
  messages: ChatMessage[];
  pending: PendingQuestion | null;
  onAsk: (question: string) => Promise<boolean>;
  onClear: () => Promise<void>;
  onCite: (c: Citation) => void;
  renderCite?: (c: Citation, label: ReactNode) => ReactNode;
  suggestions?: string[];
  placeholder?: string;
  /** Replaces the composer (e.g. "add an API key"). */
  blocked?: ReactNode;
  empty?: ReactNode;
  className?: string;
}

export function ChatThread({
  messages,
  pending,
  onAsk,
  onClear,
  onCite,
  renderCite,
  suggestions = [],
  placeholder = "Ask a question…",
  blocked,
  empty,
  className,
}: ChatThreadProps) {
  const [draft, setDraft] = useState("");
  const [confirmClear, setConfirmClear] = useState(false);
  const streamText = useStreams((s) => (pending ? (s.streams[pending.requestId]?.text ?? "") : ""));
  const scrollRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const stickToBottom = useRef(true);

  useEffect(() => {
    const el = scrollRef.current;
    if (el && stickToBottom.current) el.scrollTop = el.scrollHeight;
  }, [messages.length, streamText, pending]);

  useEffect(() => {
    const el = inputRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight + 2, 160)}px`;
  }, [draft]);

  const send = async (text: string) => {
    const q = text.trim();
    if (!q || pending) return;
    setDraft("");
    stickToBottom.current = true;
    const ok = await onAsk(q);
    if (!ok) setDraft((d) => d || q);
  };

  const showSuggestions = suggestions.length > 0 && !pending && messages.length < 4 && !blocked;

  return (
    <div className={clsx("flex min-h-0 flex-1 flex-col", className)}>
      <div
        ref={scrollRef}
        className="min-h-0 flex-1 space-y-5 overflow-y-auto px-4 py-4"
        onScroll={(e) => {
          const el = e.currentTarget;
          stickToBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
        }}
      >
        {messages.length === 0 && !pending && empty}
        {messages.map((m) =>
          m.role === "user" ? (
            <UserBubble key={m.id} content={m.content} />
          ) : (
            <AssistantMessage key={m.id} content={m.content} onCite={onCite} renderCite={renderCite} model={m.model} />
          ),
        )}
        {pending && (
          <>
            <UserBubble content={pending.question} />
            <AssistantMessage content={streamText} streaming onCite={onCite} renderCite={renderCite} />
          </>
        )}
      </div>

      {showSuggestions && (
        <div className="flex flex-wrap gap-1.5 px-4 pb-2">
          {suggestions.slice(0, 4).map((s) => (
            <button
              key={s}
              type="button"
              onClick={() => void send(s)}
              className="rounded-full border border-line bg-surface px-3 py-1 text-[12px] text-muted transition-colors hover:border-accent hover:text-fg"
              dir="auto"
            >
              {s}
            </button>
          ))}
        </div>
      )}

      <div className="border-t border-line p-3">
        {blocked ?? (
          <div className="flex items-end gap-2">
            <textarea
              ref={inputRef}
              rows={1}
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
                  e.preventDefault();
                  void send(draft);
                }
              }}
              placeholder={placeholder}
              aria-label="Your question"
              dir="auto"
              className={clsx(textareaClass, "min-h-9 font-fa leading-normal")}
            />
            {pending ? (
              <IconButton
                label="Stop generating"
                size="md"
                variant="secondary"
                onClick={() => stopLlm(pending.requestId)}
              >
                <Square className="size-3.5" fill="currentColor" />
              </IconButton>
            ) : (
              <IconButton label="Send (Enter)" size="md" variant="primary" onClick={() => void send(draft)} disabled={!draft.trim()}>
                <ArrowUp className="size-4" />
              </IconButton>
            )}
          </div>
        )}
        {messages.length > 0 && !pending && (
          <div className="mt-2 flex justify-end">
            <Button size="sm" variant="ghost" icon={<Eraser className="size-3.5" />} onClick={() => setConfirmClear(true)}>
              Clear chat
            </Button>
          </div>
        )}
      </div>

      <ConfirmDialog
        open={confirmClear}
        title="Clear this chat?"
        message="All questions and answers in this conversation will be deleted."
        confirmLabel="Clear chat"
        danger
        onCancel={() => setConfirmClear(false)}
        onConfirm={() => {
          setConfirmClear(false);
          void onClear();
        }}
      />
    </div>
  );
}
