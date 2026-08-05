# Wrapped Gifts Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Scheduled-unlock invite links — a link with `unlock_at` set shows a wrapped present +
countdown until the instant, then unwraps into the normal gift shelf; enforcement is server-side
at every layer down to the DynamoDB condition expressions.

**Architecture:** A new optional `unlock_at` on `domain::Link` (top-level numeric dynamo
attribute, stripped from the body blob — the notes' one-place contract with `expires_at`'s
numeric-compare enforcement), a new `ClaimRefusal::Sealed` variant that the codebase's
exhaustive matches force every gate to handle, a sealed `LinkView` that withholds the payload
server-side, and admin verbs to edit/remove the seal guarded by a single storage condition.
Also closes #154 (game-detail liveness hole — pre-existing, found in family review).

**Tech Stack:** Rust (axum lambdas, aws-sdk-dynamodb, moto for store tests), TypeScript React
SPA (vite, vitest). No terraform changes — read-time gating only.

**Spec:** `docs/superpowers/specs/2026-08-05-wrapped-gifts-design.md` (family-reviewed 2026-08-05).

## Global Constraints

- Workspace gate (CI): `cargo fmt --check` · `cargo clippy --workspace --all-targets --all-features -- -D warnings` · `cargo test --workspace`. Web gate: `npm run typecheck && npm run lint && npx vitest run && npm run build` in `web/`.
- Every commit GPG-signed (`git commit -S`), authored `code kitten <yourcodekitten@gmail.com>`.
- Sealed semantics everywhere: sealed iff `unlock_at > now` (strict). `unlock_at == now` is OPEN — matches `expires_at`'s `<=`-dead edge.
- Sealed responses withhold games, claims, gift_note, thank_note and carry `Cache-Control: no-store`.
- `unlock_at` storage: top-level numeric attr (epoch seconds via `schema::epoch_s`), NEVER in the body blob, REMOVE to unset (absent = born open / unsealed) — never write null.
- Seal state machine (one DDB condition, `attribute_exists(unlock_at) AND unlock_at > :now`): editable while sealed, immutable once open, never addable to a link born open.
- Friend-facing copy is lowercase, warm, no storefront energy (PRODUCT.md). Admin copy plain.
- Store/api tests need dynamodb-local (CI: `amazon/dynamodb-local:2.5.2` service on :8000 with `DYNAMODB_LOCAL_URL=http://localhost:8000` — see .github/workflows/ci.yml:12-16; locally run the same container, or any dynamodb-compatible endpoint, and export `DYNAMODB_LOCAL_URL`). Tests SKIP quietly when no endpoint is up; if `DYNAMODB_LOCAL_URL` is set but unreachable they PANIC on purpose (anti-forged-green guard in `store_or_skip` — never remove it).

---

### Task 0: close #154 — the pre-feature liveness gate (FIRST commit, cherry-pickable)

This lands BEFORE the feature exists, so a cherry-pick to main (or a revert of the gift) moves
the access-control fix independently (family review round 2). The test is named for the bug
that exists TODAY (revoked), not the feature that arrives tomorrow.

**Files:**
- Modify: `crates/public-api/src/lib.rs` (`handle_game_detail` :1047 — rename `_link` → `link`, add the gate)
- Test: `crates/public-api/tests/api_test.rs`

**Interfaces:**
- Produces: game detail refuses revoked/expired links with the endpoint's existing byte-identical 404. (Task 1's ripple later adds the `Sealed` arm to this match — the gate becomes the feature's fourth compile-forced socket.)

- [ ] **Step 1: Write the failing regression test** — harness facts: `test_link(token)` helper
exists at `api_test.rs:76`; the byte-identical-404 assertion pattern to mirror is
`owned_proxy_404s_without_live_link_byte_identical` (`api_test.rs:1137`) — clone its
compare-against-unknown-token shape.

```rust
/// KNOWN POSITIVE for #154: a revoked link must not serve game detail. Byte-identical
/// to the unknown-token 404 per owned_proxy_404s_without_live_link_byte_identical (:1137).
#[tokio::test]
async fn game_detail_refuses_revoked_link_154() {
    let Some(store) = store_or_skip("detail-revoked-154").await else {
        return;
    };
    let g = test_game(1);
    store.put_game(&g).await.unwrap();
    let mut lnk = test_link("rev-detail-tok");
    lnk.revoked = true;
    store.create_link(&lnk).await.unwrap();

    let mock = MockInvoker::new(FulfillResponse::GiftUrl {
        url: "https://x.com/g".into(),
    });
    let app = plain_router(Arc::clone(&store), mock.clone());

    // Revoked link, real game → must be byte-identical to unknown-token 404.
    let detail_req = Request::get(format!("/api/l/rev-detail-tok/games/{}/detail", g.id))
        .body(Body::empty())
        .unwrap();
    let detail_resp = app.clone().oneshot(detail_req).await.unwrap();
    let unknown_req = Request::get(format!("/api/l/{CTX_TOKEN}/games/{}/detail", g.id))
        .body(Body::empty())
        .unwrap();
    let unknown_resp = app.clone().oneshot(unknown_req).await.unwrap();

    assert_eq!(detail_resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(unknown_resp.status(), StatusCode::NOT_FOUND);
    let detail_bytes = axum::body::to_bytes(detail_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let unknown_bytes = axum::body::to_bytes(unknown_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        detail_bytes, unknown_bytes,
        "revoked-link detail 404 must be byte-identical to unknown-token 404 (#154)"
    );

    // Expired link → same refusal. Exhausted link → 200 (grid stays browsable).
    let mut expired = test_link("exp-detail-tok");
    expired.expires_at = Some(OffsetDateTime::now_utc() - time::Duration::hours(1));
    store.create_link(&expired).await.unwrap();
    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/api/l/exp-detail-tok/games/{}/detail", g.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let mut exhausted = test_link("exh-detail-tok");
    exhausted.claims_used = exhausted.claims_allowed;
    store.create_link(&exhausted).await.unwrap();
    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/api/l/exh-detail-tok/games/{}/detail", g.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
```

(Harness symbols all exist in api_test.rs today: `store_or_skip`, `test_game`, `test_link` :76,
`plain_router`, `MockInvoker`, `CTX_TOKEN`, `body_json`. If `put_game` isn't the seeding call the
neighboring tests use, copy whichever seeding call `revoked_link_active_false_games_empty` :193
uses verbatim.)

