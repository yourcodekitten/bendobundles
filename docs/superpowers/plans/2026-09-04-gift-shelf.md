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
- Modify: `crates/dynamo/tests/store_test.rs` (the `fn link()` fixture at :536 gains `friend_id: None`) and `crates/public-api/tests/api_test.rs` + `crates/admin-api/tests/api_test.rs` (their `test_link()` fixtures gain `friend_id: None`) — **any `Link { .. }` literal anywhere in the workspace stops compiling until the field is added; this task owns all of them.** Grep first: `grep -rn 'Link {' crates/ web/ 2>/dev/null` and `cargo build --workspace 2>&1 | grep 'missing field'` to enumerate.

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
    assert_eq!(back.name, f.name);
    // created_at is the ONLY field with custom serde (rfc3339) — the round-trip is meaningless
    // without asserting it (M3).
    assert_eq!(back.created_at, f.created_at);
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

The `link_body` exhaustive destructure in `crates/dynamo/src/schema.rs` now fails to compile — that is Task 2's first step, by design (the type-level strip doing its job). For THIS task's `-p domain` tests, that's fine; the workspace build is red until Task 2.

Fix every `Link { .. }` literal by adding `friend_id: None` — in `crates/domain` tests AND in the three test fixtures named in Files (dynamo `link()` :536, public-api `test_link()` :76, admin-api `test_link()`), plus anything the enumerating grep/build surfaces. This task owns the field's introduction, so it owns every literal it breaks.

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
- **FRIEND ITEM SCHEMA (single, final — no body blob):** a friend item is FOUR top-level scalar
  attrs only: `pk = FRIEND#<id>`, `sk = "META"`, `name` (S), `shelf_token` (S), `created_at` (S,
  rfc3339), plus membership `gsi1pk = "FRIEND"` and `gsi1sk = <name>`. **No `body` attribute.**
  Four scalars can't drift, which is the whole point; `friend_from_item` reads the attrs directly
  (`shelf_token` absent ⇒ `""` ⇒ revoked). (This supersedes any "parse body" phrasing — there is
  no body.)
- Produces: `link_from_item` override: `friend_id` from top-level attr (present ⇒ Some, absent ⇒ None — unconditional override like `expires_at`).
- Produces: `link_item` writes top-level `friend_id` + `gsi3pk` when `l.friend_id` is Some (so create-with-friend indexes without a second write).
- Produces: `create_table_for_tests` defines `gsi3` (pk-only, `gsi3pk` S attribute, projection ALL).
- Produces: `pub async fn create_table_for_tests_without_gsi3(&self)` — identical minus gsi3.
  **This is the deploy-window world as a fixture** (a query against an absent index ERRORS; the
  arm in Task 3 proves the shelf never renders that error as an empty).
- Produces (test-only helper for M1): `store_without_gsi3(test: &str)` in `store_test.rs` — a
  copy of `store_or_skip`'s body that calls `create_table_for_tests_without_gsi3()` instead of
  `create_table_for_tests()`. Public-api's fault test (Task 3) gets its index-less store the same
  way, via a sibling helper in `api_test.rs`.

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
    let l = link("t1"); // the store_test.rs `link()` fixture — Task 1 already added `friend_id: None`
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

Add the write-path-revert regression test — **fully written, not a comment** (the reason the
gift_note pattern is mandatory here). Modeled on `gift_note_survives_claim_body_rewrite`
(store_test.rs:236), which asserts the exact same property for `gift_note`:

```rust
/// `friend_id` must survive `claim_game`'s `SET body` — the same war
/// `gift_note_survives_claim_body_rewrite` (:236) fought for the note. A body-only
/// `friend_id` is reverted by the friend claiming; a top-level attr + gsi3pk is not.
#[tokio::test]
async fn friend_id_survives_a_claim() {
    let Some(store) = store_or_skip("friend-survives-claim").await else { return };
    store.put_game(&game(1, true)).await.unwrap();
    store.create_friend(&friend("f1", "sarah", "aa")).await.unwrap();
    store.create_link(&link("tok-fr")).await.unwrap();
    assert!(store.set_link_friend("tok-fr", Some("f1")).await.unwrap());

    let now = datetime!(2026-07-02 12:00 UTC);
    store.claim_game("tok-fr", &game_id("gk1", "mn"), "c1", now).await.unwrap();

    let after = store.get_link("tok-fr").await.unwrap().unwrap();
    assert_eq!(after.claims_used, 1, "sanity: the claim landed");
    assert_eq!(after.friend_id.as_deref(), Some("f1"),
               "claim's SET body must not disturb the top-level friend_id");
    assert_eq!(store.list_links_for_friend("f1").await.unwrap().len(), 1,
               "and gsi3 still resolves the link to the friend");
}
```

