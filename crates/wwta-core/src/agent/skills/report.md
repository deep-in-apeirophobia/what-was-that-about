---
name: report
description: Anything else - tables, summaries, study guides, comparisons, lists, dashboards.
---
# Custom report

For requests that don't fit the other guides: a table of numbers mentioned, a study guide,
a comparison of opinions, a list of recommendations, a reading list, a summary for a
particular audience…

## Plan first
Before writing, decide:
1. What is the one question this doc answers for the user?
2. What are the items (rows, sections, cards) and which fields does each one have?
3. Which part of the recording do you need? A table of "every X" needs the whole
   transcript; a summary of one topic needs `search` and the sections around the hits.

## Gather
- `get_overview` first, then read what you need. Collect each item with its segment id.
- Be complete when the user says "every" or "all": read the whole transcript, section by
  section if it's long.

## Content
Put the items into one JavaScript array of objects with the fields you planned, each with a
`seg`. Render everything from that array.

## Layout
Pick the simplest form that fits:
- **Table** for items with the same fields (numbers, dates, people). Sticky header, sortable
  columns (click a header), a time button in the last column.
- **Sections with cards** for longer text (study guide, recommendations).
- **Two or more columns** for comparisons (stack them on narrow screens).
- **Stat tiles + list** for a dashboard: 3-4 big numbers on top, details below.
Add a search box when there are more than about 15 items.
