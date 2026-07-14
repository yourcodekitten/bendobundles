/** Steam community-tag display + content-descriptor policy (#71).
 * Descriptor semantics (verified live 2026-07-14): 1 some nudity/sexual ·
 * 2 frequent violence/gore · 3 adult-ONLY sexual · 4 gratuitous sexual · 5 general mature. */

/** Sexual-content family — drives the admin 🔞 badge and the mature filter.
 * Deliberately NOT 5 (Rollerdrome/Witcher carry it) and NOT 2 (violence). */
export const MATURE_DESCRIPTOR_IDS: readonly number[] = [1, 3, 4];

export const DESCRIPTOR_LABELS: Record<number, string> = {
  1: 'some nudity or sexual content',
  2: 'frequent violence or gore',
  3: 'adult-only sexual content',
  4: 'gratuitous sexual content',
  5: 'general mature content',
};

export function isMature(descriptorIds: number[] | undefined): boolean {
  return (descriptorIds ?? []).some((id) => MATURE_DESCRIPTOR_IDS.includes(id));
}

/** Steam's store page shows tags by width-fit, not by count (Rollerdrome fits 6 short
 * ones). Deterministic mirror: popularity-order prefix within a character budget —
 * short tags ⇒ more chips. Always at least 3 (when available), never more than 6. */
export const TAG_CHAR_BUDGET = 36;
const TAG_MIN = 3;
const TAG_MAX = 6;

export function fitTags(tags: string[], budget: number = TAG_CHAR_BUDGET): string[] {
  const out: string[] = [];
  let used = 0;
  for (const t of tags) {
    if (out.length >= TAG_MAX) break;
    if (out.length >= TAG_MIN && used + t.length > budget) break;
    out.push(t);
    used += t.length;
  }
  return out;
}

/** Community tags when present, publisher genres otherwise (#71: genres are the
 * degradation path for gated/delisted apps and pre-backfill cache blobs). */
export function displayTags(x: { tags?: string[]; genres?: string[] }): string[] {
  return x.tags?.length ? x.tags : (x.genres ?? []);
}
