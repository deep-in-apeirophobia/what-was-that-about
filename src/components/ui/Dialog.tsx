import { useEffect, useRef, type ReactNode } from "react";
import { createPortal } from "react-dom";
import clsx from "clsx";
import { X } from "lucide-react";
import { Button, IconButton } from "./Button";

let openCount = 0;
/** True while any modal overlay is open (global shortcuts back off). */
export const isOverlayOpen = () => openCount > 0;

const FOCUSABLE = 'button:not([disabled]), [href], input:not([disabled]), select, textarea, [tabindex]:not([tabindex="-1"])';

export interface DialogProps {
  open: boolean;
  onClose: () => void;
  title?: ReactNode;
  description?: ReactNode;
  children?: ReactNode;
  footer?: ReactNode;
  size?: "sm" | "md" | "lg" | "xl" | "full";
  /** Hide the default header (render your own). */
  bare?: boolean;
  className?: string;
  /** Position near the top (command palette style) instead of centred. */
  top?: boolean;
  initialFocus?: React.RefObject<HTMLElement | null>;
}

const SIZES = {
  sm: "max-w-sm",
  md: "max-w-lg",
  lg: "max-w-2xl",
  xl: "max-w-5xl",
  full: "max-w-none w-[calc(100vw-48px)] h-[calc(100vh-48px)]",
};

export function Dialog({
  open,
  onClose,
  title,
  description,
  children,
  footer,
  size = "md",
  bare,
  className,
  top,
  initialFocus,
}: DialogProps) {
  const panel = useRef<HTMLDivElement>(null);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  useEffect(() => {
    if (!open) return;
    openCount++;
    const previouslyFocused = document.activeElement as HTMLElement | null;
    const focusTarget =
      initialFocus?.current ??
      panel.current?.querySelector<HTMLElement>("input:not([disabled]):not([type=hidden]), textarea:not([disabled])") ??
      panel.current;
    requestAnimationFrame(() => focusTarget?.focus());

    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        e.preventDefault();
        onCloseRef.current();
        return;
      }
      if (e.key === "Tab" && panel.current) {
        const items = Array.from(panel.current.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
          (el) => el.offsetParent !== null,
        );
        if (!items.length) return;
        const first = items[0];
        const last = items[items.length - 1];
        if (e.shiftKey && document.activeElement === first) {
          e.preventDefault();
          last.focus();
        } else if (!e.shiftKey && document.activeElement === last) {
          e.preventDefault();
          first.focus();
        }
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => {
      openCount--;
      window.removeEventListener("keydown", onKey, true);
      previouslyFocused?.focus?.();
    };
  }, [open, initialFocus]);

  if (!open) return null;

  return createPortal(
    <div
      className={clsx(
        "fixed inset-0 z-50 flex justify-center bg-black/35 backdrop-blur-[2px] animate-fade-in p-6",
        top ? "items-start pt-[12vh]" : "items-center",
      )}
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        ref={panel}
        role="dialog"
        aria-modal="true"
        aria-label={typeof title === "string" ? title : undefined}
        tabIndex={-1}
        className={clsx(
          "relative flex w-full flex-col overflow-hidden rounded-xl border border-line bg-surface shadow-pop animate-pop-in outline-none",
          SIZES[size],
          size !== "full" && "max-h-[calc(100vh-48px)]",
          className,
        )}
      >
        {!bare && (title || description) && (
          <div className="flex items-start gap-3 px-5 pt-4 pb-3">
            <div className="min-w-0 flex-1">
              {title && <h2 className="text-[15px] font-semibold">{title}</h2>}
              {description && <p className="mt-1 text-[13px] text-muted">{description}</p>}
            </div>
            <IconButton label="Close" onClick={onClose} className="-mr-1.5 -mt-0.5">
              <X className="size-4" />
            </IconButton>
          </div>
        )}
        {children && <div className={clsx("min-h-0 flex-1", !bare && "overflow-y-auto px-5 pb-4")}>{children}</div>}
        {footer && <div className="flex justify-end gap-2 border-t border-line bg-sunken/50 px-5 py-3">{footer}</div>}
      </div>
    </div>,
    document.body,
  );
}

export interface ConfirmDialogProps {
  open: boolean;
  title: string;
  message: ReactNode;
  confirmLabel?: string;
  danger?: boolean;
  busy?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}

export function ConfirmDialog({
  open,
  title,
  message,
  confirmLabel = "Confirm",
  danger,
  busy,
  onConfirm,
  onCancel,
}: ConfirmDialogProps) {
  const confirmRef = useRef<HTMLButtonElement>(null);
  return (
    <Dialog
      open={open}
      onClose={onCancel}
      title={title}
      size="sm"
      initialFocus={confirmRef}
      footer={
        <>
          <Button onClick={onCancel}>Cancel</Button>
          <Button ref={confirmRef} variant={danger ? "danger" : "primary"} loading={busy} onClick={onConfirm}>
            {confirmLabel}
          </Button>
        </>
      }
    >
      <div className="text-[13px] text-muted">{message}</div>
    </Dialog>
  );
}
