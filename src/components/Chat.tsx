import { Fragment, useEffect, useRef, useState, type ReactNode } from "react";
import clsx from "clsx";
import {
  ArrowUp,
  AudioLines,
  Check,
  ChevronDown,
  Ellipsis,
  History,
  Mic,
  Paperclip,
  Pause,
  Pencil,
  Play,
  Square,
  SquarePen,
  Trash2,
  X,
} from "lucide-react";
import type { Citation } from "../lib/citations";
import type { ChatAttachment, ChatThread } from "../lib/types";
import { MAX_VOICE_SECS, attachmentKind, clipLength } from "../lib/attachments";
import { formatDate } from "../lib/format";
import { useAssetUrl } from "../hooks/useAssetUrl";
import { mediaUrl, pickChatFiles, playableUrl } from "../lib/platform";
import { canRecordVoice, startVoiceRecording, type VoiceRecorder } from "../lib/recorder";
import { useStreams } from "../store/streams";
import {
  scopeKey,
  stopLlm,
  useChats,
  useScopeChat,
  type ChatScope,
  type DraftAttachment,
  type ScopeChat,
} from "../store/chat";
import { toast } from "../store/toasts";
import { Button, IconButton } from "./ui/Button";
import { ConfirmDialog, Dialog } from "./ui/Dialog";
import { Markdown } from "./ui/Markdown";
import { Menu } from "./ui/Menu";
import { Spinner, inputClass, textareaClass } from "./ui/misc";

// ------------------------------------------------------------------ attachments

