import { useEffect, useLayoutEffect, useRef, useState } from "react";
import clsx from "clsx";
import { Ban, StickyNote } from "lucide-react";
import { formatTimeRange } from "../../lib/format";
import { NOTE_COLOR_LABELS, noteColor, noteTint } from "../../lib/notes";
import { MOD } from "../../lib/platform";
import { NOTE_COLORS, type NoteColor } from "../../lib/types";
import { Button } from "../ui/Button";
import { textareaClass } from "../ui/misc";

const OPTIONS: (NoteColor | null)[] = [null, ...NOTE_COLORS];

/** Radio group of color dots (arrow keys move between them). */
export function NoteColorPicker({ value, onChange }: { value: NoteColor | null; onChange: (c: NoteColor | null) => void }) {
  const refs = useRef<(HTMLButtonElement | null)[]>([]);
  const onKeyDown = (e: React.KeyboardEvent) => {
    const delta = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[e.key];
    if (!delta) return;
    e.preventDefault();
    const rtl = getComputedStyle(e.currentTarget).direction === "rtl" && (e.key === "ArrowLeft" || e.key === "ArrowRight");
    const i = OPTIONS.indexOf(value);
    const next = (i + (rtl ? -delta : delta) + OPTIONS.length) % OPTIONS.length;
    onChange(OPTIONS[next]);
    refs.current[next]?.focus();
  };
  return (
    <div role="radiogroup" aria-label="Note color" className="flex items-center gap-0.5" onKeyDown={onKeyDown}>
      {OPTIONS.map((c, i) => {
        const checked = value === c;
        const label = c ? NOTE_COLOR_LABELS[c] : "No color";
        return (
          <button
            key={c ?? "none"}
            ref={(el) => {
              refs.current[i] = el;
            }}
            type="button"
            role="radio"
            aria-checked={checked}
            aria-label={label}
            title={label}
            tabIndex={checked ? 0 : -1}
            onClick={() => onChange(c)}
            className="flex size-6 items-center justify-center rounded-full outline-none focus-visible:ring-2 focus-visible:ring-accent/50"
          >
            <span
              className={clsx(
                "flex size-4 items-center justify-center rounded-full transition-shadow",
                !c && "border-[1.5px] border-line-strong",
                checked && "ring-2 ring-offset-2 ring-offset-surface",
              )}
              style={{
                background: c ? noteColor(c) : undefined,
                ["--tw-ring-color" as string]: c ? noteColor(c) : "var(--faint)",
              }}
            >
              {!c && <Ban className="size-2.5 text-faint" aria-hidden />}
            </span>
          </button>
        );
      })}
    </div>
  );
}

export interface NoteComposerProps {
  start: number;
  end: number;
  initialBody?: string;
  initialColor?: NoteColor | null;
  /** "New note" vs "Edit note" heading. */
  editing?: boolean;
  onSave: (body: string, color: NoteColor | null) => void;
  onCancel: () => void;
  className?: string;
}

/**
 * Inline editor for a note: markdown textarea, color, Save (Ctrl/Cmd+Enter) and
 * Cancel (Esc). Inherits the surrounding direction; text fields use `dir="auto"`.
 */
export function NoteComposer({
  start,
  end,
  initialBody = "",
  initialColor = null,
  editing,
  onSave,
  onCancel,
  className,
}: NoteComposerProps) {
  const [body, setBody] = useState(initialBody);
  const [color, setColor] = useState<NoteColor | null>(initialColor);
  const root = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    const el = input.current;
    if (!el) return;
    el.focus({ preventScroll: true });
    el.setSelectionRange(el.value.length, el.value.length);
    root.current?.scrollIntoView({ block: "nearest", behavior: "smooth" });
  }, []);

  useLayoutEffect(() => {
    const el = input.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight + 2, 320)}px`;
  }, [body]);

  const save = () => {
    const text = body.trim();
    if (text) onSave(text, color);
  };

  return (
    <div
      ref={root}
      data-note-ui
      role="group"
      aria-label={editing ? "Edit note" : "New note"}
      onKeyDown={(e) => {
        if (e.nativeEvent.isComposing) return;
        if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
          e.preventDefault();
          save();
        } else if (e.key === "Escape") {
          e.preventDefault();
          e.stopPropagation();
          onCancel();
        }
      }}
      className={clsx("rounded-xl border bg-surface p-2.5 text-[13px] shadow-card animate-pop-in", className)}
      style={{ borderColor: noteTint(color, 45, "var(--line)") }}
    >
      <div className="mb-1.5 flex items-center gap-1.5 px-0.5 text-[11.5px] text-muted">
        <StickyNote className="size-3.5" style={{ color: noteColor(color) }} aria-hidden />
        <span className="font-medium" dir="ltr">
          {editing ? "Edit note" : "New note"}
        </span>
        <span className="text-faint tabular-nums" dir="ltr">
          {formatTimeRange(start, end)}
        </span>
      </div>
      <textarea
        ref={input}
        value={body}
        rows={2}
        dir="auto"
        aria-label="Note text"
        placeholder="Write a note… Markdown and links work"
        onChange={(e) => setBody(e.target.value)}
        className={clsx(textareaClass, "min-h-16 font-fa text-[13.5px] leading-relaxed placeholder:font-sans")}
      />
      <div className="mt-2 flex flex-wrap items-center gap-2">
        <NoteColorPicker value={color} onChange={setColor} />
        <div className="ms-auto flex items-center gap-1.5">
          <span className="me-1 text-[11px] text-faint" dir="ltr">
            {MOD}+Enter to save · Esc to cancel
          </span>
          <Button size="sm" variant="ghost" onClick={onCancel}>
            Cancel
          </Button>
          <Button size="sm" variant="primary" onClick={save} disabled={!body.trim()}>
            Save
          </Button>
        </div>
      </div>
    </div>
  );
}
