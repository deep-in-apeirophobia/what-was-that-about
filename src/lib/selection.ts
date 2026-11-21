// Map a text selection in the transcript to the sentences it covers.
//
// Each segment row carries `data-seg-id`, and the element holding the sentence text
// carries `data-seg-text`. A selection usually starts or ends somewhere odd (a
// timestamp, a chapter heading, offset 0 of the next row after a triple-click), so
// instead of trusting the anchor/focus nodes we intersect the range with every
// sentence and keep the ones where actual text is selected.

/** Ids of the segments whose text the range really covers, in document order. */
export function segmentIdsInRange(range: Range, root: ParentNode): number[] {
  if (range.collapsed) return [];
  const doc = range.startContainer.ownerDocument ?? document;
  const out: number[] = [];
  for (const el of root.querySelectorAll<HTMLElement>("[data-seg-text]")) {
    if (!range.intersectsNode(el)) {
      // Rows are in document order: once past the selection, stop.
      if (out.length) break;
      continue;
    }
    const part = doc.createRange();
    part.selectNodeContents(el);
    if (range.compareBoundaryPoints(Range.START_TO_START, part) > 0) part.setStart(range.startContainer, range.startOffset);
    if (range.compareBoundaryPoints(Range.END_TO_END, part) < 0) part.setEnd(range.endContainer, range.endOffset);
    if (!part.toString().trim()) continue;
    const id = Number(el.closest("[data-seg-id]")?.getAttribute("data-seg-id"));
    if (Number.isInteger(id)) out.push(id);
  }
  return out;
}
