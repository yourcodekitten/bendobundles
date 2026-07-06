# Self-Claim (Admin Key Reveal) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ben can claim any unclaimed game for himself from the admin catalog — the system reveals the actual key string (durably recorded), with a one-click Steam-register button for steam keys.

**Architecture:** A new `reveal_key` humble-client call (sibling of `redeem_as_gift`, `gift` param omitted, parses `{success, key}`) feeds a new fulfillment `SelfClaim` path that reuses the proven decision/park/reconcile machinery with reveal-side terminals. Self-claims are Claim records under the reserved `LINK#SELF` partition with three SELF-specific store writes (intake without the listable-marker gate, fulfill writing `revealed_key` durable-first, compensate without the LINK decrement). Admin-api gains a synchronous RequestResponse invoker + two endpoints; the catalog gets an arm/confirm "claim for me" action.

**Tech Stack:** Rust (axum, aws-sdk-dynamodb, wreq), wiremock + moto-style dynamodb-local tests, React 18 + TypeScript + Vitest.

**Spec:** `docs/superpowers/specs/2026-07-06-self-claim-design.md` (review-hardened; read it before starting).

## Global Constraints

- All commits GPG-signed (`git commit -S`), authored `code kitten <yourcodekitten@gmail.com>`.
- No `_` catch-all arm on `HumbleError` matches — every variant consciously classified (crate convention).
- `revealed_key` must NEVER appear in any log line, ping, or friend-facing response. Logs may name key *identifiers* (machine_name, keyindex, claim_id) — never key *values*.
- The reserved token is the literal `SELF` (`domain::SELF_LINK_TOKEN`). `/api/l/SELF` must stay a byte-identical 404.
- Reconcile must NEVER call `choose_content` — the existing merge-gate test discipline extends to all SELF paths.
- Do not build boring-sys locally from scratch (box lacks clang); run `cargo test -p <crate>` per-crate (cached artifacts), push and let CI verify the full matrix.
- moto/dynamodb-local port for tests: the suite manages its own; kitten's manual moto port is 8155 (siblings own 8123).

---

### Task 1: humble-client — `RevealedKey` + `reveal_key`

**Files:**
- Modify: `crates/humble-client/src/lib.rs` (types near `GiftUrl` at :283; methods near `redeem_as_gift` at :796; `reveal_once` near `redeem_once` at :1031)
- Test: `crates/humble-client/tests/client_test.rs`

**Interfaces:**
- Consumes: existing `csrf_write`, `is_login_required`, `decode_body`, `secure_area_step_up`, `HumbleError`.
- Produces: `pub struct RevealedKey(pub String);` and
  `pub async fn reveal_key(&self, gamekey: &str, machine_name: &str, keyindex: u32) -> Result<RevealedKey, HumbleError>` — exactly `redeem_as_gift`'s signature with a different success type. Task 5/6 depend on these names.

- [ ] **Step 1: Write the failing tests** (append to `crates/humble-client/tests/client_test.rs`, following the existing wiremock test style in that file — set up the mock server the same way the `redeem_as_gift` tests do):

```rust
#[tokio::test]
async fn reveal_key_success_returns_key_and_omits_gift_param() {
    let server = wiremock::MockServer::start().await;
    // Assert the form body: keytype/key/keyindex present, `gift` ABSENT.
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/humbler/redeemkey"))
        .and(wiremock::matchers::body_string_contains("keytype=stardew_valley_steam"))
        .and(wiremock::matchers::body_string_contains("keyindex=0"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .set_body_string(r#"{"key":"AAAA-BBBB-CCCC","success":true}"#),
        )
        .expect(1)
        .mount(&server)
        .await;
    let client = test_client(&server); // same helper the redeem tests use
    let out = client.reveal_key("GAMEKEY123", "stardew_valley_steam", 0).await;
    assert_eq!(out.unwrap(), humble_client::RevealedKey("AAAA-BBBB-CCCC".into()));
    // gift-param absence: fetch the received request and assert.
    let reqs = server.received_requests().await.unwrap();
    let body = String::from_utf8(reqs[0].body.clone()).unwrap();
    assert!(!body.contains("gift="), "reveal must not send the gift param: {body}");
}

#[tokio::test]
async fn reveal_key_already_redeemed_is_typed() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/humbler/redeemkey"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(
            r#"{"success":false,"errormsg":"This key has already been redeemed."}"#,
        ))
        .mount(&server)
        .await;
    let client = test_client(&server);
    let out = client.reveal_key("GK", "mn_steam", 0).await;
    assert!(matches!(out, Err(humble_client::HumbleError::AlreadyRedeemed)));
}

#[tokio::test]
async fn reveal_key_login_interstitial_is_unauthorized() {
    // 200-with-HTML login interstitial = dead session = Unauthorized (heal-ladder eligible).
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/humbler/redeemkey"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .set_body_string("<html><body>log in to continue</body></html>"),
        )
        .mount(&server)
        .await;
    let client = test_client(&server);
    let out = client.reveal_key("GK", "mn_steam", 0).await;
    assert!(matches!(out, Err(humble_client::HumbleError::Unauthorized)));
}

#[tokio::test]
async fn reveal_key_success_true_but_no_key_is_ambiguous() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/humbler/redeemkey"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_string(r#"{"success":true}"#),
        )
        .mount(&server)
        .await;
    let client = test_client(&server);
    let out = client.reveal_key("GK", "mn_steam", 0).await;
    assert!(matches!(out, Err(humble_client::HumbleError::AmbiguousRedeem)));
}
```

NOTE: match the *actual* login-interstitial detection — read `is_login_required` (crates/humble-client/src/lib.rs:143) and reuse whatever body the existing `redeem_as_gift` unauthorized-test uses, rather than the HTML sketched above. Same for `test_client` — reuse the file's existing constructor helper verbatim (name may differ; grep `fn test_client` / how redeem tests build the client).

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd ~/bendobundles && cargo test -p humble-client reveal_key 2>&1 | tail -20`
Expected: FAIL to compile — `reveal_key` and `RevealedKey` not found.

- [ ] **Step 3: Implement.** In `crates/humble-client/src/lib.rs`:

Next to `GiftUrl` (:283):

```rust
/// The revealed key VALUE from a no-gift redeem (`/humbler/redeemkey` without `gift`) — the
/// self-claim sibling of [`GiftUrl`]. Holds a live store key: NEVER log it (Debug derive is fine —
/// the value only surfaces where explicitly serialized).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevealedKey(pub String);
```

Next to `RedeemResponse` (:356):

```rust
#[derive(serde::Deserialize)]
struct RevealResponse {
    // Same contract as RedeemResponse: a 200 body missing `success` is a parse error.
    success: bool,
    // The revealed key. Observed as a JSON string on every live capture (2026-07-06 HAR: steam +
    // bungie keytypes); a non-string here (humble drift) must surface as ambiguous, not a panic —
    // hence Value, narrowed in the caller.
    #[serde(default)]
    key: Option<serde_json::Value>,
    #[serde(default)]
    errormsg: Option<String>,
}
```

Next to `RedeemStep` (:380):

```rust
/// Reveal analog of [`RedeemStep`].
enum RevealStep {
    Done(RevealedKey),
    StepUpNeeded { status: u16 },
}
```

After `redeem_as_gift` (:840), the public method — a structural copy of `redeem_as_gift`'s step-up dance with `reveal_once` in place of `redeem_once` (copy the doc comment discipline: gated reveal returns `login_required` BEFORE touching the key, so the post-step-up retry is the first attempt that can burn it):

```rust
    /// Reveal a key's VALUE (self-claim): `/humbler/redeemkey` with the `gift` param OMITTED —
    /// proven live 2026-07-06 (Ben library HAR: two plain-bundle reveals, steam + bungie keytypes,
    /// both `{"key":"…","success":true}`). Burns-once + step-up semantics identical to
    /// [`redeem_as_gift`]; see that method's safety comment.
    pub async fn reveal_key(
        &self,
        gamekey: &str,
        machine_name: &str,
        keyindex: u32,
    ) -> Result<RevealedKey, HumbleError> {
        match self.reveal_once(gamekey, machine_name, keyindex).await? {
            RevealStep::Done(k) => Ok(k),
            RevealStep::StepUpNeeded { status } => {
                if self.step_up.is_none() {
                    tracing::warn!(
                        status,
                        "reveal gated behind secure-area step-up but step-up is not configured — parking (set humble_username to enable)"
                    );
                    return Err(HumbleError::SecureAreaStepUpFailed {
                        reason:
                            "secure-area step-up required but not configured (set humble_username)"
                                .into(),
                    });
                }
                self.secure_area_step_up().await?;
                match self.reveal_once(gamekey, machine_name, keyindex).await? {
                    RevealStep::Done(k) => Ok(k),
                    RevealStep::StepUpNeeded { status } => {
                        tracing::warn!(
                            status,
                            "reveal still gated after a successful step-up — parking (key not burned)"
                        );
                        Err(HumbleError::SecureAreaStepUpFailed {
                            reason: "reveal still returned login_required after /processlogin accepted the step-up".into(),
                        })
                    }
                }
            }
        }
    }
