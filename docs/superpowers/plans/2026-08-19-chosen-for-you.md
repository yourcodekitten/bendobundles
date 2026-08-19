# chosen for you — per-link curation — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A link can carry the specific games ben picked when he wrapped it — chosen at create in the admin catalog, enforced at the claim endpoint, presented to the friend in ben's order with honest ghost cards for games that are gone.

**Architecture:** `curated_game_ids: Option<Vec<String>>` on `domain::Link`, stored as a **top-level order-preserving dynamo `L` attribute** (never in the `body` blob — enforcement field, dynamo doctrine `lib.rs:379`; rollback-erasure immunity per spec §1). Public-api partitions the curated set into live/ghost cards and gates claims server-side. Admin-api validates and stores the set at create. Web: catalog multi-select → create-form chips (admin), no-shuffle + ghost cards (friend).

**Tech Stack:** Rust (axum, aws-sdk-dynamodb, serde), React 19 + TS + Tailwind v4, vitest + testing-library, cargo test against dynamodb-local.

**Spec:** `docs/superpowers/specs/2026-08-19-chosen-for-you-design.md` — read it first; every decision below is argued there, including the family-review reversals.

## Global Constraints

- Rust gates: `cargo fmt --check` · `cargo clippy --workspace --all-targets --all-features -- -D warnings` · `cargo test --workspace --no-fail-fast` with dynamodb-local at `DYNAMODB_LOCAL_URL=http://localhost:8000`. **Store-backed tests SILENTLY SKIP without that env var — a green run without it proves nothing about them.**
- 🔴 **EXECUTION-BOX REALITY (kitten's box, measured 2026-08-19): no docker, no java, no dynamodb-local anywhere, ~800MB free RAM on a global-OOM-kill box.** Consequences, all mandatory: (1) do NOT set `DYNAMODB_LOCAL_URL` locally — set-but-unreachable PANICS by design (`store_or_skip`'s anti-forged-green guard); (2) do NOT stand up a JVM; (3) local verification = fmt + clippy + compile + non-store tests + the FULL web suite; store-backed tests SKIP locally — expected, count and NAME the skipped tests in the task report, never call their absence green; (4) **the RED/GREEN receipt for store-backed tests is CI on the draft PR** — CI runs on `pull_request` only, so Task 1 ends by opening a DRAFT PR, and every task's push gets the full suite against a real dynamodb-local. Watch it with `gh pr checks` before calling a task done. The `DYNAMODB_LOCAL_URL=…`-prefixed commands in the task steps are CI-fidelity documentation and run verbatim in any environment that HAS a runner.
- Web gates (from `web/`): `npm run lint` · `npm run typecheck` · `npm test -- --run` · `npm run build`.
- ALL user-facing copy and aria-labels are lowercase (DESIGN.md, The Lowercase Rule).
- Burgundy (`bg-give`) ONLY where giving/claiming happens (The Button Burgundy Rule). New chips are `bg-shelf`/`bg-control` + `text-ink-soft`; ghost-card chip `bg-floor text-dust`. No shadows on cards/chips (The Ceremony Rule).
- Every commit GPG-signed (`git commit -S`), authored `code kitten <yourcodekitten@gmail.com>`.
- The wire carries `gone: true` and never the cause (spec §2). The sealed view withholds curation entirely.
- Order is meaning: the id array's order is ben's pick order, everywhere — storage, wire, grid.

---

### Task 1: domain field + attribute storage + rollback pins

**Files:**
- Modify: `crates/domain/src/lib.rs` (Link struct — `unlock_at` ends :224, insert before `created_at` :226)
- Modify: `crates/dynamo/src/schema.rs` (`link_body` :98-130, `link_item` :132-179)
- Modify: `crates/dynamo/src/lib.rs` (`link_from_item` :387-…)
- Modify: `crates/dynamo/tests/store_test.rs` (fixture `link()` :80 + new tests)
- Test: `crates/dynamo/tests/store_test.rs`

**Interfaces:**
- Produces: `Link.curated_game_ids: Option<Vec<String>>` — every later task reads this exact field name. Storage attribute name: `curated_game_ids` (top-level, `AttributeValue::L` of `S`).
- **Compiler note for the executor:** `Link` has no builder; adding the field breaks every struct literal — `schema::link_body`'s exhaustive destructure (by design), `store_test.rs::link()`, `public-api/tests/api_test.rs::test_link()`, `admin-api/src/lib.rs:678` (create handler), `admin-api/tests` fixtures. Fix each: fixtures get `curated_game_ids: None`; the admin handler is Task 4's job but needs `curated_game_ids: None` NOW to compile (Task 4 replaces it with the real value).

- [ ] **Step 1: Write the failing tests** (append to `crates/dynamo/tests/store_test.rs`; copy the `raw_client`/`store_or_skip` usage from `gift_note_never_persisted_in_body_blob` :244):

```rust
#[tokio::test]
async fn curated_ids_round_trip_in_order_and_never_in_body() {
    let Some(store) = store_or_skip("curated-roundtrip").await else { return };
    let raw = raw_client("curated-roundtrip").await;
    let mut l = link("cur-tok");
    // Deliberately NOT sorted: order is ben's pick order and must survive verbatim.
    l.curated_game_ids = Some(vec!["g-3".into(), "g-1".into(), "g-2".into()]);
    store.create_link(&l).await.unwrap();

    let got = store.get_link("cur-tok").await.unwrap().unwrap();
    assert_eq!(
        got.curated_game_ids.as_deref(),
        Some(&["g-3".to_string(), "g-1".into(), "g-2".into()][..]),
        "attribute must round-trip in pick order"
    );

    // The body blob NEVER carries the field (top-level attr is the only home —
    // spec §1, dynamo doctrine lib.rs:379: enforcement fields go top-level).
    let item = raw
        .get_item()
        .table_name("t-curated-roundtrip")
        .key("pk", aws_sdk_dynamodb::types::AttributeValue::S("LINK#cur-tok".into()))
        .key("sk", aws_sdk_dynamodb::types::AttributeValue::S("META".into()))
        .send().await.unwrap().item.unwrap();
    let body = item["body"].as_s().unwrap();
    assert!(!body.contains("curated_game_ids"), "body blob must not carry the set: {body}");
    assert!(item.contains_key("curated_game_ids"), "top-level attribute must exist");
}

#[tokio::test]
async fn open_shelf_link_stores_no_curated_attribute() {
    let Some(store) = store_or_skip("curated-absent").await else { return };
    let raw = raw_client("curated-absent").await;
    store.create_link(&link("open-tok")).await.unwrap();
    let item = raw.get_item()
        .table_name("t-curated-absent")
        .key("pk", aws_sdk_dynamodb::types::AttributeValue::S("LINK#open-tok".into()))
        .key("sk", aws_sdk_dynamodb::types::AttributeValue::S("META".into()))
        .send().await.unwrap().item.unwrap();
    assert!(!item.contains_key("curated_game_ids"),
        "absent attribute is the single representation of open shelf (spec §1)");
    let got = store.get_link("open-tok").await.unwrap().unwrap();
    assert_eq!(got.curated_game_ids, None);
}

#[tokio::test]
async fn claim_leaves_curated_attribute_standing() {
    let Some(store) = store_or_skip("curated-claim-pin").await else { return };
    store.put_game(&game(1, true)).await.unwrap();
    let mut l = link("cur-claim");
    l.curated_game_ids = Some(vec![game(1, true).id]);
    store.create_link(&l).await.unwrap();
    store.claim_game("cur-claim", &game(1, true).id, "claim-1", time::OffsetDateTime::now_utc())
        .await.unwrap();
    let got = store.get_link("cur-claim").await.unwrap().unwrap();
    assert_eq!(got.curated_game_ids.as_deref().map(<[String]>::len), Some(1),
        "claim's SET body rewrite must not touch the top-level attribute");
}

#[tokio::test]
async fn stale_binary_write_back_cannot_erase_curation() {
    // THE ROLLBACK PIN (spec §1, Lilith): a pre-field binary deserializes this link
    // with curated_game_ids: None and calls update_link_meta (revoke does exactly
    // this). The attribute must survive, because update_link_meta's SET expression
    // does not name it. If this test ever fails, someone routed the attribute
    // through a write path a stale binary also runs — undo that.
    let Some(store) = store_or_skip("curated-rollback-pin").await else { return };
    let mut l = link("cur-stale");
    l.curated_game_ids = Some(vec!["g-1".into(), "g-2".into()]);
    store.create_link(&l).await.unwrap();

    let mut stale_view = store.get_link("cur-stale").await.unwrap().unwrap();
    stale_view.curated_game_ids = None; // what a pre-field Link deserialize produces
    stale_view.revoked = true;          // the realistic stale write: a revoke
    store.update_link_meta(&stale_view).await.unwrap();

    let got = store.get_link("cur-stale").await.unwrap().unwrap();
    assert!(got.revoked, "the stale write itself must land");
    assert_eq!(got.curated_game_ids.as_deref().map(<[String]>::len), Some(2),
        "recoverable-and-loud: the attribute survives a pre-field binary's write-back");
}
```

- [ ] **Step 2: Run to verify they fail to COMPILE** (the field doesn't exist — that's the expected failure):
Run: `DYNAMODB_LOCAL_URL=http://localhost:8000 cargo test -p dynamo --test store_test curated 2>&1 | tail -20; echo "exit=${PIPESTATUS[0]}"`
Expected: compile error `no field curated_game_ids on type Link` (exit!=0).

