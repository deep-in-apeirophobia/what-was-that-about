---
name: mindmap
description: Mind map of the main topics, their subtopics and how they connect.
---
# Mind map

Shows the recording's topics as a tree around one central idea. Clicking a node plays the
moment where that topic is discussed.

## Gather
1. `get_overview` for the summary, chapters and topics.
2. Read the transcript part by part and, after each part, add 2-5 concrete subtopics per
   main topic with the segment where each one starts.

## Content
- Center: the recording's subject in 2-5 words.
- 4-7 main branches (usually the chapters or the biggest topics).
- 2-5 leaves per branch, each a short phrase (2-6 words), with the segment id where it is
  said. Leave out anything you can't tie to a segment.

```js
const MAP = { label: "…", children: [
  { label: "…", seg: 12, children: [ { label: "…", seg: 15 }, { label: "…", seg: 21 } ] },
] };
```

## Layout
Draw it as an indented tree with connecting lines, not a free-floating graph. It stays
readable at any width and right-to-left:
- Each node is a rounded pill button. The center is larger and filled with `--accent`.
- Children sit in a `<ul>` under their parent; draw the connectors with borders on
  `li::before` (use `border-inline-start` so the lines follow the text direction).
- Give each main branch its own soft color (5-7 pastel hues work in light and dark mode
  when used as background with the normal text color).
- Branches can collapse: clicking the small ▸/▾ toggle next to a node hides its children.
  Clicking the label itself calls `WWTA.seek(seg(node.seg).start)`.
- Highlight the node whose segment is closest before the current time with `WWTA.onTime`.
