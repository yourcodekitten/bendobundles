// ⭐ the wishing well — pure helpers (docs/spec-wishing-well.md). No I/O here.

/** "mar 2023" from epoch seconds, in UTC (a wishlist date is a calendar fact, not local). */
export function formatSince(addedEpochSecs: number): string | null {
  if (!addedEpochSecs) return null;
  return new Date(addedEpochSecs * 1000)
    .toLocaleString("en-US", { month: "short", year: "numeric", timeZone: "UTC" })
    .toLowerCase();
}
