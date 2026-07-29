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

- [ ] **Step 1: Write the failing tests** (append to `client_test.rs`). Scaffolding pointers, verified: the REDEEM-path pattern to copy is `redeems_as_gift` (:122) / `already_redeemed_is_typed` (:146) -- client construction `client(&server).await`, invocation `.redeem_as_gift("KEY", "machine", 0)`. Do NOT copy the test at :1361 (`reveal_key_refused_reads_error_msg_field`) for the redeem tests -- that one exercises the REVEAL path (`.reveal_key(...)`); copying it would produce four `redeem_*`-named tests that never touch `redeem_once`. The snippets below are written against that real scaffolding directly:

```rust
#[tokio::test]
async fn redeem_expired_key_maps_to_key_expired() {
    // The live 2026-07-09 doom_eternal refusal, byte-exact (cloudwatch receipt).
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/humbler/redeemkey"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": false,
            "errormsg": "This key has expired and can no longer be redeemed."
        })))
        .mount(&server)
        .await;
    let err = client(&server)
        .await
        .redeem_as_gift("AAAAbbbbCCCC", "doom_eternal_choice_steam", 0)
        .await
        .unwrap_err();
    match err {
        humble_client::HumbleError::KeyExpired { msg, code } => {
            assert_eq!(msg, "This key has expired and can no longer be redeemed.");
            assert_eq!(code, None); // no machine code captured live for the expired class yet
        }
        other => panic!("expected KeyExpired, got {other:?}"),
    }
}

#[tokio::test]
async fn expired_phrase_survives_humble_text_drift() {
    // contains-match on the long phrase (OMBB claw #2, mirroring the already-redeemed
    // precedent): prefix/suffix/CASE drift must not silently degrade terminal detection
    // back to park-forever. The mock varies case ON the phrase itself so the
    // .to_lowercase() in classify_refusal is load-bearing, not decorative.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/humbler/redeemkey"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": false,
            "errormsg": "Sorry! This key HAS EXPIRED and can NO LONGER be redeemed"
        })))
        .mount(&server)
        .await;
    let err = client(&server)
        .await
        .redeem_as_gift("AAAAbbbbCCCC", "doom_eternal_choice_steam", 0)
        .await
        .unwrap_err();
    assert!(matches!(err, humble_client::HumbleError::KeyExpired { .. }));
}

#[tokio::test]
async fn redeem_refusal_with_error_code_still_maps_to_redeem_refused() {
    // The keys_depleted_email shape (fixture precedent: reveal test at :1361) must be
    // UNCHANGED in classification by the new code parse: unknown/untyped codes fall
    // through byte-for-byte -- but the code now RIDES the error.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/humbler/redeemkey"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"error":"keys_depleted_email","error_msg":"Keys are temporarily exhausted for this product","success":false}"#,
        ))
        .mount(&server)
        .await;
    let err = client(&server)
        .await
        .redeem_as_gift("AAAAbbbbCCCC", "soulcalibur6_monthly_steam", 0)
        .await
        .unwrap_err();
    match err {
        humble_client::HumbleError::RedeemRefused { msg, code } => {
            assert_eq!(msg, "Keys are temporarily exhausted for this product");
            assert_eq!(code.as_deref(), Some("keys_depleted_email"));
        }
        other => panic!("expected RedeemRefused, got {other:?}"),
    }
}

#[tokio::test]
async fn short_expired_text_without_the_long_phrase_stays_redeem_refused() {
    // The contains-match keys on the LONG phrase; a short fragment lacking
    // "can no longer be redeemed" must NOT be buried as terminal.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/humbler/redeemkey"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": false,
            "errormsg": "This key has expired."
        })))
        .mount(&server)
        .await;
    let err = client(&server)
        .await
        .redeem_as_gift("AAAAbbbbCCCC", "some_steam", 0)
        .await
        .unwrap_err();
    assert!(matches!(err, humble_client::HumbleError::RedeemRefused { .. }));
}

#[tokio::test]
async fn reveal_expired_key_maps_to_key_expired() {
    // The REVEAL path routes through the same classify_refusal (step 3e) -- this is the
    // ONLY pin that the classifier got wired into reveal_once at all. Scaffolding
    // mirrors reveal_key_refused_reads_error_msg_field (:1361).
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/humbler/redeemkey"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"success":false,"errormsg":"This key has expired and can no longer be redeemed."}"#,
        ))
        .mount(&server)
        .await;
    let out = client(&server).await.reveal_key("GK", "mn_steam", 0).await;
    assert!(matches!(
        out,
        Err(humble_client::HumbleError::KeyExpired { .. })
    ));
}
```

All five are written against the file's REAL scaffolding (`client(&server).await` + `.redeem_as_gift(...)` / `.reveal_key(...)` -- the patterns at :122/:146/:1361); no fictional helpers.

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

3f. **Task-1-interim classification arms (B-1).** The new variant makes two deliberately `_`-free exhaustive matches in FULFILLMENT non-exhaustive (E0004) the moment this task compiles the workspace: `gift_error_decision` (fulfillment `lib.rs:192`) and `choose_decision` (`:264`). The correct `DeadKey` classification is Task 4's product and must NOT be invented here. Add exactly these arms:

