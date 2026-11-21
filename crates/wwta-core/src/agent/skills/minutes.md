---
name: minutes
description: Meeting minutes - participants, decisions, open questions and action items.
---
# Meeting minutes

Clean minutes of a meeting, class or conversation. Every line links to where it was said.

## Gather
1. `get_overview` for the summary, chapters and action items (if the recording was analyzed).
2. Read the whole transcript (or every section in order). Minutes must not miss decisions,
   so don't rely on search alone.
3. `get_notes`: the user's notes often mark what mattered.

## Content
```js
const MINUTES = {
  summary: "two or three sentences",
  participants: ["names or roles mentioned"],      // empty if nobody is named
  topics: [ { title: "…", seg: 0, points: [ { text: "…", seg: 4 } ] } ],
  decisions: [ { text: "…", seg: 31 } ],
  actions: [ { task: "…", owner: "…", due: "…", seg: 40 } ],   // owner/due "" if not said
  open: [ { text: "…", seg: 77 } ],
};
```
- Write what was said, in neutral wording. Don't invent owners or deadlines.
- Decisions are things agreed on; actions are tasks someone will do.

## Layout
- Header: title, date line from `WWTA.recording`, the summary.
- Sections in this order: Participants, Decisions, Action items, Topics discussed,
  Open questions. Skip a section when it is empty.
- Action items as a table (task, owner, due, time button) with a checkbox per row that only
  strikes the row through.
- Every item ends with a small time button.
- A "Copy as text" button that copies plain-text minutes with `navigator.clipboard.writeText`
  (wrap it in try/catch; show "Copied" for a moment).
