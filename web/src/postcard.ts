// 🖼️ the postcard (docs/spec-postcard.md) — the pure model. Helper module per web
// convention (postmark.ts / tags.ts): no DOM, no canvas, twin test file.
import { postmark, waitedYears } from "./postmark";

/** D1: EXACTLY these fields. No gift_url, no key, no token — a capability cannot reach
 *  the canvas because the type has nowhere to put one. `note` is unvetted free text
 *  ben typed (D1's limit): it is null unless the friend ticked D8's toggle. */
export type PostcardInput = {
  title: string;
  artworkUrl: string | null;
  note: string | null;
  acquiredAt: string | null;
  unwrappedAt: string;
};

// M4 (plan review): a `satisfies (keyof T)[]` list only proves SUBSET. A Record over
// keyof PostcardInput fails to compile when a key is ADDED to the type without being
// listed here — so a `giftUrl` field cannot slip in with the D1 test still green.
const KEYS_EXACT: Record<keyof PostcardInput, true> = {
  title: true,
  artworkUrl: true,
  note: true,
  acquiredAt: true,
  unwrappedAt: true,
};
export const POSTCARD_INPUT_KEYS = Object.keys(
  KEYS_EXACT,
) as (keyof PostcardInput)[];

/** D11: deterministic over every field, in a fixed order. */
export function postcardKey(i: PostcardInput): string {
  return JSON.stringify(POSTCARD_INPUT_KEYS.map((k) => i[k]));
}

export const POSTCARD_W = 1080;
export const POSTCARD_H = 1350;

/** sRGB hex of index.css oklch tokens (converted 2026-10-05; canvas gets no oklch). */
export const POSTCARD_PALETTE = {
  paper: "#e3e6b0", // --color-floor
  mat: "#d5da99", // --color-room
  frame: "#363f19", // --color-pixel
  ink: "#2a320b", // --color-ink
  dust: "#4e542c", // --color-dust
  give: "#812258", // --color-give-soft
  line: "#939761", // --color-line
} as const;

const MONTHS = [
  "jan",
  "feb",
  "mar",
  "apr",
  "may",
  "jun",
  "jul",
  "aug",
  "sep",
  "oct",
  "nov",
  "dec",
] as const;

/** D5: the FRIEND'S local date (a 9pm-eastern unwrap is that day, not tomorrow's UTC).
 *  The acquisition month stays postmark()'s UTC — that is ben's order, not this moment. */
function dayLabel(iso: string): string | null {
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return null;
  const d = new Date(t);
  return `${MONTHS[d.getMonth()]} ${d.getDate()}, ${d.getFullYear()}`;
}

export type PostcardText = {
  title: string;
  from: string;
  note: string | null;
  /** The card's headline — "waited 12 years for you" — or null under one whole year /
   *  unknown acquisition. Its own field so it is drawn as ONE line, never wrapped into
   *  the small print (the first real-browser render buried it and orphaned "for you"). */
  waited: string | null;
  postmark: string;
};

export function postcardText(i: PostcardInput): PostcardText {
  const parts: string[] = [];
  const since = postmark(i.acquiredAt ?? undefined);
  if (since !== null) parts.push(`in the attic since ${since}`);
  const day = dayLabel(i.unwrappedAt);
  if (day !== null) parts.push(`unwrapped ${day}`);
  const unwrapT = Date.parse(i.unwrappedAt);
  // D5: inject the unwrap instant — never default-now.
  const years = Number.isNaN(unwrapT)
    ? null
    : waitedYears(i.acquiredAt ?? undefined, unwrapT);
  const waited =
    years === null
      ? null
      : `waited ${years} ${years === 1 ? "year" : "years"} for you`;
  return {
    title: i.title,
    from: "from ben ♡",
    note: i.note,
    waited,
    postmark: parts.join(" · "),
  };
}

/** Word wrap with a hard break for over-long words; the last allowed line is
 *  ellipsised when text remains. `measure` is injected (canvas measureText in prod). */
export function wrapLines(
  text: string,
  maxWidth: number,
  maxLines: number,
  measure: (s: string) => number,
): string[] {
  const words = text.split(/\s+/).filter((w) => w.length > 0);
  const lines: string[] = [];
  let cur = "";
  // review-1 #3: cut by CODE POINT, never by UTF-16 unit — a `slice` can split an
  // emoji's surrogate pair and the canvas draws a broken glyph.
  const pushHardBroken = (w: string) => {
    let rest = [...w];
    while (measure(rest.join("")) > maxWidth && rest.length > 1) {
      let n = rest.length;
      while (n > 1 && measure(rest.slice(0, n).join("")) > maxWidth) n--;
      lines.push(rest.slice(0, n).join(""));
      rest = rest.slice(n);
    }
    return rest.join("");
  };
  for (const w of words) {
    const cand = cur === "" ? w : `${cur} ${w}`;
    if (measure(cand) <= maxWidth) {
      cur = cand;
      continue;
    }
    if (cur !== "") lines.push(cur);
    cur = measure(w) > maxWidth ? pushHardBroken(w) : w;
  }
  if (cur !== "") lines.push(cur);
  if (lines.length <= maxLines) return lines;
  const kept = lines.slice(0, maxLines);
  const last = [...(kept[maxLines - 1] ?? "")];
  while (last.length > 0 && measure(`${last.join("")}…`) > maxWidth) last.pop();
  kept[maxLines - 1] = `${last.join("")}…`;
  return kept;
}

export function postcardFilename(title: string): string {
  const slug = title
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 48);
  return `postcard-${slug === "" ? "gift" : slug}.png`;
}
