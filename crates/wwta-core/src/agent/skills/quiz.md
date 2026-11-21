---
name: quiz
description: Multiple-choice quiz with instant feedback, score and links to the answers.
---
# Quiz

A short multiple-choice quiz that checks understanding of the recording.

## Gather
1. `get_overview` for the main ideas.
2. Read the transcript (whole, or section by section) so every question and its answer come
   from what was actually said. Note the segment id of each answer.

## Content
```js
const QUESTIONS = [
  { q: "…?", options: ["…", "…", "…", "…"], answer: 2, why: "short explanation", seg: 57 },
];
```
- About 8 questions spread over the whole recording, easy to hard.
- 4 options each; wrong options are plausible, about the same length as the right one, and
  clearly wrong given the recording. Vary the position of the right answer.
- `why` is one sentence explaining the right answer.

## Layout
- One question at a time with a progress bar ("Question 3 of 8").
- Options are full-width buttons. After a click: lock the options, color the right one green
  and a wrong pick red, show `why` and a "▶ hear the answer" button (`WWTA.seek`), then a
  "Next" button.
- At the end: the score, a short message, a list of the questions answered wrong (each with
  its hear-the-answer button) and "Try again" (resets and shuffles the options).
