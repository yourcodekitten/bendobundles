/** 🎟️ What a link EXPOSES of ben's finite monthly humble picks.
 *
 * Not a cost. The spend is the FRIEND's act, at a time ben does not control,
 * and it may never happen (`fulfillment/src/lib.rs:127` dispatches the
 * choosecontent two-write on CLAIM, not on create). So this is an upper bound,
 * phrased as one.
 *
 * The ceiling is real, not cosmetic: `domain/src/lib.rs:314` refuses a claim
 * once `claims_used >= claims_allowed`, so N choice games behind an allowance
 * of A expose min(N, A) — true under every claim ordering.
 *
 * An EMPTY selection is not "nothing to say" — it is the open shelf, which is
 * 18 of 18 links in production, exposing the full allowance with nothing on
 * screen ever having said so.
 *
 * An UNKNOWN pick is reported and never counted as free: understating a finite
 * resource is the direction that costs ben something he cannot get back. */
export function exposureLabel(
  picked: { requiresChoice?: boolean }[],
  claimsAllowed: number,
): string {
  const allowance = Number.isFinite(claimsAllowed) ? Math.max(0, Math.trunc(claimsAllowed)) : 0;

  if (picked.length === 0) {
    return `open shelf — whoever holds this link can spend up to ${allowance} of your monthly picks`;
  }

  let spends = 0;
  let unknown = 0;
  for (const p of picked) {
    if (p.requiresChoice === undefined) unknown++;
    else if (p.requiresChoice) spends++;
  }

  const exposure = Math.min(spends, allowance);
  if (unknown > 0) {
    return `up to ${exposure} of your monthly picks, and ${unknown} not costed — reopen from the catalog`;
  }
  if (exposure === 0) return 'free to give — no picks at stake';
  return `up to ${exposure} of your monthly picks, if they claim`;
}