```

And `reveal_once`, a structural copy of `redeem_once` (:1031) with three differences — the form has NO `gift` pair, the success parse reads `RevealResponse`, and the log line says `reveal`:

```rust
    async fn reveal_once(
        &self,
        gamekey: &str,
        machine_name: &str,
        keyindex: u32,
    ) -> Result<RevealStep, HumbleError> {
        let (csrf, csrf_minted) = match self.csrf_token().await {
            Some(t) => (t, false),
            None => {
                tracing::warn!(
                    "csrf capture failed — minting a double-submit fallback (a server-validated token check will reject this)"
                );
                (uuid::Uuid::new_v4().simple().to_string(), true)
            }
        };
        let resp = self
            .csrf_write(
                format!("{}/humbler/redeemkey", self.base),
                &csrf,
                "/home/library",
            )
            .form(&[
                ("keytype", machine_name),
                ("key", gamekey),
                ("keyindex", &keyindex.to_string()),
                // NO ("gift", "true") — omitting it IS the self-claim (HAR-proven).
            ])
            .send()
            .await?;
        let status = resp.status().as_u16();
        // Names the outcome class only — never the key value.
        tracing::info!(status, machine_name, keyindex, csrf_minted, "humble reveal POST response");
        match status {
            200 => {
                let bytes = resp.bytes().await?;
                if is_login_required(&bytes) {
                    tracing::info!(status, "reveal gated: humble returned login_required — secure-area step-up needed");
                    return Ok(RevealStep::StepUpNeeded { status });
                }
                let body: RevealResponse = decode_body(&bytes)?;
                match (body.success, body.key) {
                    (true, Some(serde_json::Value::String(k))) => Ok(RevealStep::Done(RevealedKey(k))),
                    // success without a string key value: the key MAY be burned server-side —
                    // ambiguous, park-and-reconcile, never assume clean.
                    (true, _) => Err(HumbleError::AmbiguousRedeem),
                    (false, _) => {
                        let msg = body.errormsg.unwrap_or_else(|| "no error message".to_string());
                        tracing::warn!(errormsg = %msg, "humble reveal refused (success=false)");
                        let lower = msg.to_lowercase();
                        if lower.contains("already been redeemed") || lower.contains("already redeemed") {
                            Err(HumbleError::AlreadyRedeemed)
                        } else {
                            Err(HumbleError::RedeemRefused(msg))
                        }
                    }
                }
            }
            // Mirror redeem_once's non-200 arms EXACTLY (401|403|302 → the same typed rejection,
            // etc.). Copy them verbatim from redeem_once rather than re-deriving — the two
            // methods must classify identically. (Read redeem_once :1128-end first.)
            other => redeem_status_error(other, resp).await, // ← if redeem_once's tail is inline,
                                                             //   copy it inline here identically.
        }
    }
```

IMPLEMENTER NOTE: `redeem_once`'s non-200 tail (:1128 onward) may not be factored into a helper. If it's inline, copy the arms verbatim; if extracting a shared helper is trivial (same return type), prefer that — but do NOT change `redeem_once`'s behavior.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p humble-client reveal_key 2>&1 | tail -10`
Expected: 4 tests PASS (plus the whole crate's suite still green: `cargo test -p humble-client 2>&1 | tail -3`).

- [ ] **Step 5: Commit**

```bash
git add crates/humble-client && git commit -S -m "feat(humble-client): reveal_key — the no-gift redeemkey sibling (self-claim, HAR-proven)"
```

---

### Task 2: domain — `revealed_key` field + reserved SELF token

**Files:**
- Modify: `crates/domain/src/lib.rs` (Claim struct :80-101; constants near the top)
- Test: unit tests in the same file's `#[cfg(test)] mod` (or `crates/domain/src/lib.rs` tests section — follow where `merge_sync` tests live)

**Interfaces:**
- Produces: `pub const SELF_LINK_TOKEN: &str = "SELF";` and `Claim.revealed_key: Option<String>`. Tasks 3–9 depend on both names.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn claim_without_revealed_key_field_still_deserializes() {
    // Every pre-existing CLAIM item in dynamo lacks the field — this pins backcompat.
    let old = r#"{"id":"c1","link_token":"t","game_id":"g","state":"pending","gift_url":null,"created_at":"2026-07-01T00:00:00Z"}"#;
    let c: Claim = serde_json::from_str(old).expect("old claim must deserialize");
    assert_eq!(c.revealed_key, None);
}

#[test]
fn self_link_token_is_self() {
    assert_eq!(SELF_LINK_TOKEN, "SELF");
}
```

NOTE: if the existing Claim serde requires `choice_pre_tpks` in that JSON (no default), copy an existing old-claim fixture string from the current tests instead of the literal above — the point is "field absent ⇒ None".

- [ ] **Step 2: Run to verify failure** — `cargo test -p domain revealed_key self_link 2>&1 | tail -5` → compile FAIL.

- [ ] **Step 3: Implement.** In `crates/domain/src/lib.rs`:

Near the top-level items:

```rust
/// Reserved link_token partition for admin self-claims (`pk=LINK#SELF`). No Link META item ever
/// exists for it: intake/fulfill/compensate use the SELF-specific store writes, and the public
/// link fetch 404s it like any unknown token.
pub const SELF_LINK_TOKEN: &str = "SELF";
```

In `Claim` (after `gift_url`):

```rust
    /// Self-claim only: the revealed key VALUE, written durable-first exactly like `gift_url`.
    /// `default` keeps every pre-existing CLAIM item wire-valid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revealed_key: Option<String>,
```

Then fix every `Claim { … }` literal in the workspace (compiler will list them — `claim_game` at dynamo :461, test fixtures, etc.) by adding `revealed_key: None`.

- [ ] **Step 4: Verify** — `cargo test -p domain 2>&1 | tail -3` PASS, then `cargo build -p dynamo -p fulfillment -p public-api -p admin-api 2>&1 | tail -3` (all Claim literals fixed).

- [ ] **Step 5: Commit**

```bash
git add crates/ && git commit -S -m "feat(domain): Claim.revealed_key + reserved SELF link token"
```

---

### Task 3: dynamo — `claim_game_self` (two-item intake, no listable gate)

**Files:**
- Modify: `crates/dynamo/src/lib.rs` (new method after `claim_game`, which ends ~:610)
- Test: `crates/dynamo/tests/store_test.rs`

**Interfaces:**
- Consumes: `domain::SELF_LINK_TOKEN`, existing `ClaimTxError`, `game_pk`/`link_pk`/`claim_item`/`game_item` schema helpers, `av_s`-style AttributeValue building (match `claim_game`'s local style).
- Produces: `pub async fn claim_game_self(&self, game_id: &str, claim_id: &str, now: OffsetDateTime) -> Result<(), ClaimTxError>`. Task 9's handler calls this.

- [ ] **Step 1: Write the failing tests** (in `store_test.rs`, using the file's existing dynamodb-local harness helpers — grep how `claim_game` tests construct the store + seed games):

```rust
#[tokio::test]
async fn self_claim_intake_accepts_non_giftable_and_hidden() {
    let store = test_store().await;
    // A game that is available but NOT listable: giftable=false AND hidden=true — no gsi1pk.
    let mut g = sample_game("gk1:mn1");
    g.giftable = false;
    g.hidden = true;
    store.put_game(&g).await.unwrap();

    store
        .claim_game_self("gk1:mn1", "claim-1", time::OffsetDateTime::now_utc())
        .await
        .expect("non-giftable+hidden must be self-claimable");

    let after = store.get_game("gk1:mn1").await.unwrap().unwrap();
    assert_eq!(after.status, domain::GameStatus::Pending);
    assert_eq!(after.claim_id.as_deref(), Some("claim-1"));
    let claim = store
        .get_claim(domain::SELF_LINK_TOKEN, "claim-1")
        .await
        .unwrap()
        .expect("claim recorded under LINK#SELF");
    assert_eq!(claim.state, domain::ClaimState::Pending);
    assert_eq!(claim.link_token, domain::SELF_LINK_TOKEN);
}

#[tokio::test]
async fn self_claim_intake_single_winner_on_race() {
    let store = test_store().await;
    store.put_game(&sample_game("gk1:mn2")).await.unwrap();
    let a = store.claim_game_self("gk1:mn2", "claim-a", time::OffsetDateTime::now_utc()).await;
    let b = store.claim_game_self("gk1:mn2", "claim-b", time::OffsetDateTime::now_utc()).await;
    assert!(a.is_ok() ^ b.is_ok() == false || (a.is_ok() && b.is_err()));
    // Sequential calls: first wins, second refuses on the status condition.
    assert!(a.is_ok());
    assert!(matches!(b, Err(dynamo::ClaimTxError::GameUnavailable)));
}

#[tokio::test]
async fn gift_vs_self_claim_race_single_winner() {
    let store = test_store().await;
    store.put_game(&sample_game("gk1:mn3")).await.unwrap();
    let link = sample_link("tok-race");
    store.create_link(&link).await.unwrap();
    // Gift claim wins first…
    store
        .claim_game("tok-race", "gk1:mn3", "claim-g", time::OffsetDateTime::now_utc())
        .await
        .unwrap();
    // …self-claim then refuses on the same status condition.
    let s = store.claim_game_self("gk1:mn3", "claim-s", time::OffsetDateTime::now_utc()).await;
    assert!(matches!(s, Err(dynamo::ClaimTxError::GameUnavailable)));
}
```

NOTE: `test_store` / `sample_game` / `sample_link` — use the file's real helper names (grep the existing claim_game tests; adjust ids/fields to the real `Game` constructor used there).

- [ ] **Step 2: Verify failure** — `cargo test -p dynamo self_claim 2>&1 | tail -5` → compile FAIL (`claim_game_self` missing).

- [ ] **Step 3: Implement** after `claim_game`:

```rust
    /// Admin self-claim intake — the two-item sibling of [`claim_game`]. Differences, both
    /// deliberate (spec §3.1): NO LINK item (LINK#SELF has no META; there is no budget to
    /// enforce), and the GAME condition is `#st = :available` ALONE — not the gift path's
    /// `attribute_exists(gsi1pk)`. The sparse listable marker (available ∧ giftable ∧ ¬hidden)
    /// guards FRIEND claims against the hide-race; self-claim must accept exactly the
    /// non-giftable and hidden games that marker excludes. The status condition alone still
    /// makes gift-vs-self and self-vs-self races single-winner.
    pub async fn claim_game_self(
        &self,
        game_id: &str,
        claim_id: &str,
        now: OffsetDateTime,
    ) -> Result<(), ClaimTxError> {
        let game = self
            .get_game(game_id)
            .await?
            .ok_or(ClaimTxError::GameUnavailable)?;

        let mut pending = game.clone();
        pending.status = GameStatus::Pending;
        pending.claim_id = Some(claim_id.to_string());
        let claim = domain::Claim {
            id: claim_id.to_string(),
            link_token: domain::SELF_LINK_TOKEN.to_string(),
            game_id: game_id.to_string(),
            state: ClaimState::Pending,
            gift_url: None,
            revealed_key: None,
            created_at: now,
            choice_pre_tpks: None,
        };

        let av_s = |v: &str| aws_sdk_dynamodb::types::AttributeValue::S(v.to_string());
        let game_update = aws_sdk_dynamodb::types::Update::builder()
            .table_name(&self.table)
            .key("pk", av_s(&game_pk(game_id)))
            .key("sk", av_s("META"))
            .update_expression(
                "SET body = :b, #st = :pending, claim_id = :cid REMOVE gsi1pk, gsi1sk",
            )
            .condition_expression("#st = :available")
            .expression_attribute_names("#st", "status")
            .expression_attribute_values(":b", av_s(&serde_json::to_string(&pending).expect("game")))
            .expression_attribute_values(":pending", av_s("pending"))
            .expression_attribute_values(":available", av_s("available"))
            .expression_attribute_values(":cid", av_s(claim_id))
            .build()
            .expect("game_update");
        let claim_put = aws_sdk_dynamodb::types::Put::builder()
            .table_name(&self.table)
            .set_item(Some(claim_item(&claim)))
            .condition_expression("attribute_not_exists(pk)")
            .build()
            .expect("claim_put");

        let res = self
            .client
            .transact_write_items()
            .transact_items(
                aws_sdk_dynamodb::types::TransactWriteItem::builder()
                    .update(game_update)
                    .build(),
            )
            .transact_items(
                aws_sdk_dynamodb::types::TransactWriteItem::builder()
                    .put(claim_put)
                    .build(),
            )
            .send()
            .await;
        map_claim_tx_result(res) // ← reuse claim_game's cancellation-reason mapping. If that
                                 //   mapping is inline in claim_game, extract it to a shared
                                 //   helper (same error classification: first-item CCF ⇒
                                 //   GameUnavailable, claim CCF ⇒ DuplicateClaim, conflict ⇒
                                 //   TxConflict) — do NOT change claim_game's behavior.
    }
```

IMPLEMENTER NOTE: read how `claim_game` maps its `TransactWriteItems` error (cancellation reasons → `ClaimTxError` variants; it has THREE items where this has TWO — index positions matter in the mapping). Mirror the two-item positions correctly.

- [ ] **Step 4: Verify** — `cargo test -p dynamo self_claim 2>&1 | tail -5` PASS; `cargo test -p dynamo 2>&1 | tail -3` all green.

- [ ] **Step 5: Commit**

```bash
git add crates/dynamo && git commit -S -m "feat(dynamo): claim_game_self — two-item intake, status-only condition (hidden+non-giftable eligible)"
```

---

### Task 4: dynamo — `fulfill_self_claim` (durable-first key, flip BenRedeemed)

**Files:**
- Modify: `crates/dynamo/src/lib.rs` (after `fulfill_claim` :612-661)
- Test: `crates/dynamo/tests/store_test.rs`

**Interfaces:**
- Consumes: `flip_game_from_pending` (private helper :663), `claim_item`, `get_claim`.
- Produces: `pub async fn fulfill_self_claim(&self, claim_id: &str, game_id: &str, revealed_key: &str) -> Result<(), StoreError>`. Tasks 6–8 call this.

- [ ] **Step 1: Failing tests**

```rust
#[tokio::test]
async fn fulfill_self_claim_writes_key_then_flips_ben_redeemed() {
    let store = test_store().await;
    store.put_game(&sample_game("gk2:mn1")).await.unwrap();
    store.claim_game_self("gk2:mn1", "c-f1", time::OffsetDateTime::now_utc()).await.unwrap();

    store.fulfill_self_claim("c-f1", "gk2:mn1", "AAAA-BBBB-CCCC").await.unwrap();

    let claim = store.get_claim(domain::SELF_LINK_TOKEN, "c-f1").await.unwrap().unwrap();
    assert_eq!(claim.state, domain::ClaimState::Fulfilled);
    assert_eq!(claim.revealed_key.as_deref(), Some("AAAA-BBBB-CCCC"));
    let game = store.get_game("gk2:mn1").await.unwrap().unwrap();
    assert_eq!(game.status, domain::GameStatus::BenRedeemed);
}

#[tokio::test]
async fn fulfill_self_claim_is_idempotent_on_retry() {
    // Crash-after-write-1 shape: key durable, game still pending → a retry completes the flip.
    let store = test_store().await;
    store.put_game(&sample_game("gk2:mn2")).await.unwrap();
    store.claim_game_self("gk2:mn2", "c-f2", time::OffsetDateTime::now_utc()).await.unwrap();
    store.fulfill_self_claim("c-f2", "gk2:mn2", "K1").await.unwrap();
    // Second call: no error, state unchanged.
    store.fulfill_self_claim("c-f2", "gk2:mn2", "K1").await.unwrap();
    let claim = store.get_claim(domain::SELF_LINK_TOKEN, "c-f2").await.unwrap().unwrap();
    assert_eq!(claim.revealed_key.as_deref(), Some("K1"));
}
```

- [ ] **Step 2: Verify failure** — `cargo test -p dynamo fulfill_self 2>&1 | tail -5` → compile FAIL.

- [ ] **Step 3: Implement** — structural copy of `fulfill_claim` with `revealed_key` in place of `gift_url` and a `BenRedeemed` flip:

```rust
    /// Self-claim fulfillment — [`fulfill_claim`]'s sibling (spec §3.2): write the revealed key
    /// to the CLAIM durable-FIRST (conditioned on the pending marker, same fulfill-vs-compensate
    /// mutual exclusion), then flip the GAME pending → ben_redeemed gated on claim ownership.
    pub async fn fulfill_self_claim(
        &self,
        claim_id: &str,
        game_id: &str,
        revealed_key: &str,
    ) -> Result<(), StoreError> {
        let mut claim = self
            .get_claim(domain::SELF_LINK_TOKEN, claim_id)
            .await?
            .ok_or(StoreError::Corrupt("fulfill_self: claim missing"))?;
        claim.state = ClaimState::Fulfilled;
        claim.revealed_key = Some(revealed_key.to_string());

        let put_res = self
            .client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(claim_item(&claim)))
            .condition_expression("attribute_exists(gsi2pk)")
            .send()
            .await;
        match put_res {
            Ok(_) => {}
            Err(sdk_err) => {
                if !is_ccf_put(&sdk_err) {
                    return Err(StoreError::Aws(format!("{sdk_err:?}")));
                }
                let current = self
                    .get_claim(domain::SELF_LINK_TOKEN, claim_id)
                    .await?
                    .ok_or(StoreError::Corrupt("fulfill_self: claim missing on recheck"))?;
                if current.state != ClaimState::Fulfilled {
                    return Err(StoreError::Corrupt(
                        "fulfill_self lost to compensate — revealed key needs manual/reconcile recovery",
                    ));
                }
                // idempotent retry: key already durable; fall through to re-attempt the flip.
            }
        }
        self.flip_game_from_pending(game_id, Some(claim_id), GameStatus::BenRedeemed)
            .await
    }
