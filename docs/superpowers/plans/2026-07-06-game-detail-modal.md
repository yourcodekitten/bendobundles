# Game Detail Modal + Steam Enrichment Cache Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Clicking a game on the friend gift-link page or the admin catalog opens a store-page-style modal — trailer (HLS), overall + recent review badges, developer/publisher/release/tags, description — served entirely from a sync-time DynamoDB cache (Steam is never touched at request time).

**Architecture:** Three storefront reads per appid (appdetails + appreviews-overall + appreviewhistogram — recipes empirically pinned 2026-07-06, fixtures from real captures) land in `STEAMAPP#{app_id}` items during a budgeted, politely-paced `run_sync` pass. Two cache-read detail endpoints (friend: token-scoped, no-oracle; admin: superset shape). One `GameDetailModal` component with two mounts; trailers are HLS-only (hls.js + native Safari), verified CORS-open.

**Tech Stack:** Rust, steam-client crate (from the steam-integration plan — REQUIRED PREDECESSOR), dynamodb-local + wiremock tests, React 18 + TS + hls.js.

**Specs:** `docs/superpowers/specs/2026-07-06-game-detail-modal-design.md` (its §2 mapper is ALREADY BUILT by the steam-integration plan — this plan starts at §3). Real captured responses for fixtures: `docs/superpowers/specs/captures/2026-07-06-steam/*.json`. Plan template/conventions: `docs/superpowers/plans/2026-07-06-self-claim.md` Global Constraints (signing, per-crate tests, no-`_`-arm).

## Global Constraints

- Branch `kitten/game-detail-modal`, after steam-integration merges. Signed commits.
- **Be-nice rule (Ben, verbatim constraint):** Steam storefront endpoints are hit ONLY inside `run_sync`'s enrichment pass — never at request time. Pacing ≥1.5s between storefront calls; per-sync budget 75 appids AND stop when <180s of Lambda budget remains (timeout is 900s — `persist_sync` + `end_sync_run` must always land); any 429 aborts the pass for this run.
- Staleness windows: appdetails **30d**, reviews+histogram **14d**. Negative cache: `success:false` (delisted) writes a stub retried on the 30d window.
- Kill switch: env `STEAM_ENRICH_DISABLED=1` skips the pass entirely.
- One log line per sync: `steam enrichment: fetched=<n> fresh=<n> negative=<n> aborted_429=<bool>`.
- No secrets in detail responses; `gamekey`-in-id exposure is documented-accepted (spec §4) — update the stale CatalogGameView comment when touched.

---

### Task 1: steam-client — the three storefront reads + `SteamAppDetail`

**Files:** Modify `crates/steam-client/src/lib.rs`; test `crates/steam-client/tests/client_test.rs`; fixtures copied from `docs/superpowers/specs/captures/2026-07-06-steam/` into `crates/steam-client/tests/fixtures/`.

**Interfaces (Tasks 2-4 consume):**

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SteamAppDetail {
    pub app_id: u32,
    pub name: String,
    pub developers: Vec<String>,
    pub publishers: Vec<String>,
    pub genres: Vec<String>,          // genres + categories descriptions, deduped
    pub release_date: Option<String>, // "Feb 26, 2016"
    pub short_description: String,
    pub header_image: Option<String>,
    pub video_hls_url: Option<String>,   // movies[0].hls_h264 — movies are HLS/DASH-ONLY now
    pub video_thumbnail: Option<String>,
}
pub enum AppDetails { Found(SteamAppDetail), Delisted }   // appdetails success:false ⇒ Delisted
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewSummary { pub desc: String, pub total_positive: u64, pub total_negative: u64, pub total_reviews: u64 }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecentReviews { pub percent_positive: u8, pub count: u64 }  // computed from histogram buckets

