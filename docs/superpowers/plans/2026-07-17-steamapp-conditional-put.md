# STEAMAPP# Conditional Put Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the STEAMAPP# cache lost-update race by construction: a monotonic `version` attribute + guarded `PutItem`, with a re-merge-once retry in both writers (issue #75).

**Architecture:** The dynamo `Store` gains an opaque optimistic-lock token (`SteamAppVersion`, obtainable only from a new `get_steam_app_versioned` read) and `put_steam_app` becomes guarded — compile-breaking, no unguarded variant. The fulfillment crate gains one shared write policy (`persist_fetched_halves`): guarded put → on `LostRace` re-read, re-merge newest-wins-per-half (pure fn `merge_fetched_halves`), retry once, then yield. Both writers (enrichment, backfill) route through it and expose a `lost_race` counter.

**Tech Stack:** Rust workspace; `aws-sdk-dynamodb` conditional PutItem; dynamodb-local integration tests (`store_or_skip` harness, skips when no local dynamo; CI runs `amazon/dynamodb-local:2.5.2` per `.github/workflows/ci.yml`).

**Spec:** `docs/superpowers/specs/2026-07-17-steamapp-conditional-put-design.md` — read it first; the invariant sentence ("version and body travel together") and the guard semantics are normative.

## Global Constraints

