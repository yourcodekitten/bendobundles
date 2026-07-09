// ── Motion preferences ────────────────────────────────────────────────────────
// The one guarded matchMedia read for prefers-reduced-motion — shared by the
// cursor companion, the link-page landing, and the media carousel so the
// SSR/test guard can't drift between copies.

export function prefersReducedMotion(): boolean {
  if (typeof window === "undefined" || !window.matchMedia) return false;
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

// The AFFIRMATIVE gate for big ceremonies (claim celebration, boot screen):
// they play only when matchMedia exists AND reduced-motion is off. Stricter
// than !prefersReducedMotion() — an environment with no matchMedia (jsdom)
// counts as "cannot confirm motion is welcome", so ceremonies are skipped
// while ordinary transitions (guarded by prefersReducedMotion) still run.
export function motionOK(): boolean {
  return (
    typeof window !== "undefined" &&
    window.matchMedia?.("(prefers-reduced-motion: reduce)").matches === false
  );
}
