# Review Round 1 Fixes — Evidence Log

## FIX 1 (Dynamo — Manual override guard dead code)
- **RED test**: `appid_source_is_top_level_attribute` — FAILED before schema change (attribute absent in raw DDB item)
- **GREEN after**: both `appid_source_is_top_level_attribute` and `set_game_steam_appid_if_unclaimed_after_admin_set_returns_skipped` pass
- **Changes**: `crates/dynamo/src/schema.rs` — added top-level `appid_source` attr (only when Some); `crates/dynamo/src/lib.rs` — fixed `:manual` from `"Manual"` to `"manual"` (snake_case)
- **Regression**: all 44 dynamo integration tests pass

## FIX 2 (Catalog — pick-cost swallowed by owned_by_ben)
- **RED test**: `armed confirm shows pick-cost when BOTH owned_by_ben and requires_choice are true (FIX 2)` — FAILED
- **GREEN after**: test passes
- **Changes**: `web/src/admin/Catalog.tsx` — ternary now checks owned_by_ben AND adminSteamId !== null; inner branch shows "you already own this on steam — spends 1 pick, sure?" when requires_choice is also true

## FIX 3 (Ops — error fragment swallowed silently)
- **RED test**: `shows error message when consumeReturnFragment returns verify_failed (FIX 3)` — FAILED
- **GREEN after**: both verify_failed and steam_unreachable error tests pass
- **Changes**: `web/src/admin/Ops.tsx` — added `'error' in fragment` branch before steamid branch; sets steamMsg with correct copy

## FIX 4 (Catalog — owned confirm not gated on steam identity)
- **RED test**: `armed confirm does NOT claim ownership when owned_by_ben=true but steam identity null (FIX 4)` — FAILED
- **GREEN after**: test passes (same Catalog.tsx change as FIX 2)
- **Changes**: Already covered by FIX 2 edit — owned_by_ben now gated on `adminSteamId !== null`

## FIX 5 (fulfillment normalize — double space from ™/® stripping)
- **RED test**: `title_pass_maps_title_with_trademark_symbol` — FAILED (game unmapped before fix)
- **GREEN after**: `title_pass_maps_title_with_trademark_symbol ... ok` (1 passed)
- **Changes**: `crates/fulfillment/src/lib.rs` — normalize now does `split_whitespace().collect::<Vec<_>>().join(" ")` instead of double `.trim()`

## FIX 6 (public-api — login half-flow when steam unconfigured)
- **RED test**: `steam_login_unconfigured_redirects_to_steam_unreachable_fragment` — FAILED
- **GREEN after**: `steam_login_unconfigured_redirects_to_steam_unreachable_fragment ... ok` (1 passed)
- **Changes**: `crates/public-api/src/lib.rs` — added `s.steam.is_none()` guard in `handle_steam_login` after ctx check

## FIX 7 (SteamError catch-all _ banned)
- **No new test** (behavior-preserving, compile-checked)
- **Changes**:
  - `crates/public-api/src/lib.rs` ~279: verify_openid_assertion Err arm — named all 6 non-OpenIdRejected variants
  - `crates/public-api/src/lib.rs` ~289: get_player_summary Err arm — named all 7 variants
  - `crates/public-api/src/lib.rs` ~411: owned_games proxy Err arm — named all 7 variants
  - `crates/admin-api/src/lib.rs` ~840: owned_games admin proxy Err arm — named all 7 variants
  - `crates/fulfillment/src/lib.rs` ~1869: get_app_list Err arm — expanded to name 6 remaining variants (RateLimited already named)
- **Cargo build**: all 4 crates compile with no `_` catch-alls on SteamError

## Round 2 fix

### Regression: error fragment early-return skips identity load (Ops.tsx mount useEffect)

- **Bug**: error branch (`'error' in fragment`) did `setSteamMsg(...)` then `return;`, skipping the `adminSteamIdentity()` fetch. `steamIdState` stayed `undefined` → component rendered "loading…" forever; connect button never appeared after a failed Steam login.
- **Fix**: removed `return;` from error branch — falls through to identity load so `steamIdState` resolves to `null` and connect button appears for retry. Success branch (`'steamid' in fragment`) behavior unchanged.
- **Test adjusted**: renamed + extended existing FIX3 verify_failed test to `shows error message AND connect button when consumeReturnFragment returns verify_failed (FIX 3 + recovery)` — asserts (a) error text renders, (b) `adminSteamIdentity` was called, (c) connect button appears.
- **RED**: `expect(adminSteamIdentity).toHaveBeenCalled()` failed (not called — early return). 1 failed / 23 passed.
- **GREEN after fix**: 24/24 passed. Full suite: 192/192. `npm run build` (tsc -b + vite): PASS.
- **Commit**: `fe58d90` — `fix(review-r2): Ops error fragment must still load identity so the connect button recovers`
- **Pushed**: `kitten/steam-integration` → `86800b5..fe58d90`

## Verification Gates

### Rust tests (with DYNAMODB_LOCAL_URL=http://localhost:8155)
- dynamo: **44 passed, 0 failed** (includes 2 new FIX 1 tests)
- fulfillment: **57 passed, 0 failed** (includes 1 new FIX 5 test)
- public-api: **24 passed, 0 failed** (includes 1 new FIX 6 test)
- admin-api: **43 passed, 0 failed**

### cargo fmt --check
- CLEAN (no diffs)

### cargo clippy --workspace --all-targets --all-features -- -D warnings
- Exit code 0, no warnings

### Web (npm run build + vitest)
- `npm run build` (tsc -b && vite build): PASS
- `npx vitest run`: **192 passed, 0 failed** (includes 4 new tests: FIX 2, FIX 3×2, FIX 4)
