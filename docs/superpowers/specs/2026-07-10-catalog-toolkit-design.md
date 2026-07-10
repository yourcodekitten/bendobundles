# Catalog Toolkit — filter / sort / group for the admin catalog

**Date:** 2026-07-10 · **Approved:** Ben, via Discord (design sections 1 + 2, incl.
release_date_iso amendment) · **Author:** kitten

## Goal

Give the admin catalog a "toolkit for digging through the trove": filter by tags and
ratings, sort by rating / release date / title, group by publisher, studio, or
bundle/choice month — all composing with the existing text search. ~1081 games,
~853 steam-mapped (live counts, 2026-07-10).

## Approach (chosen: enriched catalog + client-side toolkit)

All steam metadata already lives server-side in the `STEAMAPP#<app_id>` cache items
(`SteamAppCache`: `SteamAppDetail` + `ReviewSummary` + `RecentReviews`, maintained by
sync on 30/14-day windows). The catalog endpoint joins a compact projection of it into
each row once; every filter/sort/group interaction after that is pure client-side
derivation. No new endpoints, no schema changes, no new fetching from Steam.

Rejected: lazy per-game detail joins (1000+ requests to filter by tag once);
server-side query API (latency per click + backend surface for a single-admin
1k-row tool).

## Server changes

### 1. `steam-client`: release-date parser

`pub fn parse_release_date(raw: &str) -> Option<chrono::NaiveDate>`

Steam `release_date` display strings wander. Rules:
- Full date ("12 Nov 2019", "Nov 12, 2019") → exact date.
- Month-year ("Nov 2019") → first of that month.
- Anything else ("Coming soon", "TBA", "", garbage) → `None`.

Unit-tested against the real observed formats. Lives in `steam-client` because that
crate owns knowledge of Steam's formats. (Check existing deps before adding `chrono`;
if the workspace already parses dates another way, reuse that. If chrono is new,
prefer hand-rolling the two fixed formats over a new dependency — the format set is
closed.)

### 2. `dynamo`: batch read

`Store::batch_get_steam_apps(app_ids: &[u32]) -> Result<HashMap<u32, SteamAppCache>>`

