import { useEffect, useMemo, useState, type ReactNode } from "react";
import clsx from "clsx";
import { Check, Eye, EyeOff, Loader2, RefreshCw, Search } from "lucide-react";
import { api } from "../lib/api";
import { errorMessage } from "../lib/errors";
import { formatPrice } from "../lib/format";
import { openExternal } from "../lib/platform";
import type { AiLanguage, LlmModelInfo, Settings, Theme, WhisperQuant } from "../lib/types";
import { useModels } from "../store/models";
import { useSettings } from "../store/settings";
import { toast } from "../store/toasts";
import { Button } from "../components/ui/Button";
import { Badge, Segmented, Spinner, Switch, inputClass } from "../components/ui/misc";

const QUICK_PICKS = ["deepseek/deepseek-v4-flash", "openai/gpt-6-luna", "anthropic/claude-haiku-5.5"];

function Section({ title, description, children }: { title: string; description?: string; children: ReactNode }) {
  return (
    <section className="rounded-xl border border-line bg-surface shadow-card">
      <div className="border-b border-line px-5 py-3.5">
        <h2 className="text-[14px] font-semibold">{title}</h2>
        {description && <p className="mt-0.5 text-[12.5px] text-muted">{description}</p>}
      </div>
      <div className="divide-y divide-line">{children}</div>
    </section>
  );
}

function Row({
  label,
  description,
  htmlFor,
  children,
  stacked,
}: {
  label: string;
  description?: ReactNode;
  htmlFor?: string;
  children: ReactNode;
  stacked?: boolean;
}) {
  return (
    <div className={clsx("px-5 py-3.5", stacked ? "space-y-2.5" : "flex items-center gap-6")}>
      <div className="min-w-0 flex-1">
        <label htmlFor={htmlFor} className="block text-[13px] font-medium">
          {label}
        </label>
        {description && <div className="mt-0.5 text-[12.5px] text-muted">{description}</div>}
      </div>
      <div className={clsx(!stacked && "shrink-0")}>{children}</div>
    </div>
  );
}

function ApiKeyField({ value, onSave }: { value: string; onSave: (v: string) => Promise<boolean> }) {
  const [draft, setDraft] = useState(value);
  const [show, setShow] = useState(false);
  const [testing, setTesting] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);
  useEffect(() => setDraft(value), [value]);
  const dirty = draft.trim() !== value;

  const test = async () => {
    setTesting(true);
    setResult(null);
    try {
      setResult({ ok: true, text: await api.testApiKey(draft.trim()) });
    } catch (e) {
      setResult({ ok: false, text: errorMessage(e) });
    } finally {
      setTesting(false);
    }
  };

  return (
    <div className="space-y-2">
      <div className="flex gap-2">
        <div className="relative flex-1">
          <input
            id="api-key"
            type={show ? "text" : "password"}
            value={draft}
            autoComplete="off"
            spellCheck={false}
            placeholder="sk-or-v1-…"
            onChange={(e) => {
              setDraft(e.target.value);
              setResult(null);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter" && dirty) void onSave(draft.trim());
            }}
            className={clsx(inputClass, "pr-9 font-mono")}
          />
          <button
            type="button"
            onClick={() => setShow((s) => !s)}
            aria-label={show ? "Hide key" : "Show key"}
            className="absolute top-1/2 right-1.5 -translate-y-1/2 rounded p-1 text-faint hover:text-fg"
          >
            {show ? <EyeOff className="size-3.5" /> : <Eye className="size-3.5" />}
          </button>
        </div>
        <Button onClick={() => void test()} loading={testing} disabled={!draft.trim()}>
          Test
        </Button>
        <Button variant="primary" disabled={!dirty} onClick={() => void onSave(draft.trim())}>
          Save
        </Button>
      </div>
      {result && (
        <div className={clsx("rounded-lg px-3 py-2 text-[12.5px]", result.ok ? "bg-ok-soft text-ok" : "bg-danger-soft text-danger")}>
          {result.text}
        </div>
      )}
    </div>
  );
}

