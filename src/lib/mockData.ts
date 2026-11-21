// Static demo content for the browser mock (see mock.ts). Not used inside Tauri.

import type { Analysis, LlmModelInfo, ModelInfo, Segment, SystemInfo, Word } from "./types";

// ---- deterministic pseudo-random numbers ------------------------------------------

export function seeded(seed: number): () => number {
  let s = seed >>> 0 || 1;
  return () => {
    s ^= s << 13;
    s ^= s >>> 17;
    s ^= s << 5;
    return ((s >>> 0) % 10_000) / 10_000;
  };
}

// ---- transcripts --------------------------------------------------------------------

export const MEETING_LINES = [
  "سلام به همه، ممنون که امروز وقت گذاشتید و توی این جلسه شرکت کردید.",
  "هدف جلسهٔ امروز اینه که برنامهٔ سه ماه آینده رو با هم مرور کنیم و اولویت‌ها رو مشخص کنیم.",
  "اول یه خلاصه از نتایج تحقیقات کاربری می‌گم، بعد می‌ریم سراغ نقشهٔ راه و بودجه.",
  "خب، توی دو هفتهٔ گذشته با حدود بیست نفر از کاربرها مصاحبه کردیم.",
  "مهم‌ترین چیزی که شنیدیم این بود که کاربرها جستجو توی فایل‌های صوتی طولانی رو خیلی سخت می‌دونن.",
  "خیلی‌ها گفتن که بعد از یه جلسهٔ یک‌ساعته، دقیقاً یادشون نمی‌آد کی چی گفته.",
  "یکی از کاربرها می‌گفت هر هفته نزدیک به سه ساعت وقت می‌ذاره تا ضبط جلسه‌ها رو دوباره گوش بده.",
  "نکتهٔ دوم این بود که دقت تبدیل گفتار به متن برای فارسی هنوز براشون یه نگرانی جدیه.",
  "مخصوصاً وقتی چند نفر هم‌زمان حرف می‌زنن یا کیفیت صدا پایینه.",
  "سومین موضوع هم حریم خصوصی بود؛ خیلی‌ها نمی‌خوان فایل‌هاشون رو روی سرورهای خارجی آپلود کنن.",
  "به همین خاطر پردازش محلی روی خود دستگاه برای ما یه مزیت رقابتی واقعیه.",
  "سارا، تو هم نکته‌ای از مصاحبه‌ها داری که بخوای اضافه کنی؟",
  "آره، من فکر می‌کنم خلاصه‌سازی خودکار هم خیلی براشون جذاب بود.",
  "مثلاً اینکه بعد از جلسه یه فهرست از کارهایی که باید انجام بشه داشته باشن.",
  "ولی بعضی‌ها نگران هزینهٔ مدل‌های زبانی بودن و می‌خواستن بدونن ماهانه چقدر خرج می‌کنن.",
  "خیلی خوبه. پس سه محور اصلی داریم: جستجو، دقت و حریم خصوصی.",
  "حالا بریم سراغ نقشهٔ راه سه ماه آینده.",
  "در ماه اول تمرکزمون روی بهبود دقت مدل تبدیل گفتار به متنه.",
  "می‌خوایم مدل بزرگ‌تر رو برای کارت‌های گرافیک بهینه کنیم و یه مدل سبک هم برای پردازنده داشته باشیم.",
  "در ماه دوم، جستجوی معنایی رو اضافه می‌کنیم تا کاربر بتونه با مفهوم جستجو کنه، نه فقط با کلمه.",
  "یعنی اگه کسی بنویسه «تصمیم دربارهٔ بودجه»، بخش‌های مرتبط رو پیدا کنه حتی اگه دقیقاً همین کلمه‌ها گفته نشده باشه.",
  "ماه سوم هم برای خلاصه‌سازی، تقسیم‌بندی موضوعی و پرسش و پاسخ با متن جلسه است.",
  "رضا، از نظر فنی این زمان‌بندی واقع‌بینانه است؟",
  "به نظرم بخش اول و دوم شدنیه، ولی برای ماه سوم یه کم نگرانم.",
  "چون کار با مدل‌های زبانی نیاز به آزمایش زیادی داره، مخصوصاً برای متن فارسی.",
  "پیشنهاد من اینه که یه نسخهٔ آزمایشی رو زودتر به چند تا کاربر بدیم و بازخورد بگیریم.",
  "موافقم، این‌طوری ریسک کمتر می‌شه.",
  "خب، می‌رسیم به موضوع بودجه.",
  "کل بودجهٔ این فصل حدود دویست میلیون تومانه.",
  "بیشترش صرف نیروی انسانی می‌شه و یه بخش کوچیک هم برای سرور و آزمایش مدل‌ها.",
  "در مورد هزینهٔ مدل‌های زبانی، با مدل‌های ارزون‌تر هزینهٔ هر جلسه کمتر از یک سنته.",
  "پس می‌تونیم به کاربرها بگیم که با کلید شخصی خودشون، هزینه‌ها کاملاً شفاف و قابل کنترله.",
  "من پیشنهاد می‌کنم یه بخشی از بودجه رو برای طراحی رابط کاربری کنار بذاریم.",
  "چون اگه کار با برنامه ساده نباشه، بهترین مدل‌ها هم کمکی نمی‌کنن.",
  "قبول دارم. ده درصد بودجه رو برای طراحی در نظر می‌گیریم.",
  "خب، بیایم کارهای بعدی رو جمع‌بندی کنیم.",
  "رضا تا آخر هفته یه نمونهٔ اولیه از جستجوی معنایی آماده می‌کنه.",
  "سارا برای هفتهٔ بعد پنج جلسهٔ تست کاربری هماهنگ می‌کنه.",
  "من هم سند نقشهٔ راه رو به‌روز می‌کنم و برای همه می‌فرستم.",
  "جلسهٔ بعدی سه‌شنبهٔ آینده ساعت ده صبحه. ممنون از همه، خسته نباشید.",
];