- [ ] **Step 3: Implement.**

`crates/domain/src/lib.rs`, after `unlock_at` (~:226), before `created_at`:
```rust
    /// The games ben picked when he wrapped this link — `None` = open shelf (the
    /// whole listable catalog, every pre-field record). CREATE-TIME-ONLY, like
    /// `unlock_at` (spec 2026-08-19 §1): no edit path exists or may be added
    /// without its own spec. ORDER IS MEANING: pick order = presentation order.
    /// Storage: top-level dynamo `L` attribute, NEVER the body blob — the claim
    /// gate reads this, making it an enforcement field (dynamo doctrine, its
    /// lib.rs "body for immutable identity, top-level attrs for enforcement"),
    /// and a body-carried copy would be erased by a pre-field binary's `SET
    /// body = :b` write-back on rollback. See the rollback pin in store_test.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curated_game_ids: Option<Vec<String>>,
```

`crates/dynamo/src/schema.rs` — `link_body`: add `curated_game_ids: _,` to the destructure and `curated_game_ids: None,` to the rebuilt stripped literal, with:
```rust
        // Stripped: lives ONLY in the top-level `curated_game_ids` attribute.
        // Body-carried curation dies by rollback: a pre-field binary's Link
        // deserialize drops the unknown field and its SET body write-back
        // erases it. Top-level is structurally out of that blast radius.
        curated_game_ids: _,
```

`crates/dynamo/src/schema.rs` — `link_item`, after the `thanked_at` insert:
```rust
    // Top-level, order-preserving L (a String Set would sort/dedupe — order is
    // ben's pick order and duplicates were already 422'd at create). Written
    // once here; no update expression anywhere names this attribute, which is
    // the rollback immunity the spec pins.
    if let Some(ids) = &l.curated_game_ids {
        item.insert(
            "curated_game_ids".into(),
            AttributeValue::L(ids.iter().map(|id| s(id)).collect()),
        );
    }
```

`crates/dynamo/src/lib.rs` — `link_from_item`, with the other top-level overrides (after the `gift_note` override):
```rust
    // Top-level attr is the ONLY source (body never carries it). Absent = open
    // shelf. Malformed entries are a Corrupt read, not a silent skip.
    link.curated_game_ids = match item.get("curated_game_ids") {
        None => None,
        Some(AttributeValue::L(list)) => Some(
            list.iter()
                .map(|v| v.as_s().map(String::clone))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| StoreError::Corrupt("curated_game_ids holds a non-string"))?,
        ),
        Some(_) => return Err(StoreError::Corrupt("curated_game_ids is not a list")),
    };
```
(Check `link_from_item`'s existing error idiom first and match it exactly — if its overrides use a helper like `n_attr`, follow the local pattern for error construction. `StoreError::Corrupt` holds a `&'static str` (lib.rs:313): plain literals as above, never `format!`.)

Also add the pre-field BODY pin (a plain `#[test]`, no store needed — put it in `store_test.rs` beside the others; serde_json is already a dynamo dep):
```rust
#[test]
fn pre_field_link_body_json_parses_with_none_curation() {
    // A body blob written before the field existed — the exact fields
    // schema::link_body keeps, nothing more. Must deserialize clean.
    let legacy = r#"{"token":"t","label":"l","claims_allowed":1,"claims_used":0,"revoked":false,"expires_at":null,"created_at":"2026-01-01T00:00:00Z"}"#;
    let l: domain::Link = serde_json::from_str(legacy).expect("legacy body parses");
    assert_eq!(l.curated_game_ids, None);
}
```

Fix every `Link` literal the compiler flags with `curated_game_ids: None` (fixtures + admin handler — Task 4 owns the real admin value).

- [ ] **Step 4: Run the new tests + the whole dynamo suite:**
Run: `DYNAMODB_LOCAL_URL=http://localhost:8000 cargo test -p dynamo --test store_test 2>&1 | tail -5; echo "exit=${PIPESTATUS[0]}"`
Expected: exit=0, all pass including the four new ones (not SKIP — verify the word "SKIP" absent for the new names; without the env var these tests SKIP GREEN, which proves nothing).

- [ ] **Step 5: Workspace still compiles:** `cargo clippy --workspace --all-targets --all-features -- -D warnings 2>&1 | tail -5` → clean.

- [ ] **Step 6: Commit, push, open the DRAFT PR** (the CI receipt for every store-backed test from here on):
```bash
git add -A && git commit -S -m "feat(domain+dynamo): curated_game_ids on Link — top-level order-preserving attribute, rollback-pinned" \
  && git push -u origin chosen-for-you \
  && gh pr create -R yourcodekitten/bendobundles --draft --title "chosen for you: per-link curation — the product thesis, implemented" --body "draft while the plan executes — real body lands at the final task. spec + plan in docs/superpowers/."
gh pr checks --watch
```
Expected: CI green — the four new store tests RAN there (open the run log and find their names; a green that skipped them is not a receipt).

---

### Task 2: public-api — curated LinkView, partition, ghosts

**Files:**
- Modify: `crates/public-api/src/lib.rs` (`GameView` :76-93, `LinkView` :127-153, sealed literal :573, games join :603-638, final literal :659-671)
- Test: `crates/public-api/tests/api_test.rs`

**Interfaces:**
- Consumes: `Link.curated_game_ids` (Task 1).
- Produces wire fields the web (Task 5) mirrors: `LinkView.curated: bool` (absent when false), `GameView.gone: bool` (absent when false). Ghost `GameView` = real `id/title/bundle/key_type/artwork_url/steam_app_id`, empty `genres/tags`, `gone: true`.
- Produces `fn live_on_link(link: &domain::Link, game: &domain::Game) -> bool` — THE one liveness computation (spec §2, family round three). Task 3's detail gate is its second caller; it must never be re-derived as parallel `||` arms.

- [ ] **Step 1: Write the failing tests** (append to `api_test.rs`; helpers `store_or_skip`/`test_game`/`test_link`/`plain_router`/`MockInvoker` per :26-:143):

```rust
#[tokio::test]
async fn curated_link_serves_the_set_in_ben_order_with_ghosts() {
    let Some(store) = store_or_skip("curated-view").await else { return };
    // three games: g2 stays live, g1 is GIFTED (a decided state → ghost),
    // g3 is hidden by ben (ghost). Pick order: g3, g1, g2 — order must survive.
    // ⚠️ Do NOT seed the ghost by claiming via another link at the store layer:
    // that leaves the game Pending, which is LIVE by live_on_link (spec §2) —
    // the adversarial review caught exactly that in this test's first draft.
    store.put_game(&test_game(2)).await.unwrap();
    let mut gifted = test_game(1); gifted.status = domain::GameStatus::Gifted;
    store.put_game(&gifted).await.unwrap();
    let mut hidden = test_game(3); hidden.hidden = true;
    store.put_game(&hidden).await.unwrap();

    let mut lnk = test_link("curated-tok");
    lnk.curated_game_ids = Some(vec![test_game(3).id, test_game(1).id, test_game(2).id]);
    store.create_link(&lnk).await.unwrap();

    let mock = MockInvoker::new(FulfillResponse::GiftUrl { url: "https://x.com/g".into() });
    let resp = plain_router(Arc::clone(&store), mock)
        .oneshot(Request::get("/api/l/curated-tok").body(Body::empty()).unwrap())
        .await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(j["curated"], serde_json::json!(true));
    let games = j["games"].as_array().unwrap();
    assert_eq!(games.len(), 3, "partitioned, never filtered — ghosts included");
    // ben's pick order: g3 (ghost), g1 (ghost), g2 (live)
    assert_eq!(games[0]["id"], serde_json::json!(test_game(3).id));
    assert_eq!(games[0]["gone"], serde_json::json!(true));
    assert_eq!(games[1]["id"], serde_json::json!(test_game(1).id));
    assert_eq!(games[1]["gone"], serde_json::json!(true));
    assert_eq!(games[2]["id"], serde_json::json!(test_game(2).id));
    assert!(games[2].get("gone").is_none(), "live cards carry no gone key at all");
    // cause-blind wire (spec §2): no status/cause field on any card
    for g in games { assert!(g.get("status").is_none() && g.get("cause").is_none()); }
}

#[tokio::test]
async fn pending_curated_game_rides_live_not_ghost() {
    let Some(store) = store_or_skip("curated-pending").await else { return };
    store.put_game(&test_game(1)).await.unwrap();
    let other = test_link("oth-p");
    store.create_link(&other).await.unwrap();
    // a claim on ANOTHER link parks g1 in Pending (MockInvoker not consulted by claim_game)
    store.claim_game("oth-p", &test_game(1).id, "c-p", time::OffsetDateTime::now_utc())
        .await.unwrap();
    // NOTE for the executor: claim_game leaves the game Pending only until
    // fulfillment resolves it; at the store layer with no invoker involved it
    // STAYS Pending — exactly the in-flight state the spec wants live. Verify
    // with a get_game assert before relying on it:
    assert_eq!(store.get_game(&test_game(1).id).await.unwrap().unwrap().status,
        domain::GameStatus::Pending);

    let mut lnk = test_link("cur-p");
    lnk.curated_game_ids = Some(vec![test_game(1).id]);
    store.create_link(&lnk).await.unwrap();
    let mock = MockInvoker::new(FulfillResponse::GiftUrl { url: "https://x.com/g".into() });
    let resp = plain_router(Arc::clone(&store), mock)
        .oneshot(Request::get("/api/l/cur-p").body(Body::empty()).unwrap()).await.unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(j["games"][0].get("gone").is_none(),
        "Pending is in flight — nobody decided it; it rides live (spec §2)");
}

#[tokio::test]
async fn open_shelf_wire_shape_is_unchanged_no_curated_key() {
    let Some(store) = store_or_skip("open-shelf-pin").await else { return };
    store.put_game(&test_game(1)).await.unwrap();
    store.create_link(&test_link("plain-tok")).await.unwrap();
    let mock = MockInvoker::new(FulfillResponse::GiftUrl { url: "https://x.com/g".into() });
    let resp = plain_router(Arc::clone(&store), mock)
        .oneshot(Request::get("/api/l/plain-tok").body(Body::empty()).unwrap()).await.unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let raw = std::str::from_utf8(&bytes).unwrap();
    assert!(!raw.contains("curated"), "open shelf serializes byte-identically to today");
    assert!(!raw.contains("gone"));
}

#[tokio::test]
async fn sealed_curated_link_withholds_curation_entirely() {
    let Some(store) = store_or_skip("sealed-curated").await else { return };
    store.put_game(&test_game(1)).await.unwrap();
    let mut lnk = test_link("sealed-cur");
    lnk.unlock_at = Some(time::OffsetDateTime::now_utc() + time::Duration::seconds(3600));
    lnk.curated_game_ids = Some(vec![test_game(1).id]);
    store.create_link(&lnk).await.unwrap();
    let mock = MockInvoker::new(FulfillResponse::GiftUrl { url: "https://x.com/g".into() });
    let resp = plain_router(Arc::clone(&store), mock)
        .oneshot(Request::get("/api/l/sealed-cur").body(Body::empty()).unwrap()).await.unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let raw = std::str::from_utf8(&bytes).unwrap();
    // raw-substring on purpose: over-matching is the safe direction for a
    // withholding pin — any future curated-ish key leaking into a sealed
    // payload should trip this.
    assert!(!raw.contains("curated"), "the seal withholds even the mode (spec §2)");
    let j: serde_json::Value = serde_json::from_str(raw).unwrap();
    assert_eq!(j["games"], serde_json::json!([]));
}
```

