import { describe, it, expect, beforeAll, afterAll } from "vitest";
import {
  POSTCARD_INPUT_KEYS,
  postcardKey,
  postcardText,
  wrapLines,
  postcardFilename,
  type PostcardInput,
} from "./postcard";

const base: PostcardInput = {
  title: "Stardew Valley",
  artworkUrl: "https://hb.imgix.net/stardew.png",
  note: null,
  acquiredAt: "2014-03-02T17:00:00Z",
  unwrappedAt: "2026-10-05T16:00:00Z",
};

// OMBB minor 6: the unwrap day is the friend's LOCAL date (M2), so these assertions are
// zone-dependent by design. Pin the zone (Node re-reads TZ on assignment) and restore it.
const savedTZ = process.env.TZ;
beforeAll(() => {
  process.env.TZ = "America/New_York";
});
afterAll(() => {
  if (savedTZ === undefined) delete process.env.TZ;
  else process.env.TZ = savedTZ;
});

describe("postcard model", () => {
  it("M2: the unwrap date is the LOCAL day — 01:00Z oct 6 is still oct 5 in new york", () => {
    expect(
      postcardText({ ...base, unwrappedAt: "2026-10-06T01:00:00Z" }).postmark,
    ).toContain("unwrapped oct 5, 2026");
  });

  it("D1: the input type has exactly five keys and none is a capability", () => {
    expect([...POSTCARD_INPUT_KEYS].sort()).toEqual(
      ["acquiredAt", "artworkUrl", "note", "title", "unwrappedAt"].sort(),
    );
    // compile-time twin: an object with an extra key must not satisfy the type.
    // The directive sits on the PROPERTY line, so a formatter reflowing the literal
    // cannot move it off the error and silently disarm it (prettier did, once).
    const bad: PostcardInput = {
      ...base,
      // @ts-expect-error gift_url is not a PostcardInput field (D1)
      gift_url: "https://humblebundle.com/gift?key=x",
    };
    void bad;
  });

  it("D11: the key changes when the note toggles, and only then for the same game", () => {
    const off = postcardKey(base);
    const on = postcardKey({ ...base, note: "for you ♡" });
    expect(on).not.toBe(off);
    expect(postcardKey({ ...base })).toBe(off);
  });

  it("D11: the key covers every field", () => {
    const k = postcardKey(base);
    for (const f of POSTCARD_INPUT_KEYS) {
      const changed = {
        ...base,
        [f]: f === "note" ? "x" : `${String(base[f])}!`,
      };
      expect(postcardKey(changed as PostcardInput), f).not.toBe(k);
    }
  });

  it("D5: the postmark line uses the UNWRAP instant, not now", () => {
    // unwrapped 2015-03-01, one day before the first anniversary: 0 whole years ⇒ no "waited" clause,
    // even though today is many years later. A default-now call would say "waited 12 years".
    const t = postcardText({ ...base, unwrappedAt: "2015-03-01T12:00:00Z" });
    expect(t.postmark).toBe(
      "in the attic since mar 2014 · unwrapped mar 1, 2015",
    );
  });

  it("D5: ≥1 whole year yields the WAITED headline, singular and plural — its own field, not buried in the postmark", () => {
    const t = postcardText(base);
    expect(t.waited).toBe("waited 12 years for you");
    expect(t.postmark).toBe(
      "in the attic since mar 2014 · unwrapped oct 5, 2026",
    );
    expect(
      postcardText({ ...base, unwrappedAt: "2015-03-02T18:00:00Z" }).waited,
    ).toBe("waited 1 year for you");
  });

  it("under one year ⇒ no waited headline at all", () => {
    expect(
      postcardText({ ...base, unwrappedAt: "2015-03-01T12:00:00Z" }).waited,
    ).toBeNull();
  });

  it("unknown acquiredAt drops the attic clause and the waited clause", () => {
    expect(postcardText({ ...base, acquiredAt: null }).postmark).toBe(
      "unwrapped oct 5, 2026",
    );
    expect(postcardText({ ...base, acquiredAt: null }).waited).toBeNull();
  });

  it("note passes through; from-line is fixed", () => {
    const t = postcardText({ ...base, note: "for you ♡" });
    expect(t.note).toBe("for you ♡");
    expect(t.from).toBe("from ben ♡");
    expect(postcardText(base).note).toBeNull();
  });

  it("wrapLines wraps on words and ellipsises the last allowed line", () => {
    const measure = (s: string) => s.length; // 1 unit per char
    expect(wrapLines("aa bb cc", 5, 3, measure)).toEqual(["aa bb", "cc"]);
    expect(wrapLines("aa bb cc dd ee", 5, 2, measure)).toEqual([
      "aa bb",
      "cc d…",
    ]);
    expect(wrapLines("abcdefghij", 4, 2, measure)).toEqual(["abcd", "efg…"]);
  });

  it("review-1 #3: wrapping never splits a surrogate pair (hard break AND ellipsis)", () => {
    const m = (s: string) => s.length * 10; // code UNITS: a lone surrogate must be able to "fit" or the arm cannot go red
    const lone = (s: string) =>
      /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/.test(
        s,
      );
    const hard = wrapLines("😘".repeat(10), 55, 5, m);
    const ell = wrapLines("ab cd 😘 ef gh ij kl", 50, 1, m);
    expect(hard.some(lone)).toBe(false);
    expect(ell.some(lone)).toBe(false);
    expect(hard.join("")).toMatch(/^(😘)+(…)?$/u);
  });

  it("OMBB m5: wrapping never splits a GRAPHEME — ZWJ families, flags, skin tones stay whole", () => {
    const m = (s: string) => s.length * 10; // code units: half a cluster must be able to "fit"
    const family = "👨‍👩‍👧"; // 8 code units, one grapheme
    const flag = "🇺🇸"; // 4 code units, one grapheme
    const wave = "👋🏽"; // 4 code units, one grapheme
    for (const g of [family, flag, wave]) {
      // 115 = 11 code units: one family (8) + 👨+ZWJ (3) fits; two flags (8) + one regional
      // indicator (2) fits; two waves (8) + 👋 (2) fits — every cut lands MID-cluster unless graphemes are respected
      const lines = [
        ...wrapLines(g.repeat(6), 115, 6, m),
        ...wrapLines(`ab ${g.repeat(4)} cd ef`, 115, 1, m),
      ];
      for (const l of lines) {
        const body = l.endsWith("…") ? l.slice(0, -1) : l;
        expect(
          body.split(g).join(""),
          `split inside ${g}: ${JSON.stringify(l)}`,
        ).toMatch(/^[a-z ]*$/);
      }
    }
  });

  it("filename is a safe slug", () => {
    expect(postcardFilename("Stardew Valley: Deluxe!")).toBe(
      "postcard-stardew-valley-deluxe.png",
    );
    expect(postcardFilename("☆☆☆")).toBe("postcard-gift.png");
  });
});
