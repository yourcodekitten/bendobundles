# Task 6 Report — `SelfClaim` bundle path, `reveal_decision`, recover-then-record on `AlreadyRedeemed`

**Branch:** `kitten/self-claim`  
**Commit:** `1121d29` (GPG-signed, `code kitten <yourcodekitten@gmail.com>`, key `F2060B93112D9ACF`)  
**Date:** 2026-07-06

---

## TDD Evidence

### RED (compile error before implementation)

Attempting to compile the tests before adding the new types/functions produced:

```
error[E0425]: cannot find function `reveal_decision` in crate `fulfillment`
error[E0560]: struct `KeyEntry` has no field named `redeemed_key_val`
error[E0431]: `RevealedKey` is not a variant of `FulfillRequest`
```

Tests were written first, compilation failed. Implementation was added to make them green.

### GREEN (all 44 tests pass in one run)

```
running 44 tests
...
test reveal_decision_ladder_matches_gift_decision ... ok
test revealed_key_value_never_appears_in_logs_or_pings ... ok
test self_claim_already_redeemed_recovers_key_from_order ... ok
test self_claim_already_redeemed_with_no_key_val_parks ... ok
test self_claim_ambiguous_failure_parks_never_compensates ... ok
test self_claim_bundle_reveals_and_records ... ok

test result: ok. 44 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 11.85s
```

---

## Files Changed

| File | What changed |
|------|-------------|
| `crates/humble-client/src/lib.rs` | Added `pub redeemed_key_val: Option<String>` to `KeyEntry`, threaded through `order()` mapping |
| `crates/dynamo/src/lib.rs` | Added `#[derive(Clone)]` to `Store` (test pattern: keep ref after passing to `deps()`) |
| `crates/fulfillment/src/lib.rs` | `FulfillRequest::SelfClaim`, `FulfillResponse::RevealedKey`, `gift_error_decision` extraction, `reveal_decision`, `handle_self_claim_choice` stub, `handle_self_claim`, `record_revealed_key`, `recover_already_redeemed_key`, internal test helper updated |
| `crates/fulfillment/tests/handler_test.rs` | 6 new tests, supporting helpers (`seed_available_game`, `mount_reveal_success`, `mount_reveal_already_redeemed`, `mount_order_with_redeemed_tpk`, `mount_order_with_redeemed_tpk_no_val`, `mount_reveal_500`, `CaptureBuf` MakeWriter, `self_claim_req`), PID-scoped table names to prevent moto cross-run `DuplicateClaim` |

---

## Test Descriptions (6 new)

1. **`self_claim_bundle_reveals_and_records`** — happy path: `reveal_key` succeeds, key recorded in store, game flips to `BenRedeemed`, reveal POST has no `gift=` param.
2. **`self_claim_already_redeemed_recovers_key_from_order`** — `AlreadyRedeemed` response triggers order re-read; recovered `redeemed_key_val` is recorded (NOT compensated).
3. **`self_claim_already_redeemed_with_no_key_val_parks`** — `AlreadyRedeemed` but order has no `redeemed_key_val` → parked + pinged, claim stays Pending.
4. **`self_claim_ambiguous_failure_parks_never_compensates`** — 500 from reveal → park; claim stays Pending; no compensate.
5. **`revealed_key_value_never_appears_in_logs_or_pings`** — captures tracing output via `CaptureBuf` MakeWriter; asserts the actual key string "AAAA-BBBB-CCCC" never appears in logs or ping payloads on either the happy or recover path.
6. **`reveal_decision_ladder_matches_gift_decision`** — pure unit test (no I/O); uses a `check_agree!` macro to assert `reveal_decision(&Err(e))` == `gift_decision(&Err(e))` for every constructable `HumbleError` variant.

---

## Self-Review

### Exhaustive arms, no `_`
`gift_error_decision` has an explicit arm for every `HumbleError` variant with no `_` catch-all. The compiler enforces this. Both `gift_decision` and `reveal_decision` delegate to it, so future variants must be handled in one place.

### `AlreadyRedeemed` → recover, never compensate
The `Decision::Compensate` arm in `handle_self_claim` calls `recover_already_redeemed_key`, not any compensation path. This is correct: the key already belongs to Ben (he redeemed it); we recover the value from the order and record it. Compensation (re-listing the game) would be wrong.

### Key value never in logs or pings
- `handle_self_claim` logs `claim_id`, `game_id`, `machine_name`, `keyindex` — not the key value.
- `record_revealed_key` on store failure pings a message that mentions `claim_id` but not the key.
- `recover_already_redeemed_key` on no-val pings mention the claim/machine but not the key.
- Test 5 (`revealed_key_value_never_appears_in_logs_or_pings`) asserts this mechanically.

### Moto cross-run isolation
`store_or_skip` now uses `format!("t-fulfill-{}-{test}", std::process::id())` so each `cargo test` invocation gets a fresh set of tables, preventing `DuplicateClaim` failures from prior-run state. Within a single `cargo test` run, all tests share the same PID prefix but have unique `test` suffixes.

### `gift_error_decision` extraction is behavior-preserving
The extracted function is a direct lift of the `Err(err) => match err { ... }` arm from the original `gift_decision`. No behavior changed; the test `reveal_decision_ladder_matches_gift_decision` confirms both decision functions agree on every error variant.

---

## Concerns / Notes

- **Task 7 stub**: `handle_self_claim_choice` returns `parked_choice("choice-self-claim-not-built")`. Task 7 replaces it. No functional path currently exercises it for real.
- **`redeemed_key_val` in humble client**: This field is `None` when the tpk has not been redeemed (expected). The field is only populated when the humble wire response includes a redeemed key value. The `recover` path relies on this being `Some` when `AlreadyRedeemed` was triggered — which is the expected humble behavior (you can re-read an order to get the key you already redeemed).
