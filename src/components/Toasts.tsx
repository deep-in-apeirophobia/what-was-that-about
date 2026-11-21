import clsx from "clsx";
import { AlertCircle, CheckCircle2, Info, X } from "lucide-react";
import { useToasts } from "../store/toasts";

const ICONS = {
  success: <CheckCircle2 className="size-4 text-ok" aria-hidden />,
  error: <AlertCircle className="size-4 text-danger" aria-hidden />,
  info: <Info className="size-4 text-accent" aria-hidden />,
};

export function Toasts() {
  const toasts = useToasts((s) => s.toasts);
  const dismiss = useToasts((s) => s.dismiss);
  return (
    <div
      className="pointer-events-none fixed right-4 bottom-32 z-[60] flex w-[360px] max-w-[calc(100vw-32px)] flex-col gap-2"
      aria-live="polite"
      role="region"
      aria-label="Notifications"
    >
      {toasts.map((t) => (
        <div
          key={t.id}
          role={t.kind === "error" ? "alert" : "status"}
          className={clsx(
            "pointer-events-auto flex items-start gap-2.5 rounded-xl border bg-surface p-3 shadow-pop animate-slide-up",
            t.kind === "error" ? "border-danger/30" : "border-line",
          )}
        >
          <div className="mt-0.5">{ICONS[t.kind]}</div>
          <div className="min-w-0 flex-1">
            <div className="text-[13px] font-medium" dir="auto">
              {t.title}
            </div>
            {t.message && (
              <div className="mt-0.5 text-[12.5px] break-words text-muted" dir="auto">
                {t.message}
              </div>
            )}
            {t.action && (
              <button
                type="button"
                onClick={() => {
                  t.action!.onClick();
                  dismiss(t.id);
                }}
                className="mt-1.5 text-[12.5px] font-medium text-accent hover:underline"
              >
                {t.action.label}
              </button>
            )}
          </div>
          <button
            type="button"
            aria-label="Dismiss notification"
            onClick={() => dismiss(t.id)}
            className="-mt-0.5 -mr-0.5 rounded-md p-1 text-faint hover:bg-hover hover:text-fg"
          >
            <X className="size-3.5" />
          </button>
        </div>
      ))}
    </div>
  );
}