pub async fn get_app_details(&self, app_id: u32) -> Result<AppDetails, SteamError>;
// GET {store}/api/appdetails?appids=<id>&cc=us&l=english
pub async fn get_review_summary(&self, app_id: u32) -> Result<ReviewSummary, SteamError>;
// GET {store}/appreviews/<id>?json=1&num_per_page=0&language=english&purchase_type=all → query_summary (ALWAYS overall — pinned empirically; no param yields recent)
pub async fn get_recent_reviews(&self, app_id: u32) -> Result<RecentReviews, SteamError>;
// GET {store}/appreviewhistogram/<id>?l=english → sum results.recent[].recommendations_up/down
```

- [ ] **Step 1: Failing tests** — fixtures ARE the real captures: mount `appdetails-413150-trimmed.json` → assert `Found` with `developers==["ConcernedApe"]`, `release_date==Some("Feb 26, 2016")`, `video_hls_url` ending `hls_264_master.m3u8?t=1754692862`, genres containing `"RPG"`; mount `{"413150":{"success":false}}` → `Delisted`; mount `appreviews-overall-413150.json` → `desc=="Overwhelmingly Positive"`, `total_reviews==460881`; mount `appreviewhistogram-413150.json` → `percent_positive==98`, `count==9200` (the verified numbers). Plus 429 → `RateLimited` on each.
- [ ] **Step 2: Verify failure.** **Step 3: Implement** (appdetails response is keyed by the appid as a STRING — `{"413150":{"success":bool,"data":{…}}}`; movies: take `movies.get(0)` and its `hls_h264` + `thumbnail`; genres = `genres[].description` ∪ `categories[].description` deduped, order-preserving; histogram: `percent = round(100*up/(up+down))`, guard the zero-division → `RecentReviews{percent_positive:0,count:0}`). **Step 4: Verify green.**
- [ ] **Step 5: Commit** — `git commit -S -m "feat(steam-client): storefront reads — appdetails (HLS-only movies), overall reviews, histogram-derived recent"`

---

### Task 2: dynamo — STEAMAPP cache items

**Files:** Modify `crates/dynamo/src/lib.rs` (+ schema.rs); test `store_test.rs`.

**Interfaces (Tasks 3-4 consume):**

```rust
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SteamAppCache {
    pub app_id: u32,
    pub detail: Option<steam_client::SteamAppDetail>,   // None ⇔ negative-cache stub (delisted)
    pub overall: Option<steam_client::ReviewSummary>,
    pub recent: Option<steam_client::RecentReviews>,
    pub fetched_at: i64,          // appdetails clock (30d window)
    pub reviews_fetched_at: i64,  // reviews+histogram clock (14d window)
}
pub async fn put_steam_app(&self, c: &SteamAppCache) -> Result<(), StoreError>;   // pk=STEAMAPP#<id> sk=META body=json
pub async fn get_steam_app(&self, app_id: u32) -> Result<Option<SteamAppCache>, StoreError>;
pub async fn list_steam_app_ids(&self) -> Result<Vec<u32>, StoreError>;           // for staleness scan (paged Scan on pk begins_with STEAMAPP# — or store fetched_at top-level and scan; at ~700 items a Scan is fine, same rationale as list_all_games)
```

- [ ] **Steps 1-4:** round-trip test (incl. a stub with `detail: None`) → fail → implement following the SYNC#STATE item shape → green.
- [ ] **Step 5: Commit** — `git commit -S -m "feat(dynamo): STEAMAPP enrichment cache items"`

---

### Task 3: fulfillment — the budgeted enrichment pass

**Files:** Modify `crates/fulfillment/src/lib.rs` (new `enrich_steam_apps(deps, deadline)` called from `run_sync` after the mapper pass; `Deps` already carries `steam` from the steam-integration plan); test `handler_test.rs`.

**Behavior (spec §3, all pinned):**
1. Skip silently when `STEAM_ENRICH_DISABLED=1` (read via Deps config, not raw env, for testability) or `deps.steam` is None.
2. Work list: every distinct `steam_app_id` across games, where the STEAMAPP item is missing OR `fetched_at` older than 30d OR `reviews_fetched_at` older than 14d. Cap at **75 appids** per pass (log how many were deferred — no silent truncation).
3. Per appid, in order, `≥1.5s` tokio::sleep between EVERY storefront call: appdetails when its clock is stale (skip when only reviews are stale — don't refetch fresh halves); appreviews + histogram when the reviews clock is stale. Write the merged `SteamAppCache` per-item as each app completes (partial progress persists).
4. `Delisted` → stub write (`detail: None`, clocks stamped) — retried on the 30d window, never every sync.
5. Any `RateLimited` → log + **abort the pass** (remaining work deferred to next sync). Any other per-app error → log, skip the app, continue.
6. Deadline guard: stop starting new apps when less than 180s of lambda budget remains (thread a `deadline: Instant` computed by the caller from the lambda context's remaining-time; in tests, inject).
7. The one summary log line (Global Constraints).

- [ ] **Step 1: Failing tests** — (a) fresh items → zero storefront calls (wiremock expect(0)); (b) stale-reviews-only → exactly 2 calls (no appdetails refetch); (c) 429 on the 3rd app aborts (apps 1-2 persisted, 4+ untouched); (d) delisted stub written and NOT refetched on a fresh-window rerun; (e) budget: 80 mapped games → 75 processed, deferral logged (assert via the persisted items count). Drive with a tiny fake clock/deadline; mount the capture fixtures.
- [ ] **Step 2/3/4:** fail → implement → whole crate green.
- [ ] **Step 5: Commit** — `git commit -S -m "feat(fulfillment): steam enrichment pass — 75-app/deadline budget, 1.5s pacing, 429-abort, negative cache, kill switch"`

---

### Task 4: detail endpoints — friend (token-scoped) + admin (superset)

**Files:** Modify `crates/public-api/src/lib.rs`, `crates/admin-api/src/lib.rs`; tests in both `api_test.rs`.

**Interfaces (Task 5 consumes — the two shapes, spec §4 verbatim):**

```jsonc
// GET /api/l/:token/games/:id/detail
{ "game": { "id","title","bundle","key_type","artwork_url","steam_app_id" },
  "steam": { "detail": {…SteamAppDetail…} | null, "overall": {…}|null, "recent": {…}|null } | null }
