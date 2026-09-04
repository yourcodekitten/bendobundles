# The Gift Shelf Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A persistent per-friend page (`/s/{shelf_token}`) showing every game ben ever gave that friend — art, unwrap year, his note, their thank-you — backed by a lightweight `Friend` entity and an optional `Link.friend_id`.

**Architecture:** Single-table DynamoDB additions (`FRIEND#`/`SHELF#` items, sparse `gsi3` on links), one read-only public endpoint, small admin CRUD, one new friend-surface React page. Zero data migration: serde defaults + sparse GSI.

**Tech Stack:** Rust (axum, aws-sdk-dynamodb), React + react-router, terraform. House test rigs: `store_or_skip` + dynamodb-local, axum `tower::ServiceExt` api tests, vitest for web.

**Spec:** `docs/spec-gift-shelf.md` — the spec's **decisions** sections are binding; where this plan and the spec disagree, the spec wins.

## Global Constraints

- All commits GPG-signed (`git commit -S`), authored `code kitten <yourcodekitten@gmail.com>`.
- Rust: match house style; `cargo fmt` before each commit. Full clippy/test matrix runs in CI on the PR (`pull_request` trigger only — do not block on local linker; keep local runs to the narrowest test scope that proves the step).
- Store tests use `store_or_skip` (SKIP locally without dynamodb-local; CI runs them). A skipped-local test still gets written first and read for correctness.
- Copy/voice: lowercase, warm, attic register (see PRODUCT.md; the ♡ is canon).
- Shelf shows `ClaimState::Fulfilled` claims only. Field name is `unwrapped_at` everywhere.
- 404 for unknown AND revoked shelf tokens must be byte-identical (no oracle).
- Any sub-fetch failure in the shelf handler ⇒ whole 500 (never a partial shelf).

---

### Task 1: Domain — `Friend` record + `Link.friend_id`

**Files:**
- Modify: `crates/domain/src/lib.rs` (add `Friend` near `Link`; add `friend_id` field to `Link`)

**Interfaces:**
- Produces: `domain::Friend { id: String, name: String, shelf_token: String, created_at: OffsetDateTime }` (all pub), serde round-trip stable, rfc3339 timestamps.
- Produces: `Link.friend_id: Option<String>` with `#[serde(default, skip_serializing_if = "Option::is_none")]`.

- [ ] **Step 1: Write the failing tests** (in `crates/domain/src/lib.rs` `#[cfg(test)] mod tests`, alongside the existing serde tests)

```rust
#[test]
fn friend_serde_round_trips() {
    let f = Friend {
        id: "f1".into(),
        name: "sarah".into(),
        shelf_token: "ab".repeat(32),
        created_at: time::macros::datetime!(2026-09-04 12:00 UTC),
    };
    let s = serde_json::to_string(&f).unwrap();
    let back: Friend = serde_json::from_str(&s).unwrap();
    assert_eq!(back.id, f.id);
    assert_eq!(back.shelf_token, f.shelf_token);
}

#[test]
fn link_friend_id_defaults_none_on_missing() {
    // A pre-field stored record must deserialize (the zero-migration guarantee).
    let json = serde_json::to_string(&link()).unwrap(); // `fn link()` — the existing fixture at domain lib.rs:535
    let stripped: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(stripped.get("friend_id").is_none(), "None must not serialize");
    let back: Link = serde_json::from_str(&json).unwrap();
    assert_eq!(back.friend_id, None);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p domain friend_ -- --nocapture` — Expected: compile FAIL (`Friend` not defined / no field `friend_id`).

- [ ] **Step 3: Implement**

Add to `crates/domain/src/lib.rs` (doc comments in house voice — say what's authoritative where, mirroring `gift_note`'s comment):

```rust
/// A friend — the person behind a shelf. The whole identity system: no auth,
/// no email. `shelf_token` is the bearer capability for `/s/{token}`; it is
/// cleared on revoke (no dead capability at rest) and replaced on reissue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Friend {
    pub id: String,
    pub name: String,
    /// `""` means REVOKED: revoke REMOVEs the top-level attribute and the
    /// read side restores absence as empty (admin renders "no shelf link").
    /// A live token is always 64 hex.
    pub shelf_token: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}
```

On `Link`, next to `gift_note` (same serde shape):