function ImageAttachment({ a }: { a: ChatAttachment }) {
  const [open, setOpen] = useState(false);
  const url = useAssetUrl(a.path);
  return (
    <>
      <button
        type="button"
        onClick={() => setOpen(true)}
        className="overflow-hidden rounded-lg border border-line bg-sunken transition-opacity hover:opacity-90"
        title={a.name}
      >
        {url ? (
          <img src={url} alt={a.name} className="h-24 max-w-[200px] object-cover" />
        ) : (
          <div className="flex h-24 w-32 items-center justify-center px-2 text-[11.5px] text-muted">{a.name}</div>
        )}
      </button>
      <Dialog open={open} onClose={() => setOpen(false)} title={a.name} size="xl">
        {url && <img src={url} alt={a.name} className="mx-auto max-h-[70vh] rounded-lg object-contain" />}
        {a.text && (
          <div className="mt-3 rounded-lg bg-sunken p-3 text-[12.5px]">
            <div className="mb-1 text-[11px] font-medium text-faint">Description by {a.textModel} (sent to models that can't see images)</div>
            <div className="whitespace-pre-wrap text-muted" dir="auto">
              {a.text}
            </div>
          </div>
        )}
      </Dialog>
    </>
  );
}

function VoiceAttachment({ a }: { a: ChatAttachment }) {
  const audio = useRef<HTMLAudioElement>(null);
  const loaded = useRef(false);
  const ownUrl = useRef<string | null>(null);
  const [playing, setPlaying] = useState(false);
  const [expanded, setExpanded] = useState(false);

  useEffect(
    () => () => {
      if (ownUrl.current) URL.revokeObjectURL(ownUrl.current);
    },
    [],
  );

  const toggle = async () => {
    const el = audio.current;
    if (!el) return;
    if (playing) {
      el.pause();
      return;
    }
    try {
      if (!loaded.current) {
        const url = await playableUrl(a.path);
        if (url !== mediaUrl(a.path)) ownUrl.current = url;
        el.src = url;
        loaded.current = true;
      }
      await el.play();
    } catch (e) {
      toast.error("Couldn't play the voice message", e);
    }
  };

  return (
    <div className="w-[260px] max-w-full rounded-xl border border-line bg-surface px-2.5 py-2 text-left">
      <div className="flex items-center gap-2">
        <IconButton label={playing ? "Pause" : "Play voice message"} size="sm" variant="secondary" onClick={() => void toggle()}>
          {playing ? <Pause className="size-3.5" /> : <Play className="size-3.5" />}
        </IconButton>
        <AudioLines className="size-4 shrink-0 text-accent" />
        <span className="min-w-0 flex-1 truncate text-[12.5px] font-medium" dir="auto">
          {a.name}
        </span>
        <span className="text-[11.5px] text-faint tabular-nums">{clipLength(a.durationSec)}</span>
      </div>
      {a.text && (
        <button
          type="button"
          onClick={() => setExpanded((v) => !v)}
          className={clsx("mt-1.5 block w-full text-left text-[12px] text-muted", !expanded && "line-clamp-2")}
          dir="auto"
          title={`Transcribed by ${a.textModel}`}
        >
          {a.text}
        </button>
      )}
      <audio
        ref={audio}
        preload="none"
        className="hidden"
        onPlay={() => setPlaying(true)}
        onPause={() => setPlaying(false)}
        onEnded={() => setPlaying(false)}
      />
    </div>
  );
}

function AttachmentView({ a }: { a: ChatAttachment }) {
  return a.kind === "image" ? <ImageAttachment a={a} /> : <VoiceAttachment a={a} />;
}

function DraftChip({ d, onRemove }: { d: DraftAttachment; onRemove: () => void }) {
  const a = d.attachment;
  const thumb = useAssetUrl(a?.kind === "image" ? a.path : null);
  return (
    <div className="flex max-w-[220px] items-center gap-1.5 rounded-lg border border-line bg-surface py-1 pr-1 pl-1.5 text-[12px]">
      {!a ? (
        <Spinner className="size-3.5" />
      ) : thumb ? (
        <img src={thumb} alt="" className="size-7 rounded object-cover" />
      ) : (
        <AudioLines className="size-4 text-accent" />
      )}
      <span className="min-w-0 flex-1 truncate" dir="auto" title={d.name}>
        {d.name}
      </span>
      {a?.kind === "audio" && <span className="text-faint tabular-nums">{clipLength(a.durationSec)}</span>}
      <IconButton label={`Remove ${d.name}`} size="xs" onClick={onRemove}>
        <X className="size-3" />
      </IconButton>
    </div>
  );
}

// ------------------------------------------------------------------ messages

function UserBubble({ content, attachments }: { content: string; attachments: ChatAttachment[] }) {
  return (
    <div className="flex flex-col items-end gap-1.5">
      {attachments.length > 0 && (
        <div className="flex max-w-[90%] flex-wrap justify-end gap-1.5">
          {attachments.map((a) => (
            <AttachmentView key={a.id} a={a} />
          ))}
        </div>
      )}
      {content && (
        <div
          className="max-w-[85%] rounded-2xl rounded-br-md bg-accent-soft px-3.5 py-2 text-[13.5px] whitespace-pre-wrap text-fg"
          dir="auto"
        >
          {content}
        </div>
      )}
    </div>
  );
}

function AssistantMessage({
  content,
  streaming,
  status,
  onCite,
  renderCite,
  model,
  contextNote,
}: {
  content: string;
  streaming?: boolean;
  status?: string | null;
  onCite: (c: Citation) => void;
  renderCite?: (c: Citation, label: ReactNode) => ReactNode;
  model?: string | null;
  contextNote?: string | null;
}) {
  return (
    <div className="text-[13.5px]">
      {content ? (
        <Markdown content={content} onCite={onCite} renderCite={renderCite} className="font-fa" />
      ) : (
        <div className="flex items-center gap-2 py-1" aria-label="Thinking">
          <div className="flex items-center gap-1">
            {[0, 1, 2].map((i) => (
              <span key={i} className="size-1.5 animate-pulse rounded-full bg-faint" style={{ animationDelay: `${i * 150}ms` }} />
            ))}
          </div>
          {status && <span className="text-[12px] text-muted">{status}</span>}
        </div>
      )}
      {streaming && content && <span className="ml-0.5 inline-block h-3.5 w-1.5 animate-pulse bg-accent align-middle" />}
      {!streaming && (model || contextNote) && (
        <div className="mt-1.5 text-[11px] text-faint">
          {model}
          {model && contextNote && " · "}
          {contextNote}
        </div>
      )}
    </div>
  );
}

function CompactionDivider({ summary }: { summary: string | null }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="py-1">
      <div className="flex items-center gap-2 text-[11.5px] text-faint">
        <span className="h-px flex-1 bg-line" />
        <span>Messages above are summarized for the AI</span>
        {summary && (
          <button type="button" className="text-accent hover:underline" onClick={() => setOpen((v) => !v)}>
            {open ? "Hide summary" : "Show summary"}
          </button>
        )}
        <span className="h-px flex-1 bg-line" />
      </div>
      {open && summary && (
        <div className="mt-2 rounded-lg bg-sunken p-3 text-[12.5px] text-muted">
          <Markdown content={summary} className="font-fa" />
        </div>
      )}
    </div>
  );
}