**RED ARM (Lilith — a written body can be vacuously green; watch it fail first):** implement
`set_link_friend` to write `friend_id` INTO the body blob only (no top-level attr, no gsi3pk).
Both assertions fail — `claim_game`'s `SET body` from a pre-transaction read reverts it, and gsi3
never had a key. Watch that red, THEN implement the real top-level version to green. Record the
red in the commit body.

- [ ] **Step 3: Run to verify failure** — `cargo test -p dynamo friend_ link_friend friend-survives` (needs dynamodb-local; without it locally, the tests SKIP not fail — so the RED must be produced in CI or against a local dynamodb-local, never assumed. If you cannot run dynamodb-local, say so in the commit and rely on CI's red→green, but do NOT mark the red-first box checked without a real red somewhere).

- [ ] **Step 4: Implement** — `schema.rs`: `friend_pk(id) -> "FRIEND#<id>"`, `shelf_pk(token) -> "SHELF#<token>"`, `friend_item` per the FRIEND ITEM SCHEMA above (four top-level scalar attrs + gsi1 membership, **NO body blob**), `shelf_pointer_item` (pk/sk/META, attr `friend_id`). `friend_from_item` reads the four attrs directly (`shelf_token` absent ⇒ `""`). `lib.rs`: the eight methods per the Interfaces block, transactions via the existing `transact_write_items` idiom (see `claim_game` ~:1240 for error mapping).
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
- Consumes (Task 2, new): `Store::get_friend_by_shelf_token`, `list_links_for_friend`.
- Consumes (pre-existing store methods, verified present): `claims_for_link` (lib.rs:1093), `batch_get_games` (lib.rs:685). Not produced by any task — they already exist.
- Produces: the response types (define them in `lib.rs`, `Serialize`):

```rust
#[derive(Serialize)]
struct ShelfGift {
    game_id: String,
    title: String,
    artwork_url: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    unwrapped_at: OffsetDateTime,
    gift_note: Option<String>,
    thank_note: Option<String>,
}
#[derive(Serialize)]
struct ShelfResponse { name: String, gifts: Vec<ShelfGift> }
```

  Gifts sorted `unwrapped_at` ascending (oldest first — the shelf accretes).
- Produces: route `GET /api/s/{token}` → `handle_get_shelf`; and internal
  `async fn assemble_shelf(store: &Store, friend: Friend) -> Result<ShelfResponse, StoreError>`
  (the `ShelfResponse` above is its return type — B4).
- **No detail-assembly extraction in v1** (YAGNI): the shelf payload needs only `title` +
  `artwork_url`, both on `Game`; it never touches the steam cache, so there is no duplication of
  `handle_game_detail`'s logic to factor out. The spec's "extract a helper" note is satisfied by
  not copying link-scoped code, not by creating a premature helper.
- 404 body/shape identical to `link_not_found_response()` for unknown AND revoked; any store error anywhere (INCLUDING a gsi3 query error during the deploy window) ⇒ 500 `{"error": "the shelf slipped — try again"}` (soft voice, no internals; never `Ok(vec![])`).

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

// A Fulfilled claim on a link belonging to a friend. `claim(state, game_n, token, year)`
// helper — build the Claim literal from the fields verified in domain::Claim
// (id, link_token, game_id, state, gift_url, revealed_key, created_at, choice_pre_tpks,
// failure_reason). put_claim seeds it directly.
fn claim(id: &str, token: &str, game_n: u32, state: ClaimState, year: i32) -> Claim {
    Claim {
        id: id.into(), link_token: token.into(),
        game_id: game_id(&format!("gk{game_n}"), "mn"), state,
        gift_url: Some("https://x.com/g".into()), revealed_key: None,
        created_at: datetime!(2024-01-01 00:00 UTC).replace_year(year).unwrap(),
        choice_pre_tpks: None, failure_reason: None,
    }
}

#[tokio::test]
async fn shelf_happy_fulfilled_only_no_cross_friend_bleed() {
    let Some(store) = store_or_skip("shelf-filter").await else { return };
    let mock = MockInvoker::new(FulfillResponse::GiftUrl { url: "https://x.com/g".into() });
    for n in 1..=3 { store.put_game(&test_game(n)).await.unwrap(); }
    // f1 ← t1 : Fulfilled g1 @2024, Fulfilled g3 @2026 (order test), Pending g2 (must be excluded)
    let f1 = test_friend("f1", "sarah", "aa");
    store.create_friend(&f1).await.unwrap();
    let mut t1 = test_link("t1"); t1.gift_note = Some("for you ♡".into());
    store.create_link(&t1).await.unwrap();
    store.set_link_friend("t1", Some("f1")).await.unwrap();
    store.put_claim(&claim("c1", "t1", 1, ClaimState::Fulfilled, 2024)).await.unwrap();
    store.put_claim(&claim("c3", "t1", 3, ClaimState::Fulfilled, 2026)).await.unwrap();
    store.put_claim(&claim("c2", "t1", 2, ClaimState::Pending, 2025)).await.unwrap();
    // f2 ← t2 : Fulfilled g3 — must NOT appear on f1's shelf
    let f2 = test_friend("f2", "dave", "bb");
    store.create_friend(&f2).await.unwrap();
    store.create_link(&test_link("t2")).await.unwrap();
    store.set_link_friend("t2", Some("f2")).await.unwrap();
    store.put_claim(&claim("c4", "t2", 3, ClaimState::Fulfilled, 2023)).await.unwrap();

    let req = Request::get(&format!("/api/s/{}", f1.shelf_token)).body(Body::empty()).unwrap();
    let resp = plain_router(store, mock).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK); // THE CANARY — cannot pass without the route
    let j = body_json(resp).await;
    assert_eq!(j["name"], "sarah");
    let gifts = j["gifts"].as_array().unwrap();
    assert_eq!(gifts.len(), 2, "Fulfilled only (Pending g2 excluded), and no g3-from-f2 bleed");
    assert_eq!(gifts[0]["game_id"], game_id("gk1", "mn"), "oldest first: 2024 before 2026");
    assert_eq!(gifts[1]["game_id"], game_id("gk3", "mn"));
    assert_eq!(gifts[0]["gift_note"], "for you ♡", "note rides from the link record");
}

