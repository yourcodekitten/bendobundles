# Genre/Category Allowlist Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop Steam store *features* (Steam Achievements, Family Sharing, Remote Play…) from rendering as genre tags — keep real genres plus an id-allowlist of top-level player-mode categories — and ship a run-once backfill bin that rebuilds the stored cache through the fixed parse.

**Architecture:** One parse-time filter in `steam-client` (the only place `genres[]` and `categories[]` merge), plus a `backfill_steam_genres` function in `fulfillment` mirroring the existing enrichment pass, wrapped by a feature-gated human-run bin. The stored field shape (`genres: Vec<String>`) does not change; dynamo, public-api, and web are untouched.

**Tech Stack:** Rust workspace (serde, tokio, wiremock for HTTP mocks, dynamodb-local/moto-gated integration tests), DynamoDB single-table store, GitHub Actions CI.

**Spec:** `docs/superpowers/specs/2026-07-07-genre-category-allowlist-design.md` (committed on this branch). Tracking issue: yourcodekitten/bendobundles#57.

## Global Constraints

- Every commit is GPG-signed: `git commit -S`. Author must be `code kitten <yourcodekitten@gmail.com>` (verify `git config user.email` before the first commit).
- CI gates are `cargo fmt --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, and `cargo test --workspace` — ALL THREE must pass at every commit. After pasting any code block from this plan, run `cargo fmt` before committing — the blocks are not guaranteed rustfmt-normal.
- Crate convention: matches on `SteamError` name **every** variant — no `_` arm. The compiler's exhaustiveness check is the guard that future variants get a decision.
- TDD: write the failing test, RUN it and see the real failure, then implement. Never write implementation before the red run.
- The fulfillment integration tests are dynamodb-gated: `store_or_skip` skips (prints `SKIP <test>: no dynamodb-local…`) unless a DynamoDB-compatible endpoint is listening on `localhost:8000` (or `DYNAMODB_LOCAL_URL` is set). To get honest reds/greens, start one first, e.g. `pip install --user 'moto[server]' && moto_server -p 8000 &` (or any dynamodb-local). If moto starts erroring `Corrupt("already exists")` across repeated suite runs, restart the moto process — it accumulates state; that error is not a real test failure.
- Working directory: the git worktree at `~/bendobundles-wt/genre-allowlist` (branch `kitten/genre-category-allowlist`).

---

### Task 1: steam-client — filter categories through an id allowlist

**Files:**
- Modify: `crates/steam-client/src/lib.rs` (field doc on `SteamAppDetail.genres`; new `CategoryWire` struct next to `DescriptionWire`; the merge loop inside `get_app_details`)
- Test: `crates/steam-client/tests/client_test.rs` (one new test in the `get_app_details` section)

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces: `SteamAppDetail.genres: Vec<String>` now contains real genres + only allowlisted player-mode categories. Task 2's tests rely on this exact behavior: for a response with `genres: [Indie]` and `categories: [Single-player (id 2)]`, the parsed `genres` is exactly `["Indie", "Single-player"]`.

**Background for a cold implementer:** `get_app_details` in `crates/steam-client/src/lib.rs` fetches `/api/appdetails` and currently builds `genres` by appending **every** `categories[].description` after the real `genres[].description` entries (deduped, order-preserving). Steam categories are mostly store features — noise. Only the five top-level player-mode categories carry signal, and they are identified by Steam's stable numeric category ids: 2 Single-player, 1 Multi-player, 9 Co-op, 49 PvP, 20 MMO. API quirk: `genres[].id` is a JSON **string**, `categories[].id` is a JSON **number** — that's why categories get their own wire struct.

The existing test fixture `crates/steam-client/tests/fixtures/appdetails-413150-trimmed.json` already contains genres `[Indie, RPG, Simulation]` and the full 15-category noise pile (Single-player 2, Multi-player 1, Co-op 9, Online Co-op 38, LAN Co-op 48, Shared/Split Screen Co-op 39, Shared/Split Screen 24, Steam Achievements 22, Full controller support 28, Steam Trading Cards 29, Steam Cloud 23, Remote Play on Phone 41, Remote Play on Tablet 42, Remote Play Together 44, Family Sharing 62). Do NOT edit the fixture — it is a live capture and the point of the test.

- [ ] **Step 1: Write the failing test**

Add to `crates/steam-client/tests/client_test.rs`, directly after the existing `app_details_found_parses_fields` test (find it by name). It reuses the `test_client` helper and `APPDETAILS_FIXTURE` constant already defined in that file:

```rust
#[tokio::test]
async fn app_details_filters_categories_to_player_mode_allowlist() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/api/appdetails"))
        .and(wiremock::matchers::query_param("appids", "413150"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(APPDETAILS_FIXTURE))
        .mount(&server)
        .await;
    let result = test_client(&server).get_app_details(413150).await.unwrap();
    let detail = match result {
        steam_client::AppDetails::Found(d) => d,
        steam_client::AppDetails::Delisted => panic!("expected Found, got Delisted"),
    };
    // Real genres in API order, then ONLY the allowlisted top-level player-mode
    // categories (ids 2, 1, 9) in API order. The fixture's 12 other categories —
    // mode variants (Online Co-op, LAN Co-op, Shared/Split Screen…) and store
    // features (Steam Achievements, Steam Cloud, Family Sharing…) — must be gone.
    assert_eq!(
        detail.genres,
        vec![
            "Indie".to_string(),
            "RPG".to_string(),
            "Simulation".to_string(),
            "Single-player".to_string(),
            "Multi-player".to_string(),
            "Co-op".to_string(),
        ],
        "genres must be real genres + allowlisted player modes only"
    );
}
```

- [ ] **Step 2: Run the test — verify it fails**

Run: `cargo test -p steam-client --test client_test app_details_filters_categories_to_player_mode_allowlist -- --nocapture`
Expected: FAIL — the assertion shows an 18-element list containing `"Steam Achievements"`, `"Family Sharing"`, etc.

- [ ] **Step 3: Implement the filter**

In `crates/steam-client/src/lib.rs`:

3a. Replace the field doc comment on `genres` in `SteamAppDetail` (currently `/// genres + categories descriptions, deduped order-preserving`) with:

```rust
    /// genres + allowlisted player-mode categories (Single-player, Multi-player,
    /// Co-op, PvP, MMO), deduped order-preserving. Store-feature categories
    /// (achievements, cloud, controller…) are filtered out by id at parse time.
```

3b. In `AppDetailDataWire`, change the `categories` field type from `Vec<DescriptionWire>` to `Vec<CategoryWire>`:

```rust
    #[serde(default)]
    categories: Vec<CategoryWire>,
```

3c. Add `CategoryWire` directly below the existing `DescriptionWire` struct, plus the allowlist const:

```rust
/// `categories[].id` is a JSON number (unlike `genres[].id`, a string) and is Steam's
/// stable category identifier — the allowlist keys on it, not on the description text.
/// A missing id deserializes to 0 (allowlisted-nothing) rather than failing the parse.
#[derive(Deserialize)]
struct CategoryWire {
    #[serde(default)]
    id: u32,
    description: String,
}

/// Steam category ids that survive into `SteamAppDetail::genres`: the top-level player
/// modes only. 2 Single-player, 1 Multi-player, 9 Co-op, 49 PvP, 20 MMO. Mode *variants*
/// (Online Co-op 38, LAN Co-op 48, …) are dropped — Steam includes the parent category
/// alongside its variants, so coverage holds while the tag count stays flat (issue #57).
const ALLOWED_CATEGORY_IDS: [u32; 5] = [2, 1, 9, 49, 20];
```

3d. In `get_app_details`, replace the merge loop (currently `for cat in data.categories { if !genres.contains(&cat.description) { genres.push(cat.description); } }`) with:

```rust
        for cat in data.categories {
            if ALLOWED_CATEGORY_IDS.contains(&cat.id) && !genres.contains(&cat.description) {
                genres.push(cat.description);
            }
        }
```

- [ ] **Step 4: Run the steam-client suite — verify green**

Run: `cargo test -p steam-client`
Expected: PASS, including the new test AND the pre-existing `app_details_found_parses_fields` (its `genres.contains("RPG")` assertion still holds).

- [ ] **Step 5: Format + lint**

Run: `cargo fmt` then `cargo fmt --check`
Expected: no diff remains (fmt is a CI gate).