// ------------------------------------------------------------------ thread bar

function RenameDialog({ thread, onClose, onSave }: { thread: ChatThread | null; onClose: () => void; onSave: (title: string) => void }) {
  const [title, setTitle] = useState("");
  useEffect(() => {
    if (thread) setTitle(thread.title);
  }, [thread]);
  return (
    <Dialog
      open={!!thread}
      onClose={onClose}
      title="Rename conversation"
      size="sm"
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" disabled={!title.trim()} onClick={() => onSave(title.trim())}>
            Save
          </Button>
        </>
      }
    >
      <form
        onSubmit={(e) => {
          e.preventDefault();
          if (title.trim()) onSave(title.trim());
        }}
      >
        <input
          value={title}
          onChange={(e) => setTitle(e.target.value)}
          aria-label="Conversation name"
          dir="auto"
          maxLength={120}
          className={inputClass}
        />
      </form>
    </Dialog>
  );
}

function ThreadBar({ scope, chat, thread }: { scope: ChatScope; chat: ScopeChat; thread: ChatThread | null }) {
  const open = useChats((s) => s.open);
  const [renaming, setRenaming] = useState<ChatThread | null>(null);
  const [deleting, setDeleting] = useState<ChatThread | null>(null);

  return (
    <div className="flex h-10 shrink-0 items-center gap-1 border-b border-line px-2">
      <Menu
        className="min-w-0"
        items={
          chat.threads.length
            ? [
                { heading: "Conversations" },
                ...chat.threads.map((t) => ({
                  key: t.id,
                  label: <span dir="auto">{t.title}</span>,
                  hint: formatDate(t.updatedAt),
                  checked: t.id === chat.activeId,
                  onSelect: () => void open(scope, t.id),
                })),
              ]
            : [{ heading: "No conversations yet" }]
        }
        trigger={(p) => (
          <button
            {...p}
            type="button"
            className="flex min-w-0 items-center gap-1.5 rounded-md px-2 py-1 text-[13px] font-medium text-fg hover:bg-hover"
            title="Switch conversation"
          >
            <History className="size-3.5 shrink-0 text-muted" />
            <span className="truncate" dir="auto">
              {thread?.title ?? "New conversation"}
            </span>
            <ChevronDown className="size-3.5 shrink-0 text-muted" />
          </button>
        )}
      />
      <span className="flex-1" />
      <IconButton label="New conversation" onClick={() => void open(scope, null)} disabled={!chat.activeId}>
        <SquarePen className="size-4" />
      </IconButton>
      {thread && (
        <Menu
          align="end"
          items={[
            { key: "rename", label: "Rename…", icon: <Pencil />, onSelect: () => setRenaming(thread) },
            { key: "delete", label: "Delete conversation", icon: <Trash2 />, danger: true, onSelect: () => setDeleting(thread) },
          ]}
          trigger={(p) => (
            <IconButton {...p} label="Conversation actions">
              <Ellipsis className="size-4" />
            </IconButton>
          )}
        />
      )}
      <RenameDialog
        thread={renaming}
        onClose={() => setRenaming(null)}
        onSave={(title) => {
          const t = renaming;
          setRenaming(null);
          if (t) void useChats.getState().rename(scope, t.id, title);
        }}
      />
      <ConfirmDialog
        open={!!deleting}
        title="Delete this conversation?"
        message={deleting ? `“${deleting.title}” and its attachments will be deleted.` : ""}
        confirmLabel="Delete"
        danger
        onCancel={() => setDeleting(null)}
        onConfirm={() => {
          const t = deleting;
          setDeleting(null);
          if (t) void useChats.getState().remove(scope, t.id);
        }}
      />
    </div>
  );
}