function LlmPicker({ value, onChange }: { value: string; onChange: (id: string) => void }) {
  const [models, setModels] = useState<LlmModelInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  const [custom, setCustom] = useState("");

  const load = () => {
    setError(null);
    setModels(null);
    api
      .listLlmModels()
      .then(setModels)
      .catch((e) => {
        setModels([]);
        setError(errorMessage(e));
      });
  };
  useEffect(load, []);

  const visible = useMemo(() => {
    const q = filter.trim().toLowerCase();
    const list = models ?? [];
    return (q ? list.filter((m) => m.id.toLowerCase().includes(q) || m.name.toLowerCase().includes(q)) : list).slice(0, 200);
  }, [models, filter]);

  const byId = (id: string) => models?.find((m) => m.id === id);

  return (
    <div className="space-y-2.5">
      <div className="flex flex-wrap gap-1.5">
        {QUICK_PICKS.map((id) => {
          const m = byId(id);
          return (
            <button
              key={id}
              type="button"
              aria-pressed={value === id}
              onClick={() => onChange(id)}
              className={clsx(
                "flex items-center gap-1.5 rounded-lg border px-2.5 py-1.5 text-[12.5px] transition-colors",
                value === id ? "border-accent bg-accent-soft text-accent" : "border-line hover:border-line-strong",
              )}
            >
              {value === id && <Check className="size-3.5" />}
              <span className="font-medium">{m?.name ?? id}</span>
              {m && <span className="text-[11.5px] text-faint">{formatPrice(m.promptPrice)} in</span>}
            </button>
          );
        })}
      </div>

      <div className="overflow-hidden rounded-lg border border-line">
        <div className="relative border-b border-line">
          <Search className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-faint" />
          <input
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="Search OpenRouter models…"
            aria-label="Search language models"
            className="h-8 w-full bg-transparent pr-2.5 pl-8 text-[13px] outline-none placeholder:text-faint"
          />
        </div>
        <div className="max-h-56 overflow-y-auto" role="listbox" aria-label="Language models">
          {models == null ? (
            <div className="flex justify-center py-5">
              <Spinner />
            </div>
          ) : error ? (
            <div className="flex items-center justify-between gap-3 px-3 py-3 text-[12.5px] text-danger">
              <span>Couldn't load the model list: {error}</span>
              <Button size="sm" icon={<RefreshCw className="size-3" />} onClick={load}>
                Retry
              </Button>
            </div>
          ) : visible.length === 0 ? (
            <div className="px-3 py-4 text-center text-[12.5px] text-muted">No models match.</div>
          ) : (
            visible.map((m) => (
              <button
                key={m.id}
                type="button"
                role="option"
                aria-selected={m.id === value}
                onClick={() => onChange(m.id)}
                className={clsx(
                  "flex w-full items-center gap-3 px-3 py-1.5 text-left text-[12.5px]",
                  m.id === value ? "bg-accent-soft" : "hover:bg-hover",
                )}
              >
                <Check className={clsx("size-3.5 shrink-0 text-accent", m.id !== value && "invisible")} />
                <span className="min-w-0 flex-1">
                  <span className="block truncate font-medium">{m.name}</span>
                  <span className="block truncate font-mono text-[11px] text-faint">{m.id}</span>
                </span>
                <span className="shrink-0 text-right text-[11.5px] text-muted tabular-nums">
                  {formatPrice(m.promptPrice)} / {formatPrice(m.completionPrice)}
                  <span className="block text-[10.5px] text-faint">in / out per 1M</span>
                </span>
              </button>
            ))
          )}
        </div>
      </div>
      <div className="flex items-center gap-2">
        <input
          value={custom}
          onChange={(e) => setCustom(e.target.value)}
          placeholder="…or type any model id, e.g. vendor/model-name"
          aria-label="Custom model id"
          spellCheck={false}
          onKeyDown={(e) => {
            if (e.key === "Enter" && custom.trim()) {
              onChange(custom.trim());
              setCustom("");
            }
          }}
          className={clsx(inputClass, "font-mono")}
        />
        <Button
          disabled={!custom.trim()}
          onClick={() => {
            onChange(custom.trim());
            setCustom("");
          }}
        >
          Use
        </Button>
      </div>
      <div className="text-[12px] text-muted">
        Current: <span className="font-mono">{value}</span>
      </div>
    </div>
  );
}

