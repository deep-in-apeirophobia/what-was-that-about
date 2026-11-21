import { useEffect, useMemo, useState } from "react";
import clsx from "clsx";
import { ArrowRight, AudioLines, CheckCircle2, Cpu, Download, Gpu, KeyRound, Sparkles } from "lucide-react";
import type { ModelInfo } from "../lib/types";
import { formatBytes } from "../lib/format";
import { api } from "../lib/api";
import { openExternal } from "../lib/platform";
import { useApp } from "../store/app";
import { hasReadyModel, useModels } from "../store/models";
import { useSettings } from "../store/settings";
import { toast } from "../store/toasts";
import { errorMessage } from "../lib/errors";
import { Button } from "./ui/Button";
import { Dialog } from "./ui/Dialog";
import { Badge, inputClass, Spinner, Switch } from "./ui/misc";
import { ManualInstall, ModelProgressView } from "./ModelCard";
import { importViaDialog } from "./Sidebar";

type Step = "model" | "ai" | "done";

function ModelChoice({
  model,
  selected,
  recommended,
  onSelect,
}: {
  model: ModelInfo;
  selected: boolean;
  recommended: boolean;
  onSelect: () => void;
}) {
  const progress = useModels((s) => s.progress[model.id]);
  const installing = model.status === "downloading" || model.status === "converting" || !!progress;
  return (
    <button
      type="button"
      role="radio"
      aria-checked={selected}
      onClick={onSelect}
      className={clsx(
        "relative flex w-full flex-col rounded-xl border p-4 text-left transition-[border-color,box-shadow]",
        selected ? "border-accent ring-3 ring-accent/15" : "border-line hover:border-line-strong",
      )}
    >
      <div className="flex items-center gap-2.5">
        <div
          className={clsx(
            "flex size-8 items-center justify-center rounded-lg",
            model.engine === "whisper" ? "bg-accent-soft text-accent" : "bg-ok-soft text-ok",
          )}
        >
          {model.engine === "whisper" ? <Gpu className="size-4" /> : <Cpu className="size-4" />}
        </div>
        <div className="min-w-0 flex-1">
          <div className="text-[14px] font-semibold">{model.name}</div>
          <div className="text-[12px] text-muted">{model.recommendedFor}</div>
        </div>
        {recommended && <Badge tone="warn">Recommended</Badge>}
      </div>
      <p className="mt-2.5 text-[12.5px] leading-relaxed text-muted">{model.description}</p>
      <div className="mt-3 flex items-center gap-3 text-[12px] text-faint">
        <span>{formatBytes(model.downloadBytes)} download</span>
        {model.status === "ready" && (
          <span className="flex items-center gap-1 font-medium text-ok">
            <CheckCircle2 className="size-3.5" /> Installed
          </span>
        )}
        {installing && <span className="font-medium text-accent">Installing…</span>}
      </div>
    </button>
  );
}

