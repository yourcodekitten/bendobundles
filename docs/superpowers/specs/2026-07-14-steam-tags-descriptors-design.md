# Steam user tags + content descriptors + 18+ auto-hide

**Date:** 2026-07-14 · **Approved:** Ben, via Discord (issue #71 "build it" order + spec-window
answers 14:28 EDT; descriptor correction + auto-hide {3,4} + width-budget display explicitly
aligned 14:33 EDT: "I am aligned on your recommendation") · **Author:** kitten

Issue: yourcodekitten/bendobundles#71.

## Goal

The chips on game cards are appdetails **genres** — publisher-assigned, four bland words
("Adventure, Casual, Indie, RPG"). The tags a human reads on a Steam store page are the
**popular user-defined tags** — community data from a different API. Replace genre chips with
user tags everywhere cards render (friend + admin), keep genres as the fallback, store the
safety-relevant **content descriptors** we currently discard, badge the sexual-content ones in
admin, and **auto-hide adult-only games during sync** so Ben never again has to catch a Puss!
by hand.

Receipts (live, 2026-07-14): Puss! (981300) genres said `Adventure, Casual, Indie, RPG` while
users tag it `Sexual Content, Nudity, Casual, Indie, Hentai, …` and its descriptors are
`[1,3,4,5]`. Dome Keeper's tags match its store page byte-for-byte.

## Decisions (Ben, spec window 2026-07-14)

1. **Tags replace genres on friend-facing cards too.** Genres are "not fine-grain enough";
   fall back to genres when tags are empty. Mature tag names (e.g. "Sexual Content") DO show
   if Ben chooses to unhide a game — no friend-side tag laundering.
2. **Display heuristic = width budget, not a count.** Verified: the store page renders ALL 20
   tags in exactly the GetItems popularity order and shows however many fit the box width
   (Rollerdrome shows 6 because `Action, Sports, Shooter…` are short). We mirror it
   deterministically: chips render in popularity order into a **character budget** (tunable
   constant, min 3 tags, max 6), so short tags ⇒ more chips, like Steam.
3. **Store top-10 tag names** per app (display cap changes client-side without a backfill).
4. **Content descriptor semantics — corrected from the issue** (verified live: Cyberpunk
   `[1,2,5]` + note "intense violence, blood and gore…nudity"; Puss! `[1,3,4,5]`, no 2):
   - 1 = some nudity/sexual content · **2 = frequent violence/gore** · 3 = adult-ONLY sexual
     content · **4 = gratuitous sexual content** · 5 = general mature (dev's generic checkbox —
     Rollerdrome, Amanda the Adventurer, Witcher 3 all carry it).
   - **Auto-hide set: {3, 4}** (the adult-sexual pair; Puss! carries both).
   - **🔞 badge set (admin): {1, 3, 4}** (sexual-content family).
   - **5: no badge** — half the catalog would wear 🔞 and it'd mean nothing. Note in the admin
     detail view only. Violence-only (2) stays invisible.

## API surface (verified live, keyless, no scrape)

- `GET api.steampowered.com/IStoreBrowseService/GetItems/v1/?input_json={"ids":[{"appid":N},…],
  "context":{"language":"english","country_code":"US"},"data_request":{"include_tag_count":20}}`
  → per-app `tagids` in popularity order + `content_descriptorids`. **Batchable** (ids is an
  array); chunks of 50, unpaced (plan-review decision: `get_app_list` already pulls 5
  unpaced 50k-row pages from this host daily — not the throttled appdetails host), no key.
- `GET api.steampowered.com/IStoreService/GetTagList/v1/?language=english` → tagid→name map
  (~448 tags, `version_hash`). Fetched **once per enrich/backfill run**, held in memory — one
  keyless call per run isn't worth persisting.