In `gift_error_decision`:

```rust
        // interim -- Task 4 reclassifies this arm to Decision::DeadKey. Park is the
        // safe holding value (today's behavior for every refusal).
        HumbleError::KeyExpired { .. } => Decision::Park,
```

In `choose_decision` (this arm is PERMANENT, not interim):

```rust
        // choose_content never yields KeyExpired (it spends picks, it doesn't redeem
        // keys) -- classified conservatively as Park; reconcile's order diff decides.
        HumbleError::KeyExpired { .. } => Decision::Park,
```

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
- Modify (compile ripples): `crates/dynamo/src/lib.rs` recheck matches (~:1356-1361 `compensate_claim`, ~:1446-1451 `compensate_self_claim`) + fulfill recheck error strings (`:1086-1090`, `:1138-1142`); there is no `_` arm anywhere on `ClaimState`
- Modify (test-literal ripples -- every `Claim { .. }` struct literal gains `failure_reason: None`): `crates/public-api/tests/api_test.rs` (`:1962`, `:2023`), `crates/admin-api/tests/api_test.rs` (`:1075`, `:1496`), plus any further literals the `--all-targets` check flags in dynamo/fulfillment test files
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

3c. Run `flock /tmp/claude-cargo.lock cargo check --workspace --all-targets 2>&1 | grep -B2 -A6 "non-exhaustive\|missing field"` -- the compiler enumerates every `ClaimState` match missing `Failed` and every `Claim` literal missing `failure_reason`. The recheck-after-cancellation MATCHES are ONLY in `compensate_claim` (~:1356) and `compensate_self_claim` (~:1446) -- the `fulfill_claim`/`fulfill_self_claim` rechecks are `!=` COMPARISONS (`:1086`, `:1138`), which the compiler will NOT flag. Conscious decision for those two: a stale fulfill retry that finds `Failed` errors loudly (correct -- fulfill must never override a terminal fail), but its error text says "fulfill lost to compensate", which is now a lie. Update BOTH sites' error text, PRESERVING each site's operational recovery tail verbatim:
- `:1087-1089`: `"fulfill lost to compensate — gift URL needs manual/reconcile recovery"` becomes `"fulfill lost — claim already terminal (compensated or failed); gift URL needs manual/reconcile recovery"`
- `:1139-1141`: `"fulfill_self lost to compensate — revealed key needs manual/reconcile recovery"` becomes `"fulfill_self lost — claim already terminal (compensated or failed); revealed key needs manual/reconcile recovery"`

At the two compensate match sites, add the arm:

```rust
                            // Failed is terminal and owned by the dead-key transaction —
                            // like Fulfilled, someone else already decided this claim's
                            // fate; a late compensate/fulfill retry is a no-op.
                            ClaimState::Failed => return Ok(()),
```

Also fix the struct-literal compile errors: every place a `Claim` is constructed (`claim_game` ~:827, `claim_game_self` ~:964, and any test builders the compiler flags) gains `failure_reason: None,`.

- [ ] **Step 4: Run tests**

Run: `flock /tmp/claude-cargo.lock cargo test -p domain && flock /tmp/claude-cargo.lock cargo check --workspace --all-targets`
Expected: domain PASS; the WHOLE workspace including every test target compiles clean (the `Claim` literal ripple reaches public-api and admin-api TEST files -- plain `cargo check` misses them and would hand Task 3 a dirty tree).

- [ ] **Step 5: Commit**