#[tokio::test]
async fn shelf_500s_when_index_absent_never_renders_empty() {
    // The deploy window as a fixture: a store whose table has NO gsi3. A query against the
    // absent index ERRORS; the handler must surface 500, never Ok(vec![]) (fail distinct).
    let Some(store) = store_without_gsi3("shelf-noindex").await else { return };
    let mock = MockInvoker::new(FulfillResponse::GiftUrl { url: "https://x.com/g".into() });
    let f = test_friend("f1", "sarah", "aa");
    store.create_friend(&f).await.unwrap();
    store.create_link(&test_link("t1")).await.unwrap();
    // NOTE: set_link_friend writes gsi3pk too — on a table without gsi3 that write itself may
    // error; if so, seed the friend_id via a create-path that doesn't require the index, or
    // assert the 500 on the READ alone by pointing the friend at a link created before the
    // assignment. The REQUIRED property: GET /api/s/<token> returns 500, body is NOT {name,gifts:[]}.
    let req = Request::get(&format!("/api/s/{}", f.shelf_token)).body(Body::empty()).unwrap();
    let resp = plain_router(store, mock).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let j = body_json(resp).await;
    assert!(j.get("gifts").is_none(), "a query error must never collapse into an empty shelf");
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p public-api shelf_` (needs dynamodb-local;
  without it the tests SKIP, so the RED for all three is produced in CI or against a local
  instance, never assumed). **Watch each fail (Lilith, red-first):** the happy test's
  `StatusCode::OK` canary reds on the missing route; the 404 test can't red on route-absence alone
  (a missing route also 404s) so it is not load-bearing for red; the index-absent test reds only
  once the route exists AND wrongly returns `Ok(vec![])` — so its red is produced during Step 3 by
  first writing the handler to swallow the error, watching this test go green-when-it-should-500,
  then fixing it. Record which reds you actually observed.

- [ ] **Step 3: Implement** — route in `router()`: `.route("/api/s/{token}", get(handle_get_shelf))`. `handle_get_shelf` resolves the friend (None ⇒ `link_not_found_response()`), then `assemble_shelf`: `list_links_for_friend` → for each, `claims_for_link` → filter `ClaimState::Fulfilled` → collect `game_id`s → ONE `batch_get_games` → build `ShelfGift`s (a claim whose game record is missing ⇒ 500, NOT a skipped row — a partial shelf is a lie), sort by `unwrapped_at` ascending, wrap in `ShelfResponse`. **Every `?`/error arm maps to the 500** — a gsi3 query error during the deploy window MUST reach the 500, never `Ok(vec![])`. The index-absent test above is exactly this guarantee's red/green; produce its red by first writing the error arm as `.unwrap_or_default()`, watch the test fail (200 with empty gifts), then fix to `?`/500 and watch it pass.

- [ ] **Step 4: Run to verify pass** — `cargo test -p public-api shelf_` (or CI mode; note in commit which reds you observed and where).

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
  - `POST /admin/api/friends` body `{"name": "sarah"}` → 200 `{id, name, shelf_token, shelf_url_path: "/s/<token>"}` (200, not 201 — house convention; `create_link` returns 200). Validation: name trimmed non-empty, ≤ 64 chars ⇒ else 422 via `unprocessable(..)`. `id` = one uuid-v4 simple; token = `mint_token()`.
  - `GET /admin/api/friends` → 200 `[{id, name, shelf_token, created_at}]` (shelf_token `""` when revoked).
  - `POST /admin/api/friends/{id}` body `{"name": "..."}` (rename) OR `{"reissue": true}` OR `{"revoke": true}` — exactly one of the three ⇒ else 422. Reissue → 200 with new token; revoke → 200 `{shelf_token: ""}`; unknown id → 404.
  - `POST /admin/api/links/{token}/friend` body `{"friend_id": "f1" | null}` → 204; friend must exist when non-null ⇒ else 422 `unknown friend`; unknown link ⇒ 404.
- Produces: `fn mint_token() -> String` — the double-uuid idiom (`format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())`, admin-api:241/708) extracted to ONE fn; both existing sites replaced with `mint_token()`. Behavior-identical (64 lowercase hex).
- Body types (define with `#[derive(Deserialize)]`):

