---
name: flashcards
description: Flip cards with a question on the front and the answer on the back.
---
# Flashcards

Cards for remembering what was said. The front asks, the back answers and offers a button
that plays the moment the answer comes from.

## Gather
1. `get_overview` for the key points.
2. Read the transcript part by part and after each part add the facts worth remembering
   (definitions, names, numbers, reasons, steps, conclusions); aim for 10-20 cards in
   total.

## Content
```js
const CARDS = [
  { q: "…?", a: "…", seg: 42 },
];
```
- One fact per card. Questions short and specific ("Why did X…?", "What is Y?"), answers
  one or two sentences in plain words.
- Every card has the segment id where the answer is said.
- Write in the language of the doc (Persian by default).

## Layout
- One card at a time, large and centered, with a counter "3 / 15" and previous/next buttons.
  Arrow keys and the space bar work too (space flips).
- Flip with a 3D rotate (`transform: rotateY(180deg)`, `backface-visibility: hidden`, about
  400 ms). In RTL the "next" arrow points left.
- The back shows the answer and a "▶ hear it" button that calls `WWTA.seek`.
- Under the card: "Shuffle" and "Show all" (a grid of all cards, both sides visible).
- Remember which cards the user marked "I knew it" in a variable (no localStorage needed)
  and show the count.
