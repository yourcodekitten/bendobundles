# Game detail: media carousel — trailer + screenshots — design

**Date:** 2026-07-08 · **Issue:** #61 · **Author:** code kitten ·
**Approval:** ben pre-approved the full pipeline on Discord (2026-07-08); design decisions
below flagged to him between steps.

## Problem

The game detail modal's media header (`web/src/GameDetailModal.tsx`, the "Video or artwork
header" block) shows exactly one thing: the trailer if `video_hls_url` is present, else
`header_image`, else the title-hash block. Ben wants the unwrap to feel like a real look at the
game — flip through the trailer *and* a handful of screenshots.

Nothing can be built client-side today because the enrichment doesn't carry screenshots:
`SteamAppDetail` (`crates/steam-client/src/lib.rs:31` and its TS mirror `web/src/api.ts:452`)
has only `header_image` / `video_hls_url` / `video_thumbnail`. Steam's `appdetails` payload
already includes a `screenshots` array (`path_thumbnail` / `path_full`) — `get_app_details`
(lib.rs:419) drops it on the floor, in the same parse that keeps `movies`. Plumbing job, not a
scraping job.

## Decision

Four components, back to front:

1. **steam-client** parses `screenshots[]` into a new `SteamAppDetail.screenshots` field —
   `Vec<Screenshot>` where `Screenshot { thumbnail: String, full: String }`, capped at the
   first **10** (ben asked for "a handful"; bounds the blob; self-heals via the 30-day refresh
   if the cap ever changes).
2. **Persistence + endpoints need no schema work** — verified: the blob is the full JSON of
   `SteamAppCache` (dynamo lib.rs:92, `body is the full JSON of this struct`), and both detail
   endpoints serialize `cache.detail` directly (`public-api/src/lib.rs:765`,
   `admin-api/src/lib.rs:383`). `#[serde(default)]` on the new field makes every pre-existing
   blob deserialize to `screenshots: []`, and the wire always carries the key after that.
3. **web** — the media header becomes a hand-rolled index-state carousel: trailer slide first
   (▶ / HLS behavior byte-identical), screenshots after. Arrows + lowercase `n / m` counter,
   no dots. Keyboard, reduced-motion, and a minimal dialog focus trap (the acceptance asks for
   it and the dialog currently has none — focus-on-open + Escape only).
4. **backfill** — the existing run-once rebuild already does the whole job: it refetches
   appdetails for EVERY catalog appid *through the current parse* and rewrites the item
   preserving the reviews half (`fulfillment/src/lib.rs:2094`). Once the parser knows
   screenshots, rerunning it backfills them. Rename it to match its real (generic) purpose.

Ben's issue comment remembered the mechanism as "a batch_get_steam_apps in the dynamo crate" —
close but that's the *read* path the games-list uses (`public-api/src/lib.rs:502`); the label
backfill was `backfill_steam_genres` + the `backfill_genres` bin (`list_all_games` →
`get_app_details` → `put_steam_app`). Same approach, correct name.

## Component 1: parse (`steam-client`)

- **Domain type:**
  ```rust
  #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
  pub struct Screenshot {
      pub thumbnail: String,
      pub full: String,
  }
  ```
  and on `SteamAppDetail`:
  ```rust
  /// First N store screenshots (thumbnail + full tiers). Empty when the app has none
  /// or the blob predates the field.
  #[serde(default)]
  pub screenshots: Vec<Screenshot>,
  ```
- **Why both tiers:** rerunning the backfill is an IAM ceremony (see Component 4 / Ops), so
  the stored shape should be the no-regrets one. `full` feeds the slides now; `thumbnail` is
  there the day the carousel grows a filmstrip, with no re-backfill. The issue floated both
  options; this is the pair variant.
- **Wire:** `AppDetailDataWire` gains `#[serde(default)] screenshots: Vec<ScreenshotWire>`
  with `ScreenshotWire { path_thumbnail: Option<String>, path_full: Option<String> }` — both
  lenient like the rest of the wire layer. An entry missing either tier is dropped (both URLs
  or nothing; asymmetric fallbacks create two sources of truth). Missing/empty array → `[]`,
  never a parse failure (the #57 lesson: one malformed field must not permanently un-enrich an
  app).
