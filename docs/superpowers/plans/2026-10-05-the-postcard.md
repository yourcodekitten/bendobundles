# the postcard 🖼️ Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A friend can save a 1080×1350 postcard PNG of a gift (cover art, title, "from ben ♡", optionally ben's note, the postmark line) from the unwrap dialog and from their shelf, via the phone share sheet or a download — and the card carries no capability field.

**Architecture:** Pure model (`web/src/postcard.ts`: input type, cache key, text lines, wrapping) → canvas drawer + asset loader (`web/src/postcardCanvas.ts`) → one React panel (`web/src/friend/PostcardPanel.tsx`) that loads assets ONCE, re-renders synchronously per input, and stores `{key, blob}` → mounted in `ClaimDialog`'s gifted step and on each `ShelfPage` gift. One backend field: `ShelfGift.acquired_at`.

**Tech Stack:** React 19 + TypeScript, vitest + happy-dom + Testing Library (web); Rust/axum + DynamoDB-local integration tests (public-api).

**Spec:** `docs/spec-postcard.md` (v4, `9905dc4`). Decisions D1–D11 there are law; this plan cites them by number.

## Global Constraints

- **D1:** `PostcardInput` has EXACTLY these keys: `title`, `artworkUrl`, `note`, `acquiredAt`, `unwrappedAt`. Never `gift_url`, `revealed_key`, `token`, `shelf_token`, any URL other than `artworkUrl`. The card draws no URL text of any kind. The note is unvetted free text — D8 (default OFF) is the mitigation.
- **D2:** art loads ONLY via `new Image()` with `crossOrigin = "anonymous"`, 4000 ms cap; failure ⇒ drawn placeholder, never a blocked save. **Never `fetch()` the art** (`connect-src` lacks `hb.imgix.net`).
- **D3:** `document.fonts.load()` for the faces used, same 4000 ms cap; timeout ⇒ render anyway.
- **D5:** the date/years math MUST inject the unwrap instant: `waitedYears(acquiredAt, Date.parse(unwrappedAt))`. A default-now call is a bug (drifts +1 every january).
- **D6:** if `HTMLCanvasElement.prototype.toBlob` is missing, the "send a postcard ♡" affordance does not render.
- **D7:** no CSP change; the preview is the `<canvas>` element itself — **never `<img src="blob:…">`** (`img-src` has no `blob:`).
- **D8:** note toggle label `include ben's note`, default **unchecked**, absent when there is no note. Preview and save come from ONE render function with ONE input.
- **D9:** on the gifted step: below the key block, only in `step === "gifted"`, styled as a **link** (text, underline), not a filled button.
- **D10:** assets load once when the panel mounts; the save tap handler performs no `await` before `navigator.share`. `AbortError` ⇒ do nothing. Any other share rejection ⇒ download fallback.
- **D11:** save is enabled only while `artifact.key === postcardKey(currentInput)`; a render completion whose key ≠ current key is discarded.
- Copy is lowercase, attic voice. Canvas colors are the hex constants in Task 2 (converted from `index.css` oklch tokens); do not use oklch strings on the canvas.
- Commits: GPG-signed (`git commit -S`), author `code kitten <yourcodekitten@gmail.com>`. Web checks: `cd web && npm test -- --run && npm run typecheck && npm run lint`. Rust: `cargo test -p public-api` (DynamoDB-local tests self-skip when it is absent; CI runs them).

## File Structure

| File | Responsibility |
|---|---|
| `crates/public-api/src/lib.rs` (modify `ShelfGift` ~L230, `assemble_shelf` ~L1617) | wire `acquired_at` on shelf gifts (D4) |
| `crates/public-api/tests/api_test.rs` (modify) | prove the field rides and is absent when `None` |
| `terraform/aws-cloudfront.tf` (modify comment block above `site_csp`) | D10's `web-share=(self)` note |
| `web/src/api.ts` (modify `ShelfGiftView`) | `acquired_at?: string` |
| `web/src/postcard.ts` + `postcard.test.ts` (create) | D1 type, D11 key, D5 lines, wrapping, palette, slug |
| `web/src/postcardCanvas.ts` + `postcardCanvas.test.ts` (create) | asset loading (D2/D3), drawing, `toBlob` |
| `web/src/friend/PostcardPanel.tsx` + `PostcardPanel.test.tsx` (create) | preview, toggle, `{key, blob}`, share/download (D8/D10/D11) |
| `web/src/friend/ClaimDialog.tsx` / `.test.tsx` (modify) | D9 entry point + `linkNote` prop |
| `web/src/friend/LinkPage.tsx` (modify L730) | pass `linkNote={data.gift_note}` |
| `web/src/friend/ShelfPage.tsx` / `.test.tsx` (modify) | per-gift entry point |

---

### Task 1: shelf gifts carry `acquired_at` (D4) + the web-share note

