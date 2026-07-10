# Catalog Toolkit Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enrich `/admin/api/catalog` rows with a compact steam summary and build a client-side filter/sort/group toolkit on the admin catalog page.

**Architecture:** Read-side join in `handle_catalog` (mirrors public-api's existing `batch_get_steam_apps` idiom) ships `steam: SteamSummary | null` per row; all toolkit interactions are pure client-side derivation (`catalogToolkit.ts`) rendered by a presentational `ToolkitBar` and wired in `Catalog.tsx` with URL-persisted state.

**Tech Stack:** Rust (axum admin-api, `time` 0.3 for date parsing — chrono in the spec supersedes to `time`, the workspace date lib; NO new dependencies), React 18 + react-router-dom 7 (`useSearchParams`), vitest + testing-library, moto for store/api tests.

**Spec:** `docs/superpowers/specs/2026-07-10-catalog-toolkit-design.md`

## Global Constraints

- `Store::batch_get_steam_apps` ALREADY EXISTS (`crates/dynamo/src/lib.rs:1745`) with moto tests — do NOT re-implement it.
- Never leak `gamekey`/`machine_name`/`keyindex` fields into `CatalogGameView` (existing comment at `crates/admin-api/src/lib.rs:244` explains why).
- Existing search semantics preserved exactly: lowercase `includes` on title OR bundle.
- Existing Catalog row UI (status badges, self-claim arm/confirm, hidden toggle, detail modal) untouched except the added rating/date readout.
- Gates before PR: `cargo fmt --check` · `cargo clippy --workspace --all-targets --all-features -- -D warnings` · `cargo test --workspace` (fresh moto) · `npm run build` · `npx vitest run` (in `web/`).
- moto: restart between workspace suite runs (`kill $(pgrep -x moto_server)`); tests hit localhost:8000.
- PATH: `export PATH="$HOME/.cargo/bin:$HOME/.local/node22/bin:$PATH"`.
- All commits GPG-signed (`-S`), author `code kitten <yourcodekitten@gmail.com>`.

---

### Task 1: `steam-client` — `parse_release_date`

**Files:**
- Modify: `crates/steam-client/src/lib.rs` (add pub fn + `time` dep usage)
- Modify: `crates/steam-client/Cargo.toml` (add `time.workspace = true` if not present)
- Test: `crates/steam-client/tests/client_test.rs` (append)

**Interfaces:**
- Produces: `pub fn parse_release_date(raw: &str) -> Option<time::Date>` — Task 2 calls this and formats with `.to_string()` (time::Date Display IS ISO-8601 `YYYY-MM-DD`).

- [ ] **Step 1: Write the failing test** (append to `crates/steam-client/tests/client_test.rs`)

```rust
/// parse_release_date: Steam's display formats → ISO date. Full dates parse
/// exact, bare month-year parses to the first of the month, everything else
/// (TBA / Coming soon / empty / garbage) is None.
#[test]
fn parse_release_date_observed_formats() {
    use steam_client::parse_release_date;
    let d = |s: &str| parse_release_date(s).map(|d| d.to_string());
    // full date, EU order (the dominant Steam format)
    assert_eq!(d("12 Nov 2019"), Some("2019-11-12".into()));
    assert_eq!(d("1 Jan 2024"), Some("2024-01-01".into()));
    // full date, US order with comma
    assert_eq!(d("Nov 12, 2019"), Some("2019-11-12".into()));
    // month-year → first of month
    assert_eq!(d("Nov 2019"), Some("2019-11-01".into()));
    // surrounding whitespace tolerated
    assert_eq!(d("  12 Nov 2019 "), Some("2019-11-12".into()));
    // unparseable → None
    assert_eq!(d("Coming soon"), None);
    assert_eq!(d("TBA"), None);
    assert_eq!(d(""), None);
    assert_eq!(d("Q3 2026"), None);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd ~/bendobundles && export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p steam-client parse_release_date`
Expected: FAIL — `cannot find function parse_release_date`

- [ ] **Step 3: Implement** (in `crates/steam-client/src/lib.rs`; add `time.workspace = true` to `crates/steam-client/Cargo.toml` `[dependencies]` first)

```rust
/// Parse Steam's `release_date` display string into a date.
/// Steam ships display strings, not timestamps; the observed closed set:
/// "12 Nov 2019" (dominant), "Nov 12, 2019", bare "Nov 2019" (→ first of
/// month), and non-dates ("Coming soon", "TBA", "Q3 2026", "") → None.
pub fn parse_release_date(raw: &str) -> Option<time::Date> {
    use time::macros::format_description;
    let raw = raw.trim();
    let eu = format_description!(
        "[day padding:none] [month repr:short case_sensitive:false] [year]"
    );
    let us = format_description!(
        "[month repr:short case_sensitive:false] [day padding:none], [year]"
    );
    time::Date::parse(raw, eu)
        .or_else(|_| time::Date::parse(raw, us))
        // bare month-year: reuse the EU shape by pinning day 1
        .or_else(|_| time::Date::parse(&format!("1 {raw}"), eu))
        .ok()
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p steam-client parse_release_date`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/steam-client
git commit -S -m "steam-client: parse_release_date — display strings to ISO dates"
```

---

### Task 2: `admin-api` — `SteamSummaryView` + catalog join

**Files:**
- Modify: `crates/admin-api/src/lib.rs` (view struct + projection fn + `handle_catalog`)
- Test: `crates/admin-api/tests/api_test.rs` (extend the existing catalog test around line 610)

**Interfaces:**
- Consumes: `steam_client::parse_release_date` (Task 1), `Store::batch_get_steam_apps` (exists), `dynamo::SteamAppCache { detail, overall, recent, .. }`.
- Produces: catalog JSON rows gain `"steam": null | {genres, developers, publishers, release_date, release_date_iso, review_desc, review_percent, review_count, recent_percent}` — Task 3's `SteamSummary` TS type mirrors this exactly.

- [ ] **Step 1: Write the failing test** — extend the existing moto catalog test (`crates/admin-api/tests/api_test.rs` ~line 610; follow its existing setup idiom for store/app construction). Add a second game mapped to a cached steam app, plus the cache item:

```rust
/// Catalog rows carry a compact steam summary when the app cache has one;
/// unmapped games carry steam: null. review_percent rounds half-up.
#[tokio::test]
async fn catalog_joins_steam_summary() {
    let (app, store) = test_app().await; // match the file's existing helper name
    let mut g = fixture_game("gk1", "mapped-game"); // match existing fixture helper
    g.steam_app_id = Some(570);
    store.put_game(&g).await.unwrap();
    let mut unmapped = fixture_game("gk2", "unmapped-game");
    unmapped.steam_app_id = None;
    store.put_game(&unmapped).await.unwrap();

    store
        .put_steam_app(&dynamo::SteamAppCache {
            app_id: 570,
            detail: Some(steam_client::SteamAppDetail {
                app_id: 570,
                name: "Mapped Game".into(),
                developers: vec!["Dev Studio".into()],
                publishers: vec!["Pub House".into()],
                genres: vec!["Action".into(), "Co-op".into()],
                release_date: Some("12 Nov 2019".into()),
                short_description: String::new(),
                header_image: None,
                video_hls_url: None,
                video_thumbnail: None,
                screenshots: vec![],
            }),
            overall: Some(steam_client::ReviewSummary {
                desc: "Very Positive".into(),
                total_positive: 2,
                total_negative: 1,
                total_reviews: 3,
            }),
            recent: Some(steam_client::RecentReviews { percent_positive: 80, count: 40 }),
            fetched_at: 1,
            reviews_fetched_at: 1,
        })
        .await
        .unwrap();

    let body = authed_get_json(&app, "/admin/api/catalog").await; // match existing helper
    let rows = body.as_array().unwrap();
    let mapped = rows.iter().find(|r| r["title"] == "mapped-game").unwrap();
    let s = &mapped["steam"];
    assert_eq!(s["genres"], serde_json::json!(["Action", "Co-op"]));
    assert_eq!(s["developers"], serde_json::json!(["Dev Studio"]));
    assert_eq!(s["publishers"], serde_json::json!(["Pub House"]));
    assert_eq!(s["release_date"], "12 Nov 2019");
    assert_eq!(s["release_date_iso"], "2019-11-12");
    assert_eq!(s["review_desc"], "Very Positive");
    assert_eq!(s["review_percent"], 67); // 2/3 rounds to 67
    assert_eq!(s["review_count"], 3);
    assert_eq!(s["recent_percent"], 80);
    let un = rows.iter().find(|r| r["title"] == "unmapped-game").unwrap();
    assert!(un["steam"].is_null());
}
```

(Adapt helper names to what the file actually defines — it already builds an app + store against moto and makes authed requests; read its first ~80 lines before writing. If assert helpers differ, keep the assertions, change the plumbing.)

- [ ] **Step 2: Run test to verify it fails**

Run: `moto_server -p 8000 &` (if not running) then `cargo test -p admin-api catalog_joins_steam_summary`
Expected: FAIL — `steam` key absent (or compile error on the struct until Step 3)

- [ ] **Step 3: Implement** in `crates/admin-api/src/lib.rs`:

3a. Add the view + projection next to `CatalogGameView` (~line 252):

```rust
/// Compact steam projection for catalog rows — the toolkit's filter/sort/group
/// data. Deliberately excludes screenshots/video/description (the fat stays on
/// the detail endpoint). `None` fields are individually absent-but-honest.
#[derive(serde::Serialize)]
struct SteamSummaryView {
    genres: Vec<String>,
    developers: Vec<String>,
    publishers: Vec<String>,
    release_date: Option<String>,
    /// "YYYY-MM-DD" parsed server-side (time::Date Display is ISO-8601).
    release_date_iso: Option<String>,
    review_desc: Option<String>,
    /// round(100 * positive / total); None when 0 reviews.
    review_percent: Option<u8>,
    review_count: Option<u64>,
    recent_percent: Option<u8>,
}

/// Project a cache entry to the summary. Returns None for entries with
/// nothing to show (negative-cache stub with no reviews either) so the row
/// serializes `steam: null` rather than an all-null husk.
fn steam_summary(cache: &dynamo::SteamAppCache) -> Option<SteamSummaryView> {
    if cache.detail.is_none() && cache.overall.is_none() && cache.recent.is_none() {
        return None;
    }
    let d = cache.detail.as_ref();
    let release_date = d.and_then(|d| d.release_date.clone());
    let release_date_iso = release_date
        .as_deref()
        .and_then(steam_client::parse_release_date)
        .map(|d| d.to_string());
    let o = cache.overall.as_ref();
    let review_percent = o.filter(|o| o.total_reviews > 0).map(|o| {
        ((o.total_positive * 100 + o.total_reviews / 2) / o.total_reviews) as u8
    });
    Some(SteamSummaryView {
        genres: d.map(|d| d.genres.clone()).unwrap_or_default(),
        developers: d.map(|d| d.developers.clone()).unwrap_or_default(),
        publishers: d.map(|d| d.publishers.clone()).unwrap_or_default(),
        release_date,
        release_date_iso,
        review_desc: o.map(|o| o.desc.clone()),
        review_percent,
        review_count: o.map(|o| o.total_reviews),
        recent_percent: cache.recent.as_ref().map(|r| r.percent_positive),
    })
}
```

3b. Add `steam: Option<SteamSummaryView>` as the last field of `CatalogGameView`.

3c. In `handle_catalog`, mirror public-api's join (`crates/public-api/src/lib.rs:496-515`): after `list_all_games` succeeds, collect distinct app ids, batch-read best-effort, join:

```rust
async fn handle_catalog(State(s): State<AppState>) -> Response {
    match s.store.list_all_games().await {
        Ok(games) => {
            // One BatchGetItem over the distinct appids (same idiom as the
            // link view in public-api). Best-effort: a failed batch degrades
            // every row to steam: null — the toolkit shows "unmapped" buckets,
            // never an error.
            let mut app_ids: Vec<u32> = games.iter().filter_map(|g| g.steam_app_id).collect();
            app_ids.sort_unstable();
            app_ids.dedup();
            let caches = s
                .store
                .batch_get_steam_apps(&app_ids)
                .await
                .unwrap_or_default();
            let views: Vec<CatalogGameView> = games
                .into_iter()
                .map(|g| CatalogGameView {
                    steam: g
                        .steam_app_id
                        .and_then(|id| caches.get(&id))
                        .and_then(steam_summary),
                    id: g.id,
                    title: g.title,
                    bundle: g.bundle,
                    key_type: g.key_type,
                    giftable: g.giftable,
                    hidden: g.hidden,
                    status: g.status,
                    claim_id: g.claim_id,
                    artwork_url: g.artwork_url,
                    requires_choice: g.requires_choice,
                    steam_app_id: g.steam_app_id,
                    owned_by_ben: g.owned_by_ben,
                })
                .collect();
            (StatusCode::OK, Json(views)).into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
```

Add `steam-client` to `crates/admin-api/Cargo.toml` `[dependencies]` if it isn't already there (check — the detail endpoint likely already imports it).

- [ ] **Step 4: Run tests**

Run: `cargo test -p admin-api` (fresh moto)
Expected: `catalog_joins_steam_summary` PASS and every pre-existing api_test PASS (rows now carry `steam: null` — existing assertions don't check full shape, verify none break).

- [ ] **Step 5: Commit**

```bash
git add crates/admin-api
git commit -S -m "admin-api: catalog rows carry a compact steam summary (toolkit data layer)"
```

---

### Task 3: web — `SteamSummary` type + `catalogToolkit.ts` pure logic

**Files:**
- Modify: `web/src/api.ts` (AdminGame + SteamSummary)
- Create: `web/src/admin/catalogToolkit.ts`
- Test: `web/src/admin/catalogToolkit.test.ts` (new)
- Modify: `web/src/admin/Catalog.test.tsx` + any fixture building `AdminGame` (add `steam: null` — compile fix only)

**Interfaces:**
- Consumes: catalog JSON shape from Task 2.
- Produces (Task 4 + 5 rely on these exact names):
  - `type SteamSummary = { genres: string[]; developers: string[]; publishers: string[]; release_date: string | null; release_date_iso: string | null; review_desc: string | null; review_percent: number | null; review_count: number | null; recent_percent: number | null }`
  - `AdminGame` gains `steam: SteamSummary | null`
  - `type ToolkitState = { q: string; tags: string[]; rating: RatingFloor; sort: SortKey; group: GroupKey }`
  - `type RatingFloor = 'any' | 'mixed' | 'mostly-positive' | 'very-positive' | 'overwhelmingly-positive'`
  - `type SortKey = 'title' | 'rating' | 'date-new' | 'date-old'`
  - `type GroupKey = 'none' | 'publisher' | 'studio' | 'bundle'`
  - `collectTagOptions(games: AdminGame[]): { tag: string; count: number }[]`
  - `applyToolkit(games: AdminGame[], state: ToolkitState): { groups: { label: string | null; games: AdminGame[] }[]; shown: number; excludedNoData: number }` (label null when group === 'none' — single anonymous group)

- [ ] **Step 1: Types in `web/src/api.ts`** — add `SteamSummary` above `AdminGame`, add `steam: SteamSummary | null;` to `AdminGame`. Then run `npx tsc -b` in `web/` and add `steam: null` to every fixture the compiler flags (Catalog.test.tsx `gameFixture` etc.). Commit separately only if green alone; otherwise fold into Step 5's commit.

- [ ] **Step 2: Write the failing tests** (`web/src/admin/catalogToolkit.test.ts`) — cover, with small inline fixtures (spread a `base` AdminGame like Catalog.test.tsx does):
  - `collectTagOptions`: union with counts, sorted count-desc then name-asc; steam-null games contribute nothing.
  - filter — search: matches title OR bundle, case-insensitive (same strings as today).
  - filter — tags AND: `['Action','Co-op']` keeps only games whose genres include both; any active tag filter excludes steam-null games and counts them in `excludedNoData`.
  - filter — rating floor: `'very-positive'` keeps `review_desc` of Very/Overwhelmingly Positive only; unrated (`review_desc: null`) and unknown descs excluded + counted; `'any'` keeps everything including steam-null.
  - sort — `'rating'`: review_percent desc, review_count desc tiebreak, unrated last, title tiebreak; `'date-new'`/`'date-old'`: lexicographic on release_date_iso, null iso always last in BOTH directions; `'title'`: localeCompare.
  - group — `'publisher'`: multi-publisher game appears in each group (honest duplication); groups sorted by count desc then label; steam-null bucket labeled `'unmapped'` last; mapped-but-empty bucket `'no publisher listed'` second-to-last; `'studio'` same over developers; `'bundle'` groups on `g.bundle`; `'none'` → one group, `label: null`.
  - composition: filter runs before grouping; sort applies within each group.

Write them as real `expect` assertions over ids, e.g.:

```ts
const ids = (r: ReturnType<typeof applyToolkit>) => r.groups.flatMap((g) => g.games.map((x) => x.id));
expect(ids(applyToolkit(games, { ...idle, tags: ['Action', 'Co-op'] }))).toEqual(['both']);
```

(`idle` = `{ q: '', tags: [], rating: 'any', sort: 'title', group: 'none' }` — export it as `IDLE_TOOLKIT` from catalogToolkit.ts for reuse.)

- [ ] **Step 3: Run tests to verify they fail**

Run: `cd web && npx vitest run src/admin/catalogToolkit.test.ts`
Expected: FAIL — module doesn't exist

- [ ] **Step 4: Implement `web/src/admin/catalogToolkit.ts`**

```ts
import type { AdminGame } from '../api';

export type RatingFloor =
  | 'any'
  | 'mixed'
  | 'mostly-positive'
  | 'very-positive'
  | 'overwhelmingly-positive';
export type SortKey = 'title' | 'rating' | 'date-new' | 'date-old';
export type GroupKey = 'none' | 'publisher' | 'studio' | 'bundle';

export type ToolkitState = {
  q: string;
  tags: string[];
  rating: RatingFloor;
  sort: SortKey;
  group: GroupKey;
};

export const IDLE_TOOLKIT: ToolkitState = {
  q: '',
  tags: [],
  rating: 'any',
  sort: 'title',
  group: 'none',
};

// Steam's review ladder, worst→best. Rank = index. An unknown desc (future
// Steam wording) ranks as unrated: excluded while a floor is active, never
// silently promoted.
const LADDER = [
  'Overwhelmingly Negative',
  'Very Negative',
  'Negative',
  'Mostly Negative',
  'Mixed',
  'Mostly Positive',
  'Positive',
  'Very Positive',
  'Overwhelmingly Positive',
] as const;
const FLOOR_RANK: Record<Exclude<RatingFloor, 'any'>, number> = {
  mixed: LADDER.indexOf('Mixed'),
  'mostly-positive': LADDER.indexOf('Mostly Positive'),
  'very-positive': LADDER.indexOf('Very Positive'),
  'overwhelmingly-positive': LADDER.indexOf('Overwhelmingly Positive'),
};

export function collectTagOptions(games: AdminGame[]): { tag: string; count: number }[] {
  const counts = new Map<string, number>();
  for (const g of games)
    for (const t of g.steam?.genres ?? []) counts.set(t, (counts.get(t) ?? 0) + 1);
  return [...counts]
    .map(([tag, count]) => ({ tag, count }))
    .sort((a, b) => b.count - a.count || a.tag.localeCompare(b.tag));
}

function matchesSearch(g: AdminGame, q: string): boolean {
  if (q === '') return true;
  return g.title.toLowerCase().includes(q) || g.bundle.toLowerCase().includes(q);
}

/** Filter → sort → group. `excludedNoData` counts games a steam-field filter
 * (tags / rating floor) dropped solely for having no data — the UI surfaces
 * this so the trove never silently shrinks. */
export function applyToolkit(
  games: AdminGame[],
  state: ToolkitState,
): { groups: { label: string | null; games: AdminGame[] }[]; shown: number; excludedNoData: number } {
  const q = state.q.toLowerCase();
  let excludedNoData = 0;
  const filtered = games.filter((g) => {
    if (!matchesSearch(g, q)) return false;
    if (state.tags.length > 0) {
      const genres = g.steam?.genres;
      if (!genres || genres.length === 0) {
        excludedNoData++;
        return false;
      }
      if (!state.tags.every((t) => genres.includes(t))) return false;
    }
    if (state.rating !== 'any') {
      const rank = g.steam?.review_desc ? LADDER.indexOf(g.steam.review_desc as (typeof LADDER)[number]) : -1;
      if (rank === -1) {
        excludedNoData++;
        return false;
      }
      if (rank < FLOOR_RANK[state.rating]) return false;
    }
    return true;
  });

  const sorted = [...filtered].sort(comparator(state.sort));
  const groups = groupGames(sorted, state.group);
  return { groups, shown: filtered.length, excludedNoData };
}

function comparator(key: SortKey): (a: AdminGame, b: AdminGame) => number {
  const byTitle = (a: AdminGame, b: AdminGame) => a.title.localeCompare(b.title);
  switch (key) {
    case 'title':
      return byTitle;
    case 'rating':
      return (a, b) => {
        const ap = a.steam?.review_percent ?? -1;
        const bp = b.steam?.review_percent ?? -1;
        if (ap !== bp) return bp - ap; // unrated (-1) sinks
        const ac = a.steam?.review_count ?? 0;
        const bc = b.steam?.review_count ?? 0;
        if (ac !== bc) return bc - ac;
        return byTitle(a, b);
      };
    case 'date-new':
    case 'date-old': {
      const dir = key === 'date-new' ? -1 : 1;
      return (a, b) => {
        const ai = a.steam?.release_date_iso ?? null;
        const bi = b.steam?.release_date_iso ?? null;
        if (ai === null && bi === null) return byTitle(a, b);
        if (ai === null) return 1; // no-date sinks in both directions
        if (bi === null) return -1;
        if (ai !== bi) return dir * ai.localeCompare(bi);
        return byTitle(a, b);
      };
    }
  }
}

const UNMAPPED = 'unmapped';

function groupGames(
  games: AdminGame[],
  key: GroupKey,
): { label: string | null; games: AdminGame[] }[] {
  if (key === 'none') return [{ label: null, games }];
  const noData = key === 'bundle' ? '' : UNMAPPED; // bundle always present
  const emptyLabel = key === 'publisher' ? 'no publisher listed' : 'no studio listed';
  const buckets = new Map<string, AdminGame[]>();
  const push = (label: string, g: AdminGame) => {
    const b = buckets.get(label);
    if (b) b.push(g);
    else buckets.set(label, [g]);
  };
  for (const g of games) {
    if (key === 'bundle') {
      push(g.bundle, g);
      continue;
    }
    if (!g.steam) {
      push(noData, g);
      continue;
    }
    const vals = key === 'publisher' ? g.steam.publishers : g.steam.developers;
    if (vals.length === 0) push(emptyLabel, g);
    else for (const v of vals) push(v, g); // multi-valued: honest duplication
  }
  const tail = (label: string) => (label === UNMAPPED ? 2 : label === emptyLabel ? 1 : 0);
  return [...buckets]
    .map(([label, gs]) => ({ label, games: gs }))
    .sort(
      (a, b) =>
        tail(a.label!) - tail(b.label!) ||
        b.games.length - a.games.length ||
        a.label!.localeCompare(b.label!),
    );
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cd web && npx vitest run src/admin/catalogToolkit.test.ts && npx tsc -b`
Expected: PASS, clean build

- [ ] **Step 6: Commit**

```bash
git add web/src/api.ts web/src/admin/catalogToolkit.ts web/src/admin/catalogToolkit.test.ts web/src/admin/Catalog.test.tsx
git commit -S -m "web: SteamSummary type + catalogToolkit pure logic (filter/sort/group)"
```

---

### Task 4: web — `ToolkitBar` component

**Files:**
- Create: `web/src/admin/ToolkitBar.tsx`
- Test: `web/src/admin/ToolkitBar.test.tsx` (new)

**Interfaces:**
- Consumes: `ToolkitState`, `RatingFloor`, `SortKey`, `GroupKey`, `IDLE_TOOLKIT`, `collectTagOptions` output shape from Task 3.
- Produces: `function ToolkitBar(props: { state: ToolkitState; tagOptions: { tag: string; count: number }[]; shown: number; total: number; excludedNoData: number; onChange: (next: ToolkitState) => void }): JSX.Element` — Task 5 renders this.

Controlled component; no internal state except the tags `<details>` disclosure. Visual family: existing admin controls (`rounded border border-line bg-floor px-3 py-1.5 text-sm`, `text-dust` labels — copy the search input's classes in `Catalog.tsx:193-200`).

- [ ] **Step 1: Write the failing tests** (`ToolkitBar.test.tsx`; plain `render`, no router needed):
  - renders a select labeled `rating`, one labeled `sort`, one labeled `group` with the option sets from Task 3's types.
  - tag chips render inside a `<details>` labeled `tags` with counts (`Action (12)`); clicking a chip calls `onChange` with the tag toggled into/out of `state.tags`.
  - selecting `rating: at least Very Positive` calls `onChange({...state, rating: 'very-positive'})`.
  - when any filter active: summary line `showing 143 of 1081` plus `· 212 unmapped hidden` when `excludedNoData > 0`, and a `clear filters` button that fires `onChange` with `{...IDLE_TOOLKIT, sort: state.sort, group: state.group}` (clear clears *filters*, not view prefs).
  - when idle (state deep-equals filters-idle): no clear button, summary just `1081 games`.

- [ ] **Step 2: Run to verify FAIL** — `npx vitest run src/admin/ToolkitBar.test.tsx`

- [ ] **Step 3: Implement.** Layout: one flex-wrap row — tags `<details>` (chips wrap inside), three labeled `<select>`s, summary text pushed right (`ml-auto`), clear button. Selected chip = `bg-pixel/20 border-pixel text-ink`; idle chip = search-input classes. Option labels: rating `any / at least Mixed / at least Mostly Positive / at least Very Positive / at least Overwhelmingly Positive`; sort `title a–z / rating / newest / oldest`; group `none / publisher / studio / bundle month`.

- [ ] **Step 4: Run to verify PASS** — `npx vitest run src/admin/ToolkitBar.test.tsx && npx tsc -b`

- [ ] **Step 5: Commit**

```bash
git add web/src/admin/ToolkitBar.tsx web/src/admin/ToolkitBar.test.tsx
git commit -S -m "web: ToolkitBar — tags/rating/sort/group controls with honest counts"
```

---

### Task 5: web — wire `Catalog.tsx` (URL state, grouped rendering, row readout)

**Files:**
- Modify: `web/src/admin/Catalog.tsx`
- Test: `web/src/admin/Catalog.test.tsx` (extend)

**Interfaces:**
- Consumes: everything from Tasks 3–4.
- Produces: final UI; URL params `q`, `tags` (comma-joined), `rating`, `sort`, `group` (each omitted when at its idle value).

- [ ] **Step 1: Write the failing tests** (extend `Catalog.test.tsx`; fixtures gain steam summaries where needed):
  - URL round-trip: render with `initialEntries={['/admin/catalog?tags=Action&rating=very-positive&sort=date-new&group=publisher&q=zork']}` → the matching controls show those values and the list is filtered accordingly.
  - toggling a tag chip updates the rendered set without reload; clearing filters restores the full list.
  - `group=publisher` renders section headers `Pub House (2)` etc., with `unmapped` last.
  - a row whose game has `review_desc: 'Very Positive'`, `review_percent: 94`, `release_date_iso: '2019-11-12'` shows `Very Positive · 94% · 2019` in its dust-tier readout; a steam-null row shows no readout.
  - existing search tests still pass (search input now writes the `q` param — same behavior through the input).

- [ ] **Step 2: Run to verify FAIL** — `npx vitest run src/admin/Catalog.test.tsx`

- [ ] **Step 3: Implement in `Catalog.tsx`:**
  - Replace `const [search, setSearch] = useState('')` with `useSearchParams()`-derived state:

```ts
const [params, setParams] = useSearchParams();
const toolkit: ToolkitState = useMemo(
  () => ({
    q: params.get('q') ?? '',
    tags: params.get('tags')?.split(',').filter(Boolean) ?? [],
    rating: (params.get('rating') as RatingFloor) ?? 'any',
    sort: (params.get('sort') as SortKey) ?? 'title',
    group: (params.get('group') as GroupKey) ?? 'none',
  }),
  [params],
);
const setToolkit = (next: ToolkitState) => {
  const p = new URLSearchParams();
  if (next.q) p.set('q', next.q);
  if (next.tags.length) p.set('tags', next.tags.join(','));
  if (next.rating !== 'any') p.set('rating', next.rating);
  if (next.sort !== 'title') p.set('sort', next.sort);
  if (next.group !== 'none') p.set('group', next.group);
  setParams(p, { replace: true });
};
```

  (Invalid param values: guard each cast with a membership check against the known keys; fall back to the idle value. Show the guard in code, not just the cast.)
  - Replace the `filtered` memo with `const result = useMemo(() => applyToolkit(games, toolkit), [games, toolkit]);` and `const tagOptions = useMemo(() => collectTagOptions(games), [games]);`
  - Keep the search `<input>` where it is, writing `setToolkit({...toolkit, q: e.target.value})`; render `<ToolkitBar>` on the next line; update the `summary` text to use `result.shown`.
  - Render groups: `result.groups.map(...)` — when `label === null` render rows as today; otherwise wrap each group in `<details open>` with `<summary className="cursor-pointer text-sm font-medium text-ink-soft">{label} ({group.games.length})</summary>`. The row renderer is the existing row JSX extracted into a local `renderRow(game)` function — extract, don't duplicate.
  - Row readout: inside the existing row, after the bundle line, when `g.steam` has any of desc/percent/iso: `<span className="text-xs text-dust">{[g.steam.review_desc, g.steam.review_percent != null ? \`\${g.steam.review_percent}%\` : null, g.steam.release_date_iso?.slice(0, 4)].filter(Boolean).join(' · ')}</span>`.

- [ ] **Step 4: Run the full web suite** — `npx vitest run && npx tsc -b`
Expected: ALL PASS (AdminApp.test, Links.test etc. untouched and green)

- [ ] **Step 5: Commit**

```bash
git add web/src/admin/Catalog.tsx web/src/admin/Catalog.test.tsx
git commit -S -m "web: catalog toolkit wired — URL-persisted filter/sort/group + grouped view"
```

---

### Task 6: gates, docs, PR

**Files:**
- Modify: `docs/superpowers/specs/2026-07-10-catalog-toolkit-design.md` (only if implementation diverged — record reality)
- This plan file: check off completed tasks.

- [ ] **Step 1: Full gates, in order** (fresh moto first: `kill $(pgrep -x moto_server) 2>/dev/null; moto_server -p 8000 &`)

```bash
cd ~/bendobundles && export PATH="$HOME/.cargo/bin:$HOME/.local/node22/bin:$PATH"
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cd web && npm run build && npx vitest run
```

Expected: every gate green. Fix-and-rerun anything red before proceeding (restart moto if rerunning the workspace suite).

- [ ] **Step 2: Push branch + open PR**

```bash
git push -u origin feat/catalog-toolkit
gh pr create -R yourcodekitten/bendobundles --title "admin catalog toolkit: filter, sort, group the trove" --body "<summary: spec link, data-layer join, toolkit UI, test counts, gates run>"
```

(Confirm the correct remote/repo with `git remote -v` first — PRs on this project have gone to `yourcodekitten/bendobundles`.)

- [ ] **Step 3: Watch CI to green** (Monitor tool for long waits), then report ONCE to Ben on Discord with the PR link.
