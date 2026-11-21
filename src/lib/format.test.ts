import { describe, expect, it } from "vitest";
import {
  formatByteProgress,
  formatBytes,
  formatDate,
  formatDuration,
  formatPrice,
  formatTime,
  pluralize,
} from "./format";

describe("formatTime", () => {
  it("formats minutes and seconds", () => {
    expect(formatTime(0)).toBe("0:00");
    expect(formatTime(5.9)).toBe("0:05");
    expect(formatTime(75.4)).toBe("1:15");
  });
  it("adds hours when needed or forced", () => {
    expect(formatTime(3725)).toBe("1:02:05");
    expect(formatTime(65, true)).toBe("0:01:05");
  });
  it("treats invalid input as zero", () => {
    expect(formatTime(Number.NaN)).toBe("0:00");
    expect(formatTime(-3)).toBe("0:00");
    expect(formatTime(Number.POSITIVE_INFINITY)).toBe("0:00");
  });
});

describe("formatDuration", () => {
  it("picks a sensible granularity", () => {
    expect(formatDuration(45)).toBe("45s");
    expect(formatDuration(725)).toBe("12m 05s");
    expect(formatDuration(3720)).toBe("1h 02m");
  });
  it("renders unknown durations as a dash", () => {
    expect(formatDuration(null)).toBe("—");
    expect(formatDuration(undefined)).toBe("—");
  });
});

describe("formatBytes", () => {
  it("uses SI units", () => {
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(1_500)).toBe("1.5 KB");
    expect(formatBytes(4_900_000_000)).toBe("4.9 GB");
    expect(formatBytes(250_000_000)).toBe("250 MB");
  });
  it("handles missing values", () => {
    expect(formatBytes(null)).toBe("—");
  });
});

describe("formatByteProgress", () => {
  it("shares the total's unit", () => {
    expect(formatByteProgress(1_200_000_000, 4_900_000_000)).toBe("1.2 / 4.9 GB");
    expect(formatByteProgress(300_000, 80_000_000)).toBe("0.3 / 80.0 MB");
  });
  it("falls back when the total is unknown", () => {
    expect(formatByteProgress(2_000_000, 0)).toBe("2.0 MB");
  });
});

describe("formatDate", () => {
  const now = new Date(2026, 10, 21, 15, 0, 0);
  it("says Today / Yesterday", () => {
    expect(formatDate(new Date(2026, 10, 21, 9, 5).toISOString(), now)).toMatch(/^Today, /);
    expect(formatDate(new Date(2026, 10, 20, 9, 5).toISOString(), now)).toMatch(/^Yesterday, /);
  });
  it("includes the year only for other years", () => {
    expect(formatDate(new Date(2026, 2, 3).toISOString(), now)).not.toMatch(/2026/);
    expect(formatDate(new Date(2024, 2, 3).toISOString(), now)).toMatch(/2024/);
  });
  it("returns an empty string for garbage", () => {
    expect(formatDate("not a date", now)).toBe("");
  });
});

describe("formatPrice / pluralize", () => {
  it("formats prices", () => {
    expect(formatPrice(0.15)).toBe("$0.15");
    expect(formatPrice(0)).toBe("free");
    expect(formatPrice(null)).toBe("—");
    expect(formatPrice(0.004)).toBe("$0.0040");
  });
  it("pluralizes", () => {
    expect(pluralize(1, "segment")).toBe("1 segment");
    expect(pluralize(3, "segment")).toBe("3 segments");
  });
});
