// 📮 the postmark (docs/spec-postmark.md D6) — when ben's bundle purchase created a
// game's order, rendered in the attic voice. Helper module per web convention
// (tags.ts / stateBadge.ts / selfClaimLabel.ts): component files export only
// components (react-refresh), helpers get twin test files.

// deterministic, environment-free (step-5 review: toLocaleString puts an ICU
// dependency inside a display assertion; twelve strings are shorter than the
// sentence explaining why that was safe)
const MONTHS = [
  "jan",
  "feb",
  "mar",
  "apr",
  "may",
  "jun",
  "jul",
  "aug",
  "sep",
  "oct",
  "nov",
  "dec",
] as const;

/** 📮 "aug 2012" — lowercase month, UTC, attic voice. Null when unknown/junk (absence
 *  must render as exactly the today-state — spec D6). */
export function postmark(iso: string | undefined): string | null {
  if (iso === undefined) return null;
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return null;
  const d = new Date(t);
  return `${MONTHS[d.getUTCMonth()]} ${d.getUTCFullYear()}`;
}

/** Whole CALENDAR years a treasure waited; null under one year (never "waited 0
 *  years" — D6) and on unknown/junk. Calendar comparison, not 365.25-day division:
 *  the average-year floor reads a gift's exact first anniversary as 0.9993 → 0 →
 *  silence on the one day the line is most deserved (step-5 review, OMBB).
 *  `now` is injectable for tests. */
export function waitedYears(
  iso: string | undefined,
  now: number = Date.now(),
): number | null {
  if (iso === undefined) return null;
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return null;
  const then = new Date(t);
  const ref = new Date(now);
  let years = ref.getUTCFullYear() - then.getUTCFullYear();
  const anniversaryNotReached =
    ref.getUTCMonth() < then.getUTCMonth() ||
    (ref.getUTCMonth() === then.getUTCMonth() &&
      ref.getUTCDate() < then.getUTCDate());
  if (anniversaryNotReached) years -= 1;
  return years >= 1 ? years : null;
}
