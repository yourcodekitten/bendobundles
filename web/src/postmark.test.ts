import { describe, expect, it } from "vitest";
import { postmark, waitedYears } from "./postmark";

describe("postmark", () => {
  it("renders lowercase month + year, UTC", () => {
    expect(postmark("2012-08-15T19:41:25.765070Z")).toBe("aug 2012");
    expect(postmark("2013-03-27T18:22:58Z")).toBe("mar 2013");
    // the SHIPPED wire shape: time's rfc3339 formatter trims trailing subsecond
    // zeros, so the pinned instant arrives with FIVE fractional digits
    expect(postmark("2012-08-15T19:41:25.76507Z")).toBe("aug 2012");
  });
  it("is null on absent or junk", () => {
    expect(postmark(undefined)).toBeNull();
    expect(postmark("not a date")).toBeNull();
  });
});

describe("waitedYears", () => {
  // `now` injected — the repo's own lesson about unpinned randomness/time in tests
  // (state issue #196's class): no Date.now() in assertions.
  const now = Date.parse("2026-09-09T12:00:00Z");
  it("counts whole calendar years and hides under one year", () => {
    expect(waitedYears("2012-08-15T19:41:25Z", now)).toBe(14);
    expect(waitedYears("2025-10-01T00:00:00Z", now)).toBeNull(); // <1y ⇒ silence, never "0 years"
  });
  it("says 1 on the exact first anniversary (the 365.25-division bug read this as 0)", () => {
    expect(waitedYears("2025-09-09T00:00:00Z", now)).toBe(1);
    expect(waitedYears("2025-09-10T00:00:00Z", now)).toBeNull(); // one day short — not yet
  });
  it("is null on absent or junk", () => {
    expect(waitedYears(undefined, now)).toBeNull();
    expect(waitedYears("junk", now)).toBeNull();
  });
});

describe('waitedYears — the two scrapbook call shapes (frozen clock)', () => {
  // characterization: these pin EXISTING semantics the scrapbook relies on.
  const acquired = '2014-08-15T00:00:00Z';

  it('keepsake card: span ends at the unwrap, not today', () => {
    // opened sep 2023 — anniversary passed → 9 (the spec's own example line)
    expect(waitedYears(acquired, Date.parse('2023-09-20T00:00:00Z'))).toBe(9);
  });

  it('same acquired_at, waiting-shaped now, DIFFERENT answer — the divergence is the bug class', () => {
    expect(waitedYears(acquired, Date.parse('2026-09-14T00:00:00Z'))).toBe(12);
  });

  it('anniversary not reached rounds down (mar 2023 → 8, not 9)', () => {
    expect(waitedYears(acquired, Date.parse('2023-03-10T00:00:00Z'))).toBe(8);
  });
});