- [ ] **Step 2: Run to verify failure:** `DYNAMODB_LOCAL_URL=http://localhost:8000 cargo test -p public-api --test api_test curated 2>&1 | tail -10; echo "exit=${PIPESTATUS[0]}"` → compile error (`curated_game_ids` ok from Task 1, but `curated`/`gone` fields and assertions fail) or assertion failures.

- [ ] **Step 3: Implement.**

`GameView` (:76-93): add
```rust
    /// Ghost marker (curated links only): this chosen game is in a DECIDED
    /// non-listable state (gifted / ben-redeemed / expired / ungiftable /
    /// hidden). Absent when false so open-shelf payloads stay byte-identical.
    /// Cause-blind by decision (spec §2) — never add the cause here casually.
    #[serde(skip_serializing_if = "is_false")]
    gone: bool,
```
plus at module level near the view structs:
```rust
fn is_false(b: &bool) -> bool {
    !*b
}
```
`GameView::from_game` (:102): set `gone: false`. Add:
```rust
    /// A curated pick in a decided non-listable state — real identity fields
    /// (the friend sees what the gift WAS), no enrichment, gone flag on.
    fn ghost(g: domain::Game) -> Self {
        let mut v = Self::from_game(g, Vec::new(), Vec::new());
        v.gone = true;
        v
    }
```

`LinkView` (:127-153): add
```rust
    /// True iff this link carries a curated set. Absent when false (open-shelf
    /// payloads unchanged); the sealed view sets false deliberately — the seal
    /// withholds even the mode.
    #[serde(skip_serializing_if = "is_false")]
    curated: bool,
```
Sealed literal (:573): `curated: false,` with comment `// withheld, not false — the seal hides the mode too`. Final literal (:663): `curated: is_curated,`.

Module-level, near the view structs:
```rust
/// THE one liveness computation (spec §2): a game is LIVE on a link iff the
/// grid offers it as a claimable card. The curated partition below and the
/// detail gate (its second caller) both use this — "the gate mirrors the
/// grid, not one id more" is true by construction, not by maintenance. The
/// gate's #154 comment records the last hand-mirrored correspondence that
/// drifted; do not add a third caller-specific rederivation.
fn live_on_link(link: &domain::Link, game: &domain::Game) -> bool {
    match &link.curated_game_ids {
        // Curated: member, and Pending rescues ONLY the status axis — never a
        // deliberate hide or an ungiftable key (spec §2, Lilith's sign-off
        // catch: hidden+Pending has no path back to claimable, so `is_listable
        // || Pending` would pin a permanently-unclaimable card live).
        Some(ids) => {
            ids.iter().any(|id| id == &game.id)
                && game.giftable
                && !game.hidden
                && matches!(
                    game.status,
                    domain::GameStatus::Available | domain::GameStatus::Pending
                )
        }
        // Open shelf: the sparse listable index that feeds the grid IS this
        // predicate — one truth, two spellings, pinned by the gate tests.
        None => game.is_listable(),
    }
}
```

Games assembly (:603-638) — compute `let is_curated = link.curated_game_ids.is_some();` before
the join; the games future takes a shared borrow of `link` (it ends at the join, before
`link.label` moves into the final literal — the claims future never touches `link`, so the
borrow checker is satisfied). Inside the games future, branch:
```rust
        if hide_games {
            return vec![];
        }
        match &link.curated_game_ids {
            Some(ids) => {
                // Partition, never filter (spec §2): every stored id becomes a
                // live card or a ghost, in ben's pick order; an id is skipped
                // only when the game record no longer exists at all.
                let found = match s.store.batch_get_games(&ids).await {
                    Ok(m) => m,
                    Err(_) => return vec![],
                };
                let mut app_ids: Vec<u32> = found
                    .values()
                    .filter(|g| live_on_link(&link, g))
                    .filter_map(|g| g.steam_app_id)
                    .collect();
                app_ids.sort_unstable();
                app_ids.dedup();
                let caches = s
                    .store
                    .batch_get_steam_genres_tags(&app_ids)
                    .await
                    .unwrap_or_default();
                ids.iter()
                    .filter_map(|id| found.get(id))
                    .map(|g| {
                        // live vs ghost: the ONE computation, shared with the
                        // detail gate. (Membership is tautological here — g came
                        // from the set — but the shared fn is the point.)
                        if live_on_link(&link, g) {
                            let gt = g.steam_app_id.and_then(|id| caches.get(&id));
                            let genres = gt
                                .map(|c| c.genres.iter().take(5).cloned().collect())
                                .unwrap_or_default();
                            let tags = gt.map(|c| c.tags.clone()).unwrap_or_default();
                            GameView::from_game(g.clone(), genres, tags)
                        } else {
                            GameView::ghost(g.clone())
                        }
                    })
                    .collect()
            }
            None => {
                /* the ENTIRE existing open-shelf body, verbatim — list_listable_games
                   → slim cache batch → GameView::from_game. Do not touch it. */
            }
        }
```
(Executor: `domain::GameStatus` — check the existing `use` lines; public-api already imports `domain::…` items, follow the local idiom.)

- [ ] **Step 4: Run:** `DYNAMODB_LOCAL_URL=http://localhost:8000 cargo test -p public-api --test api_test 2>&1 | tail -5; echo "exit=${PIPESTATUS[0]}"` → all pass including the 4 new; existing sealed/link tests untouched-green.

- [ ] **Step 5: Commit:** `git add -A && git commit -S -m "feat(public-api): curated LinkView — partition into live/ghost in pick order, seal withholds the mode"`

---

### Task 3: public-api — claim gate + detail-gate arm

**Files:**
- Modify: `crates/public-api/src/lib.rs` (`handle_post_claim` — insert at :725, between the can_claim gate ending :724 and the `// 3. Atomic claim intake` comment :726; `handle_game_detail` access gate :1150-1160)
- Test: `crates/public-api/tests/api_test.rs`

**Interfaces:**
- Consumes: `Link.curated_game_ids` (Task 1) · `fn live_on_link(&domain::Link, &domain::Game) -> bool` (Task 2 — THE single liveness computation; this task is its second caller).
- Produces: 409 refusal body `{"error": "that one isn't part of this gift"}` — no dedicated consumer needed: the friend UI's existing claim-error path already surfaces server strings verbatim.

- [ ] **Step 1: Write the failing tests:**