Run: `cargo clippy -p steam-client --all-targets --all-features -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/steam-client/src/lib.rs crates/steam-client/tests/client_test.rs
git commit -S -m "steam-client: filter categories through a player-mode id allowlist

genres[] passes through untouched; categories[] contributes only ids
{2 single-player, 1 multi-player, 9 co-op, 49 pvp, 20 mmo}. store
features (achievements, cloud, family sharing, remote play, controller,
trading cards...) and mode variants no longer render as genre tags (#57)."
```

---

### Task 2: fulfillment — `backfill_steam_genres` + run-once bin

**Files:**
- Modify: `crates/fulfillment/src/lib.rs` (add `BackfillSummary` + `backfill_steam_genres` directly after the `enrich_steam_apps` function)
- Modify: `crates/fulfillment/Cargo.toml` (add `[features] backfill = []` and the `[[bin]]` stanza)
- Create: `crates/fulfillment/src/bin/backfill_genres.rs`
- Test: `crates/fulfillment/tests/handler_test.rs` (new section after the existing enrichment tests)

**Interfaces:**
- Consumes: Task 1's parse behavior — a mocked appdetails response with `genres: [Indie]`, `categories: [Single-player (id 2)]` parses to `genres == ["Indie", "Single-player"]`. Also existing crate items: `dynamo::Store` (`list_all_games`, `get_steam_app`, `put_steam_app`), `dynamo::SteamAppCache` (fields `app_id`, `detail`, `overall`, `recent`, `fetched_at`, `reviews_fetched_at`), `steam_client::{SteamClient, AppDetails, SteamError}`, `fulfillment::STEAM_ENRICH_PACE`.
- Produces: `pub async fn backfill_steam_genres(store: &dynamo::Store, steam: &steam_client::SteamClient, pace: std::time::Duration, skip_fresh_secs: i64) -> Result<BackfillSummary, dynamo::StoreError>` and `pub struct BackfillSummary { pub fetched: u32, pub negative: u32, pub skipped: u32, pub failed: u32, pub aborted_429: bool }`, plus the `backfill_genres` bin. Nothing downstream consumes these — the bin is the terminal user.

