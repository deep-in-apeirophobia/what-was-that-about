// Formatting helpers for times, sizes, dates and prices.

/** `75.4` → `"1:15"`, `3725` → `"1:02:05"`. Invalid or negative input renders as `0:00`. */
export function formatTime(seconds: number, forceHours = false): string {
  const total = Number.isFinite(seconds) && seconds > 0 ? Math.floor(seconds) : 0;
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const ss = String(s).padStart(2, "0");
  if (h > 0 || forceHours) return `${h}:${String(m).padStart(2, "0")}:${ss}`;
  return `${m}:${ss}`;
}

/** Human duration: `"1h 02m"`, `"12m 05s"`, `"45s"`. `null` → `"—"`. */
export function formatDuration(seconds: number | null | undefined): string {
  if (seconds == null || !Number.isFinite(seconds) || seconds < 0) return "—";
  const total = Math.round(seconds);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  if (h > 0) return `${h}h ${String(m).padStart(2, "0")}m`;
  if (m > 0) return `${m}m ${String(s).padStart(2, "0")}s`;
  return `${s}s`;
}

const UNITS = ["B", "KB", "MB", "GB", "TB"] as const;

/** Decimal (SI) byte sizes, matching what download sites show. */
export function formatBytes(bytes: number | null | undefined, digits = 1): string {
  if (bytes == null || !Number.isFinite(bytes) || bytes < 0) return "—";
  if (bytes < 1000) return `${Math.round(bytes)} B`;
  let value = bytes;
  let unit = 0;
  while (value >= 1000 && unit < UNITS.length - 1) {
    value /= 1000;
    unit++;
  }
  return `${value.toFixed(value >= 100 ? 0 : digits)} ${UNITS[unit]}`;
}

/** `"1.2 / 4.9 GB"` — both values share the unit of the total. */
export function formatByteProgress(done: number, total: number): string {
  if (!total || total <= 0) return formatBytes(done);
  let unit = 0;
  let scale = 1;
  while (total / scale >= 1000 && unit < UNITS.length - 1) {
    scale *= 1000;
    unit++;
  }
  const digits = unit === 0 ? 0 : 1;
  return `${(done / scale).toFixed(digits)} / ${(total / scale).toFixed(digits)} ${UNITS[unit]}`;
}

function sameDay(a: Date, b: Date): boolean {
  return (
    a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate()
  );
}

/** Short, friendly date: `"Today, 14:03"`, `"Yesterday, 09:12"`, `"Mar 3"`, `"Mar 3, 2024"`. */
export function formatDate(iso: string, now: Date = new Date()): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  const time = d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
  if (sameDay(d, now)) return `Today, ${time}`;
  const yesterday = new Date(now);
  yesterday.setDate(now.getDate() - 1);
  if (sameDay(d, yesterday)) return `Yesterday, ${time}`;
  const opts: Intl.DateTimeFormatOptions =
    d.getFullYear() === now.getFullYear()
      ? { month: "short", day: "numeric" }
      : { month: "short", day: "numeric", year: "numeric" };
  return d.toLocaleDateString(undefined, opts);
}

/** USD per 1M tokens → `"$0.15"`, `"free"`, or `"—"` when unknown. */
export function formatPrice(perMillion: number | null | undefined): string {
  if (perMillion == null || !Number.isFinite(perMillion)) return "—";
  if (perMillion === 0) return "free";
  if (perMillion < 0.01) return `$${perMillion.toFixed(4)}`;
  return `$${perMillion.toFixed(2)}`;
}

export function pluralize(n: number, one: string, many = `${one}s`): string {
  return `${n} ${n === 1 ? one : many}`;
}