```

- [ ] **Step 4: Verify** — `cargo test -p dynamo fulfill_self 2>&1 | tail -5` PASS; full crate green.

- [ ] **Step 5: Commit**

```bash
git add crates/dynamo && git commit -S -m "feat(dynamo): fulfill_self_claim — durable-first revealed_key, flip ben_redeemed"
```

---

### Task 5: dynamo — `compensate_self_claim` (two items, no LINK decrement)

**Files:**
- Modify: `crates/dynamo/src/lib.rs` (after `compensate_claim` :770-892)
- Test: `crates/dynamo/tests/store_test.rs`

**Interfaces:**
- Produces: `pub async fn compensate_self_claim(&self, claim_id: &str, game_id: &str) -> Result<(), StoreError>`. Tasks 6–8 call this.

- [ ] **Step 1: Failing test — THE review-blocker pin.** This is the test that would have caught B1: gift-path `compensate_claim` decrements a LINK META that `LINK#SELF` doesn't have, cancelling the whole transaction.

```rust
#[tokio::test]
async fn compensate_self_claim_succeeds_with_no_link_meta_item() {
    let store = test_store().await;
    store.put_game(&sample_game("gk3:mn1")).await.unwrap();
    store.claim_game_self("gk3:mn1", "c-c1", time::OffsetDateTime::now_utc()).await.unwrap();

    // The gift-path compensate MUST fail here (pins WHY the variant exists)…
    let wrong = store.compensate_claim(domain::SELF_LINK_TOKEN, "c-c1", "gk3:mn1").await;
    assert!(wrong.is_err(), "gift compensate must cancel on the absent LINK META");

    // …and the SELF variant must succeed: claim compensated, game re-listed.
    store.compensate_self_claim("c-c1", "gk3:mn1").await.unwrap();
    let claim = store.get_claim(domain::SELF_LINK_TOKEN, "c-c1").await.unwrap().unwrap();
    assert_eq!(claim.state, domain::ClaimState::Compensated);
    let game = store.get_game("gk3:mn1").await.unwrap().unwrap();
    assert_eq!(game.status, domain::GameStatus::Available);
    assert_eq!(game.claim_id, None);
}
```

- [ ] **Step 2: Verify failure** — compile FAIL.

- [ ] **Step 3: Implement** — copy `compensate_claim`'s body (read :770-892 first) with `link_token = domain::SELF_LINK_TOKEN` and the **third transact item (LINK decrement) deleted**:

```rust
    /// Self-claim compensation — [`compensate_claim`]'s two-item sibling (spec §3.3): CLAIM →
    /// compensated (conditioned on the pending marker), GAME re-listed (conditioned
    /// `#st = :pending`), and NO link decrement — LINK#SELF has no META item; the gift variant's
    /// `claims_used >= 1` guard against it would cancel the whole transaction, wedging every
    /// self-claim compensation permanently (the review-B1 finding this method exists to fix).
    pub async fn compensate_self_claim(
        &self,
        claim_id: &str,
        game_id: &str,
    ) -> Result<(), StoreError> {
        let mut claim = self
            .get_claim(domain::SELF_LINK_TOKEN, claim_id)
            .await?
            .ok_or(StoreError::Corrupt("compensate_self: claim missing"))?;
        claim.state = ClaimState::Compensated;

        let mut game = self
            .get_game(game_id)
            .await?
            .ok_or(StoreError::Corrupt("compensate_self: game missing"))?;
        game.status = GameStatus::Available;
        game.claim_id = None;

        let claim_put = aws_sdk_dynamodb::types::Put::builder()
            .table_name(&self.table)
            .set_item(Some(claim_item(&claim)))
            .condition_expression("attribute_exists(gsi2pk)")
            .build()
            .expect("claim_put");
        let game_put = aws_sdk_dynamodb::types::Put::builder()
            .table_name(&self.table)
            .set_item(Some(game_item(&game)))
            .condition_expression("#st = :pending")
            .expression_attribute_names("#st", "status")
            .expression_attribute_values(":pending", schema::s("pending"))
            .build()
            .expect("game_put");

        let result = self
            .client
            .transact_write_items()
            .transact_items(aws_sdk_dynamodb::types::TransactWriteItem::builder().put(claim_put).build())
            .transact_items(aws_sdk_dynamodb::types::TransactWriteItem::builder().put(game_put).build())
            .send()
            .await;
        // Error mapping: mirror compensate_claim's tail (CCF classification) minus the link arm.
        match result {
            Ok(_) => Ok(()),
            Err(e) => Err(map_compensate_tx_err(e)), // reuse/extract compensate_claim's mapping
        }
    }