export const HAFEZ_LINES = [
  "امروز می‌خوایم دربارهٔ حافظ صحبت کنیم، شاعری که تقریباً در خونهٔ همهٔ ایرانی‌ها دیوانش پیدا می‌شه.",
  "حافظ در قرن هشتم هجری در شیراز زندگی می‌کرد و بیشتر عمرش رو همون‌جا گذروند.",
  "اسم اصلیش شمس‌الدین محمد بود و چون قرآن رو از حفظ داشت، به حافظ معروف شد.",
  "شعر حافظ بیشتر در قالب غزله و هر غزل معمولاً بین هفت تا دوازده بیت داره.",
  "یکی از ویژگی‌های مهم شعرش ایهامه؛ یعنی یک کلمه می‌تونه هم‌زمان چند معنی داشته باشه.",
  "به همین خاطر هر خواننده‌ای می‌تونه برداشت خودش رو از یک غزل داشته باشه.",
  "فال حافظ هم از همین ویژگی می‌آد؛ مردم دیوان رو باز می‌کنن و غزل رو به حال خودشون تعبیر می‌کنن.",
  "مضمون‌های اصلی شعرش عشق، رندی، نقد ریاکاری و شادی در لحظهٔ حاله.",
  "حافظ بارها زاهدان ریاکار رو نقد می‌کنه و صداقت رو بالاتر از ظاهرسازی می‌دونه.",
  "موسیقی شعرش هم خیلی مهمه؛ وزن و قافیه طوری انتخاب شدن که شعر به راحتی به خاطر سپرده بشه.",
  "خیلی از مصرع‌های حافظ امروز به ضرب‌المثل تبدیل شدن و در گفت‌وگوهای روزمره استفاده می‌شن.",
  "گوته، شاعر آلمانی، هم تحت تأثیر حافظ دیوان غربی‌شرقی رو نوشت.",
  "آرامگاه حافظ در شیراز هر سال میزبان هزاران بازدیدکننده از سراسر دنیاست.",
  "جلسهٔ بعد چند غزل معروف رو با هم می‌خونیم و دربارهٔ معنی‌شون بحث می‌کنیم.",
];