```bash
git add crates/domain crates/dynamo crates/public-api crates/admin-api
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
    let Some(store) = store_or_skip("fail-deadkey").await else {
        return;
    };
    store.put_game(&game(1, true)).await.unwrap();
    store.create_link(&link("tok1")).await.unwrap();
    let now = datetime!(2026-07-02 12:00 UTC);
    let gid = game_id("gk1", "mn");
    store.claim_game("tok1", &gid, "c1", now).await.unwrap();

    store
        .fail_claim_dead_key("tok1", "c1", &gid, "This key has expired and can no longer be redeemed.")
        .await
        .unwrap();

    let c = store.get_claim("tok1", "c1").await.unwrap().unwrap();
    assert_eq!(c.state, ClaimState::Failed);
    assert_eq!(
        c.failure_reason.as_deref(),
        Some("This key has expired and can no longer be redeemed.")
    );
    let g = store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(g.status, GameStatus::Expired);
    assert_eq!(g.claim_id, None);
    assert!(!g.is_listable(), "a dead-key game must never re-list");
    assert_eq!(store.list_listable_games().await.unwrap().len(), 0);
    assert_eq!(
        store.get_link("tok1").await.unwrap().unwrap().claims_used,
        0,
        "the friend's slot returns"
    );
    assert!(
        store.list_pending_claims().await.unwrap().iter().all(|p| p.id != "c1"),
        "pending marker consumed -- claim leaves the GSI"
    );
}

#[tokio::test]
async fn fail_claim_dead_key_is_idempotent() {
    // Mirror of compensate_is_idempotent: 2-slot link so single-vs-double decrement is
    // visible in the counter; retry must not double-decrement NOR overwrite the reason.
    let Some(store) = store_or_skip("fail-deadkey-idem").await else {
        return;
    };
    let mut lnk = link("tok1");
    lnk.claims_allowed = 2;
    store.create_link(&lnk).await.unwrap();
    store.put_game(&game(1, true)).await.unwrap();
    store.put_game(&game(2, true)).await.unwrap();
    let now = datetime!(2026-07-02 12:00 UTC);
    let a = game_id("gk1", "mn");
    let b = game_id("gk2", "mn");
    store.claim_game("tok1", &a, "cA", now).await.unwrap();
    store.claim_game("tok1", &b, "cB", now).await.unwrap();

    store.fail_claim_dead_key("tok1", "cA", &a, "first words").await.unwrap();
    assert_eq!(store.get_link("tok1").await.unwrap().unwrap().claims_used, 1);

    store.fail_claim_dead_key("tok1", "cA", &a, "second words").await.unwrap();
    assert_eq!(
        store.get_link("tok1").await.unwrap().unwrap().claims_used,
        1,
        "retry must not double-decrement"
    );
    let c = store.get_claim("tok1", "cA").await.unwrap().unwrap();
    assert_eq!(
        c.failure_reason.as_deref(),
        Some("first words"),
        "retry is a no-op, not an overwrite -- the FIRST reason is the durable truth"
    );
}

#[tokio::test]
async fn fail_self_claim_dead_key_skips_link_decrement() {
    let Some(store) = store_or_skip("fail-self-deadkey").await else {
        return;
    };
    store.put_game(&game(1, true)).await.unwrap();
    let now = datetime!(2026-07-02 12:00 UTC);
    let gid = game_id("gk1", "mn");
    store.claim_game_self(&gid, "c-f1", now).await.unwrap();

    store
        .fail_self_claim_dead_key("c-f1", &gid, "Keys are temporarily exhausted for this product")
        .await
        .unwrap();

    let c = store.get_claim(SELF_LINK_TOKEN, "c-f1").await.unwrap().unwrap();
    assert_eq!(c.state, ClaimState::Failed);
    // lilith's rider is "reason ON the claim item" -- BOTH variants persist it.
    assert_eq!(
        c.failure_reason.as_deref(),
        Some("Keys are temporarily exhausted for this product")
    );
    let g = store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(g.status, GameStatus::Expired);
}

#[tokio::test]
async fn stale_fulfill_after_dead_key_fail_stays_loud() {
    // Mirror of the stale-fulfill-after-compensate guard (~:743-754) -- and Task 2
    // EDITS that error text, so the modified guard gets its own pin here.
    let Some(store) = store_or_skip("fail-deadkey-stalefulfill").await else {
        return;
    };
    store.put_game(&game(1, true)).await.unwrap();
    store.create_link(&link("tok1")).await.unwrap();
    let now = datetime!(2026-07-02 12:00 UTC);
    let gid = game_id("gk1", "mn");
    store.claim_game("tok1", &gid, "c1", now).await.unwrap();
    store.fail_claim_dead_key("tok1", "c1", &gid, "dead").await.unwrap();

    let res = store
        .fulfill_claim("tok1", "c1", &gid, "https://www.humblebundle.com/gift?key=x")
        .await;
    assert!(
        res.is_err(),
        "stale fulfill after dead-key fail must take the loud path, not silently flip: {res:?}"
    );
    let g = store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(g.status, GameStatus::Expired, "the retired game must stay retired");
}

#[tokio::test]
async fn fail_dead_key_gift_variant_cancels_on_self_partition() {
    // Mirror of the compensate pin at ~:1521-1525: WHY the self variant exists.
    let Some(store) = store_or_skip("fail-deadkey-selfpin").await else {
        return;
    };
    store.put_game(&game(1, true)).await.unwrap();
    let now = datetime!(2026-07-02 12:00 UTC);
    let gid = game_id("gk1", "mn");
    store.claim_game_self(&gid, "c-f2", now).await.unwrap();

    let wrong = store.fail_claim_dead_key(SELF_LINK_TOKEN, "c-f2", &gid, "dead").await;
    assert!(
        wrong.is_err(),
        "gift fail variant must cancel on the absent LINK META"
    );

    store.fail_self_claim_dead_key("c-f2", &gid, "dead").await.unwrap();
    let c = store.get_claim(SELF_LINK_TOKEN, "c-f2").await.unwrap().unwrap();
    assert_eq!(c.state, ClaimState::Failed);
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

The choose-ladder pinning test lives at `crates/fulfillment/src/lib.rs:3496` (`choose_decision_ladder_never_compensates`, an INLINE cfg(test) module; `choose_decision` appears nowhere in handler_test.rs): append `E::KeyExpired { msg: "x".into(), code: None }` to its `park_variants` array with the comment below. NOTE (B-1): this is a PIN, not a red -- Task 1's 3f already added the permanent Park arm, so this test passes immediately; the array entry keeps the every-variant convention honest.

```rust
        // choose_content never yields KeyExpired (it spends picks, it doesn't redeem
        // keys) -- classified conservatively as Park; reconcile's order diff decides.
