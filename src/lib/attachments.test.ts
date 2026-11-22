import { describe, expect, it } from "vitest";
import { attachmentKind, clipLength, fitWithin, needsReencode } from "./attachments";

describe("attachments", () => {
  it("recognizes images and audio by type or name", () => {
    expect(attachmentKind("photo.JPG")).toBe("image");
    expect(attachmentKind("scan.heic")).toBe("image");
    expect(attachmentKind("note.opus")).toBe("audio");
    expect(attachmentKind("clip.m4a")).toBe("audio");
    expect(attachmentKind("blob", "image/png")).toBe("image");
    expect(attachmentKind("", "audio/webm")).toBe("audio");
    expect(attachmentKind("report.pdf")).toBeNull();
    expect(attachmentKind("noextension")).toBeNull();
  });

  it("shrinks to fit without enlarging", () => {
    expect(fitWithin(4000, 3000, 2048)).toEqual({ width: 2048, height: 1536 });
    expect(fitWithin(1000, 4096, 2048)).toEqual({ width: 500, height: 2048 });
    expect(fitWithin(800, 600, 2048)).toEqual({ width: 800, height: 600 });
    expect(fitWithin(0, 0, 2048)).toEqual({ width: 1, height: 1 });
  });

  it("re-encodes big or unusual images only", () => {
    expect(needsReencode("image/png", 200_000)).toBe(false);
    expect(needsReencode("image/png", 5_000_000)).toBe(true);
    expect(needsReencode("image/bmp", 10_000)).toBe(true);
    expect(needsReencode("", 10_000)).toBe(true);
  });

  it("formats clip lengths", () => {
    expect(clipLength(0)).toBe("0:00");
    expect(clipLength(65.4)).toBe("1:05");
    expect(clipLength(null)).toBe("0:00");
  });
});
