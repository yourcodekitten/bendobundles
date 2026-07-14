import { describe, it, expect } from 'vitest';
import { graphemes, codePointCount, clampCodePoints } from './text';

describe('graphemes', () => {
  it('keeps ZWJ families and flags whole', () => {
    // family = 5 code points (3 people + 2 ZWJ), flag = 2 regional indicators
    expect(graphemes('\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}x')).toEqual([
      '\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}',
      'x',
    ]);
    expect(graphemes('\u{1F1FA}\u{1F1F8}!')).toEqual(['\u{1F1FA}\u{1F1F8}', '!']);
  });

  it('plain text segments per character', () => {
    expect(graphemes('abc')).toEqual(['a', 'b', 'c']);
    expect(graphemes('')).toEqual([]);
  });
});

describe('codePointCount / clampCodePoints', () => {
  it('counts in the server unit — an astral emoji is 1, not 2', () => {
    expect(codePointCount('\u{1F381}\u{1F381}')).toBe(2);
    expect('\u{1F381}\u{1F381}'.length).toBe(4); // the UTF-16 lie the counter used to tell
  });

  it('clamps to code points without splitting a surrogate pair', () => {
    const four = '\u{1F381}\u{1F381}\u{1F381}\u{1F381}';
    const clamped = clampCodePoints(four, 3);
    expect(codePointCount(clamped)).toBe(3);
    expect(clamped).toBe('\u{1F381}\u{1F381}\u{1F381}');
    // under the limit passes through untouched (same reference, no re-join)
    expect(clampCodePoints('abc', 5)).toBe('abc');
  });
});
