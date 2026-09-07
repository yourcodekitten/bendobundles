import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

// The lambda swaps the og block anchored on these exact markers (spec D3).
// This test is the BUILD-side half of the contract; the lambda's marker
// witness is the bucket-side half. Change one, change both.
describe("og markers", () => {
  const html = readFileSync(resolve(__dirname, "../index.html"), "utf8");
  it("carries begin/end markers in order, once each", () => {
    const b = html.indexOf("<!-- og:begin -->");
    const e = html.indexOf("<!-- og:end -->");
    expect(b).toBeGreaterThan(-1);
    expect(e).toBeGreaterThan(b);
    expect(html.indexOf("<!-- og:begin -->", b + 1)).toBe(-1);
    expect(html.indexOf("<!-- og:end -->", e + 1)).toBe(-1);
  });
  it("wraps the og block (og:title inside the markers)", () => {
    const inner = html.slice(html.indexOf("<!-- og:begin -->"), html.indexOf("<!-- og:end -->"));
    expect(inner).toContain('property="og:title"');
    expect(inner).toContain('name="twitter:image"'); // the :35-42 block must be INSIDE
  });
  it("ships all nine wrap art assets", () => {
    for (const v of ["clay","rust","mustard","moss","pine","slate","heather","mauve","shelf"]) {
      expect(() => readFileSync(resolve(__dirname, `../public/art/wrap-${v}.png`))).not.toThrow();
    }
  });
});