```

IMPLEMENTER NOTE: as with Task 3, read `compensate_claim`'s error-mapping tail and mirror it for two items (extract a helper only if it doesn't change gift behavior). NOTE re-listing via `game_item(&game)` re-adds gsi1pk ONLY if the game is listable (`is_listable`) — a hidden/non-giftable self-claimed game re-lists as available-but-unlisted, which is correct.

- [ ] **Step 4: Verify** — `cargo test -p dynamo compensate_self 2>&1 | tail -5` PASS; full crate green.

- [ ] **Step 5: Commit**

```bash
git add crates/dynamo && git commit -S -m "feat(dynamo): compensate_self_claim — two-item variant, no LINK decrement (review B1)"
```

---

### Task 6: fulfillment — `SelfClaim` request, `RevealedKey` response, `reveal_decision`, bundle path

**Files:**
- Modify: `crates/fulfillment/src/lib.rs` (FulfillRequest :55-81, FulfillResponse :83-105, decisions :123-235, dispatch `handle` :384-430, new `handle_self_claim` after `handle_gift`)
- Test: `crates/fulfillment/tests/handler_test.rs`

**Interfaces:**
- Consumes: `humble_client::{RevealedKey, HumbleError}`, `store.fulfill_self_claim` / `compensate_self_claim` (Tasks 4-5), `selfheal_once`, `set_cookie_ok`, `ping`.
- Produces:
  - `FulfillRequest::SelfClaim { claim_id: String, game_id: String, gamekey: String, machine_name: String, keyindex: u32, requires_choice: bool }`
  - `FulfillResponse::RevealedKey { key: String }`
  - `pub fn reveal_decision(outcome: &Result<RevealedKey, HumbleError>) -> Decision`
  Task 9's admin handler serializes/deserializes these exact shapes.

- [ ] **Step 1: Failing tests** (in `handler_test.rs`, using the file's existing moto+wiremock harness — grep how the `handle_gift` tests mount `/humbler/redeemkey` and build `Deps`):

```rust
#[tokio::test]
async fn self_claim_bundle_reveals_and_records() {
    let (deps, store, humble) = test_deps().await; // the file's existing harness constructor
    seed_available_game(&store, "gkA:mnA", "Stardew Valley").await;
    store.claim_game_self("gkA:mnA", "sc-1", now()).await.unwrap();
    mount_reveal_success(&humble, "AAAA-BBBB-CCCC").await; // 200 {"key":"…","success":true}, asserts NO gift param

    let resp = fulfillment::handle(&deps, fulfillment::FulfillRequest::SelfClaim {
        claim_id: "sc-1".into(), game_id: "gkA:mnA".into(), gamekey: "gkA".into(),
        machine_name: "mnA".into(), keyindex: 0, requires_choice: false,
    }).await;

    assert_eq!(resp, fulfillment::FulfillResponse::RevealedKey { key: "AAAA-BBBB-CCCC".into() });
    let claim = store.get_claim(domain::SELF_LINK_TOKEN, "sc-1").await.unwrap().unwrap();
    assert_eq!(claim.revealed_key.as_deref(), Some("AAAA-BBBB-CCCC"));
    assert_eq!(store.get_game("gkA:mnA").await.unwrap().unwrap().status, domain::GameStatus::BenRedeemed);
}

#[tokio::test]
async fn self_claim_already_redeemed_recovers_key_from_order() {
    // M2 policy: AlreadyRedeemed ⇒ re-read the order, record redeemed_key_val — NOT compensate.
    let (deps, store, humble) = test_deps().await;
    seed_available_game(&store, "gkB:mnB", "Two Point Campus").await;
    store.claim_game_self("gkB:mnB", "sc-2", now()).await.unwrap();
    mount_reveal_already_redeemed(&humble).await; // success=false "already been redeemed"
    mount_order_with_redeemed_tpk(&humble, "gkB", "mnB", "RECOVERED-KEY").await; // order: tpk mnB redeemed_key_val="RECOVERED-KEY"

    let resp = fulfillment::handle(&deps, self_claim_req("sc-2", "gkB:mnB", "gkB", "mnB")).await;

    assert_eq!(resp, fulfillment::FulfillResponse::RevealedKey { key: "RECOVERED-KEY".into() });
    let claim = store.get_claim(domain::SELF_LINK_TOKEN, "sc-2").await.unwrap().unwrap();
    assert_eq!(claim.revealed_key.as_deref(), Some("RECOVERED-KEY"));
}

#[tokio::test]
async fn self_claim_already_redeemed_with_no_key_val_parks() {
    // The recover fallback: order shows redeemed but carries no redeemed_key_val ⇒ park + ping.
    let (deps, store, humble) = test_deps().await;
    seed_available_game(&store, "gkC:mnC", "Mystery Game").await;
    store.claim_game_self("gkC:mnC", "sc-3", now()).await.unwrap();
    mount_reveal_already_redeemed(&humble).await;
    mount_order_with_redeemed_tpk_no_val(&humble, "gkC", "mnC").await;

    let resp = fulfillment::handle(&deps, self_claim_req("sc-3", "gkC:mnC", "gkC", "mnC")).await;

    assert!(matches!(resp, fulfillment::FulfillResponse::Parked { .. }));
    let claim = store.get_claim(domain::SELF_LINK_TOKEN, "sc-3").await.unwrap().unwrap();
    assert_eq!(claim.state, domain::ClaimState::Pending); // reconcile owns it
}

#[tokio::test]
async fn self_claim_ambiguous_failure_parks_never_compensates() {
    let (deps, store, humble) = test_deps().await;
    seed_available_game(&store, "gkD:mnD", "Park Me").await;
    store.claim_game_self("gkD:mnD", "sc-4", now()).await.unwrap();
    mount_reveal_500(&humble).await;

    let resp = fulfillment::handle(&deps, self_claim_req("sc-4", "gkD:mnD", "gkD", "mnD")).await;
    assert!(matches!(resp, fulfillment::FulfillResponse::Parked { .. }));
    assert_eq!(store.get_claim(domain::SELF_LINK_TOKEN, "sc-4").await.unwrap().unwrap().state,
               domain::ClaimState::Pending);
}
```

Write the small `mount_*` / `self_claim_req` helpers in the test file following its existing mock-mount helpers. **Every helper that asserts on the reveal POST also asserts the body contains no `gift=` pair.**

- [ ] **Step 2: Verify failure** — `cargo test -p fulfillment self_claim 2>&1 | tail -5` → compile FAIL.

- [ ] **Step 3: Implement.**

(a) `FulfillRequest` gains (after `Gift`):

```rust
    /// Admin self-claim: reveal the key VALUE to Ben (no gift URL). Mirrors `Gift`'s field
    /// semantics: on `requires_choice=true`, `machine_name` is the OFFERED id and `keyindex` is
    /// ignored (read off the post-choose order).
    SelfClaim {
        claim_id: String,
        game_id: String,
        gamekey: String,
        machine_name: String,
        keyindex: u32,
        #[serde(default)]
        requires_choice: bool,
    },
```

(b) `FulfillResponse` gains (after `GiftUrl`):

```rust
    /// Self-claim success: the revealed key VALUE. Serialized only on the admin-api wire —
    /// never logged, never in a friend response.
    RevealedKey {
        key: String,
    },
```

(c) after `gift_decision` (:123-176) — the sibling, same arms, typed over `RevealedKey` (copy `gift_decision`'s match arms verbatim, only the success type changes):

```rust
/// Reveal ladder decision — [`gift_decision`] typed over the reveal outcome. IDENTICAL
/// classification (the two must never drift); only the Compensate arm's EXECUTION differs at the
/// call site (self-claim recovers the key instead of compensating — spec §4).
pub fn reveal_decision(outcome: &Result<RevealedKey, HumbleError>) -> Decision {
    match outcome {
        Ok(_) => Decision::Record,
        Err(err) => gift_error_decision(err), // ← extract gift_decision's Err-arm match into
                                              //   `fn gift_error_decision(&HumbleError) -> Decision`
                                              //   and have BOTH decision fns call it. This keeps
                                              //   the no-`_`-arm exhaustiveness in ONE place.
    }
}
```

(d) dispatch in `handle` (:427, before `Sync`):

```rust
        FulfillRequest::SelfClaim {
            claim_id,
            game_id,
            gamekey,
            machine_name,
            keyindex,
            requires_choice,
        } => {
            tracing::info!(claim_id, game_id, machine_name, keyindex, requires_choice,
                "fulfillment: self-claim request");
            if requires_choice {
                handle_self_claim_choice(deps, &claim_id, &game_id, &gamekey, &machine_name).await
            } else {
                handle_self_claim(deps, &claim_id, &game_id, &gamekey, &machine_name, keyindex).await
            }
        }
```

(Task 7 provides `handle_self_claim_choice`; for THIS task stub it as `parked_choice("choice-self-claim-not-built")` with a `// Task 7` note so the crate compiles, and don't test it yet.)

(e) `handle_self_claim` — mirrors `handle_gift` (:433-570) with reveal terminals:

