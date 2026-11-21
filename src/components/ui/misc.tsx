import type { ReactNode } from "react";
import clsx from "clsx";
import { Loader2 } from "lucide-react";

export function Spinner({ className }: { className?: string }) {
  return <Loader2 className={clsx("size-4 animate-spin text-muted", className)} aria-hidden />;
}

/** `value` in 0..1; omit for an indeterminate bar. */
export function ProgressBar({
  value,
  className,
  tone = "accent",
  label,
}: {
  value?: number | null;
  className?: string;
  tone?: "accent" | "ok" | "warn" | "danger";
  label?: string;
}) {
  const indeterminate = value == null || !Number.isFinite(value);
  const pct = indeterminate ? 0 : Math.max(0, Math.min(1, value)) * 100;
  const color = { accent: "bg-accent", ok: "bg-ok", warn: "bg-warn", danger: "bg-danger" }[tone];
  return (
    <div
      role="progressbar"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={indeterminate ? undefined : Math.round(pct)}
      className={clsx("relative h-1.5 w-full overflow-hidden rounded-full bg-sunken", className)}
    >
      {indeterminate ? (
        <div className={clsx("absolute inset-y-0 w-1/3 rounded-full animate-[indeterminate_1.2s_ease-in-out_infinite]", color)} />
      ) : (
        <div className={clsx("h-full rounded-full transition-[width] duration-300", color)} style={{ width: `${pct}%` }} />
      )}
    </div>
  );
}

type Tone = "neutral" | "accent" | "ok" | "warn" | "danger";
const TONES: Record<Tone, string> = {
  neutral: "bg-sunken text-muted",
  accent: "bg-accent-soft text-accent",
  ok: "bg-ok-soft text-ok",
  warn: "bg-warn-soft text-warn",
  danger: "bg-danger-soft text-danger",
};

export function Badge({ tone = "neutral", children, className, title }: { tone?: Tone; children: ReactNode; className?: string; title?: string }) {
  return (
    <span
      title={title}
      className={clsx("inline-flex items-center gap-1 rounded-md px-1.5 py-0.5 text-[11.5px] font-medium leading-4 whitespace-nowrap", TONES[tone], className)}
    >
      {children}
    </span>
  );
}

export function Kbd({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <kbd
      className={clsx(
        "inline-flex h-5 min-w-5 items-center justify-center rounded border border-line bg-sunken px-1 font-sans text-[11px] font-medium text-muted",
        className,
      )}
    >
      {children}
    </kbd>
  );
}

export function Switch({
  checked,
  onChange,
  label,
  disabled,
  id,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label: string;
  disabled?: boolean;
  id?: string;
}) {
  return (
    <button
      id={id}
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={clsx(
        "relative inline-flex h-5 w-9 shrink-0 items-center rounded-full transition-colors disabled:opacity-50",
        checked ? "bg-accent" : "bg-line-strong",
      )}
    >
      <span
        className={clsx(
          "inline-block size-4 rounded-full bg-white shadow transition-transform",
          checked ? "translate-x-[18px]" : "translate-x-0.5",
        )}
      />
    </button>
  );
}

export function EmptyState({
  icon,
  title,
  children,
  action,
  className,
}: {
  icon?: ReactNode;
  title: ReactNode;
  children?: ReactNode;
  action?: ReactNode;
  className?: string;
}) {
  return (
    <div className={clsx("flex flex-col items-center justify-center px-6 py-10 text-center", className)}>
      {icon && <div className="mb-3 flex size-11 items-center justify-center rounded-xl bg-sunken text-muted [&>svg]:size-5">{icon}</div>}
      <div className="text-[14px] font-semibold">{title}</div>
      {children && <div className="mt-1 max-w-sm text-[13px] text-muted">{children}</div>}
      {action && <div className="mt-4">{action}</div>}
    </div>
  );
}

/** Segmented control (radio group styled as pills). */
export function Segmented<T extends string>({
  value,
  options,
  onChange,
  label,
  size = "md",
}: {
  value: T;
  options: { value: T; label: ReactNode }[];
  onChange: (v: T) => void;
  label: string;
  size?: "sm" | "md";
}) {
  return (
    <div role="radiogroup" aria-label={label} className="inline-flex rounded-lg bg-sunken p-0.5">
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          role="radio"
          aria-checked={value === o.value}
          onClick={() => onChange(o.value)}
          className={clsx(
            "rounded-md font-medium transition-colors",
            size === "sm" ? "px-2 py-0.5 text-[12px]" : "px-3 py-1 text-[12.5px]",
            value === o.value ? "bg-surface text-fg shadow-sm" : "text-muted hover:text-fg",
          )}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

export const inputClass =
  "h-8 w-full rounded-lg border border-line bg-surface px-2.5 text-[13px] text-fg placeholder:text-faint outline-none transition-colors focus:border-accent focus:ring-2 focus:ring-accent/20 disabled:opacity-50";

export const textareaClass =
  "w-full rounded-lg border border-line bg-surface px-3 py-2 text-[13px] text-fg placeholder:text-faint outline-none transition-colors focus:border-accent focus:ring-2 focus:ring-accent/20 resize-none";
