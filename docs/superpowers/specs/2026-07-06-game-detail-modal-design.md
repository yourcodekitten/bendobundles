# Game detail modal + Steam enrichment cache — design

**Status:** approved by Ben 2026-07-06; revised same day after spec review (B1, M1–M8 + minors
addressed; steam endpoints live-verified from AWS egress 2026-07-06)
**Author:** kitten
**Depends on:** the sync walk in fulfillment (`run_sync`), the existing GameGrid/Catalog SPA surfaces.
**Depended on by:** `2026-07-06-steam-integration-design.md` (the appid mapper + the steam-client
crate started here are its foundation). Build order: this before steam-integration.

## 1. Goal

Clicking a game — on the friend gift-link page **and** the admin catalog — opens a modal like the
Humble/Steam store pages: trailer video, review scores (overall + recent), developer, publisher,
tags/genres, release date, short description. Content comes from Steam's keyless storefront
endpoints, fetched **only during syncs** (Ben's be-nice rule, 2026-07-06) and cached in DynamoDB;
request-time reads never touch Steam.

**Why not on-demand cache-aside** (fetch on first modal open): considered and rejected — it violates
Ben's explicit only-on-syncs rule, adds first-open latency, and turns the public API into a
friend-triggered fetch/write amplifier (a hostile link holder could stampede Steam through us).
Sync-time prefetch keeps public-api pure-read.

## 2. The appid mapper (the real work, shared with steam-integration)

Humble gives `machine_name` + title; Steam content is keyed by **appid**. A sync pass resolves
`Game.steam_app_id: Option<u32>` (new domain field, with `appid_source: manual|humble|title`) for
steam-keytype games, in resolution order:

1. **Humble's own tpk data** — *unverified assumption*: real order tpks may carry a `steam_app_id`
   field. The shipped `TpkWire` has no such field, no fixture shows one, and the choice doc's field
   list is illustrative, not a capture. **Pre-build gate:** examine ONE real order payload (Ben HAR,
   or a captured response via the deployed stack) to confirm the field name — and where, if
   anywhere, a steam store link appears (tier 2's home). If absent, tiers 1–2 are deleted, not
   worked around.
2. **Humble store link** — parse the appid out of a `store.steampowered.com/app/<id>` URL *if* the
   §2.1 payload check shows one exists. Same gate.
3. **Exact-title match** — against Steam's app list, after light normalization (case, trademark
   glyphs). **Unique exact match only**: GetAppList is full of duplicate names (demos, soundtracks,
   re-releases); an ambiguous exact match resolves to **nothing** and is logged as unmapped — a
   wrong trailer is worse than none. The app list (~10–15MB) is fetched **lazily**, only when
   unresolved steam-keytype games exist this sync.
4. **Admin override** — `POST /admin/api/games/:id/steam-app-id {app_id | null}` + a small "set
   appid" affordance in the catalog. **`null` means clear-to-auto** (auto-resolution runs again next
   sync); "pinned none" is a non-goal until needed. After setting an appid the modal stays thin
   until the next sync fetches the STEAMAPP item — accepted lag; the documented workflow is
   "set appid → hit Sync now."

**Coverage expectation, written assuming tiers 1–2 contribute nothing:** exact-unique-title match
alone will leave a real manual tail (old bundle titles are cursed — expect on the order of 10–20%
of steam-keytype games needing overrides or staying unmapped). If the §2.1 payload check confirms
tier 1, the tail shrinks to near zero. Per-sync mapping coverage (mapped/unmapped counts) is logged
either way.

**No-clobber, the actual mechanism:** `steam_app_id` + `appid_source` are **app-owned fields** —
`merge_sync` preserves the existing values in *both* of its branches (like `hidden`; they are never
humble-derived). Only the mapper pass writes them, and it skips games with `appid_source: manual`.
Residual race, accepted and documented: an admin override landing between a sync's read of the game
and its PutItem is lost (window of seconds, at personal scale; repair = set it again — the next
sync preserves it).

## 3. steam-client crate (started here) + enrichment cache

The storefront reads live in a new **steam-client crate** from day one (steam-integration extends
the same crate with OpenID + owned-games; the two specs share this placement). **Plain `reqwest`** —
none of humble-client's wreq TLS-fingerprint machinery; Steam's storefront serves vanilla clients.