- appdetails already returns `content_descriptors: {ids, notes}` — we start deserializing it.
- Caveats: adult-gated/delisted/region-hidden apps can return `visible:false` with empty
  `tagids` → empty tags stored → genre fallback (existing degradation shape). `required_age`
  stays ignored (self-reported; Puss! says 0). If Valve ever gates GetItems, fallback is the
  store-page `g_rgAppTags` blob (documented in #71, do not re-research).

## Server changes

### 1. `steam-client`

- Deserialize `content_descriptors: {ids: Vec<u32>, notes: Option<String>}` from appdetails
  (`#[serde(default)]` wire-side).
- New: `get_store_items(app_ids: &[u32]) -> Result<HashMap<u32, StoreItemTags>>` — GetItems in
  chunks of 50, `StoreItemTags { tagids: Vec<u32>, content_descriptorids: Vec<u32> }`. Missing
  /invisible apps absent from the map. 429 → `SteamError::RateLimited` (same mapping idiom).
- New: `get_tag_list() -> Result<HashMap<u32, String>>` — GetTagList, tagid→name.
- `SteamAppDetail` grows, all `#[serde(default)]` so existing cache blobs keep deserializing:
  - `tags: Vec<String>` — top-10 names, popularity order, ids resolved via GetTagList at
    enrichment time (names stored so display needs no lookup; 30-day refresh absorbs drift).
    Unknown tagid (not in GetTagList) → skipped.
  - `content_descriptor_ids: Vec<u32>` — raw ids, unfiltered (semantics live client/view-side).
  - `content_notes: Option<String>` — appdetails' descriptor note, verbatim (Steam's grammar
    and all).

### 2. `fulfillment` (enrichment + backfill)

- `enrich_steam_apps`: per pass, batch-fetch GetItems for the apps needing detail refresh
  (work list is ≤75/run) + GetTagList once; merge tags/descriptors into each `SteamAppCache`
  write. Tags ride the detail half's 30-day TTL — no new clock. Any tag-batch failure (429
  included) preserves existing tags and lets the pass continue (plan-review decision: a
  keyless tag endpoint hiccup must not starve appdetails/reviews refreshes); a missing app
  in a SUCCESSFUL response stores empty tags.
- **Auto-hide during sync:** after an app's descriptors are known, if
  `descriptor_ids ∩ {3,4} ≠ ∅`, every game mapped to that appid with `hidden == false` and
  `hidden_source != Admin` gets a guarded auto-hide write (§3). One-way: sync **hides only**,
  never unhides — descriptors changing later never un-hides anything.