```rust
    /// The friend this link was cut for. Authoritative ONLY in a top-level
    /// dynamo attribute (scoped update via `set_link_friend`), stripped from
    /// the stored `body` blob, overridden on read — the `gift_note` pattern,
    /// MECHANICALLY REQUIRED here: the claim tx writes `SET body = :b` from a
    /// pre-transaction read, so a body-only field is silently reverted by the
    /// friend claiming — while gsi3pk survives, desyncing index from record
    /// (spec, family review; enforced by friend_id_survives_a_claim).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub friend_id: Option<String>,
```

The `link_body` exhaustive destructure in `crates/dynamo/src/schema.rs` now fails to compile — that is Task 2's first step, by design (the type-level strip doing its job). For THIS task's tests to run, add `friend_id: _` is NOT done here; instead run only `-p domain`.

Fix every `Link { .. }` literal in `crates/domain` tests by adding `friend_id: None`.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p domain` — Expected: PASS (workspace-wide build will fail until Task 2; that's expected — do not "fix" schema.rs here).

- [ ] **Step 5: Commit**

```bash
git add crates/domain && git commit -S -m "🎁 domain: Friend record + Link.friend_id (gift_note-pattern field, serde-default for zero migration)"
```

---

### Task 2: Dynamo — schema keys, body strip, friend store ops, sparse gsi3

**Files:**
- Modify: `crates/dynamo/src/schema.rs` (friend/shelf pk helpers; `link_body` gains `friend_id: _`; `link_item` writes `friend_id` + `gsi3pk` when present; new `friend_item` / `shelf_pointer_item`)
- Modify: `crates/dynamo/src/lib.rs` (store ops; `link_from_item` override; `create_table_for_tests` gains gsi3)
- Test: `crates/dynamo/tests/store_test.rs`

**Interfaces:**
- Consumes: `domain::Friend`, `Link.friend_id` (Task 1).
- Produces (exact signatures, all on `Store`):
  - `pub async fn create_friend(&self, f: &Friend) -> Result<(), StoreError>` — transaction: put `FRIEND#<id>/META` + put `SHELF#<token>/META {friend_id}`; both `attribute_not_exists(pk)`.
  - `pub async fn get_friend_by_shelf_token(&self, token: &str) -> Result<Option<Friend>, StoreError>` — pointer get → friend get; either missing ⇒ `None`.
  - `pub async fn get_friend(&self, id: &str) -> Result<Option<Friend>, StoreError>`
  - `pub async fn list_friends(&self) -> Result<Vec<Friend>, StoreError>` — friend items carry
    `gsi1pk = "FRIEND"`, `gsi1sk = <name>` (the LISTABLE membership pattern, different pk value;
    name as sort key gives an ordered list for free); query gsi1.
  - `pub async fn rename_friend(&self, id: &str, name: &str) -> Result<bool, StoreError>` — scoped `SET name`, `attribute_exists(pk)`; false on condition fail.
  - `pub async fn reissue_shelf_token(&self, id: &str, old_token: &str, new_token: &str) -> Result<(), StoreError>` — ONE transaction: delete `SHELF#<old>` (`attribute_exists(pk)`) + put `SHELF#<new>` (`attribute_not_exists(pk)`) + update friend `SET shelf_token = :new`.
  - `pub async fn revoke_shelf_token(&self, id: &str, old_token: &str) -> Result<(), StoreError>` — ONE transaction: delete `SHELF#<old>` + update friend `REMOVE shelf_token`. (Read-side: `friend_from_item` treats missing `shelf_token` as `""`; document that `""` means revoked and admin list renders "no shelf link".)
  - `pub async fn set_link_friend(&self, token: &str, friend_id: Option<&str>) -> Result<bool, StoreError>` — scoped update, `attribute_exists(pk)`: `Some` ⇒ `SET friend_id = :f, gsi3pk = :g` (`:g = "FRIEND#<id>"`); `None` ⇒ `REMOVE friend_id, gsi3pk`. **The two attrs move together, always.**
  - `pub async fn list_links_for_friend(&self, friend_id: &str) -> Result<Vec<Link>, StoreError>` — query `gsi3`, `gsi3pk = FRIEND#<id>`, map via `link_from_item`.