- **Cap:** `.take(10)` at parse time, matching the movies pattern of parse-time selection
  (first movie only).

## Component 2: persistence + endpoints (verification, not work)

- `SteamAppCache` stores `detail: Option<SteamAppDetail>` as full-struct JSON → new field
  rides along on the next `put_steam_app`, old blobs deserialize via `#[serde(default)]`.
- Friend endpoint (`GET /api/l/:token/games/:id/detail`) and admin endpoint
  (`GET /admin/api/games/:id/detail`) both emit `"detail": cache.detail` — the field appears
  on both wires automatically, exactly the path genres travel.
- Old lambda code reading a new blob (deploy-order window): serde ignores unknown fields by
  default (no `deny_unknown_fields` anywhere on these structs) — safe in both directions.
- **TS mirror** (`web/src/api.ts`): `screenshots: Screenshot[]` (non-optional — the rust side
  always serializes it), `Screenshot = { thumbnail: string; full: string }`.

## Component 3: the carousel (`web/src/GameDetailModal.tsx`)

**Media list.** Inside the loaded-with-steam branch:
`items = [trailer (if hlsUrl && !hlsFailed)] + screenshots` — trailer is always slide 1 when
present. `items.length === 0` → today's artwork/title-hash fallback, untouched.
`items.length === 1` → that item alone, **zero carousel chrome** (no arrows, no counter).
Chrome appears only at ≥ 2 items.