**The three calls per app — live-verified 2026-07-06 from AWS egress (this box), exact recipes:**

1. `GET store.steampowered.com/api/appdetails?appids=<id>&cc=us&l=english` → `<id>.data`:
   `name`, `developers[]`, `publishers[]`, `genres[]`, `categories[]`, `release_date.date`,
   `short_description`, `header_image`, `movies[]`. **Movies carry ONLY streaming manifests now** —
   `hls_h264`, `dash_h264`, `dash_av1`, `thumbnail` — there are **no mp4/webm fields** anymore
   (verified live; older docs lie). Store the `hls_h264` URL + thumbnail.
2. `GET store.steampowered.com/appreviews/<id>?json=1&num_per_page=0&language=english&purchase_type=all`
   → `query_summary` = the **OVERALL** summary (`review_score_desc`, `total_positive`,
   `total_negative`, `total_reviews`). Verified: `query_summary` reflects overall regardless of
   `filter`/`day_range` params — there is **no param recipe that returns the recent badge** (tested
   `filter=recent`, `day_range=30`, `filter=summary&day_range=30`: all overall).
3. `GET store.steampowered.com/appreviewhistogram/<id>?l=english` → `results.recent` = ~30 daily
   buckets of `recommendations_up/down`. The **recent** summary is computed from these: percent
   positive + count (verified: Stardew = 98% of 9,200, matching the store's recent badge). Display
   as "98% positive (9,200 recent)"; a desc string may be derived from Steam's public score
   thresholds later — non-goal now.

Wiremock fixtures are derived from the real responses captured during this verification (saved from
the live calls), not guessed shapes.

**Cache item:** `pk=STEAMAPP#{app_id}`, `sk=META` — the fields above + `fetched_at` (epoch),
`reviews_fetched_at` (epoch). **Negative caching:** an appdetails `success:false` (delisted — common
for old bundles) writes a stub item with `fetched_at` and a `delisted` marker, retried on the
appdetails window, so it is not refetched every sync forever. Multiple games mapping to one appid
share one item.

**Refresh policy + per-sync budget (the be-nice rule, with honest arithmetic):**

- Staleness windows: **appdetails 30 days** (near-static), **reviews + histogram 14 days** (the only
  fields that drift).
- Steady state on a ~700-app catalog: ~50 apps/day hit the review window (2 calls each) + ~23/day
  hit the appdetails window (1 call) ≈ **~125 calls/sync ≈ 3–4 minutes** at the pacing below. The
  earlier "~0 calls on a normal sync" claim was wrong and is retracted.
- Pacing: **≥1.5s between storefront calls** (the informal storefront limit is ~200 req/5min/IP;
  1.5s ≈ 200/5min exactly — the earlier 350ms figure was ~4× over it).
- **Per-sync budget, both axes:** at most **75 appids** per pass, AND a **deadline guard** — the
  pass stops when less than ~180s of Lambda budget remains (the fulfillment timeout is 900s;
  `persist_sync` + `end_sync_run` must always land). Partial progress persists per-item
  (`fetched_at` written as each app completes), so the next sync resumes where this one stopped.
- **First run:** ~700 apps ÷ 75/sync = the backlog drains over ~10 daily syncs, or faster if Ben
  presses "Sync now" a few times. Stated honestly: the first-week modal coverage grows day by day.
- **429 handling:** a 429 (or any rate-limit signal) **aborts the enrichment pass for this run** —
  be-nice means back off, not skip-and-continue into a storm. The sync itself still completes and
  reports; the pass resumes next sync.
- **Observability + kill switch:** one log line per sync — enriched/skipped/negative/429-aborted +
  mapper mapped/unmapped counts; an env kill switch (`STEAM_ENRICH_DISABLED=1`) turns the pass off
  entirely if Steam starts refusing our egress IPs.

## 4. API — the two response shapes, written down

Friend and admin get **different** shapes (the admin needs fields the friend must not see):

```jsonc
// GET /api/l/:token/games/:id/detail        (friend; token must be a live link)
{ "game": { "id", "title", "bundle", "key_type", "artwork_url" },      // GameView, as today
  "steam": SteamAppDetail | null }                                     // null = unmapped/non-steam

// GET /admin/api/games/:id/detail           (admin; session-guarded)
{ "game": { ...CatalogGameView, "requires_choice": bool },             // superset incl. status,
  "steam": SteamAppDetail | null }                                     // giftable, hidden
```

`SteamAppDetail` = the §3 cache fields (video HLS url, thumbnail, developers, publishers, genres,
release date, short description, header image, overall summary, recent percent/count, fetched_at).
The SPA's thin fallback branches on `steam: null`. Exposing `requires_choice` on the admin catalog
view is required here anyway (the self-claim confirm needs it too).

**Friend endpoint access rule (no-oracle discipline):** serves games that are currently listable
**or** appear in this link's own claims history (detail-on-history is useful); anything else —
hidden, unknown id, other links' games — returns the same 404 as a bad token. Rate-limited like the
link fetch.

**On ids (correcting a false claim in the earlier draft):** the composite game id is
`{gamekey}:{machine_name}` — the humble order key is **already exposed** in every catalog/link
response on both surfaces today; this endpoint additionally moves it into URL paths (API-gateway
access logs, browser history). Accepted deliberately at this trust level: a gamekey alone redeems
nothing (every humble write also needs the session cookie), links are bearer-gated, and the admin
surface is Ben. The stale "gamekey must not leak into browser network tabs" comment on
CatalogGameView gets updated to match reality. Opaque ids would be a catalog-wide migration —
out of scope, noted as a future hardening option.

## 5. Web

One `GameDetailModal` component, two mounts (friend GameGrid card click, admin Catalog row click):

- **Trailer via HLS**: `hls.js` on Chrome/Firefox, native HLS on Safari (video CDN verified live:
  `access-control-allow-origin: *`, no referer gating — plays cross-origin from bendobundles.com).
  Click-to-play (no autoplay), poster = movie thumbnail; artwork fallback when no trailer.
- title, developer / publisher / release date line; overall review badge (desc + total) and recent
  badge (percent + count); genre/category chips; short description.
- **friend mount:** claim button in the modal footer, wiring into the existing ClaimDialog;
  disabled states follow the existing grid rules. (GameGrid groups duplicate titles into one card;
  the modal opens with the first copy's id — same appid, harmless, noted.)
- **admin mount, sequenced:** ships with a status badge in the footer only. The self-claim action
  lands there **after** the self-claim feature builds, routed through the **shared arm/confirm
  component** (the same one carrying the choice-pick warning and, later, the steam-owned warning) —
  never a modal-local confirm.
- **thin fallback** on `steam: null`: artwork, bundle, key type, "no steam page for this one."
- Escape/backdrop closes; detail fetched lazily on open, cached in component state per session.

## 6. Rollout

0. **Pre-build gate (M1):** one real order payload examined → tiers 1–2 confirmed or deleted;
   coverage expectations updated in this doc.
1. steam-client crate: the three storefront calls, fixtures from the live captures (wiremock).
2. Domain fields (`steam_app_id`, `appid_source`) + merge_sync preservation + mapper pass
   (tiers as gated) + coverage logging (moto).
3. Admin override endpoint + catalog affordance.
4. Enrichment pass: budget/deadline/pacing/429-abort/negative-cache/kill-switch (moto + fake clock).
5. Detail endpoints (both shapes, no-oracle rule) (moto).
6. `GameDetailModal` + hls.js + both mounts.

## 7. Verification

Wiremock: appdetails (with movies / without / success:false), appreviews overall, histogram, lazy
GetAppList. Moto: mapper resolution order, unique-exact-only ambiguity handling, manual-override
no-clobber (merge_sync both branches) + skip-manual, staleness windows (30d/14d), per-sync budget +
deadline guard, 429-abort, negative-cache stub, detail endpoints' shapes + no-oracle 404s. SPA:
modal full/thin variants, HLS fallback to poster on error, claim-from-modal path. Live check after
first deployed sync: enrichment log line sane, spot-read STEAMAPP items, Ben eyeballs the modal on
both surfaces (trailer actually plays from bendobundles.com).