function ModelStep({ onNext }: { onNext: () => void }) {
  const models = useModels((s) => s.models);
  const progress = useModels((s) => s.progress);
  const install = useModels((s) => s.install);
  const cancel = useModels((s) => s.cancel);
  const system = useSettings((s) => s.system);
  const settings = useSettings((s) => s.settings);
  const patch = useSettings((s) => s.patch);
  const recommendedId = useMemo(() => {
    const engine = system?.gpuAvailable ? "whisper" : "shenava";
    return models.find((m) => m.engine === engine)?.id ?? models[0]?.id;
  }, [models, system]);
  const [selectedId, setSelectedId] = useState<string | undefined>(undefined);
  const [manual, setManual] = useState(false);
  const selected = models.find((m) => m.id === (selectedId ?? recommendedId));
  const selProgress = selected ? progress[selected.id] : undefined;
  const installing = !!selected && (selected.status === "downloading" || selected.status === "converting" || !!selProgress);
  const anyReady = hasReadyModel(models);

  const startInstall = async () => {
    if (!selected) return;
    if (settings && settings.defaultModelId !== selected.id) void patch({ defaultModelId: selected.id });
    await install(selected.id);
  };

  return (
    <div className="space-y-4">
      <p className="text-[13px] leading-relaxed text-muted">
        Transcription runs entirely on this computer. Pick a speech model to download — you can add the other one later from{" "}
        <b className="font-medium text-fg">Models</b>.
        {system && (
          <>
            {" "}
            {system.gpuAvailable ? (
              <>We found a GPU ({system.gpuDevices[0]?.name ?? system.gpuBackend}), so Taraneh will be fast.</>
            ) : (
              <>No usable GPU was found, so the CPU model is the better fit.</>
            )}
          </>
        )}
      </p>
      {!models.length ? (
        <div className="flex justify-center py-6">
          <Spinner />
        </div>
      ) : (
        <div role="radiogroup" aria-label="Speech model" className="grid gap-3 sm:grid-cols-2">
          {models.map((m) => (
            <ModelChoice
              key={m.id}
              model={m}
              recommended={m.id === recommendedId}
              selected={m.id === selected?.id}
              onSelect={() => setSelectedId(m.id)}
            />
          ))}
        </div>
      )}

      {selected && installing && <ModelProgressView progress={selProgress} />}

      {selected && manual && !installing && selected.status !== "ready" && <ManualInstall model={selected} />}

      <div className="flex items-center justify-between gap-3 pt-1">
        {selected && selected.status !== "ready" && !installing ? (
          <button type="button" onClick={() => setManual((m) => !m)} className="text-[12.5px] font-medium text-muted hover:text-fg">
            {manual ? "Hide manual install" : "I already downloaded the files"}
          </button>
        ) : (
          <span className="text-[12px] text-faint">{installing ? "You can continue while it downloads." : ""}</span>
        )}
        <div className="flex gap-2">
          {installing && (
            <Button onClick={() => selected && void cancel(selected.id)} size="md">
              Cancel
            </Button>
          )}
          {selected && selected.status !== "ready" && !installing && (
            <Button variant="primary" icon={<Download className="size-3.5" />} onClick={() => void startInstall()}>
              Install {selected.name}
            </Button>
          )}
          {(installing || anyReady || selected?.status === "ready") && (
            <Button variant={anyReady ? "primary" : "secondary"} onClick={onNext}>
              Continue <ArrowRight className="size-3.5" />
            </Button>
          )}
        </div>
      </div>
    </div>
  );
}

function AiStep({ onNext }: { onNext: () => void }) {
  const settings = useSettings((s) => s.settings);
  const patch = useSettings((s) => s.patch);
  const [key, setKey] = useState(settings?.openrouterApiKey ?? "");
  const [testing, setTesting] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);

  const test = async () => {
    setTesting(true);
    setResult(null);
    try {
      const text = await api.testApiKey(key.trim());
      setResult({ ok: true, text });
    } catch (e) {
      setResult({ ok: false, text: errorMessage(e) });
    } finally {
      setTesting(false);
    }
  };

  const save = async () => {
    if (key.trim() && (await patch({ openrouterApiKey: key.trim() }))) toast.success("API key saved");
    onNext();
  };

  return (
    <div className="space-y-4">
      <div className="flex gap-3 rounded-xl bg-sunken/70 p-4">
        <Sparkles className="mt-0.5 size-4 shrink-0 text-accent" />
        <div className="text-[13px] leading-relaxed text-muted">
          With an <b className="font-medium text-fg">OpenRouter</b> API key, a low-cost language model can summarize recordings, split them
          into chapters, answer questions with citations and build interactive study docs. Only transcript text is sent — never audio.
          This is optional.
        </div>
      </div>
      <label className="block">
        <span className="mb-1.5 flex items-center justify-between text-[12.5px] font-medium">
          OpenRouter API key
          <button
            type="button"
            onClick={() => openExternal("https://openrouter.ai/keys").catch((e) => toast.error("Couldn't open the link", e))}
            className="font-normal text-accent hover:underline"
          >
            Get a key
          </button>
        </span>
        <div className="flex gap-2">
          <input
            type="password"
            autoComplete="off"
            spellCheck={false}
            value={key}
            onChange={(e) => {
              setKey(e.target.value);
              setResult(null);
            }}
            placeholder="sk-or-…"
            className={inputClass}
          />
          <Button onClick={() => void test()} loading={testing} disabled={!key.trim()}>
            Test
          </Button>
        </div>
      </label>
      {result && (
        <div className={clsx("rounded-lg px-3 py-2 text-[12.5px]", result.ok ? "bg-ok-soft text-ok" : "bg-danger-soft text-danger")}>
          {result.text}
        </div>
      )}
      <div className="flex justify-between pt-1">
        <Button variant="ghost" onClick={onNext}>
          Skip for now
        </Button>
        <Button variant="primary" icon={<KeyRound className="size-3.5" />} onClick={() => void save()} disabled={!key.trim()}>
          Save key
        </Button>
      </div>
    </div>
  );
}

