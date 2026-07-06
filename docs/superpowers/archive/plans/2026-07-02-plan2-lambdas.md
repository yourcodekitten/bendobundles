# bendobundles Plan 2: Lambdas Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The three service lambdas — `fulfillment` (sole humble-toucher: gift generation, sync,
reconcile, cookie validation), `public-api` (friend surface), `admin-api` (ben surface) — plus the
domain/humble-client/dynamo extensions they need.

**Architecture:** One rust workspace (plan 1's), three new binary crates built with lambda_http /
lambda_runtime. Trust boundary from the spec holds: only `fulfillment`'s IAM role reads the humble
session secret; `public-api` reaches humble exclusively through a narrow lambda-invoke contract.
All humble knowledge stays in `humble-client`; all storage knowledge in `dynamo`; status/merge
policy in `domain` (pure, unit-testable).

**Tech Stack:** plan-1 stack + lambda_http 0.13 / lambda_runtime 0.13, aws-sdk-lambda,
aws-sdk-ssm, axum 0.7 (via lambda_http's tower integration), argon2 0.5, tower (dev: for
oneshot handler tests).

**Read first:** the spec (`docs/superpowers/specs/2026-07-02-bendobundles-design.md`) §3-§9, and
plan 1 (`docs/superpowers/plans/2026-07-02-plan1-backend-core.md`) for conventions. Main is
`d7e7a1d` — domain/humble-client/dynamo exist and are live-validated; read their sources before
extending them.

## Global Constraints

- All commits GPG-signed (`git commit -S`), authored `code kitten <yourcodekitten@gmail.com>`.
- `cargo fmt --check` and `cargo clippy --workspace --all-targets --all-features -- -D warnings` pass at every commit.
- No live humble calls in tests, ever (wiremock only). No docker on the dev box: dynamo
  integration tests SKIP locally and run in CI — never claim a local live-dynamo run; CI is the
  only accepted integration receipt.
- The humble session cookie and admin password hash are secrets: never logged, never in error
  bodies, never in test fixtures. `SessionCookie` already redacts; keep it that way.
- **The claim invariant rules every error arm: a humble key burns exactly once, and a burned
  key's gift URL is never lost.** When an outcome is ambiguous the claim PARKS (stays pending for
  reconcile) — parking is always safe; compensating on ambiguity can double-burn. Compensate ONLY
  on outcomes where humble told us the key's state definitively.
- Friend-facing error bodies never leak humble account details, internal error strings, or claim
  ids of other links.
- Config via env vars only (set by terraform in plan 4): `TABLE_NAME`, `HUMBLE_COOKIE_PARAM`,
  `ADMIN_HASH_PARAM`, `FULFILLMENT_FN` (public-api), `DISCORD_WEBHOOK_PARAM` (fulfillment),
  `BASE_URL` (humble base, default `https://www.humblebundle.com`).
- Item shapes from plan 1's `dynamo::schema` are the storage contract — extend, never mutate
  existing attributes' meaning. Top-level `claims_used` stays the authoritative counter.