- [ ] **Step 2: Run to verify failure**: `cargo test -p public-api game_detail_refuses_revoked` → FAILS against current code (revoked serves 200 — the test proves it can catch the bug).

- [ ] **Step 3: Implement** — in `handle_game_detail`, rename `let _link` → `let link`; insert after the link resolve:

```rust
    // Liveness gate (#154 — this endpoint predates the exhaustive-match socket and never
    // consulted can_claim): detail serves iff the games GRID is visible on this link.
    // Refusals reuse the endpoint's byte-identical 404 (no oracle), and the refusing path
    // does strictly LESS work than the serving path after the same link lookup — no
    // timing channel either (spec §2).
    let now = OffsetDateTime::now_utc();
    match link.can_claim(now) {
        Ok(()) | Err(domain::ClaimRefusal::Exhausted) => {}
        Err(domain::ClaimRefusal::Revoked | domain::ClaimRefusal::Expired) => {
            return link_not_found_response();
        }
    }
```

- [ ] **Step 4: Run**: `cargo test -p public-api` → PASS; clippy clean.
- [ ] **Step 5: Commit (own commit — do not fold into feature commits)** `git commit -S -m "fix(public-api): game detail never consulted link liveness — revoked/expired links served detail (#154)"`

---

### Task 1: domain — `unlock_at`, `ClaimRefusal::Sealed`, and the compile ripple

**Files:**
- Modify: `crates/domain/src/lib.rs` (Link struct ~:168-215, ClaimRefusal enum ~:250, `can_claim` ~:266, cfg(test) block at bottom)
- Modify (mechanical compile ripple only — real behavior in Tasks 2/4/5): `crates/dynamo/src/schema.rs` `link_body`, every `Link { ... }` literal in `crates/admin-api/src/lib.rs`, `crates/dynamo/src/lib.rs`, `crates/public-api/src/lib.rs`, and test files; `can_claim` match sites in `crates/public-api/src/lib.rs` — **FIVE sites after Task 0** (step-5 gate M1 corrected this list): steam proxy :469-475, `handle_get_link` :550-556, claim pre-check :668-675, **`handle_post_thanks` :935-944** (exhaustive, no wildcard — easy to miss), and Task 0's `handle_game_detail` gate

**Interfaces:**
- Produces: `Link.unlock_at: Option<OffsetDateTime>` — serde is the **`thanked_at` combo** (`default` + `rfc3339::option` + `skip_serializing_if = "Option::is_none"`, in-repo precedent domain :194-199): None ⇒ the key is ABSENT on every serialized wire (body blob, admin list). This is what makes Task 2's `!body.contains("unlock_at")` assertion satisfiable — a no-skip shape would serialize `"unlock_at":null` and that test could never go green (step-5 gate B1). Also: `ClaimRefusal::Sealed`; `can_claim` refusal order revoked → **sealed** → expired → exhausted.

- [ ] **Step 1: Write the failing tests** (domain cfg(test); follow the existing `can_claim` test style)

```rust
#[test]
fn can_claim_sealed_before_unlock() {
    let mut l = link(); // the existing cfg(test) helper at domain/src/lib.rs:478
    let now = OffsetDateTime::now_utc();
    l.unlock_at = Some(now + time::Duration::seconds(1));
    assert_eq!(l.can_claim(now), Err(ClaimRefusal::Sealed));
}

#[test]
fn can_claim_open_at_exact_unlock_instant() {
    let mut l = link();
    let now = OffsetDateTime::now_utc();
    l.unlock_at = Some(now); // unlock_at == now ⇒ OPEN (strict >)
    assert_eq!(l.can_claim(now), Ok(()));
}

#[test]
fn can_claim_revoked_outranks_sealed() {
    let mut l = link();
    let now = OffsetDateTime::now_utc();
    l.revoked = true;
    l.unlock_at = Some(now + time::Duration::hours(1));
    assert_eq!(l.can_claim(now), Err(ClaimRefusal::Revoked));
}

#[test]
fn can_claim_sealed_outranks_expired_and_exhausted() {
    // Unreachable via admin validation (unlock must precede expiry) but the ordering is
    // still pinned: a sealed link reports sealed, whatever else is wrong with it.
    let mut l = link();
    let now = OffsetDateTime::now_utc();
    l.unlock_at = Some(now + time::Duration::hours(1));
    l.expires_at = Some(now - time::Duration::hours(1));
    l.claims_used = l.claims_allowed;
    assert_eq!(l.can_claim(now), Err(ClaimRefusal::Sealed));
}

#[test]
fn link_missing_unlock_at_deserializes_none() {
    // Extend the existing link serde-missing-field pin test (grep `default` pins near the
    // existing expires_at round-trip tests) — a pre-feature stored body must read back
    // unlock_at: None.
}
```

