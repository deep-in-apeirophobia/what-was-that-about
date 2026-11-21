import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import clsx from "clsx";
import { Check } from "lucide-react";

export interface MenuItem {
  key: string;
  label: ReactNode;
  icon?: ReactNode;
  hint?: ReactNode;
  checked?: boolean;
  disabled?: boolean;
  danger?: boolean;
  onSelect: () => void;
}

export interface MenuProps {
  trigger: (props: {
    ref: React.Ref<HTMLButtonElement>;
    onClick: () => void;
    "aria-haspopup": "menu";
    "aria-expanded": boolean;
    "aria-controls": string;
  }) => ReactNode;
  items: (MenuItem | "separator" | { heading: string })[];
  align?: "start" | "end";
  side?: "bottom" | "top";
  className?: string;
}

/** A small dropdown menu with keyboard navigation and outside-click dismissal. */
export function Menu({ trigger, items, align = "start", side = "bottom", className }: MenuProps) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const button = useRef<HTMLButtonElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const id = useId();

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (!root.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        setOpen(false);
        button.current?.focus();
      }
    };
    document.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey, true);
    requestAnimationFrame(() => list.current?.querySelector<HTMLElement>("[role=menuitem]:not([disabled]), [role=menuitemcheckbox]:not([disabled])")?.focus());
    return () => {
      document.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey, true);
    };
  }, [open]);

  const onListKey = (e: React.KeyboardEvent) => {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp" && e.key !== "Home" && e.key !== "End") return;
    e.preventDefault();
    const els = Array.from(
      list.current?.querySelectorAll<HTMLElement>("[role=menuitem]:not([disabled]), [role=menuitemcheckbox]:not([disabled])") ?? [],
    );
    if (!els.length) return;
    const i = els.indexOf(document.activeElement as HTMLElement);
    const next =
      e.key === "Home" ? 0 : e.key === "End" ? els.length - 1 : e.key === "ArrowDown" ? (i + 1) % els.length : (i - 1 + els.length) % els.length;
    els[next].focus();
  };

  return (
    <div ref={root} className={clsx("relative inline-flex", className)}>
      {trigger({
        ref: button,
        onClick: () => setOpen((o) => !o),
        "aria-haspopup": "menu",
        "aria-expanded": open,
        "aria-controls": id,
      })}
      {open && (
        <div
          ref={list}
          id={id}
          role="menu"
          onKeyDown={onListKey}
          className={clsx(
            "absolute z-40 min-w-[200px] max-h-[60vh] overflow-y-auto rounded-lg border border-line bg-surface p-1 shadow-pop animate-pop-in",
            align === "end" ? "right-0" : "left-0",
            side === "bottom" ? "top-full mt-1" : "bottom-full mb-1",
          )}
        >
          {items.map((item, i) => {
            if (item === "separator") return <div key={`sep-${i}`} className="my-1 h-px bg-line" role="separator" />;
            if ("heading" in item)
              return (
                <div key={`h-${i}`} className="px-2.5 pt-2 pb-1 text-[11px] font-medium uppercase tracking-wide text-faint">
                  {item.heading}
                </div>
              );
            return (
              <button
                key={item.key}
                type="button"
                role={item.checked !== undefined ? "menuitemcheckbox" : "menuitem"}
                aria-checked={item.checked}
                disabled={item.disabled}
                onClick={() => {
                  setOpen(false);
                  item.onSelect();
                }}
                className={clsx(
                  "flex w-full items-center gap-2.5 rounded-md px-2.5 py-1.5 text-left text-[13px] outline-none disabled:opacity-40",
                  item.danger ? "text-danger hover:bg-danger-soft focus:bg-danger-soft" : "text-fg hover:bg-hover focus:bg-hover",
                )}
              >
                {item.checked !== undefined ? (
                  <Check className={clsx("size-3.5 shrink-0", !item.checked && "invisible")} />
                ) : (
                  item.icon && <span className="shrink-0 text-muted [&>svg]:size-4">{item.icon}</span>
                )}
                <span className="min-w-0 flex-1 truncate">{item.label}</span>
                {item.hint && <span className="shrink-0 text-[11.5px] text-faint">{item.hint}</span>}
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
