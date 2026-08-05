# out-of-band redemption (#158) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** the sync detects keys redeemed out of band: reconcile routes stuck claims by their immutable snapshot (completing ben's MLU claim autonomously), a shelf-truth audit de-lists any listable row whose key humble marks revealed/expired, and every remaining park says exactly what it knows.

**Architecture:** four small changes to existing flows, no new subsystems: (1) reconcile's choice-vs-bundle discriminator widens to the claim's `choice_pre_tpks`; (2) `reconcile_choice_claim`'s arms are re-gated — B1 parks on every route, arm A diffs against the empty baseline and parks on anything-found; (3) `run_sync` grows a final shelf-truth audit pass fed by an order-truth map built during the order walk, with the shared catalog scan hoisted out of the steam gate; (4) `heal_pairs::pair_verdict` accepts audited siblings on order-confirmed evidence. Spec (family-approved, read it first): `docs/superpowers/specs/2026-08-05-out-of-band-redemption-design.md`.

**Tech Stack:** Rust workspace (crates: `humble-client`, `domain`, `fulfillment`, `dynamo`), wiremock + DynamoDB-Local integration tests in `crates/fulfillment/tests/handler_test.rs`.

## Global Constraints

- Box is 4GB: `export CARGO_BUILD_JOBS=1` before any cargo command.
- Store/handler tests need DynamoDB-Local: `export DYNAMODB_LOCAL_URL=http://localhost:8155` (moto_server on :8155; if the port is dead the tests panic loudly — that is by design, start it: `moto_server -p 8155 &`).
- CI clippy is `cargo clippy --workspace --all-targets --all-features -- -D warnings` — run before every commit.
- **A skipped red is not a red:** `store_or_skip` (handler_test.rs:81-101) silently SKIPs when `DYNAMODB_LOCAL_URL` is unset — after every "Expected: FAIL/PASS" step, confirm the test actually RAN (no `SKIP`/`0 passed; 0 failed` output).
- All commits GPG-signed (`git commit -S`), authored `code kitten <yourcodekitten@gmail.com>` (verify `git config user.email` in a fresh worktree before the first commit).
- NEVER log/ping a key value or cookie. Pings carry claim ids, titles, machine_names only.
- No auto-fail, no new auto-compensate paths. Autonomous claim WRITES are a `Some`-snapshot-only privilege (spec: "`Some` is a receipt and `None` is a shrug").
- The audit de-lists; it never deletes (`Store::delete_game` stays `#[cfg(feature = "heal")]`).
- Branch: `158-out-of-band-redemption` off `main`. Base commit for spec references: `5c789ff`.

---

### Task 1: wire — `is_gift: Option<bool>` on `TpkWire` and `KeyEntry`

**Files:**
- Modify: `crates/humble-client/src/model.rs:35-48` (`TpkWire`)
- Modify: `crates/humble-client/src/lib.rs:280-296` (`KeyEntry`), `crates/humble-client/src/lib.rs:670-680` (mapping in `order()`)
- Modify: `crates/fulfillment/src/lib.rs:~4277` (unit-test `KeyEntry` literal helper gains the field)
- Test: `crates/humble-client/tests/client_test.rs`

**Interfaces:**
- Produces: `KeyEntry.is_gift: Option<bool>` — `None` = humble didn't send the field; consumed by Task 6's audit ping and nothing else. NO gate may consult it (spec prong 4: detection-only).

- [ ] **Step 1: Write the failing test.** `TpkWire` is `pub(crate)` — integration tests see only the public API, so pin the three states through `HumbleClient::order()` with wiremock (model on `parses_order_key_states` and its `fixture()` helpers at client_test.rs:5-15):

```rust
#[tokio::test]
async fn is_gift_models_absence_distinctly() {
    // present-true, present-false, absent — three distinguishable states on KeyEntry.
    // Mount an order whose tpkd_dict.all_tpks carries three tpks:
    //   {"machine_name":"a_row_choice_steam","human_name":"A","key_type":"steam","is_gift":true}
    //   {"machine_name":"b_row_choice_steam","human_name":"B","key_type":"steam","is_gift":false}
    //   {"machine_name":"c_row_choice_steam","human_name":"C","key_type":"steam"}
    // (inline serde_json::json! body, same mount shape as the existing order fixtures)
    let order = client.order("GK").await.unwrap();
    assert_eq!(order.keys[0].is_gift, Some(true));
    assert_eq!(order.keys[1].is_gift, Some(false));
    assert_eq!(order.keys[2].is_gift, None, "absence must stay None — never default to false");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `CARGO_BUILD_JOBS=1 cargo test -p humble-client --test client_test is_gift_models_absence_distinctly`
Expected: FAIL — no field `is_gift` on `KeyEntry` (compile error naming KeyEntry, NOT TpkWire).

- [ ] **Step 3: Implement.** In `model.rs` add to `TpkWire` (with the other `#[serde(default)]` fields): `#[serde(default)] pub is_gift: Option<bool>,`. In `lib.rs` add `pub is_gift: Option<bool>,` to `KeyEntry` and `is_gift: t.is_gift,` to the mapping in `order()` (:670-680). Fix every `KeyEntry { .. }` struct literal that now misses the field (the compiler will list them; known: the `key()` helper inside `find_new_tpk_diff_and_disambiguation` at fulfillment/src/lib.rs:~4277 — add `is_gift: None`).