```rust
/// The self-claim ladder's side-effecting half — [`handle_gift`]'s reveal sibling. Same heal
/// composition; two policy differences (spec §4): Record writes `revealed_key` via
/// `fulfill_self_claim`, and AlreadyRedeemed RECOVERS (re-read order → record `redeemed_key_val`)
/// instead of compensating — for a self-claim, "already redeemed" means the key already belongs
/// to Ben and its value is recoverable; compensating would re-list a burned game and lose the key.
async fn handle_self_claim(
    deps: &Deps,
    claim_id: &str,
    game_id: &str,
    gamekey: &str,
    machine_name: &str,
    keyindex: u32,
) -> FulfillResponse {
    let (heal, outcome) = selfheal_once(deps, deps.session_store.is_some(), || {
        deps.humble.reveal_key(gamekey, machine_name, keyindex)
    })
    .await;
    if let Err(e) = &outcome {
        tracing::warn!(claim_id, game_id, error = ?e, "self-claim reveal did not return a key");
    } else {
        tracing::info!(claim_id, game_id, "self-claim reveal returned a key");
    }
    let decision = reveal_decision(&outcome);
    if let Some(h) = heal
        && decision != Decision::ParkCookieDead
    {
        set_cookie_ok(deps, h.durable()).await;
    }
    match decision {
        Decision::Record => match outcome {
            Ok(RevealedKey(key)) => record_revealed_key(deps, claim_id, game_id, key).await,
            Err(_) => FulfillResponse::Error {
                message: "internal: record decision without a revealed key".into(),
            },
        },
        // Spec §4 recover-then-record: NOT compensate.
        Decision::Compensate => {
            recover_already_redeemed_key(deps, claim_id, game_id, gamekey, machine_name).await
        }
        Decision::ParkCookieDead => {
            set_cookie_ok(deps, false).await;
            let msg = if deps.session_store.is_some() { COOKIE_DEAD_SELFHEAL_MSG } else { COOKIE_DEAD_MSG };
            ping(deps, msg).await;
            FulfillResponse::Parked { reason: "humble session needs attention".into() }
        }
        Decision::Park => {
            // Mirror handle_gift's Park arm: detail classification + the step-up ping, with
            // self-claim wording. Copy :547-570 and adjust the ping text (same failure classes).
            let detail = match &outcome {
                Err(HumbleError::RedeemRefused(_)) => "refused",
                Err(HumbleError::AmbiguousRedeem) => "ambiguous",
                Err(HumbleError::RateLimited) => "rate-limited",
                Err(HumbleError::RedeemAuthRejected { .. }) => "redeem-auth-rejected",
                Err(HumbleError::SecureAreaStepUpFailed { .. }) => "secure-area-step-up-failed",
                _ => "transient",
            };
            if let Err(HumbleError::SecureAreaStepUpFailed { reason }) = &outcome {
                ping(deps, &format!(
                    "self-claim reveal for claim {claim_id} ({machine_name}) needed humble's \
                     secure-area step-up and it did not complete: {reason}. The key was NOT \
                     revealed — the claim is parked and reconcile will finish it."
                )).await;
            }
            FulfillResponse::Parked { reason: format!("self-claim reveal inconclusive: park for reconcile ({detail})") }
        }
    }
}

/// Durable-first record of a revealed key + the RevealedKey response. Shared by the happy path,
/// the recover path, and (Task 8) reconcile.
async fn record_revealed_key(
    deps: &Deps,
    claim_id: &str,
    game_id: &str,
    key: String,
) -> FulfillResponse {
    match deps.store.fulfill_self_claim(claim_id, game_id, &key).await {
        Ok(()) => FulfillResponse::RevealedKey { key },
        Err(e) => {
            // Key exists but recording failed — loud, human decides. NEVER retry the reveal.
            // The ping names the claim, NEVER the key value.
            ping(deps, &format!(
                "self-claim fulfill failed for claim {claim_id}: {e} — the key was revealed but \
                 not recorded; it is still readable in humble's library keys page."
            )).await;
            FulfillResponse::Error { message: "key revealed but recording failed — flagged for ben".into() }
        }
    }
}

/// AlreadyRedeemed recovery (spec §4): the key's value sits in the order's
/// `all_tpks[].redeemed_key_val`. Re-read, match the tpk by machine_name, record. Fallback when
/// no value is present (e.g. the key was actually gifted away — gift-redeems may not set
/// redeemed_key_val): PARK + ping, never guess, never compensate blind.
async fn recover_already_redeemed_key(
    deps: &Deps,
    claim_id: &str,
    game_id: &str,
    gamekey: &str,
    machine_name: &str,
) -> FulfillResponse {
    let order = match deps.humble.order(gamekey).await {
        Ok(o) => o,
        Err(e) => {
            tracing::warn!(claim_id, error = ?e, "self-claim recover: order re-read failed — parking");
            return FulfillResponse::Parked { reason: "recover re-read failed: park for reconcile".into() };
        }
    };
    let tpk = order.keys.iter().find(|k| k.machine_name == machine_name);
    match tpk.and_then(|k| k.redeemed_key_val.clone()) {
        Some(val) => {
            tracing::info!(claim_id, "self-claim recover: redeemed_key_val present — recording");
            record_revealed_key(deps, claim_id, game_id, val).await
        }
        None => {
            ping(deps, &format!(
                "self-claim {claim_id} ({machine_name}): humble says already-redeemed but the \
                 order carries no key value — it may have been gifted out-of-band. Parked for \
                 review; nothing was compensated."
            )).await;
            FulfillResponse::Parked { reason: "already-redeemed with no recoverable key value".into() }
        }
    }
}
```

