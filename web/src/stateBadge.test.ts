import { describe, expect, it } from 'vitest';
import { stateBadgeClass } from './stateBadge';

describe('stateBadgeClass', () => {
  it.each([
    ['fulfilled', 'bg-green-700 text-green-100'],
    ['pending', 'bg-amber-700 text-amber-100'],
    // slate, not give-violet: the #136 verdict this module exists to enforce
    ['compensated', 'bg-slate-600 text-slate-100'],
    ['failed', 'bg-rose-950 text-rose-200'],
    // the runtime net: state unions are `as`-asserted over untrusted JSON, so this
    // arm is reachable the day the backend grows a fifth state. neutral, not amber —
    // unknown must never impersonate pending.
    ['someday-new-state', 'bg-control text-ink'],
  ])('%s → %s', (state, expected) => {
    expect(stateBadgeClass(state)).toBe(expected);
  });
});
