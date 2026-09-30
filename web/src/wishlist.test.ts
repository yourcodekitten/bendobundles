import { it, expect } from "vitest";
import { formatSince, floatRanks } from "./wishlist";
it("formats UTC month + year, lowercase", () => {
  expect(formatSince(1678000000)).toBe("mar 2023"); // 2023-03-05T07:06:40Z
});
it("uses UTC, not local time, at a month boundary", () => {
  // CI and the box run TZ=UTC, so without this override a local-time implementation also
  // passes. In Los Angeles this instant is still Mar 31, so only a UTC formatter says apr.
  const old = process.env.TZ;
  process.env.TZ = "America/Los_Angeles";
  try {
    expect(new Date(1680307200000).getDate()).toBe(31); // the override TOOK (else this test is vacuous)
    expect(formatSince(1680307200)).toBe("apr 2023"); // 2023-04-01T00:00:00Z
  } finally {
    process.env.TZ = old;
  }
});
it("returns null for a missing date", () => {
  expect(formatSince(0)).toBeNull();
});

it("floatRanks puts every starred id before every unstarred id", () => {
  let seed = 7; const rand = () => ((seed = (seed * 16807) % 2147483647) / 2147483647);
  const ids = ["a", "b", "c", "d", "e", "f"];
  const r = floatRanks(ids, new Set(["e", "b"]), rand);
  const maxStar = Math.max(r.get("e")!, r.get("b")!);
  const minRest = Math.min(...["a", "c", "d", "f"].map((x) => r.get(x)!));
  expect(maxStar).toBeLessThan(minRest);
  expect(new Set(r.values()).size).toBe(6);
});
it("floatRanks with nothing starred is a plain shuffle of all ids", () => {
  const r = floatRanks(["a", "b", "c"], new Set(), Math.random);
  expect([...r.values()].sort()).toEqual([0, 1, 2]);
});