```rust
#[tokio::test]
async fn claim_of_out_of_set_game_is_409_and_leaves_world_untouched() {
    let Some(store) = store_or_skip("curated-claim-gate").await else { return };
    store.put_game(&test_game(1)).await.unwrap();
    store.put_game(&test_game(2)).await.unwrap();
    let mut lnk = test_link("cur-gate");
    lnk.curated_game_ids = Some(vec![test_game(1).id]);
    store.create_link(&lnk).await.unwrap();
    let mock = MockInvoker::new(FulfillResponse::GiftUrl { url: "https://x.com/g".into() });
    let resp = plain_router(Arc::clone(&store), mock.clone())
        .oneshot(
            Request::post("/api/l/cur-gate/claim")
                .header("content-type", "application/json")
                .body(Body::from(format!(r#"{{"game_id":"{}"}}"#, test_game(2).id)))
                .unwrap(),
        )
        .await.unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(j["error"], serde_json::json!("that one isn't part of this gift"));
    // the surface is not the boundary — prove the world didn't move:
    assert_eq!(store.get_game(&test_game(2).id).await.unwrap().unwrap().status,
        domain::GameStatus::Available);
    assert_eq!(store.get_link("cur-gate").await.unwrap().unwrap().claims_used, 0);
    // MockInvoker's real API (api_test.rs:118) — there is no .requests():
    assert!(mock.captured_request().await.is_none(), "fulfillment never invoked");
}

#[tokio::test]
async fn claim_of_in_set_game_flows_normally() {
    let Some(store) = store_or_skip("curated-claim-ok").await else { return };
    store.put_game(&test_game(1)).await.unwrap();
    let mut lnk = test_link("cur-ok");
    lnk.curated_game_ids = Some(vec![test_game(1).id]);
    store.create_link(&lnk).await.unwrap();
    let mock = MockInvoker::new(FulfillResponse::GiftUrl { url: "https://x.com/g".into() });
    let resp = plain_router(Arc::clone(&store), mock)
        .oneshot(
            Request::post("/api/l/cur-ok/claim")
                .header("content-type", "application/json")
                .body(Body::from(format!(r#"{{"game_id":"{}"}}"#, test_game(1).id)))
                .unwrap(),
        )
        .await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn detail_gate_opens_for_curated_pending_only() {
    let Some(store) = store_or_skip("curated-detail-gate").await else { return };
    store.put_game(&test_game(1)).await.unwrap(); // will go Pending via other link
    store.put_game(&test_game(2)).await.unwrap(); // will be HIDDEN (decided) — stays 404
    let mut hidden = test_game(2); hidden.hidden = true;
    store.put_game(&hidden).await.unwrap();
    let other = test_link("oth-d");
    store.create_link(&other).await.unwrap();
    store.claim_game("oth-d", &test_game(1).id, "c-d", time::OffsetDateTime::now_utc())
        .await.unwrap();

    let mut lnk = test_link("cur-d");
    lnk.curated_game_ids = Some(vec![test_game(1).id, test_game(2).id]);
    store.create_link(&lnk).await.unwrap();
    let mock = MockInvoker::new(FulfillResponse::GiftUrl { url: "https://x.com/g".into() });

    // live Pending curated card → modal must load
    let ok = plain_router(Arc::clone(&store), mock.clone())
        .oneshot(Request::get(format!("/api/l/cur-d/games/{}/detail", test_game(1).id))
            .body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(ok.status(), StatusCode::OK, "gate mirrors what the grid offers live");

    // ghost (hidden = decided) → byte-identical 404, no oracle
    let no = plain_router(Arc::clone(&store), mock.clone())
        .oneshot(Request::get(format!("/api/l/cur-d/games/{}/detail", test_game(2).id))
            .body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(no.status(), StatusCode::NOT_FOUND, "ghosts stay non-interactive");

    // hidden + Pending → GHOST, so 404: Pending rescues only the status axis
    // (ben hid it while a claim was in flight; no resolution path re-lists it)
    let mut hp = store.get_game(&test_game(1).id).await.unwrap().unwrap();
    hp.hidden = true;
    store.put_game(&hp).await.unwrap();
    let mock3 = mock.clone();
    let hpr = plain_router(Arc::clone(&store), mock3)
        .oneshot(Request::get(format!("/api/l/cur-d/games/{}/detail", test_game(1).id))
            .body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(hpr.status(), StatusCode::NOT_FOUND);
    // un-hide to keep the earlier live-Pending arm's world intact for the
    // cross-link arm below (ordering: run this arm AFTER the live-Pending
    // assert above, which it is).
    let mut unh = store.get_game(&test_game(1).id).await.unwrap().unwrap();
    unh.hidden = false;
    store.put_game(&unh).await.unwrap();

    // and a Pending game NOT in this link's set → still 404 (not one id more)
    let mut lnk2 = test_link("cur-d2");
    lnk2.curated_game_ids = Some(vec![test_game(2).id]);
    store.create_link(&lnk2).await.unwrap();
    let cross = plain_router(Arc::clone(&store), mock)
        .oneshot(Request::get(format!("/api/l/cur-d2/games/{}/detail", test_game(1).id))
            .body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(cross.status(), StatusCode::NOT_FOUND);
}
```

- [ ] **Step 2: Run to verify failure:** `DYNAMODB_LOCAL_URL=http://localhost:8000 cargo test -p public-api --test api_test claim_of_out 2>&1 | tail -5; echo "exit=${PIPESTATUS[0]}"` → exit!=0: fails on the 409 assertion — the ungated claim returns 200 OK. (That assertion-failure red, not a compile error, is the correct red.)

- [ ] **Step 3: Implement.** In `handle_post_claim` at :725:
```rust
    // 2.5 Curation gate: a curated link claims only its own games. Server-side —
    // the grid never offers the button, but the surface is not the boundary.
    // Pre-check is race-free BECAUSE the set is create-time-only: no edit exists
    // to race the freshly-read link (spec §3).
    if let Some(ids) = &link.curated_game_ids {
        if !ids.contains(&body.game_id) {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({"error": "that one isn't part of this gift"})),
            )
                .into_response();
        }
    }
```
**Subsumption, stated so nobody deletes the wrong arm (Lilith, family round four):** today's
gate is TWO conditions answering DIFFERENT questions — `is_listable()` (*is this visible on
this link?*) and claims-history (*did YOU already take this one?*). `live_on_link` subsumes
ONLY the visibility arm. **The claims-history arm SURVIVES: it is the friend's receipt, not a
grid mirror** — under the partition, the game they just claimed IS a ghost, so the receipt arm
is the only thing keeping their own claim's modal reachable. "No third arm" must never be
over-applied as "no second arm."

In `handle_game_detail`'s gate (:1152), REPLACE the `is_listable()` arm — one computation, two
callers (spec §2, family round three; a third hand-mirrored `||` arm is the defect the #154
comment on this very gate records):
```rust
    // Friend access gate: live on THIS link's grid (the shared live_on_link
    // computation — the gate serves exactly what the grid offers, by
    // construction) OR in THIS link's claims history. Everything else is the
    // byte-identical 404, no oracle. NOTE the deliberate tightening on curated
    // links: a listable NON-member is not on this grid, so it 404s here — a
    // curated token cannot enumerate the whole catalog's details (spec §2).
    let accessible = if live_on_link(&link, &game) {
        true
    } else {
        match s.store.claims_for_link(&token).await {
            Ok(claims) => claims.iter().any(|c| c.game_id == game_id),
            Err(_) => false,
        }
    };
```
(Executor: the handler already resolves `link` before the gate — confirm the variable name in
scope; if it resolved only the token, add the read via the same idiom the handler's step 1
uses. Update the gate's `3.` friend-access-gate doc-comment to name `live_on_link` as the
single source and the claims arm as the receipt — `1b.` is the LINK-level liveness gate, a
different comment; leave it alone.)

The Step-1 test `detail_gate_opens_for_curated_pending_only` carries this fourth arm FROM THE
START (write it with the rest in Step 1 — same red, not a post-green amendment). The
enumeration tightening it pins is INTENTIONAL, not incidental (spec §2) — an unpinned side
effect is one refactor from being "optimised" back out:
```rust
    // a LISTABLE game NOT in the curated set → 404: a curated token cannot
    // enumerate the catalog (spec §2's deliberate tightening).
    store.put_game(&test_game(4)).await.unwrap();
    let tight = plain_router(Arc::clone(&store), mock2)
        .oneshot(Request::get(format!("/api/l/cur-d/games/{}/detail", test_game(4).id))
            .body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(tight.status(), StatusCode::NOT_FOUND);
```
(`mock2`: `let mock2 = mock.clone();` before the previous router build consumes `mock` —
`MockInvoker` is `Clone`; the existing tests clone it the same way.)

- [ ] **Step 4: Run:** `DYNAMODB_LOCAL_URL=http://localhost:8000 cargo test -p public-api --test api_test 2>&1 | tail -5; echo "exit=${PIPESTATUS[0]}"` → green.

- [ ] **Step 5: Commit:** `git add -A && git commit -S -m "feat(public-api): claim gate refuses out-of-set ids; detail gate mirrors the live grid"`

---

### Task 4: admin-api — create accepts and validates the set

**Files:**
- Modify: `crates/admin-api/src/lib.rs` (`CreateLinkBody` :604-614, `validate` :616-640, `handle_create_link` :643-705, Link literal :678-693; bounds consts :556-559)
- Modify: `crates/dynamo/src/lib.rs` (doc-comment on `update_link_meta` :652-658 ONLY — no behavior change)
- Test: `crates/admin-api/tests/api_test.rs`

**Interfaces:**
- Consumes: `Link.curated_game_ids`, `Store::batch_get_games` (Task 1 / existing).
- Produces: `POST /admin/api/links` accepts `"game_ids": ["…"]` (optional). 422 error strings the web (Task 6) surfaces verbatim: they must NAME the offending field/ids.

- [ ] **Step 1: Write the failing tests** (copy the shape of `create_link_absurd_expires_days_returns_422_not_panic` :485; helpers per :28-:138):

