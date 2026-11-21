import { Fragment } from "react";
import { MOD } from "../lib/platform";
import { useApp } from "../store/app";
import { Dialog } from "./ui/Dialog";
import { Kbd } from "./ui/misc";

const GROUPS: { title: string; items: [string[], string][] }[] = [
  {
    title: "Everywhere",
    items: [
      [[MOD, "K"], "Search everything"],
      [["?"], "Show this help"],
      [["Esc"], "Close dialogs and panels"],
    ],
  },
  {
    title: "Playback",
    items: [
      [["Space"], "Play / pause"],
      [["←"], "Back 5 seconds"],
      [["→"], "Forward 5 seconds"],
      [["Shift", "←"], "Previous sentence"],
      [["Shift", "→"], "Next sentence"],
    ],
  },
  {
    title: "Transcript",
    items: [
      [[MOD, "F"], "Find in transcript"],
      [["Enter"], "Next match"],
      [["Shift", "Enter"], "Previous match"],
      [["Click"], "Play from that sentence"],
      [[MOD, "Enter"], "Save a note"],
    ],
  },
];

export function ShortcutsHelp() {
  const open = useApp((s) => s.shortcutsOpen);
  const setOpen = useApp((s) => s.setShortcutsOpen);
  return (
    <Dialog open={open} onClose={() => setOpen(false)} title="Keyboard shortcuts" size="md">
      <div className="grid gap-5 sm:grid-cols-2">
        {GROUPS.map((g) => (
          <section key={g.title} className={g.title === "Playback" ? "sm:row-span-2" : undefined}>
            <h3 className="mb-2 text-[11px] font-medium uppercase tracking-wide text-faint">{g.title}</h3>
            <dl className="space-y-2">
              {g.items.map(([keys, label]) => (
                <div key={label} className="flex items-center justify-between gap-3 text-[13px]">
                  <dt className="text-muted">{label}</dt>
                  <dd className="flex shrink-0 items-center gap-1">
                    {keys.map((k, i) => (
                      <Fragment key={k}>
                        {i > 0 && <span className="text-[11px] text-faint">+</span>}
                        <Kbd>{k}</Kbd>
                      </Fragment>
                    ))}
                  </dd>
                </div>
              ))}
            </dl>
          </section>
        ))}
      </div>
    </Dialog>
  );
}
