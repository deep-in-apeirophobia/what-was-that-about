import { useMemo } from "react";
import { splitByRanges } from "../lib/text";

export function HighlightedText({
  text,
  ranges,
  markClassName = "rounded-[3px] bg-mark px-px text-inherit",
}: {
  text: string;
  ranges: readonly (readonly [number, number])[];
  markClassName?: string;
}) {
  const pieces = useMemo(() => splitByRanges(text, ranges), [text, ranges]);
  return (
    <>
      {pieces.map((p, i) =>
        p.highlighted ? (
          <mark key={i} className={markClassName}>
            {p.text}
          </mark>
        ) : (
          <span key={i}>{p.text}</span>
        ),
      )}
    </>
  );
}