/** Used for any recording transcribed in the mock (the podcast and imported files). */
export const PODCAST_LINES = [
  "سلام، به قسمت تازهٔ پادکست «ذهن آرام» خوش اومدید.",
  "امروز می‌خوایم دربارهٔ رابطهٔ خواب و بهره‌وری صحبت کنیم.",
  "تحقیقات نشون می‌ده که بیشتر بزرگسال‌ها به هفت تا نه ساعت خواب نیاز دارن.",
  "ولی خیلی از ما به‌خاطر کار و گوشی موبایل، کمتر از شش ساعت می‌خوابیم.",
  "کم‌خوابی فقط باعث خستگی نمی‌شه؛ تمرکز و حافظه رو هم ضعیف می‌کنه.",
  "یکی از ساده‌ترین راه‌ها اینه که هر شب سر یک ساعت مشخص بخوابیم.",
  "نور آبی صفحه‌نمایش ترشح ملاتونین رو کم می‌کنه، پس بهتره یک ساعت قبل از خواب گوشی رو کنار بذاریم.",
  "قهوه هم تا شش ساعت در بدن می‌مونه؛ پس بعد از ظهر سراغش نرید.",
  "چرت کوتاه بیست دقیقه‌ای وسط روز می‌تونه انرژی رو برگردونه.",
  "ولی چرت طولانی‌تر ممکنه خواب شب رو به هم بزنه.",
  "ورزش منظم، مخصوصاً صبح‌ها، کیفیت خواب رو بهتر می‌کنه.",
  "اتاق خواب باید تاریک، خنک و ساکت باشه.",
  "اگه بیشتر از بیست دقیقه خوابتون نبرد، از تخت بیاید بیرون و یه کار آروم انجام بدید.",
  "یادتون باشه بهره‌وری یعنی کار هوشمندانه، نه کار بیشتر با بدن خسته.",
  "هفتهٔ بعد دربارهٔ مدیریت استرس صحبت می‌کنیم.",
  "ممنون که همراه ما بودید، شب خوبی داشته باشید.",
];

/** Build segments with plausible timings and word-level timestamps from plain lines. */
export function buildSegments(lines: readonly string[], seed: number, startAt = 0.6): Segment[] {
  const rand = seeded(seed);
  const out: Segment[] = [];
  let t = startAt;
  lines.forEach((text, id) => {
    const words = text.split(/\s+/).filter(Boolean);
    const duration = Math.max(1.6, text.length * (0.068 + rand() * 0.014));
    const start = round(t);
    const end = round(t + duration);
    const weights = words.map((w) => w.length + 1.5);
    const totalWeight = weights.reduce((a, b) => a + b, 0);
    const wordTimes: Word[] = [];
    let wt = start;
    words.forEach((w, i) => {
      const span = (weights[i] / totalWeight) * (end - start);
      wordTimes.push({ start: round(wt), end: round(wt + span * 0.9), text: w });
      wt += span;
    });
    out.push({ id, start, end, text, words: wordTimes });
    t = end + 0.25 + rand() * 0.7;
  });
  return out;
}

function round(n: number): number {
  return Math.round(n * 100) / 100;
}

/** Speech-like normalized peaks (0..1) for a segment list. */
export function buildPeaks(segments: readonly Segment[], durationSec: number, perSecond: number, seed: number): number[] {
  const rand = seeded(seed);
  const n = Math.ceil(durationSec * perSecond);
  const peaks = new Array<number>(n).fill(0);
  for (let i = 0; i < n; i++) peaks[i] = 0.02 + rand() * 0.04;
  for (const s of segments) {
    for (const w of s.words) {
      const a = Math.floor(w.start * perSecond);
      const b = Math.min(n, Math.ceil(w.end * perSecond));
      const loud = 0.45 + rand() * 0.45;
      for (let i = a; i < b; i++) {
        const x = (i - a) / Math.max(1, b - a);
        peaks[i] = Math.max(peaks[i], loud * Math.sin(Math.PI * Math.min(1, x * 1.15)) * (0.75 + rand() * 0.25));
      }
    }
  }
  return peaks.map((p) => Math.round(Math.min(1, p) * 1000) / 1000);
}

// ---- analyses -----------------------------------------------------------------------

