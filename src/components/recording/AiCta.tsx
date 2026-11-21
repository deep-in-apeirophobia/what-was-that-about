import type { ReactNode } from "react";
import { KeyRound, Sparkles } from "lucide-react";
import { useApp } from "../../store/app";
import { Button } from "../ui/Button";

/** Friendly placeholder shown in AI features when no OpenRouter key is configured. */
export function NoKeyCta({ what, compact }: { what: string; compact?: boolean }) {
  const navigate = useApp((s) => s.navigate);
  if (compact) {
    return (
      <div className="flex items-center gap-3 rounded-lg bg-sunken/70 px-3 py-2.5 text-[12.5px]">
        <KeyRound className="size-4 shrink-0 text-accent" />
        <span className="min-w-0 flex-1 text-muted">Add an OpenRouter API key to {what}.</span>
        <Button size="sm" onClick={() => navigate({ name: "settings" })}>
          Open settings
        </Button>
      </div>
    );
  }
  return (
    <div className="mx-4 my-6 rounded-xl border border-dashed border-line-strong p-5 text-center">
      <div className="mx-auto flex size-10 items-center justify-center rounded-xl bg-accent-soft text-accent">
        <Sparkles className="size-5" />
      </div>
      <div className="mt-3 text-[13.5px] font-semibold">AI features need an API key</div>
      <p className="mx-auto mt-1 max-w-xs text-[12.5px] text-muted">
        Paste an OpenRouter key in Settings to {what}. A cheap model costs about a cent per recording, and only text is sent.
      </p>
      <Button className="mt-4" variant="primary" icon={<KeyRound className="size-3.5" />} onClick={() => navigate({ name: "settings" })}>
        Add API key
      </Button>
    </div>
  );
}

export function PanelSection({ title, children, action }: { title: string; children: ReactNode; action?: ReactNode }) {
  return (
    <section className="border-b border-line px-4 py-4 last:border-b-0">
      <div className="mb-2 flex items-center justify-between gap-2">
        <h3 className="text-[11px] font-semibold uppercase tracking-wide text-faint">{title}</h3>
        {action}
      </div>
      {children}
    </section>
  );
}