- [ ] **Step 2: Run to verify failure**: `cargo test -p domain` → compile errors (field/variant don't exist). That IS the failing state for a compile-checked language.

- [ ] **Step 3: Implement domain**

In `Link`, directly after `expires_at`:

```rust
    /// The wrapped-gift unlock moment: while `unlock_at > now` the link is SEALED — the
    /// friend surface shows a countdown and the server withholds the payload; every claim
    /// path refuses with [`ClaimRefusal::Sealed`]. Absent = born open; a seal is
    /// CREATE-TIME-ONLY (spec 2026-08-05 §4: a link born open can never gain one).
    /// Authoritative in a top-level numeric dynamo attribute like the enforcer fields;
    /// `schema::link_body` strips it from the body blob (the notes' one-place contract).
    /// Serde is thanked_at's combo — `default` restores None-on-missing under `with`,
    /// and skip-on-None keeps the stripped body free of even a null key (the Task-2
    /// body-strip test depends on absence, not null-ness).
    #[serde(
        default,
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub unlock_at: Option<OffsetDateTime>,
```

In `ClaimRefusal`:

```rust
    #[error("link sealed")]
    Sealed,
```

In `can_claim`, between the revoked check and the expires check:

```rust
        if let Some(unlock) = self.unlock_at
            && unlock > now
        {
            return Err(ClaimRefusal::Sealed);
        }
```

- [ ] **Step 4: Mechanical ripple to green** — every `Link { ... }` literal gains `unlock_at: None` (admin-api `handle_create_link` uses `unlock_at: None` FOR NOW — Task 5 wires the real value); `schema::link_body`'s stripped copy gains `unlock_at: None` (Task 2 replaces this fn wholesale); the FIVE public-api `can_claim` matches gain their arms, each one PINNED here (no site left as an executor's product decision — gate M1):
  - claim pre-check :668-675 and steam proxy :469-475: `ClaimRefusal::Sealed => "this gift is still wrapped"` (same 409 shape as the neighbors)
  - `handle_get_link` :550-556: `Err(domain::ClaimRefusal::Sealed) => ("sealed", true)` (Task 4 replaces with the real sealed response; this interim leaks nothing — games/notes hidden)
  - **`handle_post_thanks` :935-944**: `Err(domain::ClaimRefusal::Sealed)` refuses, same 409 shape as its Revoked/Expired arms, body `{"error": "this gift is still wrapped"}`. Unreachable in practice (sealed ⇒ zero claims ⇒ the handler's claims-first guard already refused) — pinned anyway: a ruling without an arm is a guess with a compiler error attached. Add that reasoning as the arm's comment.
  - `handle_game_detail` (Task 0's gate): `Sealed` joins the refusing arm alongside `Revoked | Expired` — the gate is now one of the feature's five sockets

- [ ] **Step 5: Run**: `cargo test --workspace` → PASS; `cargo clippy --workspace --all-targets --all-features -- -D warnings` → clean.

- [ ] **Step 6: Commit** `git commit -S -m "domain: unlock_at + ClaimRefusal::Sealed — a link can be born on a schedule"`

---

### Task 2: dynamo schema — type-level body strip, top-level attr, read override

**Files:**
- Modify: `crates/dynamo/src/schema.rs` (`link_body` :98-106, `link_item` :108-148)
- Modify: `crates/dynamo/src/lib.rs` (`link_from_item` :387-440)
- Test: `crates/dynamo/tests/store_test.rs`

**Interfaces:**
- Consumes: `Link.unlock_at` with skip-on-None serde (Task 1).
- Produces: stored links carry top-level `unlock_at` (N, epoch seconds) iff Some; body blob NEVER contains an `unlock_at` key; `link_from_item` unconditionally overrides from the top-level attr (absent ⇒ None).

- [ ] **Step 1: Write failing tests** — HARNESS FACTS (store_test.rs, verified): tests get a
store via `let Some(store) = store_or_skip("test-name").await else { return; };` (dynamodb-local
skip-guard — never remove the guard, it refuses to forge green when `DYNAMODB_LOCAL_URL` is set
but dead); raw access via `raw_client(test).await` against table `t-{test}`; link key is
`pk = AttributeValue::S(format!("LINK#{token}"))`, `sk = "META"` (or `dynamo::schema::link_pk`,
which is pub); build links with the file's existing link-literal helper (grep `fn link` /
existing `create_link` tests and clone their literal, adding `unlock_at`).

```rust
#[tokio::test]
async fn link_unlock_at_top_level_only_and_round_trips() {
    let Some(store) = store_or_skip("link-unlock-roundtrip").await else { return; };
    let mut l = /* clone an existing create_link test's Link literal, token "tok-sealed" */;
    l.unlock_at = Some(OffsetDateTime::now_utc() + time::Duration::hours(24));
    store.create_link(&l).await.unwrap();

    // Round-trip: unlock_at comes back (truncated to whole seconds by epoch_s).
    let got = store.get_link("tok-sealed").await.unwrap().unwrap();
    assert_eq!(
        got.unlock_at.unwrap().unix_timestamp(),
        l.unlock_at.unwrap().unix_timestamp()
    );

    // Raw item: top-level N attr present; body blob lacks the key entirely.
    let client = raw_client("link-unlock-roundtrip").await;
    let item = client
        .get_item()
        .table_name("t-link-unlock-roundtrip")
        .key("pk", AttributeValue::S("LINK#tok-sealed".into()))
        .key("sk", AttributeValue::S("META".into()))
        .send()
        .await
        .unwrap()
        .item()
        .cloned()
        .unwrap();
    assert!(item.get("unlock_at").and_then(|v| v.as_n().ok()).is_some());
    let body = item["body"].as_s().unwrap();
    assert!(!body.contains("unlock_at"), "body must never carry unlock_at: {body}");
}

#[tokio::test]
async fn link_without_unlock_at_reads_none_and_stores_no_attr() {
    // same harness: create a link with unlock_at: None → raw item has NO unlock_at attr;
    // get_link(..).unlock_at == None.
}
```

- [ ] **Step 2: Run to verify failure**: `cargo test -p dynamo --test store_test link_unlock` → body-contains assertion fails (Task 1's interim strip already sets None — verify the test failure is the MISSING top-level attr, i.e. the round-trip returns None).

- [ ] **Step 3: Implement**

`schema::link_body` — replace wholesale with the exhaustive-destructure strip (family review: adding a `Link` field without deciding its body fate must be a compile error):

```rust
pub fn link_body(l: &Link) -> String {
    // Exhaustive destructure — a new Link field breaks compilation HERE until its body
    // fate is decided (2026-08-05 family review, lilith's type-level strip). Fields bound
    // `_` are the stripped set: authoritative ONLY in top-level attrs.
    let Link {
        token,
        label,
        gift_note: _,
        thank_note: _,
        thanked_at: _,
        claims_allowed,
        claims_used,
        revoked,
        expires_at,
        unlock_at: _,
        created_at,
    } = l;
    let stripped = Link {
        token: token.clone(),
        label: label.clone(),
        gift_note: None,
        thank_note: None,
        thanked_at: None,
        claims_allowed: *claims_allowed,
        claims_used: *claims_used,
        revoked: *revoked,
        expires_at: *expires_at,
        unlock_at: None,
        created_at: *created_at,
    };
    serde_json::to_string(&stripped).expect("link serializes")
}
```

`schema::link_item` — after the `expires_at` insert:

```rust
    // unlock_at: expires_at's storage (top-level epoch seconds — the claim transaction's
    // numeric compare) with the NOTES' body contract (stripped from body; top-level is the
    // ONLY source). Omitted when None; set_link_unlock/remove_link_unlock REMOVE to unseal.
    if let Some(unlock) = l.unlock_at {
        item.insert("unlock_at".into(), epoch_s(unlock));
    }
```

`link_from_item` — mirror the `expires_at` block exactly:

```rust
    link.unlock_at = match n_attr("unlock_at") {
        None => None,
        Some(n) => {
            let secs = n
                .parse::<i64>()
                .map_err(|_| StoreError::Corrupt("link unlock_at not numeric"))?;
            Some(
                OffsetDateTime::from_unix_timestamp(secs)
                    .map_err(|_| StoreError::Corrupt("link unlock_at out of range"))?,
            )
        }
    };
```

- [ ] **Step 4: Run**: `cargo test -p dynamo` → PASS (moto).
- [ ] **Step 5: Commit** `git commit -S -m "dynamo: unlock_at top-level epoch attr, type-level body strip, read override"`

---

### Task 3: dynamo — claim transaction seal clause + the two seal verbs

**Files:**
- Modify: `crates/dynamo/src/lib.rs` (`claim_game` link_update condition ~:1006; new `set_link_unlock` / `remove_link_unlock` after `set_link_gift_note` ~:707)
- Test: `crates/dynamo/tests/store_test.rs`

**Interfaces:**
- Consumes: `Link.unlock_at` (Task 1); top-level `unlock_at` storage + `link_from_item` override (Task 2); existing `is_ccf_update`, `schema::key_pair`, `schema::epoch_s` (all pub(crate), in-crate use only).
- Produces: `pub async fn set_link_unlock(&self, token: &str, unlock_at: OffsetDateTime, now: OffsetDateTime) -> Result<bool, StoreError>` and `pub async fn remove_link_unlock(&self, token: &str, now: OffsetDateTime) -> Result<bool, StoreError>` — `Ok(false)` = condition refused (missing link / never sealed / already open; deliberately indistinguishable). `claim_game` refuses sealed links at the transaction (maps to existing `ClaimTxError::LinkNotClaimable`).

- [ ] **Step 1: Write failing moto tests**

```rust
#[tokio::test]
async fn claim_game_transaction_refuses_sealed_link() {
    // Sealed link + available game; call store.claim_game directly with now < unlock_at
    // (bypasses any handler pre-check — this pins the DDB condition itself).
    // Expect Err(ClaimTxError::LinkNotClaimable). Then set now' >= unlock_at path:
    // recreate sealed link whose unlock_at is 1s in the past → claim_game succeeds.
}

#[tokio::test]
async fn set_link_unlock_edits_only_while_sealed() {
    // sealed link (unlock +1h): set_link_unlock(+2h) → Ok(true), get_link shows +2h.
    // open link (unlock 1s ago): set_link_unlock(+1h) → Ok(false)  [immutable once open]
    // unknown token:              set_link_unlock(+1h) → Ok(false)
}

/// THE RULING, with its name on it (family review round 3 — lilith): a link born open
/// can never gain a seal. State A of the seal machine — the ONE state the complement
/// test can't see. Pins the argument lilith lost 2026-08-05 (`attribute_exists` vs
/// `attribute_not_exists ... OR`): if a future reader re-runs that argument and flips
/// the condition, every other seal test stays green and THIS one goes red. A ruling
/// without a regression test is a preference.
#[tokio::test]
async fn seal_cannot_be_added_to_an_unsealed_link() {
    let Some(store) = store_or_skip("seal-not-addable").await else {
        return;
    };
    let l = link("born-open"); // store_test's existing helper :80 — unlock_at: None
    store.create_link(&l).await.unwrap();

    let now = OffsetDateTime::now_utc();
    let accepted = store
        .set_link_unlock("born-open", now + time::Duration::hours(1), now)
        .await
        .unwrap();
    assert!(!accepted, "a link born open must refuse gaining a seal");

    // And the raw item still has NO unlock_at attr afterward.
    let client = raw_client("seal-not-addable").await;
    let item = client
        .get_item()
        .table_name("t-seal-not-addable")
        .key("pk", AttributeValue::S("LINK#born-open".into()))
        .key("sk", AttributeValue::S("META".into()))
        .send()
        .await
        .unwrap()
        .item()
        .cloned()
        .unwrap();
    assert!(item.get("unlock_at").is_none());
}

#[tokio::test]
async fn remove_link_unlock_unseals_only_while_sealed() {
    // sealed → Ok(true), get_link().unlock_at == None, raw item attr REMOVED (absent).
    // open/never-sealed/unknown → Ok(false).
}

/// THE COMPLEMENT PROPERTY (family review round 2 — step-5 gate item (b)): once
/// unlock_at exists, edit (`unlock_at > :now`) and claim (`unlock_at <= :now`) are
/// exact complements — no instant where both pass, none where neither does. The EXACT
/// row is the one earning its keep: an off-by-one surfaces there as an overlap (both
/// pass) or a gap (neither does), which one-sided testing structurally cannot see.
#[tokio::test]
async fn seal_conditions_are_exact_complements_at_the_instant() {
    let Some(store) = store_or_skip("seal-complement").await else {
        return;
    };
    // T pinned to a WHOLE second — epoch_s truncates, and the exact row must be exact.
    let t = OffsetDateTime::from_unix_timestamp(
        OffsetDateTime::now_utc().unix_timestamp() + 3600,
    )
    .unwrap();

    // Three rows x BOTH verbs. Fresh link + game per row: a successful claim consumes state.
    for (label, now, edit_ok, claim_ok) in [
        ("t-minus-1s", t - time::Duration::seconds(1), true, false),
        ("t-exact", t, false, true),
        ("t-plus-1s", t + time::Duration::seconds(1), false, true),
    ] {
        let tok = format!("comp-{label}");
        let mut l = link(&tok);
        l.unlock_at = Some(t);
        store.create_link(&l).await.unwrap();
        let g = game(1, true); // store_test's existing listable-game helper
        // NOTE: give each row its own game id if game(n, ..) collides across rows —
        // follow how neighboring claim_game tests seed games.
        store.put_game(&g).await.unwrap();

        let edited = store
            .set_link_unlock(&tok, t + time::Duration::hours(2), now)
            .await
            .unwrap();
        assert_eq!(edited, edit_ok, "edit verb at {label}");

        let claim = store
            .claim_game(&tok, &g.id, &format!("claim-{label}"), now)
            .await;
        assert_eq!(claim.is_ok(), claim_ok, "claim verb at {label}: {claim:?}");
        if !claim_ok {
            assert!(
                matches!(claim, Err(ClaimTxError::LinkNotClaimable)),
                "sealed claim must refuse as LinkNotClaimable, got {claim:?}"
            );
        }
    }
}
```

- [ ] **Step 2: Run to verify failure** — sequencing (gate minor 7): four of five tests red as COMPILE errors (`set_link_unlock`/`remove_link_unlock` don't exist). For the behavioral red on `claim_game_transaction_refuses_sealed_link`, stub the two verbs first (bodies `todo!()`), run ONLY that test: the sealed claim SUCCEEDS pre-clause — the known positive proving the test can catch the missing condition — then implement for green.

- [ ] **Step 3: Implement**

`claim_game` link_update condition (and its doc comment) gains the sibling clause:

```rust
            .condition_expression(
                "revoked = :f AND claims_used < claims_allowed \
                 AND (attribute_not_exists(expires_at) OR expires_at > :now) \
                 AND (attribute_not_exists(unlock_at) OR unlock_at <= :now)",
            )
```

(No new expression values — `:now` already bound. A sealed race maps positionally to
`LinkNotClaimable`, same generic 409 an expired race gets; the specific "still wrapped" copy
comes from the handler pre-check in the non-race case. Note this in the fn doc.)

New verbs, `set_link_gift_note`'s scoped shape with the seal condition:

```rust
    /// Move a sealed link's unlock moment. ONE storage condition enforces the whole seal
    /// state machine atomically (spec 2026-08-05 §4 — no read-compare-write; the TOCTOU
    /// review point): `attribute_exists(unlock_at) AND unlock_at > :now` ⇒ editable while
    /// sealed, immutable once open, never addable to a link born open. Ok(false) = refused;
    /// the three refusal causes are deliberately indistinguishable (no oracle).
    pub async fn set_link_unlock(
        &self,
        token: &str,
        unlock_at: OffsetDateTime,
        now: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        let (pk, sk) = schema::key_pair(link_pk(token), "META");
        let req = self
            .client
            .update_item()
            .table_name(&self.table)
            .key("pk", pk)
            .key("sk", sk)
            .condition_expression("attribute_exists(unlock_at) AND unlock_at > :now")
            .update_expression("SET unlock_at = :u")
            .expression_attribute_values(":u", schema::epoch_s(unlock_at))
            .expression_attribute_values(":now", schema::epoch_s(now));
        match req.send().await {
            Ok(_) => Ok(true),
            Err(sdk_err) if is_ccf_update(&sdk_err) => Ok(false),
            Err(sdk_err) => Err(StoreError::Aws(format!("{sdk_err:?}"))),
        }
    }

    /// Unseal — the seal's own delete verb (never expressed as a null set). Same condition,
    /// same Ok(false) contract as [`set_link_unlock`]. REMOVE keeps absent-attr = unsealed
    /// as the single representation.
    pub async fn remove_link_unlock(
        &self,
        token: &str,
        now: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        let (pk, sk) = schema::key_pair(link_pk(token), "META");
        let req = self
            .client
            .update_item()
            .table_name(&self.table)
            .key("pk", pk)
            .key("sk", sk)
            .condition_expression("attribute_exists(unlock_at) AND unlock_at > :now")
            .update_expression("REMOVE unlock_at")
            .expression_attribute_values(":now", schema::epoch_s(now));
        match req.send().await {
            Ok(_) => Ok(true),
            Err(sdk_err) if is_ccf_update(&sdk_err) => Ok(false),
            Err(sdk_err) => Err(StoreError::Aws(format!("{sdk_err:?}"))),
        }
    }
```

- [ ] **Step 4: Run**: `cargo test -p dynamo` → PASS.
- [ ] **Step 5: Commit** `git commit -S -m "dynamo: seal clause in the claim transaction + set/remove_link_unlock (one-condition state machine)"`

---

### Task 4: public-api — the sealed view, gate copy, and the #154 fix

**Files:**
- Modify: `crates/public-api/src/lib.rs` (`LinkView` :128-149, `handle_get_link` :527+, claim pre-check ~:665, steam proxy gate ~:467, `handle_game_detail` :1047)
- Test: `crates/public-api/tests/api_test.rs`

**Interfaces:**
- Consumes: `domain::ClaimRefusal::Sealed`, `Link.unlock_at` (Task 1).
- Produces (wire): `LinkView` gains `unlocks_in_seconds: Option<u64>` + `unlocks_at: Option<String>` (both `skip_serializing_if = "Option::is_none"`, present ONLY while sealed); sealed GET is `no-store`; sealed claim/proxy 409 body `{"error": "this gift is still wrapped"}`; game detail 404s for revoked/expired/sealed.

- [ ] **Step 1: Write failing axum-oneshot tests** (existing api_test.rs harness — moto store + router oneshot)

```rust
/// The devtools test: while sealed, the wire carries NOTHING a curious friend can peek
/// at — asserted on the RAW body string, not the parsed JSON (spec §2).
#[tokio::test]
async fn sealed_link_view_withholds_everything_and_counts_down() {
    let Some(store) = store_or_skip("sealed-view").await else {
        return;
    };
    store.put_game(&test_game(1)).await.unwrap(); // title "Game 1" — must NOT appear
    let mut lnk = test_link("sealed-tok");
    lnk.unlock_at = Some(OffsetDateTime::now_utc() + time::Duration::seconds(3600));
    lnk.gift_note = Some("happy birthday maya".into());
    store.create_link(&lnk).await.unwrap();

    let mock = MockInvoker::new(FulfillResponse::GiftUrl {
        url: "https://x.com/g".into(),
    });
    let resp = plain_router(Arc::clone(&store), mock.clone())
        .oneshot(Request::get("/api/l/sealed-tok").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get(header::CACHE_CONTROL).unwrap(),
        "no-store",
        "a cached sealed 200 would pin a countdown past midnight"
    );
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let raw = std::str::from_utf8(&bytes).unwrap();

    // RAW-string withholding — devtools is not a spoiler channel.
    assert!(!raw.contains("gift_note"), "sealed body leaked gift_note key: {raw}");
    assert!(!raw.contains("happy birthday"), "sealed body leaked the note text");
    assert!(!raw.contains("Game 1"), "sealed body leaked a game title");

    let j: serde_json::Value = serde_json::from_str(raw).unwrap();
    assert_eq!(j["state"], "sealed");
    assert_eq!(j["games"], serde_json::json!([]));
    assert_eq!(j["claims"], serde_json::json!([]));
    let secs = j["unlocks_in_seconds"].as_u64().unwrap();
    assert!((3595..=3600).contains(&secs), "ceiled remaining, got {secs}");
    assert!(j["unlocks_at"].as_str().unwrap().contains('T'), "rfc3339 instant");
    assert_eq!(j["claims_allowed"], 1); // the deliberate tease stays
}

#[tokio::test]
async fn sealed_link_open_at_exact_instant() {
    // unlock_at = now - 1s (the handler derives its own now_utc — the true exact row
    //   lives in the store complement test where `now` is a parameter):
    //   GET → state "active", games served,
    //   json lacks "unlocks_in_seconds" and "unlocks_at" keys entirely.
}

#[tokio::test]
async fn claim_and_steam_proxy_refuse_sealed_409() {
    // POST claim → 409 {"error":"this gift is still wrapped"};
    // GET /steam/owned/:steamid → 409 same body.
}

#[tokio::test]
async fn game_detail_refuses_sealed_link() {
    // Task 0 pinned revoked/expired; this pins the new state through the same gate:
    // sealed link + listable game → 404, byte-identical to the unknown-token 404.
}
```

- [ ] **Step 2: Run to verify failure**: sealed-view test fails against Task 1's interim arm (plain view without countdown fields / no-store); sealed claim/proxy tests already pass (Task 1 set that copy) — that's expected, they pin it here.

- [ ] **Step 3: Implement**

`LinkView` — add after `claims`:

```rust
    /// Wrapped gift: seconds until unlock, server-computed and CEILED (never arrives
    /// early; sealed ⇒ >= 1). Present ONLY while sealed — the client counts down from
    /// REMAINING, never by comparing wall clocks (spec §2).
    #[serde(skip_serializing_if = "Option::is_none")]
    unlocks_in_seconds: Option<u64>,
    /// Wrapped gift: the unlock instant, rfc3339. Present ONLY while sealed.
    #[serde(skip_serializing_if = "Option::is_none")]
    unlocks_at: Option<String>,
```

`handle_get_link` — replace the interim `("sealed", true)` arm; the sealed response is an
early return BEFORE the games/claims joins (nothing is read that isn't served):

```rust
        Err(domain::ClaimRefusal::Sealed) => {
            // Sealed response: no catalog/claims/notes reads AT ALL — the payload is
            // withheld at the source, not filtered (devtools is not a spoiler channel).
            // no-store: a cached sealed 200 outliving the moment would pin a countdown
            // past midnight (family review). Remaining is ceiled: never early, never 0.
            let unlock = link.unlock_at.expect("Sealed refusal implies unlock_at");
            let remaining_ms = (unlock - now).whole_milliseconds().max(1) as u64;
            let remaining = remaining_ms.div_ceil(1000);
            return (
                StatusCode::OK,
                [(header::CACHE_CONTROL, "no-store")],
                Json(LinkView {
                    label: link.label,
                    gift_note: None,
                    thank_note: None,
                    claims_allowed: link.claims_allowed,
                    claims_used: link.claims_used,
                    state: "sealed",
                    games: vec![],
                    claims: vec![],
                    unlocks_in_seconds: Some(remaining),
                    unlocks_at: Some(
                        unlock
                            .format(&time::format_description::well_known::Rfc3339)
                            .expect("unlock_at formats rfc3339"),
                    ),
                }),
            )
                .into_response();
        }
```

(The active/revoked/expired/exhausted `LinkView` construction gains `unlocks_in_seconds: None,
unlocks_at: None`.) Claim + proxy Sealed arms keep `"this gift is still wrapped"` (Task 1).

`handle_game_detail` — already gated (Task 0) with `Sealed` in the refusing arm (Task 1's
ripple); this task only adds the sealed regression test above. Verify the arm reads
`Err(Revoked | Expired | Sealed) => return link_not_found_response()`.

- [ ] **Step 4: Run**: `cargo test -p public-api` → PASS; clippy clean.
- [ ] **Step 5: Commit** `git commit -S -m "public-api: sealed link view — withheld payload, ceiled countdown, no-store"`

---

### Task 5: admin-api — create with a seal + the two admin verbs

**Files:**
- Modify: `crates/admin-api/src/lib.rs` (consts ~:552, `CreateLinkBody` :585, `handle_create_link` :619, router ~:104, new handlers near `handle_set_link_note`)
- Test: `crates/admin-api/tests/api_test.rs`

**Interfaces:**
- Consumes: `Store::set_link_unlock` / `Store::remove_link_unlock` (Task 3).
- Produces (admin wire): `POST /admin/api/links` accepts `unlock_at: Option<String>` (rfc3339 WITH offset — an absolute instant; the browser resolved ben's local pick); `POST /admin/api/links/:token/unlock` body `{"unlock_at": "<rfc3339>"}` (null/absent = 422 — unseal is not a null set); `DELETE /admin/api/links/:token/unlock`; both map store `Ok(false)` → 409 `{"error": "link is not sealed — seals are create-time-only and end at the unlock moment"}`. `handle_list_links` needs NO change (serializes `domain::Link`; `unlock_at` rides as rfc3339-or-ABSENT automatically — skip-on-None serde, gate B1).

- [ ] **Step 1: Write failing tests**

```rust
// create: unlock_at in the past → 422; > 370 days out → 422; not-before computed
//   expires_at → 422 (unlock 10d + expires_days 7 → 422); valid future instant → 200 and
//   GET list shows unlock_at (rfc3339). bare datetime w/o offset → 422.
// edit: sealed link POST /unlock {future instant} → 200, list reflects it;
//   past instant → 422 (rejected BEFORE the store call — fat-finger guard);
//   open link → 409; never-sealed → 409; body {"unlock_at": null} → 422.
// unseal: DELETE on sealed → 200, list omits unlock_at (absent, not null); DELETE on open/never-sealed → 409.
```

- [ ] **Step 2: Run to verify failure.**

- [ ] **Step 3: Implement**

```rust
const UNLOCK_MAX_DAYS: i64 = 370; // a typo'd year must not seal a gift forever
```

`CreateLinkBody` gains `unlock_at: Option<String>`. Parsing/validation helper (used by create
AND the edit verb):

```rust
/// Parse an absolute rfc3339 instant and bound it to (now, now + UNLOCK_MAX_DAYS].
/// The browser already resolved ben's local pick to an instant — a bare datetime
/// without offset is a client bug and parses as Err here.
fn parse_unlock_at(raw: &str, now: OffsetDateTime) -> Result<OffsetDateTime, String> {
    let t = OffsetDateTime::parse(raw, &time::format_description::well_known::Rfc3339)
        .map_err(|_| "unlock_at must be an rfc3339 instant with offset".to_string())?;
    if t <= now {
        return Err("unlock_at must be in the future".to_string());
    }
    if t > now + time::Duration::days(UNLOCK_MAX_DAYS) {
        return Err(format!("unlock_at must be within {UNLOCK_MAX_DAYS} days"));
    }
    Ok(t)
}
```

`handle_create_link`: after computing `expires_at`, parse+validate `unlock_at`; cross-check
`unlock < expires_at` when both Some (`"unlock_at must be before the link expires"`); wire
into the `Link { unlock_at, ... }` literal (replacing Task 1's `None`).

New handlers (both under the existing `session_middleware` route_layer + admin CSRF, like
`handle_set_link_note`):

```rust
#[derive(Deserialize)]
struct SetUnlockBody {
    unlock_at: String, // required: unseal is DELETE, never a null set (family review)
}

async fn handle_set_link_unlock(
    State(s): State<AppState>,
    Path(token): Path<String>,
    body: Result<Json<SetUnlockBody>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(body)) = body else {
        return unprocessable("unlock_at (rfc3339 instant) is required — to unseal, DELETE instead".into());
    };
    let now = OffsetDateTime::now_utc();
    let unlock = match parse_unlock_at(&body.unlock_at, now) {
        Ok(t) => t,
        Err(msg) => return unprocessable(msg),
    };
    // Cross-check expiry from a read (benign: the SEAL rules are enforced atomically in
    // the store condition; expiry ordering is admin-input hygiene, not enforcement).
    match s.store.get_link(&token).await {
        Ok(Some(l)) => {
            if l.expires_at.is_some_and(|exp| unlock >= exp) {
                return unprocessable("unlock_at must be before the link expires".into());
            }
        }
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
    match s.store.set_link_unlock(&token, unlock, now).await {
        Ok(true) => StatusCode::OK.into_response(),
        Ok(false) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": "link is not sealed — seals are create-time-only and end at the unlock moment"})),
        )
            .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn handle_delete_link_unlock(
    State(s): State<AppState>,
    Path(token): Path<String>,
) -> Response {
    let now = OffsetDateTime::now_utc();
    match s.store.remove_link_unlock(&token, now).await {
        Ok(true) => StatusCode::OK.into_response(),
        Ok(false) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": "link is not sealed — seals are create-time-only and end at the unlock moment"})),
        )
            .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
```

Router: `.route("/admin/api/links/:token/unlock", post(handle_set_link_unlock).delete(handle_delete_link_unlock))`

Deliberate asymmetry (gate minor 3, decided): POST distinguishes unknown-token (404) from
not-sealed (409) because it already reads the link for the expiry cross-check; DELETE does no
read — unknown and not-sealed both mean "nothing to unseal" and collapse into one 409. Admin-only
surface; no oracle concern. Stated here so nobody "fixes" the symmetry with an extra read.

- [ ] **Step 4: Run**: `cargo test -p admin-api` → PASS; full workspace + clippy → PASS.
- [ ] **Step 5: Commit** `git commit -S -m "admin-api: create sealed links + edit/unseal verbs (past rejected, null rejected, unseal is DELETE)"`

---

### Task 6: web api.ts — types + client functions

**Files:**
- Modify: `web/src/api.ts` (`LinkState` :22, `LinkView` :24, `AdminLink` :112, `adminCreateLink` :365, new fns after `adminSetLinkNote` :425)
- Test: `web/src/api.test.ts`

**Interfaces:**
- Produces: `LinkState` includes `'sealed'`; `LinkView` gains `unlocks_in_seconds?: number; unlocks_at?: string;`; `AdminLink` gains `unlock_at?: string;` (absent when unsealed — matches `thanked_at?`; skip-on-None serde, gate B1); `adminCreateLink(label, claims, expiresDays?, giftNote?, unlockAt?)` sends `unlock_at`; `adminSetLinkUnlock(token: string, unlockAtIso: string): Promise<void>`; `adminDeleteLinkUnlock(token: string): Promise<void>` — both throw the 409 message on conflict, follow `adminSetLinkNote`'s 422 mapping.

- [ ] **Step 1: Failing tests** (api.test.ts, existing fetch-mock style): sealed LinkView parses; adminCreateLink includes `unlock_at` in body when given, omits when not; adminSetLinkUnlock POSTs `{unlock_at}` with CSRF header; adminDeleteLinkUnlock sends DELETE; 409 → thrown message.
- [ ] **Step 2: Run**: `npx vitest run src/api.test.ts` → FAIL.
- [ ] **Step 3: Implement** (types + fns per interface block; copy `adminSetLinkNote`'s error mapping. On 409 BOTH verbs throw the server's own `error` string from the JSON body when present, falling back to `"the link isn't sealed anymore — refresh"` — ONE behavior, and Task 8's inline-error test asserts the server-message path).
- [ ] **Step 4: Run** → PASS; `npm run typecheck` → clean.
- [ ] **Step 5: Commit** `git commit -S -m "web/api: sealed state, countdown fields, seal verbs"`

---

### Task 7: friend surface — the wrapped present

**Files:**
- Create: `web/src/friend/SealedGift.tsx`
- Test: `web/src/friend/SealedGift.test.tsx`
- Modify: `web/src/friend/LinkPage.tsx` (early branch in the loaded state — place it right after `const { data } = view;` at ~:340, which is AFTER every hook in `LinkPageBody` (rules of hooks: the return must never sit above a hook); reuse its existing fetch function for refetch)
- Modify: `web/src/index.css` (the `gift-sway` keyframes — see Step 3; do NOT inline styles in the component)

**Interfaces:**
- Consumes: `LinkView` with `state === 'sealed'`, `unlocks_in_seconds`, `unlocks_at` (Task 6).
- Produces: `<SealedGift label={string} unlocksInSeconds={number} unlocksAt={string} onRefetch={() => void} />`.

- [ ] **Step 1: Failing tests** (fake timers):
  - renders label on the gift tag + countdown segments from `unlocksInSeconds` (e.g. 90061 → "1d 1h 1m 1s")
  - ticks: advance 1s → seconds decrement
  - at zero → `onRefetch` called exactly once (no self-unseal — component renders whatever the next fetch says)
  - a fresh `unlocksInSeconds` prop RESETS the countdown (this is the still-sealed-refetch backoff: the server hands back ≥1s and the clock simply continues — no error state exists)
  - `visibilitychange` → document visible → `onRefetch` called (tab-left-open drift resync — OMBB)
  - reduced-motion (`prefersReducedMotion() === true`): present art renders static (no `gift-sway` class), countdown text still updates
- [ ] **Step 2: Run** → FAIL (component doesn't exist).
- [ ] **Step 3: Implement** `SealedGift.tsx`:
  - pea-soup scene: inline pixel wrapped-present SVG (four palette shades — `--color-pixel` outline, `--color-give` ribbon, `--color-floor`/`--color-shelf` paper; same inline-SVG idiom as Landing's key charm), label on a gift tag, "opens `<local datetime from unlocksAt>`" prose (`toLocaleString`), countdown in the pixel font.
  - countdown: `useState(remaining)` seeded from prop + `useEffect` re-seeding on prop change; 1s `setInterval` decrement, floor at 0 → fire `onRefetch` once (ref-guard against double-fire); `visibilitychange` listener → visible → `onRefetch`.
  - gentle idle sway animation class (`motionOK` gate, like existing motion patterns); reduced-motion = static. In `index.css`, following the file's existing animation conventions (grep `@keyframes` there and match the naming/`prefers-reduced-motion` idiom):

```css
@keyframes gift-sway {
  0%, 100% { transform: rotate(-1.2deg) translateY(0); }
  50% { transform: rotate(1.2deg) translateY(-3px); }
}
.gift-sway { animation: gift-sway 4s ease-in-out infinite; transform-origin: 50% 90%; }
@media (prefers-reduced-motion: reduce) {
  .gift-sway { animation: none; }
}
```
  - copy (brand voice, lowercase): heading `"a gift is waiting for you ♡"`, sub `"ben wrapped this one — it opens itself when the moment comes"`.
- [ ] **Step 4: Wire into LinkPage**: in the loaded branch — immediately after `const { data } = view;` (~:340, AFTER every hook): `if (data.state === "sealed") return <SealedGift label={data.label} unlocksInSeconds={data.unlocks_in_seconds ?? 1} unlocksAt={data.unlocks_at ?? ""} onRefetch={refresh} />` — `refresh` is the file's EXISTING refetch mechanism (`const refresh = useCallback(() => setRefreshTick((t) => t + 1), [])` at LinkPage.tsx:180); do not invent a new one. CEREMONY DECISION (gate open-question 1, decided): the sealed→active transition renders the normal shelf, whose existing boot/typewriter entrance IS the unwrap ceremony beat — deliberate reuse, no bespoke crossfade; the spec's "soft ceremony beat" language is satisfied by that entrance (spec updated to match).
- [ ] **Step 5: Run**: `npx vitest run src/friend` → PASS; typecheck clean.
- [ ] **Step 6: Commit** `git commit -S -m "friend: SealedGift — the wrapped present, countdown from remaining, refetch-never-self-unseal"`

---

### Task 8: admin surface — wrap at create, move/unseal from the row

**Files:**
- Modify: `web/src/admin/Links.tsx` (create form ~:254-329, row meta ~:377-390, per-row editor region)
- Test: `web/src/admin/Links.test.tsx`

**Interfaces:**
- Consumes: `adminCreateLink(..., unlockAt?)`, `adminSetLinkUnlock`, `adminDeleteLinkUnlock`, `AdminLink.unlock_at` (Task 6).

- [ ] **Step 1: Failing tests**:
  - create form has a `datetime-local` input labeled "unlocks at (optional)"; submitting with a value calls `adminCreateLink` with an ISO **instant** (assert the arg matches `new Date(value).toISOString()` — the browser-resolves-ben's-zone rule)
  - a row whose `unlock_at` is future shows a `sealed until <local datetime>` chip; past or ABSENT (the field is omitted when unsealed — gate B1) shows none
  - sealed row exposes "move the moment" (datetime-local + save → `adminSetLinkUnlock` with ISO instant) and "unseal" (→ `adminDeleteLinkUnlock`); neither renders on open links
  - a 409 from either verb surfaces the server message inline (the link opened under him — the row then refreshes)
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement** (follow the file's existing form-state + per-row editor patterns, e.g. the note editor; `formatDate` exists — add a `formatDateTime` using `toLocaleString` for unlock display; refresh the list after either verb resolves, matching existing post-mutation reload).
- [ ] **Step 4: Run**: `npx vitest run src/admin` → PASS; `npm run typecheck && npm run lint` → clean.
- [ ] **Step 5: Commit** `git commit -S -m "admin: wrap links at create, move or unseal the moment from the row"`

---

### Task 9: full-gate sweep + docs

**Files:**
- Modify (if drift found): any of the above
- Verify: whole workspace + web

- [ ] **Step 1: Full gates**: `cargo fmt --check && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo test --workspace` then in `web/`: `npm run typecheck && npm run lint && npx vitest run && npm run build`.
- [ ] **Step 1b: Tip assertion for the access fix** (family review round 3 — lilith; OMBB gate item (j)): `cargo test -p public-api game_detail_refuses -- --nocapture` at the BRANCH TIP and confirm BOTH `game_detail_refuses_revoked_link_154` and `game_detail_refuses_sealed_link` ran and passed — Task 1's ripple rewrote the very gate Task 0's test guards; green-when-written is a statement about history, not about the ship.
- [ ] **Step 2: Spec status flip**: spec doc `status: draft` → `status: implemented (PR pending)`; confirm spec + this plan are committed on the branch.
- [ ] **Step 3: Two-grep sweep, two truth conditions** (family review round 4 — an assertion aimed at a grep with no invariant cries wolf until someone mutes it):
  - **`grep -rn "can_claim(" crates/public-api/src/ crates/admin-api/src/ crates/fulfillment/src/ crates/dynamo/src/`** (production code only — test files legitimately call it): assert BOTH directions against the five-site list in THIS plan doc (`docs/superpowers/plans/2026-08-05-wrapped-gifts.md`, Task 1 Files): (a) every hit is on the list — a silent sixth site is a gate nobody decided; (b) every listed site still hits — a listed site gone quiet lost its arm in a refactor. One direction without the other tests the bound from one side.
  - **`grep -rn "unlock_at" crates/ web/src/`** (INCLUDING tests — a hand-rolled null-writer fixture hides in exactly the excluded set): no site assertion (the symbol legitimately appears in serde/store/admin/web/tests by design) — this is a CLASSIFICATION pass: read each hit and confirm no stragglers writing null instead of REMOVE and no reads of a body-blob copy.
- [ ] **Step 4: Commit any fixups** `git commit -S -m "wrapped-gifts: gate sweep fixups"`.
