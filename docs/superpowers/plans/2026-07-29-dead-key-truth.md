# Dead-Key Truth Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Typed dead-key detection, a terminal `failed` claim state with durable reason, and a set-driven pending-age escalation sweep — so a humble-expired key terminally fails its claim (slot returned, game retired, ben pinged once) instead of looping silently forever.

**Architecture:** A new `HumbleError::KeyExpired` flows from humble-client's refusal classifier through `gift_error_decision` into a new `Decision::DeadKey`, executed by a new all-or-nothing dynamo transaction (`fail_claim_dead_key`, sibling of `compensate_claim`) that marks the claim `failed` + persists the reason + retires the game as `expired` + returns the friend's slot. Independently, reconcile gains an age sweep over the pending-claims GSI that pings for every claim pending >24h, BEFORE the reconcile pass so the watchdog cannot be starved by the failure it watches for.

**Tech Stack:** Rust workspace (crates: humble-client, domain, dynamo, fulfillment, public-api), wiremock + dynamodb-local (moto-style) tests, React/TS SPA in `web/`.

**Spec:** `docs/superpowers/specs/2026-07-29-dead-key-truth-design.md` (family-reviewed; lilith's riders folded).

## Global Constraints

- Branch: `kitten/dead-key-truth` (exists; spec commits already on it). Never force-push.
- Every commit GPG-signed (`git commit -S`), author `code kitten <yourcodekitten@gmail.com>`, lowercase conventional-commit style (`feat:`, `fix:`, `docs:`, `test:` …).
- Cargo on this box: `~/.cargo/config.toml` pins `jobs = 2` — do not override. Serialize cargo across agents: `flock /tmp/claude-cargo.lock cargo <…>`.
- Integration tests need dynamodb-local: run `DYNAMODB_LOCAL_URL=http://localhost:<port>` against a PRIVATE port if any sibling agent may be testing concurrently (see `store_or_skip` in `crates/fulfillment/tests/handler_test.rs` — tests SKIP silently when no dynamo is reachable; a skipped run is NOT a green run, check the test count).
- `cargo clippy --all-targets` must stay clean; a new lint suppression requires a justifying comment (workspace precedent: exactly one allow, documented).
- No `_` catch-all arms on `HumbleError` or `Decision` matches — the exhaustive-match discipline IS the safety net that finds every classification site (existing crate rule, keep it).
- Friend-facing copy is lowercase, warm, no blame (brand: "the game bowed out, your pick came back" energy). Ping copy carries claim id + game + humble's words, never key values / cookies / URLs.
- The three prod stuck claims (doom `87b9a4d8`, mylittleuniverse `3f46c058`, soulcalibur `3da0c011`) are the acceptance fixtures — nothing in this plan hand-edits them; the deployed machinery must resolve doom autonomously.

---

### Task 1: humble-client — refusal `error` code parse + `KeyExpired` + classifier

**Files:**
- Modify: `crates/humble-client/src/lib.rs` (~:193 `HumbleError` enum, ~:368-400 response structs, ~:1172-1200 `redeem_once` refusal arm, plus the `RevealResponse` refusal arm — locate with `grep -n "RevealResponse" crates/humble-client/src/lib.rs`)
- Test: `crates/humble-client/tests/client_test.rs`

**Interfaces:**
- Produces: `HumbleError::KeyExpired { msg: String, code: Option<String> }` AND `HumbleError::RedeemRefused(String)` **changes shape** to `RedeemRefused { msg: String, code: Option<String> }` (OMBB claw #1: parse the code ONCE at the edge, carry it on the variant — it is the exact value `failure_reason` persists). `pub(crate) fn classify_refusal(code: Option<String>, msg: String) -> HumbleError`. `RedeemResponse`/`RevealResponse` gain the optional `error` field. Task 4 consumes both shapes; every existing `RedeemRefused(_)` pattern in fulfillment becomes `RedeemRefused { .. }` (compiler enumerates the sites).

- [ ] **Step 1: Write the failing tests** (append to `client_test.rs`). Scaffolding pointers, verified: the REDEEM-path pattern to copy is `redeems_as_gift` (:122) / `already_redeemed_is_typed` (:146) -- client construction `client(&server).await`, invocation `.redeem_as_gift("KEY", "machine", 0)`. Do NOT copy the test at :1361 (`reveal_key_refused_reads_error_msg_field`) for the redeem tests -- that one exercises the REVEAL path (`.reveal_key(...)`); copying it would produce four `redeem_*`-named tests that never touch `redeem_once`. In the snippets below, `client_against_mock()`/`redeem_via()` are stand-ins for exactly that :122/:146 scaffolding:

```rust
#[tokio::test]
async fn redeem_expired_key_maps_to_key_expired() {
    // The live 2026-07-09 doom_eternal refusal, byte-exact (cloudwatch receipt).
    let (client, server) = client_against_mock().await; // reuse the file's existing helper for a client pointed at a MockServer; if the helper has a different name, grep the exhausted-keys test at ~:1370 and reuse ITS scaffolding verbatim
    Mock::given(method("POST"))
        .and(path("/humbler/redeemkey"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": false,
            "errormsg": "This key has expired and can no longer be redeemed."
        })))
        .mount(&server)
        .await;
    let err = redeem_via(&client).await.unwrap_err(); // same call shape the ~:1370 test uses
    match err {
        humble_client::HumbleError::KeyExpired { msg, code } => {
            assert_eq!(msg, "This key has expired and can no longer be redeemed.");
            assert_eq!(code, None); // no code captured live for the expired class yet
        }
        other => panic!("expected KeyExpired, got {other:?}"),
    }
}

#[tokio::test]
async fn expired_phrase_survives_humble_punctuation_drift() {
    // contains-match on the long phrase (OMBB claw #2, mirroring the already-redeemed
    // precedent): a tweaked period or prefix must not silently degrade terminal
    // detection back to park-forever.
    let (client, server) = client_against_mock().await;
    Mock::given(method("POST"))
        .and(path("/humbler/redeemkey"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": false,
            "errormsg": "Sorry! This key has expired and can no longer be redeemed"
        })))
        .mount(&server)
        .await;
    let err = redeem_via(&client).await.unwrap_err();
    assert!(matches!(err, humble_client::HumbleError::KeyExpired { .. }));
}

#[tokio::test]
async fn redeem_refusal_with_error_code_still_maps_to_redeem_refused() {
    // The keys_depleted_email shape (fixture precedent at ~:1370) must be UNCHANGED
    // by the new code parse: unknown/untyped codes fall through byte-for-byte.
    let (client, server) = client_against_mock().await;
    Mock::given(method("POST"))
        .and(path("/humbler/redeemkey"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "error": "keys_depleted_email",
            "error_msg": "Keys are temporarily exhausted for this product",
            "success": false
        })))
        .mount(&server)
        .await;
    let err = redeem_via(&client).await.unwrap_err();
    match err {
        humble_client::HumbleError::RedeemRefused { msg, code } => {
            assert_eq!(msg, "Keys are temporarily exhausted for this product");
            assert_eq!(code.as_deref(), Some("keys_depleted_email")); // the code now RIDES the error
        }
        other => panic!("expected RedeemRefused, got {other:?}"),
    }
}

#[tokio::test]
async fn short_expired_text_without_the_long_phrase_stays_redeem_refused() {
    // The contains-match keys on the LONG phrase; a short fragment lacking
    // "can no longer be redeemed" must NOT be buried as terminal.
    let (client, server) = client_against_mock().await;
    Mock::given(method("POST"))
        .and(path("/humbler/redeemkey"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": false,
            "errormsg": "This key has expired."
        })))
        .mount(&server)
        .await;
    let err = redeem_via(&client).await.unwrap_err();
    assert!(matches!(err, humble_client::HumbleError::RedeemRefused { .. }));
}

#[tokio::test]
async fn reveal_expired_key_maps_to_key_expired() {
    // The REVEAL path routes through the same classify_refusal (step 3e) -- one test
    // pins that the shared classifier really is wired there. Scaffolding: copy
    // reveal_key_refused_reads_error_msg_field (:1361), which calls
    // client(&server).await.reveal_key("GK", "mn_steam", 0).
    // Mock: 200 {"success": false, "errormsg": "This key has expired and can no longer be redeemed."}
    // Assert: matches!(err, HumbleError::KeyExpired { .. })
}
```

Note for the implementer: write the reveal test fully against the :1361 scaffolding -- the comment lines are its required mock + assertion, not optional prose. The redeem tests use the :122/:146 scaffolding named above. The assertions are the contract; the scaffolding is the file's.

- [ ] **Step 2: Run tests to verify they fail**

Run: `flock /tmp/claude-cargo.lock cargo test -p humble-client --test client_test redeem_expired -- --nocapture`
Expected: FAIL — `KeyExpired` variant does not exist (compile error).

- [ ] **Step 3: Implement**

3a. `HumbleError` (~:193): add after `AlreadyRedeemed` (~:237):

```rust
    /// Humble refused because the key itself is DEAD — expired server-side, unredeemable
    /// forever. TERMINAL: no retry can succeed and reconcile must not loop on it. Carries
    /// humble's exact refusal text (+ machine code when sent) for the operator ping and
    /// the durable claim record. Constructed ONLY by `classify_refusal` on the long
    /// phrase (lowercase-contains, mirroring the already-redeemed routing); anything
    /// without the phrase falls through to `RedeemRefused` (park-and-retry, the safe
    /// direction). Misclassification is recoverable in BOTH directions by design:
    /// missed terminal → park + daily sweep nag; false terminal → one transition ping
    /// carrying these exact words for ben to contradict.
    #[error("humble key expired: {msg}")]
    KeyExpired { msg: String, code: Option<String> },
```

Also RESHAPE `RedeemRefused` (~:238-244) from `RedeemRefused(String)` to:

```rust
    /// Humble returned success=false with a reason that is neither already-redeemed nor
    /// the expired phrase. `code` is humble's machine-readable refusal code when sent
    /// (e.g. "keys_depleted_email") — parsed ONCE here at the client edge and carried so
    /// downstream never re-parses (it feeds escalation copy and the durable
    /// failure_reason).
    #[error("humble redeem refused: {msg}")]
    RedeemRefused { msg: String, code: Option<String> },
```

The reshape's FULL blast radius (verified census — plain `cargo check` does NOT compile cfg(test) modules or `tests/` targets, so run `flock /tmp/claude-cargo.lock cargo check --workspace --all-targets` and expect exactly these sites):
- pattern-only matches -> `RedeemRefused { .. }`: fulfillment `lib.rs` :220, :292, :690, :1136, :1241, :1412 (the `handle_self_claim` "refused" detail label -- easy to miss).
- constructor sites -> `RedeemRefused { msg: "x".into(), code: None }` (keep each site's existing message string as `msg`): fulfillment `lib.rs:3517`, `handler_test.rs:52`, `:3218`, `:3257`.
- payload-READING patterns (these bind the message -- `{ .. }` is NOT the fix): `client_test.rs:184` and `:1377` are `RedeemRefused(ref msg) if msg == ...` -> rewrite as `RedeemRefused { ref msg, .. } if msg == ...`.

3b. `RedeemResponse` + `RevealResponse` (~:368-390): add to BOTH structs:

```rust
    /// Humble's machine-readable refusal code (e.g. "keys_depleted_email"), when present.
    /// Parsed for logging/registry-growth from day one — you can't grow a code registry
    /// from codes you drop. Classification still keys off the message text until a
    /// terminal code is captured live.
    #[serde(default)]
    error: Option<String>,
```

3c. New classifier, placed next to the response structs:

```rust
/// Classify a `success=false` redeem/reveal refusal. Ladder (spec §1): already-redeemed
/// phrase → AlreadyRedeemed (pre-existing routing, precedence preserved); the expired
/// long phrase → KeyExpired; everything else → RedeemRefused, message byte-for-byte as
/// before, now carrying the machine `code` when humble sent one (parsed ONCE here at
/// the edge — downstream never re-parses).
pub(crate) fn classify_refusal(code: Option<String>, msg: String) -> HumbleError {
    if let Some(c) = &code {
        tracing::info!(code = %c, errormsg = %msg, "humble refusal carried a machine code");
    }
    let lower = msg.to_lowercase();
    if lower.contains("already been redeemed") || lower.contains("already redeemed") {
        return HumbleError::AlreadyRedeemed;
    }
    // Lowercase-CONTAINS on the long phrase, mirroring the already-redeemed precedent
    // above (belt-and-suspenders on purpose): an exact match would silently degrade
    // terminal detection back to park-forever the day humble tweaks a period. The
    // phrase is long enough that a live key's refusal can't plausibly contain it.
    // Live capture: cloudwatch, claim 87b9a4d8, 2026-07-09.
    if lower.contains("has expired and can no longer be redeemed") {
        return HumbleError::KeyExpired { msg, code };
    }
    HumbleError::RedeemRefused { msg, code }
}
```

3d. `redeem_once` refusal arm (~:1181-1198): replace the body of the `(false, _)` arm so it reads:

```rust
                    (false, _) => {
                        let msg = body
                            .errormsg
                            .unwrap_or_else(|| "no error message".to_string());
                        // The refusal text is humble's own — safe to log and the
                        // single most useful clue for a redeem that won't complete.
                        tracing::warn!(errormsg = %msg, "humble redeem refused (success=false)");
                        Err(classify_refusal(body.error, msg))
                    }
```

(The already-redeemed routing moves INTO `classify_refusal` — delete the inline `lower`/`contains` block; behavior is identical, pinned by the existing already-redeemed test at ~:151.)

3e. The `RevealResponse` refusal arm lives in `reveal_once` (~:1332-1345) — the same `(false, _)`-shaped construction with its own inline already-redeemed contains-check: replace that whole `if lower…else` block with `Err(classify_refusal(body.error, msg))` (delete the now-local `lower` binding), exactly as in 3d. The two methods must classify identically — the file says so itself at the non-200 arms.

- [ ] **Step 4: Run the humble-client suite AND the cross-crate compile gates**

Run: `flock /tmp/claude-cargo.lock cargo test -p humble-client && flock /tmp/claude-cargo.lock cargo check -p fulfillment --all-targets`
Expected: humble-client PASS (including the pre-existing already-redeemed :146 and exhausted :1361 tests unchanged); fulfillment compiles clean including its test targets -- the reshape census above is complete only when this gate is green.

- [ ] **Step 5: Commit -- BOTH crates (the reshape edits fulfillment too; a humble-client-only commit leaves a broken commit + dirty tree for Task 2's cold subagent)**

```bash
git add crates/humble-client crates/fulfillment
git commit -S -m "feat(humble-client): typed KeyExpired refusal + machine-code parse; RedeemRefused carries the code"
```

---

### Task 2: domain — `ClaimState::Failed` + durable `failure_reason`

**Files:**
- Modify: `crates/domain/src/lib.rs` (~:29-35 `ClaimState`, ~:207-233 `Claim`)
- Modify (compile ripples, enumerated by `cargo check --workspace`): `crates/dynamo/src/lib.rs` idempotency-recheck matches (~:1356-1361 in `compensate_claim`, ~:1446-1451 in `compensate_self_claim`, plus any recheck match in `fulfill_claim`/`fulfill_self_claim` — the compiler lists them; there is no `_` arm anywhere on `ClaimState`)
- Test: `crates/domain/src/lib.rs` (inline `#[cfg(test)]` module, following the file's existing test placement)

**Interfaces:**
- Produces: `ClaimState::Failed` (wire `"failed"`); `Claim.failure_reason: Option<String>`. Task 3 writes both; Task 7 renders `"failed"`.

- [ ] **Step 1: Write the failing test** (in domain's existing test module):

```rust
    #[test]
    fn claim_state_failed_wire_value_and_reason_roundtrip() {
        // Wire value is load-bearing for web + admin rendering: exactly "failed".
        assert_eq!(
            serde_json::to_string(&ClaimState::Failed).unwrap(),
            "\"failed\""
        );
        // failure_reason must be absent-tolerant (every pre-existing claim item) and
        // round-trip when present.
        let json = r#"{"id":"c1","link_token":"t","game_id":"g:m","state":"failed",
            "gift_url":null,"created_at":"2026-07-09T21:38:28Z",
            "failure_reason":"This key has expired and can no longer be redeemed."}"#;
        let c: Claim = serde_json::from_str(json).unwrap();
        assert_eq!(c.state, ClaimState::Failed);
        assert_eq!(
            c.failure_reason.as_deref(),
            Some("This key has expired and can no longer be redeemed.")
        );
        // Absent field ⇒ None (pre-existing items stay wire-valid).
        let json_old = r#"{"id":"c1","link_token":"t","game_id":"g:m","state":"pending",
            "gift_url":null,"created_at":"2026-07-09T21:38:28Z"}"#;
        let c_old: Claim = serde_json::from_str(json_old).unwrap();
        assert_eq!(c_old.failure_reason, None);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `flock /tmp/claude-cargo.lock cargo test -p domain claim_state_failed`
Expected: FAIL — no `Failed` variant / no `failure_reason` field.

- [ ] **Step 3: Implement**

3a. `ClaimState`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimState {
    Pending,
    Fulfilled,
    Compensated,
    /// Terminal failure: the claim can never complete (today's one producer: a DEAD
    /// humble key — expired server-side). Generic on purpose: states are lifecycle,
    /// reasons are evidence — the *why* lives in [`Claim::failure_reason`], written in
    /// the same transaction (spec §2, family review 2026-07-29). The friend's slot is
    /// returned by that transaction; the game is retired, never re-listed.
    Failed,
}
```

3b. `Claim` — add after `choice_pre_tpks`:

```rust
    /// Why this claim terminally failed (humble's refusal text or matched code), written
    /// by the fail transaction in the same write that flips `state` to
    /// [`ClaimState::Failed`]. Durable on purpose: pings scroll away and log groups
    /// have retention; the claim record is the truth a future admin surface reads.
    /// `default` keeps every pre-existing CLAIM item wire-valid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
```

3c. Run `flock /tmp/claude-cargo.lock cargo check --workspace --all-targets 2>&1 | grep -B2 -A6 "non-exhaustive\|missing field"` -- the compiler enumerates every `ClaimState` match missing `Failed` and every `Claim` literal missing `failure_reason`. The recheck-after-cancellation MATCHES are ONLY in `compensate_claim` (~:1356) and `compensate_self_claim` (~:1446) -- the `fulfill_claim`/`fulfill_self_claim` rechecks are `!=` COMPARISONS (`:1086`, `:1138`), which the compiler will NOT flag. Conscious decision for those two: a stale fulfill retry that finds `Failed` errors loudly (correct -- fulfill must never override a terminal fail), but its error text says "fulfill lost to compensate", which is now a lie. Update BOTH sites' error text to `"fulfill lost -- claim already terminal (compensated or failed)"` so the loud path tells the truth. At the two compensate match sites, add the arm:

```rust
                            // Failed is terminal and owned by the dead-key transaction —
                            // like Fulfilled, someone else already decided this claim's
                            // fate; a late compensate/fulfill retry is a no-op.
                            ClaimState::Failed => return Ok(()),
```

Also fix the struct-literal compile errors: every place a `Claim` is constructed (`claim_game` ~:827, `claim_game_self` ~:964, and any test builders the compiler flags) gains `failure_reason: None,`.

- [ ] **Step 4: Run tests**

Run: `flock /tmp/claude-cargo.lock cargo test -p domain && flock /tmp/claude-cargo.lock cargo check --workspace`
Expected: domain PASS; workspace compiles clean.

- [ ] **Step 5: Commit**

```bash
git add crates/domain crates/dynamo
git commit -S -m "feat(domain): ClaimState::Failed + durable failure_reason on Claim"
```

---

### Task 3: dynamo — `fail_claim_dead_key` / `fail_self_claim_dead_key`

**Files:**
- Modify: `crates/dynamo/src/lib.rs` (add both methods directly below `compensate_self_claim` ~:1460)
- Test: `crates/dynamo/tests/store_test.rs`

**Interfaces:**
- Consumes: `ClaimState::Failed`, `Claim.failure_reason` (Task 2); `GameStatus::Expired` (pre-existing).
- Produces: `pub async fn fail_claim_dead_key(&self, link_token: &str, claim_id: &str, game_id: &str, reason: &str) -> Result<(), StoreError>` and `pub async fn fail_self_claim_dead_key(&self, claim_id: &str, game_id: &str, reason: &str) -> Result<(), StoreError>`. Task 4 calls both.

- [ ] **Step 1: Write the failing tests** (in `store_test.rs`, using the file's existing table-setup/claim-seeding helpers — read the `compensate_claim` tests first and mirror their scaffolding exactly; they already build a link + listed game + pending claim):

```rust
#[tokio::test]
async fn fail_claim_dead_key_flips_all_three_items() {
    let Some(store) = store_or_skip("fail_dead_key").await else { return };
    // Seed exactly as the compensate tests do: link (claims_allowed 2), available game,
    // then claim_game → Pending + claims_used 1.
    // ... (reuse the compensate test's seeding lines verbatim)
    store
        .fail_claim_dead_key(&token, &claim_id, &game_id, "This key has expired and can no longer be redeemed.")
        .await
        .unwrap();
    let claim = store.get_claim(&token, &claim_id).await.unwrap().unwrap();
    assert_eq!(claim.state, domain::ClaimState::Failed);
    assert_eq!(
        claim.failure_reason.as_deref(),
        Some("This key has expired and can no longer be redeemed.")
    );
    let game = store.get_game(&game_id).await.unwrap().unwrap();
    assert_eq!(game.status, domain::GameStatus::Expired);
    assert_eq!(game.claim_id, None);
    assert!(!game.is_listable(), "a dead-key game must never re-list");
    let link = store.get_link(&token).await.unwrap().unwrap();
    assert_eq!(link.claims_used, 0, "the friend's slot returns");
    // The pending marker is consumed: the claim no longer appears in the GSI.
    assert!(store.list_pending_claims().await.unwrap().iter().all(|c| c.id != claim_id));
}

#[tokio::test]
async fn fail_claim_dead_key_is_idempotent_and_loses_races_gracefully() {
    let Some(store) = store_or_skip("fail_dead_key_idem").await else { return };
    // ... seed as above, fail once ...
    // Idempotent retry after full success:
    store.fail_claim_dead_key(&token, &claim_id, &game_id, "again").await.unwrap();
    // claims_used must NOT double-decrement:
    let link = store.get_link(&token).await.unwrap().unwrap();
    assert_eq!(link.claims_used, 0);
    // And the durable reason is the FIRST write's (retry is a no-op, not an overwrite):
    let claim = store.get_claim(&token, &claim_id).await.unwrap().unwrap();
    assert_eq!(
        claim.failure_reason.as_deref(),
        Some("This key has expired and can no longer be redeemed.")
    );
}

#[tokio::test]
async fn fail_self_claim_dead_key_skips_link_decrement() {
    let Some(store) = store_or_skip("fail_self_dead_key").await else { return };
    // Seed a SELF claim exactly as the compensate_self tests do.
    store
        .fail_self_claim_dead_key(&claim_id, &game_id, "Keys are temporarily exhausted for this product")
        .await
        .unwrap();
    let claim = store.get_claim(domain::SELF_LINK_TOKEN, &claim_id).await.unwrap().unwrap();
    assert_eq!(claim.state, domain::ClaimState::Failed);
    let game = store.get_game(&game_id).await.unwrap().unwrap();
    assert_eq!(game.status, domain::GameStatus::Expired);
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `flock /tmp/claude-cargo.lock cargo test -p dynamo --test store_test fail_ -- --nocapture` (with `DYNAMODB_LOCAL_URL` pointing at a running dynamodb-local; start one on a private port if needed)
Expected: FAIL — methods do not exist.

- [ ] **Step 3: Implement.** Copy `compensate_claim` (~:1258-1330) wholesale as `fail_claim_dead_key` with exactly these deltas (everything else — transaction structure, condition expressions, cancellation handling — byte-identical):

```rust
    /// Dead-key terminal — [`compensate_claim`]'s sibling for a key humble can NEVER
    /// redeem again (spec §2). Same one-transaction/all-or-nothing shape and the same
    /// guards, with three deltas: the CLAIM lands `Failed` carrying `failure_reason`
    /// (durable evidence, written in the same txn — pings scroll, dynamo doesn't); the
    /// GAME retires as `Expired` (never re-lists — `is_listable` excludes it); the LINK
    /// decrement is identical (the friend did nothing wrong; the slot returns).
    /// Idempotency rechecks mirror compensate: any already-terminal state → Ok(()).
    pub async fn fail_claim_dead_key(
        &self,
        link_token: &str,
        claim_id: &str,
        game_id: &str,
        reason: &str,
    ) -> Result<(), StoreError> {
        let mut claim = self
            .get_claim(link_token, claim_id)
            .await?
            .ok_or(StoreError::Corrupt("fail_dead_key: claim missing"))?;
        claim.state = ClaimState::Failed;
        claim.failure_reason = Some(reason.to_string());

        let mut game = self
            .get_game(game_id)
            .await?
            .ok_or(StoreError::Corrupt("fail_dead_key: game missing"))?;
        game.status = GameStatus::Expired;
        game.claim_id = None;
        // … claim_put / game_put / link_update / transact_write_items EXACTLY as in
        // compensate_claim (same conditions: attribute_exists(gsi2pk), #st = :pending,
        // claims_used >= :one) …
    }
```

In the `TransactionCanceledException` recheck, the state match becomes:

```rust
                        match current.state {
                            // idempotent retry after a prior full success.
                            ClaimState::Failed => return Ok(()),
                            // another terminal owner won the marker race; their fate stands.
                            ClaimState::Fulfilled | ClaimState::Compensated => return Ok(()),
                            // marker gone but still Pending is impossible-by-construction →
                            // fall through to the loud error below.
                            ClaimState::Pending => {}
                        }
```

`fail_self_claim_dead_key` copies `compensate_self_claim` (~:1375-1458) with the same three deltas and NO link update (LINK#SELF has no META item — the gift variant's `claims_used >= 1` guard would cancel the whole transaction; same reasoning as compensate's self variant, keep the comment).

- [ ] **Step 4: Run the dynamo suite**

Run: `flock /tmp/claude-cargo.lock cargo test -p dynamo --test store_test`
Expected: PASS (new tests + all pre-existing compensate/fulfill tests untouched). Verify the new tests RAN (not skipped): the summary count must include them.

- [ ] **Step 5: Commit**

```bash
git add crates/dynamo
git commit -S -m "feat(dynamo): fail_claim_dead_key txns — Failed claim + durable reason + game retired + slot returned"
```

---

### Task 4: fulfillment — `Decision::DeadKey`, `FulfillResponse::KeyDead`, executors, structural pre-check

**Files:**
- Modify: `crates/fulfillment/src/lib.rs`:
  - `Decision` enum (~:176-186) + `gift_error_decision` (~:191) + `choose_decision` (~:260+)
  - `FulfillResponse` enum (~:145-170)
  - Every `Decision` match site (the compiler enumerates them after the variant lands; known sites: `handle_gift` executor ~:600-745, `redeem_claimed_tpk` ~:1030-1180, `reveal_claimed_tpk` ~:1215-1290, plus the self-claim and choose executors `cargo check` flags)
  - `claimed_tpk_terminal` (~:1018) — structural `tpk.expired` pre-check
- Test: `crates/fulfillment/tests/handler_test.rs` (ladder test ~:24 + new integration tests)

**Interfaces:**
- Consumes: `HumbleError::KeyExpired { msg, code }` / reshaped `RedeemRefused { msg, code }` (Task 1); `Store::{fail_claim_dead_key, fail_self_claim_dead_key}` (Task 3).
- Produces: `Decision::DeadKey`; `FulfillResponse::KeyDead` (serde tag `key_dead`); private helper `async fn fail_dead_key_any(deps: &Deps, link_token: &str, claim_id: &str, game_id: &str, reason: &str) -> Result<(), StoreError>`. Task 6 maps `KeyDead` → HTTP 410.

- [ ] **Step 1: Extend the pure ladder test first** (in `gift_decision_ladder_is_exhaustive_and_safe`, ~:24):

```rust
    // TERMINAL: a dead key never parks — it fails the claim, returns the slot, retires
    // the game. The ladder's one new arm (spec §2).
    assert!(matches!(
        gift_decision(&Err(E::KeyExpired {
            msg: "This key has expired and can no longer be redeemed.".into(),
            code: None
        })),
        Decision::DeadKey
    ));
```

And in the choose-ladder pinning test -- which lives at `crates/fulfillment/src/lib.rs:3496` (`choose_decision_ladder_never_compensates`, an INLINE cfg(test) module; `choose_decision` appears nowhere in handler_test.rs): append `E::KeyExpired { msg: "x".into(), code: None }` to its `park_variants` array -- the array-driven assertion then mechanically covers both the Park classification and the whole-map never-compensate property. Comment on the new entry:

```rust
        // choose_content never yields KeyExpired (it spends picks, it doesn't redeem
        // keys) -- classified conservatively as Park; reconcile's order diff decides.
```

- [ ] **Step 2: Run to verify failure**

Run: `flock /tmp/claude-cargo.lock cargo test -p fulfillment --test handler_test ladder && flock /tmp/claude-cargo.lock cargo test -p fulfillment --lib choose_decision_ladder`
Expected: FAIL -- no `KeyExpired` arm / no `DeadKey` variant (compile errors are the failure). Both commands must be re-run green at Step 5 (the `--lib` target is where the choose ladder actually runs).

- [ ] **Step 3: Implement the enum + classifications**

3a. `Decision`:

```rust
    /// The key is definitively DEAD (humble: expired, unredeemable forever). Terminal:
    /// fail the claim with its reason, return the slot, retire the game as Expired.
    /// NEVER park (retry cannot succeed) and NEVER compensate (re-listing would hand
    /// the next friend the same dead key).
    DeadKey,
```

3b. `gift_error_decision`: add the arm (keep the no-`_` discipline):

```rust
        // Definitively dead server-side — terminal, not park: retrying a key humble
        // has expired loops forever (live receipt: claim 87b9a4d8, 21 silent days).
        HumbleError::KeyExpired { .. } => Decision::DeadKey,
```

3c. `choose_decision`: add `HumbleError::KeyExpired { .. } => Decision::Park,` with the comment from Step 1's test.

3d. `FulfillResponse` — add after `AlreadyRedeemed`:

```rust
    /// Terminal dead key: the claim was failed (reason persisted), the friend's slot
    /// returned, the game retired as expired. public-api maps this to 410 with its own
    /// friend-honest message — the AlreadyRedeemed pattern, different words.
    KeyDead,
```

3e. Dispatch helper next to `compensate_any` (~:1511):

```rust
/// Dead-key dispatch — [`compensate_any`]'s sibling: SELF claims use the self variant
/// (no link decrement), everything else the gift variant.
async fn fail_dead_key_any(
    deps: &Deps,
    link_token: &str,
    claim_id: &str,
    game_id: &str,
    reason: &str,
) -> Result<(), StoreError> {
    if link_token == domain::SELF_LINK_TOKEN {
        deps.store.fail_self_claim_dead_key(claim_id, game_id, reason).await
    } else {
        deps.store.fail_claim_dead_key(link_token, claim_id, game_id, reason).await
    }
}
```

3f. Run `flock /tmp/claude-cargo.lock cargo check -p fulfillment` — the compiler lists every non-exhaustive `Decision` match. Implement a `DeadKey` arm at EACH. The canonical executor body (adapt only the surrounding identifiers each site already has in scope — `claim_id`, `game_id`, `link_token`, the outcome, and the game/tpk name the site logs with):

```rust
        Decision::DeadKey => {
            let reason = match &outcome {
                // The persisted failure_reason is the same value the wire carried:
                // "msg" or "msg [code: c]" — one truth from wire to dynamo.
                Err(HumbleError::KeyExpired { msg, code }) => match code {
                    Some(c) => format!("{msg} [code: {c}]"),
                    None => msg.clone(),
                },
                // DeadKey is only produced from KeyExpired today; a future producer
                // must carry its own words. Never leaves this arm blank.
                _ => "dead key (unclassified producer)".to_string(),
            };
            match fail_dead_key_any(deps, link_token, claim_id, game_id, &reason).await {
                Ok(()) => {
                    tracing::warn!(claim_id, game_id, %reason, "dead key: claim terminally failed, slot returned, game retired");
                    ping(
                        deps,
                        &format!(
                            "claim {claim_id} ({game_id}) hit a DEAD key — humble says: \
                             \"{reason}\". The claim is failed (reason recorded on the \
                             claim), the game is retired as expired and will not re-list, \
                             and the slot was returned. Nothing retries this.",
                        ),
                    )
                    .await;
                    FulfillResponse::KeyDead
                }
                Err(e) => {
                    // The store write failed — the claim is STILL pending; reconcile
                    // will re-detect the dead key next pass and retry this transition.
                    ping(deps, &format!("dead-key fail-claim write for {claim_id} failed: {e} — still pending, reconcile retries")).await;
                    FulfillResponse::Parked {
                        reason: "dead key detected but recording failed — will retry".into(),
                    }
                }
            }
        }
```

Site-specific deltas (scope facts verified -- two of the flagged sites have NO `link_token` in scope):
- `handle_gift` (executor ~:615): has `link_token` -- the canonical body works as written.
- `redeem_claimed_tpk` (executor ~:1080): has `link_token` (param). CHOICE-ONLY (every `claimed_tpk_terminal` Gift caller is a choice context -- verified :866/:981/:1583), so its Ok-arm ping states the stranded pick PLAINLY -- append: `" The monthly pick was already spent and is stranded -- the dead key was the pick's product."`
- `reveal_claimed_tpk` (executor ~:1217): NO `link_token` param -- always a SELF claim; call `fail_dead_key_any(deps, domain::SELF_LINK_TOKEN, claim_id, game_id, &reason)`. It serves BOTH choice terminals and bundle-B1 reconcile (:3214), so its ping uses the hedged line -- append: `" If this was a choice claim, its spent pick is stranded."`
- `handle_self_claim` (executor ~:1364): NO `link_token` in scope -- same `domain::SELF_LINK_TOKEN` call, same hedged ping line as `reveal_claimed_tpk`.
- Choose executor sites (matching `Decision` on a choose outcome, e.g. ~:912): `DeadKey` is unreachable from `choose_decision` (it maps KeyExpired -> Park) -- implement as a never-panic fallback: `tracing::error!` + return the site's Park response with reason `"dead key decision on a choose outcome -- classified conservatively"`.

3g. Structural pre-check in `claimed_tpk_terminal` (~:1018), FIRST thing in the fn body before the flavor dispatch:

```rust
    // Structural truth beats string-matching (spec §1 ladder rung 1): a tpk humble
    // already marks expired is dead — don't spend a redeem/reveal call to learn it.
    // Same trust sync already grants tpk.expired at listing time.
    if tpk.expired {
        let reason = format!(
            "tpk {} is marked expired on the order (structural is_expired)",
            tpk.machine_name
        );
        // Executor parity with the DeadKey arm: fail, ping, KeyDead.
        return match fail_dead_key_any(deps, link_token, claim_id, game_id, &reason).await {
            Ok(()) => {
                tracing::warn!(claim_id, game_id, %reason, "dead key (structural): claim terminally failed");
                ping(
                    deps,
                    &format!(
                        "claim {claim_id} ({game_id}) sits on a key humble marks expired \
                         ({}) — failed terminally without a redeem attempt. Reason recorded, \
                         slot returned, game retired. If this was a choice claim, its spent \
                         pick is stranded.",
                        tpk.machine_name
                    ),
                )
                .await;
                FulfillResponse::KeyDead
            }
            Err(e) => {
                ping(deps, &format!("dead-key (structural) fail-claim write for {claim_id} failed: {e} — still pending, reconcile retries")).await;
                FulfillResponse::Parked {
                    reason: "dead key detected but recording failed — will retry".into(),
                }
            }
        };
    }
```

Note: `claimed_tpk_terminal` currently has params `(deps, flavor, claim_id, link_token, game_id, gamekey, tpk, allow_heal)` — everything the pre-check needs is in scope.

(No claim-time pre-check in `handle_gift`/`handle_self_claim`: neither handler reads an order or holds a tpk -- `machine_name`/`keyindex` arrive on the request and go straight to the humble call -- and bundle-shelf expiry is already honored at listing time (`Expired` games are unlistable). The `claimed_tpk_terminal` pre-check above is the spec's whole structural rung. A refusal from the direct humble call still classifies through Task 1's ladder, so a claim-time expired key terminal-fails via the string rung.)

- [ ] **Step 4: Integration tests** (handler_test.rs). Concrete scaffolding, verified against the file: `fn deps(store: Store, humble_uri: &str, webhook_url: Option<String>) -> Deps` (~:116) builds the deps; `seed_pending_claim(store, gamekey, machine)` (~:134) seeds link+game+pending-claim; `store_or_skip(name)` gates on dynamo-local. Webhook assertion pattern: start a second `MockServer`, `Mock::given(method("POST")).and(path("/hook")).respond_with(ResponseTemplate::new(204)).expect(1).mount(&hook_server)`, pass `Some(format!("{}/hook", hook_server.uri()))` as `webhook_url`, and let the `.expect(1)` verify on drop; body-content assertions use `hook_server.received_requests().await`. :

```rust
#[tokio::test]
async fn gift_claim_on_expired_refusal_fails_terminally() {
    let Some(store) = store_or_skip("dead_key_gift").await else { return };
    // Seed link + available game + pending claim exactly like the existing
    // wiremock gift tests, then mock the redeem endpoint with the doom refusal:
    // 200 {"success": false, "errormsg": "This key has expired and can no longer be redeemed."}
    // Also mount a webhook mock (Mock::given(method("POST")).and(path("/hook")))
    // and set deps.webhook_url to the mock's /hook URL.
    // Invoke the gift handler as the existing tests do.
    // Assertions:
    // 1. response is FulfillResponse::KeyDead
    // 2. claim state == Failed, failure_reason == the doom string
    // 3. game status == Expired, not listable
    // 4. link claims_used decremented back
    // 5. the webhook mock received EXACTLY ONE ping containing "DEAD key"
}

#[tokio::test]
async fn structural_expired_tpk_fails_without_a_redeem_call() {
    let Some(store) = store_or_skip("dead_key_structural").await else { return };
    // DRIVE PATH: copy `reconcile_self_choice_b2_reveals_never_chooses`
    // (handler_test.rs:3548) wholesale -- it already seeds a choice-shaped pending
    // claim with a choice_pre_tpks snapshot, builds the choice-order mock
    // (is_expired: false in its fixtures), and invokes reconcile through sync so
    // execution reaches claimed_tpk_terminal via branch B2. Deltas from that test:
    // 1. flip the new tpk's fixture field to "is_expired": true
    // 2. replace its reveal-endpoint mock with .expect(0) -- the assertion that NO
    //    reveal/redeem call is spent on a structurally dead key
    // 3. webhook mock (Task 4 Step 4 /hook pattern) -- one ping containing "expired"
    // Assertions: claim Failed with the structural reason (contains "is_expired");
    // game Expired; the .expect(0) verifies on MockServer drop.
}
```

(Write these fully against the actual helper names found in the file — the comment lines above are the required assertions, not skippable prose.)

- [ ] **Step 5: Run the fulfillment suite**

Run: `flock /tmp/claude-cargo.lock cargo test -p fulfillment`
Expected: PASS; confirm the two new integration tests RAN (dynamo-local reachable), not skipped.

- [ ] **Step 6: Commit**

```bash
git add crates/fulfillment
git commit -S -m "feat(fulfillment): Decision::DeadKey — terminal dead-key path with structural tpk.expired pre-check"
```

---

### Task 5: fulfillment — pending-age escalation sweep (set-driven)

**Files:**
- Modify: `crates/fulfillment/src/lib.rs` — `reconcile` (~:3111, immediately after `list_pending_claims` succeeds and `now` is computed, BEFORE the per-claim loop)
- Test: `crates/fulfillment/tests/handler_test.rs`

**Interfaces:**
- Consumes: `list_pending_claims()` (existing), `RECONCILE_STUCK_ALERT_AGE` (existing, 24h), `ping()` (existing).
- Produces: no new public surface — an invariant: every pending claim older than 24h produces one ping per sync.

- [ ] **Step 1: Write the failing test:**

```rust
#[tokio::test]
async fn stale_pending_claim_pings_every_sync_before_reconcile_acts() {
    let Some(store) = store_or_skip("stale_sweep").await else { return };
    // Seed via seed_pending_claim(&store, "GK26H", "stalegame") (~:134), then age the
    // claim: re-write its item with created_at = now - 26h (grep the existing
    // stuck-alert/RECONCILE_MIN_AGE tests for the file's aging seam — they already
    // rewrite created_at; reuse that exact technique, do NOT invent a new seam).
    // humble MockServer: mount the order-read path returning 401 for EVERY request so
    // the reconcile PASS aborts on its first order read — the sweep must ping ANYWAY:
    // that ordering (sweep before pass) is the invariant lilith named; a dead session
    // must not starve the watchdog.
    // webhook MockServer: the /hook pattern from Task 4 Step 4, NO .expect() count
    // (cookie-dead pings may also fire on this lane) — assert on received_requests():
    // at least one body contains the claim id AND "STILL PENDING".
    // Invoke reconcile through the same entry the existing reconcile tests use
    // (grep "reconcile" in handler_test.rs for the established invocation).
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `flock /tmp/claude-cargo.lock cargo test -p fulfillment --test handler_test stale_sweep`
Expected: FAIL — no ping is sent.

- [ ] **Step 3: Implement.** In `reconcile`, after `let now = OffsetDateTime::now_utc();` and before the existing `for claim in claims` loop. The sweep iterates `for claim in &claims`; its borrow ends before the existing by-value loop, so NO change to the later loop is needed -- do not refactor it:

```rust
    // ── Pending-age sweep (spec §3, set-driven) ──────────────────────────────────
    // Runs over the GSI list BEFORE the reconcile pass, so nothing can starve it:
    // not a dead session aborting the pass, not a claim shape reconcile can't touch,
    // not reconcile itself regressing. The invariant is on the SET: every claim both
    // pending and older than RECONCILE_STUCK_ALERT_AGE pings, every sync, until a
    // terminal transition removes it from the GSI. Daily-by-cadence (sync schedule),
    // deliberately not deduplicated: a once-ever alert that scrolls away IS the
    // silent-loop bug this exists to kill (family review 2026-07-29).
    for claim in &claims {
        let age = now - claim.created_at;
        if age > RECONCILE_STUCK_ALERT_AGE {
            let days = age.whole_days();
            tracing::warn!(
                claim_id = %claim.id,
                game_id = %claim.game_id,
                age_days = days,
                "pending-age sweep: claim is still pending past the alert age"
            );
            ping(
                deps,
                &format!(
                    "claim {} ({}) is STILL PENDING after ~{days}d. Reconcile retries \
                     it every sync (or cannot reach it — see logs for this run). It \
                     will nag daily until it completes, compensates, or fails.",
                    claim.id, claim.game_id
                ),
            )
            .await;
        }
    }
```

- [ ] **Step 4: Run the suite**

Run: `flock /tmp/claude-cargo.lock cargo test -p fulfillment`
Expected: PASS, including Task 4's tests (a fresh <24h claim in those tests must NOT trip the sweep — if one does, its seeded `created_at` is stale-aged; fix the seed, not the sweep).

- [ ] **Step 5: Commit**

```bash
git add crates/fulfillment
git commit -S -m "feat(fulfillment): set-driven pending-age sweep — no claim is pending and silent past 24h"
```

---

### Task 6: public-api — `KeyDead` → HTTP 410

**Files:**
- Modify: `crates/public-api/src/lib.rs` (~:680-703, the claim outcome match + its log match)
- Modify: `crates/admin-api/src/lib.rs` (~:793, the self-claim endpoint's response mapping -- KeyDead must not surface as 500)
- Test: whatever claim-endpoint test exists (`grep -rn "GONE\|already redeemed" crates/public-api` locates the AlreadyRedeemed mapping test to mirror; if none exists, the two match arms below are covered by Task 4's response-variant tests plus compile — add the arm, note the gap in the PR body)

**Interfaces:**
- Consumes: `FulfillResponse::KeyDead` (Task 4).
- Produces: HTTP 410 `{"error": "that key can't be redeemed anymore — choose another"}` — the friend-facing copy of record (spec-aligned, no choice-pick jargon for bundle friends; web renders the server's message verbatim through the existing `refused` lane).

- [ ] **Step 1: Add the log-match arm** (in the outcome-logging match ~:682):

```rust
        Ok(FulfillResponse::KeyDead) => tracing::info!("claim: dead-key (410)"),
```

- [ ] **Step 2: Add the response arm** (before the `_ => park_response()` arm):

```rust
        Ok(FulfillResponse::KeyDead) => (
            StatusCode::GONE,
            Json(serde_json::json!({
                "error": "that key can't be redeemed anymore — choose another"
            })),
        )
            .into_response(),
```

- [ ] **Step 3: admin-api sibling arm.** The admin self-claim endpoint (`crates/admin-api/src/lib.rs:793` region) currently folds `FulfillResponse::KeyDead` into its catch-all -> HTTP 500 "fulfillment failed" -- a designed terminal outcome reported as an error. Read the handler's existing non-500 outcome arms (mirror their exact response construction and error-envelope JSON shape) and add an explicit arm: `FulfillResponse::KeyDead` -> status 410 with message `"key is dead on humble's side — claim failed terminally, reason recorded on the claim"`.

- [ ] **Step 4: Mirror the AlreadyRedeemed test** (if the grep in Files found one) asserting 410 + the exact message for a `KeyDead` fulfillment result.

- [ ] **Step 4: Run**

Run: `flock /tmp/claude-cargo.lock cargo test -p public-api -p admin-api && flock /tmp/claude-cargo.lock cargo clippy --all-targets -p public-api -p admin-api`
Expected: PASS / clean.

- [ ] **Step 5: Commit**

```bash
git add crates/public-api crates/admin-api
git commit -S -m "feat(api): dead-key claims answer 410 with honest copy on friend + admin surfaces"
```

---

### Task 7: web — `failed` rendered warm on friend + admin surfaces

**Files:**
- Modify: `web/src/api.ts` (`ClaimView` state union, ~:18/:98/:107 — grep `'pending' | 'fulfilled' | 'compensated'`)
- Modify: `web/src/friend/ClaimsHistory.tsx` (STATE_CHIP ~:4-8, row detail ~:40-52)
- Modify: `web/src/admin/Links.tsx` (`stateBadgeClass` ~:30)
- Modify: `web/src/admin/Catalog.tsx` (~:338, the self-claim state branch)

**Interfaces:**
- Consumes: wire state `"failed"` (Task 2), the 410 message (Task 6 — flows through the EXISTING `refused` lane in `claimGame`/`ClaimDialog`; no dialog change needed).
- Produces: friend chip label `returned`; admin badge for `failed`.

- [ ] **Step 1: Widen the type.** Every occurrence of the union `'pending' | 'fulfilled' | 'compensated'` in `api.ts` becomes `'pending' | 'fulfilled' | 'compensated' | 'failed'`.

- [ ] **Step 2: Friend chip + row detail** in `ClaimsHistory.tsx`:

```tsx
const STATE_CHIP: Record<ClaimView['state'], { label: string; className: string }> = {
  fulfilled: { label: 'gifted', className: 'bg-give text-give-ink' },
  pending: { label: 'processing', className: 'bg-amber-900 text-amber-200' },
  compensated: { label: 'compensated', className: 'bg-slate-800 text-slate-300' },
  failed: { label: 'returned', className: 'bg-rose-950 text-rose-200' },
};
```

And in the row (the ternary at ~:40-52), add a `failed` branch between the fulfilled and pending branches:

```tsx
              ) : claim.state === 'failed' ? (
                <span className="shrink-0 text-xs text-dust-faint">
                  that key expired on humble&apos;s side — your pick came back
                </span>
              ) : claim.state === 'pending' ? (
```

- [ ] **Step 3: Admin badges.** In `Links.tsx` `stateBadgeClass`, add a `case 'failed':` returning a rose-toned class consistent with the function's existing entries (read the function; mirror its class-string style, e.g. `'bg-rose-950 text-rose-200'`). ALSO: `web/src/admin/Catalog.tsx:338` branches on `sc.state === 'compensated'` for self-claims -- read that conditional and add a sibling `'failed'` branch mirroring the compensated branch's structure with the same rose tone and the literal label `key dead` (without this, a failed self-claim silently renders through the fallback styling).

- [ ] **Step 4: Verify**

Run: `cd web && npm run typecheck && npm run lint && npm run build`
Expected: all clean. (`npm test` if the vitest suite covers these components — run `npx vitest run --reporter=dot` once; pre-existing failures are out of scope, new ones are not.)

- [ ] **Step 5: Commit**

```bash
git add web
git commit -S -m "feat(web): failed claims render warm — 'returned' chip, honest copy, admin badge"
```

---

### Task 8: terraform — the watchdog's watchdog (out-of-process alarms)

**Files:**
- Create: `terraform/aws-cloudwatch-alarms.tf`
- Modify: `terraform/tf-variables.tf` (the `ops_alarm_email` variable)

**Interfaces:**
- Consumes: the fulfillment lambda's function name -- `module.lambda_fulfillment.lambda_function_name` (lambdas in this repo are MODULE instantiations, `bendoerr-terraform-modules/lambda/aws`; there is no `resource "aws_lambda_function"` anywhere. Precedent for exactly this reference: `aws-eventbridge.tf:26`).
- Produces: two `aws_cloudwatch_metric_alarm`s + one SNS topic with ben's email subscribed. Applied at DEPLOY time (pounce step 11), not by CI.

- [ ] **Step 1: Write the terraform** (mirror the file-local label-module + tag conventions the other terraform files use — read `aws-eventbridge.tf` for the smallest example):

```hcl
# The watchdog's watchdog (spec §3, OMBB's claw #3): the pending-age sweep lives
# INSIDE the fulfillment lambda — if the cron misfires, a deploy bricks the
# function, or IAM rots, the sweep dies with it and every in-process alarm dies
# too. These two alarms are the out-of-process layer: they fire when the sync
# lambda errors, or when it hasn't been invoked for a full day (25h > the 24h
# schedule, one hour of grace). Layer map: the sweep catches the claim reconcile
# never touches; these catch the reconcile that never runs.

resource "aws_sns_topic" "ops_alarms" {
  name = "${module.label.id}-ops-alarms"
  tags = module.label.tags
}

resource "aws_sns_topic_subscription" "ops_alarms_email" {
  topic_arn = aws_sns_topic.ops_alarms.arn
  protocol  = "email"
  endpoint  = var.ops_alarm_email # ben confirms the subscription once by mail
}

resource "aws_cloudwatch_metric_alarm" "fulfillment_errors" {
  alarm_name          = "${module.label.id}-fulfillment-errors"
  alarm_description   = "bendobundles fulfillment lambda reported errors — sync/reconcile may be silently down"
  namespace           = "AWS/Lambda"
  metric_name         = "Errors"
  dimensions          = { FunctionName = module.lambda_fulfillment.lambda_function_name }
  statistic           = "Sum"
  period              = 3600
  evaluation_periods  = 1
  threshold           = 1
  comparison_operator = "GreaterThanOrEqualToThreshold"
  treat_missing_data  = "notBreaching"
  alarm_actions       = [aws_sns_topic.ops_alarms.arn]
  tags                = module.label.tags
}

resource "aws_cloudwatch_metric_alarm" "fulfillment_silent" {
  alarm_name          = "${module.label.id}-fulfillment-silent"
  alarm_description   = "bendobundles fulfillment lambda has not been invoked in 25h — the daily sync (and its pending-age sweep) is not running"
  namespace           = "AWS/Lambda"
  metric_name = "Invocations"
  dimensions  = { FunctionName = module.lambda_fulfillment.lambda_function_name }
  statistic   = "Sum"
  # 25 consecutive silent hours = the 24h schedule + 1h grace. Expressed as 25
  # one-hour periods because CloudWatch caps a single period at 86400s -- a bare
  # period=90000 is rejected at the API (apply-time), invisible to validate.
  period              = 3600
  evaluation_periods  = 25
  threshold           = 1
  comparison_operator = "LessThanThreshold"
  treat_missing_data  = "breaching" # NO data in an hour = not invoked = counts toward the alarm
  alarm_actions       = [aws_sns_topic.ops_alarms.arn]
  tags                = module.label.tags
}
```

Adjust the one reference the plan leaves file-local: the label module instance (`module.label` above) -- create a file-local `module "label_alarms"` following `aws-dynamodb.tf`'s `module "label_table"` pattern (label/null 1.0.1, `context = module.context.shared`, `name = "alarms"`), and use `module.label_alarms.id`/`.tags`. Add `variable "ops_alarm_email"` to `terraform/tf-variables.tf` with `description = "Email endpoint for the ops-alarm SNS topic."` and NO default (the deploy tfvars provides it -- ben's contact email, never committed).

- [ ] **Step 2: Validate**

Run: `cd terraform && terraform fmt -check aws-cloudwatch-alarms.tf && terraform validate` (validate needs init; if the backend isn't initialized in this clone, `terraform init -backend=false` first — read-only, no state touched).
Expected: fmt clean, validate passes.

- [ ] **Step 3: Commit**

```bash
git add terraform
git commit -S -m "feat(terraform): out-of-process alarms -- fulfillment errors + 25h-silent, sns to ben"
```

---

### Task 9: workspace verify + docs touch

**Files:**
- Modify: `docs/superpowers/specs/2026-07-29-dead-key-truth-design.md` (status line → implemented-by reference)

- [ ] **Step 1: Full trust-nothing verify** (stale-binary lesson: clean the crates whose results matter):

```bash
flock /tmp/claude-cargo.lock cargo clean -p humble-client -p domain -p dynamo -p fulfillment -p public-api
flock /tmp/claude-cargo.lock cargo fmt --all -- --check
flock /tmp/claude-cargo.lock cargo clippy --all-targets
flock /tmp/claude-cargo.lock cargo test --workspace
cd web && npm run typecheck && npm run lint && npm run build
```

Expected: everything green; integration tests RAN (check counts), not skipped.

- [ ] **Step 2: Update the spec status line** to `status: implemented on kitten/dead-key-truth (plan docs/superpowers/plans/2026-07-29-dead-key-truth.md)` and commit:

```bash
git add docs/
git commit -S -m "docs: dead-key-truth spec → implemented status"
```

---

## Post-plan arc (NOT plan tasks — pounce steps 7-14, for the record)

PR → two /review passes (findings posted on-thread per the 2026-07-06 rule) → OMBB sign-off (discord) → merge (my own PR, hard rule 1) → full deploy as kitten-deploy (lambdas + the new alarms ⇒ terraform apply per terraform/README "Deploying as kitten"; `ops_alarm_email` rides the deploy tfvars, ben confirms the SNS email once) → admin sync-now → live acceptance: doom `87b9a4d8` transitions to `failed` autonomously with one ping; exhausted pair starts daily nags; pending GSI 3→2; friend page shows `returned`; the two alarms exist in `OK` state; reveal to ben.
