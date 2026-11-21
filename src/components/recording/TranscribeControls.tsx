import { useState } from "react";
import { AudioLines, ChevronDown, RefreshCw } from "lucide-react";
import type { ModelInfo, Recording } from "../../lib/types";
import { useApp } from "../../store/app";
import { useModels } from "../../store/models";
import { useSettings } from "../../store/settings";
import { Button } from "../ui/Button";
import { Menu, type MenuItem } from "../ui/Menu";
import { startTranscription } from "./actions";

export function readyModels(models: ModelInfo[]): ModelInfo[] {
  return models.filter((m) => m.status === "ready");
}

/** Model id to use by default: settings.defaultModelId when ready, else the first ready model. */
export function useDefaultModelId(): string | null {
  const models = useModels((s) => s.models);
  const def = useSettings((s) => s.settings?.defaultModelId);
  const ready = readyModels(models);
  return ready.find((m) => m.id === def)?.id ?? ready[0]?.id ?? null;
}

/** "Transcribe" button with an attached model picker (only ready models). */
export function TranscribeButton({
  recording,
  size = "md",
  variant,
}: {
  recording: Recording;
  size?: "sm" | "md" | "lg";
  variant?: "primary" | "secondary";
}) {
  const models = useModels((s) => s.models);
  const navigate = useApp((s) => s.navigate);
  const defaultId = useDefaultModelId();
  const [chosen, setChosen] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const ready = readyModels(models);
  const modelId = chosen && ready.some((m) => m.id === chosen) ? chosen : defaultId;
  const model = ready.find((m) => m.id === modelId);
  const again = recording.status === "ready" || recording.status === "error";
  const v = variant ?? (again ? "secondary" : "primary");

  if (!ready.length) {
    return (
      <Button size={size} variant={v} icon={<AudioLines className="size-3.5" />} onClick={() => navigate({ name: "models" })}>
        Install a speech model
      </Button>
    );
  }

  const run = async () => {
    setBusy(true);
    await startTranscription(recording.id, modelId);
    setBusy(false);
  };

  const items: MenuItem[] = ready.map((m) => ({
    key: m.id,
    label: m.name,
    hint: m.recommendedFor,
    checked: m.id === modelId,
    onSelect: () => setChosen(m.id),
  }));

  return (
    <div className="inline-flex">
      <Button
        size={size}
        variant={v}
        loading={busy}
        icon={again ? <RefreshCw className="size-3.5" /> : <AudioLines className="size-3.5" />}
        onClick={() => void run()}
        className="rounded-r-none"
        title={model ? `Transcribe with ${model.name}` : undefined}
      >
        {again ? "Re-transcribe" : "Transcribe"}
        {ready.length > 1 && model && <span className="font-normal opacity-75">· {model.name}</span>}
      </Button>
      <Menu
        align="end"
        items={[{ heading: "Speech model" }, ...items]}
        trigger={(p) => (
          <Button
            {...p}
            size={size}
            variant={v}
            aria-label="Choose speech model"
            className="rounded-l-none border-l border-l-black/10 px-1.5 dark:border-l-white/10"
          >
            <ChevronDown className="size-3.5" />
          </Button>
        )}
      />
    </div>
  );
}