```rust
#[derive(Deserialize)] struct CreateFriendBody { name: String }
#[derive(Deserialize)] struct PatchFriendBody {
    name: Option<String>, reissue: Option<bool>, revoke: Option<bool>,
}
#[derive(Deserialize)] struct AssignFriendBody { friend_id: Option<String> }
```

- [ ] **Step 1: Write failing tests** — the admin rig, verified in `api_test.rs`: build the router
  `router(Arc::clone(&store), Arc::clone(&invoker), admin_hash.clone(), None)`, authenticate with
  `let session = admin_login(&store, &invoker, &admin_hash, password).await`, and every request
  carries `.header("cookie", format!("session={session}"))` + `.header("x-admin-request", "1")`.
  Responses are 200 (not 201 — house convention: `create_link` returns 200). Add:

```rust
#[tokio::test]
async fn friends_create_list_rename_reissue_revoke() {
    let Some(store) = store_or_skip("admin-friends").await else { return };
    let password = "pw"; let admin_hash = test_admin_hash(password);
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new();
    let session = admin_login(&store, &invoker, &admin_hash, password).await;
    let call = |req| router(Arc::clone(&store), Arc::clone(&invoker), admin_hash.clone(), None).oneshot(req);
    let post = |path: &str, body: serde_json::Value| Request::post(path)
        .header("content-type", "application/json")
        .header("cookie", format!("session={session}"))
        .header("x-admin-request", "1")
        .body(Body::from(serde_json::to_vec(&body).unwrap())).unwrap();
    let get = |path: &str| Request::get(path)
        .header("cookie", format!("session={session}"))
        .header("x-admin-request", "1").body(Body::empty()).unwrap();

    // create
    let r = call(post("/admin/api/friends", serde_json::json!({"name": "sarah"}))).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let j = body_json(r).await;
    let id = j["id"].as_str().unwrap().to_string();
    let tok1 = j["shelf_token"].as_str().unwrap().to_string();
    assert_eq!(tok1.len(), 64);
    assert_eq!(j["shelf_url_path"], format!("/s/{tok1}"));
    // list shows it
    let j = body_json(call(get("/admin/api/friends")).await.unwrap()).await;
    assert_eq!(j.as_array().unwrap().len(), 1);
    // rename
    let r = call(post(&format!("/admin/api/friends/{id}"), serde_json::json!({"name": "sara"}))).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    // reissue → token changes
    let j = body_json(call(post(&format!("/admin/api/friends/{id}"), serde_json::json!({"reissue": true}))).await.unwrap()).await;
    let tok2 = j["shelf_token"].as_str().unwrap().to_string();
    assert_ne!(tok1, tok2, "reissue must mint a new token");
    // revoke → empty token in list
    call(post(&format!("/admin/api/friends/{id}"), serde_json::json!({"revoke": true}))).await.unwrap();
    let j = body_json(call(get("/admin/api/friends")).await.unwrap()).await;
    assert_eq!(j[0]["shelf_token"], "");
}

#[tokio::test]
async fn friends_validation_422s() {
    let Some(store) = store_or_skip("admin-friends-422").await else { return };
    let password = "pw"; let admin_hash = test_admin_hash(password);
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new();
    let session = admin_login(&store, &invoker, &admin_hash, password).await;
    let post = |path: &str, body: serde_json::Value| Request::post(path)
        .header("content-type", "application/json")
        .header("cookie", format!("session={session}"))
        .header("x-admin-request", "1")
        .body(Body::from(serde_json::to_vec(&body).unwrap())).unwrap();
    let call = |req| router(Arc::clone(&store), Arc::clone(&invoker), admin_hash.clone(), None).oneshot(req);

    // empty name
    let r = call(post("/admin/api/friends", serde_json::json!({"name": "  "}))).await.unwrap();
    assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY);
    // create one to mutate
    let j = body_json(call(post("/admin/api/friends", serde_json::json!({"name": "sarah"}))).await.unwrap()).await;
    let id = j["id"].as_str().unwrap().to_string();
    // two ops at once (rename AND revoke) → 422
    let r = call(post(&format!("/admin/api/friends/{id}"), serde_json::json!({"name": "x", "revoke": true}))).await.unwrap();
    assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY);
    // assign a nonexistent friend to a link → 422
    let l = test_link("t1"); store.create_link(&l).await.unwrap();
    let r = call(post("/admin/api/links/t1/friend", serde_json::json!({"friend_id": "ghost"}))).await.unwrap();
    assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY);
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p admin-api friends_` — Expected: compile FAIL (routes/handlers undefined).
- [ ] **Step 3: Implement** — in `router()`, add the four routes (all `post`/`get`, `/admin/api/...` prefix, inside the session-guarded group). Handlers:
  - `POST /admin/api/friends`: parse `CreateFriendBody`; `let name = body.name.trim(); if name.is_empty() || name.len() > 64 { return unprocessable("name must be 1–64 characters") }`; `id = Uuid::new_v4().simple().to_string()`; `token = mint_token()`; `store.create_friend(&Friend{ id, name, shelf_token: token, created_at: OffsetDateTime::now_utc() })`; 200 `{id, name, shelf_token, shelf_url_path}`.
  - `GET /admin/api/friends`: `store.list_friends()` → 200 array of `{id, name, shelf_token, created_at}`.
  - `POST /admin/api/friends/{id}`: parse `PatchFriendBody`; **exactly one of `name`/`reissue==Some(true)`/`revoke==Some(true)`** — count the set ones, `!= 1 ⇒ unprocessable("provide exactly one of: name, reissue, revoke")`. rename→`rename_friend`; reissue→read friend for old token, `mint_token()`, `reissue_shelf_token`, 200 with new token; revoke→read friend, `revoke_shelf_token`, 200 `{shelf_token: ""}`. `rename_friend` false / friend not found ⇒ 404.
  - `POST /admin/api/links/{token}/friend`: parse `AssignFriendBody`; if `Some(fid)`, `store.get_friend(fid)` None ⇒ `unprocessable("unknown friend")`; `set_link_friend(token, body.friend_id.as_deref())` false ⇒ 404; else 204.
  - `mint_token()`: extract, replace both inline sites, `// three call sites; extracted at the third (plan)`.
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

