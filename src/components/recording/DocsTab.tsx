import { useState, type ReactNode } from "react";
import {
  BookA,
  BrainCircuit,
  ClipboardList,
  FileCode2,
  GalleryVerticalEnd,
  GraduationCap,
  ListChecks,
  Wand2,
} from "lucide-react";
import { api } from "../../lib/api";
import { formatBytes, formatDate } from "../../lib/format";
import { newRequestId } from "../../lib/platform";
import type { Doc } from "../../lib/types";
import { isCancellation, stopLlm } from "../../store/chat";
import { useRecording } from "../../store/recording";
import { hasApiKey, useSettings } from "../../store/settings";
import { useStreams } from "../../store/streams";
import { toast } from "../../store/toasts";
import { Button } from "../ui/Button";
import { ProgressBar, textareaClass } from "../ui/misc";
import { NoKeyCta, PanelSection } from "./AiCta";
import { DocViewer } from "./DocViewer";

/** `key` is also the name of the template's design guide (crates/wwta-core/src/agent/skills). */
const TEMPLATES: { key: string; label: string; icon: ReactNode; prompt: string }[] = [
  {
    key: "mindmap",
    label: "Mind map",
    icon: <BrainCircuit />,
    prompt:
      "Create an interactive mind map of the main topics and how they connect. Clicking a node plays the moment where it is discussed.",
  },
  {
    key: "timeline",
    label: "Timeline",
    icon: <GalleryVerticalEnd />,
    prompt:
      "Create an interactive timeline of the chapters and key moments. Clicking an item plays it, and the current item is highlighted while the audio plays.",
  },
  {
    key: "flashcards",
    label: "Flashcards",
    icon: <GraduationCap />,
    prompt:
      "Create flip-able flashcards (question on the front, answer on the back) covering the key facts, each with a button that plays the moment it comes from.",
  },
  {
    key: "quiz",
    label: "Quiz",
    icon: <ListChecks />,
    prompt:
      "Create a short multiple-choice quiz (about 8 questions) with instant feedback, a final score, and a “hear the answer” button that plays the relevant moment.",
  },
  {
    key: "minutes",
    label: "Meeting minutes",
    icon: <ClipboardList />,
    prompt:
      "Write clean meeting minutes: participants mentioned, decisions, open questions and action items with owners and deadlines — each linked to the moment it was said.",
  },
  {
    key: "glossary",
    label: "Glossary",
    icon: <BookA />,
    prompt:
      "Build a searchable glossary of names, terms and jargon used, with short definitions and links to where each one first appears.",
  },
];

function GeneratingCard({ requestId, prompt }: { requestId: string; prompt: string }) {
  const len = useStreams((s) => s.streams[requestId]?.text.length ?? 0);
  const status = useStreams((s) => s.streams[requestId]?.status ?? null);
  return (
    <div className="rounded-xl border border-accent/25 bg-accent-soft/30 p-3">
      <div className="flex items-center gap-2 text-[13px] font-medium">
        <Wand2 className="size-3.5 shrink-0 animate-pulse text-accent" />
        {len > 0 ? "Writing…" : "Generating…"}
        <span className="ml-auto text-[12px] font-normal text-muted tabular-nums">{len > 0 ? formatBytes(len) : "thinking"}</span>
      </div>
      <p className="mt-1 line-clamp-2 text-[12px] text-muted" dir="auto">
        {prompt}
      </p>
      {status && len === 0 && (
        <p className="mt-1 text-[11.5px] text-faint" dir="auto">
          {status}
        </p>
      )}
      <ProgressBar className="mt-2.5" label="Generating doc" />
      <div className="mt-2 flex justify-end">
        <Button size="sm" onClick={() => stopLlm(requestId)}>
          Stop
        </Button>
      </div>
    </div>
  );
}