- [ ] **Step 4: Run tests + clippy**

Run: `CARGO_BUILD_JOBS=1 cargo test -p humble-client --test client_test && CARGO_BUILD_JOBS=1 cargo clippy --workspace --all-targets --all-features -- -D warnings`
Expected: PASS, no warnings. (If clippy flags `is_gift` as dead code, it's consumed in Task 6 — silence is NOT allowed; reorder your commit to land with Task 6 or reference it in the audit ping now per Task 6's text.)

- [ ] **Step 5: Commit**

```bash
git add crates/humble-client crates/fulfillment/src/lib.rs
git commit -S -m "wire(#158): model is_gift as Option<bool> — absence stays None, detection-only"
```

---

### Task 2: reconcile discriminator — route by the claim's snapshot

**Files:**
- Modify: `crates/fulfillment/src/lib.rs:3876-3888` (the choice-branch condition + its comment)
- Test: `crates/fulfillment/tests/handler_test.rs`

**Interfaces:**
- Consumes: `claim.choice_pre_tpks: Option<Vec<String>>` (domain), existing `reconcile_choice_claim`.
- Produces: claims with `choice_pre_tpks: Some(_)` reach `reconcile_choice_claim` even when `game.requires_choice == false`. Bundle claims (`None`) are untouched.

- [ ] **Step 1: Write the failing test.** Model on `reconcile_self_choice_b2_reveals_never_chooses` (handler_test.rs:4492) for harness shape — seed a SELF claim with `choice_pre_tpks: Some(vec![])`, but the game row with `requires_choice: false` (the D7-flipped shape), order carrying one tpk with `redeemed_key_val` set (use `mount_order_with_key`, :3656). Today this claim takes the bundle path and (name mismatch) parks unreconcilable; after the fix it must reach B3 and fulfill:

```rust
#[tokio::test]
async fn reconcile_routes_by_snapshot_when_flip_already_happened() {
    // claim born choice (snapshot Some), row since flipped requires_choice=false by D7,
    // humble later shows the tpk revealed → must route choice → B3 → autonomous recovery.
    // Seed: game id "GK:mlu" (offered name), requires_choice false, status Pending, claim_id set;
    // claim: SELF, game_id "GK:mlu", choice_pre_tpks Some(vec![]), state Pending, aged > 15min;
    // order GK: one tpk "mlu_row_choice_steam" / title matching game.title, redeemed_key_val set.
    // Assert after run_sync: claim state == Fulfilled, recovered key recorded on the claim.
}
```