function TelemetryNotice() {
  const settings = useSettings((s) => s.settings);
  const patch = useSettings((s) => s.patch);
  if (!settings) return null;
  const on = settings.analyticsEnabled && settings.crashReportsEnabled;
  return (
    <div className="flex items-start gap-3 rounded-lg border border-line bg-sunken/50 p-3 text-left">
      <p className="flex-1 text-[12px] leading-relaxed text-muted">
        The app sends anonymous usage statistics and crash reports to help improve it — never recordings,
        transcripts, notes, names or keys. You can change this anytime in Settings → Privacy.
      </p>
      <Switch
        label="Share anonymous usage statistics and crash reports"
        checked={on}
        onChange={(v) => void patch({ analyticsEnabled: v, crashReportsEnabled: v })}
      />
    </div>
  );
}

function DoneStep({ onClose }: { onClose: () => void }) {
  const models = useModels((s) => s.models);
  const ready = hasReadyModel(models);
  return (
    <div className="space-y-4 text-center">
      <div className="mx-auto flex size-12 items-center justify-center rounded-2xl bg-ok-soft text-ok">
        <CheckCircle2 className="size-6" />
      </div>
      <div>
        <div className="text-[15px] font-semibold">{ready ? "You're all set" : "Almost there"}</div>
        <p className="mx-auto mt-1 max-w-sm text-[13px] text-muted">
          {ready
            ? "Import a recording — or drag files anywhere into the window — and transcribe it."
            : "The model keeps installing in the background. Meanwhile you can import recordings; transcribe them once it's ready."}
        </p>
      </div>
      <TelemetryNotice />
      <div className="flex justify-center gap-2">
        <Button onClick={onClose}>Close</Button>
        <Button
          variant="primary"
          onClick={() => {
            onClose();
            void importViaDialog();
          }}
        >
          Import audio
        </Button>
      </div>
    </div>
  );
}

const STEPS: { id: Step; label: string }[] = [
  { id: "model", label: "Speech model" },
  { id: "ai", label: "AI features" },
  { id: "done", label: "Done" },
];

/** First-run setup, shown while no speech model is ready (until dismissed). */
export function Onboarding() {
  const modelsLoaded = useModels((s) => s.loaded);
  const models = useModels((s) => s.models);
  const dismissed = useApp((s) => s.onboardingDismissed);
  const dismiss = useApp((s) => s.dismissOnboarding);
  const [open, setOpen] = useState(false);
  const [step, setStep] = useState<Step>("model");
  const needed = modelsLoaded && models.length > 0 && !hasReadyModel(models) && !dismissed;

  // Latch open: once shown, stay until the user closes it (even after a model becomes ready).
  useEffect(() => {
    if (needed) setOpen(true);
  }, [needed]);

  const close = () => {
    void api.trackUi(step === "done" ? "onboarding completed" : "onboarding skipped", {
      step,
      model_ready: hasReadyModel(models),
    });
    setOpen(false);
    dismiss();
  };

  return (
    <Dialog open={open} onClose={close} size="lg" bare>
      <div className="flex flex-col">
        <div className="flex items-center gap-3 border-b border-line px-6 py-4">
          <div className="flex size-9 items-center justify-center rounded-xl bg-accent text-accent-fg">
            <AudioLines className="size-5" />
          </div>
          <div className="min-w-0 flex-1">
            <h2 className="text-[15px] font-semibold">Welcome to What Was That About</h2>
            <p className="text-[12.5px] text-muted">Transcribe Persian audio, then search it, skim it and ask it questions.</p>
          </div>
          <Button variant="ghost" size="sm" onClick={close}>
            Skip setup
          </Button>
        </div>
        <ol className="flex gap-1 px-6 pt-4" aria-label="Setup steps">
          {STEPS.map((s, i) => {
            const idx = STEPS.findIndex((x) => x.id === step);
            return (
              <li key={s.id} className="flex flex-1 flex-col gap-1.5" aria-current={s.id === step ? "step" : undefined}>
                <div className={clsx("h-1 rounded-full", i <= idx ? "bg-accent" : "bg-sunken")} />
                <span className={clsx("text-[11.5px] font-medium", i <= idx ? "text-fg" : "text-faint")}>{s.label}</span>
              </li>
            );
          })}
        </ol>
        <div className="max-h-[70vh] overflow-y-auto px-6 pt-4 pb-6">
          {step === "model" && <ModelStep onNext={() => setStep("ai")} />}
          {step === "ai" && <AiStep onNext={() => setStep("done")} />}
          {step === "done" && <DoneStep onClose={close} />}
        </div>
      </div>
    </Dialog>
  );
}