// steam: null ⇔ unmapped or no cache item yet. SPA branches on it.

// GET /admin/api/games/:id/detail  — superset:
{ "game": { …CatalogGameView (incl. status, giftable, hidden, requires_choice, owned_by_ben) },
  "steam": … same shape … }
```

**Friend access rule (no-oracle, spec §4):** resolve the link (unknown → byte-identical 404); serve the game only if it is currently listable OR its id appears in THIS link's claims history; anything else → the same 404. Admin: session-guarded, any game id.

- [ ] **Step 1: Failing tests** — friend: listable game 200 with steam blob; hidden game → 404 byte-identical to unknown-id 404; claimed-by-this-link game 200; other-link's claimed game 404; unmapped game → `"steam": null`. Admin: superset fields present; 401 without session.
- [ ] **Step 2/3/4:** fail → implement (both handlers: `get_game` + `get_steam_app(game.steam_app_id?)` → assemble; reuse the CatalogGameView serializer on the admin side) → green.
- [ ] **Step 5: Commit** — `git commit -S -m "feat(api): game detail endpoints — friend token-scoped no-oracle, admin superset, cache-only reads"`

---

### Task 5: web — `GameDetailModal`, two mounts, HLS

**Files:**
- Create: `web/src/GameDetailModal.tsx` + `web/src/GameDetailModal.test.tsx` (shared component — it lives outside admin/ and friend/ because both mount it)
- Modify: `web/src/api.ts` (types + `fetchGameDetail(token, gameId)` + `adminGameDetail(gameId)`), `web/src/friend/GameGrid.tsx` (card click → modal; claim button in modal footer wiring into the existing ClaimDialog flow), `web/src/admin/Catalog.tsx` (row click → modal; footer shows status badge + routes self-claim through the SHARED arm/confirm from the self-claim feature — never a modal-local confirm)
- Deps: `cd web && npm install hls.js` (+ `@types` if needed)

**Component behavior (spec §5):**
- Lazy fetch on open; per-session component-state cache keyed by game id.
- Trailer: `<video>` with poster=`video_thumbnail`; on play-click attach hls.js when `!video.canPlayType('application/vnd.apple.mpegurl')` (native Safari path otherwise); on any HLS error → fall back to poster/artwork (never a broken player). Click-to-play only, no autoplay.
- Badges: overall = `desc + total_reviews` (tooltip: positive/negative counts); recent = `percent_positive% positive (count recent)`.
- Body: dev/pub/release line, genre chips, short description.
- Thin fallback when `steam: null`: artwork, bundle, key_type, "no steam page for this one."
- Escape/backdrop close; friend footer = claim button honoring the grid's disabled rules; admin footer = status badge (+ self-claim arm/confirm integration).

- [ ] **Step 1: Failing tests** — renders full variant from a mocked detail response (fixture mirrors the API shape with the capture data); thin variant on `steam: null`; claim button fires the existing claim flow; admin mount shows status; hls fallback path (mock hls.js). Follow the existing component-test style (Testing Library patterns in GameGrid.test.tsx / Catalog.test.tsx).
- [ ] **Step 2/3/4:** fail → implement → `npx vitest run && npx tsc --noEmit` green.
- [ ] **Step 5: Commit** — `git commit -S -m "feat(web): GameDetailModal — HLS trailer, review badges, two mounts, thin fallback"`

---

### Task 6: ship — CI, PR, DEPLOY, live check (fork-to-deployed)

- [ ] **Step 1:** signed-commit audit; workspace + web suites green locally (per-crate; CI is the full builder).
- [ ] **Step 2:** push branch, PR, CI green, merge per HR#1.
- [ ] **Step 3: DEPLOY** per the proven procedure: lambda-zips → stage **fulfillment + public-api + admin-api** → deploy.tfvars → targeted plan (expect lambda code changes only, 0 destroy) → apply → verify CodeShas → shred; SPA build + s3 sync + CF invalidation (hls.js changes the bundle — the SPA deploy is NOT optional).
- [ ] **Step 4: LIVE CHECK** (spec §7): trigger sync; read the enrichment log line (kitten-debug CloudWatch) — first run fetches ≤75 apps, politely; spot-read 2-3 STEAMAPP items; open the modal on both surfaces — trailer PLAYS from bendobundles.com (the CORS verification was pre-done 2026-07-06 but the live check confirms end-to-end), badges match the store; a non-steam game shows the thin fallback. Repeat sync → `fetched=0`-ish (fresh windows hold).
- [ ] **Step 5: report ONCE** to Ben with receipts. The 2019–2021 catalog fully enriches over ~10 daily syncs — say so in the report (no silent partial coverage).

---

## Self-Review Notes (applied)

- Spec §2 (mapper) intentionally absent — built by the steam-integration plan (Ben's ordering); §1 cache-aside rejection documented in spec, no task. §3 → Tasks 1/2/3 (every knob: windows, budget, pacing, 429, negative, kill switch, observability). §4 → Task 4 incl. both JSON shapes + no-oracle rule + the id-exposure note (update the CatalogGameView comment in Task 4 when touching admin-api). §5 → Task 5 (incl. the shared-confirm sequencing note + GameGrid dup-title note honored by keying the modal on the clicked card's id). §6/§7 → tests distributed per task + Task 6 live check.
- Type consistency: `SteamAppDetail`/`ReviewSummary`/`RecentReviews` defined once in steam-client (T1), serialized into `SteamAppCache` (T2), served verbatim inside the `steam` blob (T4), typed in api.ts (T5). Field `video_hls_url` end-to-end.
- Fixtures: REAL captures from `docs/superpowers/specs/captures/2026-07-06-steam/` — never hand-guessed shapes (the lesson that built this spec).