DynamoDB `BatchGetItem`, chunks of 100 keys, `UnprocessedKeys` retried (bounded, with
the store's existing backoff idiom if one exists). Missing items are simply absent
from the map. Negative-cache stubs (`detail: None`) come back as present-but-empty;
the caller decides what to project.

### 3. `admin-api`: enriched catalog view

`CatalogGameView` grows `steam: Option<SteamSummaryView>`:

```rust
struct SteamSummaryView {
    genres: Vec<String>,          // from SteamAppDetail.genres
    developers: Vec<String>,
    publishers: Vec<String>,
    release_date: Option<String>,     // Steam's display string, for showing
    release_date_iso: Option<String>, // "YYYY-MM-DD" via parse_release_date
    review_desc: Option<String>,      // ReviewSummary.desc ("Very Positive", …)
    review_percent: Option<u8>,       // round(100 * total_positive / total_reviews); None if 0 reviews
    review_count: Option<u64>,        // ReviewSummary.total_reviews
    recent_percent: Option<u8>,       // RecentReviews.percent_positive
}
```

`handle_catalog`: collect distinct `steam_app_id`s → `batch_get_steam_apps` → join.
`steam: None` when: no app id mapped, no cache item, or negative-cache stub with no
reviews either. A stub with reviews but no detail projects what it has (fields
individually optional). Explicitly NOT included: screenshots, video, description,
header image — the fat stays on the detail endpoint.

Payload estimate: ~300KB total for ~1081 rows; acceptable for a session-gated admin
fetch that happens once per catalog visit.

## Web changes

### Types (`web/src/api.ts`)

`AdminGame` grows `steam: SteamSummary | null` mirroring the server view.

### Pure logic (`web/src/admin/catalogToolkit.ts` — new, unit-tested)

- `collectTagOptions(games)` → `[{tag, count}]`, union of genres, count = games carrying it, sorted by count desc then name.
- `filterGames(games, {search, tags, minRatingTier})`:
  - search: existing title-match semantics, moved here unchanged;
  - tags: AND — every selected tag must be in the game's genres; games with `steam: null` are excluded while any tag filter is active;
  - rating: Steam ladder mapped to ranks (Overwhelmingly Positive > Very Positive > Positive > Mostly Positive > Mixed > Mostly Negative > Negative > Very Negative > Overwhelmingly Negative); "at least tier X" keeps rank ≥ X; unrated/unmapped excluded while active.
  - Returns `{games, excludedNoData}` — the count of steam-less games a steam-field filter dropped, so the UI can say "212 unmapped hidden" instead of silently shrinking.
- `sortGames(games, key)`: `title` (default, locale compare) · `rating` (review_percent desc, review_count desc tiebreak, unrated last) · `date-new` / `date-old` (lexicographic on release_date_iso; null dates always last).
- `groupGames(games, key)`: `none` · `publisher` · `studio` (developers) · `bundle` (existing `bundle` field). Multi-valued games appear in every group they belong to (honest duplication). Bucket for missing data ("unmapped" / "no publisher") sorts last; otherwise groups sort by game count desc, then name. Returns `[{label, games}]`.
- Pipeline: filter → sort → group (sort applies within each group).

### UI (`web/src/admin/ToolkitBar.tsx` — new; `Catalog.tsx` wires it)

- Bar sits under the existing search input, same visual family as the rest of admin (bg-floor cards, ink/dust text, label-tier controls — quiet, not the friend-page arcade).
- Tag chips: horizontally wrapped chip set with counts; selected = filled; sits collapsed behind a "tags" disclosure if the full set is tall (decide in implementation by eyeballing ~20-40 real genres).
- Rating: native `<select>` "rating: any / at least Mixed / at least Mostly Positive / at least Very Positive / at least Overwhelmingly Positive" (coarse tiers only — the full 9-rung ladder is noise for filtering).
- Sort: native `<select>` (title a-z / rating / newest / oldest).
- Group: native `<select>` (none / publisher / studio / bundle month). Grouped view renders collapsible `<details>`-style sections with `label (n)` headers; default open.
- Active-filter summary line: "showing 143 of 1081 · 212 unmapped hidden" + a "clear all" button when any filter is active.
- **URL state:** toolkit state lives in `useSearchParams` (`?tags=Co-op,Action&rating=very-positive&sort=date-new&group=publisher&q=…`) so refresh/back/forward preserve the dig. Search box moves its state there too (it currently lives in `useState`).
- `Catalog.tsx` derives `visible = useMemo(pipeline)` and renders; existing row UI (status badges, giftable chip, self-claim arm/confirm, detail modal, hidden toggle) is untouched. Rows additionally show a compact rating + date readout where the toolkit makes them relevant (small dust-tier text; exact placement an implementation taste call).

## Error handling

- Server: batch-get failure → 500 as today (`list_all_games` already 500s); no partial-enrichment retry logic — a missing map entry just yields `steam: null` rows.
- Client: `steam: null` rows render exactly like today's rows; toolkit treats them as "no data" buckets. No new error states.

## Testing

- `steam-client`: parser table-test over observed formats (exact, month-year, TBA/Coming soon/empty/garbage).
- `dynamo`: batch chunking (101 ids → 2 calls), unprocessed-keys retry, missing-item absence (moto).
- `admin-api`: catalog join — mapped game gets summary, unmapped gets null, stub projection, review_percent rounding, iso parse wired.
- web `catalogToolkit.test.ts`: each pure function + composition (filter→sort→group), AND semantics, excludedNoData counting, null-date sinking, multi-publisher duplication, ladder ordering.
- web `Catalog.test.tsx` additions: toolkit renders from fixture data, URL round-trip (set params → state restored), clear-all, grouped sections render with counts.
- Gates: cargo fmt --check · clippy --workspace --all-targets --all-features -D warnings · cargo test --workspace · npm run build (tsc -b) · vitest — all green before PR.

## Out of scope

Friend-page surfacing of any of this; saved filter presets; server-side search API;
admin visual redesign beyond the toolkit bar (separate conversation).