## Self-review (r2, after OMBB's step-5 gate — 4 blockers + 6 majors + 3 minors integrated)

- **Coverage:** spec§domain→T1, §dynamo→T2, §public-api→T3, §admin-api→T4, §web friend→T5, §web admin→T6, §deploy/gsi→T7. Detail-assembly helper: spec said "extracted"; plan REFUSES the extraction for v1 (shelf needs only Game fields — no duplication exists, so no helper is owed; T3 Interfaces).
- **Types:** `unwrapped_at` everywhere; `set_link_friend(token, Option<&str>) -> bool`; token = 64 hex; friend item is top-level-attrs-only (ONE schema, T2, no body blob); `ShelfResponse`/`ShelfGift` defined in T3; body types defined in T4.
- **Every test body is written in full.** There are NO comment-only test fns (B1). Each of the three review-added arms — `friend_id_survives_a_claim`, `shelf_500s_when_index_absent_never_renders_empty`, `shelf_happy_…` — carries its RED arm explicitly (Lilith: a written body can be vacuously green; watch it fail first).
- **Store construction:** the index-less fixture is reachable via `store_without_gsi3` helpers in both test crates (M1); the standard `store_or_skip` was the only path before.
- **Cross-task attribution checked:** `claims_for_link`/`batch_get_games` marked pre-existing, not produced by T2 (M5); every `Consumes` traces to an earlier `Produces` or existing code.
- **All Link literals** across domain + the three test crates are T1's responsibility (M2), enumerated by build error.
- **No PATCH/PUT anywhere** — all mutations POST `/admin/api/...` (M4 fixed: the retracted PATCH copy is gone).