```

ALSO append a `KeyExpired { msg: "x".into(), code: None }` entry to `reveal_decision_ladder_matches_gift_decision` (handler_test.rs:3202), following that test's existing per-variant agreement convention -- one line keeps the file's every-variant discipline honest for the new variant.

- [ ] **Step 2: Run to verify failure**

Run: `flock /tmp/claude-cargo.lock cargo test -p fulfillment --test handler_test ladder && flock /tmp/claude-cargo.lock cargo test -p fulfillment --lib choose_decision_ladder`
Expected (B-1 shape): the GIFT ladder test FAILS as an ASSERTION -- `gift_decision(KeyExpired)` returns Task 1's interim `Park`, the test demands `DeadKey`; that mismatch is the true red. (The reveal-side `check_agree!` line is agreement-only -- delegation makes it pass even at red time.) The `--lib` choose ladder already PASSES (its Park arm is permanent from Task 1) -- a 0-test or compile-error outcome on either command means the census or Task 1 was incomplete. Both commands re-run green at Step 5.

- [ ] **Step 3: Implement the enum + classifications**

3a. `Decision`:

```rust
    /// The key is definitively DEAD (humble: expired, unredeemable forever). Terminal:
    /// fail the claim with its reason, return the slot, retire the game as Expired.
    /// NEVER park (retry cannot succeed) and NEVER compensate (re-listing would hand
    /// the next friend the same dead key).
    DeadKey,
```

3b. `gift_error_decision`: REPLACE Task 1's interim `Park` arm (delete its interim comment) with:

```rust
        // Definitively dead server-side — terminal, not park: retrying a key humble
        // has expired loops forever (live receipt: claim 87b9a4d8, 21 silent days).
        HumbleError::KeyExpired { .. } => Decision::DeadKey,
```

3c. `choose_decision`: NO change -- confirm the permanent `KeyExpired { .. } => Decision::Park` arm from Task 1 step 3f is present with its comment.

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
- Reconcile B2's response match (`reconcile_choice_claim` ~:1595-1603): the `other =>` arm logs "claim stays pending" -- FALSE for `KeyDead` (the doom acceptance path lands here). Add an explicit arm BEFORE `other =>`: `FulfillResponse::KeyDead => tracing::info!(claim_id = %claim.id, "reconcile(choice): dead key -- claim terminally failed from reconcile"),`

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
    let Some(store) = store_or_skip("gift-deadkey").await else {
        return;
    };
    let gid = seed_pending_claim(&store, "gk1", "mn").await;

    let humble = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/humbler/redeemkey"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": false,
            "errormsg": "This key has expired and can no longer be redeemed."
        })))
        .mount(&humble)
        .await;
    let discord = discord_ok().await;

    let deps = deps(store, &humble.uri(), Some(discord.uri()));
    let resp = handle(&deps, gift_req(&gid, "gk1", "mn")).await;
    assert_eq!(resp, FulfillResponse::KeyDead);

    let claim = deps.store.get_claim("tok1", "c1").await.unwrap().unwrap();
    assert_eq!(claim.state, ClaimState::Failed);
    assert_eq!(
        claim.failure_reason.as_deref(),
        Some("This key has expired and can no longer be redeemed.")
    );
    let game = deps.store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(game.status, GameStatus::Expired);
    assert_eq!(
        deps.store.list_listable_games().await.unwrap().len(),
        0,
        "a dead-key game must not re-list"
    );
    assert_eq!(
        deps.store.get_link("tok1").await.unwrap().unwrap().claims_used,
        0,
        "slot returned"
    );

    let reqs = discord.received_requests().await.unwrap();
    assert_eq!(reqs.len(), 1, "exactly one transition ping");
    let body = String::from_utf8(reqs[0].body.clone()).unwrap();
    assert!(body.contains("DEAD key") && body.contains("c1"));
    assert!(!body.contains("AAAA"), "never a key value in a ping");
}

#[tokio::test]
async fn structural_expired_tpk_fails_without_a_redeem_call() {
    // Drive path mirrors reconcile_self_choice_b2_reveals_never_chooses (:3548):
    // choice claim + pre=[] snapshot + order carrying the tpk -> reconcile branch B2
    // -> claimed_tpk_terminal, where the structural pre-check must fire FIRST.
    let Some(store) = store_or_skip("deadkey-structural").await else {
        return;
    };
    seed_choice_game(&store, "gkH:off_h", "Dead On Arrival").await;
    store
        .claim_game_self("gkH:off_h", "sc-dk", old_enough())
        .await
        .unwrap();
    store
        .record_choice_intent(SELF_LINK_TOKEN, "sc-dk", vec![])
        .await
        .unwrap();

    let humble = MockServer::start().await;
    mount_empty_listing(&humble).await;
    // Order mounted INLINE with is_expired: true -- do NOT modify tpk_json or
    // mount_order_with_unredeemed_tpk (M-4: those are SHARED fixtures; editing them
    // silently reshapes the fixture under the whole file).
    Mock::given(method("GET"))
        .and(path("/api/v1/order/gkH"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "gamekey": "gkH",
            "product": { "human_name": "Choice Month" },
            "tpkd_dict": { "all_tpks": [{
                "machine_name": "off_h_choice_steam",
                "human_name": "Dead On Arrival",
                "key_type": "steam",
                "is_expired": true,
                "keyindex": 0,
            }]},
            "subproducts": [],
        })))
        .mount(&humble)
        .await;
    // Deliberately NO reveal/redeem mock: a structurally dead key must spend no humble write.
    let discord = discord_ok().await;

    let deps_val = deps(store.clone(), &humble.uri(), Some(discord.uri()));
    run_reconcile(&deps_val).await;

    let claim = store.get_claim(SELF_LINK_TOKEN, "sc-dk").await.unwrap().unwrap();
    assert_eq!(claim.state, ClaimState::Failed);
    assert!(
        claim.failure_reason.as_deref().unwrap_or_default().contains("expired"),
        "the structural reason names the is_expired flag: {:?}",
        claim.failure_reason
    );
    let reqs = humble.received_requests().await.unwrap();
    assert_eq!(
        count_path(&reqs, "/humbler/redeemkey"),
        0,
        "no redeem/reveal call may be spent on a structurally dead key"
    );
    let pings = discord.received_requests().await.unwrap();
    assert!(
        pings.iter().any(|r| String::from_utf8(r.body.clone()).unwrap().contains("expired")),
        "the transition ping fires"
    );
}
```