export function meetingAnalysis(segments: readonly Segment[], createdAt: string): Analysis {
  const ch = (title: string, a: number, b: number, summary: string, tags: string[]) => ({
    title,
    startSegment: a,
    endSegment: b,
    start: segments[a].start,
    end: segments[b].end,
    summary,
    tags,
  });
  return {
    title: "Product planning meeting — next quarter",
    summary:
      "The team reviewed findings from **20 user interviews** and agreed on a three-month roadmap.\n\n" +
      "Users struggle to **search long recordings**, still worry about **Persian speech accuracy**, and strongly prefer " +
      "**on-device processing** for privacy. The roadmap prioritizes accuracy (month 1), semantic search (month 2) and " +
      "AI summaries / Q&A (month 3), with an early beta to reduce risk. About **10% of the budget** goes to UI design.",
    keyPoints: [
      "Searching long audio is the biggest pain point; one user spends ~3 hours a week re-listening.",
      "Accuracy with overlapping speakers and poor audio is still a concern.",
      "Local, on-device processing is a real competitive advantage for privacy.",
      "LLM costs stay below one cent per meeting with cheaper models.",
      "Ship an early beta to a few users before the AI features are final.",
    ],
    chapters: [
      ch("Opening & agenda", 0, 2, "Goals of the meeting and the agenda.", ["agenda"]),
      ch("User research findings", 3, 15, "Three themes from the interviews: search, accuracy and privacy.", [
        "user research",
        "privacy",
        "speech accuracy",
      ]),
      ch("Roadmap for the next quarter", 16, 26, "Month-by-month plan and a proposal for an early beta.", [
        "roadmap",
        "semantic search",
      ]),
      ch("Budget", 27, 34, "Quarterly budget, LLM costs and money for UI design.", ["budget"]),
      ch("Next steps", 35, 39, "Owners and dates for follow-up work.", ["action items"]),
    ],
    tags: [
      { name: "user research", description: "Findings from the user interviews.", segments: [3, 4, 5, 6, 11, 12, 13] },
      { name: "privacy", description: "Keeping recordings on the user's device.", segments: [9, 10, 15] },
      { name: "speech accuracy", description: "Quality of Persian speech recognition.", segments: [7, 8, 17, 18] },
      { name: "semantic search", description: "Searching by meaning instead of exact words.", segments: [4, 19, 20, 36] },
      { name: "roadmap", description: "The three-month plan.", segments: [16, 17, 18, 19, 21, 22, 23, 25] },
      { name: "budget", description: "Money for the quarter and per-meeting LLM costs.", segments: [14, 27, 28, 29, 30, 31, 32, 34] },
      { name: "action items", description: "Who does what next.", segments: [36, 37, 38, 39] },
    ],
    actionItems: [
      "Reza: prototype semantic search by the end of the week.",
      "Sara: schedule five user-testing sessions for next week.",
      "Update the roadmap document and share it with everyone.",
      "Next meeting: Tuesday, 10:00.",
    ],
    questions: [
      "What did users say about privacy?",
      "How much will the LLM features cost per meeting?",
      "Who is responsible for the semantic search prototype?",
      "چرا نسخهٔ آزمایشی زودتر منتشر می‌شه؟",
    ],
    model: "deepseek/deepseek-v4-flash",
    language: "en",
    createdAt,
  };
}