**Files:**
- Modify: `crates/public-api/src/lib.rs` (`struct ShelfGift`, `assemble_shelf`'s `gifts.push`)
- Modify: `crates/public-api/tests/api_test.rs` (new test after `shelf_happy_fulfilled_only_no_cross_friend_bleed`)
- Modify: `web/src/api.ts` (`ShelfGiftView`)
- Modify: `terraform/aws-cloudfront.tf` (comment only)

**Interfaces:**
- Produces: JSON key `acquired_at` (rfc3339) on each `/api/s/{token}` gift, ABSENT when unknown; TS `ShelfGiftView.acquired_at?: string`.

- [ ] **Step 1: Write the failing test** (append to `api_test.rs`)

```rust
#[tokio::test]
async fn shelf_gift_carries_acquired_at_and_omits_it_when_unknown() {
    let Some(store) = store_or_skip("shelf-acquired").await else {
        return;
    };
    let mock = MockInvoker::new(FulfillResponse::GiftUrl {
        url: "https://x.com/g".into(),
    });
    let mut g1 = test_game(1);
    g1.acquired_at = Some(datetime!(2014-03-02 17:00 UTC));
    store.put_game(&g1).await.unwrap();
    store.put_game(&test_game(2)).await.unwrap(); // acquired_at: None
    let f = test_friend("f1", "sarah", "aa");
    store.create_friend(&f).await.unwrap();
    store.create_link(&test_link("t1")).await.unwrap();
    store.set_link_friend("t1", Some("f1")).await.unwrap();
    store
        .put_claim(&claim("c1", "t1", 1, ClaimState::Fulfilled, 2024))
        .await
        .unwrap();
    store
        .put_claim(&claim("c2", "t1", 2, ClaimState::Fulfilled, 2025))
        .await
        .unwrap();

    let req = Request::get(format!("/api/s/{}", f.shelf_token))
        .body(Body::empty())
        .unwrap();
    let resp = plain_router(store, mock).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let j = body_json(resp).await;
    let gifts = j["gifts"].as_array().unwrap();
    assert_eq!(gifts.len(), 2);
    assert_eq!(gifts[0]["acquired_at"], "2014-03-02T17:00:00Z");
    assert!(
        gifts[1].get("acquired_at").is_none(),
        "unknown acquired_at is ABSENT, never null: {}",
        gifts[1]
    );
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p public-api --test api_test shelf_gift_carries_acquired_at -- --nocapture`
Expected: FAIL on `gifts[0]["acquired_at"]` (Null ≠ string). If the output says the test was SKIPPED (no DynamoDB local), start it per `crates/dynamo` README / CI's service config before continuing — a skipped red is not a red.

- [ ] **Step 3: Implement**

In `struct ShelfGift` add after `thank_note`:

```rust
    /// 📮 when ben's bundle purchase created this game's order (docs/spec-postcard.md D4);
    /// read from the Game record assemble_shelf already holds. Absent when unknown.
    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    acquired_at: Option<OffsetDateTime>,
```

In `assemble_shelf`'s `gifts.push(ShelfGift { … })` add `acquired_at: game.acquired_at,` after `thank_note,`.

In `web/src/api.ts` `ShelfGiftView`, after `thank_note`:

```ts
  /** 📮 rfc3339 instant ben's bundle order was created; absent when unknown (postcard D4). */
  acquired_at?: string;
```

In `terraform/aws-cloudfront.tf`, inside the comment block directly above `locals { site_csp`, add:

```
# No Permissions-Policy header is set, so `web-share` defaults to self — the postcard's
# navigator.share depends on that (docs/spec-postcard.md D10). If one is ever added it
# MUST carry `web-share=(self)`.
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p public-api` then `cd web && npm run typecheck`
Expected: PASS (new test green; existing shelf tests unchanged — `open_shelf_*` wire-shape tests concern the LINK payload, not the shelf).

- [ ] **Step 5: Commit**

```bash
git add crates/public-api terraform/aws-cloudfront.tf web/src/api.ts
git commit -S -m "📮 shelf gifts carry acquired_at (postcard D4)"
```

---

### Task 2: `postcard.ts` — the pure model (D1, D5, D11)

**Files:**
- Create: `web/src/postcard.ts`, `web/src/postcard.test.ts`

**Interfaces:**
- Consumes: `postmark`, `waitedYears` from `web/src/postmark.ts`.
- Produces:
  - `type PostcardInput = { title: string; artworkUrl: string | null; note: string | null; acquiredAt: string | null; unwrappedAt: string }`
  - `const POSTCARD_INPUT_KEYS: readonly (keyof PostcardInput)[]`
  - `function postcardKey(i: PostcardInput): string`
  - `type PostcardText = { title: string; from: string; note: string | null; postmark: string }`
  - `function postcardText(i: PostcardInput): PostcardText`
  - `function wrapLines(text: string, maxWidth: number, maxLines: number, measure: (s: string) => number): string[]`
  - `function postcardFilename(title: string): string`
  - `const POSTCARD_PALETTE: { paper: string; frame: string; ink: string; dust: string; give: string; line: string; mat: string }`
  - `const POSTCARD_W = 1080, POSTCARD_H = 1350`

- [ ] **Step 1: Write the failing tests** (`web/src/postcard.test.ts`)

```ts
import { describe, it, expect } from "vitest";
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
  unwrappedAt: "2026-10-05T11:00:00Z",
};

describe("postcard model", () => {
  it("D1: the input type has exactly five keys and none is a capability", () => {
    expect([...POSTCARD_INPUT_KEYS].sort()).toEqual(
      ["acquiredAt", "artworkUrl", "note", "title", "unwrappedAt"].sort(),
    );
    // compile-time twin: an object with an extra key must not satisfy the type
    // @ts-expect-error gift_url is not a PostcardInput field (D1)
    const bad: PostcardInput = { ...base, gift_url: "https://humblebundle.com/gift?key=x" };
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
      const changed = { ...base, [f]: f === "note" ? "x" : `${String(base[f])}!` };
      expect(postcardKey(changed as PostcardInput), f).not.toBe(k);
    }
  });

  it("D5: the postmark line uses the UNWRAP instant, not now", () => {
    // unwrapped 2015-03-01, one day before the first anniversary: 0 whole years ⇒ no "waited" clause,
    // even though today is many years later. A default-now call would say "waited 12 years".
    const t = postcardText({ ...base, unwrappedAt: "2015-03-01T12:00:00Z" });
    expect(t.postmark).toBe("in the attic since mar 2014 · unwrapped mar 1, 2015");
  });

  it("D5: ≥1 whole year adds the waited clause, singular and plural", () => {
    expect(postcardText(base).postmark).toBe(
      "in the attic since mar 2014 · unwrapped oct 5, 2026 · waited 12 years for you",
    );
    expect(postcardText({ ...base, unwrappedAt: "2015-03-02T18:00:00Z" }).postmark).toContain(
      "waited 1 year for you",
    );
  });

  it("unknown acquiredAt drops the attic clause and the waited clause", () => {
    expect(postcardText({ ...base, acquiredAt: null }).postmark).toBe("unwrapped oct 5, 2026");
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
    expect(wrapLines("aa bb cc dd ee", 5, 2, measure)).toEqual(["aa bb", "cc d…"]);
    expect(wrapLines("abcdefghij", 4, 2, measure)).toEqual(["abcd", "efg…"]);
  });

  it("filename is a safe slug", () => {
    expect(postcardFilename("Stardew Valley: Deluxe!")).toBe("postcard-stardew-valley-deluxe.png");
    expect(postcardFilename("☆☆☆")).toBe("postcard-gift.png");
  });
});
```

- [ ] **Step 2: Run to verify failure**

Run: `cd web && npx vitest run src/postcard.test.ts`
Expected: FAIL — `Cannot find module './postcard'`.

- [ ] **Step 3: Implement** (`web/src/postcard.ts`)

```ts
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

export const POSTCARD_INPUT_KEYS = [
  "title",
  "artworkUrl",
  "note",
  "acquiredAt",
  "unwrappedAt",
] as const satisfies readonly (keyof PostcardInput)[];

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

const MONTHS = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"] as const;

function dayLabel(iso: string): string | null {
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return null;
  const d = new Date(t);
  return `${MONTHS[d.getUTCMonth()]} ${d.getUTCDate()}, ${d.getUTCFullYear()}`;
}

export type PostcardText = {
  title: string;
  from: string;
  note: string | null;
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
  const years = Number.isNaN(unwrapT) ? null : waitedYears(i.acquiredAt ?? undefined, unwrapT);
  if (years !== null) parts.push(`waited ${years} ${years === 1 ? "year" : "years"} for you`);
  return { title: i.title, from: "from ben ♡", note: i.note, postmark: parts.join(" · ") };
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
  const pushHardBroken = (w: string) => {
    let rest = w;
    while (measure(rest) > maxWidth && rest.length > 1) {
      let n = rest.length;
      while (n > 1 && measure(rest.slice(0, n)) > maxWidth) n--;
      lines.push(rest.slice(0, n));
      rest = rest.slice(n);
    }
    return rest;
  };
  for (const w of words) {
    const cand = cur === "" ? w : `${cur} ${w}`;
    if (measure(cand) <= maxWidth) { cur = cand; continue; }
    if (cur !== "") lines.push(cur);
    cur = measure(w) > maxWidth ? pushHardBroken(w) : w;
  }
  if (cur !== "") lines.push(cur);
  if (lines.length <= maxLines) return lines;
  const kept = lines.slice(0, maxLines);
  let last = kept[maxLines - 1];
  while (last.length > 0 && measure(`${last}…`) > maxWidth) last = last.slice(0, -1);
  kept[maxLines - 1] = `${last}…`;
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
```

⚠️ The `wrapLines` ellipsis test (`"aa bb cc dd ee"`, width 5, 2 lines ⇒ `["aa bb", "cc d…"]`): line 2 is `"cc dd"` (5) then `"cc dd…"` (6) > 5 ⇒ trim to `"cc d…"`. And `"abcdefghij"` width 4 ⇒ hard-broken `abcd`,`efgh`,`ij` ⇒ 2 lines ⇒ `"efgh…"`(5)>4 ⇒ `"efg…"`. If your implementation disagrees with these expectations, fix the implementation, not the test.

- [ ] **Step 4: Run tests** — `cd web && npx vitest run src/postcard.test.ts && npm run typecheck` ⇒ PASS.

- [ ] **Step 5: Commit**

```bash
git add web/src/postcard.ts web/src/postcard.test.ts
git commit -S -m "🖼️ postcard model: D1 input type, D11 key, D5 postmark line"
```

---

### Task 3: `postcardCanvas.ts` — assets once (D2/D3), draw, blob

**Files:**
- Create: `web/src/postcardCanvas.ts`, `web/src/postcardCanvas.test.ts`

**Interfaces:**
- Consumes: everything Task 2 produces.
- Produces:
  - `type PostcardAssets = { art: HTMLImageElement | null }`
  - `function loadPostcardAssets(artworkUrl: string | null, timeoutMs?: number): Promise<PostcardAssets>` — never rejects
  - `type DrawCtx = Pick<CanvasRenderingContext2D, "fillStyle" | "strokeStyle" | "lineWidth" | "font" | "textAlign" | "textBaseline" | "fillRect" | "strokeRect" | "fillText" | "measureText" | "drawImage" | "save" | "restore">`
  - `function drawPostcard(ctx: DrawCtx, input: PostcardInput, assets: PostcardAssets): void` — synchronous
  - `function renderPostcardBlob(canvas: HTMLCanvasElement, input: PostcardInput, assets: PostcardAssets): Promise<Blob | null>` — draws then `toBlob("image/png")`; resolves `null` on taint (`SecurityError`) after redrawing WITHOUT art and retrying once
  - `const POSTCARD_FONTS: readonly string[]`
  - `function canMakePostcards(): boolean` — D6

- [ ] **Step 1: Write the failing tests** (`web/src/postcardCanvas.test.ts`)

```ts
import { describe, it, expect, vi, afterEach } from "vitest";
import { drawPostcard, loadPostcardAssets, type DrawCtx } from "./postcardCanvas";
import { postcardText, type PostcardInput } from "./postcard";

const input: PostcardInput = {
  title: "Stardew Valley",
  artworkUrl: "https://hb.imgix.net/stardew.png",
  note: "for you ♡",
  acquiredAt: "2014-03-02T17:00:00Z",
  unwrappedAt: "2026-10-05T11:00:00Z",
};

function recordingCtx() {
  const texts: string[] = [];
  const images: unknown[] = [];
  const ctx = {
    fillStyle: "", strokeStyle: "", lineWidth: 1, font: "", textAlign: "left", textBaseline: "alphabetic",
    fillRect: vi.fn(), strokeRect: vi.fn(), save: vi.fn(), restore: vi.fn(),
    fillText: vi.fn((s: string) => { texts.push(s); }),
    measureText: vi.fn((s: string) => ({ width: s.length * 20 }) as TextMetrics),
    drawImage: vi.fn((img: unknown) => { images.push(img); }),
  } as unknown as DrawCtx;
  return { ctx, texts, images };
}

afterEach(() => vi.restoreAllMocks());

describe("drawPostcard", () => {
  it("draws the title, from-line, note and postmark — and NO url text (D1)", () => {
    const { ctx, texts } = recordingCtx();
    drawPostcard(ctx, input, { art: null });
    const all = texts.join("\n");
    const t = postcardText(input);
    expect(all).toContain("Stardew Valley");
    expect(all).toContain(t.from);
    expect(all).toContain("for you ♡");
    expect(all).toContain("waited 12 years for you");
    expect(all).toContain("bendobundles");
    expect(all).not.toMatch(/https?:|www\.|\.com\//);
  });

  it("note null ⇒ no quote drawn (D8 off)", () => {
    const { ctx, texts } = recordingCtx();
    drawPostcard(ctx, { ...input, note: null }, { art: null });
    expect(texts.join("\n")).not.toContain("for you ♡");
  });

  it("art present ⇒ drawImage once; art absent ⇒ placeholder, no drawImage (D2)", () => {
    const a = recordingCtx();
    const img = { naturalWidth: 400, naturalHeight: 400 } as HTMLImageElement;
    drawPostcard(a.ctx, input, { art: img });
    expect(a.images).toEqual([img]);
    const b = recordingCtx();
    drawPostcard(b.ctx, input, { art: null });
    expect(b.images).toEqual([]);
  });
});

describe("loadPostcardAssets", () => {
  it("null url ⇒ { art: null } without creating an Image", async () => {
    const spy = vi.spyOn(window, "Image");
    await expect(loadPostcardAssets(null, 50)).resolves.toEqual({ art: null });
    expect(spy).not.toHaveBeenCalled();
  });

  it("sets crossOrigin=anonymous and resolves art on load (D2)", async () => {
    let made: HTMLImageElement | null = null;
    vi.spyOn(window, "Image").mockImplementation(() => {
      const el = document.createElement("img");
      made = el;
      queueMicrotask(() => el.onload?.(new Event("load")));
      return el;
    });
    const a = await loadPostcardAssets("https://hb.imgix.net/x.png", 1000);
    expect(made!.crossOrigin).toBe("anonymous");
    expect(a.art).toBe(made);
  });

  it("error OR timeout ⇒ { art: null }, never rejects (D2)", async () => {
    vi.spyOn(window, "Image").mockImplementation(() => {
      const el = document.createElement("img");
      queueMicrotask(() => el.onerror?.(new Event("error")));
      return el;
    });
    await expect(loadPostcardAssets("https://x/y.png", 1000)).resolves.toEqual({ art: null });
    vi.spyOn(window, "Image").mockImplementation(() => document.createElement("img")); // never settles
    await expect(loadPostcardAssets("https://x/y.png", 20)).resolves.toEqual({ art: null });
  });
});
```

- [ ] **Step 2: Run to verify failure** — `cd web && npx vitest run src/postcardCanvas.test.ts` ⇒ FAIL (module missing).

- [ ] **Step 3: Implement** (`web/src/postcardCanvas.ts`)

```ts
// 🖼️ the postcard's canvas half (docs/spec-postcard.md D2/D3/D10). Assets load ONCE
// per panel; drawPostcard is synchronous so a D8 toggle re-render never awaits.
import {
  POSTCARD_H, POSTCARD_PALETTE as P, POSTCARD_W, postcardText, wrapLines, type PostcardInput,
} from "./postcard";

export type PostcardAssets = { art: HTMLImageElement | null };

export type DrawCtx = Pick<
  CanvasRenderingContext2D,
  "fillStyle" | "strokeStyle" | "lineWidth" | "font" | "textAlign" | "textBaseline"
  | "fillRect" | "strokeRect" | "fillText" | "measureText" | "drawImage" | "save" | "restore"
>;

export const POSTCARD_FONTS = [
  '64px "Pixelify Sans Variable"',
  '36px "Chivo Variable"',
  'italic 36px "Chivo Variable"',
  '28px "Silkscreen"',
] as const;

const SERIF_FALLBACK = "ui-sans-serif, system-ui, sans-serif";
const F = {
  title: `64px "Pixelify Sans Variable", ${SERIF_FALLBACK}`,
  from: `36px "Chivo Variable", ${SERIF_FALLBACK}`,
  note: `italic 36px "Chivo Variable", ${SERIF_FALLBACK}`,
  post: `26px "Chivo Variable", ${SERIF_FALLBACK}`,
  mark: `28px "Silkscreen", ui-monospace, monospace`,
};

export function canMakePostcards(): boolean {
  return typeof HTMLCanvasElement !== "undefined"
    && typeof HTMLCanvasElement.prototype.toBlob === "function";
}

function withTimeout<T>(p: Promise<T>, ms: number, fallback: T): Promise<T> {
  return new Promise((resolve) => {
    const t = setTimeout(() => resolve(fallback), ms);
    p.then((v) => { clearTimeout(t); resolve(v); }, () => { clearTimeout(t); resolve(fallback); });
  });
}

/** D2 + D3. Never rejects. Art via <img crossOrigin>, NEVER fetch (connect-src). */
export async function loadPostcardAssets(
  artworkUrl: string | null,
  timeoutMs = 4000,
): Promise<PostcardAssets> {
  const fonts = typeof document !== "undefined" && document.fonts
    ? Promise.all(POSTCARD_FONTS.map((f) => document.fonts.load(f))).then(() => undefined)
    : Promise.resolve(undefined);
  const art: Promise<HTMLImageElement | null> = artworkUrl === null
    ? Promise.resolve(null)
    : new Promise((resolve) => {
        const img = new Image();
        img.crossOrigin = "anonymous";
        img.onload = () => resolve(img);
        img.onerror = () => resolve(null);
        img.src = artworkUrl;
      });
  const [, a] = await Promise.all([
    withTimeout(fonts, timeoutMs, undefined),
    withTimeout(art, timeoutMs, null),
  ]);
  return { art: a };
}

const M = 72; // outer margin
const ART = { x: M + 24, y: M + 24, w: POSTCARD_W - 2 * (M + 24), h: 620 };

function drawPlaceholder(ctx: DrawCtx) {
  ctx.fillStyle = P.mat;
  ctx.fillRect(ART.x, ART.y, ART.w, ART.h);
  // the house pixel-gift glyph, drawn in blocks (no asset, cannot fail)
  const s = 24, cx = ART.x + ART.w / 2, cy = ART.y + ART.h / 2;
  ctx.fillStyle = P.give;
  ctx.fillRect(cx - 4 * s, cy - 2 * s, 8 * s, 6 * s); // box
  ctx.fillStyle = P.paper;
  ctx.fillRect(cx - s / 2, cy - 2 * s, s, 6 * s); // ribbon v
  ctx.fillRect(cx - 4 * s, cy, 8 * s, s); // ribbon h
  ctx.fillStyle = P.give;
  ctx.fillRect(cx - 3 * s, cy - 4 * s, 2 * s, 2 * s); // bow l
  ctx.fillRect(cx + s, cy - 4 * s, 2 * s, 2 * s); // bow r
}

function drawArt(ctx: DrawCtx, img: HTMLImageElement) {
  ctx.fillStyle = P.mat;
  ctx.fillRect(ART.x, ART.y, ART.w, ART.h);
  const iw = img.naturalWidth || ART.w, ih = img.naturalHeight || ART.h;
  const k = Math.min(ART.w / iw, ART.h / ih); // contain, never crop (spec: the card)
  const w = iw * k, h = ih * k;
  ctx.drawImage(img, ART.x + (ART.w - w) / 2, ART.y + (ART.h - h) / 2, w, h);
}

export function drawPostcard(ctx: DrawCtx, input: PostcardInput, assets: PostcardAssets): void {
  const t = postcardText(input);
  ctx.save();
  ctx.fillStyle = P.mat;
  ctx.fillRect(0, 0, POSTCARD_W, POSTCARD_H);
  ctx.fillStyle = P.paper;
  ctx.fillRect(M, M, POSTCARD_W - 2 * M, POSTCARD_H - 2 * M);
  ctx.strokeStyle = P.frame;
  ctx.lineWidth = 6;
  ctx.strokeRect(M, M, POSTCARD_W - 2 * M, POSTCARD_H - 2 * M);
  if (assets.art !== null) drawArt(ctx, assets.art); else drawPlaceholder(ctx);

  const x = ART.x, maxW = ART.w;
  let y = ART.y + ART.h + 80;
  ctx.textAlign = "left";
  ctx.textBaseline = "alphabetic";
  ctx.font = F.title;
  ctx.fillStyle = P.ink;
  for (const line of wrapLines(t.title, maxW, 2, (s) => ctx.measureText(s).width)) {
    ctx.fillText(line, x, y); y += 72;
  }
  y += 8;
  ctx.font = F.from;
  ctx.fillStyle = P.give;
  ctx.fillText(t.from, x, y); y += 52;
  if (t.note !== null) {
    ctx.font = F.note;
    for (const line of wrapLines(`“${t.note}”`, maxW, 3, (s) => ctx.measureText(s).width)) {
      ctx.fillText(line, x, y); y += 46;
    }
  }
  ctx.font = F.post;
  ctx.fillStyle = P.dust;
  const postY = POSTCARD_H - M - 70;
  for (const [i, line] of wrapLines(t.postmark, maxW, 2, (s) => ctx.measureText(s).width).entries()) {
    ctx.fillText(line, x, postY - (1 - i) * 34);
  }
  ctx.font = F.mark;
  ctx.textAlign = "right";
  ctx.fillStyle = P.line;
  ctx.fillText("bendobundles", POSTCARD_W - M - 24, POSTCARD_H - M - 24);
  ctx.restore();
}

function toBlob(canvas: HTMLCanvasElement): Promise<Blob | null> {
  return new Promise((resolve) => {
    try { canvas.toBlob((b) => resolve(b), "image/png"); } catch { resolve(null); }
  });
}

/** Draw + encode. A tainted canvas (SecurityError) redraws WITHOUT art and retries once
 *  — the postcard still saves (D2). Null only when encoding fails twice. */
export async function renderPostcardBlob(
  canvas: HTMLCanvasElement,
  input: PostcardInput,
  assets: PostcardAssets,
): Promise<Blob | null> {
  canvas.width = POSTCARD_W;
  canvas.height = POSTCARD_H;
  const ctx = canvas.getContext("2d");
  if (ctx === null) return null;
  drawPostcard(ctx, input, assets);
  const b = await toBlob(canvas);
  if (b !== null || assets.art === null) return b;
  drawPostcard(ctx, input, { art: null });
  return toBlob(canvas);
}
```

⚠️ The postmark draws bottom-anchored: with 1 line it sits at `postY`; with 2 lines the first sits at `postY - 34`. Notes cap at 3 lines and the title at 2, so with the 620px art box the text block ends ≤ `ART.y+ART.h+80+144+60+138 ≈ 1138` < `postY - 34 ≈ 1176`. If you change any size, re-check that inequality.

- [ ] **Step 4: Run tests** — `cd web && npx vitest run src/postcardCanvas.test.ts && npm run typecheck && npm run lint` ⇒ PASS.

- [ ] **Step 5: Commit**

```bash
git add web/src/postcardCanvas.ts web/src/postcardCanvas.test.ts
git commit -S -m "🖼️ postcard canvas: assets once (D2/D3), synchronous draw, taint retry"
```

---

### Task 4: `PostcardPanel.tsx` — preview, toggle, `{key, blob}`, share (D8/D10/D11)

**Files:**
- Create: `web/src/friend/PostcardPanel.tsx`, `web/src/friend/PostcardPanel.test.tsx`

**Interfaces:**
- Consumes: Task 2 (`PostcardInput`, `postcardKey`, `postcardFilename`), Task 3 (`loadPostcardAssets`, `renderPostcardBlob`, `PostcardAssets`).
- Produces: `export function PostcardPanel(props: { base: Omit<PostcardInput, "note">; note: string | null }): JSX.Element` — `note` is ben's note if any; the panel decides (D8) whether it enters the input.

Behaviour contract:
1. On mount: `loadPostcardAssets(base.artworkUrl)` once (state `assets: PostcardAssets | null`). While null: text `getting your postcard ready…`, save disabled.
2. Toggle: `<input type="checkbox">` labelled `include ben's note`, rendered only when `note !== null`, default unchecked.
3. `current: PostcardInput = { ...base, note: include ? note : null }`, `curKey = postcardKey(current)`.
4. Effect on `[assets, curKey]`: call `renderPostcardBlob(canvasRef.current, current, assets)`; on completion, store `{ key, blob }` ONLY if `key === latestKeyRef.current` (D11 discard).
5. Save button `send a postcard ♡` (inside the panel this IS a button — D9 governs only the entry link) — `disabled` unless `artifact !== null && artifact.blob !== null && artifact.key === curKey`.
6. onClick (synchronous until share): re-check key; build `new File([blob], postcardFilename(base.title), { type: "image/png" })`; if `navigator.canShare?.({ files: [file] })` ⇒ `navigator.share({ files: [file] })`, `.catch(e => { if (e?.name !== "AbortError") download(blob) })`; else `download(blob)`.
7. `download(blob)`: `URL.createObjectURL` → temporary `<a download=filename>` → `.click()` → `URL.revokeObjectURL` in a `setTimeout(…, 0)`.
8. The preview is the `<canvas>` itself, `aria-label="postcard preview"`, CSS `w-full max-w-[270px] h-auto` (D7 — never an `<img src=blob:>`).
9. On `blob === null` after render: text `couldn't make the postcard this time` and save stays disabled.

- [ ] **Step 1: Write the failing tests** (`web/src/friend/PostcardPanel.test.tsx`)

```tsx
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, it, expect, vi, beforeEach } from "vitest";

vi.mock("../postcardCanvas", () => ({
  loadPostcardAssets: vi.fn(),
  renderPostcardBlob: vi.fn(),
}));
import { loadPostcardAssets, renderPostcardBlob } from "../postcardCanvas";
import { PostcardPanel } from "./PostcardPanel";
import type { PostcardInput } from "../postcard";

const base: Omit<PostcardInput, "note"> = {
  title: "Stardew Valley",
  artworkUrl: null,
  acquiredAt: "2014-03-02T17:00:00Z",
  unwrappedAt: "2026-10-05T11:00:00Z",
};

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(loadPostcardAssets).mockResolvedValue({ art: null });
  vi.mocked(renderPostcardBlob).mockImplementation(async (_c, input) =>
    new Blob([JSON.stringify(input)], { type: "image/png" }),
  );
});

describe("PostcardPanel", () => {
  it("loads assets ONCE and renders a canvas preview, not an <img> (D7/D10)", async () => {
    render(<PostcardPanel base={base} note="for you ♡" />);
    await waitFor(() => expect(screen.getByRole("button", { name: /send a postcard/ })).toBeEnabled());
    expect(loadPostcardAssets).toHaveBeenCalledTimes(1);
    expect(screen.getByLabelText("postcard preview").tagName).toBe("CANVAS");
    await userEvent.click(screen.getByLabelText(/include ben's note/));
    await waitFor(() => expect(renderPostcardBlob).toHaveBeenCalledTimes(2));
    expect(loadPostcardAssets).toHaveBeenCalledTimes(1);
  });

  it("D8: the toggle is OFF by default and absent without a note", async () => {
    const { unmount } = render(<PostcardPanel base={base} note="for you ♡" />);
    expect(screen.getByLabelText(/include ben's note/)).not.toBeChecked();
    await waitFor(() => expect(renderPostcardBlob).toHaveBeenCalled());
    expect(vi.mocked(renderPostcardBlob).mock.calls[0][1].note).toBeNull();
    unmount();
    render(<PostcardPanel base={base} note={null} />);
    expect(screen.queryByLabelText(/include ben's note/)).toBeNull();
  });

  it("D11: save is disabled while the blob is for an older input, and a stale completion is discarded", async () => {
    let releaseFirst!: (b: Blob) => void;
    vi.mocked(renderPostcardBlob)
      .mockImplementationOnce(() => new Promise((r) => { releaseFirst = r; }))
      .mockImplementation(async (_c, input) => new Blob([String(input.note)], { type: "image/png" }));
    render(<PostcardPanel base={base} note="for you ♡" />);
    await waitFor(() => expect(renderPostcardBlob).toHaveBeenCalledTimes(1));
    await userEvent.click(screen.getByLabelText(/include ben's note/)); // note ON while note-OFF render is pending
    await waitFor(() => expect(renderPostcardBlob).toHaveBeenCalledTimes(2));
    releaseFirst(new Blob(["STALE"], { type: "image/png" })); // the OFF render finishes LAST
    const share = vi.fn().mockResolvedValue(undefined);
    Object.assign(navigator, { canShare: () => true, share });
    const btn = await screen.findByRole("button", { name: /send a postcard/ });
    await waitFor(() => expect(btn).toBeEnabled());
    await userEvent.click(btn);
    const file = share.mock.calls[0][0].files[0] as File;
    expect(await file.text()).toBe("for you ♡"); // the CURRENT input's blob, never "STALE"
  });

  it("D10: AbortError is a cancel — no download fallback", async () => {
    const share = vi.fn().mockRejectedValue(Object.assign(new Error("x"), { name: "AbortError" }));
    Object.assign(navigator, { canShare: () => true, share });
    const create = vi.spyOn(URL, "createObjectURL");
    render(<PostcardPanel base={base} note={null} />);
    const btn = await screen.findByRole("button", { name: /send a postcard/ });
    await waitFor(() => expect(btn).toBeEnabled());
    await userEvent.click(btn);
    await waitFor(() => expect(share).toHaveBeenCalled());
    expect(create).not.toHaveBeenCalled();
  });

  it("no canShare ⇒ download with the slug filename", async () => {
    Object.assign(navigator, { canShare: undefined, share: undefined });
    const create = vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:x");
    const click = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {});
    render(<PostcardPanel base={base} note={null} />);
    const btn = await screen.findByRole("button", { name: /send a postcard/ });
    await waitFor(() => expect(btn).toBeEnabled());
    await userEvent.click(btn);
    expect(create).toHaveBeenCalled();
    expect((click.mock.contexts[0] as HTMLAnchorElement).download).toBe("postcard-stardew-valley.png");
  });

  it("render failure ⇒ soft message, save stays disabled", async () => {
    vi.mocked(renderPostcardBlob).mockResolvedValue(null);
    render(<PostcardPanel base={base} note={null} />);
    expect(await screen.findByText(/couldn't make the postcard this time/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /send a postcard/ })).toBeDisabled();
  });
});
```

- [ ] **Step 2: Run to verify failure** — `cd web && npx vitest run src/friend/PostcardPanel.test.tsx` ⇒ FAIL (module missing).

- [ ] **Step 3: Implement** (`web/src/friend/PostcardPanel.tsx`)

```tsx
import { useEffect, useMemo, useRef, useState } from "react";
import { postcardFilename, postcardKey, type PostcardInput } from "../postcard";
import { loadPostcardAssets, renderPostcardBlob, type PostcardAssets } from "../postcardCanvas";

// 🖼️ the postcard panel (docs/spec-postcard.md D7/D8/D10/D11). Assets load once on
// mount; every input change re-renders synchronously-then-encodes; the stored artifact
// is a {key, blob} PAIR and save is live only while its key is the current input's.

type Artifact = { key: string; blob: Blob | null };

function download(blob: Blob, filename: string) {
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 0);
}

export function PostcardPanel({
  base,
  note,
}: {
  base: Omit<PostcardInput, "note">;
  note: string | null;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [assets, setAssets] = useState<PostcardAssets | null>(null);
  const [includeNote, setIncludeNote] = useState(false); // D8: default OFF
  const [artifact, setArtifact] = useState<Artifact | null>(null);

  const current: PostcardInput = useMemo(
    () => ({ ...base, note: includeNote && note !== null ? note : null }),
    [base, includeNote, note],
  );
  const curKey = postcardKey(current);
  const latestKey = useRef(curKey);
  latestKey.current = curKey;

  // D10: load ONCE. base.artworkUrl is fixed for the panel's life.
  const artworkUrl = base.artworkUrl;
  useEffect(() => {
    let live = true;
    void loadPostcardAssets(artworkUrl).then((a) => { if (live) setAssets(a); });
    return () => { live = false; };
  }, [artworkUrl]);

  useEffect(() => {
    if (assets === null || canvasRef.current === null) return;
    const key = curKey;
    void renderPostcardBlob(canvasRef.current, current, assets).then((blob) => {
      if (key === latestKey.current) setArtifact({ key, blob }); // D11: stale ⇒ discarded
    });
    // `current` is fully determined by curKey
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [assets, curKey]);

  const ready = artifact !== null && artifact.blob !== null && artifact.key === curKey;
  const failed = artifact !== null && artifact.key === curKey && artifact.blob === null;

  function onSave() {
    // synchronous until share (D10): no await before navigator.share
    if (artifact === null || artifact.blob === null || artifact.key !== latestKey.current) return;
    const filename = postcardFilename(base.title);
    const blob = artifact.blob;
    const file = new File([blob], filename, { type: "image/png" });
    if (typeof navigator.canShare === "function" && navigator.canShare({ files: [file] })) {
      navigator.share({ files: [file] }).catch((e: unknown) => {
        if ((e as { name?: string } | null)?.name !== "AbortError") download(blob, filename);
      });
      return;
    }
    download(blob, filename);
  }

  return (
    <div className="mt-3 flex flex-col items-center gap-3">
      <canvas
        ref={canvasRef}
        aria-label="postcard preview"
        className="h-auto w-full max-w-[270px] rounded shadow"
      />
      {assets === null && <p className="text-xs text-dust">getting your postcard ready…</p>}
      {failed && <p className="text-xs text-dust">couldn&apos;t make the postcard this time</p>}
      {note !== null && (
        <label className="flex items-center gap-2 text-sm text-ink-soft">
          <input
            type="checkbox"
            checked={includeNote}
            onChange={(e) => setIncludeNote(e.target.checked)}
          />
          include ben&apos;s note
        </label>
      )}
      <button
        type="button"
        disabled={!ready}
        onClick={onSave}
        className="rounded bg-give px-4 py-2 text-sm text-give-ink transition-colors hover:bg-give-bright disabled:opacity-50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-pixel focus-visible:ring-offset-2 focus-visible:ring-offset-floor"
      >
        send a postcard ♡
      </button>
    </div>
  );
}
```

- [ ] **Step 4: Run tests** — `cd web && npx vitest run src/friend/PostcardPanel.test.tsx && npm run typecheck && npm run lint` ⇒ PASS. If happy-dom's `canvas.getContext` returns null that is fine — `renderPostcardBlob` is mocked here.

- [ ] **Step 5: Commit**

```bash
git add web/src/friend/PostcardPanel.tsx web/src/friend/PostcardPanel.test.tsx
git commit -S -m "🖼️ PostcardPanel: note off by default (D8), tap only shares (D10), {key, blob} (D11)"
```

---

### Task 5: the two doorways — unwrap (D9) and shelf

**Files:**
- Modify: `web/src/friend/ClaimDialog.tsx` (props + gifted block), `web/src/friend/ClaimDialog.test.tsx`
- Modify: `web/src/friend/LinkPage.tsx` (~L730)
- Modify: `web/src/friend/ShelfPage.tsx` (gift `<li>`), `web/src/friend/ShelfPage.test.tsx`

**Interfaces:**
- Consumes: `PostcardPanel` (Task 4), `canMakePostcards` (Task 3), `ShelfGiftView.acquired_at` (Task 1).
- Produces: `ClaimDialogProps.linkNote?: string` (optional — existing tests stay valid).

Entry-point contract (both pages): a text-styled toggle `send a postcard ♡` (class `text-sm text-give-soft underline hover:text-give`, `aria-expanded`), rendered only when `canMakePostcards()`; clicking mounts `<PostcardPanel>` below it.
- ClaimDialog: only inside `step === "gifted" && result?.kind === "gifted"`, placed AFTER the `keys may be region-locked` line and BEFORE the close row. Input: `{ title: game.title, artworkUrl: game.artwork_url, acquiredAt: game.acquired_at ?? null, unwrappedAt: <ISO captured at the moment the step became "gifted"> }`, `note = game.note ?? linkNote ?? null` (per-game curated note wins, spec "the card").
- ShelfPage: inside each gift's text column, after the thank-note paragraph. Input: `{ title, artworkUrl: gift.artwork_url, acquiredAt: gift.acquired_at ?? null, unwrappedAt: gift.unwrapped_at }`, `note = gift.gift_note`.

- [ ] **Step 1: Write the failing tests**

Add the `vi.mock("../postcardCanvas", …)` block at the TOP LEVEL of `ClaimDialog.test.tsx` (next to the existing `vi.mock('../api')`), and the `it(…)` inside `describe('gifted path', …)`:

```tsx
vi.mock("../postcardCanvas", async (orig) => ({
  ...(await orig<typeof import("../postcardCanvas")>()),
  canMakePostcards: () => true,
  loadPostcardAssets: vi.fn().mockResolvedValue({ art: null }),
  renderPostcardBlob: vi.fn().mockResolvedValue(new Blob(["x"], { type: "image/png" })),
}));

it("D9: a postcard link appears only after the key, as a link not a button, and its input never sees the gift url", async () => {
  const user = userEvent.setup();
  vi.mocked(claimGame).mockResolvedValue({ kind: "gifted", gift_url: GIFT_URL });
  render(
    <ClaimDialog
      token="tok"
      game={{ ...mockGame, acquired_at: "2014-03-02T17:00:00Z" }}
      linkNote="for you ♡"
      onClose={onClose}
      onRefresh={onRefresh}
    />,
  );
  expect(screen.queryByText(/send a postcard/)).toBeNull(); // confirm step
  await user.click(screen.getByRole("button", { name: /confirm/i }));
  await waitFor(() => expect(screen.getByText(GIFT_URL)).toBeInTheDocument());
  const entry = screen.getByRole("button", { name: /send a postcard/ });
  expect(entry.className).toContain("underline");
  expect(entry.className).not.toContain("bg-give");
  await user.click(entry);
  const { renderPostcardBlob } = await import("../postcardCanvas");
  await waitFor(() => expect(renderPostcardBlob).toHaveBeenCalled());
  const input = vi.mocked(renderPostcardBlob).mock.calls[0][1];
  expect(JSON.stringify(input)).not.toContain("abc123xyz"); // D1 end-to-end: GIFT_URL's key
  expect(input.title).toBe("Hollow Knight");
  expect(input.acquiredAt).toBe("2014-03-02T17:00:00Z");
  expect(input.note).toBeNull(); // D8: off by default even with a linkNote
});
```

This uses the file's own harness (`vi.mock('../api')` auto-mock, `mockGame`, `GIFT_URL`, `onClose`, `onRefresh`) — place it inside `describe('gifted path', …)`. The entry is a `<button>` (it toggles UI) styled as a link; D9 is about visual weight, which the class assertions pin.
📏 **EXISTING FOCUS TEST — read, expected to pass UNCHANGED:** `'gifted step (multi-focusable, one-time URL): Tab wraps…'` (~L351) focuses `close` and asserts Tab stays inside the dialog and leaves `close`. The postcard entry renders BEFORE the close row, so `close` is still the last focusable and the assertion is unaffected. **If it goes red anyway, that is a finding about focus order — stop and report it; do not edit the test to pass.**

Append to `ShelfPage.test.tsx`:

```tsx
vi.mock("../postcardCanvas", async (orig) => ({
  ...(await orig<typeof import("../postcardCanvas")>()),
  canMakePostcards: () => true,
  loadPostcardAssets: vi.fn().mockResolvedValue({ art: null }),
  renderPostcardBlob: vi.fn().mockResolvedValue(new Blob(["x"], { type: "image/png" })),
}));

it("each gift offers a postcard carrying its acquired_at and unwrap instant", async () => {
  vi.mocked(fetchShelf).mockResolvedValue({
    name: "sarah",
    gifts: [{
      game_id: "g1", title: "Stardew Valley", artwork_url: null,
      unwrapped_at: "2026-10-05T11:00:00Z", gift_note: "for you ♡", thank_note: null,
      acquired_at: "2014-03-02T17:00:00Z",
    }],
  });
  renderShelfPage();
  await userEvent.click(await screen.findByRole("button", { name: /send a postcard/ }));
  const { renderPostcardBlob } = await import("../postcardCanvas");
  await waitFor(() => expect(renderPostcardBlob).toHaveBeenCalled());
  expect(vi.mocked(renderPostcardBlob).mock.calls[0][1]).toEqual({
    title: "Stardew Valley", artworkUrl: null, note: null,
    acquiredAt: "2014-03-02T17:00:00Z", unwrappedAt: "2026-10-05T11:00:00Z",
  });
});
```

- [ ] **Step 2: Run to verify failure** — `cd web && npx vitest run src/friend/ClaimDialog.test.tsx src/friend/ShelfPage.test.tsx` ⇒ the two new tests FAIL (no `send a postcard` button).

- [ ] **Step 3: Implement**

ClaimDialog: add `linkNote?: string;` to `ClaimDialogProps` and destructure it. Add state:

```tsx
const [postcardOpen, setPostcardOpen] = useState(false);
const [unwrappedAt, setUnwrappedAt] = useState<string | null>(null);
```

Wherever the code calls `setStep("gifted")` (both the `celebrating` `onDone` and the reduced-motion direct path), also call `setUnwrappedAt((u) => u ?? new Date().toISOString())` (D5: the client's now at success; set once). Then inside the gifted fragment, after the `keys may be region-locked` paragraph:

```tsx
{canMakePostcards() && unwrappedAt !== null && (
  <div className="mt-4 text-center">
    <button
      type="button"
      aria-expanded={postcardOpen}
      onClick={() => setPostcardOpen((o) => !o)}
      className="text-sm text-give-soft underline hover:text-give focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-pixel"
    >
      send a postcard ♡
    </button>
    {postcardOpen && (
      <PostcardPanel
        base={{
          title: game.title,
          artworkUrl: game.artwork_url,
          acquiredAt: game.acquired_at ?? null,
          unwrappedAt,
        }}
        note={game.note ?? linkNote ?? null}
      />
    )}
  </div>
)}
```

⚠️ `base` is a new object each render; `PostcardPanel`'s asset effect depends only on `base.artworkUrl` and its render effect on `curKey`, so this does not re-load or loop. Do not "fix" it with `useMemo` unless a test shows a loop.

LinkPage L730: add `linkNote={data.gift_note}` to `<ClaimDialog …>` (use whatever the loaded `LinkView` variable is named there; `gift_note` is `string | undefined`).

ShelfPage: add `const [openPostcard, setOpenPostcard] = useState<string | null>(null);` and after the thank-note block inside each gift:

```tsx
{canMakePostcards() && (
  <div className="mt-2">
    <button
      type="button"
      aria-expanded={openPostcard === gift.game_id}
      onClick={() => setOpenPostcard((g) => (g === gift.game_id ? null : gift.game_id))}
      className="text-sm text-give-soft underline hover:text-give focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-pixel"
    >
      send a postcard ♡
    </button>
    {openPostcard === gift.game_id && (
      <PostcardPanel
        base={{
          title: gift.title,
          artworkUrl: gift.artwork_url,
          acquiredAt: gift.acquired_at ?? null,
          unwrappedAt: gift.unwrapped_at,
        }}
        note={gift.gift_note}
      />
    )}
  </div>
)}
```

Imports: `import { PostcardPanel } from "./PostcardPanel"; import { canMakePostcards } from "../postcardCanvas";` in both files; `useState` in ShelfPage if not already imported.

- [ ] **Step 4: Run the full web suite** — `cd web && npm test -- --run && npm run typecheck && npm run lint && npm run build` ⇒ all PASS. Existing ClaimDialog/ShelfPage/LinkPage tests must stay green untouched (the prop is optional; the entry only appears in the gifted step / when canvas exists — happy-dom: if `canMakePostcards()` is true there, existing snapshot-free tests are unaffected because they query by role/text that does not overlap `send a postcard`).

- [ ] **Step 5: Commit**

```bash
git add web/src/friend
git commit -S -m "🖼️ postcard doorways: after the key on the unwrap (D9), and on every shelf gift"
```

---

### Task 6: real-browser proof (no new code)

**Files:** none (verification only; findings that need code go back to the owning task).

- [ ] **Step 1:** `cd web && npm run build && npx vite preview --port 4173 &` then, with the playwright tools, open the ShelfPage route against a stubbed API is NOT available — instead verify the **pure render** in a real Chromium: open `about:blank`, inject the built `postcardCanvas` via `npx vite` dev server route `/src/postcardCanvas.ts` (dev server on 5173: `npx vite --port 5173 &`), and run in the page:

```js
const m = await import('/src/postcardCanvas.ts');
const c = document.createElement('canvas');
const a = await m.loadPostcardAssets('https://shared.akamai.steamstatic.com/store_item_assets/steam/apps/413150/header.jpg');
const b = await m.renderPostcardBlob(c, { title: 'Stardew Valley', artworkUrl: 'x', note: 'for you ♡', acquiredAt: '2014-03-02T17:00:00Z', unwrappedAt: '2026-10-05T11:00:00Z' }, a);
[a.art !== null, b && b.size, b && b.type]
```

Expected: `[true, <non-zero>, "image/png"]` — art loaded cross-origin and the canvas was NOT tainted (a taint would have forced the art-less retry; assert `a.art !== null` AND inspect the screenshot). Screenshot the canvas (append it to the body, `browser_take_screenshot`) and look at it: art contained, title, from-line, note in quotes, postmark line, `bendobundles` mark, nothing overlapping.
- [ ] **Step 2:** Kill the servers. Record the screenshot path + the tuple in the PR description.