// ------------------------------------------------------------------ composer

function VoiceRecorderBar({ recorder, onDone, onCancel }: { recorder: VoiceRecorder; onDone: () => void; onCancel: () => void }) {
  const [, tick] = useState(0);
  const secs = recorder.elapsed();
  useEffect(() => {
    const id = window.setInterval(() => tick((n) => n + 1), 100);
    return () => window.clearInterval(id);
  }, []);
  useEffect(() => {
    // stop just under the backend's limit
    if (secs >= MAX_VOICE_SECS - 1) onDone();
  }, [secs, onDone]);
  const level = Math.min(1, recorder.level() * 1.6);
  return (
    <div className="flex h-9 items-center gap-2.5 rounded-lg border border-danger/40 bg-danger-soft/40 px-2.5" role="status">
      <span className="size-2.5 animate-pulse rounded-full bg-danger" />
      <span className="text-[12.5px] font-medium tabular-nums">{clipLength(secs)}</span>
      <div className="h-1.5 flex-1 overflow-hidden rounded-full bg-sunken" aria-hidden>
        <div className="h-full rounded-full bg-danger transition-[width] duration-100" style={{ width: `${level * 100}%` }} />
      </div>
      <IconButton label="Discard recording" size="sm" onClick={onCancel}>
        <X className="size-3.5" />
      </IconButton>
      <IconButton label="Attach recording" size="sm" variant="primary" onClick={onDone}>
        <Check className="size-3.5" />
      </IconButton>
    </div>
  );
}