**Mechanics.** Hand-rolled, no new dependency (the repo's only heavy media dep is hls.js,
dynamically imported; a carousel lib for one header is not happening). `mediaIndex` state; a
flex strip translated `-index * 100%`, `transition-transform` with Tailwind's
`motion-reduce:transition-none` so `prefers-reduced-motion` gets an instant swap — same DOM,
no JS branch. Slides that are screenshots render `<img src={full} loading="lazy">` with the
game title + `screenshot n` as lowercase alt. Off-screen slides get `aria-hidden` + `inert`
(focus can't wander into them).

**Trailer slide.** The existing `<video>` + ▶ overlay moves into slide 1 unchanged — same
refs, same `handlePlay`, same `hlsFailed` fallback (when HLS fails the trailer slide is
dropped from the list and screenshots remain). Navigating away from slide 1 pauses the video
(`videoRef.current?.pause()`); navigating back does not auto-resume.

**Chrome (DESIGN.md, the attic arcade).**
- prev/next: real `<button>`s, `‹` / `›`, **Control** bg + inherited ink (hover
  Control Bright) — *neutral*; burgundy stays the claim button's. Lowercase
  `aria-label="previous"` / `"next"`. Wrap-around navigation.
- counter: lowercase `n / m` in the Shelf-chip style, bottom-right of the frame. It is text,
  not a control.
- active frame: the carousel viewport wears the **pixel bezel** (`ring-1 ring-pixel`) — the
  bezel motif marks the active media frame. **No drop shadows** on slides or chrome (the
  Ceremony Rule: the dialog is the only shadow in the room). No hearts (One Heart Rule).
- the room stays one green: screenshots bring their own color; every piece of chrome is tonal
  olive.

**Keyboard + a11y.**
- Carousel container: `role="region"`, `aria-roledescription="carousel"`,
  lowercase `aria-label="media"`; ArrowLeft/ArrowRight handled on the container (buttons are
  tabbable as themselves). Arrow keydowns whose target is the `<video>` are ignored — native
  video controls own their arrows.
- Counter is `aria-live="polite"` so slide changes announce.
- **Minimal focus trap** on the dialog (currently missing, acceptance requires it): a Tab /
  Shift+Tab keydown handler on the container that wraps focus across the dialog's focusable
  elements. Escape behavior unchanged.

**Thin fallback (delight never gates).** No trailer + no screenshots → exactly today's
header-image / title-hash rendering. One item → it renders alone, no chrome. `steam: null`
branch untouched. A pre-backfill blob (screenshots `[]`) behaves as "no screenshots" — the
carousel is a layer on a thing that already works.

## Component 4: backfill (`fulfillment`)

- **Rename to what it now is:** `backfill_steam_genres` → `backfill_steam_details`, bin
  `backfill_genres` → `backfill_details` (feature flag `backfill` unchanged). The function
  body already does the right thing — full appdetails refetch through the current parse,
  reviews half preserved, resumable, 429-aborts, exit-1 on failures. Doc comments update to
  say "generic detail rebuild (issues #57, #61)".
- **No logic change** beyond the rename. This is ben's "same approach as the labels" answer.

## Testing

Rust (`cargo test --workspace`, moto on `:8000`):
- steam-client `client_test.rs`: appdetails fixture with `screenshots[]` → parsed pairs in
  order, capped at 10; fixture without the key → `[]`; entry missing `path_full` → dropped.
- dynamo `store_test.rs`: a stored pre-#61 blob JSON (no `screenshots` key) round-trips —
  deserializes with `screenshots: []` (the serde(default) compat pin).
- fulfillment `handler_test.rs`: existing backfill tests follow the rename; one assert that a
  rewritten item carries screenshots from the (mock) storefront.

Web (`vitest` + `npm run build`):
- trailer + screenshots → trailer is slide 1, arrows navigate (wrap), counter updates.
- one screenshot, no trailer → image alone, no arrows/counter.
- no media → header-image fallback (regression pin on today's behavior).
- navigating away from a playing trailer pauses it.
- reduced-motion: strip carries `motion-reduce:transition-none` (class pin).
- focus trap: Tab on last focusable wraps to first.

Gates before push (checkpoint facts): `cargo fmt --check`, `clippy --workspace --all-targets
--all-features -- -D warnings`, `cargo test --workspace`, `npm run build`, vitest.

## Rollout & ops

1. Merge → deploy lambdas (public-api, admin-api, fulfillment) + web per
   `terraform/README.md` "Deploying as kitten". Deploy order doesn't matter for safety
   (unknown-field tolerance both ways) but lambdas go first so the UI never waits on data the
   API can't serve.
2. Run the renamed backfill bin locally against prod
   (`TABLE_NAME=brd-prod-ue1-bendobundles-table`, `AWS_PROFILE` with write): **the write grant
   is the catch** — kitten-deploy has no data plane; the #57 run used ben's temp
   console-inlined `dynamodb:PutItem` on kitten-debug, which per the journal is live until his
   next terraform-iam apply. Verify with `aws iam get-role-policy` before launching; if it's
   been swept, ask ben to re-inline (option c) or apply the closed #59 diff.
   **Skip-fresh window must be shorter than the time since the last pre-deploy fetch** — apps
   the daily 09:00Z sync touched with the old parser have fresh `fetched_at` but no
   screenshots; a 12h window would skip them. Run with a ~1h window (or whatever the bin
   exposes; confirm at plan time), and don't overlap the 09:00Z sync.
3. Verify live: friend detail endpoint returns `screenshots[]` for a known-screenshotted app;
   modal shows the carousel; a no-media game still renders the plain header.

## Out of scope

- Filmstrip/thumbnail nav, dots, swipe gestures, pinch zoom, fullscreen lightbox.
- Screenshots anywhere but the detail modal (cards stay carts).
- Any reviews-half or enrichment-budget changes.

## Acceptance (issue #61 → this design)

- [x] `SteamAppDetail` (rust + ts) carries screenshots → Component 1 + TS mirror
- [x] persist + reach friend & admin endpoints → Component 2 (verified free)
- [x] carousel, trailer first → Component 3
- [x] keyboard + reduced-motion + focus trap + on-brand chrome → Component 3
- [x] thin fallback holds → Component 3 + web tests
- [x] backfill for existing catalog → Component 4 + Ops (ben's issue comment)