**Background for a cold implementer:** The stored `STEAMAPP#<app_id>` cache items were written with the OLD merged genre lists and only self-heal on a 30-day TTL at ≤75 apps per once-daily sync (~10 days for the ~700-app catalog). This task adds a run-once rebuild that refetches appdetails for EVERY catalog appid through the NEW parse and rewrites each item, preserving the reviews half (`overall`, `recent`, `reviews_fetched_at` — appdetails and reviews are independently-clocked halves of the same item). It deliberately IGNORES the 30-day freshness window (refetching regardless is the point) but skips items whose `fetched_at` is within `skip_fresh_secs` (default 12 h) so an aborted run resumes where it left off. Mirror `enrich_steam_apps` (the function directly above where you'll add this) for pacing, error arms, and Delisted semantics. The function takes `Store`/`SteamClient` directly — NOT `Deps` — because the bin can't (and shouldn't) construct the humble/webhook/session baggage `Deps` carries.

The test file already has every helper you need: `store_or_skip` (dynamodb-gated store), `seed_steam_game` (writes a `Game` with a `steam_app_id`), `appdetails_found_body(name)` (wiremock JSON: genres `[Indie]`, categories `[Single-player id 2]`), `appdetails_delisted_body()`, `fresh_cache(app_id, now)` (a fully-populated cache item with both clocks at `now`), `days_ago(n)`, and `steam_mock_empty()` (a MockServer with nothing mounted, so any storefront call is a countable miss). Tests construct a `SteamClient` pointed at a wiremock server exactly like this: `steam_client::SteamClient::new(&server.uri(), &server.uri(), &server.uri(), steam_client::SteamApiKey::new("TESTKEY".into())).unwrap()`.

- [ ] **Step 1: Write the failing tests**

Add a new section at the end of `crates/fulfillment/tests/handler_test.rs` (after the existing enrichment tests). Add `backfill_steam_genres` (only — the tests never name `BackfillSummary`, and an unused import fails the `-D warnings` gate) to the existing `use fulfillment::{...}` import at the top of the file, inserted in sorted position: directly BEFORE `enrich_steam_apps` (rustfmt sorts brace lists; appending at the end fails the fmt gate).

After pasting the block below, run `cargo fmt` — some assert lines are longer than rustfmt-normal form and it will resplit them.

```rust
// =================================================================================================
// backfill_steam_genres (issue #57): run-once STEAMAPP# rebuild through the current parse.
// =================================================================================================

/// A wiremock-backed SteamClient for driving backfill directly (no Deps involved).
fn backfill_steam_client(server: &wiremock::MockServer) -> steam_client::SteamClient {
    steam_client::SteamClient::new(
        &server.uri(),
        &server.uri(),
        &server.uri(),
        steam_client::SteamApiKey::new("TESTKEY".into()),
    )
    .unwrap()
}

// (a) A stale-but-within-30d dirty item is refetched (enrichment would have skipped it),
//     genres come back clean, and the reviews half is preserved byte-for-byte.
#[tokio::test]
async fn backfill_rewrites_dirty_detail_and_preserves_reviews() {
    let Some(store) = store_or_skip("t-bf-rewrite").await else {
        return;
    };
    let now = OffsetDateTime::now_utc().unix_timestamp();
    seed_steam_game(&store, "gk-bf1", "mn-bf1", "Dirty Game", Some(413150), None).await;
    // Seed: 1-day-old detail (fresh by the 30d TTL — enrichment would skip it) with the
    // old merged tag soup; a distinctive reviews half that must survive untouched.
    let mut dirty = fresh_cache(413150, now);
    dirty.fetched_at = days_ago(1);
    dirty.reviews_fetched_at = 777_777;
    dirty.detail.as_mut().unwrap().genres = vec![
        "Indie".into(),
        "Single-player".into(),
        "Steam Achievements".into(),
        "Family Sharing".into(),
    ];
    store.put_steam_app(&dirty).await.unwrap();

    let steam_mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/appdetails"))
        .and(query_param("appids", "413150"))
        .respond_with(ResponseTemplate::new(200).set_body_json(appdetails_found_body("Dirty Game")))
        .mount(&steam_mock)
        .await;

    let steam = backfill_steam_client(&steam_mock);
    let summary = backfill_steam_genres(&store, &steam, std::time::Duration::ZERO, 43_200)
        .await
        .unwrap();

    assert_eq!(summary.fetched, 1, "one item rewritten");
    assert_eq!(summary.skipped, 0);
    let cache = store.get_steam_app(413150).await.unwrap().unwrap();
    let detail = cache.detail.expect("detail must be present");
    assert_eq!(
        detail.genres,
        vec!["Indie".to_string(), "Single-player".to_string()],
        "genres must be rebuilt through the new parse (allowlisted only)"
    );
    assert!(cache.fetched_at >= now, "fetched_at must be restamped");
    // Reviews half preserved exactly as seeded.
    assert_eq!(cache.reviews_fetched_at, 777_777, "reviews clock untouched");
    assert_eq!(cache.overall, dirty.overall, "overall reviews untouched");
    assert_eq!(cache.recent, dirty.recent, "recent reviews untouched");
    // Exactly ONE storefront call: appdetails. No reviews/histogram refetch.
    let reqs = steam_mock.received_requests().await.unwrap();
    assert_eq!(reqs.len(), 1, "backfill must touch appdetails only");
}

// (b) Items fetched within the skip-fresh window are skipped — zero storefront calls.
//     This is the resume mechanism after an aborted run.
#[tokio::test]
async fn backfill_skips_items_within_skip_fresh_window() {
    let Some(store) = store_or_skip("t-bf-skip").await else {
        return;
    };
    let now = OffsetDateTime::now_utc().unix_timestamp();
    seed_steam_game(&store, "gk-bf2", "mn-bf2", "Fresh Game", Some(570), None).await;
    store.put_steam_app(&fresh_cache(570, now)).await.unwrap();

    let steam_mock = steam_mock_empty().await;
    let steam = backfill_steam_client(&steam_mock);
    let summary = backfill_steam_genres(&store, &steam, std::time::Duration::ZERO, 43_200)
        .await
        .unwrap();

    assert_eq!(summary.skipped, 1);
    assert_eq!(summary.fetched, 0);
    let reqs = steam_mock.received_requests().await.unwrap();
    assert_eq!(reqs.len(), 0, "fresh item must trigger zero storefront calls");
}

// (c) A game with an appid but NO cache item yet gets fetched and written.
#[tokio::test]
async fn backfill_fetches_missing_cache_items() {
    let Some(store) = store_or_skip("t-bf-missing").await else {
        return;
    };
    seed_steam_game(&store, "gk-bf3", "mn-bf3", "New Game", Some(730), None).await;

    let steam_mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/appdetails"))
        .and(query_param("appids", "730"))
        .respond_with(ResponseTemplate::new(200).set_body_json(appdetails_found_body("New Game")))
        .mount(&steam_mock)
        .await;

    let steam = backfill_steam_client(&steam_mock);
    let summary = backfill_steam_genres(&store, &steam, std::time::Duration::ZERO, 43_200)
        .await
        .unwrap();

    assert_eq!(summary.fetched, 1);
    let cache = store.get_steam_app(730).await.unwrap().unwrap();
    assert_eq!(
        cache.detail.unwrap().genres,
        vec!["Indie".to_string(), "Single-player".to_string()]
    );
    assert_eq!(cache.reviews_fetched_at, 0, "no reviews were fetched — clock stays 0");
}

// (d) Delisted → negative stub with BOTH clocks stamped (mirrors enrichment semantics).
#[tokio::test]
async fn backfill_delisted_writes_negative_stub() {
    let Some(store) = store_or_skip("t-bf-delisted").await else {
        return;
    };
    let now = OffsetDateTime::now_utc().unix_timestamp();
    seed_steam_game(&store, "gk-bf4", "mn-bf4", "Dead Game", Some(999), None).await;
    let mut dirty = fresh_cache(999, now);
    dirty.fetched_at = days_ago(1);
    store.put_steam_app(&dirty).await.unwrap();

    let steam_mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/appdetails"))
        .and(query_param("appids", "999"))
        .respond_with(ResponseTemplate::new(200).set_body_json(appdetails_delisted_body()))
        .mount(&steam_mock)
        .await;

    let steam = backfill_steam_client(&steam_mock);
    let summary = backfill_steam_genres(&store, &steam, std::time::Duration::ZERO, 43_200)
        .await
        .unwrap();

    assert_eq!(summary.negative, 1);
    assert_eq!(summary.fetched, 0);
    let cache = store.get_steam_app(999).await.unwrap().unwrap();
    assert!(cache.detail.is_none(), "delisted → negative stub");
    assert!(cache.fetched_at >= now && cache.reviews_fetched_at >= now, "both clocks stamped");
}

// (e) A 429 aborts the run (persisted progress survives; rerun resumes via skip-fresh).
#[tokio::test]
async fn backfill_429_aborts_with_flag() {
    let Some(store) = store_or_skip("t-bf-429").await else {
        return;
    };
    seed_steam_game(&store, "gk-bf5", "mn-bf5", "Limited Game", Some(440), None).await;

    let steam_mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/appdetails"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&steam_mock)
        .await;

    let steam = backfill_steam_client(&steam_mock);
    let summary = backfill_steam_genres(&store, &steam, std::time::Duration::ZERO, 43_200)
        .await
        .unwrap();

    assert!(summary.aborted_429, "429 must abort the run");
    assert_eq!(summary.fetched, 0);
}
```

- [ ] **Step 2: Run the tests — verify they fail to compile (red)**

Run: `cargo test -p fulfillment --test handler_test backfill -- --nocapture`
Expected: COMPILE ERROR — unresolved import: `backfill_steam_genres` does not exist in `fulfillment` yet. A compile-error red is the honest red here; note it and move on.

- [ ] **Step 3: Implement `backfill_steam_genres`**

In `crates/fulfillment/src/lib.rs`, directly AFTER the closing brace of `enrich_steam_apps` (find `pub async fn enrich_steam_apps`), add:

```rust
/// Outcome of one [`backfill_steam_genres`] run.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct BackfillSummary {
    /// Live detail refetched and persisted.
    pub fetched: u32,
    /// Delisted stubs written.
    pub negative: u32,
    /// Skipped: `fetched_at` within the skip-fresh window (resume support).
    pub skipped: u32,
    /// Per-app failures (store read/write or storefront error) — logged, app skipped.
    pub failed: u32,
    /// True when a 429 aborted the run early. Rerun to resume; persisted progress survives.
    pub aborted_429: bool,
}

/// Run-once STEAMAPP# rebuild (issue #57): refetch appdetails for EVERY catalog appid through
/// the current parse (id-allowlisted genres) and rewrite each item, preserving the reviews half
/// (`overall`, `recent`, `reviews_fetched_at`). Unlike [`enrich_steam_apps`] this ignores the
/// 30-day freshness window — refetching regardless is the point — but skips items whose
/// `fetched_at` is within `skip_fresh_secs`, so an aborted run resumes where it left off.
///
/// Takes `Store`/`SteamClient` directly rather than [`Deps`]: the caller is the feature-gated
/// `backfill_genres` bin (human-run, never the lambda), which has no humble/webhook/session to
/// carry. Paced like the enrichment pass; a 429 aborts with `aborted_429` set.
pub async fn backfill_steam_genres(
    store: &dynamo::Store,
    steam: &steam_client::SteamClient,
    pace: std::time::Duration,
    skip_fresh_secs: i64,
) -> Result<BackfillSummary, dynamo::StoreError> {
    let games = store.list_all_games().await?;
    // Distinct appids, ascending — deterministic order so an aborted run resumes predictably.
    let appids: std::collections::BTreeSet<u32> =
        games.iter().filter_map(|g| g.steam_app_id).collect();
    let total = appids.len();
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let mut summary = BackfillSummary::default();
    for app_id in appids {
        let existing = match store.get_steam_app(app_id).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(app_id, error = ?e, "backfill: get_steam_app failed — skipping app");
                summary.failed += 1;
                continue;
            }
        };
        if let Some(c) = &existing {
            if now - c.fetched_at < skip_fresh_secs {
                summary.skipped += 1;
                tracing::debug!(app_id, "backfill: fetched recently — skipped (resume window)");
                continue;
            }
        }
        let mut cache = existing.unwrap_or(dynamo::SteamAppCache {
            app_id,
            detail: None,
            overall: None,
            recent: None,
            fetched_at: 0,
            reviews_fetched_at: 0,
        });
        let mut delisted = false;
        tokio::time::sleep(pace).await;
        match steam.get_app_details(app_id).await {
            Ok(steam_client::AppDetails::Found(d)) => {
                cache.detail = Some(*d);
                cache.fetched_at = now;
            }
            // Delisted: negative stub, BOTH clocks stamped — same semantics as enrichment.
            Ok(steam_client::AppDetails::Delisted) => {
                cache.detail = None;
                cache.fetched_at = now;
                cache.reviews_fetched_at = now;
                delisted = true;
            }
            Err(steam_client::SteamError::RateLimited) => {
                summary.aborted_429 = true;
                break;
            }
            Err(
                e @ (steam_client::SteamError::Api(_)
                | steam_client::SteamError::Network(_)
                | steam_client::SteamError::Parse(_)
                | steam_client::SteamError::KeyRejected
                | steam_client::SteamError::NotFound
                | steam_client::SteamError::OpenIdRejected(_)),
            ) => {
                tracing::warn!(app_id, error = ?e, "backfill: appdetails failed — skipping app");
                summary.failed += 1;
                continue;
            }
        }
        if let Err(e) = store.put_steam_app(&cache).await {
            tracing::warn!(app_id, error = ?e, "backfill: put_steam_app failed — this app not persisted");
            summary.failed += 1;
            continue;
        }
        if delisted {
            summary.negative += 1;
        } else {
            summary.fetched += 1;
        }
        let done = summary.fetched + summary.negative + summary.skipped + summary.failed;
        tracing::info!(app_id, done, total, "backfill: item rewritten");
    }
    tracing::info!(
        fetched = summary.fetched,
        negative = summary.negative,
        skipped = summary.skipped,
        failed = summary.failed,
        aborted_429 = summary.aborted_429,
        "backfill: done"
    );
    Ok(summary)
}
```

- [ ] **Step 4: Run the backfill tests — verify green**

(Ensure dynamodb-local/moto is listening on `localhost:8000` first — see Global Constraints; without it these tests silently SKIP, which is NOT a green.)

Run: `cargo test -p fulfillment --test handler_test backfill -- --nocapture`
Expected: 5 passed, output shows no `SKIP` lines.

- [ ] **Step 5: Add the feature-gated bin**

5a. In `crates/fulfillment/Cargo.toml`, append (mirroring humble-client's `probe` pattern; the feature is an empty build-gate — every dep the bin needs is already a normal dependency):

```toml
[features]
# Build-gate for the run-once backfill bin (see src/bin/backfill_genres.rs). Empty on
# purpose: the bin's deps are all normal deps; the gate just keeps it out of default builds.
backfill = []

[[bin]]
name = "backfill_genres"
required-features = ["backfill"]
```

5b. Create `crates/fulfillment/src/bin/backfill_genres.rs`:

```rust
//! Run-once STEAMAPP# cache rebuild (issue #57): refetches appdetails for every catalog
//! appid through the current parse (id-allowlisted genres) and rewrites each item,
//! preserving the reviews half. Run by a human with AWS credentials, never by CI or the
//! lambda:
//!
//!   TABLE_NAME=<table> cargo run -p fulfillment --features backfill --bin backfill_genres
//!
//! Optional env: SKIP_FRESH_SECS (default 43200 = 12h) — items whose appdetails were
//! fetched more recently than this are skipped, which is what makes a rerun after a 429
//! abort resume where it left off.
//!
//! Paced at STEAM_ENRICH_PACE (1.5s/app): the ~700-app catalog takes ~18 minutes.
use dynamo::Store;
use steam_client::{SteamApiKey, SteamClient};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().init();
    let table = std::env::var("TABLE_NAME").expect("TABLE_NAME required");
    let skip_fresh_secs: i64 = std::env::var("SKIP_FRESH_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(43_200);
    let aws_cfg = aws_config::load_from_env().await;
    let store = Store::new(aws_sdk_dynamodb::Client::new(&aws_cfg), table);
    // The appdetails storefront endpoint is keyless; no web-API call is made, so an
    // empty key is fine here.
    let steam = SteamClient::new(
        "https://api.steampowered.com",
        "https://store.steampowered.com",
        "https://steamcommunity.com",
        SteamApiKey::new(String::new()),
    )
    .expect("SteamClient construction");
    let summary = fulfillment::backfill_steam_genres(
        &store,
        &steam,
        fulfillment::STEAM_ENRICH_PACE,
        skip_fresh_secs,
    )
    .await
    .expect("backfill: list_all_games failed");
    println!(
        "backfill: fetched={} negative={} skipped={} failed={} aborted_429={}",
        summary.fetched, summary.negative, summary.skipped, summary.failed, summary.aborted_429
    );
    if summary.aborted_429 {
        eprintln!("rate-limited — rerun to resume (items already rewritten are skipped)");
        std::process::exit(2);
    }
}
```

- [ ] **Step 6: Verify the bin builds and the workspace is green**

Run: `cargo fmt` then `cargo fmt --check`
Expected: no diff remains (fmt is a CI gate).

Run: `cargo build -p fulfillment --features backfill --bin backfill_genres`
Expected: builds clean.

Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
Expected: clean (this is the CI gate and it DOES lint the feature-gated bin).

Run: `cargo test --workspace`
Expected: all green (with dynamodb-local up, no `SKIP` lines in the fulfillment suite).

- [ ] **Step 7: Commit**

```bash
git add crates/fulfillment/src/lib.rs crates/fulfillment/src/bin/backfill_genres.rs crates/fulfillment/Cargo.toml crates/fulfillment/tests/handler_test.rs
git commit -S -m "fulfillment: backfill_steam_genres + run-once backfill_genres bin

refetches every catalog appid through the id-allowlisted parse and
rewrites the STEAMAPP# item, preserving the reviews half. ignores the
30d freshness window (rebuilding is the point) but skips items fetched
within SKIP_FRESH_SECS (12h default) so an aborted run resumes. paced
at STEAM_ENRICH_PACE; 429 aborts with progress persisted (#57)."
```

---

## Post-merge rollout (NOT part of plan execution — the operator's runbook)

1. PR → CI green → review → squash-merge (closes #57).
2. Deploy the fulfillment lambda from the merge commit's CI `lambda-zips` artifact (`gh run download` — the #55 runbook; tfvars must carry `lambda_permissions_boundary_arn`, expect exactly 1 plan change per lambda: `source_code_hash`). No web deploy — web is untouched.
3. From the box: `TABLE_NAME=<prod table> cargo run -p fulfillment --features backfill --bin backfill_genres` with deploy AWS creds. Expect `fetched≈700`, ~18 min.
4. Verify: public API detail for 413150 shows exactly `Indie, RPG, Simulation, Single-player, Multi-player, Co-op`; no store-feature strings in any genre list; spot-check a reviews block is unchanged.
