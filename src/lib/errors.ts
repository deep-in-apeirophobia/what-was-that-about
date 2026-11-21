/** Turn anything thrown by `invoke` (string, Error, `{ message }`, …) into readable text. */
export function errorMessage(e: unknown): string {
  if (e == null) return "Unknown error";
  if (typeof e === "string") return e.trim() || "Unknown error";
  if (e instanceof Error) return e.message || e.name;
  if (typeof e === "object") {
    const o = e as Record<string, unknown>;
    for (const key of ["message", "error", "reason", "details"]) {
      const v = o[key];
      if (typeof v === "string" && v.trim()) return v;
    }
    try {
      return JSON.stringify(e);
    } catch {
      return String(e);
    }
  }
  return String(e);
}