export function hafezAnalysis(segments: readonly Segment[], createdAt: string): Analysis {
  const ch = (title: string, a: number, b: number, summary: string, tags: string[]) => ({
    title,
    startSegment: a,
    endSegment: b,
    start: segments[a].start,
    end: segments[b].end,
    summary,
    tags,
  });
  return {
    title: "درس‌گفتار: آشنایی با حافظ",
    summary:
      "این درس‌گفتار مقدمه‌ای بر زندگی و شعر **حافظ شیرازی** است. سخنران از زندگی حافظ در شیراز قرن هشتم شروع می‌کند، " +
      "سپس به ویژگی‌های غزل او، به‌ویژه **ایهام**، می‌پردازد و توضیح می‌دهد چرا «فال حافظ» شکل گرفته است.\n\n" +
      "در پایان به مضمون‌های اصلی شعر، موسیقی کلام و تأثیر حافظ بر ادبیات جهان (مثلاً **گوته**) اشاره می‌شود.",
    keyPoints: [
      "حافظ در قرن هشتم هجری در شیراز زندگی می‌کرد.",
      "قالب اصلی شعر او غزل است.",
      "ایهام باعث می‌شود هر خواننده برداشت خودش را داشته باشد.",
      "نقد ریاکاری از مضمون‌های پرتکرار شعر اوست.",
      "گوته تحت تأثیر حافظ «دیوان غربی‌شرقی» را نوشت.",
    ],
    chapters: [
      ch("زندگی حافظ", 0, 2, "زادگاه، دوره و وجه تسمیهٔ حافظ.", ["حافظ", "شیراز"]),
      ch("غزل و ایهام", 3, 6, "ساختار غزل، ایهام و فال حافظ.", ["غزل", "ایهام"]),
      ch("مضمون‌ها و موسیقی شعر", 7, 10, "عشق، رندی، نقد ریاکاری و موسیقی کلام.", ["ادبیات فارسی"]),
      ch("تأثیر و میراث", 11, 13, "تأثیر بر گوته و آرامگاه حافظ.", ["حافظ"]),
    ],
    tags: [
      { name: "حافظ", description: "شاعر قرن هشتم شیراز.", segments: [0, 1, 2, 8, 10, 11, 12] },
      { name: "غزل", description: "قالب اصلی شعر حافظ.", segments: [3, 5, 6, 13] },
      { name: "ایهام", description: "چندمعنایی در شعر.", segments: [4, 5, 6] },
      { name: "شیراز", description: "شهر حافظ.", segments: [1, 12] },
    ],
    actionItems: ["برای جلسهٔ بعد چند غزل معروف را بخوانید."],
    questions: ["ایهام یعنی چه؟", "Why is Hafez used for divination (fal)?", "گوته چه ارتباطی با حافظ دارد؟"],
    model: "deepseek/deepseek-v4-flash",
    language: "fa",
    createdAt,
  };
}

export function podcastAnalysis(segments: readonly Segment[], createdAt: string): Analysis {
  const last = segments.length - 1;
  const clampIdx = (i: number) => Math.max(0, Math.min(last, i));
  const ch = (title: string, a: number, b: number, summary: string, tags: string[]) => ({
    title,
    startSegment: clampIdx(a),
    endSegment: clampIdx(b),
    start: segments[clampIdx(a)].start,
    end: segments[clampIdx(b)].end,
    summary,
    tags,
  });
  const inRange = (ids: number[]) => ids.filter((i) => i <= last);
  return {
    title: "Sleep and productivity",
    summary:
      "A short podcast episode on why **sleep** matters for focus and memory, with practical habits: a fixed bedtime, " +
      "no screens an hour before bed, no coffee in the afternoon, short naps and morning exercise.",
    keyPoints: [
      "Most adults need 7–9 hours of sleep.",
      "Blue light suppresses melatonin; put the phone away an hour before bed.",
      "Caffeine lasts up to six hours.",
      "Short 20-minute naps help; long naps hurt night sleep.",
    ],
    chapters: [
      ch("Intro", 0, 1, "Episode intro.", []),
      ch("Why sleep matters", 2, 4, "How much sleep we need and what lack of sleep does.", ["sleep"]),
      ch("Practical habits", 5, 12, "Concrete tips for better sleep.", ["habits"]),
      ch("Wrap-up", 13, last, "Productivity means working smart.", ["productivity"]),
    ],
    tags: [
      { name: "sleep", description: "Sleep needs and quality.", segments: inRange([1, 2, 3, 4, 9, 10]) },
      { name: "habits", description: "Practical routines.", segments: inRange([5, 6, 7, 8, 10, 11, 12]) },
      { name: "productivity", description: "Working smart.", segments: inRange([1, 13]) },
    ],
    actionItems: ["Try a fixed bedtime for one week."],
    questions: ["How long does caffeine stay in the body?", "چرت کوتاه چقدر باید باشد؟"],
    model: "deepseek/deepseek-v4-flash",
    language: "en",
    createdAt,
  };
}

// ---- docs ---------------------------------------------------------------------------

export type MockDocKind = "mindmap" | "timeline" | "flashcards" | "quiz" | "minutes" | "glossary";

export function docKindFromPrompt(prompt: string): MockDocKind {
  const p = prompt.toLowerCase();
  if (p.includes("mind")) return "mindmap";
  if (p.includes("flash")) return "flashcards";
  if (p.includes("quiz")) return "quiz";
  if (p.includes("minute")) return "minutes";
  if (p.includes("gloss")) return "glossary";
  return "timeline";
}

