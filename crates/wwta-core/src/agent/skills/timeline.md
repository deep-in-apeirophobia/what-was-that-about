---
name: timeline
description: Timeline of chapters and key moments that follows the audio as it plays.
---
# Timeline

A vertical timeline of the recording. Each item is a moment worth jumping to. The item
being played is highlighted while the audio runs.

## Gather
1. `get_overview` for the chapters (they give the skeleton: title, segment range, summary).
2. Read the transcript part by part (or chapter by chapter with
   `read_transcript_section`) and after each part add 1-4 key moments: a claim, a number,
   a decision, a story, a question.

## Content
```js
const ITEMS = [
  { seg: 0,  title: "…", text: "one or two sentences", chapter: true },
  { seg: 14, title: "…", text: "…" },
];
```
- Chapters are the big markers; moments are smaller dots between them.
- Titles 2-6 words. Text says what happens, not "they talk about".
- Sort by segment id. Use real segment ids only.

## Layout
- A vertical line along the inline-start side (`border-inline-start`) with dots on it. Each
  item: time button (from `timeButton`), title, text.
- Chapter items get a bigger dot and bold title; moments a small dot.
- Clicking anywhere on an item seeks to its segment.
- `WWTA.onTime(t => …)`: the current item is the last one whose segment starts at or before
  `t`. Give it `--accent-soft` background and scroll it into view with
  `scrollIntoView({ block: "nearest", behavior: "smooth" })` only when it changes.
- Optional: a thin progress bar at the top showing `t / WWTA.recording.durationSec`.