- [ ] **Step 5: Run the fulfillment suite**

Run: `flock /tmp/claude-cargo.lock cargo test -p fulfillment`
Expected: PASS; confirm the two new integration tests RAN (dynamo-local reachable), not skipped.

- [ ] **Step 6: Commit**

```bash
git add crates/fulfillment
git commit -S -m "feat(fulfillment): Decision::DeadKey — terminal dead-key path with structural tpk.expired pre-check"
```

---

### Task 5: fulfillment — pending-age escalation sweep (set-driven, at the TOP of run_sync)

**Files:**
- Modify: `crates/fulfillment/src/lib.rs` — new fn `pending_age_sweep` (place it directly above `reconcile` ~:3110) + ONE call at the very top of `run_sync` (~:2743, before the listing acquisition). `reconcile` itself is UNCHANGED.
- Modify: `crates/fulfillment/tests/handler_test.rs` — new placement-pin test + two existing exactly-once ping tests updated (B-3, Step 4)

**Interfaces:**
- Consumes: `list_pending_claims()` (existing), `RECONCILE_STUCK_ALERT_AGE` (existing, 24h), `ping()` (existing).
- Produces: no new public surface — an invariant: every pending claim older than 24h produces one ping per sync, from the top of `run_sync`, before ANY humble acquisition (gate review B-4: the sweep needs only dynamo + discord; placement above every early return is the whole point, and it is PINNED by this task's test).

- [ ] **Step 1: Write the failing test:**

```rust
#[tokio::test]
async fn stale_pending_claim_pings_even_when_listing_is_dead() {
    // B-4 placement pin (lilith's rider on the gate): "the sweep ran even though
    // everything after it died." A dead-cookie LISTING -- run_sync's :2766 early
    // return, where reconcile is NEVER called -- must not starve the sweep. If a
    // future refactor adds an early return above the sweep call, THIS test fails.
    let Some(store) = store_or_skip("stale-sweep-deadlisting").await else {
        return;
    };
    seed_aged_pending(&store, &game_id("gkS9", "mnSTALE"), "tokS9", "cS9", hours_ago(26)).await;

    let humble = MockServer::start().await;
    // Listing 302 -> /login = Unauthorized; deps() has no session_store, so no heal:
    // run_sync pings COOKIE_DEAD and returns before reconcile ever runs.
    Mock::given(method("GET"))
        .and(path("/api/v1/user/order"))
        .respond_with(ResponseTemplate::new(302).append_header("location", "/login"))
        .mount(&humble)
        .await;
    let discord = discord_ok().await;

    let deps = deps(store, &humble.uri(), Some(discord.uri()));
    handle(&deps, FulfillRequest::Sync).await;

    let reqs = discord.received_requests().await.unwrap();
    let bodies: Vec<String> = reqs
        .iter()
        .map(|r| String::from_utf8(r.body.clone()).unwrap())
        .collect();
    assert!(
        bodies.iter().any(|b| b.contains("STILL PENDING") && b.contains("cS9")),
        "sweep must ping the stale claim even though the listing died: {bodies:?}"
    );
    assert!(
        bodies.iter().any(|b| b.contains("session")),
        "the cookie-dead ping also fires on this lane"
    );
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `flock /tmp/claude-cargo.lock cargo test -p fulfillment --test handler_test stale_pending_claim`
Expected: **1 test run, 1 failed** — the STILL PENDING assertion. The filter is the test FN name (`stale_pending_claim…`), NOT the `store_or_skip` label; a "0 tests run; ok" outcome means the filter missed and the red was never observed — that is NOT a red.

- [ ] **Step 3: Implement.** TWO edits:

3a. New fn, placed directly above `reconcile` (~:3110):

```rust
/// ── Pending-age sweep (spec §3, set-driven; placement per gate review B-4) ──────
/// The FIRST thing run_sync does, before any humble acquisition: the sweep needs
/// only dynamo + the discord webhook, so no session death, listing failure, or
/// reconcile regression can starve it. The invariant is on the SET: every claim
/// both pending and older than RECONCILE_STUCK_ALERT_AGE pings, every sync, until
/// a terminal transition removes it from the GSI. Daily-by-cadence (sync schedule),
/// deliberately NOT deduplicated: a once-ever alert that scrolls away IS the
/// silent-loop bug this exists to kill (family review 2026-07-29). Placement is
/// PINNED by stale_pending_claim_pings_even_when_listing_is_dead — an early return
/// added above this call fails that test.
async fn pending_age_sweep(deps: &Deps) {
    let claims = match deps.store.list_pending_claims().await {
        Ok(c) => c,
        Err(_) => return, // can't read this pass — the next sync retries.
    };
    let now = OffsetDateTime::now_utc();
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
}
```

3b. The call — the FIRST statement in `run_sync` (~:2743), immediately after its opening `tracing::info!` line and BEFORE the listing acquisition:

```rust
    // Watchdog first (gate review B-4): needs no humble session — must run before
    // anything that can die. Do not add early returns above this line.
    pending_age_sweep(deps).await;
```

`reconcile` itself is UNCHANGED — no sweep code inside it.

- [ ] **Step 4: Update the two exactly-once ping tests (B-3), then run the suite.**

The sweep double-pings two EXISTING tests whose 30h-old seeds now correctly (spec §3 accepts the double ping) receive BOTH the structural stuck-alert AND the sweep ping:
- `reconcile_unreconcilable_over_threshold_pings_once` (handler_test.rs:1148)
- `reconcile_unsplittable_game_id_over_threshold_pings` (handler_test.rs:1192)

Update BOTH: keep the 30h seeds EXACTLY as they are (aged seeds are the tests' point), change `assert_eq!(reqs.len(), 1)` to `assert_eq!(reqs.len(), 2)`, and assert one body contains the stuck-alert copy (`"cannot act on"`) and one contains `"STILL PENDING"`. Do NOT add dedup to the sweep to make them pass — the double ping is the specified behavior for structurally-stuck claims. Any OTHER test that newly fails on an unexpected extra ping seeded its claim older than 24h without meaning to; those seeds may be brought under 24h (but ≥ RECONCILE_MIN_AGE) — the two tests named above may NOT.

Run: `flock /tmp/claude-cargo.lock cargo test -p fulfillment`
Expected: PASS, with the two updated tests asserting 2 pings and Task 4's fresh-claim tests unaffected (their seeds are younger than 24h).

- [ ] **Step 5: Commit**

```bash
git add crates/fulfillment
git commit -S -m "feat(fulfillment): set-driven pending-age sweep — no claim is pending and silent past 24h"
```

---

### Task 6: public-api + admin-api — `KeyDead` → HTTP 410 everywhere it surfaces

**Files:**
- Modify: `crates/public-api/src/lib.rs` (~:680-703, the claim outcome match + its log match)
- Modify: `crates/admin-api/src/lib.rs` — the ONE `FulfillResponse` match in the crate, `handle_self_claim`'s `match s.invoker.call(req).await` at :774-798 (the `Ok(_) | Err(_)` catch-all at ~:793 is INSIDE this match, not a second site; :852 is sync's `fire`, no FulfillResponse arms); it currently folds `KeyDead` into a 500 whose copy ("the claim is recorded") is wrong twice for a terminally failed claim
- Test: `crates/public-api/tests/api_test.rs` (MockInvoker rig at :100-135) + `crates/admin-api/tests/api_test.rs` (MockAdminInvoker/MockCallInvoker rigs at :86-135)

**Interfaces:**
- Consumes: `FulfillResponse::KeyDead` (Task 4).
- Produces: friend HTTP 410 `{"error": "that key can't be redeemed anymore — pick another"}` (verb matches the NEIGHBORING AlreadyRedeemed 410 at public-api :692 — two adjacent 410s must not disagree; this supersedes the spec's copy discussion, "pick" here is the neighbor's plain English, not choice jargon); admin HTTP 410 `{"error": "key is dead on humble's side — claim failed terminally, reason recorded on the claim"}` on both admin matches. Task 7 renders the friend message through the existing `refused` lane.

- [ ] **Step 1: public-api log arm** (in the outcome-logging match ~:682):

```rust
        Ok(FulfillResponse::KeyDead) => tracing::info!("claim: dead-key (410)"),
```

- [ ] **Step 2: public-api response arm** (before the `_ => park_response()` arm):

```rust
        Ok(FulfillResponse::KeyDead) => (
            StatusCode::GONE,
            Json(serde_json::json!({
                "error": "that key can't be redeemed anymore — pick another"
            })),
        )
            .into_response(),
```

- [ ] **Step 3: admin-api arm — ONE match site.** In `handle_self_claim`'s match (:774-798), add before the `Ok(_) | Err(_)` catch-all, mirroring the neighboring `AlreadyRedeemed => GONE` construction:

```rust
        Ok(FulfillResponse::KeyDead) => (
            StatusCode::GONE,
            Json(serde_json::json!({
                "error": "key is dead on humble's side — claim failed terminally, reason recorded on the claim"
            })),
        )
            .into_response(),
```

- [ ] **Step 4: mirror tests, UNCONDITIONAL** (the rigs exist — `MockInvoker::new(FulfillResponse::…)` public-api api_test.rs:100-135; the admin mock invokers at admin-api api_test.rs:86-135; there is no "no harness" escape):
  - public-api: `grep -n "MockInvoker::new" crates/public-api/tests/api_test.rs`, copy the claim-endpoint test that drives a non-200 FulfillResponse through the POST claim route, swap the mock to `MockInvoker::new(FulfillResponse::KeyDead)`, assert status 410 and the byte-exact body from Step 2.
  - admin-api: same recipe once — copy the nearest AlreadyRedeemed/GONE-shaped self-claim test (`grep -n "GONE\|already redeemed" crates/admin-api/tests/api_test.rs`), swap the mock's response to `FulfillResponse::KeyDead`, assert 410 + the byte-exact Step 3 body.

- [ ] **Step 5: Run**

Run: `flock /tmp/claude-cargo.lock cargo test -p public-api -p admin-api && flock /tmp/claude-cargo.lock cargo clippy --all-targets -p public-api -p admin-api`
Expected: PASS / clean, including the TWO new mirror tests (one public-api, one admin-api).

- [ ] **Step 6: Commit**

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

- [ ] **Step 3: Admin badges.** In `Links.tsx` `stateBadgeClass`, add a `case 'failed':` returning a rose-toned class consistent with the function's existing entries (read the function; mirror its class-string style, e.g. `'bg-rose-950 text-rose-200'`). ALSO: `web/src/admin/Catalog.tsx:338` branches on `sc.state === 'compensated'` for self-claims — the rendered LABEL there is the raw `{sc.state}` expression, so no label edit is possible or wanted: extend only the CLASS ternary with a `sc.state === 'failed'` case using the same rose tone (read the existing ternary and mirror its structure). Naming stays two-names-total across the product: the friend chip says `returned` (brand-warm), every admin surface shows the raw wire state `failed`.

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
- Modify: `terraform/tf-variables.tf` (ADD the `ops_alarm_email` variable — it does not exist yet)

**Interfaces:**
- Consumes: the fulfillment lambda's function name -- `module.lambda_fulfillment.lambda_function_name` (lambdas in this repo are MODULE instantiations, `bendoerr-terraform-modules/lambda/aws`; there is no `resource "aws_lambda_function"` anywhere. Precedent for exactly this reference: `aws-eventbridge.tf:26`).
- Produces: two `aws_cloudwatch_metric_alarm`s + one SNS topic with ben's email subscribed. Applied at DEPLOY time (pounce step 11), not by CI.

- [ ] **Step 1: Write the terraform** (mirror the file-local label-module + tag conventions the other terraform files use — read `aws-eventbridge.tf` for the smallest example):

```hcl
# The watchdog's watchdog (spec §3, OMBB's claw #3): the pending-age sweep lives
# INSIDE the fulfillment lambda — if the cron misfires, a deploy bricks the
# function, or IAM rots, the sweep dies with it and every in-process alarm dies
# too. These two alarms are the out-of-process layer: they fire when the sync
# lambda errors, or when it goes silent for 24h -- the maximum window CloudWatch
# can express (see the silent alarm's limits comment) against the daily schedule.
# Layer map: the sweep catches the claim reconcile
# never touches; these catch the reconcile that never runs.

resource "aws_sns_topic" "ops_alarms" {
  # label name is "ops" -> id already ends "-ops"; suffix once, not twice.
  name = "${module.label_alarms.id}-alarms"
  tags = module.label_alarms.tags
}

resource "aws_sns_topic_subscription" "ops_alarms_email" {
  topic_arn = aws_sns_topic.ops_alarms.arn
  protocol  = "email"
  endpoint  = var.ops_alarm_email # ben confirms the subscription once by mail
}

resource "aws_cloudwatch_metric_alarm" "fulfillment_errors" {
  alarm_name          = "${module.label_alarms.id}-fulfillment-errors"
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
  tags                = module.label_alarms.tags
}

resource "aws_cloudwatch_metric_alarm" "fulfillment_silent" {
  alarm_name        = "${module.label_alarms.id}-fulfillment-silent"
  alarm_description   = "bendobundles fulfillment lambda has not been invoked in 24h — the daily sync (and its pending-age sweep) is not running"
  namespace           = "AWS/Lambda"
  metric_name = "Invocations"
  dimensions  = { FunctionName = module.lambda_fulfillment.lambda_function_name }
  statistic   = "Sum"
  # 24 consecutive silent hours. CloudWatch enforces TWO limits invisible to
  # `terraform validate`: period <= 86400 AND period * evaluation_periods <= 86400
  # TOTAL -- so 3600x24 is the maximum expressible window (gate review B-2; both
  # 90000x1 and 3600x25 are rejected at the API, at apply time). Residual: with the
  # daily cron at a fixed minute, minute-level jitter can graze one false nag per
  # miss -- accepted; a false silent-alarm nag is the cheap direction.
  period              = 3600
  evaluation_periods  = 24
  threshold           = 1
  comparison_operator = "LessThanThreshold"
  treat_missing_data  = "breaching" # NO data in an hour = not invoked = counts toward the alarm
  alarm_actions       = [aws_sns_topic.ops_alarms.arn]
  tags                = module.label_alarms.tags
}
```

Adjust the one reference the plan leaves file-local: the label module instance (`module.label` above) -- create a file-local `module "label_alarms"` following `aws-dynamodb.tf`'s `module "label_table"` pattern (label/null 1.0.1, `context = module.context.shared`, `name = "ops"`), and use `module.label_alarms.id`/`.tags` as already written in the HCL above. Add `variable "ops_alarm_email"` to `terraform/tf-variables.tf` with `description = "Email endpoint for the ops-alarm SNS topic."` and NO default (the deploy tfvars provides it -- ben's contact email, never committed).

- [ ] **Step 2: Validate**

Run: `cd terraform && terraform fmt -check aws-cloudwatch-alarms.tf && terraform validate` (validate needs init; if the backend isn't initialized in this clone, `terraform init -backend=false` first — read-only, no state touched, but it DOES need network for provider/module downloads).
Expected: fmt clean, validate passes. Remember validate CANNOT see the CloudWatch API limits — the 3600×24 arithmetic above is load-bearing, do not "round it up".

- [ ] **Step 3: Commit**

```bash
git add terraform
git commit -S -m "feat(terraform): out-of-process alarms -- fulfillment errors + 24h-silent, sns to ben"
```

---

### Task 9: workspace verify + docs touch

**Files:**
- Modify: `docs/superpowers/specs/2026-07-29-dead-key-truth-design.md` (status line → implemented-by reference)

- [ ] **Step 1: Full trust-nothing verify** (stale-binary lesson: clean the crates whose results matter):

```bash
flock /tmp/claude-cargo.lock cargo clean -p humble-client -p domain -p dynamo -p fulfillment -p public-api -p admin-api
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

### Task 10: live acceptance (deploy-phase — run by the DEPLOYING OPERATOR after the terraform apply + admin sync-now, NOT by a coding subagent)

The spec's §4 acceptance criteria, as commands with expected outputs. Run all five after the first post-deploy sync completes (~5 min after the admin `POST /admin/api/sync` 202):

- [ ] **A1 — doom transitioned autonomously.** `AWS_PROFILE=kitten-debug aws dynamodb query --table-name brd-prod-ue1-bendobundles-table --index-name pending-claims --key-condition-expression "gsi2pk = :p" --expression-attribute-values '{":p":{"S":"PENDINGCLAIM"}}' --output json | python3 -c "import sys,json; d=json.load(sys.stdin); print(d['Count']); [print(json.loads(i['body']['S'])['id']) for i in d['Items']]"` → Expected: count **2**, and `87b9a4d8-…` is NOT in the list (was 3, with doom).
- [ ] **A2 — the failed claim carries its reason.** Read the doom claim item (pk `LINK#14e9ec4b0b984019975fa88d499bde2be5f8fae5c7494bb4a4ad8cef9416a7f4`, sk `CLAIM#87b9a4d8-9855-47fb-bc3b-aa0be42ed4a2` via `aws dynamodb get-item`) → Expected: body has `"state":"failed"` and `failure_reason` containing "expired".
- [ ] **A3 — exactly one transition ping fired for doom** (not a daily loop): `AWS_PROFILE=kitten-debug aws logs filter-log-events --log-group-name /aws/lambda/brd-prod-ue1-bendobundles-fulfillment --start-time <deploy-epoch-ms> --filter-pattern '"87b9a4d8"'` → Expected: the dead-key transition log line ONCE; NO new `reconcile(choice): terminal did not complete` lines after the transition.
- [ ] **A4 — the exhausted pair nags.** Same log filter for `"STILL PENDING"` → Expected: **three** entries on THIS first sync (claims `3f46c058` + `3da0c011` + `87b9a4d8` — the sweep runs before reconcile terminally fails doom, so doom is still pending at sweep time), dropping to exactly two (`3f46c058` + `3da0c011`) on subsequent syncs once doom has transitioned; the corresponding discord pings arrived in ben's ops feed.
- [ ] **A5 — the loop-pair is gone going forward.** Next scheduled sync (09:00 UTC): re-run A3's filter over the new window → Expected: zero `terminal did not complete` for `87b9a4d8`; A4's two nags recur (daily-by-design until ben acts). The two CloudWatch alarms exist in `OK` state (`aws cloudwatch describe-alarms --alarm-name-prefix brd-prod-ue1-bendobundles`).

Failure of any A-item = pounce step 13 (address fallout) before the reveal.

---

## Post-plan arc (NOT plan tasks — pounce steps 7-14, for the record)

PR → two /review passes (findings posted on-thread per the 2026-07-06 rule) → OMBB sign-off (discord) → merge (my own PR, hard rule 1) → full deploy as kitten-deploy (lambdas + the new alarms ⇒ terraform apply per terraform/README "Deploying as kitten"; `ops_alarm_email` rides the deploy tfvars, ben confirms the SNS email once) → admin sync-now → **Task 10's acceptance checklist** (the concrete commands live there) → reveal to ben.
