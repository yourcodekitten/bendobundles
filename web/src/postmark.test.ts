import { describe, expect, it } from "vitest";
import { postmark, waitedYears } from "./postmark";

describe("postmark", () => {
  it("renders lowercase month + year, UTC", () => {
    expect(postmark("2012-08-15T19:41:25.765070Z")).toBe("aug 2012");
    expect(postmark("2013-03-27T18:22:58Z")).toBe("mar 2013");
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