const DOC_TITLES: Record<MockDocKind, string> = {
  mindmap: "Mind map",
  timeline: "Interactive timeline",
  flashcards: "Flashcards",
  quiz: "Quiz",
  minutes: "Meeting minutes",
  glossary: "Glossary",
};

export function docTitle(kind: MockDocKind): string {
  return DOC_TITLES[kind];
}

/** A self-contained interactive doc that drives the player through window.WWTA. */
export function mockDocHtml(kind: MockDocKind, accentHue = 245): string {
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>${DOC_TITLES[kind]}</title>
<style>
  :root { --accent: hsl(${accentHue} 70% 58%); --bg: #fff; --fg: #1f1f1d; --muted: #6b6a66; --card: #f6f6f4; --line: #e7e6e3; }
  @media (prefers-color-scheme: dark) { :root { --bg: #151517; --fg: #ececee; --muted: #a1a1aa; --card: #1f1f23; --line: #2e2e33; } }
  * { box-sizing: border-box; }
  body { margin: 0; padding: 28px; font: 15px/1.6 system-ui, "Vazirmatn", sans-serif; background: var(--bg); color: var(--fg); }
  h1 { font-size: 22px; margin: 0 0 4px; }
  .sub { color: var(--muted); margin-bottom: 22px; }
  .item { display: flex; gap: 14px; padding: 12px 14px; border: 1px solid var(--line); border-radius: 12px; margin-bottom: 10px; background: var(--card); cursor: pointer; transition: border-color .15s, transform .15s; }
  .item:hover { border-color: var(--accent); }
  .item.active { border-color: var(--accent); box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 25%, transparent); }
  .time { font-variant-numeric: tabular-nums; color: var(--accent); font-weight: 600; min-width: 52px; }
  .muted { color: var(--muted); font-size: 13px; }
  .card { perspective: 800px; height: 150px; margin-bottom: 12px; cursor: pointer; }
  .inner { position: relative; width: 100%; height: 100%; transition: transform .45s; transform-style: preserve-3d; }
  .card.flip .inner { transform: rotateY(180deg); }
  .face { position: absolute; inset: 0; backface-visibility: hidden; border: 1px solid var(--line); border-radius: 14px; background: var(--card); display: flex; align-items: center; justify-content: center; padding: 18px; text-align: center; }
  .back { transform: rotateY(180deg); }
  button { font: inherit; border: 1px solid var(--line); background: var(--bg); color: var(--fg); border-radius: 8px; padding: 6px 12px; cursor: pointer; }
  button:hover { border-color: var(--accent); }
  .bar { height: 4px; background: var(--line); border-radius: 4px; overflow: hidden; margin: 0 0 20px; }
  .bar > div { height: 100%; background: var(--accent); width: 0; transition: width .2s linear; }
  svg text { fill: var(--fg); font-size: 12px; }
  [dir=rtl] { text-align: right; }
</style>
</head>
<body>
<h1 id="title"></h1>
<div class="sub" id="sub"></div>
<div class="bar"><div id="progress"></div></div>
<div id="root"></div>
<script>
  var KIND = ${JSON.stringify(kind)};
  var W = window.WWTA;
  var A = W.analysis || { chapters: [], keyPoints: [], tags: [], actionItems: [], summary: "" };
  function fmt(s) { s = Math.max(0, Math.floor(s)); var m = Math.floor(s / 60), r = s % 60; return m + ":" + (r < 10 ? "0" : "") + r; }
  function el(tag, attrs, text) { var e = document.createElement(tag); for (var k in attrs || {}) e.setAttribute(k, attrs[k]); if (text != null) e.textContent = text; return e; }
  document.getElementById("title").textContent = ${JSON.stringify(DOC_TITLES[kind])};
  document.getElementById("sub").textContent = W.recording.title + " · click anything to jump there";
  var root = document.getElementById("root");
  var items = [];
  function addItem(label, detail, start) {
    var row = el("div", { "class": "item", dir: "auto" });
    row.appendChild(el("div", { "class": "time" }, fmt(start)));
    var body = el("div");
    body.appendChild(el("div", { dir: "auto" }, label));
    if (detail) body.appendChild(el("div", { "class": "muted", dir: "auto" }, detail));
    row.appendChild(body);
    row.onclick = function () { W.seek(start); W.play(); };
    root.appendChild(row);
    items.push({ row: row, start: start });
  }
  if (KIND === "timeline" || KIND === "minutes") {
    (A.chapters.length ? A.chapters : W.segments.filter(function (_, i) { return i % 5 === 0; }).map(function (s) { return { title: s.text, start: s.start, summary: "" }; }))
      .forEach(function (c) { addItem(c.title, c.summary, c.start); });
    if (KIND === "minutes" && A.actionItems.length) {
      root.appendChild(el("h3", {}, "Action items"));
      A.actionItems.forEach(function (a) { root.appendChild(el("div", { dir: "auto" }, "☐ " + a)); });
    }
  } else if (KIND === "flashcards" || KIND === "quiz") {
    var points = A.keyPoints.length ? A.keyPoints : W.segments.slice(0, 6).map(function (s) { return s.text; });
    points.forEach(function (p, i) {
      var card = el("div", { "class": "card" });
      var inner = el("div", { "class": "inner" });
      inner.appendChild(el("div", { "class": "face", dir: "auto" }, (KIND === "quiz" ? "Q" : "Card ") + (i + 1) + ": what was said about “" + p.split(" ").slice(0, 3).join(" ") + "…”?"));
      inner.appendChild(el("div", { "class": "face back", dir: "auto" }, p));
      card.appendChild(inner);
      card.onclick = function () { card.classList.toggle("flip"); };
      root.appendChild(card);
    });
  } else if (KIND === "glossary") {
    A.tags.forEach(function (t) {
      var seg = W.segments[t.segments[0]];
      addItem(t.name, t.description, seg ? seg.start : 0);
    });
  } else {
    var chapters = A.chapters;
    var svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.setAttribute("viewBox", "0 0 640 420"); svg.setAttribute("width", "100%");
    var cx = 320, cy = 210;
    chapters.forEach(function (c, i) {
      var ang = (i / Math.max(1, chapters.length)) * Math.PI * 2 - Math.PI / 2;
      var x = cx + Math.cos(ang) * 200, y = cy + Math.sin(ang) * 150;
      var line = document.createElementNS(svg.namespaceURI, "line");
      line.setAttribute("x1", cx); line.setAttribute("y1", cy); line.setAttribute("x2", x); line.setAttribute("y2", y);
      line.setAttribute("stroke", "currentColor"); line.setAttribute("opacity", ".25");
      svg.appendChild(line);
      var g = document.createElementNS(svg.namespaceURI, "g"); g.style.cursor = "pointer";
      var circle = document.createElementNS(svg.namespaceURI, "circle");
      circle.setAttribute("cx", x); circle.setAttribute("cy", y); circle.setAttribute("r", 8); circle.setAttribute("fill", "var(--accent)");
      var text = document.createElementNS(svg.namespaceURI, "text");
      text.setAttribute("x", x); text.setAttribute("y", y + 24); text.setAttribute("text-anchor", "middle"); text.textContent = c.title;
      g.appendChild(circle); g.appendChild(text);
      g.onclick = function () { W.seek(c.start); W.play(); };
      svg.appendChild(g);
    });
    var center = document.createElementNS(svg.namespaceURI, "text");
    center.setAttribute("x", cx); center.setAttribute("y", cy + 4); center.setAttribute("text-anchor", "middle");
    center.setAttribute("font-weight", "700"); center.textContent = A.title || W.recording.title;
    svg.appendChild(center);
    root.appendChild(svg);
  }
  var duration = W.recording.durationSec || (W.segments.length ? W.segments[W.segments.length - 1].end : 0);
  W.onTime(function (t) {
    if (duration) document.getElementById("progress").style.width = Math.min(100, (t / duration) * 100) + "%";
    var active = -1;
    for (var i = 0; i < items.length; i++) if (items[i].start <= t) active = i;
    items.forEach(function (it, i) { it.row.classList.toggle("active", i === active); });
  });
</script>
</body>
</html>`;
}

// ---- models & system ----------------------------------------------------------------

export function initialModels(dataDir: string): ModelInfo[] {
  return [
    {
      id: "taraneh-wlarge",
      name: "Taraneh-WLarge",
      engine: "whisper",
      description:
        "Whisper large-v3 fine-tuned on Persian speech. The most accurate option — runs best on a GPU (Vulkan / Metal) and works on CPU, slowly.",
      license: "Apache-2.0",
      recommendedFor: "Best accuracy · GPU",
      downloadBytes: 3_090_000_000,
      diskBytes: 1_660_000_000,
      homepage: "https://huggingface.co/",
      status: "notInstalled",
      error: null,
      installDir: `${dataDir}/models/taraneh-wlarge`,
      manualFiles: [
        { name: "model.safetensors", url: "https://huggingface.co/", sizeBytes: 3_087_000_000, required: true },
        { name: "config.json", url: "https://huggingface.co/", sizeBytes: 1_300, required: true },
        { name: "tokenizer.json", url: "https://huggingface.co/", sizeBytes: 2_480_000, required: true },
        { name: "preprocessor_config.json", url: "https://huggingface.co/", sizeBytes: 340, required: false },
      ],
      manualInstructions:
        "1. Download the files below (a browser or `huggingface-cli` both work).\n" +
        "2. Click **Import files…** and select all of them at once.\n" +
        "3. The app converts the weights for whisper.cpp — this takes a few minutes and needs about 5 GB of free space.",
    },
    {
      id: "shenava-koochik",
      name: "Shenava Koochik",
      engine: "shenava",
      description: "A small, fast Persian speech model that runs comfortably on any CPU. Good for quick drafts and older laptops.",
      license: "MIT",
      recommendedFor: "Fast · any CPU",
      downloadBytes: 118_000_000,
      diskBytes: 118_000_000,
      homepage: "https://huggingface.co/",
      status: "notInstalled",
      error: null,
      installDir: `${dataDir}/models/shenava-koochik`,
      manualFiles: [
        { name: "model.onnx", url: "https://huggingface.co/", sizeBytes: 117_000_000, required: true },
        { name: "tokens.txt", url: "https://huggingface.co/", sizeBytes: 9_000, required: true },
      ],
      manualInstructions: "Download both files, then click **Import files…** and select them. No conversion is needed.",
    },
  ];
}

export const MOCK_SYSTEM: SystemInfo = {
  os: "linux",
  arch: "x86_64",
  appVersion: "0.1.0 (browser mock)",
  cpuCores: 16,
  totalMemoryBytes: 32_000_000_000,
  gpuBackend: "vulkan",
  gpuAvailable: true,
  gpuDevices: [{ name: "AMD Radeon RX 7800 XT", description: "Vulkan 1.3 · discrete", memoryBytes: 16_000_000_000 }],
  dataDir: "/home/you/.local/share/what-was-that-about",
  engines: { whisper: true, shenava: true },
};

export const MOCK_LLM_MODELS: LlmModelInfo[] = [
  { id: "deepseek/deepseek-v4-flash", name: "DeepSeek V4 Flash", contextLength: 128_000, promptPrice: 0.1, completionPrice: 0.3 },
  { id: "openai/gpt-6-luna", name: "OpenAI GPT-6 Luna", contextLength: 256_000, promptPrice: 0.15, completionPrice: 0.6 },
  { id: "anthropic/claude-haiku-5.5", name: "Anthropic Claude Haiku 5.5", contextLength: 200_000, promptPrice: 0.8, completionPrice: 4 },
  { id: "google/gemini-3-flash", name: "Google Gemini 3 Flash", contextLength: 1_000_000, promptPrice: 0.1, completionPrice: 0.4 },
  { id: "qwen/qwen3.5-72b-instruct", name: "Qwen 3.5 72B Instruct", contextLength: 131_000, promptPrice: 0.12, completionPrice: 0.39 },
  { id: "mistralai/mistral-small-4", name: "Mistral Small 4", contextLength: 128_000, promptPrice: 0.05, completionPrice: 0.15 },
  { id: "meta-llama/llama-5-scout", name: "Meta Llama 5 Scout", contextLength: 512_000, promptPrice: 0.08, completionPrice: 0.3 },
  { id: "openrouter/free-router", name: "Free router", contextLength: 32_000, promptPrice: 0, completionPrice: 0 },
];