```rust
#[tokio::test]
async fn cur_create_stores_pick_order() {
    let Some(store) = store_or_skip("cur-create-ok").await else { return };
    let password = "valpw";
    let admin_hash = test_admin_hash(password);
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new();
    let session = admin_login(&store, &invoker, &admin_hash, password).await;
    for n in 1..=3 { store.put_game(&test_game(n)).await.unwrap(); }
    let resp = post_create_link(&store, &invoker, &admin_hash, &session,
        serde_json::json!({"label": "for maya", "claims_allowed": 2,
            "game_ids": [test_game(3).id, test_game(1).id]})).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let j = body_json(resp).await;
    let token = j["token"].as_str().unwrap();
    let stored = store.get_link(token).await.unwrap().unwrap();
    assert_eq!(stored.curated_game_ids.as_deref(),
        Some(&[test_game(3).id, test_game(1).id][..]), "pick order stored verbatim");
}

#[tokio::test]
async fn cur_create_unknown_game_id_is_422_naming_it() {
    let Some(store) = store_or_skip("cur-create-unknown").await else { return };
    let password = "valpw";
    let admin_hash = test_admin_hash(password);
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new();
    let session = admin_login(&store, &invoker, &admin_hash, password).await;
    store.put_game(&test_game(1)).await.unwrap();
    let resp = post_create_link(&store, &invoker, &admin_hash, &session,
        serde_json::json!({"label": "x", "claims_allowed": 1,
            "game_ids": [test_game(1).id, "ghost-id"]})).await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let j = body_json(resp).await;
    assert!(j["error"].as_str().unwrap().contains("ghost-id"),
        "error must name the unknown id, got: {j}");
}

#[tokio::test]
async fn cur_create_unlistable_game_is_422_naming_it() {
    let Some(store) = store_or_skip("cur-create-unlist").await else { return };
    let password = "valpw";
    let admin_hash = test_admin_hash(password);
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new();
    let session = admin_login(&store, &invoker, &admin_hash, password).await;
    let mut g = test_game(1); g.hidden = true;
    store.put_game(&g).await.unwrap();
    let resp = post_create_link(&store, &invoker, &admin_hash, &session,
        serde_json::json!({"label": "x", "claims_allowed": 1, "game_ids": [g.id]})).await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let j = body_json(resp).await;
    assert!(j["error"].as_str().unwrap().contains(&g.id));
}

#[tokio::test]
async fn cur_create_422_arms_empty_dupes_overpromise_toolarge() {
    let Some(store) = store_or_skip("cur-create-arms").await else { return };
    let password = "valpw";
    let admin_hash = test_admin_hash(password);
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new();
    let session = admin_login(&store, &invoker, &admin_hash, password).await;
    store.put_game(&test_game(1)).await.unwrap();
    // empty
    let r = post_create_link(&store, &invoker, &admin_hash, &session,
        serde_json::json!({"label": "x", "claims_allowed": 1, "game_ids": []})).await;
    assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body_json(r).await["error"].as_str().unwrap().contains("game_ids"));
    // duplicate
    let r = post_create_link(&store, &invoker, &admin_hash, &session,
        serde_json::json!({"label": "x", "claims_allowed": 1,
            "game_ids": [test_game(1).id, test_game(1).id]})).await;
    assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body_json(r).await["error"].as_str().unwrap().contains("duplicate"));
    // claims_allowed > set size
    let r = post_create_link(&store, &invoker, &admin_hash, &session,
        serde_json::json!({"label": "x", "claims_allowed": 2,
            "game_ids": [test_game(1).id]})).await;
    assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body_json(r).await["error"].as_str().unwrap().contains("claims_allowed"));
    // set larger than CURATED_GAMES_MAX — every 422 arm gets its own named
    // assertion (spec testing rule; the cap is kept deliberately: the claims
    // cap does NOT bound the set size, and an unbounded admin array is still
    // an unbounded array).
    let big: Vec<String> = (0..101).map(|i| format!("id-{i}")).collect();
    let r = post_create_link(&store, &invoker, &admin_hash, &session,
        serde_json::json!({"label": "x", "claims_allowed": 1, "game_ids": big})).await;
    assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body_json(r).await["error"].as_str().unwrap().contains("at most"));
}
```

- [ ] **Step 2: Run to verify failure:** `DYNAMODB_LOCAL_URL=http://localhost:8000 cargo test -p admin-api --test api_test cur_create 2>&1 | tail -8; echo "exit=${PIPESTATUS[0]}"` → the happy-path test fails (game_ids silently ignored → stored None).

- [ ] **Step 3: Implement.**

Bounds (:559): `const CURATED_GAMES_MAX: usize = 100;`

`CreateLinkBody` add `game_ids: Option<Vec<String>>,`. `validate()` — append before `parse_gift_note`:
```rust
        if let Some(ids) = &self.game_ids {
            if ids.is_empty() {
                return Err("game_ids must not be empty when provided — omit it for an open-shelf link".into());
            }
            if ids.len() > CURATED_GAMES_MAX {
                return Err(format!("game_ids must be at most {CURATED_GAMES_MAX} games"));
            }
            let mut seen = std::collections::HashSet::new();
            if let Some(dup) = ids.iter().find(|id| !seen.insert(id.as_str())) {
                return Err(format!("game_ids contains a duplicate: {dup}"));
            }
            if self.claims_allowed as usize > ids.len() {
                return Err(format!(
                    "claims_allowed ({}) exceeds the {} curated games — the link would promise more than it can deliver",
                    self.claims_allowed, ids.len()
                ));
            }
        }
```
In `handle_create_link`, after `body.validate()` (:647) and before the Link literal — the async arms:
```rust
    // Store-backed validation: every curated id must exist and be listable NOW.
    // (It can stop being listable later — the friend surface ghosts it; spec §2.)
    if let Some(ids) = &body.game_ids {
        let found = match s.store.batch_get_games(ids).await {
            Ok(m) => m,
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
        let unknown: Vec<&str> = ids.iter()
            .filter(|id| !found.contains_key(*id))
            .map(String::as_str).collect();
        if !unknown.is_empty() {
            return unprocessable(format!("unknown game ids: {}", unknown.join(", ")));
        }
        let unlistable: Vec<&str> = ids.iter()
            .filter(|id| found.get(*id).is_some_and(|g| !g.is_listable()))
            .map(String::as_str).collect();
        if !unlistable.is_empty() {
            return unprocessable(format!("not claimable right now: {}", unlistable.join(", ")));
        }
    }
```
Link literal (:678-693): replace Task 1's stopgap `curated_game_ids: None` with `curated_game_ids: body.game_ids.clone(),`.

`crates/dynamo/src/lib.rs` — extend `update_link_meta`'s doc-comment (:652-657). **This is a
DEFERRAL, written out loud — deliberately NOT an in-body guard, and deliberately NOT listed as
a covered invariant** (family sign-off, final round): no endpoint edits `claims_allowed` today
(revoke is the sole caller and carries it untouched), so there is nothing for a guard to catch —
and an in-store check would make REVOKE the hostage: the one caller is the one safety operation
that must never be refused, and a hand-drifted record would brick its own revoke button over an
unrelated invariant.
```rust
    /// DEFERRED INVARIANT (spec §4) — enforced at CREATE (422), NOT here: on a
    /// curated link, claims_allowed <= set length. No endpoint edits
    /// claims_allowed today (revoke is this fn's only caller and never moves
    /// the number). WHOEVER ADDS a claims_allowed editor owns re-checking it
    /// with a 422 — and must NOT add the check here: this fn's callers
    /// include revoke, which must never be refused over an unrelated
    /// invariant (a drifted record still gets to be revoked).
```

- [ ] **Step 4: Run:** `DYNAMODB_LOCAL_URL=http://localhost:8000 cargo test -p admin-api --test api_test 2>&1 | tail -5; echo "exit=${PIPESTATUS[0]}"` → green.
- [ ] **Step 5: Full rust gates** (pipefail so a failing suite cannot hide behind `tail`): `set -o pipefail && export DYNAMODB_LOCAL_URL=http://localhost:8000 && cargo fmt --check && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo test --workspace --no-fail-fast 2>&1 | tail -5` → exit 0, green.
- [ ] **Step 6: Commit:** `git add -A && git commit -S -m "feat(admin-api): create accepts game_ids — six 422 arms, pick order stored verbatim"`

---

### Task 5: web — api.ts types + adminCreateLink arg

**Files:**
- Modify: `web/src/api.ts` (:2-40 types, :117-135 AdminLink, :373-410 adminCreateLink)
- Modify: `web/src/admin/Links.test.tsx` (every `adminCreateLink` positional assertion — lines ~323, ~350, and the seal/note tests)
- Test: existing suites keep passing (this task changes no behavior beyond the new optional arg)

**Interfaces:**
- Consumes: wire fields from Tasks 2/4.
- Produces (Tasks 6-8 import these): `GameView.gone?: boolean` · `LinkView.curated?: boolean` · `AdminLink.curated_game_ids?: string[]` · `adminCreateLink(label, claims, expiresDays?, giftNote?, unlockAt?, gameIds?)` sending `game_ids` in the JSON body.

