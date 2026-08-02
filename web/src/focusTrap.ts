// Shared focus trap for role="dialog" aria-modal surfaces: Tab / Shift+Tab wrap
// inside the container instead of leaking to the page behind the backdrop
// (WCAG 2.4.3 / 2.1.2). Extracted from GameDetailModal (#62) so ClaimDialog can
// share the exact behavior (#63).
//
// The focusable set is computed at keydown time, never cached: a dialog's
// focusables are dynamic — carousel slides go inert per-index, buttons appear
// and disappear, and some steps (e.g. a loading spinner) have none at all.

import type { KeyboardEvent, RefObject } from "react";

const FOCUSABLE_SELECTOR =
  'button, [href], input, select, textarea, video, [tabindex]:not([tabindex="-1"])';

export function dialogFocusables(container: HTMLElement): HTMLElement[] {
  return Array.from(
    container.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR),
  ).filter(
    (el) =>
      el.closest('[inert], [aria-hidden="true"]') === null &&
      !el.hasAttribute("disabled"),
  );
}

// Build the container's onKeyDown handler. Tab is the only key it owns — Escape
// and any arrow handling stay with the caller. With zero focusables it swallows
// Tab so focus can't escape (which relies on the container itself holding focus).
export function makeTrapKeyDown<T extends HTMLElement>(
  containerRef: RefObject<T | null>,
) {
  return (e: KeyboardEvent) => {
    if (e.key !== "Tab") return;
    const container = containerRef.current;
    if (container === null) return;
    const els = dialogFocusables(container);
    if (els.length === 0) {
      e.preventDefault(); // nowhere to go — focus stays on the container
      return;
    }
    const first = els[0];
    const last = els[els.length - 1];
    if (first === undefined || last === undefined) return;
    const active = document.activeElement;
    if (e.shiftKey) {
      if (active === first || active === container) {
        e.preventDefault();
        last.focus();
      }
    } else if (active === last || active === container) {
      e.preventDefault();
      first.focus();
    }
  };
}
