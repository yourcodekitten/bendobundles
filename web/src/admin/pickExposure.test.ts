import { describe, it, expect } from 'vitest';
import { exposureLabel } from './pickExposure';

const choice = { requiresChoice: true };
const free = { requiresChoice: false };
const unknown = {};

describe('exposureLabel', () => {
  it('an EMPTY selection is an open shelf, and says what it exposes', () => {
    // The spec's strongest finding: 18 of 18 links in production are open
    // shelves and none of them ever said this. Silence here is the defect.
    expect(exposureLabel([], 3)).toBe(
      'open shelf — whoever holds this link can spend up to 3 of your monthly picks',
    );
  });

  it('interpolates the allowance into the open-shelf row', () => {
    // Named for what it asserts. It was "uses the singular for an allowance of
    // one", which asserted no such thing — "up to 1 …picks" is identical in form
    // to the plural and exposureLabel has no singular handling. A copy rule that
    // exists only in a test title is worse than no rule: it reads as covered.
    expect(exposureLabel([], 1)).toBe(
      'open shelf — whoever holds this link can spend up to 1 of your monthly picks',
    );
  });

  it('says free when nothing picked spends a pick', () => {
    expect(exposureLabel([free, free], 3)).toBe('free to give — no picks at stake');
  });

  it('counts only the picks that spend', () => {
    expect(exposureLabel([free, choice, choice], 5)).toBe(
      'up to 2 of your monthly picks, if they claim',
    );
  });

  it('is CEILINGED by claims_allowed — the claim path refuses past it', () => {
    // domain/src/lib.rs:314 refuses a claim once claims_used >= claims_allowed,
    // so three choice games behind an allowance of one expose ONE pick, not three.
    expect(exposureLabel([choice, choice, choice], 1)).toBe(
      'up to 1 of your monthly picks, if they claim',
    );
  });

  it('an UNKNOWN pick is reported, never counted as free', () => {
    // Understating a finite resource is the dangerous direction. An absent
    // requiresChoice means the state predates the contract amendment.
    expect(exposureLabel([choice, unknown], 5)).toBe(
      'up to 1 of your monthly picks, and 1 not costed — reopen from the catalog',
    );
  });

  it('reports unknowns even when nothing known spends', () => {
    expect(exposureLabel([free, unknown], 5)).toBe(
      'up to 0 of your monthly picks, and 1 not costed — reopen from the catalog',
    );
  });

  it('reports every pick as uncosted when NONE of them is known', () => {
    // The all-unknown case is the one where "0 known spends" is most likely to
    // be mistaken for "free".
    expect(exposureLabel([unknown, unknown], 5)).toBe(
      'up to 0 of your monthly picks, and 2 not costed — reopen from the catalog',
    );
  });

  it('is total for a nonsense allowance rather than printing a negative', () => {
    // The UI clamps to >= 1 (Links.tsx:364), but this function is exported and
    // tested on its own; a future caller has no such clamp.
    expect(exposureLabel([choice], 0)).toBe('free to give — no picks at stake');
  });
});
