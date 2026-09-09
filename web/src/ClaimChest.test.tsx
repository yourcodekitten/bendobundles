// review pass 1 MINOR 2: the D6.2 ceremony line was untested — the verbatim copy
// (trailing ♡ included), the singular/plural ternary, and the ≥1 gate all ship
// silently without these. postmark.test.ts covers only the arithmetic.
import { render, screen } from "@testing-library/react";
import { vi, describe, it, expect } from "vitest";
import { ClaimChest } from "./ClaimChest";

function chest(waitedYears: number | null | undefined) {
  return (
    <ClaimChest
      charge={100}
      phase="bursting"
      pulse={0}
      onMash={vi.fn()}
      onCancel={vi.fn()}
      waitedYears={waitedYears}
    />
  );
}

describe("ClaimChest waited-years ceremony (spec D6.2)", () => {
  it("renders the verbatim singular copy at 1 year, ♡ included", () => {
    render(chest(1));
    expect(screen.getByText("it waited 1 year for you ♡")).toBeInTheDocument();
  });

  it("pluralizes at 14 years", () => {
    render(chest(14));
    expect(
      screen.getByText("it waited 14 years for you ♡"),
    ).toBeInTheDocument();
  });

  it("renders no line at null, undefined, or 0 — never 'waited 0 years'", () => {
    for (const v of [null, undefined, 0]) {
      const { unmount } = render(chest(v));
      expect(screen.queryByText(/it waited/)).toBeNull();
      unmount();
    }
  });

  it("still says it's yours ♡ either way", () => {
    render(chest(null));
    expect(screen.getByText("it's yours ♡")).toBeInTheDocument();
  });
});
