import { describe, it, expect, afterEach } from "vitest";
import { dialogFocusables } from "./focusTrap";

// Direct unit tests for the shared primitive both dialogs rely on. The trap's
// wrap behavior is exercised end-to-end in GameDetailModal.test / ClaimDialog.test;
// here we pin the exclusion rules that keep focus off hidden/inert/disabled nodes.

function mount(html: string): HTMLElement {
  const container = document.createElement("div");
  container.innerHTML = html;
  document.body.appendChild(container);
  return container;
}

afterEach(() => {
  document.body.innerHTML = "";
});

describe("dialogFocusables", () => {
  it("collects standard focusables in document order", () => {
    const c = mount(`
      <button>a</button>
      <a href="#">b</a>
      <input />
    `);
    const names = dialogFocusables(c).map((el) => el.tagName.toLowerCase());
    expect(names).toEqual(["button", "a", "input"]);
  });

  it('excludes tabindex="-1" (programmatic-only focus targets)', () => {
    const c = mount(`
      <button>real</button>
      <div tabindex="-1">not tabbable</div>
    `);
    expect(dialogFocusables(c)).toHaveLength(1);
  });

  it("excludes disabled controls", () => {
    const c = mount(`
      <button>on</button>
      <button disabled>off</button>
    `);
    expect(dialogFocusables(c).map((el) => el.textContent)).toEqual(["on"]);
  });

  it("excludes anything inside an [inert] subtree", () => {
    const c = mount(`
      <button>live</button>
      <div inert><button>slide</button><a href="#">x</a></div>
    `);
    expect(dialogFocusables(c).map((el) => el.textContent)).toEqual(["live"]);
  });

  it('excludes anything inside an [aria-hidden="true"] subtree', () => {
    const c = mount(`
      <button>live</button>
      <div aria-hidden="true"><button>decorative</button></div>
    `);
    expect(dialogFocusables(c).map((el) => el.textContent)).toEqual(["live"]);
  });

  it("excludes hidden inputs so one can never become a dead focus boundary", () => {
    // A hidden input eats .focus() as a no-op; at a wrap boundary (where preventDefault already
    // fired) focus would stick there instead of advancing. It must not be in the set.
    const c = mount(
      `<button>a</button><input type="hidden" /><input type="text" /><button>b</button>`,
    );
    const els = dialogFocusables(c);
    expect(els).toHaveLength(3); // two buttons + the text input; hidden input excluded
    expect(els.some((el) => el.getAttribute("type") === "hidden")).toBe(false);
  });

  it("returns an empty set when a step has no focusables (the loading-step shape)", () => {
    const c = mount(`<p>claiming...</p>`);
    expect(dialogFocusables(c)).toEqual([]);
  });
});
