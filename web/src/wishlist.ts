// ⭐ the wishing well — pure helpers (docs/spec-wishing-well.md). No I/O here.

/** "mar 2023" from epoch seconds, in UTC (a wishlist date is a calendar fact, not local). */
export function formatSince(addedEpochSecs: number): string | null {
  if (!addedEpochSecs) return null;
  return new Date(addedEpochSecs * 1000)
    .toLocaleString("en-US", { month: "short", year: "numeric", timeZone: "UTC" })
    .toLowerCase();
}

/** Per-visit shelf ranks with the ⭐ float (spec §2.2): starred ids first, each group
 *  Fisher–Yates-shuffled on its own, so the rummage survives inside both groups. */
export function floatRanks(ids: string[], starred: Set<string>, rand: () => number): Map<string, number> {
  const shuffle = (xs: string[]) => {
    for (let i = xs.length - 1; i > 0; i--) {
      const j = Math.floor(rand() * (i + 1));
      [xs[i], xs[j]] = [xs[j]!, xs[i]!];
    }
    return xs;
  };
  const ordered = [
    ...shuffle(ids.filter((id) => starred.has(id))),
    ...shuffle(ids.filter((id) => !starred.has(id))),
  ];
  return new Map(ordered.map((id, pos) => [id, pos]));
}
