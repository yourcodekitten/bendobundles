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
   blob deserialize to `screenshots: []`; the *stored* blob may lack the key until rewritten,
   but the endpoint re-serializes the deserialized struct, so the *wire* always carries it
   (from the new lambda code onward).
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
- **Cap semantics:** a cap change reaches *existing* blobs only via a backfill rerun or the
  30-day enrichment refresh — up-to-30-day lag on a cap change is accepted.
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
- **The wire shape is an external-API claim with no in-repo fixture** — nothing in
  `crates/steam-client/tests/fixtures/` carries a `screenshots` key today. The parse fixture
  MUST be a real captured `appdetails` response for a screenshotted app (Stardew Valley,
  413150 — the existing trimmed fixture's app), not a hand-fabricated shape. If the real
  field names differ, a fabricated fixture would ship the feature empty with green tests.
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
- **TS mirror** (`web/src/api.ts`): `screenshots?: Screenshot[]` (**optional**, with
  `detail.screenshots ?? []` at the single read site), `Screenshot = { thumbnail: string;
  full: string }`. Optional because a new web bundle can race an old lambda during deploy —
  a response without the key must not throw. This makes deploy order genuinely
  order-independent; lambdas-first (Rollout step 1) remains the preference, not a safety
  requirement.

## Component 3: the carousel (`web/src/GameDetailModal.tsx`)

**Media list.** Inside the loaded-with-steam branch:
`items = [trailer (if hlsUrl && !hlsFailed)] + screenshots` — trailer is always slide 1 when
present. `items.length === 0` → today's artwork/title-hash fallback, untouched.
`items.length === 1` → that item alone, **zero carousel chrome** (no arrows, no counter).
Chrome appears only at ≥ 2 items.

**Index invalidation.** `items` can shrink under `mediaIndex` — a fatal HLS error mid-session
sets `hlsFailed` and drops the trailer slide, shifting every screenshot down one. The render
clamps: `index = Math.min(mediaIndex, items.length - 1)` (effective index derived at render,
state left alone), so the counter and visible slide stay consistent whatever the user was
looking at when the list changed.

**Mechanics.** Hand-rolled, no new dependency (the repo's only heavy media dep is hls.js,
dynamically imported; a carousel lib for one header is not happening). `mediaIndex` state; a
flex strip translated `-index * 100%`. **Reduced motion follows the repo's established JS
`matchMedia` pattern** (`LinkPage.tsx:37`, `CursorCompanion.tsx:71` — this codebase does
reduced-motion via `matchMedia`/`@media`, never Tailwind's `motion-reduce:` variant): the
strip gets `transition-transform` only when `prefers-reduced-motion` is NOT set; otherwise no
transition class → instant swap. This is behavior-testable in vitest by mocking `matchMedia`
(the class-pin-only alternative would test the source string, not the behavior). Slides that
are screenshots render `<img src={full} loading="lazy">` with the game title +
`screenshot n` as lowercase alt. Off-screen slides get `aria-hidden` + `inert` (React 19
supports the prop; focus can't wander into them).

**Trailer slide.** The existing `<video>` + ▶ overlay moves into slide 1 unchanged — same
refs, same `handlePlay`, same `hlsFailed` fallback (when HLS fails the trailer slide is
dropped from the list and screenshots remain). Navigating away from slide 1 pauses the video
(`videoRef.current?.pause()`); navigating back does not auto-resume.

**Chrome (DESIGN.md, the attic arcade).**
- prev/next: real `<button>`s, `‹` / `›`, **Control** bg + inherited ink (hover
  Control Bright) — *neutral*; burgundy stays the claim button's. Lowercase
  `aria-label="previous"` / `"next"`. Wrap-around navigation (accepted oddity: on a 2-item
  carousel prev/next are functionally identical — fine).
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
  tabbable as themselves). Arrow keydowns whose target is (or is inside) the `<video>` are
  ignored — native video controls own their arrows.
- Counter is `aria-live="polite"` so slide changes announce.
- **Focus trap contract** (the dialog currently has none — focus-on-open + a document-level
  Escape listener only; acceptance requires a trap). One `onKeyDown` handler **on the dialog
  container** handles Tab/Shift+Tab (the carousel's Arrow handling stays on the carousel
  region; Escape stays the existing document listener — three keys, but Tab and Arrow each
  live at their owning scope):
  - Focusable set is computed **at keydown time**:
    `container.querySelectorAll('button, [href], input, select, textarea, video, [tabindex]:not([tabindex="-1"])')`
    filtered to drop any element inside an `[inert]` or `aria-hidden="true"` subtree and any
    `disabled` element — the set is dynamic per slide, and off-screen slides are inert, so a
    static query would trap focus into hidden slides.
  - Tab on the last focusable wraps to the first; Shift+Tab on the first wraps to the last.
  - Focus on the container itself (`tabIndex={-1}`, the initial focus holder): Tab moves to
    the **first** focusable, Shift+Tab to the **last**.
  - Empty focusable set (degenerate): Tab is swallowed (preventDefault), focus stays on the
    container.

**Thin fallback (delight never gates).** No trailer + no screenshots → exactly today's
header-image / title-hash rendering. One item → it renders alone, no chrome. `steam: null`
branch untouched. A pre-backfill blob (screenshots `[]`) behaves as "no screenshots" — the
carousel is a layer on a thing that already works.

## Component 4: backfill (`fulfillment`)

- **Rename to what it now is:** `backfill_steam_genres` → `backfill_steam_details`, bin
  `backfill_genres` → `backfill_details` (feature flag `backfill` unchanged). The function
  body already does the right thing — full appdetails refetch through the current parse,
  reviews half preserved, resumable, 429-aborts, exit-1 on failures. Doc comments update to
  say "generic detail rebuild (issues #57, #61)". The rename must also sweep every reference
  to the old bin name: the bin's own module doc / invocation example, Cargo.toml `[[bin]]`,
  and any runbook text mentioning `backfill_genres`.
- **Bin knobs (confirmed against the source, not deferred):** env `TABLE_NAME` (required),
  `SKIP_FRESH_SECS` (optional, default 43 200 = 12h, `0` disables skipping); pace hardcoded
  1.5 s/app; exit 2 on 429-abort, exit 1 on failures.
- **No logic change** beyond the rename. This is ben's "same approach as the labels" answer.

## Testing

Rust (`cargo test --workspace`, moto on `:8000`):
- steam-client `client_test.rs`: appdetails fixture with `screenshots[]` → parsed pairs in
  order, capped at 10; fixture without the key → `[]`; entry missing `path_full` → dropped.
  **The screenshots fixture is a real captured response** (413150), per Component 1.
- dynamo `store_test.rs`: a stored pre-#61 blob JSON (no `screenshots` key) round-trips —
  deserializes with `screenshots: []` (the serde(default) compat pin).
- fulfillment `handler_test.rs`: existing backfill tests follow the rename; one assert that a
  rewritten item carries screenshots from the (mock) storefront.

Web (`vitest` + `npm run build`):
- trailer + screenshots → trailer is slide 1, arrows navigate (wrap), counter updates.
- one screenshot, no trailer → image alone, no arrows/counter.
- no media → header-image fallback (regression pin on today's behavior).
- navigating away from a playing trailer pauses it.
- HLS fails fatally mid-carousel → trailer slide dropped, effective index clamps, counter
  and visible slide stay consistent.
- reduced-motion: `matchMedia` mocked both ways — transition class present when motion is
  allowed, absent under `prefers-reduced-motion` (behavior pin, not a class-string pin).
- off-screen slides carry `inert` + `aria-hidden` (focus containment pin).
- focus trap: Tab on last focusable wraps to first; Shift+Tab on first wraps to last; Tab
  from the freshly-opened container lands on the first focusable.

Gates before push (checkpoint facts): `cargo fmt --check`, `clippy --workspace --all-targets
--all-features -- -D warnings`, `cargo test --workspace`, `npm run build`, vitest.

## Rollout & ops

1. Merge → deploy lambdas (public-api, admin-api, fulfillment) + web per
   `terraform/README.md` "Deploying as kitten". Deploy order doesn't matter for safety
   (unknown-field tolerance both ways) but lambdas go first so the UI never waits on data the
   API can't serve.
2. Run the renamed backfill bin locally against prod
   (`TABLE_NAME=brd-prod-ue1-bendobundles-table`, `AWS_PROFILE` with write): **the write grant
   is the catch** — kitten-deploy has no app-table data plane; the #57 run used ben's temp
   console-inlined `dynamodb:PutItem` on kitten-debug's `diagnostics-read-only` policy.
   **Verified still live 2026-07-08 (`aws iam get-role-policy`)**; re-verify immediately
   before launching (it vanishes on ben's next terraform-iam apply) — if swept, ask ben to
   re-inline (option c) or apply the closed #59 diff.
   **First full run: `SKIP_FRESH_SECS=0` — skipping disabled.** The whole catalog is
   stale-parser data; every app must be rewritten regardless of `fetched_at`. In particular,
   apps the daily 09:00Z sync refreshed pre-deploy have *fresh* timestamps and *no*
   screenshots — the bin's default 12h window (and even a ~1h window) would wrongly skip
   them; the bin's own doc says 0 for exactly this case. A nonzero window is only for
   resuming a 429-aborted run. Don't overlap the 09:00Z sync. Expected runtime ~22 min
   (~853 apps × 1.5 s hardcoded pace); run detached (`setsid nohup`) with a log file, exit 0
   + `failed=0` is the success contract.
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
