---
name: base
description: Rules and building blocks every doc follows (always in the prompt).
---
# Making a doc

A doc is ONE self-contained HTML file. The app shows it in a sandboxed frame next to the
audio player. Your final answer is that file and nothing else.

## Hard rules
- Start with `<!doctype html>` and end with `</html>`. No markdown fences, no commentary.
- Inline all CSS and JavaScript. No external resources: no CDNs, web fonts, image URLs,
  fetch/XHR, iframes or forms. The frame's security policy blocks them and the doc breaks.
- Don't paste the transcript into the HTML. Read it at runtime from `WWTA.segments`. Embed
  only what you write yourself (titles, explanations, questions) and the segment ids you
  refer to.
- Anything that refers to a moment in the audio is clickable and calls `WWTA.seek(seconds)`.
- Persian is written right to left: use `<html dir="rtl" lang="fa">` for a Persian doc,
  and `dir="auto"` on elements that show transcript text.
- Keep the file under about 60 KB.

## The WWTA bridge (already defined; don't redefine it)
```js
WWTA.recording   // { id, title, durationSec }
WWTA.segments    // [{ id, start, end, text }]  the whole transcript, times in seconds
WWTA.notes       // [{ start, end, body, color }]  the user's notes (markdown text)
WWTA.analysis    // null, or { title, summary, keyPoints[], chapters[{ title, startSegment,
                 //   endSegment, start, end, summary, tags[] }], tags[{ name, description,
                 //   segments[] }], actionItems[], questions[] }
WWTA.seek(seconds)                        // jump the player there and play
WWTA.play(); WWTA.pause();
WWTA.onTime((seconds, isPlaying) => {})   // called again and again while the audio plays
```

## Skeleton
Start from this and fill in `<main>` and the script. It already handles light/dark mode,
Persian fonts and clickable times.
```html
<!doctype html>
<html lang="fa" dir="rtl">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Short title</title>
<style>
:root { color-scheme: light dark; --bg:#fafaf9; --card:#fff; --fg:#1c1917; --muted:#78716c;
  --line:#e7e5e4; --accent:#4f46e5; --accent-soft:#eef2ff; }
@media (prefers-color-scheme: dark) { :root { --bg:#1c1917; --card:#292524; --fg:#f5f5f4;
  --muted:#a8a29e; --line:#44403c; --accent:#a5b4fc; --accent-soft:#312e81; } }
* { box-sizing: border-box; }
body { margin: 0; background: var(--bg); color: var(--fg);
  font: 15px/1.75 Vazirmatn, Tahoma, system-ui, sans-serif; }
main { max-width: 860px; margin: 0 auto; padding: 28px 20px 56px; }
h1 { font-size: 22px; margin: 0 0 4px; }
h2 { font-size: 17px; margin: 28px 0 10px; }
.muted { color: var(--muted); }
.card { background: var(--card); border: 1px solid var(--line); border-radius: 12px; padding: 14px 16px; }
button { font: inherit; color: inherit; cursor: pointer; }
.time { font: 12px ui-monospace, monospace; color: var(--accent); background: var(--accent-soft);
  border: 0; border-radius: 6px; padding: 2px 7px; direction: ltr; }
</style>
</head>
<body>
<main>
  <h1>…</h1>
  <p class="muted">…</p>
  <div id="list"></div>
</main>
<script>
const seg = id => WWTA.segments.find(s => s.id === id);
const fmt = t => Math.floor(t / 60) + ":" + String(Math.floor(t % 60)).padStart(2, "0");
function timeButton(segId) {
  const s = seg(segId);
  const b = document.createElement("button");
  b.className = "time";
  b.textContent = s ? fmt(s.start) : "–";
  b.onclick = () => { if (s) WWTA.seek(s.start); };
  return b;
}
</script>
</body>
</html>
```
For an English doc use `<html lang="en" dir="ltr">`.

## Design
- A clear title and a one-line description, then the content. Generous spacing, the colors
  from the skeleton, no heavy shadows, subtle transitions at most.
- It has to work 360 px wide and on a large screen.
- The script must run without errors: define functions before you use them, check that a
  segment exists before using it, and don't assume `WWTA.analysis` is set.
- Put the content you wrote into a JavaScript array near the top of the script and render it
  from there. That keeps the HTML short and the data easy to check.
