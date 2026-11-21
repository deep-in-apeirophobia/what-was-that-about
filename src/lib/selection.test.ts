import { beforeEach, describe, expect, it } from "vitest";
import { segmentIdsInRange } from "./selection";

function row(id: number, text: string): string {
  return `<div data-seg-id="${id}"><button>0:0${id}</button><div data-seg-text>${text}</div></div>`;
}

let root: HTMLElement;
beforeEach(() => {
  document.body.innerHTML = `<div id="t">
    ${row(0, "سلام به همه")}
    <div class="chapter">Chapter two</div>
    ${row(1, "هدف جلسه")}
    ${row(2, "Third sentence")}
    <div data-note-id="x">A note card</div>
    ${row(3, "Last one")}
  </div>`;
  root = document.getElementById("t")!;
});

const text = (id: number) => root.querySelector(`[data-seg-id="${id}"] [data-seg-text]`)!.firstChild!;

describe("segmentIdsInRange", () => {
  it("finds the single sentence a selection lies in", () => {
    const r = document.createRange();
    r.setStart(text(1), 1);
    r.setEnd(text(1), 4);
    expect(segmentIdsInRange(r, root)).toEqual([1]);
  });

  it("finds every sentence across rows, even when the ends are outside sentence text", () => {
    const r = document.createRange();
    r.setStart(root.querySelector(".chapter")!.firstChild!, 2);
    r.setEnd(root.querySelector("[data-note-id]")!.firstChild!, 3);
    expect(segmentIdsInRange(r, root)).toEqual([1, 2]);
  });

  it("ignores a row the selection only touches at offset 0", () => {
    const r = document.createRange();
    r.setStart(text(0), 0);
    r.setEnd(text(1), 0);
    expect(segmentIdsInRange(r, root)).toEqual([0]);
  });

  it("ignores selections in timestamps, headings and note cards", () => {
    const r = document.createRange();
    r.selectNodeContents(root.querySelector("[data-note-id]")!);
    expect(segmentIdsInRange(r, root)).toEqual([]);
    const ts = document.createRange();
    ts.selectNodeContents(root.querySelector('[data-seg-id="2"] button')!);
    expect(segmentIdsInRange(ts, root)).toEqual([]);
  });

  it("returns nothing for a collapsed range", () => {
    const r = document.createRange();
    r.setStart(text(2), 2);
    expect(segmentIdsInRange(r, root)).toEqual([]);
  });
});
