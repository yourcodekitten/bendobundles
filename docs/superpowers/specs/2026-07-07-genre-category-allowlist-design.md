# Genre tags: allowlist player-mode categories — design

**Date:** 2026-07-07 · **Approved:** ben, on Discord (2026-07-07 evening) · **Author:** code kitten

## Problem

`SteamAppDetail.genres` is built by merging the Steam appdetails response's `genres[]` with
**all** of its `categories[]` descriptions (`crates/steam-client/src/lib.rs:411-414`; the field
doc at lib.rs:36 says so explicitly). Steam categories are store *features* — Steam Achievements,
Steam Cloud, Trading Cards, Remote Play, Family Sharing, controller support — not genres. The
result is tag soup: Stardew Valley (413150) currently renders **18 tags** where 6 carry signal.
This has been the behavior since the field was introduced; it is baked into every stored
`SteamAppCache.detail` blob.

## Decision

Keep real genres, plus an **id-based allowlist of top-level player-mode categories**:

| id | description   |
|----|---------------|
| 2  | Single-player |
| 1  | Multi-player  |
| 9  | Co-op         |
| 49 | PvP           |
| 20 | MMO           |

Mode *variants* (Online Co-op 38, LAN Co-op 48, Shared/Split Screen 24, Shared/Split Screen
Co-op 39, Online PvP 36, LAN PvP 47, Cross-Platform Multiplayer 27, …) are dropped: Steam
includes the parent category alongside its variants (verified on live appdetails data), so
coverage holds while tag count stays flat. Everything else in `categories[]` is noise and dies.

Result for Stardew: `Indie, RPG, Simulation, Single-player, Multi-player, Co-op` — 6 tags.

## Component 1: parse-time filter (`steam-client`)

- **Wire structs:** `categories[]` currently deserializes into `DescriptionWire { description }`.
  Categories gain their numeric `id`. NOTE the API quirk: `genres[].id` is a **string**,
  `categories[].id` is a **number** — so categories get their own
  `CategoryWire { id: u32, description: String }`; genres keep `DescriptionWire` untouched.
- **Filter:** a `const` allowlist of the five ids above. Parse appends only allowlisted
  categories after the genres, preserving API order, deduped order-preserving exactly as today.
- **Docs:** update the `SteamAppDetail.genres` field doc ("genres + allowlisted player-mode
  categories, deduped order-preserving").
- **No schema or API change.** The field stays `genres: Vec<String>`; the dynamo blob shape,
  public-api views, and web are untouched. Newly synced data is clean by construction.

## Component 2: backfill bin (`fulfillment`)

Stored blobs only self-heal via the 30-day appdetails TTL, and the enrichment pass is capped at
`STEAM_ENRICH_MAX_APPS` (75) per once-daily sync — ~10 days for the ~700-app catalog. Instead,
a run-once tool rebuilds the cache through the exact production code paths:

- **Shape:** feature-gated `[[bin]]` (`backfill_genres`, `required-features = ["backfill"]`) in
  the fulfillment crate — the same pattern as humble-client's `probe` bin.
- **Behavior:** distinct `steam_app_id`s from `Store::list_all_games` (the same universe the
  enrichment pass uses) → for each, `get_app_details` through the **new** parse → merge into the
  existing `SteamAppCache` **preserving the reviews half** (`reviews`, `recent`,
  `reviews_fetched_at`) → stamp `fetched_at = now` → `put_steam_app`. `Delisted` writes the same
  negative stub the enrichment pass writes.
- **Resumability:** skips apps whose `fetched_at` is within the last 12 hours, so an aborted run
  (e.g. a 429, which aborts just like the enrichment pass) resumes where it left off on rerun.
- **Pacing:** `STEAM_ENRICH_PACE` (1.5 s) between storefront calls; ~700 apps ≈ ~18 minutes.
- **Runtime needs:** DynamoDB table name via the same env config the lambdas use, standard AWS
  credentials chain; the appdetails storefront endpoint needs no API key. Run locally, once,
  right after deploy.
- **Output:** per-app log line plus a final summary (`fetched / negative / skipped / failed`).
- The bin is a thin wrapper over a testable async fn.

## Testing (TDD, red before green)

- **steam-client parse test:** fixture with real genres plus the full Stardew-style category
  pile → asserts the output is exactly genres + allowlisted modes, API order, deduped; asserts
  the noise strings are absent.
- **fulfillment backfill test (moto):** seed `STEAMAPP#` items with dirty merged genres and a
  mocked appdetails endpoint → run the backfill fn → assert genres rewritten clean, reviews half
  preserved byte-for-byte, fresh items skipped (12-hour rule), delisted apps stubbed.
- **Existing fixtures:** the only test fixture carrying `categories` today is
  `fulfillment/tests/handler_test.rs:4199` (`Single-player`, id 2) — allowlisted, so its
  assertions hold unchanged. steam-client's existing genre assertions (RPG etc.) hold.

## Rollout

1. One PR: parse filter + backfill bin + tests. CI green → review → squash-merge.
2. Deploy lambdas from the merge commit's CI `lambda-zips` artifact (the #55 runbook), then the
   web is untouched — no web deploy needed.
3. Run the backfill bin once from the box. Verify: spot-check Stardew (413150) via the public
   API shows 6 tags; no store-feature strings in any genre list.
4. No public-api, web, or terraform changes.

## Acceptance

- Stardew's link-list card shows `Indie, RPG, Simulation, Single-player, Multi-player` (take-5);
  its detail view shows those plus `Co-op`.
- No store-feature strings (Steam Achievements, Steam Cloud, Trading Cards, Remote Play*,
  Family Sharing, controller/input categories, Includes level editor, …) appear as genre tags
  anywhere.
- Review/histogram data is byte-identical before and after the backfill.