Write it fully using this file's existing seeding helpers (`sync_deps`, `mount_order_with_redeemed_tpk` at :3657 — note the exact name, there is no `mount_order_with_key`, and the claim/game seeding used by :4447's test); assert `store.get_claim(...).state == ClaimState::Fulfilled`. (The spec's discriminator pins "snapshot-`None` still routes bundle" and "`requires_choice: true` + snapshot-`None` still routes choice" are held by the untouched existing `reconcile_*` tests — Step 4 running the full set IS those pins; no new tests needed for them.)

- [ ] **Step 2: Run test to verify it fails**

Run: `CARGO_BUILD_JOBS=1 cargo test -p fulfillment --test handler_test reconcile_routes_by_snapshot_when_flip_already_happened`
Expected: FAIL — claim still Pending (bundle path parked it).

- [ ] **Step 3: Implement.** At :3881 the condition

```rust
if let Ok(Some(game)) = deps.store.get_game(&claim.game_id).await
    && game.requires_choice
```

becomes

```rust
if let Ok(Some(game)) = deps.store.get_game(&claim.game_id).await
    && (game.requires_choice || claim.choice_pre_tpks.is_some())
```

and the comment above it (:3876-3880) is rewritten to say: routing keys on the claim's own immutable snapshot FIRST — `requires_choice` is D7-mutable (flips false the moment a tpk appears) and stays in the OR only for legacy pre-snapshot choice claims; a transient game-read miss still falls through to the bundle path.

- [ ] **Step 3b: The B3 completion ping (the acceptance criterion's "pings for both" has no producer today).** `recover_already_redeemed_key`'s success is currently silent (`let _ = ...` at :1995 choice-path and :3911 bundle-path; `record_revealed_key`'s Ok arm returns quietly at :1802-1803). At BOTH call sites, match the result instead of discarding it, and on success ping (do NOT ping inside `record_revealed_key` — that path is shared with the ordinary self-claim happy path, which must stay quiet). Pinned copy:
  `"reconcile recovered the already-revealed key for self claim {id} ({title}) from the order — claim completed autonomously; the key was redeemed out of band{gift_flag}."`
  where `{gift_flag}` is `" (humble marks it a gift)"` when the recovered tpk's `is_gift == Some(true)`, else empty (this is prong 4's redeemed-arm mention; the tpk is in scope at the choice-path site, and at the bundle-path site via `key`). Extend Step 1's test to assert this ping text arrives on the webhook.

- [ ] **Step 4: Run the new test + the full reconcile set**

Run: `CARGO_BUILD_JOBS=1 cargo test -p fulfillment --test handler_test reconcile`
Expected: new test PASS; every existing `reconcile_*` test still green (widen-only).

- [ ] **Step 5: Commit**

```bash
git add crates/fulfillment
git commit -S -m "reconcile(#158): route choice claims by their immutable snapshot, not the D7-mutable flag"
```

---

### Task 3: B1 parks on every route

**Files:**
- Modify: `crates/fulfillment/src/lib.rs:1929-1943` (arm B1 in `reconcile_choice_claim`)
- Test: `crates/fulfillment/tests/handler_test.rs:2281` (`reconcile_choice_not_spent_compensates` — rename + invert)

**Interfaces:**
- Consumes: `ping(deps, msg)` (fulfillment:4121).
- Produces: B1 (`Some(pre)` + `TpkPick::None`) NEVER compensates; parks + pings. Exact ping text (pin it):
  `"choice claim {id} ({title}) has an intent snapshot but no new key on humble — NOT auto-compensating: a snapshot can hide a pick spent out of band. Left pending for review."`

- [ ] **Step 1: Rewrite the pin test.** Rename `reconcile_choice_not_spent_compensates` → `reconcile_choice_not_spent_parks_never_compensates`. Keep its seeding (snapshot `Some`, no new tpk in order — note `seed_pending_choice_claim` at :1659 already seeds `requires_choice: true`, so the original IS the unflipped half). Invert assertions: claim stays `Pending`, game row does NOT return to listable (no `gsi1pk`), webhook received the exact ping text above. Add a second variant for the FLIPPED half — spec: "on BOTH routing halves": after `seed_pending_choice_claim`, re-put the game row with `requires_choice: false` (read via `store.get_game`, mutate, write back through the store's normal put path) so the claim reaches B1 only through Task 2's snapshot widening; assert the identical park. (Depends on Task 2 — satisfied by ordering.)

- [ ] **Step 2: Run to verify both fail** (current code compensates)

Run: `CARGO_BUILD_JOBS=1 cargo test -p fulfillment --test handler_test reconcile_choice_not_spent`
Expected: FAIL — claim is Compensated, not Pending.

- [ ] **Step 3: Implement.** Replace B1's body (`TpkPick::None` arm inside `Some(pre) =>`, :1929-1943): delete the `compensate_any` call and its ping; insert `tracing::warn!` + the pinned ping; claim stays Pending (no store write at all). Update the arm's comment to the spec's reasoning: the re-choose backstop covers picks, not re-listing a revealed key; a `Some` snapshot can hide an out-of-band spend (in-snapshot tpk revealed after the claim).

- [ ] **Step 4: Run the reconcile set**

Run: `CARGO_BUILD_JOBS=1 cargo test -p fulfillment --test handler_test reconcile`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/fulfillment
git commit -S -m "reconcile(#158): B1 parks on every route — the re-choose backstop never covered re-listing a revealed key"
```

---

### Task 4: arm A looks before it acts (diff against empty)

**Files:**
- Modify: `crates/fulfillment/src/lib.rs:1906-1922` (arm A, the `None =>` match arm)
- Test: `crates/fulfillment/tests/handler_test.rs` (`reconcile_choice_no_snapshot_compensates` :2348 and `reconcile_self_choice_no_snapshot_compensates_via_self_variant` :4447 — keep, tighten; three new park tests)

**Interfaces:**
- Produces: arm A = compensate ONLY on verified-nothing **for this game**; anything title-matched → park + ping. **Deliberate resolution of a spec self-contradiction (flag at the plan gate):** the spec prescribes `find_new_tpk(order, &[], title)` in one sentence and "genuinely nothing in the order for this game" in the next — those conflict: `find_new_tpk` with one tpk returns `Unique` with NO title check (lib.rs:353-355), so an unrelated tpk elsewhere in the month's order would park a claim the spec's own words say should compensate (live counterexample: `reconcile_choice_no_snapshot_compensates` :2348 seeds one unrelated redeemed tpk). The intent the family signed is "for this game" — so arm A uses a title-scoped probe, NOT raw `find_new_tpk`:

```rust
let hits: Vec<&humble_client::KeyEntry> = order.keys.iter()
    .filter(|k| k.human_name.eq_ignore_ascii_case(&game.title))
    .collect();
// 0 hits → compensate (verified-nothing for this game); 1 → park (redeemed vs clean
// per hits[0].redeemed); ≥2 → park ambiguous.
```

  Exact ping texts (pin them):
  - unique, redeemed: `"choice claim {id} ({title}) has no intent snapshot, but humble shows a key for this title (`{machine_name}`) already revealed — a pick was spent outside the app's writes. Cannot attribute it to this claim (no snapshot = no arrival-order evidence). Left pending for review."`
  - unique, clean: `"choice claim {id} ({title}) has no intent snapshot, but humble shows an unredeemed key for this title (`{machine_name}`). Cannot attribute it to this claim. Left pending for review."`
  - ambiguous: `"choice claim {id} ({title}) has no intent snapshot and multiple keys on humble could match the title. Left pending for review."`

- [ ] **Step 1: Write the three failing park tests** (`reconcile_choice_no_snapshot_found_redeemed_parks`, `..._found_clean_parks`, `..._found_ambiguous_parks`): seed snapshot `None`, `requires_choice: true` row, order carrying (a) one title-matched redeemed tpk, (b) one title-matched clean tpk, (c) two tpks the title can't split. Assert: claim stays `Pending`, no re-list, exact ping text.

- [ ] **Step 2: Run to verify they fail** (current arm A compensates blind)

Run: `CARGO_BUILD_JOBS=1 cargo test -p fulfillment --test handler_test no_snapshot`
Expected: three FAIL (Compensated). The two existing no-snapshot tests MUST still pass unchanged: `:2348` seeds one *unrelated* (non-title-matched) tpk — under the title-scoped probe that is verified-nothing-for-this-game → still compensates; `:4447` mounts a genuinely empty order. If either fails, the probe is wrong — do not edit those tests to make it pass.

- [ ] **Step 3: Implement.** Arm A (`None =>` at :1910) becomes:

```rust
None => {
    let hits: Vec<&humble_client::KeyEntry> = order.keys.iter()
        .filter(|k| k.human_name.eq_ignore_ascii_case(&game.title))
        .collect();
    match hits.as_slice() {
        [] => {
            // verified-nothing FOR THIS GAME: zero title-matched tpks exist NOW, so
            // zero keys anyone could have revealed — compensate having actually
            // looked (was: blind). Title-scoped, not find_new_tpk: an unrelated tpk
            // elsewhere in the order must not park this claim.
            /* existing compensate_any + ping body moves here unchanged */
        }
        [tpk] => {
            // Absence of a measurement is not a measurement of absence: with no
            // snapshot the wire cannot date this tpk (nothing ordinal on TpkWire) —
            // attribution to THIS claim is unfounded. Park; the human attributes.
            // SELF included: auto-recovery is a Some-only privilege.
            /* warn! + pinned ping (redeemed vs clean per tpk.redeemed) */
        }
        _ => { /* warn! + pinned ambiguous ping */ }
    }
},
```

- [ ] **Step 4: Run the reconcile set**

Run: `CARGO_BUILD_JOBS=1 cargo test -p fulfillment --test handler_test reconcile`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/fulfillment
git commit -S -m "reconcile(#158): arm A diffs against the empty baseline — compensate on verified-nothing, park on anything-found"
```

---

### Task 5: hoist the catalog scan out of the steam gate

**Files:**
- Modify: `crates/fulfillment/src/lib.rs:3403-3418` (shared scan) and the two consumers directly below (title pass, ownership pass)
- Test: covered structurally by Task 6's `steam: None` audit test; here just keep the suite green.

**Interfaces:**
- Produces: `shared_scan: Option<Vec<Game>>` is `Some` whenever `list_all_games` succeeds — steam presence no longer decides it. **The title/ownership passes need NO change:** `map_missing_appids` (lib.rs:3096-3099) and `refresh_ben_ownership` (lib.rs:2197-2199) already early-return when `deps.steam` is `None` — they self-guard. Do not add guards, do not restructure the `shared_scan` rebinding flow (:3423-3437).

- [ ] **Step 1: Implement.** Replace :3408's `if deps.steam.is_some() { match ... } else { None }` with the plain `match deps.store.list_all_games().await { Ok(g) => Some(g), Err(e) => { warn!(...); None } }`, and update the comment: the scan feeds three consumers (title pass, ownership pass, shelf-truth audit); the audit's invariant must not inherit steam's off-switch; the two steam passes self-guard internally. That is the WHOLE change.

- [ ] **Step 2: Run the full handler suite** (no new test here; regressions are the risk)

Run: `CARGO_BUILD_JOBS=1 cargo test -p fulfillment --test handler_test`
Expected: all PASS — steamless tests must not suddenly run title/ownership passes.

- [ ] **Step 3: Commit**

```bash
git add crates/fulfillment
git commit -S -m "sync(#158): hoist catalog scan out of the steam gate — an every-sync invariant can't inherit a stranger's off-switch"
```

---

### Task 6: the shelf-truth audit pass

**Files:**
- Modify: `crates/fulfillment/src/lib.rs` — order walk (:3339-3401) builds the truth map; new `async fn shelf_truth_audit` called after `enrich_steam_apps` (:3459) and before the summary `msg` build (:3461); summary string gains the pull count.
- Test: `crates/fulfillment/tests/handler_test.rs`

**Interfaces:**
- Consumes: `shared_scan: Option<Vec<Game>>` (Task 5), `Game::is_listable()` (domain:280), `upsert_game_from_sync` (dynamo:2115), `ping` (:4121), `domain::sync_status` (domain:366), `KeyEntry.is_gift` (Task 1).
- Produces: `async fn shelf_truth_audit(deps: &Deps, scan: &[Game], truth: &TruthMap) -> u32` (returns pulls) where `type TruthMap = std::collections::HashMap<(String, String), TruthEntry>` and

```rust
/// Order-walk truth for one tpk, keyed by (gamekey, machine_name).
struct TruthEntry { redeemed: bool, expired: bool, key: humble_client::KeyEntry }
```

- Ping texts (pin): per-row `"shelf audit: pulled {title} ({id}) — key {reason} on humble"` with reason `"revealed outside the app"` / `"expired"` (append `" (humble marks it a gift)"` when `key.is_gift == Some(true)`); >3 pulls in one run collapse to ONE ping LISTING the rows (spec's words): `"shelf audit: pulled {n} listed games whose keys are spent on humble: {title1}, {title2}, …"` (titles comma-joined; ids in `warn!` logs).
- **CRITICAL — what the audit can and cannot see (shapes every test):** for a plain (non-choice-suffixed) tpk, the ORDER WALK ITSELF writes fresh truth to the same-id row minutes before the audit runs (`game_id(gamekey, machine_name)` :3349 → merge Available-arm → de-listed by the walk, audit pulls 0). **the only rows the audit can ever pull are ones the walk's write BYPASSES — the D7 frozen-sibling shape** (offered row exists → routing ladder diverts the write → the tpk-named sibling starves). every positive audit test therefore seeds a miniature MLU: an offered row (so D7 diverts), a listable sibling row under the tpk id, and a choice-suffixed tpk (`*_row_choice_steam`).
- Summary: when pulls > 0, `msg` becomes `"sync ok: {games_written} written, {orders_failed} order(s) failed, {pulls} audit-pulled"`.

- [ ] **Step 1: Write the fixture, then the failing tests.** First the fixture the existing helpers cannot express: `remount_order` (:3123) mounts CLEAN-only tpks (via `order_with_product_json` — no `redeemed_key_val`, no `is_expired` knob) and `order_json` (:907) toggles redeemed but is not a mount and can't set expired. Write:

```rust
/// Priority-1 remount of one gamekey whose tpks carry explicit redeemed/expired state.
/// tpks: (machine_name, human_name, redeemed, expired). Mind wiremock first-mounted-wins:
/// this mounts at higher priority than the sync_deps originals, same shape as remount_order.
async fn remount_order_with_states(
    humble: &MockServer, gamekey: &str, product: &str,
    tpks: &[(&str, &str, bool, bool)],
) { /* build the order body via serde_json::json! — each tpk gets
       "redeemed_key_val": "REDEEMED-KEY-VALUE" when redeemed, "is_expired": expired —
       and mount GET /api/v1/order/{gamekey} at priority 1. */ }
```

Then the tests (all through `run_sync` with `sync_deps` — `sync_deps` leaves `steam: None`, which IS the decoupling assertion lilith required; do not set a steam client in any of these; every positive case seeds the frozen-sibling shape per the CRITICAL note above):

```rust
#[tokio::test]
async fn audit_delists_frozen_sibling_when_order_shows_key_revealed() {
    // Seed: offered row "GK:mlu" (Available, any state that exists — its presence is what
    //   makes D7 divert) + sibling row "GK:mlu_row_choice_steam" Available+listable.
    // Order GK carries tpk "mlu_row_choice_steam" (choice-suffixed) — pass 1 clean
    //   (sibling stays listable: walk's write went to the offered row), then
    //   remount_order_with_states(.., &[("mlu_row_choice_steam", "MLU", true, false)])
    //   → run_sync → sibling: status BenRedeemed, gsi1pk ABSENT (raw get_item), pinned
    //   per-row ping fired, summary contains "1 audit-pulled". steam: None.
}
#[tokio::test]
async fn audit_never_touches_rows_absent_from_fetched_orders() {
    // a listable row whose gamekey is in failed_orders (fetch fails) → untouched:
    // still listable after run_sync, no audit ping. THE absence pin.
}
#[tokio::test]
async fn audit_delists_on_expired() { /* frozen-sibling shape, (.., false, true), reason "expired" */ }
#[tokio::test]
async fn audit_batches_pings_above_three() {
    // five frozen siblings (five offered+sibling pairs, one order), all revealed on
    // pass 2 → exactly ONE audit webhook ping, the batch text listing all five titles,
    // summary "5 audit-pulled".
}
```

(raw `aws_sdk_dynamodb` client against `DYNAMODB_LOCAL_URL` for the `gsi1pk` assertions, same shape the store tests use.) **Consciously struck from the spec's testing list:** "contested-write row de-lists on the following sync" — a mid-`upsert` race cannot be orchestrated deterministically in handler_test without test hooks; the guarded-write behavior is pinned in `store_test`'s `put_game_if_unchanged` tests and the audit's every-sync retry covers the outcome. Note this strike in the commit message.

- [ ] **Step 2: Run to verify all four fail**

Run: `CARGO_BUILD_JOBS=1 cargo test -p fulfillment --test handler_test audit_`
Expected: 4 FAIL (sibling stays listable — no audit exists). Confirm they RAN, not SKIPped.

- [ ] **Step 3: Implement.**
  1. Declare the map above the orders loop (:3291): `let mut truth: TruthMap = std::collections::HashMap::new();`. In the order walk (:3340), before the D7 routing, insert every key: `truth.insert((order.gamekey.clone(), key.machine_name.clone()), TruthEntry { redeemed: key.redeemed, expired: key.expired, key: key.clone() });` — the map is built from orders that FETCHED; a failed order contributes nothing (that is the absence rule, no special case).
  2. `shelf_truth_audit`: for each `g` in scan where `g.is_listable()`, look up `(g.gamekey.clone(), g.machine_name.clone())`; on hit with `entry.redeemed || entry.expired`, build the fresh row **from the existing `g`, updating ONLY the truth fields** — NOT the order-walk construction (:3369-3390 needs `order.bundle_name`/subproducts which `TruthEntry` doesn't carry, and `merge_sync`'s Available arm is fresh-wins on `bundle`/`artwork_url`/`title` (domain:456): a guessed empty value would WIPE real data on every pulled row):

```rust
let fresh = domain::Game {
    status: domain::sync_status(entry.redeemed, entry.expired),
    giftable: entry.key.giftable,
    key_type: entry.key.key_type.clone(),
    keyindex: entry.key.keyindex,
    steam_app_id: entry.key.steam_app_id.or(g.steam_app_id),
    appid_source: entry.key.steam_app_id.map(|_| domain::AppidSource::Humble).or(g.appid_source),
    ..g.clone() // id (own id — deliberately NOT the D7 routing ladder: correcting an
                // existing row, not minting), title, bundle, artwork_url, gamekey,
                // machine_name, hidden, hidden_source, claim_id, requires_choice,
                // owned_by_ben all carry from the row being corrected.
};
```

  then `upsert_game_from_sync(fresh)`. `SyncWrite` variants are `Written`/`SkippedInFlight`/`Unchanged` (dynamo:21-31 — there is NO `Contested`): `Written` counts as a pull; `SkippedInFlight`/`Unchanged` → skip silently (set-driven, next run self-corrects — comment it). Collect (title, id, reason, is_gift) per pull; ping per the pinned texts after the loop (batch rule at >3).
  3. Call site (after `enrich_steam_apps(...)`, before `msg` at :3461): `let pulls = match &shared_scan { Some(scan) => shelf_truth_audit(deps, scan, &truth).await, None => 0 };` and extend `msg` per the Interfaces pin.

- [ ] **Step 4: Run the audit tests + full suite + clippy**

Run: `CARGO_BUILD_JOBS=1 cargo test -p fulfillment --test handler_test && CARGO_BUILD_JOBS=1 cargo clippy --workspace --all-targets --all-features -- -D warnings`
Expected: all PASS, no warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/fulfillment
git commit -S -m "sync(#158): shelf-truth audit — no listable row may reference a key humble marks revealed or expired"
```

---

### Task 7: heal gate accepts audited siblings on order-confirmed evidence

**Files:**
- Modify: `crates/fulfillment/src/heal_pairs.rs:19-31` (`pair_verdict` signature + status gate) and its `mod tests`
- Modify: `crates/fulfillment/src/bin/heal_choice_pairs.rs:73` (caller passes `&[KeyEntry]` now)

**Interfaces:**
- Produces: `pub fn pair_verdict(sibling: &Game, offered: Option<&Game>, live_order_tpks: &[humble_client::KeyEntry]) -> Verdict`. Existing name-membership checks read `.machine_name` off the entries; the widened status gate reads `.redeemed` / `.expired`.
- Gate — **match key pinned: exact `entry.machine_name == sibling.machine_name` (the sibling IS the tpk row; grammar via `choice_tpk_matches` stays for the offered-row corroboration at :43-48, untouched), and the widened check runs INSIDE the status gate (the current :21 early-return), before the offered-row checks:** sibling `Available` → as today; sibling `BenRedeemed` → proceed only if the exact-matched live tpk has `redeemed == true`, else `Skip("status BenRedeemed but live order does not confirm redemption")`; sibling `Expired` → exact-matched tpk `expired == true` likewise; `Pending`/`Gifted` → `Skip` unchanged; `hidden` → `Skip` unchanged. Note: all 8 existing `mod tests` callers pass `&[String]` and must migrate to `KeyEntry` fixtures — the compiler forces this; it is expected, not a mistake.

- [ ] **Step 1: Write the failing unit tests** in `heal_pairs.rs` `mod tests` (model on `status_not_available_skips` :121): `benredeemed_sibling_heals_when_order_confirms`, `benredeemed_sibling_skips_when_order_does_not_confirm`, `expired_sibling_heals_when_order_confirms`, `gifted_sibling_still_skips`. Build `KeyEntry` fixtures inline (all fields, `is_gift: None`).

- [ ] **Step 2: Run to verify they fail** (signature change → compile fail first, that counts)

Run: `CARGO_BUILD_JOBS=1 cargo test -p fulfillment --features heal heal_pairs`
Expected: FAIL.

- [ ] **Step 3: Implement** the signature + gate change; fix the bin caller (it already fetches the live order — pass the entries instead of the collected names; delete the now-unused name-vec if nothing else reads it).

- [ ] **Step 4: Run heal tests + clippy with the feature**

Run: `CARGO_BUILD_JOBS=1 cargo test -p fulfillment --features heal && CARGO_BUILD_JOBS=1 cargo clippy --workspace --all-targets --all-features -- -D warnings`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/fulfillment
git commit -S -m "heal(#158): gate accepts audited siblings iff the live order confirms — evidence bar up, runbook step 4 unbroken"
```

---

### Task 8: the nag says what it knows

**Files:**
- Modify: `crates/fulfillment/src/lib.rs:3889-3900` (the bundle-path absent-from-order park)
- Test: `crates/fulfillment/tests/handler_test.rs`

**Interfaces:**
- Consumes: `domain::choice_tpk_bases` (domain:329) — messaging only, arms nothing.
- Produces: enriched `alert_unreconcilable` reason. Exact copy (pin): existing reason + (when a probe hits) `"; NOTE: humble carries a key for this game under `{tpk_machine_name}`{flags}"` where `{flags}` is `", already revealed outside the app"` and/or `", expired"` (both possible). No probe hit → byte-identical current reason (pin that too).

- [ ] **Step 1: Write the two failing message-pin tests**: `unreconcilable_nag_names_out_of_band_key_when_grammar_finds_one` (claim machine_name `mlu`, order tpk `mlu_row_choice_steam` revealed, snapshot `None`, `requires_choice: false` → bundle path → assert webhook text contains the NOTE with the tpk name and "already revealed outside the app") and `unreconcilable_nag_stays_generic_without_a_grammar_hit` (no matching tpk → assert exact current string, no NOTE substring).

- [ ] **Step 2: Run to verify the first fails**

Run: `CARGO_BUILD_JOBS=1 cargo test -p fulfillment --test handler_test unreconcilable_nag`
Expected: first FAIL (generic text), second may already pass (keep it as the pin).

- [ ] **Step 3: Implement** at the `else` site (:3889): before calling `alert_unreconcilable`, probe `order.keys` using the existing `domain::choice_tpk_matches` (already used by heal_pairs:45 — do not re-derive the probe from `choice_tpk_bases`) to find entries whose derived base equals `machine_name`, build the NOTE from the first hit's `redeemed`/`expired`, append to the reason string. `alert_unreconcilable` itself unchanged.

- [ ] **Step 4: Run the set**

Run: `CARGO_BUILD_JOBS=1 cargo test -p fulfillment --test handler_test unreconcilable_nag`
Expected: both PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/fulfillment
git commit -S -m "reconcile(#158): unreconcilable nag names the out-of-band key the grammar can see — messaging only, arms nothing"
```

---

### Task 9: the MLU end-to-end test — the acceptance criterion in miniature

**Files:**
- Test: `crates/fulfillment/tests/handler_test.rs` (one test + one seeding helper)

**Interfaces:**
- Consumes: everything above. Produces: the named known positive the deploy verifies against.

- [ ] **Step 1: Write the seeding helper** — prod's REAL legacy shape, version-less:

```rust
/// Seed a game item exactly as pre-#134 prod rows look: pk/sk/body/status/gsi1pk*, NO
/// `version` attribute. Bypasses Store deliberately (Store always stamps version) using a
/// raw aws_sdk_dynamodb client against DYNAMODB_LOCAL_URL — the deploy-day run hits the
/// adopt-at-1 guarded-write arm (dynamo:1970-1973) and so does this test.
async fn seed_legacy_game_item(table: &str, g: &domain::Game) { /* call the pub
    dynamo::schema::game_item(g, 1), then .remove("version") from the returned map, then
    raw put_item — zero schema drift forever, and the ONLY delta from prod's real writer
    is the missing version attribute, which is the point. */ }
```

- [ ] **Step 2: Write the failing end-to-end test:**

```rust
#[tokio::test]
async fn mlu_scenario_first_run_resolves_claim_and_delists_sibling() {
    // THE known positive (spec: acceptance criterion in miniature). steam: None.
    // Seed VERSION-LESS via seed_legacy_game_item:
    //   offered row  "GK:mlu": Pending, requires_choice false, claim_id set, giftable true
    //   sibling row  "GK:mlu_row_choice_steam": Available, giftable, unhidden (listable)
    // Seed claim — NOT via claim_game_self (its condition demands the row be Available,
    // and the offered row is raw-seeded already-Pending): build the domain::Claim literal
    // (link_token: domain::SELF_LINK_TOKEN.into(), state: Pending, created_at aged
    // > RECONCILE_STUCK_ALERT_AGE, choice_pre_tpks: Some(vec![]), failure_reason: None,
    // gift_url: None) and store.put_claim(&claim) — put_claim re-adds the gsi2 pending
    // marker for Pending state, which is what reconcile's list_pending_claims reads.
    // Order GK: exactly one tpk "mlu_row_choice_steam", title "My Little Universe"
    //   matching both rows' titles, redeemed_key_val = "REDEEMED-KEY-VALUE".
    // ONE run_sync. Assert:
    //   claim Fulfilled, recovered key == "REDEEMED-KEY-VALUE"
    //   sibling raw item: status ben_redeemed, NO gsi1pk, version == 1 (adopt-at-1)
    //   webhook saw BOTH pings (B3 recovery's + the audit's pinned text)
}
```

- [ ] **Step 3: Run to verify it fails, then passes** (it should pass already if Tasks 2-6 are correct — a first-try pass is only trustworthy because each component's test failed first; if it fails, the failure names the broken seam)

Run: `CARGO_BUILD_JOBS=1 cargo test -p fulfillment --test handler_test mlu_scenario`
Expected: PASS.

- [ ] **Step 4: Full workspace test + clippy**

Run: `CARGO_BUILD_JOBS=1 cargo test --workspace && CARGO_BUILD_JOBS=1 cargo clippy --workspace --all-targets --all-features -- -D warnings`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/fulfillment
git commit -S -m "test(#158): MLU end-to-end on prod's version-less shape — the acceptance criterion in miniature"
```

---

### Task 10: deploy-time re-enumeration gate + runbook

**Files:**
- Create: `scripts/158-deploy-changelist.sh` (read-only; adapted from the reviewed enumerator — source of truth: code-kitten `state/receipts/2026-08-05-158-deploy-day-changelist.json` documents the three clauses)
- Create: `docs/runbook-158-deploy.md`

**Interfaces:**
- Produces: the OMBB plan-gate line item — deploy halts unless the pre-press count is exactly `reroute=1 / armA=0 / audit=1`.

- [ ] **Step 1: Write the script** — profiles parameterized (`PROFILE_DB` default `kitten-debug` for table reads, `PROFILE_SSM` default `kitten-deploy` for the cookie), cookie never echoed, key values reduced to presence, explicit `if` blocks (no `[ ] && echo` tails — under `set -e` a false condition kills the run), final line `CHANGELIST: reroute=<n> armA=<n> audit=<n>`, exit 1 unless `1 0 1`. The three clauses, verbatim from the reviewed enumerator (receipt documenting the run: `/home/code-kitten/code-kitten/state/receipts/2026-08-05-158-deploy-day-changelist.json`):

```bash
T=brd-prod-ue1-bendobundles-table; R=us-east-1
SSM_PARAM=/brd-prod-ue1-bendobundles-param/humble-cookie
# clause (a) reroute set: pending claims (GSI "pending-claims", gsi2pk="PENDINGCLAIM")
#   with choice_pre_tpks != null, whose game row (pk "GAME#<game_id>", sk "META") has
#   body.requires_choice == false. Claims: body.S | fromjson | {id, game_id,
#   link_token, choice_pre_tpks}. Count members; also emit SELF vs gift split.
aws dynamodb query --profile "$PROFILE_DB" --region "$R" --table-name "$T" \
  --index-name pending-claims --key-condition-expression "gsi2pk = :p" \
  --expression-attribute-values '{":p":{"S":"PENDINGCLAIM"}}'
# clause (b) armA set: same pending claims with choice_pre_tpks == null AND their game
#   row requires_choice == true. Count.
# clause (c) audit pull list: LISTABLE rows (GSI "listable", gsi1pk="LISTABLE"),
#   body.S | fromjson | {id, gamekey, machine_name, title}; for each DISTINCT gamekey:
COOKIE=$(aws ssm get-parameter --profile "$PROFILE_SSM" --region "$R" \
  --name "$SSM_PARAM" --with-decryption --query Parameter.Value --output text)
curl -sfS "https://www.humblebundle.com/api/v1/order/$gk?all_tpkds=true" \
  -H "Cookie: _simpleauth_sess=$COOKIE"
#   → per listable row, exact match .tpkd_dict.all_tpks[] on machine_name; pull iff
#   (redeemed_key_val != null) or is_expired. Pace order fetches: sleep 0.3 between.
#   Track and print: rows_checked, would_pull, tpk_absent, order_fetch_fail — a fetch
#   failure makes the run INCONCLUSIVE for that order's rows: print it and exit 1
#   (an empty terminal is not a passing check).
```

- [ ] **Step 2: Write the runbook** — deploy sequence: (1) merge PR; (2) `terraform` deploy per `terraform/README.md` (boundary arn + ops_alarm_email mandatory, admin hash verbatim); (3) **run `scripts/158-deploy-changelist.sh` — on anything but `1 0 1`, STOP and take the fresh list to ben before any sync fires** (a count is a measurement, not a bound); (4) trigger/await the first sync; (5) verify: claim `3f46c058` Fulfilled, row `GAME#HAXSVMZHBvK2E7dW:mylittleuniverse_row_choice_steam` de-listed, both pings in discord; (6) ship-green only then.

- [ ] **Step 3: Shellcheck + dry-read**

Run: `shellcheck scripts/158-deploy-changelist.sh`
Expected: clean (or documented suppressions).

- [ ] **Step 4: Commit**

```bash
git add scripts/158-deploy-changelist.sh docs/runbook-158-deploy.md
git commit -S -m "deploy(#158): pre-press 1/0/1 re-enumeration gate + runbook — a count is a measurement, not a bound"
```

---

### Task 11: docs close-out

**Files:**
- Modify: `docs/superpowers/specs/2026-08-05-out-of-band-redemption-design.md:3` (status → `implemented (PR pending)`)
- Modify: `docs/runbook-choice-pair-heal.md` (one paragraph: the gate now accepts `BenRedeemed`/`Expired` siblings on order-confirmed evidence — audited rows stay healable; step 4's MLU instructions unchanged otherwise)

- [ ] **Step 1: Make both edits.**
- [ ] **Step 2: Commit**

```bash
git add docs/
git commit -S -m "docs(#158): spec status + heal runbook note for the widened gate"
```

---

## Self-Review (run before handing off)

1. **Spec coverage:** prong 1 → Tasks 2-4 (incl. the B3 completion ping, Step 3b — the acceptance criterion's second ping); prong 2 → Tasks 5-6 (decoupling, map, audit, pings, summary) + Task 7 (heal companion); prong 3 → Task 8; prong 4 → Task 1 (+ Task 6's ping mention + Task 2's gift_flag); MLU known positive + version-less shape + steam:None → Task 9 (audit tests in 6 also run steamless); deploy-time 1/0/1 → Task 10; non-goals hold (no D7/merge_sync/delete changes anywhere). **Two deliberate spec deviations, flagged for the plan gate:** (a) arm A uses a title-scoped probe, not `find_new_tpk(order, &[], title)` — the spec self-contradicted ("for this game" vs a function that ignores the title on single-tpk orders) and the family-signed INTENT is "for this game"; (b) the "contested-write" test is struck (not deterministically orchestrable in handler_test; pinned in store_test + every-sync retry).
2. **Ordering:** audit call site is after every writer in `run_sync` (spec's stated invariant) — Task 6 Step 3.3 pins it before the summary only, which is after reconcile/walk/passes.
3. **Type consistency:** `TruthEntry`/`TruthMap` defined once (Task 6); `pair_verdict` new signature used by bin caller (Task 7); `is_gift: Option<bool>` everywhere (Tasks 1, 6, 7 fixtures).
