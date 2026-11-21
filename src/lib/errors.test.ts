import { describe, expect, it } from "vitest";
import { errorMessage } from "./errors";

describe("errorMessage", () => {
  it("handles the shapes invoke can reject with", () => {
    expect(errorMessage("boom")).toBe("boom");
    expect(errorMessage(new Error("bad"))).toBe("bad");
    expect(errorMessage({ message: "from rust" })).toBe("from rust");
    expect(errorMessage({ kind: "Io", error: "disk full" })).toBe("disk full");
    expect(errorMessage({ code: 3 })).toBe('{"code":3}');
    expect(errorMessage(null)).toBe("Unknown error");
    expect(errorMessage("  ")).toBe("Unknown error");
  });
});