function TextSetting({
  id,
  value,
  onSave,
  placeholder,
  type = "text",
  min,
  max,
  className,
}: {
  id: string;
  value: string;
  onSave: (v: string) => void;
  placeholder?: string;
  type?: "text" | "number";
  min?: number;
  max?: number;
  className?: string;
}) {
  const [draft, setDraft] = useState(value);
  useEffect(() => setDraft(value), [value]);
  const commit = () => {
    if (draft.trim() !== value) onSave(draft.trim());
  };
  return (
    <input
      id={id}
      type={type}
      min={min}
      max={max}
      value={draft}
      placeholder={placeholder}
      spellCheck={false}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") commit();
      }}
      className={clsx(inputClass, className)}
    />
  );
}

export function SettingsView() {
  const settings = useSettings((s) => s.settings);
  const system = useSettings((s) => s.system);
  const patch = useSettings((s) => s.patch);
  const models = useModels((s) => s.models);
  const [saving, setSaving] = useState(false);
  const [savedAt, setSavedAt] = useState<number | null>(null);

  if (!settings) {
    return (
      <div className="flex h-full items-center justify-center">
        <Spinner />
      </div>
    );
  }

  const update = async (partial: Partial<Settings>) => {
    setSaving(true);
    const ok = await patch(partial);
    setSaving(false);
    if (ok) setSavedAt(Date.now());
    return ok;
  };

  const gpuUsable = !system || (system.gpuBackend !== "none" && system.gpuAvailable);

  return (
    <div className="flex h-full flex-col">
      <header className="flex items-center border-b border-line px-6 py-3.5">
        <div className="mr-auto">
          <h1 className="text-[17px] font-semibold">Settings</h1>
          <p className="text-[12.5px] text-muted">Changes are saved automatically.</p>
        </div>
        <span className="flex items-center gap-1.5 text-[12px] text-faint" aria-live="polite">
          {saving ? (
            <>
              <Loader2 className="size-3.5 animate-spin" /> Saving…
            </>
          ) : savedAt ? (
            <>
              <Check className="size-3.5 text-ok" /> Saved
            </>
          ) : null}
        </span>
      </header>

      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto max-w-3xl space-y-5 p-6">
          <Section title="AI features" description="Summaries, chapters, topics, Q&A and docs use OpenRouter. Only transcript text is sent.">
            <Row
              label="OpenRouter API key"
              htmlFor="api-key"
              stacked
              description={
                <>
                  Stored on this computer only.{" "}
                  <button
                    type="button"
                    className="text-accent hover:underline"
                    onClick={() => openExternal("https://openrouter.ai/keys").catch((e) => toast.error("Couldn't open the link", e))}
                  >
                    Get a key
                  </button>
                </>
              }
            >
              <ApiKeyField
                value={settings.openrouterApiKey}
                onSave={async (v) => {
                  const ok = await update({ openrouterApiKey: v });
                  if (ok) toast.success(v ? "API key saved" : "API key removed");
                  return ok;
                }}
              />
            </Row>
            <Row label="Language model" stacked description="Cheaper models work well for summaries and Q&A. Prices are USD per million tokens.">
              <LlmPicker value={settings.llmModel} onChange={(llmModel) => void update({ llmModel })} />
            </Row>
            <Row label="Embedding model" htmlFor="embedding-model" description="Used for semantic search. Changing it requires re-indexing.">
              <TextSetting
                id="embedding-model"
                value={settings.embeddingModel}
                placeholder="baai/bge-m3"
                onSave={(v) => void update({ embeddingModel: v || "baai/bge-m3" })}
                className="w-64 font-mono"
              />
            </Row>
            <Row label="AI output language" description="“Auto” answers in the language of the recording.">
              <Segmented<AiLanguage>
                label="AI output language"
                value={settings.aiLanguage}
                onChange={(aiLanguage) => void update({ aiLanguage })}
                options={[
                  { value: "auto", label: "Auto" },
                  { value: "en", label: "English" },
                  { value: "fa", label: "فارسی" },
                ]}
              />
            </Row>
            <Row label="Analyze automatically" description="Run AI analysis as soon as a transcription finishes.">
              <Switch
                label="Analyze automatically"
                checked={settings.autoAnalyze}
                onChange={(autoAnalyze) => void update({ autoAnalyze })}
              />
            </Row>
          </Section>

          <Section title="Transcription">
            <Row label="Default speech model" htmlFor="default-model">
              <select
                id="default-model"
                value={settings.defaultModelId}
                onChange={(e) => void update({ defaultModelId: e.target.value })}
                className={clsx(inputClass, "w-60")}
              >
                {models.map((m) => (
                  <option key={m.id} value={m.id}>
                    {m.name}
                    {m.status === "ready" ? "" : " (not installed)"}
                  </option>
                ))}
                {!models.some((m) => m.id === settings.defaultModelId) && <option value={settings.defaultModelId}>{settings.defaultModelId}</option>}
              </select>
            </Row>
            <Row
              label="Use GPU"
              description={
                gpuUsable ? (
                  "Much faster for Taraneh-WLarge."
                ) : (
                  <span className="flex items-center gap-1.5">
                    <Badge tone="warn">No usable GPU</Badge> Transcription runs on the CPU.
                  </span>
                )
              }
            >
              <Switch label="Use GPU" checked={settings.useGpu && gpuUsable} disabled={!gpuUsable} onChange={(useGpu) => void update({ useGpu })} />
            </Row>
            <Row label="CPU threads" htmlFor="threads" description={`0 = automatic${system ? ` (this computer has ${system.cpuCores} cores)` : ""}.`}>
              <TextSetting
                id="threads"
                type="number"
                min={0}
                max={256}
                value={String(settings.threads)}
                onSave={(v) => {
                  const n = Math.round(Number(v));
                  if (!Number.isFinite(n) || n < 0) {
                    toast.error("Threads must be 0 or a positive number");
                    return;
                  }
                  void update({ threads: n });
                }}
                className="w-24 text-right tabular-nums"
              />
            </Row>
            <Row label="Whisper model format" description="Applies when converting Taraneh-WLarge. q8_0 is smaller and faster; f16 is marginally more accurate.">
              <Segmented<WhisperQuant>
                label="Whisper model format"
                value={settings.whisperQuant}
                onChange={(whisperQuant) => void update({ whisperQuant })}
                options={[
                  { value: "q8_0", label: "q8_0 · smaller" },
                  { value: "f16", label: "f16" },
                ]}
              />
            </Row>
            <Row label="Keep source weights" description="Keep the original safetensors after conversion (uses ~3 GB more disk).">
              <Switch
                label="Keep source weights"
                checked={settings.keepSourceWeights}
                onChange={(keepSourceWeights) => void update({ keepSourceWeights })}
              />
            </Row>
            <Row label="Persian digits" description="Write spoken numbers as digits (Shenava).">
              <Switch label="Persian digits" checked={settings.persianDigits} onChange={(persianDigits) => void update({ persianDigits })} />
            </Row>
            <Row label="Precise word timings" description="More accurate karaoke highlighting with Whisper; transcription is slower.">
              <Switch
                label="Precise word timings"
                checked={settings.preciseWordTimings}
                onChange={(preciseWordTimings) => void update({ preciseWordTimings })}
              />
            </Row>
          </Section>

          <Section title="Library">
            <Row label="Copy imported files into the library" description="Keeps recordings playable even if you move or delete the originals.">
              <Switch
                label="Copy imported files into the library"
                checked={settings.copyIntoLibrary}
                onChange={(copyIntoLibrary) => void update({ copyIntoLibrary })}
              />
            </Row>
          </Section>

          <Section
            title="Privacy"
            description="Anonymous and on by default. Never includes recordings, transcripts, notes, titles, file names, search text or keys."
          >
            <Row
              label="Share anonymous usage statistics"
              description="Counts of app opens, which models and file formats are used, recording lengths and speeds. Helps decide what to improve."
            >
              <Switch
                label="Share anonymous usage statistics"
                checked={settings.analyticsEnabled}
                onChange={(analyticsEnabled) => void update({ analyticsEnabled })}
              />
            </Row>
            <Row
              label="Send crash reports"
              description="Error details and crash dumps with personal paths removed. Turning it back on takes effect after a restart."
            >
              <Switch
                label="Send crash reports"
                checked={settings.crashReportsEnabled}
                onChange={(crashReportsEnabled) => void update({ crashReportsEnabled })}
              />
            </Row>
          </Section>

          <Section title="Appearance">
            <Row label="Theme">
              <Segmented<Theme>
                label="Theme"
                value={settings.theme}
                onChange={(theme) => void update({ theme })}
                options={[
                  { value: "system", label: "System" },
                  { value: "light", label: "Light" },
                  { value: "dark", label: "Dark" },
                ]}
              />
            </Row>
          </Section>
        </div>
      </div>
    </div>
  );
}