- Produces: `link_from_item` override: `friend_id` from top-level attr (present ⇒ Some, absent ⇒ None — unconditional override like `expires_at`).
- Produces: `link_item` writes top-level `friend_id` + `gsi3pk` when `l.friend_id` is Some (so create-with-friend indexes without a second write).
- Produces: `create_table_for_tests` defines `gsi3` (pk-only, `gsi3pk` S attribute, projection ALL — match gsi2's shape).

- [ ] **Step 1: Fix the type-level strip (compile gate)** — in `schema.rs` `link_body`'s destructure add `friend_id: _` to the stripped set with a comment: `// authoritative top-level only; ALSO a gsi3 key — body copy would desync the index`.

- [ ] **Step 2: Write failing store tests** (append to `store_test.rs`; follow the existing test shape — every test takes `store_or_skip`, builds its own items):

```rust
#[tokio::test]
async fn friend_create_resolve_and_list() {
    let Some(store) = store_or_skip("friend_create").await else { return };
    let f = friend("f1", "sarah", "aa");
    store.create_friend(&f).await.unwrap();
    let got = store.get_friend_by_shelf_token(&f.shelf_token).await.unwrap().unwrap();
    assert_eq!(got.id, "f1");
    assert_eq!(store.list_friends().await.unwrap().len(), 1);
    assert!(store.get_friend_by_shelf_token(&"ff".repeat(32)).await.unwrap().is_none());
}

#[tokio::test]
async fn reissue_kills_old_token_atomically() {
    let Some(store) = store_or_skip("friend_reissue").await else { return };
    let f = friend("f1", "sarah", "aa");
    store.create_friend(&f).await.unwrap();
    let new_tok = "bb".repeat(32);
    store.reissue_shelf_token("f1", &f.shelf_token, &new_tok).await.unwrap();
    assert!(store.get_friend_by_shelf_token(&f.shelf_token).await.unwrap().is_none(), "old token must die");
    let got = store.get_friend_by_shelf_token(&new_tok).await.unwrap().unwrap();
    assert_eq!(got.shelf_token, new_tok, "friend record carries the new token");
}

#[tokio::test]
async fn revoke_leaves_no_capability_at_rest() {
    let Some(store) = store_or_skip("friend_revoke").await else { return };
    let f = friend("f1", "sarah", "aa");
    store.create_friend(&f).await.unwrap();
    store.revoke_shelf_token("f1", &f.shelf_token).await.unwrap();
    assert!(store.get_friend_by_shelf_token(&f.shelf_token).await.unwrap().is_none());
    let raw = store.get_friend("f1").await.unwrap().unwrap();
    assert_eq!(raw.shelf_token, "", "revoked friend must not retain the token");
}

#[tokio::test]
async fn set_link_friend_moves_index_and_survives_round_trip() {
    let Some(store) = store_or_skip("link_friend").await else { return };
    let l = link("t1"); // existing helper; add `friend_id: None` there in Task 1's literal fix
    store.create_link(&l).await.unwrap();
    assert!(store.set_link_friend("t1", Some("f1")).await.unwrap());
    let got = store.get_link("t1").await.unwrap().unwrap();
    assert_eq!(got.friend_id.as_deref(), Some("f1"), "read override");
    assert_eq!(store.list_links_for_friend("f1").await.unwrap().len(), 1, "sparse gsi hit");
    assert!(store.set_link_friend("t1", None).await.unwrap());
    assert!(store.list_links_for_friend("f1").await.unwrap().is_empty(), "unassign leaves the index");
    assert!(!store.set_link_friend("missing", Some("f1")).await.unwrap(), "no upsert-from-nothing");
}
```

Add the `friend` fixture helper next to `game`/`link`:

```rust
fn friend(id: &str, name: &str, tok2: &str) -> Friend {
    Friend { id: id.into(), name: name.into(), shelf_token: tok2.repeat(32),
             created_at: time::macros::datetime!(2026-09-04 12:00 UTC) }
}
```

Add the write-path-revert regression test (the reason the gift_note pattern is mandatory here):

```rust
#[tokio::test]
async fn friend_id_survives_a_claim() {
    let Some(store) = store_or_skip("friend_survives_claim").await else { return };
    // THE EXEMPLAR EXISTS: store_test.rs:231 "the note must survive claim_game's SET body"
    // — copy its claim_game("tok-noted", &game_id("gk1","mn"), "c1", now) construction (:247)
    // and its seeding, swapping gift_note for a set_link_friend("t","f1") assignment. Then:
    // - get_link(t).friend_id must still be Some("f1")
    // - list_links_for_friend("f1") must still return the link
    // A body-only implementation FAILS this test (SET body from a pre-tx read reverts it).
}
```

- [ ] **Step 3: Run to verify failure** — `cargo test -p dynamo friend_ link_friend` (needs dynamodb-local; if absent locally, verify COMPILE failure is the missing methods, and lean on CI for the red→green cycle — note which mode you used in the commit body).

- [ ] **Step 4: Implement** — `schema.rs`: `friend_pk(id) -> "FRIEND#<id>"`, `shelf_pk(token) -> "SHELF#<token>"`, `friend_item` (pk/sk/META, `body` = full serde json of Friend, `gsi1pk = "FRIEND"`, `gsi1sk = <name>` for ordered list), `shelf_pointer_item` (pk/sk/META, attr `friend_id`). `lib.rs`: the eight methods per the Interfaces block, transactions via the existing `transact_write_items` idiom (see `claim_game` ~:1240 for error mapping); `friend_from_item` = parse body then override `shelf_token` from top-level? — NO: friend body is small and rewritten whole by no concurrent writer except rename/reissue/revoke which are scoped updates ⇒ store `name` and `shelf_token` as top-level attrs too and make `body` carry only `{id, created_at}`… **decide simpler: friend item stores NO body blob; all four fields are top-level attrs** (`id`, `name`, `shelf_token`, `created_at` rfc3339 string). Friends are 4 scalar fields; a body blob would just create the copy-drift problem the link pattern exists to fight. `friend_from_item` reads the four attrs (`shelf_token` missing ⇒ `""`).
  `create_table_for_tests`: the existing `gsi` closure (lib.rs:3253) REQUIRES a sort key —
  gsi3 has none, so add a sibling `gsi_pk_only(name, pk)` helper beside it rather than passing a
  dummy sk; define the `gsi3pk` attribute and build `gsi3` hash-key-only. (Do NOT copy gsi2's
  call verbatim — gsi2 has a sort key, gsi3 deliberately doesn't.)

- [ ] **Step 5: Run to verify pass** — `cargo test -p dynamo` (or CI as in Step 3). Also `cargo test -p domain -p dynamo` to prove the workspace compiles again.

- [ ] **Step 6: Commit**

```bash
git add crates/dynamo && git commit -S -m "🎁 dynamo: friend/shelf items, transactional revoke+reissue (no dead capability at rest), set_link_friend moves gsi3pk atomically, sparse gsi3 in the test table"
```

---

### Task 3: public-api — extract detail assembly, add the shelf endpoint

**Files:**
- Modify: `crates/public-api/src/lib.rs`
- Test: `crates/public-api/tests/api_test.rs`

**Interfaces:**
- Consumes: `Store::get_friend_by_shelf_token`, `list_links_for_friend`, `claims_for_link`, `batch_get_games` (Task 2).
- Produces: route `GET /api/s/{token}`; response body:

```json
{ "name": "sarah",
  "gifts": [ { "game_id": "...", "title": "...", "artwork_url": "...|null",
               "unwrapped_at": "2026-01-05T00:00:00Z",
               "gift_note": "...|null", "thank_note": "...|null" } ] }
```

  Gifts sorted `unwrapped_at` ascending (oldest first — the shelf accretes).
- Produces: `async fn assemble_shelf(store, friend) -> Result<ShelfResponse, StoreError>` internal; **detail-assembly extraction**: pull the game+steam-cache join out of `handle_game_detail` into `fn game_detail_payload(...)` ONLY IF the shelf ends up needing the same steam fields — v1 shelf payload above needs `title`/`artwork_url` from `Game` alone, so DO NOT extract yet (YAGNI; the spec's helper note is satisfied by not duplicating: the shelf never touches the steam cache in v1). If review wants richer cards later, that's when the helper is born.
- 404 body/shape identical to `link_not_found_response()` for unknown AND revoked; any store error anywhere ⇒ 500 `{"error": "the shelf slipped — try again"}` (soft voice, no internals).

- [ ] **Step 1: Write failing api tests** — the rig, verified in `api_test.rs`: routers are built
  `plain_router(store, mock)` with `MockInvoker::new(FulfillResponse::GiftUrl{..})`, requests are
  `Request::get("/api/s/x").body(Body::empty()).unwrap()` + `.oneshot(req)`, bodies read via
  `body_json(resp)`. Fixtures `test_link(token)` / `test_game(n)` exist in the file; add beside
  them:

```rust
fn test_friend(id: &str, name: &str, tok2: &str) -> Friend {
    Friend { id: id.into(), name: name.into(), shelf_token: tok2.repeat(32),
             created_at: datetime!(2026-09-04 12:00 UTC) }
}
```

```rust
#[tokio::test]
async fn shelf_unknown_token_404s_byte_identical_to_revoked() {
    let Some(store) = store_or_skip("shelf-404").await else { return };
    let mock = MockInvoker::new(FulfillResponse::GiftUrl { url: "https://x.com/g".into() });
    let req = Request::get("/api/s/no-such-shelf").body(Body::empty()).unwrap();
    let r1 = plain_router(Arc::clone(&store), mock.clone()).oneshot(req).await.unwrap();
    assert_eq!(r1.status(), StatusCode::NOT_FOUND);
    let f = test_friend("f1", "sarah", "aa");
    store.create_friend(&f).await.unwrap();
    store.revoke_shelf_token("f1", &f.shelf_token).await.unwrap();
    let req2 = Request::get(&format!("/api/s/{}", f.shelf_token)).body(Body::empty()).unwrap();
    let r2 = plain_router(store, mock).oneshot(req2).await.unwrap();
    assert_eq!(r2.status(), StatusCode::NOT_FOUND);
    // byte-identical bodies: no oracle
    assert_eq!(body_json(r1).await, body_json(r2).await);
}

#[tokio::test]
async fn shelf_happy_fulfilled_only_no_cross_friend_bleed() {
    let Some(store) = store_or_skip("shelf-filter").await else { return };
    // seed: g1,g2,g3 · f1←t1 (Fulfilled on g1 @2024, Pending on g2) · f2←t2 (Fulfilled on g3)
    // claims: copy the Claim{} literal shape used by this file's ClaimsHistory tests and set
    // state/created_at explicitly (states: Fulfilled / Pending).
    // asserts: status OK (THE CANARY — this line cannot pass without the route existing);
    //   j["name"] == "sarah"; gifts == exactly [g1]; unwrapped_at asc when a second Fulfilled
    //   claim @2026 is added to t1; gift_note/thank_note ride from t1's link record.
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p public-api shelf_` — Expected: FAIL, and
  specifically the happy test's `StatusCode::OK` canary (a missing route trivially satisfies the
  404 asserts; the OK is the assertion that cannot false-pass). Note: axum route-not-found may
  render 404 before compile even fails — the compile failure on `create_friend` etc. imports
  arrives first; both are valid reds.

- [ ] **Step 3: Implement** — route in `router()`: `.route("/api/s/{token}", get(handle_get_shelf))`. Handler: resolve friend (None ⇒ `link_not_found_response()`), `list_links_for_friend`, for each `claims_for_link`, filter `ClaimState::Fulfilled`, collect `game_id`s, ONE `batch_get_games`, build gifts (skip a claim whose game record is missing? NO — that's a partial shelf; treat as 500 per spec), sort by `unwrapped_at`, respond. **Every store error INCLUDING a gsi3 query failure ⇒ 500** — during the deploy's index-backfill window the index errors, and a soft-empty here would render the empty state as a lie (see Task 7 deploy order). Honesty note: the partial-failure 500 has no fault-injection rig to test it; it is enforced by construction (every `?`/match arm maps to the 500) and read in review, not asserted by a test — say so in the PR body rather than writing a fake test.

- [ ] **Step 4: Run to verify pass** — `cargo test -p public-api shelf_` (or CI mode; note in commit).

- [ ] **Step 5: Commit**

```bash
git add crates/public-api && git commit -S -m "🎁 public-api: GET /api/s/{token} — Fulfilled-only, oldest-first, oracle-proof 404, partial failure is whole failure"
```

---

### Task 4: admin-api — friends CRUD + link assignment + token mint helper

**Files:**
- Modify: `crates/admin-api/src/lib.rs`
- Test: `crates/admin-api/tests/api_test.rs`

**Interfaces:**
- Consumes: Task 2 store methods.
- Produces routes — **house convention verified: prefix is `/admin/api/...` and mutations are
  POST** (`/admin/api/links/{token}/note` → `post(handle_set_link_note)` at lib.rs:107; do NOT
  invent an `/api/admin` namespace or PATCH/PUT verbs):
  - `POST /admin/api/friends` body `{"name": "sarah"}` → 201 `{id, name, shelf_token, shelf_url_path: "/s/<token>"}`. Validation: name trimmed non-empty, ≤ 64 chars ⇒ else 422 via `unprocessable(..)`. `id` = one uuid-v4 simple; token = `mint_token()`.
  - `GET /admin/api/friends` → 200 `[{id, name, shelf_token, created_at}]` (shelf_token `""` when revoked).
  - `POST /admin/api/friends/{id}` body `{"name": "..."}` (rename) OR `{"reissue": true}` OR `{"revoke": true}` — exactly one of the three ⇒ else 422. Reissue → 200 with new token; revoke → 200 `{shelf_token: ""}`; unknown id → 404.
  - `POST /admin/api/links/{token}/friend` body `{"friend_id": "f1" | null}` → 204; friend must exist when non-null ⇒ else 422 `unknown friend`; unknown link ⇒ 404.
- Produces: `fn mint_token() -> String` — the double-uuid idiom extracted; the two existing inline sites (:240, :707 region) now call it. Behavior-identical (64 lowercase hex).

- [ ] **Step 1: Write failing tests** — follow `api_test.rs` house patterns (session fixture, json posts): create→list→rename→reissue (old token 404s on public side is Task 3's proof; here assert the returned token CHANGED and list reflects it)→revoke (list shows `""`)→422s (empty name, both-ops PATCH, unknown friend on link assign).
- [ ] **Step 2: Run to verify failure** — `cargo test -p admin-api friends_` → compile fail (routes missing).
- [ ] **Step 3: Implement** — handlers per interfaces; `mint_token()` extraction with a `// three call sites; extracted at the third (spec)` note.
- [ ] **Step 4: Run to verify pass** — `cargo test -p admin-api`.
- [ ] **Step 5: Commit**

```bash
git add crates/admin-api && git commit -S -m "🎁 admin-api: friends create/list/rename/reissue/revoke + link↔friend assignment; mint_token extracted at its third site"
```

---

### Task 5: web — ShelfPage (friend surface)

**Files:**
- Create: `web/src/friend/ShelfPage.tsx`
- Create: `web/src/friend/ShelfPage.test.tsx`
- Modify: `web/src/App.tsx` (route)

**Interfaces:**
- Consumes: `GET /api/s/{token}` (Task 3 shape).
- Produces: route `/s/:token` → `<ShelfPage />`.

- [ ] **Step 1: Write failing tests** (vitest + testing-library, mirror `LinkPage.test.tsx`'s fetch-mock style): renders loading → populated shelf (greeting contains the name lowercase + ♡; gifts in given order; note and thank-you rendered when present); empty state copy `"this shelf is waiting for its first story"`; 404 → soft error state (reuse LinkPage's not-found voice pattern); fetch error → soft retry state.
- [ ] **Step 2: Run to verify failure** — `cd web && npx vitest run src/friend/ShelfPage.test.tsx` → module not found.
- [ ] **Step 3: Implement** — page composition: warm header (`ben's shelf for ⟨name⟩ ♡`), vertical gift list (art via `artwork_url` with the GameGrid fallback treatment, title, `unwrapped ⟨year⟩`, gift_note in ben's voice styling, thank_note as the friend's reply styling — match `ThanksCard`'s tone). Oldest first as delivered; scroll is the timeline. No claim buttons, no links out except nothing — it's a keepsake page.
- [ ] **Step 4: Run to verify pass** — `npx vitest run src/friend/ShelfPage.test.tsx` then the full `npx vitest run`.
- [ ] **Step 5: Commit**

```bash
git add web/src/friend/ShelfPage.tsx web/src/friend/ShelfPage.test.tsx web/src/App.tsx
git commit -S -m "🎁 web: the shelf itself — /s/:token, oldest-first keepsake page in the attic voice"
```

---

### Task 6: web — admin friends panel + link assignment

**Files:**
- Create: `web/src/admin/Friends.tsx` + `web/src/admin/Friends.test.tsx`
- Modify: `web/src/admin/Links.tsx` (friend select on create + row assignment), `web/src/App.tsx` (admin subroute `friends`), admin nav (wherever `catalog`/`links`/`ops` tabs live — follow the existing tab component)

**Interfaces:** consumes Task 4 routes verbatim.

- [ ] **Step 1: Failing tests** — Friends panel: list renders, create flow posts and shows copyable `/s/<token>` URL, reissue confirms (`the old link stops working — hand them the new one`) and updates, revoke shows `no shelf link`; Links: picker present on create, row shows friend name, reassign fires `POST /admin/api/links/{token}/friend` and warns in-voice (`this moves its gifts to the other shelf`); **unassigned indicator: with 3 links of which 1 has `friend_id`, the list header renders `2 gifts aren't on a shelf yet` (derived client-side from the already-fetched links payload — friend_id rides the read-override), and renders nothing when 0**.
- [ ] **Step 2: Verify failure** — vitest, module/route missing.
- [ ] **Step 3: Implement** — workbench register (it's a tool, warm but compact); ctx allowlist note: `/admin/friends` is `[a-z]+` ⇒ already allowed by `ctx_is_allowed` (verified — one lowercase segment).
- [ ] **Step 4: Verify pass** — full `npx vitest run`.
- [ ] **Step 5: Commit**

```bash
git add web/src/admin web/src/App.tsx && git commit -S -m "🎁 web/admin: friends panel (create/copy/reissue/revoke) + link↔friend assignment with move-warning copy"
```

---

### Task 7: terraform — gsi3

**Files:**
- Modify: `terraform/aws-dynamodb.tf`

- [ ] **Step 1:** Add `attribute { name = "gsi3pk"  type = "S" }` and a `global_secondary_index` block named `gsi3`, hash key `gsi3pk` only, projection `ALL` — copy gsi2's block shape exactly, bump names.
- [ ] **Step 2:** `terraform -chdir=terraform fmt` and `terraform -chdir=terraform validate` (validate needs no creds; if init is required use `-backend=false`).
- [ ] **Step 3: Commit**

```bash
git add terraform/aws-dynamodb.tf && git commit -S -m "🎁 terraform: sparse gsi3 (links by friend) — in-place GSI add"
```

*(Apply happens at deploy, step 11 of the pounce arc, under the #217 runbook: derive the tfvars
set, non-zero destroy = STOP.)*

**DEPLOY ORDER IS LOAD-BEARING (OMBB, plan review): a new GSI backfills asynchronously —
`CREATING` → `ACTIVE` is not instant on a table with real items, and `list_links_for_friend`
CANNOT SERVE until `ACTIVE`.** Order: ① terraform apply (index only) → ② wait
`aws dynamodb describe-table --table-name <t> --query 'Table.GlobalSecondaryIndexes[?IndexName==`gsi3`].IndexStatus'`
= `ACTIVE` → ③ deploy lambda zips. The shelf handler fails LOUD on a gsi3 query error (500,
Task 3) — the soft failure would be the empty state lying "a shelf waiting for its first story"
to a friend, on the one page built to prove they were remembered.**

---

### Task 8: docs — spec status flip

- [ ] Flip `docs/spec-gift-shelf.md` status line to `BUILT — family review (…) → plan docs/superpowers/plans/2026-09-04-gift-shelf.md → implemented`, mirroring spec-attic-bell.md's convention, in the SAME commit as the last code task or its own tiny one:

```bash
git add docs/spec-gift-shelf.md && git commit -S -m "🎁 spec: status → BUILT"
```

## Self-review (done at write time)

- **Coverage:** spec§domain→T1, §dynamo→T2, §public-api→T3, §admin-api→T4, §web friend→T5, §web admin→T6, §deploy/gsi→T7. Detail-assembly helper: spec said "extracted"; plan REFUSES the extraction for v1 (shelf needs only Game fields — no duplication exists, so no helper is owed; noted inline at T3, spec's intent — no copy-paste of link-scoped logic — is honored by not touching it).
- **Types:** `unwrapped_at` everywhere; `set_link_friend(token, Option<&str>) -> bool`; token = 64 hex; friend item is top-level-attrs-only (decided in T2 with reason).
- **No placeholders** except two test bodies explicitly deferred to implementation time with the exact fixtures named (api_test claim literals) — the shapes and assertions are specified.