- **Lambdas deploy on ARM64 (Graviton — ben's call, 2026-07-02).** Pure-rust code needs nothing
  special, but: no x86-only assumptions, no `cc`-dependent crates without checking aarch64, and
  the deploy artifact build (plan 4 CI) is `cargo lambda build --release --arm64`. rustls (already
  the TLS stack) is arm-clean.

## File Structure (locked by this plan)

```
crates/
  domain/src/lib.rs            # + Game.keyindex, sync merge policy, games_from_order mapper
  humble-client/src/lib.rs     # + keyindex/subproducts parse, redeem takes keyindex
  humble-client/src/model.rs   # + keyindex + subproducts wire types
  dynamo/src/lib.rs            # + upsert_game_from_sync, list_pending_claims, sync state, sessions
  fulfillment/src/main.rs      # lambda_runtime dispatch (op enum)
  fulfillment/src/lib.rs       # gift ladder, sync pass, reconcile pass, cookie validate, discord ping
  fulfillment/tests/           # wiremock-driven handler tests
  public-api/src/main.rs       # lambda_http entry
  public-api/src/lib.rs        # axum router: GET /api/l/:token, POST /api/l/:token/claim
  public-api/tests/
  admin-api/src/main.rs        # lambda_http entry
  admin-api/src/lib.rs         # login, sessions, links CRUD, hidden toggle, cookie paste, sync-now
  admin-api/tests/
```

---

### Task 1: humble-client — keyindex + subproducts on the wire; redeem takes the true index

**Files:**
- Modify: `crates/humble-client/src/model.rs`, `crates/humble-client/src/lib.rs`,
  `crates/humble-client/tests/client_test.rs`, `crates/humble-client/tests/fixtures/order_detail.json`
- Modify: `crates/humble-client/src/bin/probe.rs` (only if the compiler forces it — KeyEntry gains a field)

**Interfaces:**
- Consumes: existing `HumbleClient`, `Order`, `KeyEntry`.
- Produces:
  - `KeyEntry` gains `pub keyindex: u32` (wire `keyindex`, present in 100% of live-captured tpks;
    `#[serde(default)]` on the wire type anyway — a missing keyindex defaults 0, today's behavior).
  - `Order` gains `pub subproducts: Vec<Subproduct>`;
    `Subproduct { pub machine_name: String, pub human_name: String, pub icon: Option<String> }`.
  - `redeem_as_gift(&self, gamekey: &str, machine_name: &str, keyindex: u32)` — third param
    replaces the hardcoded `"0"`; form sends `keyindex=<value>`. All call sites updated
    (tests; probe has no redeem path).

- [ ] **Step 1: Extend the fixture** — in `order_detail.json` add `"keyindex": 0`, `"keyindex": 1`,
  `"keyindex": 2` to the three tpks respectively, and add a top-level sibling of `tpkd_dict`:

```json
  "subproducts": [
    { "machine_name": "stardew_valley", "human_name": "Stardew Valley",
      "icon": "https://hb.imgix.net/stardew.png", "downloads": [] },
    { "machine_name": "noicon_game", "human_name": "No Icon Game", "downloads": [] }
  ]
```

- [ ] **Step 2: Write the failing tests** — extend `parses_order_key_states` with:

```rust
    assert_eq!(order.keys[0].keyindex, 0);
    assert_eq!(order.keys[1].keyindex, 1);
    assert_eq!(order.keys[2].keyindex, 2);
    assert_eq!(order.subproducts.len(), 2);
    assert_eq!(order.subproducts[0].human_name, "Stardew Valley");
    assert_eq!(
        order.subproducts[0].icon.as_deref(),
        Some("https://hb.imgix.net/stardew.png")
    );
    assert_eq!(order.subproducts[1].icon, None);
```

  and change `redeems_as_gift` to call `.redeem_as_gift("AAAAbbbbCCCC", "stardew_valley_steam", 3)`
  with matcher `body_string_contains("keyindex=3")` (replacing the `keyindex=0` matcher).

- [ ] **Step 3: Run to verify failure** — `cargo test -p humble-client` → compile errors (new
  fields/param). That's the fail.

- [ ] **Step 4: Implement** — `model.rs`: `TpkWire` gains `#[serde(default)] pub keyindex: u32`;
  new `SubproductWire { machine_name, human_name, #[serde(default)] icon: Option<String> }`;
  `OrderWire` gains `#[serde(default)] pub subproducts: Vec<SubproductWire>`. `lib.rs`: map both
  through in `order()`; `redeem_as_gift` signature + form value `("keyindex", &keyindex.to_string())`.
  Keep the existing keyindex-semantics comment, updated: we now pass the tpk's true index.

- [ ] **Step 5: Verify** — `cargo test -p humble-client` (all pass) + workspace clippy
  (`--all-features`, probe must still compile) + fmt.

- [ ] **Step 6: Commit** — `git add crates/humble-client && git commit -S -m "feat(humble-client): keyindex + subproducts on the wire; redeem passes true index"`

---

### Task 2: domain — Game.keyindex + the pure sync-merge policy

**Files:**
- Modify: `crates/domain/src/lib.rs`
- Modify (compiler-forced only): `crates/dynamo/tests/store_test.rs` and any `Game{..}` literal
  (a new field breaks struct literals — add `keyindex: 0` where tests construct games).

**Interfaces:**
- Consumes: nothing new.
- Produces:
  - `Game` gains `pub keyindex: u32`.
  - `pub fn sync_status(redeemed: bool, expired: bool) -> GameStatus` — Expired if expired,
    BenRedeemed if redeemed, else Available. (Expired wins over redeemed.)
  - `pub fn merge_sync(existing: Option<&Game>, fresh: Game) -> Option<Game>` — THE sync-merge
    policy, pure and unit-tested. Rules:
    - `existing == None` → Some(fresh) (new game).
    - existing status `Pending` or `Gifted` → app owns the record: keep existing status +
      claim_id + hidden, but refresh humble-cosmetic fields (title, bundle, artwork_url,
      keyindex, key_type) from fresh; giftable stays existing (a mid-claim key's giftability is
      app business). Return Some(merged).
    - otherwise (Available / BenRedeemed / Expired — humble-owned states) → fresh wins entirely
      EXCEPT `hidden` (ben's toggle survives every sync). Return Some(merged).
    - If nothing changed (merged == *existing.unwrap()) → None (caller skips the write).
  - `pub fn games_from_order(order: &humble... )` — NO. domain must not depend on humble-client
    (dependency direction). The order→games mapping lives in `fulfillment` (Task 4); domain
    provides only `sync_status` + `merge_sync` + an artwork matcher:
  - `pub fn match_artwork<'a>(human_name: &str, subproducts: &'a [(String, Option<String>)]) -> Option<&'a str>`
    — inputs are (human_name, icon) pairs; exact case-insensitive match on human_name first, then
    the first subproduct whose name is a case-insensitive prefix of the key's human_name or vice
    versa, else None.

- [ ] **Step 1: Write the failing tests** (append to domain's test module)

```rust
    #[test]
    fn sync_status_derivation() {
        assert_eq!(sync_status(false, false), GameStatus::Available);
        assert_eq!(sync_status(true, false), GameStatus::BenRedeemed);
        assert_eq!(sync_status(false, true), GameStatus::Expired);
        assert_eq!(sync_status(true, true), GameStatus::Expired);
    }

    fn fresh_game() -> Game {
        Game {
            id: game_id("gk", "mn"),
            title: "New Title".into(),
            bundle: "B".into(),
            gamekey: "gk".into(),
            machine_name: "mn".into(),
            key_type: "steam".into(),
            giftable: true,
            hidden: false,
            status: GameStatus::Available,
            claim_id: None,
            artwork_url: Some("new.png".into()),
            keyindex: 4,
        }
    }

    #[test]
    fn merge_new_game_is_fresh() {
        assert_eq!(merge_sync(None, fresh_game()), Some(fresh_game()));
    }

    #[test]
    fn merge_preserves_hidden_on_humble_owned() {
        let mut existing = fresh_game();
        existing.hidden = true;
        existing.title = "Old Title".into();
        let merged = merge_sync(Some(&existing), fresh_game()).unwrap();
        assert!(merged.hidden);
        assert_eq!(merged.title, "New Title");
        assert_eq!(merged.status, GameStatus::Available);
    }

    #[test]
    fn merge_never_touches_app_owned_status() {
        let mut existing = fresh_game();
        existing.status = GameStatus::Gifted;
        existing.claim_id = Some("c1".into());
        existing.title = "Old Title".into();
        let mut fresh = fresh_game();
        fresh.status = GameStatus::BenRedeemed; // humble sees the gifted key as redeemed
        let merged = merge_sync(Some(&existing), fresh).unwrap();
        assert_eq!(merged.status, GameStatus::Gifted);
        assert_eq!(merged.claim_id.as_deref(), Some("c1"));
        assert_eq!(merged.title, "New Title"); // cosmetics refresh
    }

    #[test]
    fn merge_no_change_returns_none() {
        let g = fresh_game();
        assert_eq!(merge_sync(Some(&g), g.clone()), None);
    }

    #[test]
    fn artwork_matching() {
        let subs = vec![
            ("Stardew Valley".to_string(), Some("s.png".to_string())),
            ("Undertale".to_string(), None),
            ("BIT.TRIP".to_string(), Some("b.png".to_string())),
        ];
        assert_eq!(match_artwork("stardew valley", &subs), Some("s.png"));
        assert_eq!(match_artwork("Undertale", &subs), None); // matched but no icon
        assert_eq!(match_artwork("BIT.TRIP BEAT Steam Key", &subs), Some("b.png")); // prefix
        assert_eq!(match_artwork("Nothing Alike", &subs), None);
    }
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p domain` → compile errors.
- [ ] **Step 3: Implement** exactly the semantics the tests + Interfaces block define. Fix
  compiler-forced `Game` literals in dynamo tests (`keyindex: 0`).
- [ ] **Step 4: Verify** — `cargo test --workspace` + clippy `--all-features` + fmt.
- [ ] **Step 5: Commit** — `git add crates/domain crates/dynamo && git commit -S -m "feat(domain): keyindex + pure sync-merge policy (hidden survives, app-owned states untouchable)"`

---

### Task 3: dynamo — guarded sync-upsert, pending-claims query, sync state, sessions

**Files:**
- Modify: `crates/dynamo/src/lib.rs`, `crates/dynamo/src/schema.rs`, `crates/dynamo/tests/store_test.rs`

**Interfaces:**
- Produces:
  - `store.upsert_game_from_sync(fresh: Game) -> Result<SyncWrite, StoreError>` where
    `pub enum SyncWrite { Written, SkippedInFlight, Unchanged }`:
    read existing → `domain::merge_sync` → None ⇒ Unchanged (no write); Some(merged) where
    existing was `Pending` ⇒ **conditional** put with `#st = :pending` (if the condition fails
    the claim finished mid-sync — return SkippedInFlight, do NOT retry blind); otherwise
    conditional put `attribute_not_exists(pk) OR #st = :expected` (`:expected` = the status the
    read saw — optimistic lock; CCF ⇒ SkippedInFlight). Doc comment: this is the ONLY correct
    writer for sync; `put_game` remains unsafe for sync (existing warning stays).
  - `store.list_pending_claims() -> Result<Vec<Claim>, StoreError>` — Query GSI `pending-claims`
    (`gsi2pk = "PENDINGCLAIM"`), ascending by gsi2sk (oldest first), single page + doc comment
    (same convention as list_listable_games).
  - `SyncState { pub last_run_epoch: i64, pub ok: bool, pub cookie_ok: bool, pub games_written: u32, pub message: String }`
    (define in `dynamo` — it's storage-shaped, not domain policy); `store.put_sync_state(&SyncState)`,
    `store.get_sync_state() -> Result<Option<SyncState>, StoreError>`. Item: `pk="SYNC#STATE"`,
    `sk="META"`, `body`=json.
  - Admin sessions: `store.create_session(token: &str, expires_epoch: i64)`,
    `store.get_session(token: &str) -> Result<Option<i64 /*expires_epoch*/>, StoreError>` (caller
    checks expiry against now), `store.delete_session(token: &str)`. Item: `pk="SESSION#<token>"`,
    `sk="META"`, top-level `expires_epoch` N + `ttl` N (same value; terraform enables TTL on `ttl`
    in plan 4 — until then expiry enforcement is the epoch check in code).

- [ ] **Step 1: Write the failing tests** (store_or_skip pattern, unique tables):

```rust
#[tokio::test]
async fn sync_upsert_respects_ownership() {
    let Some(store) = store_or_skip("sync-upsert").await else { return };
    // new game → Written
    let g = game(1, true);
    assert!(matches!(store.upsert_game_from_sync(g.clone()).await.unwrap(), SyncWrite::Written));
    // unchanged → Unchanged
    assert!(matches!(store.upsert_game_from_sync(g.clone()).await.unwrap(), SyncWrite::Unchanged));
    // hidden survives a humble-side change
    let mut hidden = g.clone();
    hidden.hidden = true;
    store.put_game(&hidden).await.unwrap();
    let mut fresh = g.clone();
    fresh.title = "Renamed".into();
    assert!(matches!(store.upsert_game_from_sync(fresh).await.unwrap(), SyncWrite::Written));
    let now = store.get_game(&g.id).await.unwrap().unwrap();
    assert!(now.hidden);
    assert_eq!(now.title, "Renamed");
    // pending game: sync may refresh cosmetics but never the status
    store.put_link(&link("tok1")).await.unwrap(); // uses create_link if Task renamed it — match current API
    store.claim_game("tok1", &g.id, "c1", datetime!(2026-07-02 12:00 UTC)).await.unwrap();
    let mut fresh2 = g.clone();
    fresh2.status = GameStatus::BenRedeemed;
    fresh2.title = "Renamed Again".into();
    let w = store.upsert_game_from_sync(fresh2).await.unwrap();
    assert!(matches!(w, SyncWrite::Written | SyncWrite::SkippedInFlight));
    let after = store.get_game(&g.id).await.unwrap().unwrap();
    assert_eq!(after.status, GameStatus::Pending); // status untouched either way
}

#[tokio::test]
async fn pending_claims_and_sync_state_and_sessions() {
    let Some(store) = store_or_skip("pending-state-sessions").await else { return };
    store.put_game(&game(1, true)).await.unwrap();
    store.create_link(&link("tok1")).await.unwrap();
    store.claim_game("tok1", &game_id("gk1", "mn"), "c1", datetime!(2026-07-02 12:00 UTC)).await.unwrap();
    let pending = store.list_pending_claims().await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, "c1");

    let st = SyncState { last_run_epoch: 1_800_000_000, ok: true, cookie_ok: true, games_written: 3, message: "ok".into() };
    store.put_sync_state(&st).await.unwrap();
    assert_eq!(store.get_sync_state().await.unwrap().unwrap(), st);

    store.create_session("sess1", 2_000_000_000).await.unwrap();
    assert_eq!(store.get_session("sess1").await.unwrap(), Some(2_000_000_000));
    store.delete_session("sess1").await.unwrap();
    assert_eq!(store.get_session("sess1").await.unwrap(), None);
}
```

  (NOTE: `link()` test helper currently exists; `create_link` replaced `put_link` in the review
  wave — use the current store API, read the source.)

- [ ] **Step 2: Run to verify failure**, **Step 3: Implement** (SyncState derives
  Debug/Clone/PartialEq/Serialize/Deserialize), **Step 4: Verify** (workspace tests SKIP locally,
  clippy `--all-features`, fmt), **Step 5: Commit** —
  `git add crates/dynamo && git commit -S -m "feat(dynamo): guarded sync-upsert, pending-claims query, sync state, admin sessions"`

---

### Task 4: fulfillment lib — gift ladder, sync pass, reconcile pass, cookie ops, discord ping

**Files:**
- Create: `crates/fulfillment/Cargo.toml`, `crates/fulfillment/src/lib.rs`,
  `crates/fulfillment/tests/handler_test.rs`
- Modify: root `Cargo.toml` (add member)

**Interfaces:**
- Consumes: `HumbleClient` (incl. Task 1 signature), `Store` (incl. Task 3), `domain` policy fns.
- Produces (Task 5's main and public-api's invoke contract):
  - request/response types (serde, exact):
    ```rust
    #[derive(Serialize, Deserialize)]
    #[serde(tag = "op", rename_all = "snake_case")]
    pub enum FulfillRequest {
        Gift { claim_id: String, link_token: String, game_id: String,
               gamekey: String, machine_name: String, keyindex: u32 },
        Sync,
        ValidateCookie,
    }
    #[derive(Serialize, Deserialize)]
    #[serde(tag = "result", rename_all = "snake_case")]
    pub enum FulfillResponse {
        GiftUrl { url: String },
        /// definitive: key was already redeemed; claim compensated; friend should pick another
        AlreadyRedeemed,
        /// ambiguous or refused: claim stays PENDING for reconcile; friend told "processing"
        Parked { reason: String },
        SyncDone { games_written: u32, orders_failed: u32 },
        CookieStatus { ok: bool },
        Error { message: String },
    }
    ```
  - `pub struct Deps { pub store: Store, pub humble: HumbleClient, pub webhook_url: Option<String>, pub http: reqwest::Client }`
  - `pub async fn handle(deps: &Deps, req: FulfillRequest) -> FulfillResponse` — never panics;
    every arm returns a typed response.
- **The gift ladder (the heart — implement EXACTLY this policy):**
  ```rust
  match deps.humble.redeem_as_gift(&gamekey, &machine_name, keyindex).await {
      Ok(gift) => {
          // URL durable BEFORE returning — the invariant.
          match deps.store.fulfill_claim(&link_token, &claim_id, &game_id, &gift.0).await {
              Ok(()) => FulfillResponse::GiftUrl { url: gift.0 },
              // fulfill lost to compensate = loud Corrupt; the URL exists but the game moved on.
              // Surface as Error + discord ping — human decides. NEVER retry the redeem.
              Err(e) => { ping(deps, &format!("fulfill after redeem failed for claim {claim_id}: {e}")).await;
                          FulfillResponse::Error { message: "gift generated but recording failed — flagged for ben".into() } }
          }
      }
      // definitive from humble: the key was already gone. Compensate (slot returns, game re-lists;
      // the next sync corrects the game to ben-redeemed via merge policy).
      Err(HumbleError::AlreadyRedeemed) => {
          match deps.store.compensate_claim(&link_token, &claim_id, &game_id).await {
              Ok(()) => FulfillResponse::AlreadyRedeemed,
              Err(e) => { ping(deps, &format!("compensate failed for claim {claim_id}: {e}")).await;
                          FulfillResponse::Error { message: "recording failed — flagged for ben".into() } }
          }
      }
      // dead cookie: park + flag cookie state + ping. Friend sees "processing".
      Err(HumbleError::Unauthorized) => {
          let mut st = deps.store.get_sync_state().await.ok().flatten().unwrap_or_default();
          st.cookie_ok = false;
          let _ = deps.store.put_sync_state(&st).await;
          ping(deps, "humble session cookie is DEAD — paste a fresh one in admin").await;
          FulfillResponse::Parked { reason: "humble session needs attention".into() }
      }
      // EVERYTHING else is ambiguous-or-refused → PARK (never compensate blind).
      // RedeemRefused: probably not burned, but unverified. Ambiguous/Network/Api/Parse/RateLimited:
      // possibly burned. Reconcile re-checks against humble truth (Task 4 reconcile).
      Err(e) => FulfillResponse::Parked { reason: format!("humble call inconclusive: park for reconcile ({})",
                    match e { HumbleError::RedeemRefused(_) => "refused", HumbleError::AmbiguousRedeem => "ambiguous",
                              HumbleError::RateLimited => "rate-limited", _ => "transient" }) },
  }
  ```
- **Sync pass** (`FulfillRequest::Sync`): gamekeys → each order (300ms `tokio::time::sleep`
  pacing, same as the probe) → per key entry build `Game` (via `domain::sync_status`,
  `domain::match_artwork` on the order's subproducts mapped to `(human_name, icon)` pairs,
  `giftable` from KeyEntry, `keyindex` through) → `upsert_game_from_sync`; count Written; per-order
  errors increment orders_failed and continue; Unauthorized anywhere → set cookie_ok=false + ping +
  stop early. Finish: put_sync_state (ok, cookie_ok, games_written, message, last_run_epoch =
  `OffsetDateTime::now_utc().unix_timestamp()`), return SyncDone.
- **Reconcile pass** (runs at the START of every Sync, before the walk): `list_pending_claims`;
  for each claim older than 15 minutes (compare created_at to now): fetch its order from humble,
  find the key by machine_name — if `redeemed` (the gift WAS generated but we crashed pre-record):
  we cannot recover the URL from this endpoint → ping ben with claim id + game (manual recovery
  via humble's gift-history page), leave pending (loud, human-owned); if NOT redeemed → the redeem
  never landed → `compensate_claim` (slot + game return). Humble fetch error → skip (next pass).
- **`ping(deps, msg)`**: POST `{"content": "🐱 bendobundles: <msg>"}` to webhook_url if Some;
  never fails the caller (log-and-continue on error); msg must never contain cookie/URL secrets.
- **Cookie validate** (`ValidateCookie`): `deps.humble.gamekeys()` → CookieStatus{ok} + update
  SyncState.cookie_ok accordingly.
- `SyncState` needs `impl Default` (add in Task 3's file if missed — all false/0/empty).

**Tests** (`handler_test.rs`, wiremock for humble + real Store via store_or_skip; when no local
dynamo, test ONLY the pure pieces — split the gift ladder so the humble-outcome→decision mapping
is a pure fn `pub fn gift_decision(outcome: &Result<GiftUrl, HumbleError>) -> Decision` with
`enum Decision { Record, Compensate, ParkCookieDead, Park }`, unit-test THAT exhaustively (all 8
error variants), and keep the side-effecting ladder thin):

```rust
#[test]
fn gift_decision_ladder_is_exhaustive_and_safe() {
    use humble_client::{GiftUrl, HumbleError as E};
    use fulfillment::{gift_decision, Decision};
    assert!(matches!(gift_decision(&Ok(GiftUrl("u".into()))), Decision::Record));
    assert!(matches!(gift_decision(&Err(E::AlreadyRedeemed)), Decision::Compensate));
    assert!(matches!(gift_decision(&Err(E::Unauthorized)), Decision::ParkCookieDead));
    assert!(matches!(gift_decision(&Err(E::AmbiguousRedeem)), Decision::Park));
    assert!(matches!(gift_decision(&Err(E::RedeemRefused("x".into()))), Decision::Park));
    assert!(matches!(gift_decision(&Err(E::RateLimited)), Decision::Park));
    assert!(matches!(gift_decision(&Err(E::Api(500))), Decision::Park));
    // Network/Parse constructed via serde/reqwest are awkward — cover via a helper that maps
    // remaining variants to Park in the same match (compiler exhaustiveness is the real guard).
}
```

plus a wiremock+dynamo integration test (skips locally): full gift happy path (mock redeemkey
success → handle(Gift) → GiftUrl response, claim Fulfilled with URL, game Gifted) and the
already-redeemed path (claim compensated, game re-listed).

- [ ] Steps: manifest (deps: domain, humble-client, dynamo, tokio, serde, serde_json, thiserror,
  reqwest workspace-style, time; dev: wiremock, aws-config) + failing tests → red → implement →
  green (`cargo test --workspace`, clippy `--all-features`, fmt) → commit
  `git add Cargo.toml crates/fulfillment && git commit -S -m "feat(fulfillment): gift decision ladder, sync + reconcile passes, cookie ops, discord ping"`

---

### Task 5: fulfillment main — lambda_runtime dispatch + SSM config

**Files:**
- Create: `crates/fulfillment/src/main.rs`
- Modify: `crates/fulfillment/Cargo.toml` (bin deps: lambda_runtime, aws-sdk-ssm, aws-sdk-dynamodb, aws-config, tracing + tracing-subscriber)

**Interfaces:**
- Consumes: `fulfillment::handle`, `Deps`, request/response types.
- Produces: the deployable binary. `main()`:
  1. `tracing_subscriber::fmt().with_ansi(false).without_time().init()` (CloudWatch-friendly).
  2. Read env: `TABLE_NAME`, `HUMBLE_COOKIE_PARAM`, `DISCORD_WEBHOOK_PARAM` (optional), `BASE_URL`
     (default humble).
  3. aws_config once; build dynamo client + Store; SSM client.
  4. Per-invocation (inside the service fn, NOT cached): `get_parameter(with_decryption=true)` for
     the cookie param → build HumbleClient (cookie freshness beats latency here — a paste in admin
     must take effect on the next claim, no warm-container staleness); webhook URL fetched once at
     startup (non-secret-ish but stored as param; cache it).
  5. `lambda_runtime::run(service_fn(...))` deserializing `FulfillRequest` → `handle` →
     serialize `FulfillResponse`. An EventBridge scheduled event won't parse as FulfillRequest:
     the service fn first tries FulfillRequest; on parse failure of a `{"source":"aws.events",...}`
     shaped payload, treat as `Sync` (doc comment: eventbridge → sync). Anything else unparseable →
     FulfillResponse::Error.
  - SSM read failure for the cookie → `handle` is never reached for Gift/Sync/ValidateCookie needing
    humble; return `Parked/Error` shape: implement as building Deps lazily per-op — simplest: fetch
    cookie in main's service fn; on failure return `FulfillResponse::Error { message: "humble session unavailable" }`
    (never the SSM error text).
- No meaningful unit tests for main (glue); the compile gate + clippy is the bar. Keep main <100 lines.

- [ ] Steps: implement → `cargo build -p fulfillment` + `cargo clippy --workspace --all-targets --all-features -- -D warnings` + fmt + workspace tests → commit
  `git add crates/fulfillment && git commit -S -m "feat(fulfillment): lambda_runtime dispatch, per-invoke SSM cookie, eventbridge→sync"`

---

### Task 6: public-api — friend surface

**Files:**
- Create: `crates/public-api/Cargo.toml`, `crates/public-api/src/lib.rs`,
  `crates/public-api/src/main.rs`, `crates/public-api/tests/api_test.rs`
- Modify: root `Cargo.toml`

**Interfaces:**
- Consumes: Store (get_link, claims_for_link, list_listable_games, claim_game + ClaimTxError),
  domain (Link::can_claim, ClaimRefusal), fulfillment's FulfillRequest/FulfillResponse types
  (public-api depends on the fulfillment CRATE for the types only — fine, it's a lib).
- Produces:
  - `pub trait Invoker: Send + Sync { async fn gift(&self, req: FulfillRequest) -> Result<FulfillResponse, String>; }`
    (use `#[async_trait::async_trait]` or RPITIT — pick what compiles clean on stable; implementers:
    `LambdaInvoker { client: aws_sdk_lambda::Client, fn_name: String }` (InvocationType
    RequestResponse, payload = serde_json of req) and test `MockInvoker`).
  - `pub fn router(store: Store, invoker: Arc<dyn Invoker>) -> axum::Router`
  - Routes (JSON):
    - `GET /api/l/:token` → 200 `{ "label": .., "claims_allowed": .., "claims_used": ..,
      "active": bool, "games": [ { "id", "title", "bundle", "key_type", "artwork_url" } ],
      "claims": [ { "game_id", "title"?, "state", "gift_url" } ] }`.
      Unknown token → 404 `{ "error": "unknown link" }` (indistinguishable from any other bad
      token). Revoked/expired/exhausted → 200 with `active: false` and `games: []` but claims
      history intact (spec §7). Games list = `list_listable_games` (title-sorted comes free from
      the GSI sort key).
    - `POST /api/l/:token/claim` body `{ "game_id": ".." }` →
      1. get_link → 404 if none; `link.can_claim(now)` → 409 `{ "error": "..." }` friendly per
         ClaimRefusal variant.
      2. `claim_id = uuid::Uuid::new_v4().to_string()`; `store.claim_game(...)` → map
         ClaimTxError: GameUnavailable → 409 "someone beat you to it"; LinkNotClaimable → 409
         "no claims left on this link"; DuplicateClaim → 500 (should be impossible with uuid);
         Store → 500 "try again".
      3. read the game (for gamekey/machine_name/keyindex) → `invoker.gift(...)` →
         - `GiftUrl { url }` → 200 `{ "gift_url": url }`
         - `AlreadyRedeemed` → 410 `{ "error": "that key was already redeemed on humble — pick another" }`
         - `Parked { .. }` → 202 `{ "status": "processing", "message": "your claim is recorded — the gift link is taking longer than usual; check back on this page" }`
         - `Error { .. }` → 202 same body (claim intake succeeded; fate owned by reconcile/ben —
           NEVER surface internal messages)
         - invoker transport error → 202 same body (the claim transaction landed; the gift attempt
           may or may not have started → park semantics, reconcile owns it).
    - anything else → 404.
  - main.rs: lambda_http glue — env TABLE_NAME + FULFILLMENT_FN, aws_config once, LambdaInvoker,
    `lambda_http::run(router(...))`. <60 lines.

**Tests** (`api_test.rs`): axum `tower::ServiceExt::oneshot` against `router(store, mock)` with
store_or_skip (skips locally, runs in CI):
- unknown token → 404; revoked link → active:false, claims visible, games empty.
- happy claim: seed game+link, MockInvoker returning GiftUrl → 200 with gift_url; game gone from a
  second GET's games list; claim appears in claims with gift_url.
- race loser: claim the same game twice (second via fresh link) → 409.
- parked: MockInvoker → Parked → 202 processing body; claim still pending in claims list.
- MockInvoker records the FulfillRequest it received — assert keyindex/gamekey/machine_name came
  from the seeded game.

- [ ] Steps: manifest + failing tests → red → implement → green (workspace + clippy `--all-features` + fmt) → commit
  `git add Cargo.toml crates/public-api && git commit -S -m "feat(public-api): friend surface — link view + claim flow with park semantics"`

---

### Task 7: admin-api — ben surface

**Files:**
- Create: `crates/admin-api/Cargo.toml`, `crates/admin-api/src/lib.rs`,
  `crates/admin-api/src/main.rs`, `crates/admin-api/tests/api_test.rs`
- Modify: root `Cargo.toml`

**Interfaces:**
- Consumes: Store (games via a full-catalog read — ADD `store.list_all_games()` here if plan-1
  lacks it: Query is impossible on scattered pks, use a paginated Scan filtered `begins_with(pk, "GAME#")`
  — doc-comment the scan-is-fine-at-this-scale rationale, mirror single-page convention BUT
  paginate this one fully (admin needs the whole catalog; loop on last_evaluated_key)), links CRUD
  (create_link/update_link_meta/get_link/claims_for_link), sessions (Task 3), SyncState,
  fulfillment types, Invoker trait (reuse public-api's? NO — duplicate the minimal trait locally
  to avoid an api→api dependency; same shape).
- Produces `pub fn router(store: Store, invoker: Arc<dyn AdminInvoker>, ssm: SsmPutter, admin_hash: String) -> Router`
  where `SsmPutter` is a small trait (`put_cookie(&str)`) with real-SSM and mock impls, and
  `admin_hash` is the argon2 PHC string loaded from SSM at startup.
  - `POST /admin/api/login` `{ "password": ".." }` → verify with `argon2::Argon2::default()` +
    `PasswordHash::new(&admin_hash)`; ok → create session (token = uuid v4 twice concatenated,
    expires now+7d), `Set-Cookie: session=<token>; HttpOnly; Secure; SameSite=Strict; Path=/admin`,
    200. Wrong → 401 + 500ms tokio sleep (cheap throttle).
  - Session middleware for everything else under `/admin/api/*`: cookie → get_session → expiry
    check → 401 if absent/expired.
  - `GET /admin/api/catalog` → all games (full fields incl hidden/status/claim_id).
  - `POST /admin/api/games/:id/hidden` `{ "hidden": bool }` → read game → set → put_game (safe
    here: admin owns hidden; doc-comment why put_game is acceptable — it's not the sync path;
    accepted small race vs sync cosmetics).
  - `POST /admin/api/links` `{ "label", "claims_allowed", "expires_days"? }` → token =
    uuid-v4-no-hyphens ×2 (≥128 bits), create_link → 200 `{ "token", "url_path": "/l/<token>" }`.
  - `GET /admin/api/links` → NEEDS `store.list_links()` — add alongside list_all_games (Scan
    `begins_with(pk, "LINK#")`, full pagination) → links + per-link claims_used/allowed.
  - `POST /admin/api/links/:token/revoke` → get_link → revoked=true → update_link_meta.
  - `GET /admin/api/links/:token/claims` → claims_for_link.
  - `POST /admin/api/cookie` `{ "cookie": ".." }` → ssm.put_cookie (SecureString overwrite) →
    invoker ValidateCookie → 200 `{ "ok": bool }`. The cookie value never logged/echoed.
  - `POST /admin/api/sync` → invoker Sync (RequestResponse is fine — admin waits; lambda timeout
    is the apigw concern in plan 4; doc-comment) → SyncDone passthrough.
  - `GET /admin/api/status` → SyncState + counts (games by status from list_all_games).
  - main.rs: env TABLE_NAME/FULFILLMENT_FN/ADMIN_HASH_PARAM/HUMBLE_COOKIE_PARAM; load admin hash
    from SSM at startup; real SsmPutter + AdminInvoker; lambda_http::run.

**Tests**: mock invoker + mock ssm + store_or_skip; hash a known password with argon2 in the test
to build `admin_hash`:
- login wrong password → 401; right → Set-Cookie, subsequent authed call 200; no cookie → 401.
- create link → token ≥ 32 chars, get catalog, toggle hidden → reflected, revoke link →
  update_link_meta result visible via get_link.
- cookie paste → mock ssm records value, mock invoker returned CookieStatus ok.

- [ ] Steps: manifest + failing tests → red → implement (incl. the two Scan-based store additions
  committed as part of this task, in crates/dynamo, with their own store_test coverage:
  `list_all_games` ≥2 games incl a non-listable one; `list_links` ≥2) → green → commit
  `git add Cargo.toml crates/admin-api crates/dynamo && git commit -S -m "feat(admin-api): ben surface — argon2 login, links CRUD, hidden toggles, cookie paste, sync-now"`

---

### Task 8: final whole-branch review + PR

- [ ] Generate the branch review package (merge-base main), dispatch the final code reviewer on
  the most capable model with: the spec, this plan, the deferred-minors ledger, and the plan-1
  final-review carry list. Fix wave for Critical/Important. Controller eyeballs any cheap-model
  fix on paths local tests can't exercise (the plan-1 lesson).
- [ ] `gh pr create` on branch `kitten/plan2-lambdas`, body links this plan; CI green (remember:
  CI is the only dynamo receipt); mark ready; ben reviews + merges.

---

## Self-Review (done at write time)

1. **Spec coverage (plan-2 scope):** claim flow full-magic incl. park semantics (§5 — Tasks 4+6);
   sync + reconcile + humble-truth self-healing (§6, §9 — Task 4); cookie paste + validation +
   death ping (§8, §9 — Tasks 4/5/7); admin surface complete per §8 (Task 7 — badges data comes
   from status/giftable/hidden fields; UI renders in plan 3); friend token-state behavior (§7 —
   Task 6); no-enumeration 404 (§7 — Task 6); trust boundary (§3 — public-api never sees the
   cookie; only fulfillment's role reads the SSM param — IAM enforced in plan 4, code-shaped here).
   Deferred BY DESIGN: choice ingestion (fast-follow), frontend (plan 3), terraform/IAM/TTL (plan 4),
   artwork perfection (heuristic only), gift-redemption-state badges (spec §13 nice-to-have).
2. **Placeholder scan:** none — every task carries exact signatures, routes, policies, and test
   bodies or their exhaustive intent.
3. **Type consistency:** FulfillRequest/Response shared via the fulfillment crate (Tasks 4/5/6/7);
   `SyncWrite` naming consistent (Task 3 defines, Task 4 consumes); `keyindex: u32` everywhere
   (Tasks 1/2/6); `create_link`/`update_link_meta` (current post-review API) used, never `put_link`
   except the documented admin hidden-toggle `put_game` case.
