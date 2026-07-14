import { describe, expect, it } from 'vitest';
import { DESCRIPTOR_LABELS, displayTags, fitTags, isMature } from './tags';

describe('fitTags', () => {
  it('fills by character budget in given order', () => {
    // 6+6+7=19 fits; "Resource Management" (19) would blow a 36 budget at position 4
    expect(fitTags(['Action', 'Sports', 'Shooter', 'Resource Management', 'Indie'], 36)).toEqual([
      'Action',
      'Sports',
      'Shooter',
    ]);
  });
  it('always shows at least 3 tags even over budget', () => {
    const long = ['Interactive Fiction', 'Psychological Horror', 'Resource Management', 'Indie'];
    expect(fitTags(long, 10)).toEqual(long.slice(0, 3));
  });
  it('never shows more than 6', () => {
    expect(fitTags(['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h'], 999)).toHaveLength(6);
  });
  it('handles fewer than 3 tags', () => {
    expect(fitTags(['Action'], 36)).toEqual(['Action']);
    expect(fitTags([], 36)).toEqual([]);
  });
});

describe('displayTags', () => {
  it('prefers tags over genres', () => {
    expect(displayTags({ tags: ['Roguelike'], genres: ['Action'] })).toEqual(['Roguelike']);
  });
  it('falls back to genres when tags are empty or absent', () => {
    expect(displayTags({ tags: [], genres: ['Action'] })).toEqual(['Action']);
    expect(displayTags({ genres: ['Action'] })).toEqual(['Action']);
  });
  it('returns empty when neither exists', () => {
    expect(displayTags({})).toEqual([]);
  });
});

describe('isMature', () => {
  it('true for the sexual-content family {1,3,4}', () => {
    expect(isMature([1])).toBe(true);
    expect(isMature([3])).toBe(true);
    expect(isMature([2, 4])).toBe(true);
  });
  it('false for violence-only, general-mature-only, none, undefined', () => {
    expect(isMature([2])).toBe(false);
    expect(isMature([5])).toBe(false); // Rollerdrome must not badge
    expect(isMature([])).toBe(false);
    expect(isMature(undefined)).toBe(false);
  });
});

describe('DESCRIPTOR_LABELS', () => {
  it('labels all five known ids', () => {
    for (const id of [1, 2, 3, 4, 5]) expect(DESCRIPTOR_LABELS[id]).toBeTruthy();
  });
});
