// ── Motion preferences ────────────────────────────────────────────────────────
// The one guarded matchMedia read for prefers-reduced-motion — shared by the
// cursor companion, the link-page landing, and the media carousel so the
// SSR/test guard can't drift between copies.

export function prefersReducedMotion(): boolean {
  if (typeof window === 'undefined' || !window.matchMedia) return false;
  return window.matchMedia('(prefers-reduced-motion: reduce)').matches;
}