function Composer({
  scope,
  chat,
  placeholder,
  blocked,
}: {
  scope: ChatScope;
  chat: ScopeChat;
  placeholder: string;
  blocked?: ReactNode;
}) {
  const [draft, setDraft] = useState("");
  const [recorder, setRecorder] = useState<VoiceRecorder | null>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const recorderRef = useRef<VoiceRecorder | null>(null);
  recorderRef.current = recorder;
  const pending = chat.pending;
  const uploading = chat.drafts.some((d) => !d.attachment);
  const canSend = !pending && !uploading && (!!draft.trim() || chat.drafts.length > 0);

  useEffect(() => {
    const el = inputRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight + 2, 160)}px`;
  }, [draft]);

  // a recording in progress is dropped when the panel goes away
  useEffect(() => () => recorderRef.current?.cancel(), []);

  const send = async (text: string) => {
    const q = text.trim();
    if (pending || uploading || (!q && !chat.drafts.length)) return;
    setDraft("");
    const ok = await useChats.getState().ask(scope, q);
    if (!ok) setDraft((d) => d || q);
  };

  const attach = async () => {
    const { paths, files } = await pickChatFiles();
    if (paths.length) await useChats.getState().attachPaths(scope, paths);
    if (files.length) await useChats.getState().attachFiles(scope, files);
  };

  const record = async () => {
    try {
      setRecorder(await startVoiceRecording());
    } catch (e) {
      toast.error("Couldn't record", e);
    }
  };

  const finishRecording = () => {
    const r = recorderRef.current;
    if (!r) return;
    setRecorder(null);
    const { samples, sampleRate } = r.stop();
    if (samples.length < sampleRate * 0.4) {
      toast.info("The recording was too short");
      return;
    }
    void useChats.getState().attachVoice(scope, samples, sampleRate);
  };

  if (blocked) return <div className="border-t border-line p-3">{blocked}</div>;

  return (
    <div className="border-t border-line p-3">
      {chat.drafts.length > 0 && (
        <div className="mb-2 flex flex-wrap gap-1.5">
          {chat.drafts.map((d) => (
            <DraftChip key={d.key} d={d} onRemove={() => useChats.getState().discardDraft(scope, d.key)} />
          ))}
        </div>
      )}
      <div className="flex items-end gap-1.5">
        {recorder ? (
          <div className="min-w-0 flex-1">
            <VoiceRecorderBar
              recorder={recorder}
              onDone={finishRecording}
              onCancel={() => {
                recorder.cancel();
                setRecorder(null);
              }}
            />
          </div>
        ) : (
          <>
            <IconButton label="Attach images or audio" size="md" onClick={() => void attach()}>
              <Paperclip className="size-4" />
            </IconButton>
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
              onPaste={(e) => {
                const files = Array.from(e.clipboardData.items)
                  .filter((i) => i.kind === "file")
                  .map((i) => i.getAsFile())
                  .filter((f): f is File => !!f && !!attachmentKind(f.name, f.type));
                if (!files.length) return;
                if (!e.clipboardData.types.includes("text/plain")) e.preventDefault();
                void useChats.getState().attachFiles(scope, files);
              }}
              placeholder={placeholder}
              aria-label="Your question"
              dir="auto"
              className={clsx(textareaClass, "min-h-9 font-fa leading-normal")}
            />
            {canRecordVoice() && !pending && (
              <IconButton label="Record a voice message" size="md" onClick={() => void record()}>
                <Mic className="size-4" />
              </IconButton>
            )}
          </>
        )}
        {pending ? (
          <IconButton label="Stop generating" size="md" variant="secondary" onClick={() => stopLlm(pending.requestId)}>
            <Square className="size-3.5" fill="currentColor" />
          </IconButton>
        ) : (
          !recorder && (
            <IconButton label="Send (Enter)" size="md" variant="primary" onClick={() => void send(draft)} disabled={!canSend}>
              <ArrowUp className="size-4" />
            </IconButton>
          )
        )}
      </div>
    </div>
  );
}

// ------------------------------------------------------------------ panel

export interface ChatPanelProps {
  scope: ChatScope;
  onCite: (c: Citation) => void;
  renderCite?: (c: Citation, label: ReactNode) => ReactNode;
  suggestions?: string[];
  placeholder?: string;
  /** Replaces the composer (e.g. "add an API key"). */
  blocked?: ReactNode;
  empty?: ReactNode;
  className?: string;
}

/** Ask conversations for one scope: thread switcher, messages, and a composer that takes
 *  text, images and voice. Files dropped on it are attached (see `data-chat-drop`). */
export function ChatPanel({
  scope,
  onCite,
  renderCite,
  suggestions = [],
  placeholder = "Ask a question…",
  blocked,
  empty,
  className,
}: ChatPanelProps) {
  const chat = useScopeChat(scope);
  const load = useChats((s) => s.load);
  const scrollRef = useRef<HTMLDivElement>(null);
  const stickToBottom = useRef(true);
  const thread = chat.threads.find((t) => t.id === chat.activeId) ?? null;
  const messages = (chat.activeId && chat.messages[chat.activeId]) || [];
  const pending = chat.pending && chat.pending.threadId === chat.activeId ? chat.pending : null;
  const stream = useStreams((s) => (pending ? s.streams[pending.requestId] : undefined));
  const streamText = stream?.text ?? "";

  useEffect(() => {
    void load(scope);
  }, [load, scope]);

  useEffect(() => {
    stickToBottom.current = true;
  }, [chat.activeId]);

  useEffect(() => {
    const el = scrollRef.current;
    if (el && stickToBottom.current) el.scrollTop = el.scrollHeight;
  }, [messages.length, streamText, pending, chat.activeId]);

  const showSuggestions = suggestions.length > 0 && !chat.pending && messages.length < 4 && !blocked && chat.drafts.length === 0;
  const ask = (q: string) => {
    stickToBottom.current = true;
    void useChats.getState().ask(scope, q);
  };

  return (
    <div data-chat-drop={blocked ? undefined : scopeKey(scope)} className={clsx("flex min-h-0 flex-1 flex-col", className)}>
      <ThreadBar scope={scope} chat={chat} thread={thread} />
      <div
        ref={scrollRef}
        className="min-h-0 flex-1 space-y-5 overflow-y-auto px-4 py-4"
        onScroll={(e) => {
          const el = e.currentTarget;
          stickToBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
        }}
      >
        {chat.activeId && chat.loadErrors[chat.activeId] && !chat.messages[chat.activeId] ? (
          <div className="flex flex-col items-center gap-2 pt-10 text-center text-[12.5px] text-muted">
            <span>Couldn't load this conversation: {chat.loadErrors[chat.activeId]}</span>
            <Button size="sm" onClick={() => void useChats.getState().reloadMessages(scope, chat.activeId!)}>
              Retry
            </Button>
          </div>
        ) : !chat.loaded || (chat.activeId && !chat.messages[chat.activeId]) ? (
          <div className="flex justify-center pt-10">
            <Spinner />
          </div>
        ) : (
          <>
            {messages.length === 0 && !pending && empty}
            {messages.map((m, i) => (
              <Fragment key={m.id}>
                {thread && thread.compactedCount > 0 && i === thread.compactedCount && <CompactionDivider summary={thread.summary} />}
                {m.role === "user" ? (
                  <UserBubble content={m.content} attachments={m.attachments ?? []} />
                ) : (
                  <AssistantMessage
                    content={m.content}
                    onCite={onCite}
                    renderCite={renderCite}
                    model={m.model}
                    contextNote={m.contextNote}
                  />
                )}
              </Fragment>
            ))}
            {pending && (
              <>
                <UserBubble content={pending.question} attachments={pending.attachments} />
                <AssistantMessage content={streamText} streaming status={stream?.status} onCite={onCite} renderCite={renderCite} />
              </>
            )}
          </>
        )}
      </div>

      {showSuggestions && (
        <div className="flex flex-wrap gap-1.5 px-4 pb-2">
          {suggestions.slice(0, 4).map((s) => (
            <button
              key={s}
              type="button"
              onClick={() => ask(s)}
              className="rounded-full border border-line bg-surface px-3 py-1 text-[12px] text-muted transition-colors hover:border-accent hover:text-fg"
              dir="auto"
            >
              {s}
            </button>
          ))}
        </div>
      )}

      {chat.pending && !pending && (
        <div className="px-4 pb-1 text-[11.5px] text-faint">Answering in another conversation…</div>
      )}
      <Composer scope={scope} chat={chat} placeholder={placeholder} blocked={blocked} />
    </div>
  );
}

/** The chat panel under a point (in CSS pixels), for routing dropped files. */
export function chatScopeAt(x: number, y: number): ChatScope | undefined {
  for (const el of Array.from(document.querySelectorAll<HTMLElement>("[data-chat-drop]"))) {
    const r = el.getBoundingClientRect();
    if (x >= r.left && x <= r.right && y >= r.top && y <= r.bottom) {
      const key = el.dataset.chatDrop!;
      return key === "library" ? null : key;
    }
  }
  return undefined;
}