export function DocsTab() {
  const detail = useRecording((s) => s.detail)!;
  const pending = useRecording((s) => s.pendingDoc);
  const settings = useSettings((s) => s.settings);
  const [prompt, setPrompt] = useState("");
  const [openDoc, setOpenDoc] = useState<string | null>(null);
  const { recording, docs, segments } = detail;
  const keyOk = hasApiKey(settings);
  const canGenerate = keyOk && segments.length > 0 && recording.status === "ready" && !pending;
  const viewing = docs.find((d) => d.id === openDoc) ?? null;

  /** `skill` is the template's design guide; custom prompts let the model pick one. */
  const generate = async (text: string, skill: string | null = null) => {
    const p = text.trim();
    if (!p || !canGenerate) return;
    const id = recording.id;
    const requestId = newRequestId();
    useStreams.getState().start(requestId);
    useRecording.getState().setPendingDoc({ requestId, prompt: p });
    try {
      const doc: Doc = await api.generateDoc(id, p, skill, requestId);
      const rec = useRecording.getState();
      if (rec.id === id) {
        rec.upsertDoc(doc);
        setPrompt("");
        setOpenDoc(doc.id);
      }
      toast.success("Doc ready", doc.title);
    } catch (e) {
      if (isCancellation(e)) toast.info("Generation stopped");
      else toast.error("Couldn't generate the doc", e);
    } finally {
      useStreams.getState().clear(requestId);
      if (useRecording.getState().pendingDoc?.requestId === requestId) useRecording.getState().setPendingDoc(null);
    }
  };

  return (
    <div>
      <PanelSection title="Create an interactive doc">
        {!keyOk ? (
          <NoKeyCta what="generate interactive docs" compact />
        ) : (
          <>
            <div className="grid grid-cols-2 gap-1.5">
              {TEMPLATES.map((t) => (
                <button
                  key={t.key}
                  type="button"
                  disabled={!canGenerate}
                  onClick={() => {
                    void api.trackUi("doc template used", { template: t.key });
                    void generate(t.prompt, t.key);
                  }}
                  title={t.prompt}
                  className="flex items-center gap-2 rounded-lg border border-line bg-surface px-2.5 py-2 text-left text-[12.5px] font-medium transition-colors hover:border-accent hover:bg-accent-soft/40 disabled:pointer-events-none disabled:opacity-50 [&>svg]:size-4 [&>svg]:shrink-0 [&>svg]:text-accent"
                >
                  {t.icon}
                  {t.label}
                </button>
              ))}
            </div>
            <form
              className="mt-3"
              onSubmit={(e) => {
                e.preventDefault();
                void generate(prompt);
              }}
            >
              <textarea
                value={prompt}
                onChange={(e) => setPrompt(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                    e.preventDefault();
                    void generate(prompt);
                  }
                }}
                rows={3}
                dir="auto"
                placeholder="Or describe your own… e.g. “a table of every number mentioned, with what it refers to”"
                aria-label="Custom doc prompt"
                className={textareaClass}
              />
              <div className="mt-2 flex items-center justify-between gap-2">
                <span className="text-[11.5px] text-faint">Docs run sandboxed, offline, and can play the audio.</span>
                <Button type="submit" size="sm" variant="primary" icon={<Wand2 className="size-3.5" />} disabled={!canGenerate || !prompt.trim()}>
                  Generate
                </Button>
              </div>
            </form>
            {segments.length === 0 && <p className="mt-2 text-[12px] text-muted">Docs can be generated once the transcript is ready.</p>}
          </>
        )}
      </PanelSection>

      {(pending || docs.length > 0) && (
        <PanelSection title={`Docs${docs.length ? ` · ${docs.length}` : ""}`}>
          <div className="space-y-2">
            {pending && <GeneratingCard requestId={pending.requestId} prompt={pending.prompt} />}
            {docs.map((d) => (
              <button
                key={d.id}
                type="button"
                onClick={() => setOpenDoc(d.id)}
                className="flex w-full items-start gap-3 rounded-xl border border-line bg-surface p-3 text-left transition-colors hover:border-line-strong hover:bg-hover/40"
              >
                <div className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-accent-soft text-accent">
                  <FileCode2 className="size-4" />
                </div>
                <div className="min-w-0 flex-1">
                  <div className="truncate text-[13px] font-medium" dir="auto">
                    {d.title}
                  </div>
                  <div className="mt-0.5 line-clamp-2 text-[12px] text-muted" dir="auto">
                    {d.prompt}
                  </div>
                  <div className="mt-1 text-[11px] text-faint">
                    {formatDate(d.updatedAt)} · {d.model}
                  </div>
                </div>
              </button>
            ))}
          </div>
        </PanelSection>
      )}

      {viewing && <DocViewer doc={viewing} onClose={() => setOpenDoc(null)} />}
    </div>
  );
}
