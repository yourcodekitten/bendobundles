import { describe, it, expect } from 'vitest';
import { selfClaimLabel } from './selfClaimLabel';

// The full wording ladder, pinned verbatim — Catalog and the modal both render
// these strings, and their component tests match on them (#51 PR D dedup).
describe('selfClaimLabel', () => {
  const base = { armed: false, ownedOnSteam: false, requiresChoice: false, claiming: false };

  it('idle → claim for me', () => {
    expect(selfClaimLabel(base)).toBe('claim for me');
  });

  it('claiming (un-armed) → claiming…', () => {
    expect(selfClaimLabel({ ...base, claiming: true })).toBe('claiming…');
  });

  it('armed, plain → confirm?', () => {
    expect(selfClaimLabel({ ...base, armed: true })).toBe('confirm?');
  });

  it('armed, choice game → spends 1 pick', () => {
    expect(selfClaimLabel({ ...base, armed: true, requiresChoice: true })).toBe(
      'confirm? spends 1 pick',
    );
  });

  it('armed, already owned → ownership warning', () => {
    expect(selfClaimLabel({ ...base, armed: true, ownedOnSteam: true })).toBe(
      'you already own this on steam — sure?',
    );
  });

  it('armed, already owned, choice game → warning with pick cost', () => {
    expect(
      selfClaimLabel({ ...base, armed: true, ownedOnSteam: true, requiresChoice: true }),
    ).toBe('you already own this on steam — spends 1 pick, sure?');
  });

  it('armed wins over claiming — the two-step is already past the spinner state', () => {
    expect(selfClaimLabel({ ...base, armed: true, claiming: true })).toBe('confirm?');
  });
});