- NO unguarded STEAMAPP# write may survive anywhere (prod or test) — `put_steam_app` takes a mandatory guard; there is no escape-hatch variant.
- `SteamAppVersion`'s inner field stays private — a token can only come from `Store::get_steam_app_versioned`.
- `SteamAppCache` (the struct) and the `body` wire format are UNCHANGED — `version` is a top-level item attribute only.
- Readers (`get_steam_app`, `batch_get_steam_apps`) are behavior-unchanged.
- Counters: `fresh`/`negative` keep their fetch-time increment sites (pre-existing semantics); `fetched` counts successful persists; `lost_race` counts detected races (a twice-lost app also counts in the pass's failed/skip accounting, never in `fetched`).
- Run integration tests with dynamodb-local: `docker run -d --rm -p 8000:8000 amazon/dynamodb-local:2.5.2` (or rely on skip behavior; NEVER set `DYNAMODB_LOCAL_URL` without a live endpoint — the harness panics to avoid forging green).
- All commits GPG-signed (`git commit -S`), authored `code kitten <yourcodekitten@gmail.com>`.

---

### Task 1: dynamo store — token, guard, error, versioned read, guarded put

**Files:**
- Modify: `crates/dynamo/src/lib.rs` (types near `SteamAppCache` ~line 141; put/get near lines 1995–2015)
- Modify: `crates/dynamo/src/schema.rs:213-227` (`steam_app_item`)
- Test: `crates/dynamo/tests/store_test.rs` (new tests + ~7 mechanical call-site updates)

**Interfaces:**
- Consumes: existing `is_ccf_put` (lib.rs ~227), `schema::key_pair`, `parse_body`, `steam_app_pk`, test helpers `store_or_skip` / `raw_client` / `steam_app_cache_full` / `steam_app_cache_stub`.
- Produces (later tasks rely on these exact shapes):
  - `pub struct SteamAppVersion(Option<i64>)` — private field, `Debug, Clone, Copy, PartialEq, Eq`.
  - `pub enum SteamAppPutGuard { Absent, Unchanged(SteamAppVersion) }` — `Debug, Clone, Copy, PartialEq, Eq`.
  - `pub enum SteamAppPutError { LostRace, Store(StoreError) }` (thiserror; `#[from] StoreError`).
  - `pub async fn get_steam_app_versioned(&self, app_id: u32) -> Result<Option<(SteamAppCache, SteamAppVersion)>, StoreError>`
  - `pub async fn put_steam_app(&self, cache: &SteamAppCache, guard: SteamAppPutGuard) -> Result<(), SteamAppPutError>`
  - `steam_app_item(cache: &SteamAppCache, version: i64) -> HashMap<String, AttributeValue>`

- [ ] **Step 1: Write the failing tests** — add to `crates/dynamo/tests/store_test.rs` (import `SteamAppPutError, SteamAppPutGuard` in the existing `use dynamo::{...}` list; check `serde_json` is in dynamo's `[dev-dependencies]`, add if missing):

```rust
/// #75: Absent guard — create-only. Writes version=1; a second Absent put is a
/// detected race, not a silent overwrite.
#[tokio::test]
async fn put_steam_app_absent_guard() {
    let Some(store) = store_or_skip("steamapp-absent-guard").await else {
        return;
    };
    let full = steam_app_cache_full(570);
    store
        .put_steam_app(&full, SteamAppPutGuard::Absent)
        .await
        .unwrap();

    // enforcement attr present alongside body, at 1
    let raw = raw_client("steamapp-absent-guard").await;
    let item = raw
        .get_item()
        .table_name("t-steamapp-absent-guard")
        .key("pk", AttributeValue::S("STEAMAPP#570".into()))
        .key("sk", AttributeValue::S("META".into()))
        .send()
        .await
        .unwrap()
        .item
        .unwrap();
    assert_eq!(item.get("version").unwrap().as_n().unwrap(), "1");

    let err = store
        .put_steam_app(&full, SteamAppPutGuard::Absent)
        .await
        .unwrap_err();
    assert!(matches!(err, SteamAppPutError::LostRace));
}

/// #75: Unchanged guard — the current token writes and moves the version; a stale
/// token is a detected race; the fresh token works again.
#[tokio::test]
async fn put_steam_app_unchanged_guard() {
    let Some(store) = store_or_skip("steamapp-unchanged-guard").await else {
        return;
    };
    store
        .put_steam_app(&steam_app_cache_stub(570), SteamAppPutGuard::Absent)
        .await
        .unwrap();

    let (cache, v1) = store.get_steam_app_versioned(570).await.unwrap().unwrap();
    assert!(cache.detail.is_none(), "stub round-trips");

    let full = steam_app_cache_full(570);
    store
        .put_steam_app(&full, SteamAppPutGuard::Unchanged(v1))
        .await
        .unwrap();

    let err = store
        .put_steam_app(&full, SteamAppPutGuard::Unchanged(v1))
        .await
        .unwrap_err();
    assert!(matches!(err, SteamAppPutError::LostRace), "stale token loses");

    let (read_back, v2) = store.get_steam_app_versioned(570).await.unwrap().unwrap();
    assert!(read_back.detail.is_some(), "guarded write landed");
    assert_ne!(v1, v2, "token moved");
    store
        .put_steam_app(&read_back, SteamAppPutGuard::Unchanged(v2))
        .await
        .unwrap();
}

/// #75: legacy items ({pk, sk, body} written before the version attr) are adopted
/// by the migration arm — and fully guarded from then on.
#[tokio::test]
async fn put_steam_app_legacy_migration() {
    let Some(store) = store_or_skip("steamapp-legacy").await else {
        return;
    };
    // Pre-#75 item shape — impossible via the Store API, hence the raw client.
    let raw = raw_client("steamapp-legacy").await;
    let legacy = steam_app_cache_full(570);
    raw.put_item()
        .table_name("t-steamapp-legacy")
        .item("pk", AttributeValue::S("STEAMAPP#570".into()))
        .item("sk", AttributeValue::S("META".into()))
        .item(
            "body",
            AttributeValue::S(serde_json::to_string(&legacy).unwrap()),
        )
        .send()
        .await
        .unwrap();

    let (cache, legacy_token) = store.get_steam_app_versioned(570).await.unwrap().unwrap();
    assert_eq!(cache.app_id, 570, "legacy body parses");

    // migration arm accepts exactly once…
    store
        .put_steam_app(&cache, SteamAppPutGuard::Unchanged(legacy_token))
        .await
        .unwrap();

    // …then the item is versioned and the legacy token is dead.
    let err = store
        .put_steam_app(&cache, SteamAppPutGuard::Unchanged(legacy_token))
        .await
        .unwrap_err();
    assert!(matches!(err, SteamAppPutError::LostRace));
    let (_, v) = store.get_steam_app_versioned(570).await.unwrap().unwrap();
    store
        .put_steam_app(&cache, SteamAppPutGuard::Unchanged(v))
        .await
        .unwrap();
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p dynamo --test store_test steamapp` → compile error (`put_steam_app` takes 1 arg; `SteamAppPutGuard` not found). That IS the failing state for a signature change.

- [ ] **Step 3: Implement the dynamo side.**

In `crates/dynamo/src/lib.rs`, directly under the `SteamAppCache` impl (~line 167):

```rust
/// Opaque optimistic-lock token for a STEAMAPP# item — obtainable ONLY from
/// [`Store::get_steam_app_versioned`] (private field: a guard value cannot be
/// fabricated, it must come from a read). `None` inside = legacy item written
/// before the `version` attribute existed; the guarded put adopts it (#75).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SteamAppVersion(Option<i64>);

/// The caller's precondition for [`Store::put_steam_app`] — what the read that
/// seeded this write saw. Deliberately no unguarded variant: an unconditional
/// escape hatch is the next silent lost-update waiting for a caller (#75).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SteamAppPutGuard {
    /// The read returned `None`: create-only.
    Absent,
    /// The read returned an item carrying this token: write only if it still does.
    Unchanged(SteamAppVersion),
}

/// Errors from the guarded [`Store::put_steam_app`].
#[derive(Debug, thiserror::Error)]
pub enum SteamAppPutError {
    /// The item changed between the caller's read and this put — a concurrent
    /// writer won. Nothing is wrong with the payload; re-read, re-merge, retry.
    /// (`ClaimTxError::TxConflict` precedent: a timing race is not an AWS error.)
    #[error("STEAMAPP# item changed since read — lost the race, safe to re-merge")]
    LostRace,
    #[error(transparent)]
    Store(#[from] StoreError),
}
```

Replace `put_steam_app` and add `get_steam_app_versioned` next to `get_steam_app` (~line 1995):

```rust
    /// Write (or refresh) a Steam app enrichment cache entry — guarded (#75).
    /// pk="STEAMAPP#<app_id>", sk="META", body=JSON of [`SteamAppCache`],
    /// version=N monotonic counter. Succeeds only if the item still matches the
    /// read in `guard`; otherwise [`SteamAppPutError::LostRace`] — re-read via
    /// [`Store::get_steam_app_versioned`], re-merge, retry. `detail: None` is a
    /// valid negative-cache stub.
    pub async fn put_steam_app(
        &self,
        cache: &SteamAppCache,
        guard: SteamAppPutGuard,
    ) -> Result<(), SteamAppPutError> {
        let req = self.client.put_item().table_name(&self.table);
        let req = match guard {
            SteamAppPutGuard::Absent => req
                .set_item(Some(steam_app_item(cache, 1)))
                .condition_expression("attribute_not_exists(pk)"),
            // Legacy item (pre-version): adopt at version 1. Cannot false-pass —
            // any concurrent new-code write stamps `version`, flipping this arm
            // to a CCF. (A vanished item also passes, which is create — correct.)
            SteamAppPutGuard::Unchanged(SteamAppVersion(None)) => req
                .set_item(Some(steam_app_item(cache, 1)))
                .condition_expression("attribute_not_exists(version)"),
            SteamAppPutGuard::Unchanged(SteamAppVersion(Some(v))) => req
                .set_item(Some(steam_app_item(cache, v + 1)))
                .condition_expression("version = :v")
                .expression_attribute_values(
                    ":v",
                    aws_sdk_dynamodb::types::AttributeValue::N(v.to_string()),
                ),
        };
        req.send().await.map_err(|e| {
            if is_ccf_put(&e) {
                SteamAppPutError::LostRace
            } else {
                SteamAppPutError::Store(e.into())
            }
        })?;
        Ok(())
    }

    /// Writer-side read of a STEAMAPP# item: parsed cache + the optimistic-lock
    /// token for a subsequent guarded [`Store::put_steam_app`]. Read-only paths
    /// (admin/public views) keep using [`Store::get_steam_app`].
    pub async fn get_steam_app_versioned(
        &self,
        app_id: u32,
    ) -> Result<Option<(SteamAppCache, SteamAppVersion)>, StoreError> {
        let (pk, sk) = schema::key_pair(&steam_app_pk(app_id), "META");
        let out = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", pk)
            .key("sk", sk)
            .send()
            .await?;
        let Some(item) = out.item else {
            return Ok(None);
        };
        let cache: SteamAppCache = parse_body(&item)?;
        let version = match item.get("version") {
            None => SteamAppVersion(None),
            Some(v) => SteamAppVersion(Some(
                v.as_n()
                    .ok()
                    .and_then(|s| s.parse::<i64>().ok())
                    .ok_or(StoreError::Corrupt("bad version attr"))?,
            )),
        };
        Ok(Some((cache, version)))
    }
```

In `crates/dynamo/src/schema.rs`, replace `steam_app_item`:

```rust
/// Build the full item for a STEAMAPP enrichment cache entry.
/// pk="STEAMAPP#<app_id>", sk="META", body=JSON of [`crate::SteamAppCache`],
/// version=N (optimistic-lock counter — `Store::put_steam_app` computes it from
/// the guard).
/// INVARIANT (#75, recorded as a decision): `version` and `body` travel together,
/// always, atomically — this is the only builder of the item shape, the guarded
/// put is the only writer, and no UpdateItem ever touches STEAMAPP# items.
/// Use `Store::put_steam_app` / `Store::get_steam_app` — do not write STEAMAPP#
/// items directly.
pub fn steam_app_item(
    cache: &crate::SteamAppCache,
    version: i64,
) -> std::collections::HashMap<String, AttributeValue> {
    std::collections::HashMap::from([
        ("pk".into(), s(steam_app_pk(cache.app_id))),
        ("sk".into(), s("META")),
        (
            "body".into(),
            s(serde_json::to_string(cache).expect("SteamAppCache serializes")),
        ),
        ("version".into(), AttributeValue::N(version.to_string())),
    ])
}
```

- [ ] **Step 4: Update existing store_test call sites** — the ~7 existing `put_steam_app(&x)` calls (lines ~1180-81, ~2248-49, ~2276, ~2280, ~2298) each first-write a distinct app_id on a fresh per-test table → append `, SteamAppPutGuard::Absent`. Verify each really is a first write for its id before doing so (if any site overwrites an id, read the token back with `get_steam_app_versioned` and use `Unchanged`).

- [ ] **Step 5: Run** — `docker run -d --rm -p 8000:8000 amazon/dynamodb-local:2.5.2` if not already up, then `cargo test -p dynamo` → all pass (new tests + regressions).

- [ ] **Step 6: Commit**

```bash
git add crates/dynamo
git commit -S -m "feat(dynamo): guarded put_steam_app with opaque version token (#75)"
```

---

### Task 2: reader-crate test call sites (admin-api + public-api)

**Files:**
- Modify: `crates/admin-api/tests/api_test.rs:2105,2218,2302`
- Modify: `crates/public-api/tests/api_test.rs:1817,2172,2263,2270`

**Interfaces:**
- Consumes: `SteamAppPutGuard::Absent` from Task 1.
- Produces: nothing new — mechanical compile fix.

- [ ] **Step 1: Update the seven seeding calls** (3 admin-api + 4 public-api; `crates/public-api/tests/api_test.rs:2179` is a comment, not a call) — each seeds a fresh per-test table (verify: distinct app_id, first write for that id in its test) → append `, SteamAppPutGuard::Absent`. Add `SteamAppPutGuard` to each file's `use dynamo::{...}` import. If any site turns out to overwrite an id already written in the same test, read the token back with `get_steam_app_versioned` and use `Unchanged` instead.
- [ ] **Step 2: Run** — `cargo test -p admin-api -p public-api` → pass.
- [ ] **Step 3: Commit**

```bash
git add crates/admin-api crates/public-api
git commit -S -m "test(admin-api,public-api): put_steam_app call sites adopt the Absent guard (#75)"
```

---

### Task 3: fulfillment — FetchedHalves, pure merge policy, persist helper

**Files:**
- Modify: `crates/fulfillment/src/lib.rs` (new section directly above `run_steam_enrichment`; unit tests in the existing `#[cfg(test)]` module at the file tail, ~line 3236)

**Interfaces:**
- Consumes: Task 1's `SteamAppPutGuard`, `SteamAppPutError`, `SteamAppVersion`, `get_steam_app_versioned`; `dynamo::SteamAppCache::empty`; `steam_client::{SteamAppDetail, ReviewSummary, RecentReviews}` (all `Clone`).
- Produces (Tasks 4–6 rely on these exact shapes):
  - `pub struct FetchedHalves { pub now: i64, pub detail: Option<DetailFetch>, pub reviews: Option<(steam_client::ReviewSummary, steam_client::RecentReviews)> }`
  - `pub enum DetailFetch { Live(Box<steam_client::SteamAppDetail>), Delisted }`
  - `pub fn merge_fetched_halves(cache: &mut dynamo::SteamAppCache, ours: &FetchedHalves)`
  - `pub enum PersistResult { Written { cache: dynamo::SteamAppCache, after_race: bool }, LostTwice }`
  - `pub async fn persist_fetched_halves(store: &Store, app_id: u32, snapshot: Option<(dynamo::SteamAppCache, dynamo::SteamAppVersion)>, ours: &FetchedHalves) -> Result<PersistResult, StoreError>`

- [ ] **Step 1: Write the failing unit tests** for the pure merge inside the EXISTING `#[cfg(test)] mod tests { use super::*; … }` at the tail of `crates/fulfillment/src/lib.rs` (verified at ~line 3257 — `use super::*;` is already there, so the new pub items resolve without imports). Add the three local builders below and the tests:

```rust
    fn halves(now: i64) -> FetchedHalves {
        FetchedHalves {
            now,
            detail: None,
            reviews: None,
        }
    }

    /// #75 merge policy: our fresh detail applies over a staler snapshot half; the
    /// snapshot's untouched reviews half survives.
    #[test]
    fn merge_ours_newer_detail_applies() {
        let mut cache = dynamo::SteamAppCache::empty(570);
        cache.fetched_at = 100;
        cache.reviews_fetched_at = 900;
        let ours = FetchedHalves {
            detail: Some(DetailFetch::Live(Box::new(test_detail(570)))),
            ..halves(500)
        };
        merge_fetched_halves(&mut cache, &ours);
        assert!(cache.detail.is_some());
        assert_eq!(cache.fetched_at, 500);
        assert_eq!(cache.reviews_fetched_at, 900, "reviews half untouched");
    }

    /// #75 merge policy: a snapshot half NEWER than ours survives — the concurrent
    /// writer's fresher fetch wins, ours is dropped (correct, not a loss).
    #[test]
    fn merge_theirs_newer_detail_survives() {
        let mut cache = dynamo::SteamAppCache::empty(570);
        cache.detail = Some(test_detail(570));
        cache.fetched_at = 800;
        let ours = FetchedHalves {
            detail: Some(DetailFetch::Delisted),
            ..halves(500)
        };
        merge_fetched_halves(&mut cache, &ours);
        assert!(cache.detail.is_some(), "their live detail survives our stale stub");
        assert_eq!(cache.fetched_at, 800);
        assert_eq!(cache.reviews_fetched_at, 0, "delisted reviews stamp only applies with the detail half");
    }

    /// #75 merge policy, mirror direction: a NEWER concurrent Delisted verdict is
    /// not resurrected by our stale Live detail — the dead app stays dead.
    #[test]
    fn merge_theirs_newer_delisted_not_resurrected() {
        let mut cache = dynamo::SteamAppCache::empty(570);
        cache.detail = None; // concurrent writer's delisted stub…
        cache.fetched_at = 800; // …stamped fresher than our fetch
        cache.reviews_fetched_at = 800;
        let ours = FetchedHalves {
            detail: Some(DetailFetch::Live(Box::new(test_detail(570)))),
            ..halves(500)
        };
        merge_fetched_halves(&mut cache, &ours);
        assert!(cache.detail.is_none(), "our stale Live must not resurrect their newer Delisted");
        assert_eq!(cache.fetched_at, 800);
    }

    /// #75 merge policy: equal stamps go to us — we hold data fetched moments ago.
    #[test]
    fn merge_equal_stamp_ours_wins() {
        let mut cache = dynamo::SteamAppCache::empty(570);
        cache.fetched_at = 500;
        let ours = FetchedHalves {
            detail: Some(DetailFetch::Live(Box::new(test_detail(570)))),
            ..halves(500)
        };
        merge_fetched_halves(&mut cache, &ours);
        assert!(cache.detail.is_some());
    }

    /// #75 merge policy: delisted stamps BOTH clocks (dead apps skip review fetches
    /// for the whole window) but never regresses a fresher concurrent reviews stamp.
    #[test]
    fn merge_delisted_stamps_both_clocks_forward_only() {
        let mut cache = dynamo::SteamAppCache::empty(570);
        cache.fetched_at = 100;
        cache.reviews_fetched_at = 100;
        let ours = FetchedHalves {
            detail: Some(DetailFetch::Delisted),
            ..halves(500)
        };
        merge_fetched_halves(&mut cache, &ours);
        assert!(cache.detail.is_none());
        assert_eq!(cache.fetched_at, 500);
        assert_eq!(cache.reviews_fetched_at, 500);

        let mut cache2 = dynamo::SteamAppCache::empty(571);
        cache2.fetched_at = 100;
        cache2.reviews_fetched_at = 800; // concurrent writer's fresher reviews
        merge_fetched_halves(&mut cache2, &FetchedHalves {
            detail: Some(DetailFetch::Delisted),
            ..halves(500)
        });
        assert_eq!(cache2.reviews_fetched_at, 800, "never stamps backward");
    }

    /// #75 merge policy: the reviews half applies independently of the detail half.
    #[test]
    fn merge_reviews_half_independent() {
        let mut cache = dynamo::SteamAppCache::empty(570);
        cache.detail = Some(test_detail(570));
        cache.fetched_at = 800;
        cache.reviews_fetched_at = 100;
        let ours = FetchedHalves {
            reviews: Some((test_review_summary(), test_recent_reviews())),
            ..halves(500)
        };
        merge_fetched_halves(&mut cache, &ours);
        assert!(cache.overall.is_some());
        assert!(cache.recent.is_some());
        assert_eq!(cache.reviews_fetched_at, 500);
        assert_eq!(cache.fetched_at, 800, "detail half untouched");
    }
```

Local builders for the module (exact code — all fields pub):

```rust
    fn test_detail(app_id: u32) -> steam_client::SteamAppDetail {
        steam_client::SteamAppDetail {
            app_id,
            name: "T".into(),
            developers: vec![],
            publishers: vec![],
            genres: vec![],
            release_date: None,
            short_description: "t".into(),
            header_image: None,
            video_hls_url: None,
            video_thumbnail: None,
            screenshots: vec![],
            tags: vec![],
            content_descriptor_ids: vec![],
            content_notes: None,
        }
    }

    fn test_review_summary() -> steam_client::ReviewSummary {
        steam_client::ReviewSummary {
            desc: "Positive".into(),
            total_positive: 10,
            total_negative: 1,
            total_reviews: 11,
        }
    }

    fn test_recent_reviews() -> steam_client::RecentReviews {
        steam_client::RecentReviews {
            percent_positive: 90,
            count: 11,
        }
    }
```

- [ ] **Step 2: Run to verify failure** — `cargo check -p fulfillment --tests 2>&1 | head -40` (`--tests` matters: a plain `cargo check` never compiles `#[cfg(test)]` code, so the RED below would be invisible) → expected errors, ALL of them known: (a) missing types `FetchedHalves`/`DetailFetch`/`merge_fetched_halves` in the test module (the RED for this task), and (b) `E0061: this function takes 2 arguments but 1 argument was supplied` at exactly two prod call sites — the enrichment put (~lib.rs:2268) and the backfill put (~lib.rs:2423). (b) is Task 4/5's work — do NOT fix those sites in this task.

- [ ] **Step 3: Implement** — new section in `crates/fulfillment/src/lib.rs` directly above `run_steam_enrichment`:

```rust
// ── #75: guarded STEAMAPP# persistence ───────────────────────────────────────

/// One pass's freshly fetched Steam halves for a single app — what the writer
/// wants to persist, independent of which snapshot it lands on (#75).
pub struct FetchedHalves {
    /// The pass clock: the `fetched_at`/`reviews_fetched_at` stamp for whichever
    /// halves are present.
    pub now: i64,
    /// Detail half, if fetched this pass.
    pub detail: Option<DetailFetch>,
    /// Reviews half (summary + recent histogram), if fetched this pass.
    pub reviews: Option<(steam_client::ReviewSummary, steam_client::RecentReviews)>,
}

/// Outcome of a detail fetch.
pub enum DetailFetch {
    Live(Box<steam_client::SteamAppDetail>),
    /// Steam says the app no longer exists: negative-cache stub. Stamps BOTH
    /// clocks (a dead app has no reviews to fetch).
    Delisted,
}

/// Newest-wins-per-half merge of this pass's fetched halves onto a store
/// snapshot. Pure — the single definition of the re-merge policy shared by
/// enrichment and backfill, unit-testable without staging a live race (#75).
///
/// Each half applies only if our stamp is >= the snapshot's (ties go to us: we
/// hold data fetched moments ago). A snapshot half NEWER than ours survives —
/// that's the concurrent writer's fresher fetch, not a loss.
pub fn merge_fetched_halves(cache: &mut dynamo::SteamAppCache, ours: &FetchedHalves) {
    match &ours.detail {
        Some(DetailFetch::Live(d)) if ours.now >= cache.fetched_at => {
            cache.detail = Some((**d).clone());
            cache.fetched_at = ours.now;
        }
        Some(DetailFetch::Delisted) if ours.now >= cache.fetched_at => {
            cache.detail = None;
            cache.fetched_at = ours.now;
            // Both clocks, forward-only: never regress a fresher concurrent stamp.
            cache.reviews_fetched_at = cache.reviews_fetched_at.max(ours.now);
        }
        _ => {}
    }
    if let Some((overall, recent)) = &ours.reviews
        && ours.now >= cache.reviews_fetched_at
    {
        cache.overall = Some(overall.clone());
        cache.recent = Some(recent.clone());
        cache.reviews_fetched_at = ours.now;
    }
}

/// Result of [`persist_fetched_halves`].
pub enum PersistResult {
    /// The merged cache below is what's now in the store.
    Written {
        cache: dynamo::SteamAppCache,
        /// True = the first put lost a race and the retry (re-read + re-merge)
        /// landed.
        after_race: bool,
    },
    /// Two consecutive lost races — this app is skipped; the next pass retries.
    LostTwice,
}

fn snapshot_parts(
    app_id: u32,
    snapshot: Option<(dynamo::SteamAppCache, dynamo::SteamAppVersion)>,
) -> (dynamo::SteamAppCache, dynamo::SteamAppPutGuard) {
    match snapshot {
        None => (
            dynamo::SteamAppCache::empty(app_id),
            dynamo::SteamAppPutGuard::Absent,
        ),
        Some((c, v)) => (c, dynamo::SteamAppPutGuard::Unchanged(v)),
    }
}

/// The single home of the #75 write policy: guarded put; on a lost race,
/// re-read, re-merge (newest-wins per half — zero extra Steam calls, the data
/// is in hand), retry exactly once; a second loss yields the pass.
pub async fn persist_fetched_halves(
    store: &Store,
    app_id: u32,
    snapshot: Option<(dynamo::SteamAppCache, dynamo::SteamAppVersion)>,
    ours: &FetchedHalves,
) -> Result<PersistResult, StoreError> {
    let (mut cache, guard) = snapshot_parts(app_id, snapshot);
    merge_fetched_halves(&mut cache, ours);
    match store.put_steam_app(&cache, guard).await {
        Ok(()) => {
            return Ok(PersistResult::Written {
                cache,
                after_race: false,
            });
        }
        Err(dynamo::SteamAppPutError::Store(e)) => return Err(e),
        Err(dynamo::SteamAppPutError::LostRace) => {}
    }
    // Lost the race: someone wrote between our read and our put. Their write is
    // real data — re-read, merge ours onto THEIR item, retry once.
    let fresh = store.get_steam_app_versioned(app_id).await?;
    let (mut cache, guard) = snapshot_parts(app_id, fresh);
    merge_fetched_halves(&mut cache, ours);
    match store.put_steam_app(&cache, guard).await {
        Ok(()) => Ok(PersistResult::Written {
            cache,
            after_race: true,
        }),
        Err(dynamo::SteamAppPutError::Store(e)) => Err(e),
        Err(dynamo::SteamAppPutError::LostRace) => Ok(PersistResult::LostTwice),
    }
}
```

- [ ] **Step 4: Verify scope** — `cargo check -p fulfillment --tests 2>&1 | head -40` → the ONLY remaining errors are the two known `E0061` prod call sites (enrichment ~2268, backfill ~2423); the new types and the test module produce no errors. The merge_ tests first RUN GREEN in Task 5 Step 4, after both call sites are fixed — do not chase a passing test run in this task.
- [ ] **Step 5: Commit**

```bash
git add crates/fulfillment/src/lib.rs
git commit -S -m "feat(fulfillment): shared #75 write policy — pure newest-wins merge + persist-with-one-retry"
```

---

### Task 4: enrichment caller — route through persist_fetched_halves

**Files:**
- Modify: `crates/fulfillment/src/lib.rs` — `run_steam_enrichment`'s fetch loop (~lines 2168–2280) and its summary line; the `Work` struct (~2075) and decide-pass push.

**Interfaces:**
- Consumes: Task 3's `FetchedHalves`, `DetailFetch`, `PersistResult`, `persist_fetched_halves`; Task 1's `get_steam_app_versioned`.
- Produces: enrichment summary line gains `lost_race={lost_race}`.

- [ ] **Step 1: Rewrite the fetch-loop body.** The JIT re-read becomes the versioned read; the match arms fill `ours` instead of mutating a cache copy; the put becomes the persist helper. Exact replacement for the loop body (from the `let Work {...}` destructure to the `supersede_adult` call inclusive):

```rust
        let Work {
            app_id,
            need_detail,
            need_reviews,
        } = work;
        // Just-in-time versioned read: the decide-pass snapshot can be minutes
        // stale by the time this item's turn comes (paced loop). The snapshot +
        // token seed the guarded merge write below — a concurrent writer inside
        // the read→put gap is now DETECTED (LostRace ⇒ re-merge + retry, #75),
        // not silently overwritten. The decide-pass snapshot only classified
        // the work.
        let snapshot = match deps.store.get_steam_app_versioned(app_id).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(app_id, error = ?e, "steam enrichment: re-read failed — skipping app");
                continue 'apps;
            }
        };
        let mut ours = FetchedHalves {
            now,
            detail: None,
            reviews: None,
        };
        let mut delisted = false;

        if need_detail {
            tokio::time::sleep(deps.steam_enrich_pace).await;
            match steam.get_app_details(app_id).await {
                Ok(steam_client::AppDetails::Found(d)) => {
                    let mut detail = *d;
                    detail.tags = tags_for_app(
                        app_id,
                        tag_data.as_ref(),
                        snapshot.as_ref().and_then(|(c, _)| c.detail.as_ref()),
                    );
                    ours.detail = Some(DetailFetch::Live(Box::new(detail)));
                    fresh += 1;
                }
                // Delisted: negative-cache stub. The merge stamps BOTH clocks so
                // it's retried on the 30d window (not every sync); reviews are
                // skipped — a dead app has none.
                Ok(steam_client::AppDetails::Delisted) => {
                    ours.detail = Some(DetailFetch::Delisted);
                    negative += 1;
                    delisted = true;
                }
                Err(steam_client::SteamError::RateLimited) => {
                    aborted_429 = true;
                    break 'apps;
                }
                Err(
                    e @ (steam_client::SteamError::Api(_)
                    | steam_client::SteamError::Network(_)
                    | steam_client::SteamError::Parse(_)
                    | steam_client::SteamError::KeyRejected
                    | steam_client::SteamError::NotFound
                    | steam_client::SteamError::OpenIdRejected(_)),
                ) => {
                    tracing::warn!(app_id, error = ?e, "steam enrichment: appdetails failed — skipping app");
                    continue 'apps;
                }
            }
        }

        if need_reviews && !delisted {
            // Keep the existing two paced fetches (`get_review_summary` then
            // `get_recent_reviews`, each with its sleep + full SteamError match)
            // VERBATIM — then DELETE the old three assignment lines:
            //     cache.overall = Some(overall);
            //     cache.recent = Some(recent);
            //     cache.reviews_fetched_at = now;
            // and replace them with this single line:
            ours.reviews = Some((overall, recent));
        }

        // Merge write per-item: partial progress survives an abort/timeout later
        // in the pass. Guarded + re-merged per #75 — see persist_fetched_halves.
        let fresh_detail = matches!(ours.detail, Some(DetailFetch::Live(_)));
        match persist_fetched_halves(&deps.store, app_id, snapshot, &ours).await {
            Ok(PersistResult::Written { cache, after_race }) => {
                if after_race {
                    lost_race += 1;
                }
                fetched += 1;
                if fresh_detail {
                    supersede_adult(&mut adult_appids, app_id, cache.detail.as_ref());
                }
            }
            Ok(PersistResult::LostTwice) => {
                lost_race += 1;
                tracing::warn!(app_id, "steam enrichment: lost the STEAMAPP# race twice — skipping app, next sync retries");
                continue 'apps;
            }
            Err(e) => {
                tracing::warn!(app_id, error = ?e, "steam enrichment: put_steam_app failed — this app not persisted");
                continue 'apps;
            }
        }
```

Also in this step:
- Remove the now-dead `cache` field from `Work` (~line 2075) and from the decide-pass `worklist.push`; ALSO delete the decide-pass binding `let cache = existing.unwrap_or_else(|| dynamo::SteamAppCache::empty(app_id));` that fed it — leaving it dead fails clippy `-D warnings` in Task 6. (The decide pass keeps reading via plain `get_steam_app` — classification only.)
- Note the old loop's `reviews_fetched_at`/`overall`/`recent` assignments and `cache.fetched_at = now` lines are all subsumed by the merge; delete them with the old body.
- Declare `let mut lost_race = 0u32;` beside the other counters (~line 2150).
- Extend the summary line: `"steam enrichment: fetched={fetched} fresh={fresh} negative={negative} lost_race={lost_race} aborted_429={aborted_429} auto_hidden={auto_hidden} tag_batch_failed={tag_batch_failed}"`.

- [ ] **Step 2: Run** — `cargo check -p fulfillment --tests` → exactly ONE error remains: the backfill `E0061` at ~lib.rs:2423 (Task 5's work — do not fix it here). No test run is possible yet; the merge_ tests first run green in Task 5 Step 4.
- [ ] **Step 3: Commit**

```bash
git add crates/fulfillment/src/lib.rs
git commit -S -m "feat(fulfillment): enrichment writes STEAMAPP# through the #75 guard"
```

---

### Task 5: backfill caller + BackfillSummary.lost_race

**Files:**
- Modify: `crates/fulfillment/src/lib.rs` — `BackfillSummary` (~line 2296) and `backfill_steam_details`' fetch loop (~lines 2380–2445).

**Interfaces:**
- Consumes: Task 3's shapes.
- Produces: `BackfillSummary { …, pub lost_race: u32 }`; backfill "done" log gains `lost_race`.

- [ ] **Step 1: Add the counter to `BackfillSummary`:**

```rust
    /// STEAMAPP# write races detected by the #75 guard (each was re-merged and
    /// retried; a twice-lost app is also counted in `failed`).
    pub lost_race: u32,
```

- [ ] **Step 2: Rewrite the backfill loop body** (from the JIT re-read to the `supersede_adult` call inclusive) on the same pattern as Task 4:

```rust
        // Just-in-time versioned read (#75): snapshot + token seed the guarded
        // merge write; a concurrent writer is detected and re-merged, not
        // clobbered — the don't-run-during-cron rule is now politeness, not
        // load-bearing.
        let snapshot = match store.get_steam_app_versioned(app_id).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(app_id, error = ?e, "backfill: re-read failed — skipping app");
                summary.failed += 1;
                continue;
            }
        };
        let mut ours = FetchedHalves {
            now,
            detail: None,
            reviews: None,
        };
        tokio::time::sleep(pace).await;
        match steam.get_app_details(app_id).await {
            Ok(steam_client::AppDetails::Found(d)) => {
                let mut detail = *d;
                detail.tags = tags_for_app(
                    app_id,
                    tag_data.as_ref(),
                    snapshot.as_ref().and_then(|(c, _)| c.detail.as_ref()),
                );
                ours.detail = Some(DetailFetch::Live(Box::new(detail)));
            }
            // Delisted: negative stub, BOTH clocks stamped by the merge — same
            // semantics as enrichment.
            Ok(steam_client::AppDetails::Delisted) => {
                ours.detail = Some(DetailFetch::Delisted);
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
        let fresh_detail = matches!(ours.detail, Some(DetailFetch::Live(_)));
        match persist_fetched_halves(store, app_id, snapshot, &ours).await {
            Ok(PersistResult::Written { cache, after_race }) => {
                if after_race {
                    summary.lost_race += 1;
                }
                // What was just written IS the outcome: Some detail = live
                // refetch, None = stub. (Post-merge, a concurrent writer's newer
                // half may be what landed — still the honest count.)
                if cache.detail.is_some() {
                    summary.fetched += 1;
                } else {
                    summary.negative += 1;
                }
                if fresh_detail {
                    supersede_adult(&mut adult_appids, app_id, cache.detail.as_ref());
                }
            }
            Ok(PersistResult::LostTwice) => {
                summary.lost_race += 1;
                summary.failed += 1;
                tracing::warn!(app_id, "backfill: lost the STEAMAPP# race twice — skipping app, next pass retries");
                continue;
            }
            Err(e) => {
                tracing::warn!(app_id, error = ?e, "backfill: put_steam_app failed — this app not persisted");
                summary.failed += 1;
                continue;
            }
        }
```

- [ ] **Step 3: Extend the backfill "done" log** with `lost_race = summary.lost_race,` alongside the other fields.
- [ ] **Step 4: Run** — `cargo check -p fulfillment` → clean; `cargo test -p fulfillment --lib` → pass. Workspace compiles again except fulfillment's integration tests (Task 6).
- [ ] **Step 5: Commit**

```bash
git add crates/fulfillment/src/lib.rs
git commit -S -m "feat(fulfillment): backfill writes STEAMAPP# through the #75 guard + lost_race counter"
```

---

### Task 6: handler_test call sites + deterministic race test + full verify

**Files:**
- Modify: `crates/fulfillment/tests/handler_test.rs` (~15 mechanical call-site updates + 1 new test)

**Interfaces:**
- Consumes: everything above; test helpers `store_or_skip` (line 67), `fresh_cache` (4283), `stale_detail_cache` (4910).
- Produces: the deterministic lost-race integration test.

- [ ] **Step 1: Update existing call sites** — every `put_steam_app(&x)` in handler_test.rs seeds a fresh per-test table → append `, SteamAppPutGuard::Absent` (verify each id is a first write; token-read + `Unchanged` for any overwrite). Import `SteamAppPutGuard` (and Task 3's types) at the top.
- [ ] **Step 2: Write the race test** (fails before wiring is right, passes after):

```rust
/// #75: a concurrent STEAMAPP# writer between the JIT read and the put is
/// detected (LostRace), re-merged newest-wins, and retried — NEITHER writer's
/// half is lost. This is the deterministic version of the race the guard closes:
/// we hand persist_fetched_halves a stale snapshot on purpose.
#[tokio::test]
async fn persist_fetched_halves_remerges_on_lost_race() {
    let Some(store) = store_or_skip("persist-lost-race").await else {
        return;
    };
    // Seed (clocks at 100, all halves present — fresh_cache's shape), then take
    // the snapshot a writer would hold.
    store
        .put_steam_app(&fresh_cache(570, 100), SteamAppPutGuard::Absent)
        .await
        .unwrap();
    let stale_snapshot = store.get_steam_app_versioned(570).await.unwrap();

    // Concurrent writer lands a fresh, DISTINGUISHABLE reviews half after our read.
    let (mut theirs, v) = store.get_steam_app_versioned(570).await.unwrap().unwrap();
    theirs.overall = Some(steam_client::ReviewSummary {
        desc: "Overwhelmingly Positive".into(),
        total_positive: 99,
        total_negative: 1,
        total_reviews: 100,
    });
    theirs.reviews_fetched_at = 500;
    store
        .put_steam_app(&theirs, SteamAppPutGuard::Unchanged(v))
        .await
        .unwrap();

    // We persist a fresh, DISTINGUISHABLE detail half against the STALE snapshot.
    let mut our_detail = fresh_cache(570, 600).detail.unwrap();
    our_detail.name = "Fresh".into();
    let ours = FetchedHalves {
        now: 600,
        detail: Some(DetailFetch::Live(Box::new(our_detail))),
        reviews: None,
    };
    let result = persist_fetched_halves(&store, 570, stale_snapshot, &ours)
        .await
        .unwrap();

    let PersistResult::Written { cache, after_race } = result else {
        panic!("expected Written, got LostTwice");
    };
    assert!(after_race, "first put must lose to the concurrent write");
    // BOTH halves survive, provably each writer's own: our detail, their reviews.
    assert_eq!(cache.detail.as_ref().unwrap().name, "Fresh");
    assert_eq!(cache.fetched_at, 600);
    assert_eq!(
        cache.overall.as_ref().unwrap().desc,
        "Overwhelmingly Positive"
    );
    assert_eq!(cache.reviews_fetched_at, 500);
    // The store agrees with the return value.
    let (in_store, _) = store.get_steam_app_versioned(570).await.unwrap().unwrap();
    assert_eq!(in_store.detail.as_ref().unwrap().name, "Fresh");
    assert_eq!(
        in_store.overall.as_ref().unwrap().desc,
        "Overwhelmingly Positive"
    );
}
```

(Verified: `fresh_cache(app_id, now)` sets BOTH `fetched_at: now` and `reviews_fetched_at: now` — handler_test.rs:4312-13 — so the seed's clocks are 100 and the arithmetic seed=100 < theirs=500 < ours=600 holds as written.)

- [ ] **Step 3: Full verify** — dynamodb-local up, then:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Expected: all green (handler tests, store tests, admin/public api tests, pure merges).

- [ ] **Step 4: Commit**

```bash
git add crates/fulfillment/tests/handler_test.rs
git commit -S -m "test(fulfillment): deterministic lost-race re-merge test + guard adoption in seeds (#75)"
```

---

### Task 7: docs + issue linkage

**Files:**
- Modify: `DESIGN.md` (only if it documents the STEAMAPP# item shape or the backfill/cron caveat — search `STEAMAPP`/`backfill`; if silent, skip), `docs/superpowers/specs/2026-07-17-steamapp-conditional-put-design.md` (status → implemented)

- [ ] **Step 1:** Flip the spec's `status: draft` → `status: implemented`. If DESIGN.md documents the STEAMAPP# item or the don't-run-backfill-during-cron rule, update it (the rule is now politeness, not load-bearing); otherwise skip.
- [ ] **Step 2:** Commit:

```bash
git add DESIGN.md docs/superpowers
git commit -S -m "docs: #75 spec status + STEAMAPP# guard notes"
```