- `backfill_steam_details` (the "resync the database" bin): also fetches GetItems +
  GetTagList and applies the same auto-hide, so one post-deploy run populates
  tags/descriptors across the whole catalog and sweeps existing adult games. Run once after
  deploy (NOT during the 09:00Z cron window, per the bin's existing caveat).

### 3. Hidden provenance — the "never fights Ben" mechanism

Precedent: `appid_source` Manual-override (`domain::AppidSource`, top-level DDB mirror,
conditional writes). Same pattern:

- `domain::Game` grows `hidden_source: Option<HiddenSource>` (`#[serde(default)]`),
  `enum HiddenSource { Admin, Sync }` (snake_case on the wire — remember the serde-case
  lesson from plan-2).
- `schema::game_item` mirrors it as a **top-level DDB attribute** when `Some` (condition
  expressions can't see blob-embedded fields — the void-conditional-write lesson).
- Admin toggle (`set_game_hidden`) sets `hidden_source = Admin` on **both** hide and unhide.
  Once Ben touches the toggle, sync never overrides him, forever.
- New `Store::auto_hide_game(game_id, expected_status)`: keeps the status optimistic-lock
  from `set_game_hidden` (mid-claim → contested → skip, next sync retries) **plus**
  `(attribute_not_exists(#hsrc) OR #hsrc <> :admin)`, sets `hidden = true,
  hidden_source = Sync`. Legacy rows (no `hidden_source`) are eligible — correct, because
  every existing unhidden game is untouched-by-Ben by definition; his first toggle stamps
  `Admin` and immunizes it.
- `merge_sync` carries `hidden_source` alongside `hidden` (which it already preserves).
- Not silent: auto-hidden games are visible in admin (§ web) as
  `hidden && hidden_source == Sync`.

### 4. API views

- `public-api` `GameView`: keep `genres` (deploy-window back-compat for cached SPA bundles),
  add `tags: Vec<String>` (top-10, `skip_serializing_if empty`). Client prefers tags, falls
  back to genres. The friend LIST payload carries no descriptor data — the badge is
  admin-only. (The friend DETAIL endpoint serializes the whole `SteamAppDetail` blob, which
  now includes descriptor ids/notes on the wire; accepted — it's public Steam metadata and
  the friend UI never renders it. Do not add a strip layer.)
- `admin-api` `SteamSummaryView`: add `tags: Vec<String>`,
  `content_descriptor_ids: Vec<u32>`. `CatalogGameView` additionally exposes
  `hidden_source: Option<String>` so the catalog can label auto-hides.
- Detail endpoints: `SteamAppDetail` is already on the wire — `tags`,
  `content_descriptor_ids`, `content_notes` flow through for free. TS mirrors updated.

## Web changes

- Shared helper `displayTags(game)` → tags if non-empty else genres; and
  `fitTags(tags, budget)` → popularity-order prefix within the character budget (min 3,
  max 6). Unit-tested; both surfaces use it so friend and admin agree.
- **Friend `GameGrid.tsx`:** chip row renders `fitTags(displayTags(game))` instead of
  `genres.slice(0,4)`. Chip visuals unchanged (title-hash hue).
- **Admin `Catalog.tsx`:** 🔞 badge in the badge cluster when
  `steam.content_descriptor_ids ∩ {1,3,4} ≠ ∅`; row note `auto-hidden: adult content` (dust
  tier, near the hidden toggle) when `hidden && hidden_source == 'sync'`.
- **`catalogToolkit.ts` / `ToolkitBar.tsx`:** tag filter options now derive from
  `displayTags` (same chips you see = same chips you filter). New `mature` toolkit key:
  `all / hide 🔞 / only 🔞` (predicate = badge-set intersection), URL param like the rest of
  the #67 state.
- **`GameDetailModal.tsx`:** chips block uses `displayTags`; admin mount additionally shows
  the descriptor note (`content_notes`) + a small descriptor-id → label legend line when
  descriptors exist.

## Error handling

- GetItems batch failure mid-pass: abort tags for the pass (detail writes already done keep
  their old tags — fields merge, not clear); next sync retries. Never wipe existing tags
  because one endpoint hiccuped: empty-tags-from-API only overwrites when the GetItems call
  for that app *succeeded*.
- Auto-hide contested (mid-claim): skip, log, next sync retries.
- GetTagList failure: skip tag-name resolution for the pass (store no tags rather than raw
  ids-as-strings); descriptors (from appdetails) still land.

## Testing

- `steam-client` (wiremock): GetItems parse (tagids order, descriptorids, missing app,
  visible:false/empty), GetTagList parse, appdetails content_descriptors parse (present,
  absent, notes null), 429 mapping.
- `domain`: merge_sync carries hidden_source; HiddenSource serde round-trip (snake_case).
- `dynamo` (real-race lesson from plan-2 — the DB-level guard must actually fire):
  auto_hide sets hidden+source; auto_hide vs pre-seeded `hidden_source=admin` row is a
  **conditional-check failure at the DDB layer**, not just the in-memory path; admin unhide
  then auto_hide attempt → no-op; mid-claim (status flipped by second handle) → contested;
  top-level `hidden_source` attribute present and snake_cased.
- `fulfillment`: enrich merges tags/descriptors into cache; auto-hide fires for {3,4},
  not for {1,5} or {2}; one-way (descriptors clearing doesn't unhide); backfill does all of
  the above ignoring TTL.
- `admin-api` / `public-api`: views expose new fields; empty-tags game omits `tags`.
- web: `fitTags` budget/min/max table-test; `displayTags` fallback; GameGrid renders tags and
  falls back to genres; 🔞 badge presence/absence per descriptor sets; mature filter
  predicate + URL round-trip; modal note rendering.
- Gates: cargo fmt --check · clippy --workspace --all-targets --all-features -D warnings ·
  cargo test --workspace · npm run build · vitest — green before PR.

## Deploy & resync (Ben's explicit requirement)

1. Terraform + lambda + web deploy per runbook.
2. **Run `backfill_details`** (with the new GetItems merge + auto-hide) once, outside the
   09:00Z cron window — populates tags/descriptors on the existing catalog (~30-day organic
   refresh otherwise) and auto-hides existing adult games.
3. Verify live: tags on cards, 🔞 badges, mature filter, Puss! state (already Ben-hidden —
   auto-hide must NOT have touched it: hidden stays true; his eventual unhide stamps Admin).

## Out of scope

Friend-side descriptor surfacing; per-tag filtering on the friend page; tightening the
auto-hide set (Ben can add ids later — it's a constant); persisting the GetTagList map;
required_age anywhere.