IMPLEMENTER NOTE: check `Order`'s field name for tpks (`order.keys` vs `order.all_tpks`) — use whatever `handle_gift_choice`'s pre-check (:666-703) uses (`pre_order.keys` there) and the same `redeemed` / `redeemed_key_val` accessors (the wire model may expose `redeemed: bool` + `redeemed_key_val: Option<String>` — mirror the pre-check's usage exactly).

- [ ] **Step 4: Verify** — `cargo test -p fulfillment self_claim 2>&1 | tail -8` → 4 PASS; whole crate still green.

- [ ] **Step 5: Commit**

```bash
git add crates/fulfillment && git commit -S -m "feat(fulfillment): SelfClaim bundle path — reveal_decision + recover-then-record on AlreadyRedeemed"
```

---

### Task 7: fulfillment — choice self-claim path (flavor-parameterized orchestration)

**Files:**
- Modify: `crates/fulfillment/src/lib.rs` (`handle_gift_choice` :616-816, `redeem_claimed_tpk` :832+)
- Test: `crates/fulfillment/tests/handler_test.rs`

**Interfaces:**
- Consumes: everything from Task 6; `choose_content(gamekey, chosen, is_gift)`; `record_choice_intent`; `find_new_tpk`.
- Produces: `handle_self_claim_choice(deps, claim_id, game_id, gamekey, offered_id) -> FulfillResponse` (replacing Task 6's stub) and `reveal_claimed_tpk(...)` — the reveal sibling of `redeem_claimed_tpk`, which Task 8's reconcile also calls.

**Approach:** parameterize, don't duplicate. `handle_gift_choice`'s skeleton (pre-read → pre-check → intent snapshot → choose → re-read → find tpk → terminal) is flavor-independent; only THREE points differ: `link_token` (real vs SELF), `is_gift` on the choose, and the terminal (redeem-to-gift vs reveal). Refactor `handle_gift_choice` into a private `handle_choice_claim(deps, claim_id, link_token, game_id, gamekey, offered_id, flavor)` with:

```rust
#[derive(Clone, Copy, PartialEq, Eq)]
enum ClaimFlavor {
    Gift,
    SelfClaim,
}
```

- `is_gift = matches!(flavor, ClaimFlavor::Gift)` on the `choose_content` call (:727).
- The two terminal call sites (:693 pre-check resume, :806 happy tail) call `claimed_tpk_terminal(deps, flavor, …)` which dispatches to `redeem_claimed_tpk` or `reveal_claimed_tpk`.
- The pre-check's already-claimed-AND-redeemed arm (:671-686): for `SelfClaim`, instead of the human-recovery ping, call Task 6's `recover_already_redeemed_key` (the value is recoverable for self-claims).
- Public wrappers keep the old entry points: `handle_gift_choice(…)` calls `handle_choice_claim(…, ClaimFlavor::Gift)`; new `handle_self_claim_choice(…)` passes `SELF_LINK_TOKEN` and `ClaimFlavor::SelfClaim` (and replaces Task 6's stub).

`reveal_claimed_tpk` is `redeem_claimed_tpk`'s (:832) structural copy: `reveal_key(gamekey, tpk.machine_name, tpk.keyindex)` via the same `selfheal_once(allow_heal)` composition, classified by `reveal_decision`, Record → `record_revealed_key`, Compensate-class → `recover_already_redeemed_key` (NOT park-for-human — the self-claim difference), everything else mirroring the redeem sibling's arms. Read `redeem_claimed_tpk`'s full body first and mirror arm-for-arm.

- [ ] **Step 1: Failing tests**

```rust
#[tokio::test]
async fn choice_self_claim_chooses_without_gift_then_reveals() {
    let (deps, store, humble) = test_deps().await;
    seed_choice_game(&store, "gkE:offered_sim", "Construction Simulator").await; // requires_choice=true
    store.claim_game_self("gkE:offered_sim", "sc-c1", now()).await.unwrap();
    mount_order_pre_choose(&humble, "gkE").await;                    // no matching tpk yet
    mount_choose_success_asserting_no_is_gift(&humble, "gkE").await; // asserts is_gift ABSENT from form
    mount_order_post_choose(&humble, "gkE", "constructionsim_choice_steam", 0).await; // new unredeemed tpk
    mount_reveal_success(&humble, "SIM-KEY-123").await;

    let resp = fulfillment::handle(&deps, fulfillment::FulfillRequest::SelfClaim {
        claim_id: "sc-c1".into(), game_id: "gkE:offered_sim".into(), gamekey: "gkE".into(),
        machine_name: "offered_sim".into(), keyindex: 0, requires_choice: true,
    }).await;

    assert_eq!(resp, fulfillment::FulfillResponse::RevealedKey { key: "SIM-KEY-123".into() });
    // Intent snapshot landed before the choose:
    let claim = store.get_claim(domain::SELF_LINK_TOKEN, "sc-c1").await.unwrap().unwrap();
    assert!(claim.choice_pre_tpks.is_some());
    assert_eq!(claim.revealed_key.as_deref(), Some("SIM-KEY-123"));
}

#[tokio::test]
async fn choice_self_claim_ambiguous_choose_parks_no_reveal_attempted() {
    let (deps, store, humble) = test_deps().await;
    seed_choice_game(&store, "gkF:offered_x", "Parked Sim").await;
    store.claim_game_self("gkF:offered_x", "sc-c2", now()).await.unwrap();
    mount_order_pre_choose(&humble, "gkF").await;
    mount_choose_500(&humble, "gkF").await; // ambiguous: pick MAY be spent

    let resp = fulfillment::handle(&deps, self_claim_choice_req("sc-c2", "gkF:offered_x", "gkF", "offered_x")).await;
    assert!(matches!(resp, fulfillment::FulfillResponse::Parked { .. }));
    // No reveal POST happened (mock has zero expectations for redeemkey) and the claim is pending.
}

#[tokio::test]
async fn gift_choice_path_still_sends_is_gift_true() {
    // Regression pin for the refactor: the GIFT flavor still chooses with is_gift=true.
    let (deps, store, humble) = test_deps().await;
    // …reuse an existing handle_gift_choice happy-path test's setup, asserting the choose form
    // CONTAINS is_gift=true (this may already exist — if so, just confirm it still passes).
}
```

- [ ] **Step 2: Verify failure** — `cargo test -p fulfillment choice_self 2>&1 | tail -5` → FAIL (stub parks).

- [ ] **Step 3: Implement the refactor + `reveal_claimed_tpk`** per the Approach block above. The refactor of `handle_gift_choice` must be behavior-preserving for the Gift flavor — the existing choice tests are the net; run them before writing any new code path.

- [ ] **Step 4: Verify** — `cargo test -p fulfillment 2>&1 | tail -5` — the ENTIRE crate (existing gift-choice + merge-gate tests prove the refactor preserved behavior; new tests prove the flavor).

- [ ] **Step 5: Commit**

```bash
git add crates/fulfillment && git commit -S -m "feat(fulfillment): choice self-claim — flavor-parameterized orchestration, reveal terminal, no is_gift on choose"
```

---

### Task 8: fulfillment — reconcile learns SELF

**Files:**
- Modify: `crates/fulfillment/src/lib.rs` (`reconcile` :1554+, `reconcile_choice_claim` :1036-1141)
- Test: `crates/fulfillment/tests/handler_test.rs`

**Interfaces:**
- Consumes: `claim.link_token == domain::SELF_LINK_TOKEN` as the discriminator; `reveal_claimed_tpk` (Task 7), `compensate_self_claim`, `recover_already_redeemed_key`, `record_revealed_key`.
- Produces: reconcile handles parked self-claims per the spec §4 terminal table. No new public API.

**The terminal table to implement (spec §4):** read `reconcile` (:1554+) and `reconcile_choice_claim` (:1036) first; at each terminal call-site branch on the discriminator:

| Branch | Gift terminal (existing) | SELF terminal |
|---|---|---|
| bundle: tpk unredeemed | `redeem_as_gift` → fulfill | `reveal_claimed_tpk` (allow_heal=false) |
| bundle: tpk already redeemed | ping human | `recover_already_redeemed_key` |
| bundle: provably never redeemed | `compensate_claim` | `compensate_self_claim` |
| choice A (no snapshot) | `compensate_claim` | `compensate_self_claim` |
| choice B1 (no new tpk) | `compensate_claim` | `compensate_self_claim` |
| choice B2 (new tpk unredeemed) | `redeem_claimed_tpk(false)` | `reveal_claimed_tpk(false)` |
| choice B3 (new tpk redeemed) | ping human | `recover_already_redeemed_key` |
| choice B4 (ambiguous) | ping human | ping human (unchanged) |

Implementation shape: where the branches call `compensate_claim(&claim.link_token, …)`, insert:

```rust
let compensated = if claim.link_token == domain::SELF_LINK_TOKEN {
    deps.store.compensate_self_claim(&claim.id, &claim.game_id).await
} else {
    deps.store.compensate_claim(&claim.link_token, &claim.id, &claim.game_id).await
};
```

(or extract `async fn compensate_any(deps, claim) -> Result<(), StoreError>` used by every branch — preferred, one discriminator instead of five). Same pattern for the redeem/reveal terminal (`claimed_tpk_terminal` from Task 7 already dispatches on flavor — derive flavor from the token: `if claim.link_token == SELF_LINK_TOKEN { SelfClaim } else { Gift }`).

- [ ] **Step 1: Failing tests**

```rust
#[tokio::test]
async fn reconcile_self_choice_no_snapshot_compensates_via_self_variant() {
    // A parked SELF choice claim with no intent snapshot: choose never ran ⇒ compensate — and it
    // must SUCCEED despite the absent LINK META (the gift compensate would wedge it).
    let (deps, store, humble) = test_deps().await;
    seed_choice_game(&store, "gkG:off_g", "Reconcile Me").await;
    store.claim_game_self("gkG:off_g", "sc-r1", old_enough()).await.unwrap(); // aged past the reconcile threshold
    mount_order_pre_choose(&humble, "gkG").await;

    run_reconcile(&deps).await; // however the existing reconcile tests trigger it (via handle Sync or direct)

    let claim = store.get_claim(domain::SELF_LINK_TOKEN, "sc-r1").await.unwrap().unwrap();
    assert_eq!(claim.state, domain::ClaimState::Compensated);
    assert_eq!(store.get_game("gkG:off_g").await.unwrap().unwrap().status, domain::GameStatus::Available);
}

#[tokio::test]
async fn reconcile_self_choice_b2_reveals_never_chooses() {
    // Snapshot present + new unredeemed tpk ⇒ reveal from reconcile. The mock mounts NO
    // choosecontent route — a choose attempt would 404 and fail the test (merge-gate style).
    let (deps, store, humble) = test_deps().await;
    seed_choice_game(&store, "gkH:off_h", "Crashed Mid-Claim").await;
    store.claim_game_self("gkH:off_h", "sc-r2", old_enough()).await.unwrap();
    store.record_choice_intent(domain::SELF_LINK_TOKEN, "sc-r2", vec![]).await.unwrap(); // pre=[] ⇒ any tpk is new
    mount_order_with_unredeemed_tpk(&humble, "gkH", "off_h_choice_steam", 0, "Crashed Mid-Claim").await;
    mount_reveal_success(&humble, "RECONCILED-KEY").await;
    // deliberately NO choosecontent mock.

    run_reconcile(&deps).await;

    let claim = store.get_claim(domain::SELF_LINK_TOKEN, "sc-r2").await.unwrap().unwrap();
    assert_eq!(claim.state, domain::ClaimState::Fulfilled);
    assert_eq!(claim.revealed_key.as_deref(), Some("RECONCILED-KEY"));
}

#[tokio::test]
async fn reconcile_self_bundle_already_redeemed_recovers_key() {
    // SELF bundle claim whose tpk shows redeemed WITH a key value ⇒ recover + record (gift would ping).
    let (deps, store, humble) = test_deps().await;
    seed_available_game(&store, "gkI:mnI", "Old Reveal").await;
    store.claim_game_self("gkI:mnI", "sc-r3", old_enough()).await.unwrap();
    mount_order_with_redeemed_tpk(&humble, "gkI", "mnI", "OLD-KEY").await;

    run_reconcile(&deps).await;

    let claim = store.get_claim(domain::SELF_LINK_TOKEN, "sc-r3").await.unwrap().unwrap();
    assert_eq!(claim.revealed_key.as_deref(), Some("OLD-KEY"));
    assert_eq!(claim.state, domain::ClaimState::Fulfilled);
}
```

IMPLEMENTER NOTE: `old_enough()` / `run_reconcile` — mirror how the EXISTING reconcile tests age claims + trigger the pass (grep `reconcile` in handler_test.rs; the merge-gate test at :1603 shows the no-choosecontent-mount discipline). The bundle-branch discriminator: check how reconcile's bundle arm reads the game (`requires_choice=false`) and finds its tpk — the SELF variant slots in at the same three decision points.

- [ ] **Step 2: Verify failure** — the two compensate/reveal tests fail (gift-shaped terminals wedge or ping).

- [ ] **Step 3: Implement** per the table + `compensate_any` extraction. Also extend the merge-gate test (:1603) — it must now also cover a SELF choice claim in the same no-choosecontent-route run (add a parked SELF claim to its setup and assert it completes/parks without any choose POST).

- [ ] **Step 4: Verify** — `cargo test -p fulfillment 2>&1 | tail -5` — entire crate green (existing reconcile matrix + merge gate + new SELF tests).

- [ ] **Step 5: Commit**

```bash
git add crates/fulfillment && git commit -S -m "feat(fulfillment): reconcile discriminates SELF claims — reveal/recover/self-compensate terminals, merge-gate extended"
```

---

### Task 9: admin-api — invoker + endpoints + views

**Files:**
- Modify: `crates/admin-api/src/lib.rs` (AdminInvoker :35-43, router :60-89, CatalogGameView ~:185-197, AdminClaimView :373-378)
- Modify: `crates/admin-api/src/main.rs` (the AdminInvoker impl — add the RequestResponse method, mirroring public-api's LambdaInvoker :30-53)
- Test: `crates/admin-api/tests/api_test.rs`

**Interfaces:**
- Consumes: `FulfillRequest::SelfClaim` / `FulfillResponse::{RevealedKey, AlreadyRedeemed, Parked, Error}`; `store.claim_game_self`, `store.claims_for_link`, `store.get_game`.
- Produces (Task 11's web client consumes these wire shapes):
  - `POST /admin/api/games/:id/self-claim` → 200 `{"revealed_key": "...", "key_type": "steam"}` | 202 `{"status":"processing","message":"…"}` | 409 `{"error":"…"}` | 500 `{"error":"…"}`
  - `GET /admin/api/claims/self` → `[{ "game_id", "state", "revealed_key": string|null, "created_at" }]`
  - `CatalogGameView` gains `requires_choice: bool`.

- [ ] **Step 1: Failing tests** (in `api_test.rs`, following its existing session-cookie + mock-invoker harness — grep how existing tests fake `AdminInvoker`):

```rust
#[tokio::test]
async fn self_claim_endpoint_intakes_invokes_and_returns_key() {
    // Mock invoker returns RevealedKey; assert 200 body + that the SelfClaim request carried the
    // game's real gamekey/machine_name/keyindex/requires_choice.
    let (app, store, invoker_log) = test_app_with_call_invoker(
        fulfillment::FulfillResponse::RevealedKey { key: "K-123".into() },
    ).await;
    seed_available_game(&store, "gkJ:mnJ", "Endpoint Game").await;

    let resp = authed_post(&app, "/admin/api/games/gkJ:mnJ/self-claim", "{}").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = body_json(resp).await;
    assert_eq!(body["revealed_key"], "K-123");
    assert_eq!(body["key_type"], "steam");
    // Intake really happened:
    let claim = store.claims_for_link(domain::SELF_LINK_TOKEN).await.unwrap();
    assert_eq!(claim.len(), 1);
    // And the invoke carried the right identifiers:
    let sent = invoker_log.lock().unwrap().clone();
    assert!(matches!(sent[0], fulfillment::FulfillRequest::SelfClaim { ref game_id, .. } if game_id == "gkJ:mnJ"));
}

#[tokio::test]
async fn self_claim_endpoint_409s_when_game_pending() {
    let (app, store, _) = test_app_with_call_invoker(
        fulfillment::FulfillResponse::RevealedKey { key: "unused".into() },
    ).await;
    let mut g = sample_game("gkK:mnK");
    g.status = domain::GameStatus::Pending;
    store.put_game(&g).await.unwrap();
    let resp = authed_post(&app, "/admin/api/games/gkK:mnK/self-claim", "{}").await;
    assert_eq!(resp.status(), 409);
}

#[tokio::test]
async fn self_claim_endpoint_202_on_parked() {
    let (app, store, _) = test_app_with_call_invoker(
        fulfillment::FulfillResponse::Parked { reason: "x".into() },
    ).await;
    seed_available_game(&store, "gkL:mnL", "Parked Game").await;
    let resp = authed_post(&app, "/admin/api/games/gkL:mnL/self-claim", "{}").await;
    assert_eq!(resp.status(), 202);
}

#[tokio::test]
async fn claims_self_lists_revealed_keys_without_link_precheck() {
    // No LINK#SELF META exists — the endpoint must NOT 404 (the /links/:token/claims handler would).
    let (app, store, _) = test_app_with_call_invoker(
        fulfillment::FulfillResponse::RevealedKey { key: "unused".into() },
    ).await;
    seed_available_game(&store, "gkM:mnM", "Listed Game").await;
    store.claim_game_self("gkM:mnM", "c-l1", time::OffsetDateTime::now_utc()).await.unwrap();
    store.fulfill_self_claim("c-l1", "gkM:mnM", "LIST-KEY").await.unwrap();

    let resp = authed_get(&app, "/admin/api/claims/self").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = body_json(resp).await;
    assert_eq!(body[0]["revealed_key"], "LIST-KEY");
}

#[tokio::test]
async fn catalog_exposes_requires_choice() {
    let (app, store, _) = test_app_with_call_invoker(
        fulfillment::FulfillResponse::RevealedKey { key: "unused".into() },
    ).await;
    let mut g = sample_game("gkN:mnN");
    g.requires_choice = true;
    store.put_game(&g).await.unwrap();
    let resp = authed_get(&app, "/admin/api/catalog").await;
    let body: serde_json::Value = body_json(resp).await;
    assert_eq!(body[0]["requires_choice"], true);
}

#[tokio::test]
async fn gift_link_claims_still_hide_gift_url() {
    // The scoped invariant: AdminClaimView (gift surface) is UNCHANGED — no gift_url, no revealed_key.
    // Reuse the existing handle_link_claims test and additionally assert the JSON has no
    // "gift_url"/"revealed_key" keys.
}
```

- [ ] **Step 2: Verify failure** — compile FAIL (`call` missing on AdminInvoker, routes missing).

- [ ] **Step 3: Implement.**

(a) Extend the trait (:35-43) — and UPDATE its doc comment, which currently claims the blocking invoke "left with" the cookie-paste teardown:

```rust
#[async_trait]
pub trait AdminInvoker: Send + Sync {
    /// Fire-and-forget invoke (`Event`) — sync-now. See handle_sync.
    async fn fire(&self, req: FulfillRequest) -> Result<(), String>;
    /// Blocking `RequestResponse` invoke — self-claim needs the fulfillment RESULT (the revealed
    /// key) inside the request/response cycle, exactly like public-api's claim path. A reveal is
    /// seconds, not minutes: safe through the HTTP path.
    async fn call(&self, req: FulfillRequest) -> Result<fulfillment::FulfillResponse, String>;
}
```

In `main.rs`, implement `call` on the existing invoker struct exactly like public-api's `LambdaInvoker::gift` (:30-53) — `InvocationType::RequestResponse`, serialize req, deserialize `FulfillResponse` from the payload blob. (No new IAM: `lambda:InvokeFunction` already covers both invocation types.)

(b) Routes (in the protected router block):

```rust
        .route("/admin/api/games/:id/self-claim", post(handle_self_claim))
        .route("/admin/api/claims/self", get(handle_self_claims))
```

(c) `CatalogGameView` gains `requires_choice: bool` (+ populate in `handle_catalog`: `requires_choice: g.requires_choice`).

(d) Update the AdminClaimView doc comment (:370-372) — the invariant is now SCOPED: "the friend's one-time gift URL never reaches the admin surface. Self-claims are different by design: `revealed_key` is Ben's own key and is served by `handle_self_claims` ONLY (never on this gift-claim view)."

(e) Handlers:

```rust
// ── POST /admin/api/games/:id/self-claim ─────────────────────────────────────

/// Self-claim view of a claim — the ONE admin surface that serves a key value (Ben's own).
#[derive(serde::Serialize)]
struct SelfClaimView {
    game_id: String,
    state: domain::ClaimState,
    revealed_key: Option<String>,
    created_at: String,
}

async fn handle_self_claim(State(s): State<AppState>, Path(id): Path<String>) -> Response {
    // 1. Read the game — need gamekey/machine_name/keyindex/requires_choice for the invoke,
    //    and key_type for the response.
    let game = match s.store.get_game(&id).await {
        Ok(Some(g)) => g,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    if game.status != domain::GameStatus::Available {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": "game is not available"})),
        )
            .into_response();
    }

    // 2. Intake under LINK#SELF (single-winner on the status condition).
    let claim_id = uuid::Uuid::new_v4().to_string();
    if let Err(e) = s
        .store
        .claim_game_self(&id, &claim_id, OffsetDateTime::now_utc())
        .await
    {
        use dynamo::ClaimTxError;
        return match e {
            ClaimTxError::GameUnavailable | ClaimTxError::TxConflict => (
                StatusCode::CONFLICT,
                Json(serde_json::json!({"error": "game was just claimed — refresh"})),
            )
                .into_response(),
            _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
    }

    // 3. Synchronous fulfillment — the reveal happens now; parks return 202.
    let req = FulfillRequest::SelfClaim {
        claim_id: claim_id.clone(),
        game_id: id.clone(),
        gamekey: game.gamekey.clone(),
        machine_name: game.machine_name.clone(),
        keyindex: game.keyindex,
        requires_choice: game.requires_choice,
    };
    match s.invoker.call(req).await {
        Ok(fulfillment::FulfillResponse::RevealedKey { key }) => (
            StatusCode::OK,
            Json(serde_json::json!({"revealed_key": key, "key_type": game.key_type})),
        )
            .into_response(),
        Ok(fulfillment::FulfillResponse::AlreadyRedeemed) => (
            // Only reachable if a future path compensates; today recover-then-record owns it.
            StatusCode::GONE,
            Json(serde_json::json!({"error": "key was already redeemed"})),
        )
            .into_response(),
        Ok(fulfillment::FulfillResponse::Parked { .. }) => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({
                "status": "processing",
                "message": "reveal parked — the key will appear under self-claims when reconcile completes"
            })),
        )
            .into_response(),
        Ok(_) | Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "fulfillment failed — check self-claims later; the claim is recorded"})),
        )
            .into_response(),
    }
}

// ── GET /admin/api/claims/self ────────────────────────────────────────────────

/// Self-claims list. NOTE: deliberately no link-existence pre-check — LINK#SELF has no META item
/// (handle_link_claims' pre-check would 404 this; do not reuse it).
async fn handle_self_claims(State(s): State<AppState>) -> Response {
    match s.store.claims_for_link(domain::SELF_LINK_TOKEN).await {
        Ok(claims) => {
            let views: Vec<SelfClaimView> = claims
                .into_iter()
                .map(|c| SelfClaimView {
                    game_id: c.game_id,
                    state: c.state,
                    revealed_key: c.revealed_key,
                    created_at: c
                        .created_at
                        .format(&time::format_description::well_known::Rfc3339)
                        .unwrap_or_default(),
                })
                .collect();
            (StatusCode::OK, Json(views)).into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
```

Also update the module doc-comment route list (:3-12) with the two new routes, and add every existing mock `AdminInvoker` in tests a `call` impl.

- [ ] **Step 4: Verify** — `cargo test -p admin-api 2>&1 | tail -5` all green.

- [ ] **Step 5: Commit**

```bash
git add crates/admin-api && git commit -S -m "feat(admin-api): self-claim endpoint (RequestResponse invoker) + self-claims list + requires_choice on catalog"
```

---

### Task 10: public-api — pin `/l/SELF` 404

**Files:**
- Test: `crates/public-api/tests/api_test.rs` (test-only task)

- [ ] **Step 1: Write the test** (it should pass immediately — it PINS existing behavior against future drift):

```rust
#[tokio::test]
async fn self_reserved_token_is_a_plain_404() {
    let (app, _store) = test_app().await; // the file's existing constructor
    let resp = get(&app, "/api/l/SELF").await;
    assert_eq!(resp.status(), 404);
    // Byte-identical to any unknown token (no oracle):
    let other = get(&app, "/api/l/nonexistent0000000000000000000000000000000000000000000000000000").await;
    assert_eq!(body_bytes(resp).await, body_bytes(other).await);
}
```

- [ ] **Step 2: Run** — `cargo test -p public-api self_reserved 2>&1 | tail -3` → PASS (if it fails, STOP: the reserved-token assumption is broken; escalate).

- [ ] **Step 3: Commit**

```bash
git add crates/public-api && git commit -S -m "test(public-api): pin /l/SELF as a byte-identical 404 (reserved token)"
```

---

### Task 11: web — catalog self-claim action + self-claims list

**Files:**
- Modify: `web/src/api.ts` (types + two functions), `web/src/admin/Catalog.tsx` (arm/confirm action + result panel), `web/src/admin/AdminApp.tsx` ONLY if a nav entry is added (not required: self-claims render as a Catalog section)
- Test: `web/src/api.test.ts`, `web/src/admin/Catalog.test.tsx`

**Interfaces:**
- Consumes: Task 9's wire shapes.
- Produces: UI only.

- [ ] **Step 1: Failing tests.** In `web/src/api.test.ts` (follow the existing fetch-mock style):

```typescript
it('adminSelfClaim returns the revealed key on 200', async () => {
  mockFetch(200, { revealed_key: 'K-1', key_type: 'steam' });
  const out = await adminSelfClaim('gk:mn');
  expect(out).toEqual({ kind: 'revealed', key: 'K-1', keyType: 'steam' });
});

it('adminSelfClaim maps 202 to processing', async () => {
  mockFetch(202, { status: 'processing', message: 'parked' });
  const out = await adminSelfClaim('gk:mn');
  expect(out.kind).toBe('processing');
});

it('adminSelfClaim maps 409 to refused with message', async () => {
  mockFetch(409, { error: 'game was just claimed — refresh' });
  const out = await adminSelfClaim('gk:mn');
  expect(out).toEqual({ kind: 'refused', message: 'game was just claimed — refresh' });
});

it('adminSelfClaims lists claims', async () => {
  mockFetch(200, [{ game_id: 'g', state: 'fulfilled', revealed_key: 'K', created_at: '2026-07-06T00:00:00Z' }]);
  const out = await adminSelfClaims();
  expect(out[0].revealed_key).toBe('K');
});
```

In `Catalog.test.tsx` (follow its existing render/mocking pattern):

```typescript
it('self-claim is two-step: arm then confirm, loud on choice games', async () => {
  // render catalog with one available requires_choice game; click "claim for me" → button
  // becomes "confirm? spends 1 pick"; click again → adminSelfClaim called once.
});

it('revealed key panel shows copy box and steam register link for steam keys', async () => {
  // adminSelfClaim resolves { kind:'revealed', key:'AAAA', keyType:'steam' } → expect an
  // <a href="https://store.steampowered.com/account/registerkey?key=AAAA"> and the key text.
});

it('non-steam reveal shows key without the steam button', async () => {
  // keyType 'gog' → key text present, no registerkey link.
});
```

- [ ] **Step 2: Verify failure** — `cd web && npx vitest run src/api.test.ts src/admin/Catalog.test.tsx 2>&1 | tail -8` → FAIL.

- [ ] **Step 3: Implement.**

`web/src/api.ts` — types + calls (match the existing error-mapping style of `claimGame` :129):

```typescript
export type AdminGame = {
  // …existing fields…
  requires_choice: boolean;   // ← add
};

export type SelfClaimResult =
  | { kind: 'revealed'; key: string; keyType: string }
  | { kind: 'processing' }
  | { kind: 'refused'; message: string }
  | { kind: 'error' };

export type SelfClaimView = {
  game_id: string;
  state: 'pending' | 'fulfilled' | 'compensated';
  revealed_key: string | null;
  created_at: string;
};

export async function adminSelfClaim(gameId: string): Promise<SelfClaimResult> {
  let response: Response;
  try {
    response = await fetch(`/admin/api/games/${encodeURIComponent(gameId)}/self-claim`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: '{}',
    });
  } catch {
    return { kind: 'error' };
  }
  if (response.status === 401) throw new Unauthorized();
  try {
    if (response.status === 200) {
      const b = await response.json();
      return { kind: 'revealed', key: b.revealed_key, keyType: b.key_type };
    }
    if (response.status === 202) return { kind: 'processing' };
    if (response.status === 409 || response.status === 410) {
      const b = await response.json();
      return { kind: 'refused', message: b.error ?? 'refused' };
    }
  } catch {
    return { kind: 'error' };
  }
  return { kind: 'error' };
}

export async function adminSelfClaims(): Promise<SelfClaimView[]> {
  const response = await fetch('/admin/api/claims/self');
  if (response.status === 401) throw new Unauthorized();
  if (!response.ok) throw new FetchFailed();
  return response.json();
}
```

`web/src/admin/Catalog.tsx` — per available-game row, an arm/confirm button (the Links.tsx revoke idiom :327-345 — a component-state `armedId: string | null`, first click arms, second fires; the armed label is `confirm?` or, when `game.requires_choice`, `confirm? spends 1 pick`), then a result panel. Sketch of the state + handlers (adapt to the component's existing structure — it already has optimistic-update state for `hidden`):

```typescript
const [armedId, setArmedId] = useState<string | null>(null);
const [claiming, setClaiming] = useState<string | null>(null);
const [result, setResult] = useState<{ gameId: string; r: SelfClaimResult } | null>(null);

async function handleSelfClaim(g: AdminGame) {
  if (armedId !== g.id) {
    setArmedId(g.id);
    return;
  }
  setArmedId(null);
  setClaiming(g.id);
  const r = await adminSelfClaim(g.id);
  setClaiming(null);
  setResult({ gameId: g.id, r });
  refresh(); // re-fetch the catalog — status changed server-side
}
```

Result panel (rendered under the row / as a dismissible strip):

```tsx
{result?.r.kind === 'revealed' && (
  <div className="rounded bg-emerald-950 p-3 text-sm">
    <span className="select-all font-mono">{result.r.key}</span>
    <button type="button" onClick={() => copyToClipboard(result.r.key)} className="ml-2 rounded bg-zinc-700 px-2 py-1 text-xs">copy</button>
    {result.r.keyType === 'steam' && (
      <a
        href={`https://store.steampowered.com/account/registerkey?key=${encodeURIComponent(result.r.key)}`}
        target="_blank" rel="noreferrer"
        className="ml-2 rounded bg-blue-700 px-2 py-1 text-xs"
      >
        redeem on steam
      </a>
    )}
    <button type="button" onClick={() => setResult(null)} className="ml-2 text-xs text-zinc-400">dismiss</button>
  </div>
)}
{result?.r.kind === 'processing' && (
  <div className="rounded bg-amber-950 p-3 text-sm">
    reveal is processing — the key will appear under self-claims below.
    <button type="button" onClick={() => setResult(null)} className="ml-2 text-xs">dismiss</button>
  </div>
)}
{result?.r.kind === 'refused' && (
  <div className="rounded bg-red-950 p-3 text-sm">{result.r.message}
    <button type="button" onClick={() => setResult(null)} className="ml-2 text-xs">dismiss</button>
  </div>
)}
```

Self-claims section at the bottom of Catalog (fetch `adminSelfClaims()` on mount + after any claim): a simple list — title/game_id, state badge, `revealed_key` in a select-all mono span with copy button when present. Follow the ClaimsHistory/Links audit-list styling.

- [ ] **Step 4: Verify** — `cd web && npx vitest run 2>&1 | tail -5` → whole suite green; `npx tsc --noEmit` clean.

- [ ] **Step 5: Commit**

```bash
git add web && git commit -S -m "feat(web): catalog self-claim — arm/confirm, key reveal panel, steam register link, self-claims list"
```

---

### Task 12: ship — branch, CI, PR, DEPLOY, live receipts (fork-to-deployed; Ben is NOT involved until it's live)

Ben's standing instruction (2026-07-06): do the whole thing — build → merge → **deploy → live-verify** — and only then report. He pre-designates the validation games (ask for them ONCE, up front, alongside the deploy webhook secret if deploy.tfvars needs recreating).

- [ ] **Step 1:** all work above happens on branch `kitten/self-claim` (create it BEFORE Task 1: `git checkout -b kitten/self-claim`). Verify every commit is signed: `git log --format='%h %GK %s' main..HEAD` — every line shows `F2060B93112D9ACF`.
- [ ] **Step 2:** full local sweep: `cargo test --workspace 2>&1 | tail -5` (skip if boring cache is cold — CI is the builder) + `cd web && npx vitest run && npx tsc --noEmit`.
- [ ] **Step 3:** push + PR: `git push -u origin kitten/self-claim && gh pr create -R yourcodekitten/bendobundles --title "feat: admin self-claim — key reveal (spec 2026-07-06)" --body "<summary + spec link + live-receipt plan>"`.
- [ ] **Step 4:** watch CI to green (`gh pr checks --watch`); fix reds fork-to-ready.
- [ ] **Step 5:** per HR#1, merge when ready (squash + delete branch), then wait for the main-push CI run to build `lambda-zips`.
- [ ] **Step 6: DEPLOY** (the proven targeted-apply procedure — state/JOURNAL.md 2026-07-06 + state/decisions.md have the scars):
  - `gh run download <mainRunId> -R yourcodekitten/bendobundles -n lambda-zips`; stage `fulfillment/bootstrap.zip` AND `admin-api/bootstrap.zip` into `~/bendobundles/terraform/artifacts/` (this feature changes BOTH lambdas).
  - Recreate `~/bendobundles/terraform/deploy.tfvars` (600, gitignored, NEVER committed): 4 knowns (aws_account_id=672812236571, domain_zone_id, boundary arn, admin_password_hash sentinel) + humble_username=craftsman@bendoerr.me + discord_webhook_url (the one secret — from Ben, once).
  - `AWS_PROFILE=kitten-deploy terraform plan -target=module.lambda_fulfillment.aws_lambda_function.this -target=module.lambda_admin_api.aws_lambda_function.this -var-file=deploy.tfvars -out=<scratchpad>/deploy.plan` → READ every line (expect 2 change / 0 destroy; a webhook value change means the value is WRONG — STOP) → apply the saved plan → verify both CodeSha values changed.
  - **Deploy the SPA too** — lambda deploy ≠ SPA deploy (journal 2026-07-06 lesson): build web, `aws s3 sync` + CloudFront invalidation per the admin-SPA redeploy procedure in the journal.
  - `shred -u` the tfvars + plan file.
- [ ] **Step 7: LIVE RECEIPTS** (spec §6.5) on the games Ben pre-designated: one plain bundle key → one non-giftable key (the §2 assumption check) → one choice game (spends a real pick). Verify each end-to-end: 200 with the key (or 202→reconcile→key in self-claims), claim `fulfilled` with `revealed_key` recorded, game `ben_redeemed`, CloudWatch (AWS_PROFILE=kitten-debug) shows the reveal POST → 200 chain and NO key value in any log line.
- [ ] **Step 8: report ONCE** to Ben on discord: deployed + tested + live, with the receipts.

---

## Self-Review Notes (already applied)

- Spec §3's three writes → Tasks 3/4/5 one-to-one; §4's decision + recover + choice + reconcile table → Tasks 6/7/8; §5's API/UI (incl. the no-precheck note, requires_choice exposure, scoped secrets invariant) → Tasks 9/11; §7's test list → distributed: races (T3), durable-first + idempotence (T4), SELF-compensate (T5), recover + fallback (T6), never-choose (T7/T8 + merge-gate), /l/SELF 404 (T10).
- Log-scrubbing (`revealed_key` never logged): enforced by construction in Tasks 1/6 (log lines name identifiers only); reviewer checklist item on the PR.
- Type-consistency check: `RevealedKey(pub String)` (T1) → `reveal_decision(&Result<RevealedKey, HumbleError>)` (T6) → `FulfillResponse::RevealedKey { key: String }` (T6) → admin 200 `{revealed_key, key_type}` (T9) → `SelfClaimResult.kind='revealed'` (T11). `claim_game_self(game_id, claim_id, now)` (T3) called with that exact arg order in T9.
- Known judgment calls an implementer may hit: the shared error-mapping extractions (T3/T5/T6's `gift_error_decision`) must be behavior-preserving refactors — existing tests are the net; run them first.
