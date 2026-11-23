---
name: glossary
description: Searchable glossary of names, terms and jargon with short definitions.
---
# Glossary

An A-Z list of the names, terms, abbreviations and jargon in the recording, with short
definitions and a link to where each first appears.

## Gather
1. `get_overview` for the topics.
2. Read the transcript part by part and after each part add every term a newcomer might
   not know: people, organizations, places, technical words, abbreviations, foreign
   words.
3. Use `search` to find a term's first mention when you're not sure.

## Content
```js
const TERMS = [
  { term: "…", kind: "person", def: "one or two sentences, from the recording", seg: 8 },
];
```
- `kind` is one of: person, organization, place, term, abbreviation.
- Definitions explain the term as used in the recording. If the recording doesn't explain
  it, give a short general definition.
- 15-40 terms. Sort alphabetically (Persian order for Persian terms: use
  `localeCompare(b, "fa")`).

## Layout
- A search box at the top that filters as you type (match term and definition, ignore case).
- Filter chips for the kinds, with counts.
- Letter headings (the first letter of each term) with the terms under them, each as a row:
  term in bold, kind as a small tag, definition, time button.
- "No matches" message when the filter hides everything.
