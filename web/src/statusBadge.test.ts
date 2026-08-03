import { describe, it, expect } from 'vitest';
import { statusBadgeClass } from './statusBadge';

// Exact mapping from plan — one row per serde status value + the fallback.
describe('statusBadgeClass', () => {
  it.each([
    ['available', 'bg-green-700 text-green-100'],
    ['pending', 'bg-amber-700 text-amber-100'],
    ['gifted', 'bg-give text-give-ink'],
    ['ben_redeemed', 'bg-slate-600 text-slate-100'],
    ['expired', 'bg-red-700 text-red-100'],
    ['something_unknown', 'bg-control text-ink'],
  ])('%s → %s', (status, expected) => {
    expect(statusBadgeClass(status)).toBe(expected);
  });
});
