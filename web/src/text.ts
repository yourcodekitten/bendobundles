// Text units, and who counts in what (OMBB's #69 review, fish #22/#23):
// - the SERVER bounds the gift note with Rust `chars().count()` = Unicode code
//   points — so client-side limits/counters must count CODE POINTS to agree.
// - DISPLAY slicing (the typewriter) must go one level higher: GRAPHEME
//   clusters. Code points fixed split surrogates, but a ZWJ family or a flag
//   is several code points — slicing between them renders a family assembling
//   itself member by member.

// Intl.Segmenter is ES2022; this project's TS lib is ES2020, so type the
// constructor locally instead of widening the whole project's lib for one
// helper. Runtime support is everywhere the app runs (all evergreen browsers,
// node 16+); the fallback is for anything older.
type SegmenterCtor = new (
  locale?: string,
  options?: { granularity: 'grapheme' },
) => { segment(s: string): Iterable<{ segment: string }> };

const Segmenter = (Intl as { Segmenter?: SegmenterCtor }).Segmenter;

/** Grapheme-cluster segmentation for display: never splits an emoji family,
 * flag, or other ZWJ sequence. Falls back to code points where Intl.Segmenter
 * is unavailable — worst case is the pre-segmenter behavior (whole scalar
 * values), never a split surrogate. */
export function graphemes(s: string): string[] {
  if (Segmenter !== undefined) {
    const seg = new Segmenter(undefined, { granularity: 'grapheme' });
    return Array.from(seg.segment(s), (g) => g.segment);
  }
  return Array.from(s);
}

/** Length in the server's units (Unicode code points, like Rust `chars()`). */
export function codePointCount(s: string): number {
  return Array.from(s).length;
}

/** Clamp to at most `max` code points — the exact server bound, unlike the
 * textarea `maxLength` attribute, which counts UTF-16 units and cuts an
 * emoji-heavy text at roughly half the real limit. */
export function clampCodePoints(s: string, max: number): string {
  const cps = Array.from(s);
  return cps.length > max ? cps.slice(0, max).join('') : s;
}