- [ ] **Step 0: Write the failing body-key test** (in `web/src/api.test.ts`, which already
stubs fetch via `vi.stubGlobal('fetch', mockFetch)` (:36) — follow that file's existing
adminCreateLink test shape for the mock's resolved Response):
```ts
it('adminCreateLink sends game_ids as the request-body key', async () => {
  // The wire key is the contract with admin-api's CreateLinkBody — a typo'd
  // key here ships the whole feature open-shelf-only with every task green
  // (OMBB, sign-off): downstream tests all mock this fn away.
  mockFetch.mockResolvedValueOnce(okJson({ token: 't', url_path: '/l/t' }));
  await adminCreateLink('l', 1, undefined, undefined, undefined, ['g-2', 'g-1']);
  const [, init] = mockFetch.mock.calls.at(-1)!;
  const body = JSON.parse((init as RequestInit).body as string);
  expect(body.game_ids).toEqual(['g-2', 'g-1']);
});
```
(Executor: `okJson` — use whatever response-builder helper the file's existing tests use;
grep the first adminCreateLink test and copy its mock line verbatim. Run it: FAILS —
`body.game_ids` is `undefined` before Step 1 adds the param.)

- [ ] **Step 1: Make the type/arg changes.**
`GameView`: add `/** ghost marker (curated links): a chosen game in a decided non-listable state. cause-blind by decision (spec §2). */ gone?: boolean;`
`LinkView`: add `/** true iff this link carries a curated set (absent = open shelf). */ curated?: boolean;`
`AdminLink`: add `/** ben's pick order; absent = open shelf. */ curated_game_ids?: string[];`
`adminCreateLink`: append 6th param `gameIds?: string[]` and `game_ids: gameIds,` in the body literal.

- [ ] **Step 2: Update every existing `toHaveBeenCalledWith` for adminCreateLink** in `Links.test.tsx` to append `undefined` as the 6th arg. Find them all: `grep -n "adminCreateLink).toHaveBeenCalledWith" web/src/admin/Links.test.tsx` — update each. (Consistency housekeeping, not load-bearing: vitest equality treats trailing explicit `undefined` and an absent arg the same. Do it anyway so the assertions document the real call shape.)

- [ ] **Step 3: Run:** `cd web && npm run typecheck && npm test -- --run 2>&1 | tail -5` → green.

- [ ] **Step 4: Commit:** `git add -A && git commit -S -m "feat(web/api): curated + gone wire fields, adminCreateLink gains gameIds"`

---

### Task 6: web — Links page: chips at create, count chip in the list

**Files:**
- Modify: `web/src/admin/Links.tsx` (form state :67-78, handleCreate :111-159, form JSX :327-410, list row :437-478)
- Test: `web/src/admin/Links.test.tsx`

**Interfaces:**
- Consumes: `adminCreateLink(...gameIds)` (Task 5).
- Produces (and DEFINES): the `picked` router-state contract — `location.state.picked: { id: string; title: string }[]`. Task 7 sends this exact shape LATER; this task's tests inject it directly via MemoryRouter initialEntries, so there is no forward dependency at execution time.

- [ ] **Step 1: Write the failing tests** (partial-mock pattern per :10-33; `renderLinks` helper — extend it to accept an `initialEntries` override so router state can be injected):

```tsx
// sits beside the file's existing renderLinks() helper, same shape, one addition
function renderLinksWithPicks(picked: { id: string; title: string }[]) {
  return render(
    <MemoryRouter initialEntries={[{ pathname: '/admin/links', state: { picked } }]}>
      <Routes>
        <Route path="/admin/links" element={<Links />} />
        <Route path="/admin/login" element={<div>login page</div>} />
      </Routes>
    </MemoryRouter>,
  );
}

it('shows picked games as chips and sends their ids in pick order', async () => {
  const user = userEvent.setup();
  vi.mocked(adminLinks).mockResolvedValue([]);
  vi.mocked(adminCreateLink).mockResolvedValue({ token: 't1', url_path: '/l/t1' });
  renderLinksWithPicks([{ id: 'g-3', title: 'Celeste' }, { id: 'g-1', title: 'Hades' }]);
  await waitFor(() => screen.getByText('Celeste'));
  await user.type(screen.getByRole('textbox', { name: 'label' }), 'for maya');
  await user.click(screen.getByRole('button', { name: /create invite link/i }));
  await waitFor(() => {
    expect(adminCreateLink).toHaveBeenCalledWith(
      'for maya', 1, undefined, undefined, undefined, ['g-3', 'g-1'],
    );
  });
});

it('removes a chip and reorders with the arrow buttons', async () => {
  const user = userEvent.setup();
  vi.mocked(adminLinks).mockResolvedValue([]);
  vi.mocked(adminCreateLink).mockResolvedValue({ token: 't1', url_path: '/l/t1' });
  renderLinksWithPicks([
    { id: 'g-1', title: 'Hades' }, { id: 'g-2', title: 'Celeste' }, { id: 'g-3', title: 'Ori' },
  ]);
  await waitFor(() => screen.getByText('Ori'));
  await user.click(screen.getByRole('button', { name: 'remove Celeste from this gift' }));
  expect(screen.queryByText('Celeste')).toBeNull();
  await user.click(screen.getByRole('button', { name: 'move Ori earlier' }));
  await user.type(screen.getByRole('textbox', { name: 'label' }), 'x');
  await user.click(screen.getByRole('button', { name: /create invite link/i }));
  await waitFor(() => {
    expect(adminCreateLink).toHaveBeenCalledWith(
      'x', 1, undefined, undefined, undefined, ['g-3', 'g-1'],
    );
  });
});

it('clears the picks after a successful create and confirms the wrapped titles', async () => {
  const user = userEvent.setup();
  vi.mocked(adminLinks).mockResolvedValue([]);
  vi.mocked(adminCreateLink).mockResolvedValue({ token: 't1', url_path: '/l/t1' });
  renderLinksWithPicks([{ id: 'g-1', title: 'Hades' }]);
  await waitFor(() => screen.getByText('Hades'));
  await user.type(screen.getByRole('textbox', { name: 'label' }), 'x');
  await user.click(screen.getByRole('button', { name: /create invite link/i }));
  // chips gone (exact-text query misses the longer confirmation string)…
  await waitFor(() => expect(screen.queryByText('Hades')).toBeNull());
  // …but the success card confirms what got wrapped (spec §4: the
  // confirmation lives at create time, at zero fetch cost).
  expect(screen.getByText('wrapped: Hades')).toBeInTheDocument();
});

it('shows a chosen-count chip on curated links in the list', async () => {
  vi.mocked(adminLinks).mockResolvedValue([
    { ...link1, token: 'a', label: 'open one' },
    { ...link1, token: 'b', label: 'curated one', curated_game_ids: ['g1', 'g2', 'g3'] },
  ]);
  renderLinks();
  await waitFor(() => screen.getByText('curated one'));
  expect(screen.getByText('3 chosen')).toBeInTheDocument();
  expect(screen.queryAllByText(/chosen/)).toHaveLength(1);
});
```
(Executor: the file's fixtures are `link1`/`link2` at Links.test.tsx:46-64 — spread `link1` as above. There is no `baseAdminLink`.)

- [ ] **Step 2: Run to verify failure:** `cd web && npx vitest run src/admin/Links.test.tsx 2>&1 | tail -10` → new tests fail (no chips rendered).

- [ ] **Step 3: Implement in `Links.tsx`.**
Add `useLocation` to the existing `react-router-dom` import line. State (after :78):
```tsx
  const location = useLocation();
  // picks arrive from the catalog's "wrap these into a link" (router state) —
  // order is ben's pick order and is the order sent to the api.
  const [picked, setPicked] = useState<{ id: string; title: string }[]>(
    () => (location.state as { picked?: { id: string; title: string }[] } | null)?.picked ?? [],
  );
```
handleCreate: `const gameIds = picked.length > 0 ? picked.map((p) => p.id) : undefined;` → pass as 6th arg. In `.then`: extend `createdInfo` with the chosen titles (captured from the `picked` closure BEFORE the reset) and add `setPicked([]);` beside the other resets —
```tsx
      setCreatedInfo({
        fullUrl: inviteUrl(result.token),
        label: trimmedLabel,
        chosen: picked.map((p) => p.title),
      });
      // …existing resets…
      setPicked([]);
```
Widen the state type: `createdInfo: { fullUrl: string; label: string; chosen?: string[] } | null`. In the created-card JSX (find the block rendering `createdInfo.fullUrl`), add:
```tsx
        {createdInfo.chosen !== undefined && createdInfo.chosen.length > 0 && (
          <p className="text-xs text-dust">wrapped: {createdInfo.chosen.join(', ')}</p>
        )}
```
Form JSX, between the gift-note label and the submit button:
```tsx
        {picked.length > 0 && (
          <div className="flex flex-col gap-1 text-xs text-dust">
            <span>chosen for this gift ({picked.length})</span>
            <ul className="flex flex-wrap gap-1.5">
              {picked.map((p, i) => (
                <li key={p.id}
                  className="flex items-center gap-1 rounded bg-shelf px-2 py-0.5 text-xs text-ink-soft">
                  {p.title}
                  <button type="button" aria-label={`move ${p.title} earlier`}
                    disabled={i === 0} className="disabled:opacity-40"
                    onClick={() => setPicked((cur) => {
                      const next = [...cur];
                      [next[i - 1], next[i]] = [next[i], next[i - 1]];
                      return next;
                    })}>↑</button>
                  <button type="button" aria-label={`move ${p.title} later`}
                    disabled={i === picked.length - 1} className="disabled:opacity-40"
                    onClick={() => setPicked((cur) => {
                      const next = [...cur];
                      [next[i], next[i + 1]] = [next[i + 1], next[i]];
                      return next;
                    })}>↓</button>
                  <button type="button" aria-label={`remove ${p.title} from this gift`}
                    onClick={() => setPicked((cur) => cur.filter((q) => q.id !== p.id))}>×</button>
                </li>
              ))}
            </ul>
          </div>
        )}
```
List row, beside the seal chip (:465-ish, same flex-wrap header):
```tsx
        {link.curated_game_ids !== undefined && (
          <span className="rounded bg-shelf px-2 py-0.5 text-xs text-ink-soft">
            {link.curated_game_ids.length} chosen
          </span>
        )}
```

- [ ] **Step 4: Run:** `cd web && npx vitest run src/admin/Links.test.tsx 2>&1 | tail -5` → green.
- [ ] **Step 5: Commit:** `git add -A && git commit -S -m "feat(web/admin): create-form pick chips (reorder/remove) + chosen-count chip in the links list"`

---

### Task 7: web — Catalog multi-select → "wrap these into a link"

**Files:**
- Modify: `web/src/admin/Catalog.tsx` (state :58-96, renderRow :360+, toolbar area; `navigate` already in scope at :58)
- Test: `web/src/admin/Catalog.test.tsx`

**Interfaces:**
- Consumes: nothing new.
- Produces: `navigate('/admin/links', { state: { picked: [{ id, title }, …] } })` — EXACTLY the contract Task 6 reads.

- [ ] **Step 1: Write the failing tests** (automock pattern per Catalog.test.tsx:9; `renderCatalog` helper exists — extend its `<Routes>` with `<Route path="/admin/links" element={<div>links page</div>} />` so navigation can land):

```tsx
it('picks games across groups and wraps them into a link in pick order', async () => {
  const user = userEvent.setup();
  vi.mocked(adminCatalog).mockResolvedValue([
    makeAdminGame({ id: 'g-1', title: 'Hades' }),
    makeAdminGame({ id: 'g-2', title: 'Celeste' }),
  ]);
  renderCatalog();
  await waitFor(() => screen.getByText('Hades'));
  await user.click(screen.getByRole('checkbox', { name: 'pick Celeste for a link' }));
  await user.click(screen.getByRole('checkbox', { name: 'pick Hades for a link' }));
  await user.click(screen.getByRole('button', { name: 'wrap these into a link (2)' }));
  await waitFor(() => screen.getByText('links page'));
});

it('pick toggles off and the wrap bar hides when empty', async () => {
  const user = userEvent.setup();
  vi.mocked(adminCatalog).mockResolvedValue([
    makeAdminGame({ id: 'g-1', title: 'Hades' }),
  ]);
  renderCatalog();
  await waitFor(() => screen.getByText('Hades'));
  const box = screen.getByRole('checkbox', { name: 'pick Hades for a link' });
  await user.click(box);
  expect((box as HTMLInputElement).checked).toBe(true);
  await user.click(box);
  expect(screen.queryByRole('button', { name: /wrap these into a link/i })).toBeNull();
});
```
(Executor: Catalog.test.tsx has NO factory — only const fixtures, with `gameFixture` as the spread base (:26). Define at the top of the test file:
```tsx
const makeAdminGame = (o: Partial<AdminGame> & { id: string; title: string }): AdminGame => ({
  ...gameFixture,
  giftable: true,
  hidden: false,
  ...o,
});
```
The navigation assertion goes through a real `<Route>`, not a navigate mock — the automock covers `../api` only. The ORDER pin is code, not prose — use this probe as the `/admin/links` route element:
```tsx
function PickProbe() {
  const { state } = useLocation() as { state: { picked: { id: string; title: string }[] } };
  return <div>{state.picked.map((p) => p.title).join('|')}</div>;
}
```
and end the first test with `await waitFor(() => screen.getByText('Celeste|Hades'));` — pick order, asserted across the navigation boundary.)

- [ ] **Step 2: Run to verify failure:** `cd web && npx vitest run src/admin/Catalog.test.tsx 2>&1 | tail -8`.

- [ ] **Step 3: Implement in `Catalog.tsx`.**
State: `const [picked, setPicked] = useState<{ id: string; title: string }[]>([]);` — array IS the order; membership check `picked.some((p) => p.id === game.id)`.
In `renderRow`, as a SIBLING control in the flex row (the row div must NOT be the control — the :373 comment is load-bearing; copy the hidden-switch label shape at :482). Membership derives from the id-keyed `picked` array, so the duplicated rows grouping creates stay in sync BY CONSTRUCTION — no per-row state to reconcile:
```tsx
        <label className="flex cursor-pointer items-center gap-1.5">
          <input type="checkbox"
            aria-label={`pick ${game.title} for a link`}
            checked={picked.some((p) => p.id === game.id)}
            onChange={() => setPicked((cur) =>
              cur.some((p) => p.id === game.id)
                ? cur.filter((p) => p.id !== game.id)
                : [...cur, { id: game.id, title: game.title }],
            )}
            className="h-4 w-4 cursor-pointer accent-give" />
          <span className="text-xs text-dust">pick</span>
        </label>
```
Toolbar (above the groups, near the toolkit controls), only when picks exist:
```tsx
      {picked.length > 0 && (
        <div className="flex items-center gap-2">
          <button type="button"
            onClick={() => navigate('/admin/links', { state: { picked } })}
            className="rounded bg-control px-4 py-2 text-sm hover:bg-control-bright">
            wrap these into a link ({picked.length})
          </button>
          <button type="button" onClick={() => setPicked([])}
            className="text-xs text-dust hover:text-ink-soft">
            clear picks
          </button>
        </div>
      )}
```
(Only listable games are pickable? NO — keep it simple and honest: the checkbox renders on every row, and create-time 422 names any unlistable pick; the admin sees exactly what the API refuses. Note this in a one-line code comment.)

- [ ] **Step 4: Run:** `cd web && npx vitest run src/admin/Catalog.test.tsx 2>&1 | tail -5` → green.
- [ ] **Step 5: Commit:** `git add -A && git commit -S -m "feat(web/admin): catalog multi-select — pick across groups, wrap into a link"`

---

### Task 8: web — the curated unwrap (friend surface)

**Files:**
- Modify: `web/src/friend/LinkPage.tsx` (DIALOG_BODY :63-84, shuffle :275-300, GameGrid call :555-561)
- Modify: `web/src/friend/GameGrid.tsx` (props :6-27, dedupe :29-60, card key :118, chip :96-100)
- Test: `web/src/friend/LinkPage.test.tsx`, `web/src/friend/GameGrid.test.tsx`

**Interfaces:**
- Consumes: `LinkView.curated?`, `GameView.gone?` (Task 5).
- Produces: nothing downstream.

- [ ] **Step 1: Write the failing tests.**

`GameGrid.test.tsx`:
```tsx
it('curated: keeps server order, skips dedupe, keys by id', () => {
  const games = [
    makeGame({ id: 'a', title: 'Twin' }),
    makeGame({ id: 'b', title: 'Twin' }), // two copies picked on purpose = two cards
    makeGame({ id: 'c', title: 'Solo' }),
  ];
  render(<GameGrid games={games} curated onDetail={vi.fn()} />);
  expect(screen.queryByText(/copies/)).toBeNull();
  expect(screen.getAllByText('Twin')).toHaveLength(2);
});

it('curated: a gone game renders as a non-interactive ghost', async () => {
  const onDetail = vi.fn();
  const games = [
    makeGame({ id: 'a', title: 'Kept' }),
    makeGame({ id: 'b', title: 'Gone', gone: true }),
  ];
  render(<GameGrid games={games} curated onDetail={onDetail} />);
  expect(screen.getByText("this one's spoken for")).toBeInTheDocument();
  // the ghost offers no details control; the live card still does
  expect(screen.getAllByRole('button', { name: /details/i })).toHaveLength(1);
});

it('open shelf: dedupe and copies chip unchanged', () => {
  const games = [
    makeGame({ id: 'a', title: 'Twin' }),
    makeGame({ id: 'b', title: 'Twin' }),
  ];
  render(<GameGrid games={games} onDetail={vi.fn()} />);
  expect(screen.getByText('×2 copies')).toBeInTheDocument();
});
```

`LinkPage.test.tsx` (fixture `baseLink` :51-58). LinkPage.test.tsx has NO `makeGame` and NO
`mockGetLink` helper — copy `makeGame` VERBATIM from GameGrid.test.tsx:7 (it is a local const
there, not exported), and mock the link fetch exactly the way the file's existing tests do:
the idiom is `vi.mocked(<the api fn the file's vi.mock block stubs — check its name at the
top of the file>).mockResolvedValue(...)`, referred to as `mockLink(...)` below — define it as
a two-line local helper wrapping that idiom.
```tsx
it('curated link renders games in server order without shuffling', async () => {
  // 4 games: a preserved order under the open-shelf shuffle would be a 1/24
  // coincidence — this asserts the shuffle is BYPASSED, not merely lucky.
  mockLink({
    ...baseLink,
    curated: true,
    games: [
      makeGame({ id: 'g-3', title: 'Ccc' }),
      makeGame({ id: 'g-1', title: 'Aaa' }),
      makeGame({ id: 'g-4', title: 'Ddd' }),
      makeGame({ id: 'g-2', title: 'Bbb' }),
    ],
  });
  renderLinkPage();
  await waitFor(() => screen.getByText('Ccc'));
  const labels = screen
    .getAllByRole('button', { name: /details/i })
    .map((b) => b.getAttribute('aria-label'));
  expect(labels).toEqual([
    'Ccc — details', 'Aaa — details', 'Ddd — details', 'Bbb — details',
  ]);
});

it('curated link swaps the dialog body copy', async () => {
  mockLink({ ...baseLink, curated: true, games: [makeGame({ id: 'g-1' })] });
  renderLinkPage();
  await waitFor(() => screen.getByText(/picked these out just for you/));
});
```
(Executor: if the details-button aria-label format differs from `"${title} — details"`, read
GameGrid.tsx's button and match it — the assertion's substance is exact ORDER equality.)

- [ ] **Step 2: Run to verify failure:** `cd web && npx vitest run src/friend 2>&1 | tail -8`.

- [ ] **Step 3: Implement.**

`GameGrid.tsx`: props gain `/** curated link: server order is ben's pick order — no shuffle upstream, no dedupe here. */ curated?: boolean;`. Entry building:
```tsx
  // Curated: ben picked ids, not titles — two copies of one title are two
  // gifts (spec §5). Dedupe is the open-shelf storefront affordance only.
  const entries = curated
    ? games.map((game) => ({ game, count: 1 }))
    : dedupedByTitle(games);
```
(refactor the existing Map loop into `dedupedByTitle` or branch inline — match the file's style; card `key={curated ? game.id : game.title}`.)
Small-set layout (spec §5): the curated section caps its columns at the entry count so 1-2
games don't rattle around a 3-column frame —
```tsx
  <section className={curated
    ? `grid grid-cols-1 gap-4 p-6 ${entries.length >= 2 ? 'sm:grid-cols-2' : ''} ${entries.length >= 3 ? 'lg:grid-cols-3' : ''}`
    : 'grid grid-cols-1 gap-4 p-6 sm:grid-cols-2 lg:grid-cols-3'}>
```
(open shelf keeps today's literal class string, byte-stable; no test — presentation only.)

Ghost rendering in the card (where the details button/card body renders): when `game.gone`,
```tsx
    // A ghost is an acknowledgment, not a control: dimmed art, neutral copy,
    // no button — the gate 404s it anyway (the surface mirrors the boundary).
```
render the art/title block in a plain `div` (not the button), `className` additions `opacity-50 grayscale`, and instead of the details button:
```tsx
    <span className="rounded bg-floor px-2 py-0.5 text-xs text-dust">this one's spoken for</span>
```

`LinkPage.tsx`:
- Hoist to module scope above the component:
```tsx
const DIALOG_BODY_SHELF =
  "games from ben's humble stash, picked for you ♡ open one for details, claim it, and the key is yours.";
const DIALOG_BODY_CURATED =
  "ben picked these out just for you ♡ open one for details, claim it, and the key is yours.";
```
- In the component: `const DIALOG_BODY = view.kind === 'loaded' && view.data.curated === true ? DIALOG_BODY_CURATED : DIALOG_BODY_SHELF;` (both referents are module constants, so the `useMemo([DIALOG_BODY])` dep stays stable per mode; the existing `playedKeyRef` snap logic already handles a mid-session body swap by snapping, not retyping — do not touch it).
- Shuffle bypass (:280-300): first line of the `useMemo`:
```tsx
    if (view.kind !== 'loaded') return [];
    if (view.data.curated === true) {
      // ben's pick order IS the presentation order (spec §5). Do not populate
      // the shuffle ranks: a mode flip must not inherit stale ranks.
      return view.data.games;
    }
```
- GameGrid call (:556-561): `<GameGrid games={shelfGames} curated={view.data.curated === true} owned={ownedSet} onDetail={setDetailGame} />` — a primitive prop, memo-safe.
- The beacon (:383-398) is UNTOUCHED (spec §5: claims-remaining is the true count).

- [ ] **Step 4: Run the full web gates:** `cd web && npm run lint && npm run typecheck && npm test -- --run 2>&1 | tail -5` → green.
- [ ] **Step 5: Commit:** `git add -A && git commit -S -m "feat(web/friend): the curated unwrap — ben's order, no dedupe, honest ghosts, personal dialog copy"`

---

### Task 9: full verification + spec flip + PR

**Files:**
- Modify: `docs/superpowers/specs/2026-08-19-chosen-for-you-design.md` (status line only: draft → accepted)

- [ ] **Step 1: Full rust + web gates, one chain, dynamodb-local running:**
```bash
set -o pipefail \
  && export DYNAMODB_LOCAL_URL=http://localhost:8000 \
  && cargo fmt --check \
  && cargo clippy --workspace --all-targets --all-features -- -D warnings \
  && cargo test --workspace --no-fail-fast 2>&1 | tail -3 \
  && (cd web && npm run lint && npm run typecheck && npm test -- --run && npm run build) 2>&1 | tail -3
```
Expected: every gate green; confirm the new store-backed tests RAN (grep the output for `curated` test names, assert none say SKIP).

- [ ] **Step 2: Flip the spec status line** to `status: accepted (family-reviewed 2026-08-19); implemented on this branch` and commit: `git add -A && git commit -S -m "docs: chosen-for-you spec accepted"`

- [ ] **Step 3: Push, finalize the body, mark the draft ready** (the draft PR exists since Task 1):
```bash
git push
cat > /tmp/chosen-for-you-pr.md <<'EOF'
PRODUCT.md's design principle 2 — "Chosen-for-you, never shopping" — was never implemented: every friend on every link saw ben's whole listable catalog. a link now carries the exact games ben picked when he wrapped it.

- **domain/storage**: `curated_game_ids: Option<Vec<String>>` on `Link`, stored as a top-level order-preserving `L` attribute — an enforcement field per dynamo's own doctrine, structurally immune to `SET body = :b` erasure by a rolled-back binary. recoverable-and-loud, never unrecoverable-and-silent; the rollback is pinned by test.
- **public-api**: curated links partition their set into live/ghost cards in ben's pick order (`gone: true`, cause-blind by decision). `live_on_link` is ONE computation with two callers (grid partition + detail gate); fallout, pinned as intentional: a curated token can no longer enumerate catalog details. the claim endpoint 409s out-of-set ids — the surface is not the boundary.
- **admin**: create accepts `game_ids` behind six named 422 arms; the catalog grows multi-select → "wrap these into a link"; the create form shows reorderable pick chips; the success card lists the wrapped titles.
- **friend**: no shuffle on curated links — ben's order is the presentation order; honest ghost cards ("this one's spoken for"); personal dialog copy. the gifts-waiting beacon is deliberately unchanged (claims-remaining is the true count).
- **deploy note**: all lambdas ship with this release. a stale/rolled-back public-api serves the open shelf on a curated link and gates nothing for that window — data survives, the promise degrades visibly, and it self-heals on redeploy.

spec: `docs/superpowers/specs/2026-08-19-chosen-for-you-design.md` (three family-review rounds)
plan: `docs/superpowers/plans/2026-08-19-chosen-for-you.md` (adversarially reviewed pre-execution)
EOF
gh pr edit -R yourcodekitten/bendobundles --body-file /tmp/chosen-for-you-pr.md
gh pr ready -R yourcodekitten/bendobundles
```

- [ ] **Step 4: Watch CI to green:** `gh pr checks --watch` — and confirm in the run log that the new store-backed test NAMES appear as executed (they SKIP on the local box by design; CI is their only receipt). Also confirm the spec+plan docs ride the PR diff: `git log --oneline origin/main..HEAD -- docs/superpowers | head` — they were committed during planning, before any task ran; no task re-commits them.

---

## Self-review notes (write-time; adversarial review integrated 2026-08-19T07:5x-04:00)

The pre-execution adversarial review (cold reader, full source verification) returned READY
AFTER FIXES; all 4 blockers, 7 majors, and the actionable minors are integrated above — the
ghost-test seeding (Pending is live, seed `Gifted` instead), `captured_request()` not
`.requests()`, real fixture code for `makeAdminGame`/`mockLink`/`makeGame`, `DYNAMODB_LOCAL_URL`
on every store-backed command (skip-green is the most misleading signal an executor can get),
pipefail on gate chains, the `cur_create` filter fix, the sixth 422 arm, the receipt-arm
subsumption statement, the real PR body, and the small-set column cap.

- Spec §4 admin "reorderable chips" → Task 6 (up/down buttons). §4 "links list count chip" → Task 6. §4 catalog picker → Task 7. §2 partition/ghost/Pending → Tasks 2, 3, 8. §1 storage + pins → Task 1. §3 claim gate → Task 3. §5 friend presentation → Task 8 (beacon deliberately untouched, spec corrected 2026-08-19). §6 out-of-scope list → no task, correct.
- Type consistency: `curated_game_ids` (rust + AdminLink), `curated`/`gone` (wire + TS), `picked: {id,title}[]` (Tasks 6↔7 contract), `gameIds` 6th positional (Tasks 5↔6).
- Known intentional gaps: no edit endpoint (spec §6), no reservation (spec §6), friend-side filters (spec §6).
