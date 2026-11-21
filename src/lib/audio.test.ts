import { describe, expect, it } from "vitest";
import { downmixToMono, resampleLinear } from "./audio";

describe("downmixToMono", () => {
  it("returns an empty buffer for no channels", () => {
    expect(downmixToMono([]).length).toBe(0);
  });
  it("copies a single channel", () => {
    const ch = new Float32Array([0.1, -0.2]);
    const out = downmixToMono([ch]);
    expect(Array.from(out)).toEqual(Array.from(ch));
    expect(out).not.toBe(ch);
  });
  it("averages channels", () => {
    const out = downmixToMono([new Float32Array([1, 0.5, -1]), new Float32Array([0, 0.5, 1])]);
    expect(Array.from(out)).toEqual([0.5, 0.5, 0]);
  });
  it("pads shorter channels with silence", () => {
    const out = downmixToMono([new Float32Array([1, 1, 1]), new Float32Array([1])]);
    expect(Array.from(out)).toEqual([1, 0.5, 0.5]);
  });
});

describe("resampleLinear", () => {
  it("is a copy at the same rate", () => {
    const s = new Float32Array([1, 2, 3]);
    expect(Array.from(resampleLinear(s, 16000, 16000))).toEqual([1, 2, 3]);
  });
  it("downsamples 48k → 16k to a third of the length", () => {
    const s = new Float32Array(4800).map((_, i) => i / 4800);
    const out = resampleLinear(s, 48000, 16000);
    expect(out.length).toBe(1600);
    expect(out[0]).toBeCloseTo(0);
    expect(out[800]).toBeCloseTo(s[2400]);
  });
  it("interpolates when upsampling", () => {
    const out = resampleLinear(new Float32Array([0, 1]), 1, 2);
    expect(Array.from(out)).toEqual([0, 0.5, 1, 1]);
  });
});
