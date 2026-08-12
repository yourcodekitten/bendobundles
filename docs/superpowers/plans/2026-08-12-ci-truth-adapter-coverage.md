# CI-Truth: Adapter Coverage + Complete Failure Census — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Put the `lambda_http` event-translation layer of both API crates under test with production-shaped (REST v1) fixtures, make CI report a complete failure census, and harden the two probable flake mechanisms behind #168.

**Architecture:** New dedicated test binaries (`tests/adapter_test.rs`) per API crate feed checked-in API-Gateway **v1** proxy-event JSON through `lambda_http::request::from_str` (request translation) and `lambda_http::Adapter::from(router)` (response translation), then into the same `pub fn router(...)` constructors the existing 109 tests use. The stage env flag `AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH` is set once per binary via `std::sync::Once` + edition-2024 `unsafe { set_var }`; an env-free sibling binary proves the flag is load-bearing. CI is the test oracle (this box cannot link test binaries — banked measurement, `cargo test` LINK ≥1638M); local gate is `cargo check`/`clippy` at `-j 1`.

**Tech Stack:** Rust edition 2024 · axum 0.8.9 · lambda_http 1.3.0 (default features: `apigw_rest` first in the deserializer, `pass_through` OFF) · tower 0.5.3 `ServiceExt::oneshot` · dynamodb-local (CI service container, `store_or_skip` idiom) · GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-08-12-ci-truth-adapter-coverage.md` (this plan implements it; the spec's D-numbers are referenced below).

## Global Constraints

- Commits are GPG-signed (`git commit -S`), author `code kitten <yourcodekitten@gmail.com>`. Never force-push. Space out pushes (throttle).
- **Family-review verdicts are FINAL and folded in** (spec rev 2): Q1 separate binaries ([env] can't unset); Q2 hidden seam accepted, both halves; Q3 #168 closes with the exact wording in Task 9 — no task branches on an open question.
- **One manifest change is permitted and required** (plan-review B1): `crates/dynamo/Cargo.toml` `[dependencies]` gains `tokio = { workspace = true, features = ["time"] }` (Task 2). No other Cargo.toml edit anywhere.
- The stage env flag is **PRESENCE-triggered** (`request.rs:408` `env::var(...).is_ok()` — `"false"` and `""` both activate). Controls that need it off need it ABSENT.
- **The box cannot link test binaries.** Local verification = `cargo check -p <crate> --tests -j 1` and `cargo clippy -p <crate> --all-targets -j 1 -- -D warnings`. Behavior verification = CI on the draft PR (CI triggers on `pull_request` plus `push` to main only — a feature-branch push alone runs nothing; the draft PR is what makes CI fire). "Run test, expect FAIL/PASS" steps below therefore name **which oracle** (check vs CI).
- `unsafe { std::env::set_var }` appears ONLY inside the `Once`-guarded `init()` of the two adapter binaries — never in `api_test.rs`, never mid-test (edition-2024 racy; spec D2).
- New store-backed tests use table prefixes **`t-pubadp-`** / **`t-admadp-`** — NOT `t-adm-`/`t-pub-` (adapter binaries run in parallel with `api_test.rs` binaries against the same dynamodb-local; a shared prefix + same test name would collide).
- Fixture JSON lives at `crates/<crate>/tests/fixtures/*.json`, loaded with `include_str!` (repo idiom).
- Existing `api_test.rs` files: do not modify except where Task 3 names exact lines. The 109 existing tests' isolation profile must not change (spec AC6).
- Branch: `kitten/ci-truth-adapter` on `yourcodekitten/bendobundles`. PR will carry `Fixes #185`, `Fixes #186`; #168 is closed manually with a verdict comment (Task 9).

## File Structure

- Modify `.github/workflows/ci.yml:36` — one line (Task 1).
- Modify `crates/dynamo/Cargo.toml` (+1 dependency line) and `crates/dynamo/src/lib.rs:2895-2948` — `create_table_for_tests` waiter (Task 2).
- Modify `crates/admin-api/tests/api_test.rs` — the two uuid-minting helpers and their callers (Task 3).
- Create `crates/public-api/tests/adapter_test.rs` + `crates/public-api/tests/fixtures/{apigw_v1_fallback.json, apigw_v1_link_unknown.json, apigw_v1_multi_value_query.json, apigw_v1_base64_thanks.json, apigw_v2_guard.json, alb_guard.json, apigw_v1_degenerate_missing_key.json, apigw_v1_degenerate_inconsistent.json}` (Task 4).
- Create `crates/public-api/tests/adapter_stage_control_test.rs` + `crates/public-api/tests/fixtures/{apigw_v1_no_stage.json, apigw_v1_default_stage.json}` (Task 5) and `crates/public-api/tests/adapter_stage_false_test.rs` (Task 5b).
- Create `crates/admin-api/tests/adapter_test.rs` + `crates/admin-api/tests/fixtures/{apigw_v1_note_with_cookie.json, apigw_v1_login.json}` (Tasks 6-7; the login fixture is created in Task 7).
- No other `Cargo.toml` changes: `lambda_http` is a normal dep of both API crates; `tower`, `serde_json`, `aws-config`, `async_trait` usage mirrors what each crate's api_test.rs already compiles with — Task 4/6 Step 3's `cargo check` is the verification that no new dev-dep was needed.

---

### Task 1: CI runs the complete failure census (#185)

**Files:**
- Modify: `.github/workflows/ci.yml:36`

**Interfaces:**
- Produces: CI behavior consumed by Task 8's sabotage census (a full red list instead of first-failure-stops).

- [ ] **Step 1: Edit the line**

At `.github/workflows/ci.yml:36`, change:

```yaml
      - run: cargo test --workspace
```

to:

```yaml
      # --no-fail-fast: a truncated failure census is indistinguishable from a complete
      # one (#185, paid for in #180 — the second broken crate compiled, never ran, and
      # its red later read as a fresh regression).
      - run: cargo test --workspace --no-fail-fast
```

- [ ] **Step 2: Verify the workflow still parses**

Run: `cd ~/bendobundles && python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/ci.yml')); print('YAML OK')"`
Expected: `YAML OK` (and nothing else changed: `git diff --stat` shows exactly 1 file, +4/−1).
Fallback if PyYAML is absent (`ModuleNotFoundError`): skip the local parse, verify by eye that the diff is the 4 lines above, and let CI's own workflow parser be the gate — a broken workflow file fails loudly on push, which is the failure direction we want.

- [ ] **Step 3: Commit**

```bash
cd ~/bendobundles && python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml')); print('YAML OK')" \
  && git add .github/workflows/ci.yml \
  && git commit -S -m "ci: --no-fail-fast so a two-crate breakage reports both crates (#185)"
```

---

### Task 2: `create_table_for_tests` waits for ACTIVE and drains DELETING (#168 hardening, mechanism a)

**Files:**
- Modify: `crates/dynamo/Cargo.toml` (add ONE `[dependencies]` line — permitted by Global Constraints)
- Modify: `crates/dynamo/src/lib.rs:2895-2948`
- Test: `crates/dynamo/tests/store_test.rs` (append one test)

**Interfaces:**
- Consumes: existing `Store { client, table }`, `StoreError::Aws`.
- Produces: same signature `pub async fn create_table_for_tests(&self) -> Result<(), StoreError>` — now returns only after the table + both GSIs are ACTIVE, and retries the create while a prior DELETE is draining. Every store-backed test in the workspace inherits this.

**Why:** the current body fire-and-forgets `delete_table`, then `create_table`, then returns immediately (no waiter). Under `cargo test --workspace` parallelism against ONE shared dynamodb-local, a create can land while the old table is DELETING (`ResourceInUseException`), and a query can land before a GSI is ready. This is the highest-probability mechanism behind #168's observed flakes (measured inventory: no `set_var`, no globals — the shared server is the only cross-test state).

- [ ] **Step 1: Write the failing-shape test**

Append to `crates/dynamo/tests/store_test.rs` (follow the file's existing `store_or_skip`-style guard — it uses a `t-` prefix table via its own helper at `:53`):

```rust
/// create_table_for_tests must be safe to call twice back-to-back: the second call's
/// delete→create races its own DELETING state on a real dynamodb-local, which is the
/// #168 candidate mechanism. Before the waiter existed this intermittently threw
/// ResourceInUseException; with the drain+waiter it must be deterministic.
#[tokio::test]
async fn create_table_for_tests_is_idempotent_under_immediate_recreate() {
    let Some(store) = store_or_skip("recreate-race").await else { return };
    // store_or_skip already created once. Slam it twice more, no pause.
    store.create_table_for_tests().await.expect("second create");
    store.create_table_for_tests().await.expect("third create");
    // And the GSIs must be queryable immediately after return.
    let games = store.list_listable_games().await.expect("GSI must be ACTIVE on return");
    assert!(games.is_empty(), "virgin table");
}
```

(Verified in plan review: store_test.rs's `store_or_skip` is at `:30` and returns `Option<Store>` — NOT `Option<Arc<Store>>` like the API crates' versions. The test body above is compatible with the bare `Store`; keep it as written.)

- [ ] **Step 1b: Add the tokio dependency (the waiter needs `tokio::time::sleep` in the lib build)**

dynamo has tokio only in `[dev-dependencies]`, and the workspace tokio lacks the `time`
feature — without this line, Step 3 cannot compile (plan-review B1). In
`crates/dynamo/Cargo.toml` `[dependencies]`, add:

```toml
tokio = { workspace = true, features = ["time"] }
```

(Explicit feature, not transitive unification — unification through another dep is an
accident waiting for a bump.)

- [ ] **Step 2: Local gate**

Run: `cd ~/bendobundles && cargo check -p dynamo --tests -j 1`
Expected: compiles. (Behavioral RED is not locally demonstrable — deterministic RED requires the race; the CI-side proof of value is the disappearance of the flake class plus the drain path exercised by this test. Say exactly that in the commit message; do not claim an observed RED.)

- [ ] **Step 3: Implement drain + waiter**

Replace the tail of `create_table_for_tests` (`crates/dynamo/src/lib.rs:2923-2948`) so the delete drains and the create waits:

```rust
        // NotFound is the virgin-container case (CI's fresh service, first local run).
        let _ = self
            .client
            .delete_table()
            .table_name(&self.table)
            .send()
            .await;
        // Drain: a delete on dynamodb-local is fast but NOT instantaneous; a create
        // that lands while the table is DELETING throws ResourceInUseException. Retry
        // the create through that window, bounded. (#168 mechanism a.)
        let create = || {
            self.client
                .create_table()
                .table_name(&self.table)
                .billing_mode(BillingMode::PayPerRequest)
                .attribute_definitions(attr("pk"))
                .attribute_definitions(attr("sk"))
                .attribute_definitions(attr("gsi1pk"))
                .attribute_definitions(attr("gsi1sk"))
                .attribute_definitions(attr("gsi2pk"))
                .attribute_definitions(attr("gsi2sk"))
                .key_schema(key("pk", KeyType::Hash))
                .key_schema(key("sk", KeyType::Range))
                .global_secondary_indexes(gsi(schema::GSI_LISTABLE, "gsi1pk", "gsi1sk"))
                .global_secondary_indexes(gsi(schema::GSI_PENDING, "gsi2pk", "gsi2sk"))
                .send()
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            match create().await {
                Ok(_) => break,
                Err(e) => {
                    let in_use = matches!(
                        e.as_service_error(),
                        Some(se) if se.is_resource_in_use_exception()
                    );
                    if in_use && std::time::Instant::now() < deadline {
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        continue;
                    }
                    return Err(StoreError::Aws(format!("{e:?}")));
                }
            }
        }
        // Wait for ACTIVE — table AND both GSIs. Returning before ACTIVE lets the
        // caller's first GSI query race table creation. Bounded: a table that isn't
        // ACTIVE in 30s is a broken environment, and hanging forever would hide it.
        loop {
            let desc = self
                .client
                .describe_table()
                .table_name(&self.table)
                .send()
                .await
                .map_err(|e| StoreError::Aws(format!("{e:?}")))?;
            let table = desc.table().ok_or_else(|| {
                StoreError::Aws("describe_table returned no table".into())
            })?;
            let table_active =
                table.table_status() == Some(&aws_sdk_dynamodb::types::TableStatus::Active);
            let gsis_active = table
                .global_secondary_indexes()
                .iter()
                .all(|g| g.index_status() == Some(&aws_sdk_dynamodb::types::IndexStatus::Active));
            if table_active && gsis_active {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(StoreError::Aws(format!(
                    "table {} not ACTIVE within 30s (status {:?})",
                    self.table,
                    table.table_status()
                )));
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
```

Notes for the implementer: `attr`/`key`/`gsi` closures already exist above this block — keep them. The exact service-error predicate name may differ by SDK version (`is_resource_in_use_exception` on the create-table error type); `cargo check` will name it — fix to what the SDK exposes, do NOT string-match on `format!("{e:?}")`. Also update the doc comment above the fn (`:2890-2894`) — it currently documents delete-tolerates-NotFound + create-propagates; extend it to describe the drain+waiter (keep what's still true).

- [ ] **Step 4: Local gate**

Run: `cd ~/bendobundles && cargo check -p dynamo --tests -j 1 && cargo clippy -p dynamo --all-targets -j 1 -- -D warnings`
Expected: clean. (CI proves behavior on the draft PR after Task 4 opens it.)

- [ ] **Step 5: Commit**

```bash
cd ~/bendobundles && cargo check -p dynamo --tests -j 1 \
  && git add crates/dynamo/Cargo.toml crates/dynamo/src/lib.rs crates/dynamo/tests/store_test.rs \
  && git commit -S -m "test-support: create_table_for_tests drains DELETING and waits for ACTIVE (#168 mechanism a)

The old body fire-and-forgot the delete and returned before the create was
ACTIVE. Under --workspace parallelism against one shared dynamodb-local that
is a live race (ResourceInUseException / query-before-GSI-ready) and the
strongest candidate for #168's undiagnosed flakes. No observed RED exists —
the race is nondeterministic by nature; this commit claims hardening, not a
reproduced diagnosis."
```

---

### Task 3: stop leaking uuid-named tables (#168 hardening, mechanism b)

**Files:**
- Modify: `crates/admin-api/tests/api_test.rs:1299` and `:1598` (exact lines may drift ±5; locate by the `sc{}`/`steam{}` format strings)

**Interfaces:** none — test-internal.

**Why:** two helpers mint uuid-derived table names (`sc<uuid[..10]>`, `steam<uuid[..8]>`) every run and never drop them. Against a persistent local dynamodb they accumulate forever (`ListTables` pressure on the shared instance — cross-test state). The stable-name convention (`t-adm-<test>` + delete-then-create) exists precisely to prevent this; these two are the only exceptions.

- [ ] **Step 1: Read the two helpers and enumerate their callers**

Run: `cd ~/bendobundles && grep -n 'uid\[\.\.' crates/admin-api/tests/api_test.rs` (hits at `:1298-1299` and `:1597-1598`) and `grep -n 'test_app_with_call_invoker\|steam_test_app' crates/admin-api/tests/api_test.rs`.
Verified in plan review: `test_app_with_call_invoker` has **13 callers** (OMBB's recount, 14 grep hits − 1 definition; the listed lines `:1409,:1440,:1456,:1467,:1486,:1509,:1535,:1996,:2022` are a sample, Step 1's grep derives the full list) and `steam_test_app` ~6; the two helpers use DIFFERENT error idioms (`test_app_with_call_invoker` uses `.await.expect(...)` and returns a tuple; `steam_test_app` uses `?`) — keep each helper's own idiom, change ONLY where the table name comes from.

- [ ] **Step 2: Replace with caller-supplied stable names**

Change each helper's signature to take `test: &str` as its FIRST parameter and use it in the
table name (`format!("sc-{test}")` / `format!("steam-{test}")`), deleting the uuid minting.
Then update every caller to pass **the enclosing test fn's name, verbatim, as a string
literal** (they are unique in the file — the same guarantee `store_or_skip("t-adm-{test}")`
already relies on). Rule for the edge case: if any single test calls the same helper twice,
suffix `-a`/`-b` at those two call sites (plan review found none, but the rule exists so the
executor doesn't improvise). Keep bodies otherwise untouched. Table names stay ≤255 chars,
charset `[a-zA-Z0-9_.-]` — test fn names satisfy both.

- [ ] **Step 3: Local gate + commit**

```bash
cd ~/bendobundles && cargo check -p admin-api --tests -j 1 \
  && git add crates/admin-api/tests/api_test.rs \
  && git commit -S -m "test-support: stable table names for the two uuid-minting helpers (#168 mechanism b)

uuid-per-run names accumulate forever on a persistent dynamodb-local; the
stable-name + delete-then-create convention already used everywhere else is
the fix, now applied to the last two exceptions."
```

---

### Task 4: public-api adapter tests — request-side translation (spec G1, D3, D4)

**Files:**
- Create: `crates/public-api/tests/adapter_test.rs`
- Create: `crates/public-api/tests/fixtures/apigw_v1_fallback.json`
- Create: `crates/public-api/tests/fixtures/apigw_v1_link_unknown.json`
- Create: `crates/public-api/tests/fixtures/apigw_v1_multi_value_query.json`
- Create: `crates/public-api/tests/fixtures/apigw_v1_base64_thanks.json`
- Create: `crates/public-api/tests/fixtures/apigw_v2_guard.json`

**Interfaces:**
- Consumes: `public_api::router(store: Arc<Store>, invoker: Arc<dyn Invoker>, steam: Option<Arc<SteamClient>>, base_url: String) -> Router` (`crates/public-api/src/lib.rs:162`); `lambda_http::request::from_str`; `lambda_http::RequestExt`; `Store::new` / `create_table_for_tests`.
- Produces: the fixture files and the `init_stage_env()` idiom Task 6 copies; the two-shape 404 assertion pair (route-matched `{"error":"unknown link"}` vs fallback `{"error":"not found"}`) Task 5 reuses.

**The two-shape 404 pair (load-bearing design):** `router()` has a `.fallback(handle_not_found)` returning `{"error":"not found"}` (`lib.rs:189-195`), while a *matched* link route with an unknown token returns `{"error":"unknown link"}` (`lib.rs:1082`). Path-derivation tests can therefore distinguish "the translated path matched the intended route" from "the translated path matched nothing" **by response body**, with no handler instrumentation.

- [ ] **Step 1: Write the fixtures**

`crates/public-api/tests/fixtures/apigw_v1_fallback.json` — a path no route matches (fallback proof, storeless):

```json
{
  "resource": "/api/{proxy+}",
  "path": "/api/definitely/no/such/route",
  "httpMethod": "GET",
  "headers": {
    "Host": "wt6mne2s9k.execute-api.us-east-1.amazonaws.com",
    "X-Forwarded-Proto": "https",
    "X-Forwarded-Port": "443"
  },
  "multiValueHeaders": {
    "Host": ["wt6mne2s9k.execute-api.us-east-1.amazonaws.com"]
  },
  "queryStringParameters": null,
  "multiValueQueryStringParameters": null,
  "pathParameters": { "proxy": "definitely/no/such/route" },
  "stageVariables": null,
  "requestContext": {
    "accountId": "123456789012",
    "resourceId": "abc123",
    "stage": "live",
    "requestId": "41b45ea3-70b5-11e6-b7bd-69b5aaebc7d9",
    "requestTimeEpoch": 1583798639428,
    "path": "/live/api/definitely/no/such/route",
    "identity": { "sourceIp": "203.0.113.10", "userAgent": "adapter-test" },
    "resourcePath": "/api/{proxy+}",
    "httpMethod": "GET",
    "apiId": "wt6mne2s9k"
  },
  "body": null,
  "isBase64Encoded": false
}
```

**⚠️ EVERY derived fixture must update `requestContext.path` to `"/live" + its new top-level
path` in the same edit** — the loader's D5-2 consistency assert makes any drift a panic (OMBB
step-5 B1: the creation steps must match the sabotage steps' care).

`apigw_v1_link_unknown.json` — same skeleton, with:
- `"path": "/api/l/definitely-not-a-token"`, `"pathParameters": { "proxy": "l/definitely-not-a-token" }`, `"requestContext.path": "/live/api/l/definitely-not-a-token"` (this mirrors the manual prod probe from #186's interim mitigation).

`apigw_v1_multi_value_query.json` — same skeleton, with:
- `"path": "/api/l/definitely-not-a-token"`, matching `pathParameters` AND `requestContext.path` = `/live/api/l/definitely-not-a-token`,
- `"queryStringParameters": { "a": "2", "b": "3" }`,
- `"multiValueQueryStringParameters": { "a": ["1", "2"], "b": ["3"] }`.

`apigw_v1_base64_thanks.json` — same skeleton, with:
- `"path": "/api/l/definitely-not-a-token/thanks"`, `"pathParameters": { "proxy": "l/definitely-not-a-token/thanks" }`, `"requestContext.path": "/live/api/l/definitely-not-a-token/thanks"`,
- `"httpMethod": "POST"` (both top-level and in `requestContext`),
- headers gain `"Content-Type": "application/json"` (and mirror it into `multiValueHeaders`),
- `"body": "eyJub3RlIjoidGhhbmsgeW91IGJlbiDinaQifQ=="` (base64 of `{"note":"thank you ben ❤"}` — non-ASCII on purpose: it proves byte-level decode, not just ASCII luck; ❤ is U+2764 → bytes `e2 9d a4` → `DinaQ` in the encoding, NOT `DinaE`/U+2761 — plan review caught the wrong constant here once already),
- `"isBase64Encoded": true`.

Verify the constant anyway (a check, not a repair): `python3 -c "import base64; print(base64.b64encode('{\"note\":\"thank you ben ❤\"}'.encode()).decode())"` must print exactly the value above; if it differs, STOP and reconcile before committing.

**Full-key-set requirement (spec D5, applies to EVERY apigw_v1_*.json above):** each fixture
must carry ALL of: `resource`, `path`, `httpMethod`, `headers`, `multiValueHeaders` (every
header mirrored), `queryStringParameters`, `multiValueQueryStringParameters` (both maps
populated together or both null-and-then-empty-maps — production sends both; when the
fixture has a query, BOTH maps carry it), `pathParameters`, `stageVariables`,
`requestContext` (with `accountId, resourceId, stage, requestId, requestTimeEpoch, identity,
resourcePath, httpMethod, apiId`, **and `path` = `"/" + stage + top-level path`**), `body`,
`isBase64Encoded`. The corpus file this skeleton descends from is a parser-exercise artifact
missing five of these — the loader predicate below is what makes "real-shaped" a machine
check instead of an adjective.

`apigw_v2_guard.json` — copy `~/.cargo/registry/src/*/lambda_http-1.3.0/tests/data/apigw_v2_proxy_request_minimal.json` verbatim, then set `"rawPath": "/api/l/x"`, `"routeKey": "$default"`. (It must keep parsing as v2 AND not be captured by the v1 arm — spec D4.)

`alb_guard.json` — copy `~/.cargo/registry/src/*/lambda_http-1.3.0/tests/data/alb_request.json` verbatim, set its path to `/api/l/x`. (The cascade has more arms than v1/v2; one ALB fixture pins the fall-through class — spec D4, OMBB Q4.)

**Degenerate fixtures — ONE PLANTED DEFECT PER COPY** (Lilith's required edit: a fixture
carrying both defect classes proves only the arm that panics first — the masked arm's
asserts could be deleted tomorrow and the suite stays green forever):
- `apigw_v1_degenerate_missing_key.json` — a COPY of `apigw_v1_fallback.json` with
  `multiValueHeaders` deleted, everything else untouched (key-set arm).
- `apigw_v1_degenerate_inconsistent.json` — a COPY of `apigw_v1_fallback.json`,
  **key-complete**, with `requestContext.path` set to `"/wrong/prefix"` (correlated-field
  arm — reachable only because every key is present).
Both are loaded ONLY by the predicate's own red tests, never by a translation test.

- [ ] **Step 2: Write `crates/public-api/tests/adapter_test.rs`**

```rust
//! Adapter-boundary tests: canned API-Gateway **v1 (REST, payload 1.0)** events through
//! the REAL lambda_http translation into the same `router()` the unit tests use.
//! v1 is what production sends — terraform/aws-apigateway.tf is a REST API; there is
//! no apigatewayv2 resource in the repo (spec 2026-08-12, correction 1).
//!
//! THIS BINARY IS SEPARATE from api_test.rs ON PURPOSE: production sets
//! AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH=true (terraform/aws-lambda.tf:89,:132) and
//! lambda_http reads it from process env per request. Edition-2024 set_var is unsafe
//! and racy against concurrent readers, so it is set exactly once, before any
//! translation, via the Once below — and no other binary inherits it (spec D2).
//! The env-free twin (adapter_stage_control_test.rs) proves the flag is load-bearing.
//!
//! Fixture provenance (spec D5): all apigw_v1_*.json derive from lambda_http 1.3.0's
//! shipped corpus (registry: lambda_http-1.3.0/tests/data/apigw_proxy_request.json and
//! siblings), adapted to this app's routes and the deployed shape: REST v1, stage
//! `live`, CloudFront origin_path /live (terraform/aws-cloudfront.tf:135-140).

use std::sync::Arc;
use std::sync::Once;

use lambda_http::RequestExt;
use public_api::router;
use tower::ServiceExt;

static STAGE_ENV: Once = Once::new();

/// Reproduce production's stage config. Every test calls this before any translation.
fn init_stage_env() {
    STAGE_ENV.call_once(|| {
        // SAFETY: called before any test in this binary performs env reads via
        // lambda_http; Once blocks racing callers until the write completes. No other
        // code in this binary writes env. (Global Constraints forbid set_var elsewhere.)
        unsafe { std::env::set_var("AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH", "true") }
    });
}

/// Same store idiom as api_test.rs::store_or_skip, but prefix `t-pubadp-` — this
/// binary runs in parallel with api_test.rs against the same dynamodb-local, and a
/// shared prefix + same test name would collide (Global Constraints).
async fn store_or_skip(test: &str) -> Option<Arc<dynamo::Store>> {
    let (url, explicit) = match std::env::var("DYNAMODB_LOCAL_URL") {
        Ok(v) => (v, true),
        Err(_) => ("http://localhost:8000".into(), false),
    };
    let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .endpoint_url(&url)
        .region("us-east-1")
        .test_credentials()
        .load()
        .await;
    let client = aws_sdk_dynamodb::Client::new(&config);
    if client.list_tables().send().await.is_err() {
        if explicit {
            panic!("DYNAMODB_LOCAL_URL is set but dynamodb-local is unreachable — refusing to skip");
        }
        eprintln!("SKIP {test}: no dynamodb-local at {url}");
        return None;
    }
    let store = dynamo::Store::new(client, format!("t-pubadp-{test}"));
    store.create_table_for_tests().await.unwrap();
    Some(Arc::new(store))
}

/// Storeless fake for tests that never reach the store (fallback 404).
async fn fake_store() -> Arc<dynamo::Store> {
    let cfg = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .endpoint_url("http://127.0.0.1:0")
        .region("us-east-1")
        .test_credentials()
        .load()
        .await;
    Arc::new(dynamo::Store::new(aws_sdk_dynamodb::Client::new(&cfg), "fake".into()))
}

// The real trait (verified in plan review): public_api::Invoker has exactly ONE method,
// `gift`, and its types live in the fulfillment crate — NOT domain. Mirror of
// public api_test.rs's MockInvoker error arm; async_trait usage matches that file.
struct NoInvoker;
#[async_trait::async_trait]
impl public_api::Invoker for NoInvoker {
    async fn gift(
        &self,
        _req: fulfillment::FulfillRequest,
    ) -> Result<fulfillment::FulfillResponse, String> {
        Err("adapter tests never invoke".into())
    }
}

/// spec D5: "real-shaped" is a predicate. Every v1 fixture goes through here before
/// from_str ever sees it: (i) full production key-set present, (ii) correlated fields
/// consistent. Closes the hand-edit class, not the instance.
fn load_v1_fixture(raw: &'static str) -> String {
    let v: serde_json::Value = serde_json::from_str(raw).expect("fixture must be JSON");
    for key in [
        "resource", "path", "httpMethod", "headers", "multiValueHeaders",
        "queryStringParameters", "multiValueQueryStringParameters", "pathParameters",
        "stageVariables", "requestContext", "body", "isBase64Encoded",
    ] {
        assert!(
            v.get(key).is_some(),
            "fixture missing production key {key:?} — a corpus-minimal fixture exercises \
             arms production never takes (spec D5-1)"
        );
    }
    let ctx = &v["requestContext"];
    for key in ["accountId", "resourceId", "stage", "requestId", "requestTimeEpoch",
                "identity", "resourcePath", "httpMethod", "apiId", "path"] {
        assert!(ctx.get(key).is_some(), "requestContext missing {key:?} (spec D5-1)");
    }
    let stage = ctx["stage"].as_str().expect("stage is a string");
    let path = v["path"].as_str().expect("path is a string");
    assert_eq!(
        ctx["path"].as_str().unwrap(),
        format!("/{stage}{path}"),
        "requestContext.path must equal /<stage><path> (spec D5-2 correlated-field check)"
    );
    assert_eq!(v["httpMethod"], ctx["httpMethod"], "method must agree in both places (D5-2)");
    assert_eq!(v["resource"], ctx["resourcePath"], "resource must agree in both places (D5-2)");
    raw.to_string()
}

/// The predicate must earn its red PER ARM (spec AC4; one planted defect per copy —
/// a both-defects fixture proves only whichever arm panics first, masking the other).
#[test]
#[should_panic(expected = "fixture missing production key")]
fn degenerate_missing_key_fails_the_keyset_arm() {
    load_v1_fixture(include_str!("fixtures/apigw_v1_degenerate_missing_key.json"));
}

#[test]
#[should_panic(expected = "requestContext.path must equal")]
fn degenerate_inconsistent_path_fails_the_correlation_arm() {
    load_v1_fixture(include_str!("fixtures/apigw_v1_degenerate_inconsistent.json"));
}

fn test_router(store: Arc<dynamo::Store>) -> axum::Router {
    router(store, Arc::new(NoInvoker), None, "https://bendobundles.com".into())
}

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).expect("response body must be JSON")
}

/// v1 event → translated request: stage `live` must NOT be prepended (flag set),
/// and a path matching no route must land on the router fallback — the generic
/// `{"error":"not found"}`, which is distinct from the matched-route unknown-link
/// shape asserted in the next test. Together they pin path derivation from both sides.
#[tokio::test]
async fn v1_translation_derives_stageless_path_and_falls_back() {
    init_stage_env();
    let req = lambda_http::request::from_str(&load_v1_fixture(include_str!("fixtures/apigw_v1_fallback.json")))
        .expect("v1 fixture must translate");
    assert_eq!(req.uri().path(), "/api/definitely/no/such/route", "stage must not be prepended");
    let resp = test_router(fake_store().await).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 404);
    assert_eq!(body_json(resp).await, serde_json::json!({"error": "not found"}));
}

/// The automated twin of #186's manual prod probe: unknown token through the REAL
/// translation must reach handle_get_link and answer with the app's own JSON —
/// proof the adapter delivered the event to the Router, the half no test covered.
#[tokio::test]
async fn v1_translation_reaches_link_route_with_unknown_token() {
    init_stage_env();
    let Some(store) = store_or_skip("link-unknown").await else { return };
    let req = lambda_http::request::from_str(&load_v1_fixture(include_str!("fixtures/apigw_v1_link_unknown.json")))
        .expect("v1 fixture must translate");
    assert_eq!(req.uri().path(), "/api/l/definitely-not-a-token");
    let resp = test_router(store).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 404);
    assert_eq!(body_json(resp).await, serde_json::json!({"error": "unknown link"}),
        "matched-route 404, NOT the fallback shape — path derivation reached the handler");
}

/// multiValueQueryStringParameters survive translation: both values of `a` visible
/// via RequestExt AND in the reconstructed URI (build_request_uri re-serializes the
/// map, preferring the multi-value form — that reconstruction is the assertion target).
#[tokio::test]
async fn v1_translation_preserves_multi_value_query() {
    init_stage_env();
    let req = lambda_http::request::from_str(&load_v1_fixture(include_str!("fixtures/apigw_v1_multi_value_query.json")))
        .expect("v1 fixture must translate");
    let qs = req.query_string_parameters();
    assert_eq!(qs.all("a").expect("a present"), vec!["1", "2"]);
    assert_eq!(qs.first("b"), Some("3"));
    let query = req.uri().query().expect("query must be reconstructed");
    assert!(query.contains("a=1") && query.contains("a=2") && query.contains("b=3"),
        "reconstructed URI must carry every value: got {query}");
    // Deliberately NOT asserted: parameter ORDER in the reconstructed URI (map-backed,
    // not part of the property) — recorded as asserted-not-pinned in spec AC1.
    // (Verified: QueryMap::all returns Option<Vec<&str>> — query_map-0.7.0 lib.rs:101 —
    // so the assert_eq against vec!["1", "2"] compiles as written.)
}

/// isBase64Encoded body is DECODED by translation (to bytes, non-ASCII intact), and
/// axum's Json extractor parses it — proven by reaching the handler's own link lookup
/// (unknown-link 404) instead of a 4xx extractor refusal. Validation precedes the
/// link read in handle_post_thanks, so a non-empty note is required to get there.
#[tokio::test]
async fn v1_translation_decodes_base64_body_to_reach_json_extractor() {
    init_stage_env();
    let Some(store) = store_or_skip("b64-thanks").await else { return };
    let req = lambda_http::request::from_str(&load_v1_fixture(include_str!("fixtures/apigw_v1_base64_thanks.json")))
        .expect("v1 fixture must translate");
    assert_eq!(
        std::str::from_utf8(req.body().as_ref()).expect("decoded body must be the original UTF-8"),
        "{\"note\":\"thank you ben ❤\"}",
    );
    let resp = test_router(store).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 404);
    assert_eq!(body_json(resp).await, serde_json::json!({"error": "unknown link"}),
        "the decoded body must parse as JSON and clear note-validation to reach the link read");
}

/// Discriminator guard (spec D4): a v2 event must still parse AS v2 — if a features
/// change or lambda_http upgrade ever reorders the deserializer so this parses as v1
/// (or stops parsing), this fails loudly instead of the suite silently testing the
/// wrong arm. pass_through is OFF, apigw_rest is tried first (deserializer.rs:29-34).
#[tokio::test]
async fn v2_guard_still_parses_as_v2() {
    init_stage_env();
    let req = lambda_http::request::from_str(include_str!("fixtures/apigw_v2_guard.json"))
        .expect("v2 must remain parseable");
    // v2's rawPath carries no stage; the translated path is the rawPath verbatim.
    assert_eq!(req.uri().path(), "/api/l/x");
    // v1 events carry requestContext.identity.sourceIp; v2 carries http.sourceIp.
    // RequestExt::request_context() exposes which arm parsed it:
    match req.request_context() {
        lambda_http::request::RequestContext::ApiGatewayV2(_) => {}
        other => panic!("v2 fixture parsed as {other:?} — deserializer arm changed"),
    }
}

/// ALB guard (spec D4, OMBB Q4): the cascade has more arms than v1/v2 — one ALB fixture
/// pins the fall-through class. Same pair logic as the v2 guard: parses AS ALB, which
/// simultaneously asserts the v1 arm didn't capture it.
#[tokio::test]
async fn alb_guard_still_parses_as_alb() {
    init_stage_env();
    let req = lambda_http::request::from_str(include_str!("fixtures/alb_guard.json"))
        .expect("ALB must remain parseable");
    assert_eq!(req.uri().path(), "/api/l/x");
    match req.request_context() {
        lambda_http::request::RequestContext::Alb(_) => {}
        other => panic!("ALB fixture parsed as {other:?} — deserializer arm changed"),
    }
}
```

(The two guard fixtures deliberately do NOT go through `load_v1_fixture` — they are not v1
events and the predicate would rightly reject them; their doc comments say so.)

Implementer notes: (1) the `NoInvoker` impl above IS the verified trait shape (one `gift`
method, `fulfillment::` types — read from `public-api/src/lib.rs:28-30` during plan review);
if cargo check disagrees, `public api_test.rs:105-140`'s `MockInvoker` is the tiebreaker —
mirror it, never invent; (2) `RequestContext` variant names (`ApiGatewayV2`, `Alb`, and the
v1 variant) come from `lambda_http::request` — `cargo check` will name them exactly;
(3) `async_trait`: use whatever `public api_test.rs` uses for its mocks — same crate, same
dev-deps, guaranteed available. (4) **Deliberate deviation from spec D3's first draft
(recorded per plan review M5):** the multi-value test does not drive the steam-return
extractor — see the spec's Non-goals for the reasoning; the narrowing is a decision, not
drift.

- [ ] **Step 3: Local gate**

Run: `cd ~/bendobundles && cargo check -p public-api --tests -j 1 && cargo clippy -p public-api --all-targets -j 1 -- -D warnings`
Expected: clean compile. Fix trait-shape mismatches per implementer notes; do NOT modify production code to make tests fit.

- [ ] **Step 4: Commit and open the draft PR (CI = the test oracle from here on)**

```bash
cd ~/bendobundles && cargo check -p public-api --tests -j 1 \
  && git add crates/public-api/tests/ \
  && git commit -S -m "test: public-api adapter boundary — v1 fixtures through real lambda_http translation (#186)

Production is REST v1/payload 1.0 (no apigatewayv2 resource exists in
terraform/) — #186's text said v2; the spec corrects it. Covers: stageless
path derivation + both 404 shapes, multi-value query reconstruction, base64
body decode through the Json extractor, and a v2 discriminator guard." \
  && git push -u origin kitten/ci-truth-adapter \
  && gh pr create -R yourcodekitten/bendobundles --draft \
       --title "Green must mean green: adapter coverage + complete failure census" \
       --body "Draft while the arc executes. Spec: docs/superpowers/specs/2026-08-12-ci-truth-adapter-coverage.md. Fixes #185. Fixes #186. (#168 handled by comment+close, not autolink.)"
```

- [ ] **Step 5: Watch CI — the new binary must RUN, not just compile**

Run: `cd ~/bendobundles && gh pr checks --watch`, then:
```bash
run_id=$(gh run list --branch kitten/ci-truth-adapter -L 1 --json databaseId -q '.[0].databaseId')
gh run view "$run_id" --log | grep -E 'Running.*(adapter|api)_test|test result'
```
Expected: a `Running tests/adapter_test.rs` line for public-api, all its tests listed pass, 0 fail, and the pre-existing suites all present (per-suite counts read from the log, per #185's lesson — never infer from the green check alone).

---

### Task 5: the stage flag is load-bearing — env-free control binary (spec D3 third bullet)

**Files:**
- Create: `crates/public-api/tests/adapter_stage_control_test.rs`
- Create: `crates/public-api/tests/fixtures/apigw_v1_no_stage.json`
- Create: `crates/public-api/tests/fixtures/apigw_v1_default_stage.json`

**Interfaces:**
- Consumes: `apigw_v1_link_unknown.json` from Task 4 (same file — that's the point: same input, different env, different verdict).

**Why a separate binary:** the control needs the env var ABSENT for its whole process lifetime; the Once in adapter_test.rs sets it for that whole process. Two binaries = two processes = both worlds observable. This is the spec's "exercise the probe against a state different from the one it currently reports" — if we cannot make translation disagree with the flag-set world, we haven't controlled it.

- [ ] **Step 1: Write the control**

```rust
//! CONTROL BINARY — the env-free twin of adapter_test.rs. Proves
//! AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH is load-bearing: without it, lambda_http
//! RE-PREPENDS requestContext.stage and every route 404s on the fallback — which is
//! exactly what production would do if the terraform env var were ever dropped
//! (terraform/aws-lambda.tf:85-89). No test here sets any env var; keeping this file
//! Once-free is the invariant that makes it a control.

#[tokio::test]
async fn without_stage_flag_the_stage_is_prepended_and_routing_breaks() {
    assert!(
        std::env::var("AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH").is_err(),
        "control precondition: flag must be ABSENT in this binary — if this fires, \
         something leaked ambient env (a .cargo [env] entry?) and the control is void"
    );
    let req = lambda_http::request::from_str(include_str!("fixtures/apigw_v1_link_unknown.json"))
        .expect("fixture must still translate");
    assert_eq!(
        req.uri().path(),
        "/live/api/l/definitely-not-a-token",
        "without the flag, stage IS prepended — this is the world terraform's env var prevents"
    );
}

/// The stage-ABSENT arm (spec D2, plan-review M4): with no stage in the event
/// (requestContext.stage null), the path is not prefixed even without the flag —
/// request.rs's `None => path.into()` branch, otherwise unreachable in these suites.
#[tokio::test]
async fn without_stage_in_event_the_path_is_untouched_even_without_flag() {
    assert!(
        std::env::var("AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH").is_err(),
        "control precondition: flag must be ABSENT in this binary"
    );
    let req = lambda_http::request::from_str(include_str!("fixtures/apigw_v1_no_stage.json"))
        .expect("fixture must translate");
    assert_eq!(req.uri().path(), "/api/l/definitely-not-a-token",
        "no stage in event ⇒ nothing to prepend, flag or no flag");
}
```

`apigw_v1_no_stage.json`: copy `apigw_v1_link_unknown.json`, set `requestContext.stage` to
`null` and `requestContext.path` equal to the top-level `path` (no stage to prefix).

`apigw_v1_default_stage.json`: copy `apigw_v1_no_stage.json`, set `requestContext.stage` to
`"$default"` (path fields unchanged — `$default` is never a real path prefix). Third test in
this binary (spec AC1's fourth arm; OMBB step-5 M1 — `Some("$default")` and `None` are
DISTINCT match arms in `apigw_path_with_stage` and each deserves its own pin):

```rust
/// `$default` stage arm: treated like no-stage — never prepended, flag or no flag.
#[tokio::test]
async fn default_stage_is_never_prepended_even_without_flag() {
    assert!(
        std::env::var("AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH").is_err(),
        "control precondition: flag must be ABSENT in this binary"
    );
    let req = lambda_http::request::from_str(include_str!("fixtures/apigw_v1_default_stage.json"))
        .expect("fixture must translate");
    assert_eq!(req.uri().path(), "/api/l/definitely-not-a-token",
        "$default is a sentinel, not a prefix");
}
```

These control fixtures do NOT go through `load_v1_fixture` (it lives in the other binary,
and the stage variants intentionally break the `/<stage><path>` correlation the predicate
enforces) — the control binary's fixtures are pinned by their own assertions instead;
say this in the file header.

(No router/oneshot needed: the path assertions alone prove the divergence, and storeless
tests keep the control binary trivial. The precondition asserts make the controls
self-checking — if ambient env ever leaks in, they report VOID loudly instead of passing
vacuously.)

- [ ] **Step 2: Local gate + commit + push**

```bash
cd ~/bendobundles && cargo check -p public-api --tests -j 1 \
  && git add crates/public-api/tests/adapter_stage_control_test.rs crates/public-api/tests/fixtures/apigw_v1_no_stage.json crates/public-api/tests/fixtures/apigw_v1_default_stage.json \
  && git commit -S -m "test: env-free control — without the stage flag, translation prepends /live and routing breaks

Same fixture as the flag-set binary, opposite verdict: the pair proves the
env var is load-bearing rather than decorative (spec D2/D3). Plus the two
sentinel arms: stage absent and stage \$default — nothing prepended, flag
or no flag; distinct match arms, each with its own pin." \
  && git push
```

- [ ] **Step 3: CI check as in Task 4 Step 5** — both new binaries must appear in the census.

---

### Task 5b: the presence footgun — `="false"` still strips (spec D2 third binary)

**Files:**
- Create: `crates/public-api/tests/adapter_stage_false_test.rs`

**Interfaces:**
- Consumes: `apigw_v1_link_unknown.json` from Task 4 (same fixture, third env world).

**Why its own binary:** the flag check is `env::var(...).is_ok()` — presence, not value
(verified `request.rs:408`). An operator who "disables" the flag by setting it to `"false"`
changes nothing, silently. This binary pins that semantic so the surprise has a test with
its name on it. It needs env = `"false"` process-wide, which neither sibling binary can host.

- [ ] **Step 1: Write it**

```rust
//! THIRD ENV WORLD (spec D2): AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH set to the string
//! "false". lambda_http checks PRESENCE (request.rs:408 `env::var(...).is_ok()`), so
//! "false" still strips the stage — terraform's `= "true"` works by presence, not truth.
//! If this test ever fails, lambda_http moved to value-semantics and terraform's config
//! (and both sibling binaries' assumptions) must be re-read.

use std::sync::Once;

static STAGE_ENV: Once = Once::new();

fn init_false_env() {
    STAGE_ENV.call_once(|| {
        // SAFETY: same containment contract as adapter_test.rs's init (spec D2).
        unsafe { std::env::set_var("AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH", "false") }
    });
}

#[tokio::test]
async fn flag_set_to_false_still_strips_the_stage() {
    init_false_env();
    let req = lambda_http::request::from_str(include_str!("fixtures/apigw_v1_link_unknown.json"))
        .expect("fixture must translate");
    assert_eq!(req.uri().path(), "/api/l/definitely-not-a-token",
        "the flag is PRESENCE-triggered: \"false\" activates it too");
}
```

- [ ] **Step 2: Local gate + commit (batch the push with Task 6's)**

```bash
cd ~/bendobundles && cargo check -p public-api --tests -j 1 \
  && git add crates/public-api/tests/adapter_stage_false_test.rs \
  && git commit -S -m "test: the stage flag is presence-triggered — =\"false\" still strips (spec D2)"
```

---

### Task 6: admin-api adapter test — cookie-carried session through translation (spec D3 second bullet)

**Files:**
- Create: `crates/admin-api/tests/adapter_test.rs`
- Create: `crates/admin-api/tests/fixtures/apigw_v1_note_with_cookie.json`

**Interfaces:**
- Consumes: `admin_api::router(store, invoker, admin_hash: String, steam: Option<Arc<SteamClient>>) -> Router` (`crates/admin-api/src/lib.rs:78`); helpers mirrored from `crates/admin-api/tests/api_test.rs`: `store_or_skip` (`:28-56`, reprefix `t-admadp-`), `test_admin_hash` (`:78-88`), `admin_login` (`:138-178`), `MockAdminInvoker` (`:92-125`), `test_link` (`:204` — plan review corrected this ref), plus `body_json` (`:129-134`).
- Produces: `crates/admin-api/tests/adapter_test.rs` (with the mirrored helpers + `init_stage_env`) and `apigw_v1_note_with_cookie.json`; Task 7 appends to this file and creates its own fixture.

**Flow:** mint a real session via direct-router `admin_login` (that half is already covered by api_test.rs — not the subject), then send the note POST **through translation**: fixture carries `Cookie: session=__SESSION__` and `X-Admin-Request: 1` headers; the test substitutes the real session into the JSON string before `from_str`. Assert the note landed (read back through the store or a direct-router GET) — proving the Cookie header and CSRF header both survive translation and satisfy the middleware (`crates/admin-api/src/lib.rs:145-194`).

- [ ] **Step 1: Fixture**

`apigw_v1_note_with_cookie.json` — Task 4's v1 skeleton with:
- `"resource": "/admin/api/{proxy+}"`, `"resourcePath": "/admin/api/{proxy+}"`,
- `"path": "/admin/api/links/fixture-tok/note"`, `"pathParameters": { "proxy": "links/fixture-tok/note" }`, `"requestContext.path": "/live/admin/api/links/fixture-tok/note"` (B1: derived fixtures update requestContext.path with the top-level path, always),
- `"httpMethod": "POST"` (both places),
- headers — **every one of them mirrored into `multiValueHeaders` too; a real REST event
  populates both maps, always, and translation merges them (OMBB Q4 / spec D3)**:
  `"Content-Type": "application/json"`, `"Cookie": "session=__SESSION__"`,
  `"X-Admin-Request": "1"` — verified in plan review: the middleware const is
  `ADMIN_REQUEST_HEADER = "x-admin-request"` and only `contains_key` is checked (any value
  passes; `"1"` is fine); no cookie → 401, cookie-without-header → 403,
- `"body": "{\"gift_note\":\"from the adapter side\"}"`, `"isBase64Encoded": false`.

- [ ] **Step 2: Test**

```rust
//! Adapter-boundary tests for admin-api. Same design as public-api's
//! adapter_test.rs (see its header comment for the Once/env rationale — spec D2).

// HELPERS — copy, do not invent (the repo has no tests/common; duplication is the idiom):
//   init_stage_env()        → verbatim from crates/public-api/tests/adapter_test.rs (Task 4)
//   load_v1_fixture()       → verbatim COPY from Task 4's adapter_test.rs (spec AC4 covers
//                             EVERY v1 fixture — the admin ones included; OMBB step-5 B2).
//                             A test binary CANNOT import another crate's test binary —
//                             there is no `use public_api_tests::...`; mirroring the fn body
//                             is the ONLY mechanism, same as init_stage_env (Lilith's rider).
//                             The degenerate-red tests stay public-side only: AC4's
//                             predicate-red needs one home, not two.
//   store_or_skip()         → crates/admin-api/tests/api_test.rs:28-56, table prefix
//                             changed to format!("t-admadp-{test}") — ONLY that change
//   test_admin_hash()       → api_test.rs:78-88 verbatim
//   MockAdminInvoker        → api_test.rs:92-125 verbatim
//   admin_login()           → api_test.rs:138-178 verbatim
//   body_json()             → api_test.rs:129-134 verbatim
//   test_link()             → api_test.rs:204, copy verbatim

/// The admin session cookie and CSRF header survive translation: a note POST through
/// the REAL v1 event path lands on the middleware, passes both auth layers, and the
/// note is durably written. This is the half of admin auth no test observed (#186).
#[tokio::test]
async fn v1_translation_carries_session_cookie_and_csrf_header() {
    init_stage_env();
    let Some(store) = store_or_skip("note-cookie").await else { return };
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new();
    let hash = test_admin_hash("pw");
    store.create_link(&test_link("fixture-tok")).await.unwrap();

    // Session minted via the already-covered direct-router path (not the subject here).
    let session = admin_login(&store, &invoker, &hash, "pw").await;

    // Predicate BEFORE substitution (spec AC4 — every v1 fixture; the __SESSION__
    // placeholder doesn't touch any predicate-checked key, so raw validation is exact).
    let fixture = load_v1_fixture(include_str!("fixtures/apigw_v1_note_with_cookie.json"))
        .replace("__SESSION__", &session);
    let req = lambda_http::request::from_str(&fixture).expect("v1 fixture must translate");
    assert_eq!(req.uri().path(), "/admin/api/links/fixture-tok/note");
    assert_eq!(
        req.headers().get("cookie").expect("cookie must survive translation"),
        &format!("session={session}"),
    );

    let resp = router(Arc::clone(&store), Arc::clone(&invoker), hash.clone(), None)
        .oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200, "both auth layers must pass through translation");

    // Durable proof, not just a 200: read the link back and find the note.
    let link = store.get_link("fixture-tok").await.unwrap().expect("link exists");
    assert_eq!(link.gift_note.as_deref(), Some("from the adapter side"));
}
```

Implementer notes: verified in plan review — `test_link` at api_test.rs:204,
`Link.gift_note` exists (`domain/src/lib.rs:181`), success response is 200 `{"ok":true}`,
`store.create_link(&Link)` exists. The code block omits `use` statements — take them from
api_test.rs's own imports (same crate). The 200-vs-401 distinction is the test's teeth:
sabotage in Task 8 strips the Cookie from BOTH header maps and must produce 401, observed red.

- [ ] **Step 3: Local gate + commit**

```bash
cd ~/bendobundles && cargo check -p admin-api --tests -j 1 && cargo clippy -p admin-api --all-targets -j 1 -- -D warnings \
  && git add crates/admin-api/tests/ \
  && git commit -S -m "test: admin-api adapter boundary — session cookie + CSRF header survive v1 translation (#186)"
```

- [ ] **Step 4: Push + CI census check as in Task 4 Step 5** (three adapter binaries now).

---

### Task 7: response-side translation — Set-Cookie's v1 fate (spec G2, D7)

**Files:**
- Create: `crates/admin-api/tests/fixtures/apigw_v1_login.json`
- Modify: `crates/admin-api/tests/adapter_test.rs` (append one test)

**Interfaces:**
- Consumes: `lambda_http::Adapter::from(router)` (`#[doc(hidden)]`, public via `From` — spec D7 accepts the churn risk with eyes open); `lambda_http::request::LambdaRequest`; `lambda_runtime::{LambdaEvent, Context}`; `tower::ServiceExt::oneshot`.

**Why:** V1 responses put ALL headers (including `Set-Cookie`) into `multiValueHeaders` and leave `headers` empty (lambda_http `response.rs:78-93`); V2 would hoist cookies into a `cookies` array. The admin session cookie's fate is origin-type-dependent and currently unobserved by any test. One test pins it.

- [ ] **Step 1: Fixture**

`apigw_v1_login.json` — the v1 skeleton with `"resource": "/admin/api/{proxy+}"` and
`requestContext.resourcePath` matching (the admin shape — plan review caught the public
skeleton's `/api/{proxy+}` leaking in here), `"path": "/admin/api/login"`,
`"requestContext.path": "/live/admin/api/login"` (B1), `"pathParameters": { "proxy": "login" }`, `"httpMethod": "POST"` (both places),
`Content-Type: application/json` in both header maps, `"body": "{\"password\":\"pw\"}"`,
`"isBase64Encoded": false`.

- [ ] **Step 2: Test (append to adapter_test.rs)**

```rust
/// FULL adapter round-trip (request AND response translation): login through
/// Adapter::from(router) and assert the V1 response shape — Set-Cookie lands in
/// multiValueHeaders (v1 puts ALL headers there and leaves `headers` for v2's
/// cookie-hoisting; lambda_http response.rs:78-93). Asserted on the SERIALIZED
/// JSON, not the hidden enum, so a lambda_http 2.x reshuffle breaks this test
/// loudly at compile/parse instead of silently changing prod behavior.
/// NOTE: Adapter is #[doc(hidden)] but constructible via the public From impl —
/// accepted with eyes open (spec D7); cheap to rewrite if 2.x hides it.
#[tokio::test]
async fn v1_response_translation_puts_set_cookie_in_multi_value_headers() {
    init_stage_env();
    let Some(store) = store_or_skip("resp-cookie").await else { return };
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new();
    let hash = test_admin_hash("pw");

    // lambda_runtime is NOT a dependency of admin-api and must not become one —
    // lambda_http RE-EXPORTS it (lambda_http-1.3.0/src/lib.rs:77: `pub use
    // lambda_runtime::{self, Context, LambdaEvent}`), which is the only sanctioned path
    // here (plan-review M3).
    use lambda_http::lambda_runtime;
    // load_v1_fixture first — spec AC4 covers every v1 fixture, this one included (B2).
    let payload: lambda_http::request::LambdaRequest =
        serde_json::from_str(&load_v1_fixture(include_str!("fixtures/apigw_v1_login.json")))
            .expect("fixture must deserialize as a LambdaRequest");
    let event = lambda_runtime::LambdaEvent::new(payload, lambda_runtime::Context::default());

    let adapter = lambda_http::Adapter::from(router(store, invoker, hash, None));
    let resp = adapter.oneshot(event).await.expect("adapter must produce a response");

    let json = serde_json::to_value(&resp).expect("LambdaResponse must serialize");
    assert_eq!(json["statusCode"], 200);
    let set_cookie = json["multiValueHeaders"]["set-cookie"]
        .as_array()
        .expect("v1: Set-Cookie must be in multiValueHeaders");
    assert!(
        set_cookie[0].as_str().unwrap().starts_with("session="),
        "the session cookie must survive response translation: {set_cookie:?}"
    );
    assert!(
        json["cookies"].is_null(),
        "v1 must NOT hoist cookies into a v2-style cookies array"
    );
    assert_eq!(json["isBase64Encoded"], false, "a JSON body must not be base64-flagged (spec G2)");
}
```

Implementer notes: `LambdaEvent`/`Context` paths and whether `Adapter` implements `tower::Service` directly (needing `.oneshot` from ServiceExt) are cargo-check-verified; if `Context::default()` is not available, build the minimal Context the constructor requires (`lambda_runtime` types.rs:66,90-96 documents `LambdaEvent::new`). If `serde_json::to_value(&resp)` fails because `LambdaResponse` isn't `Serialize` in this direction, serialize via the type the runtime actually returns to AWS — find `into_response`/serialization in lambda_http `response.rs` and assert on that exact struct. Header-name case: VERIFIED — `serialize_multi_value_headers` writes `key.as_str()` from `http::HeaderName` (aws_lambda_events-1.2.0 custom_serde/headers.rs:9-22), which is always lowercase; `"set-cookie"` as written is correct.

- [ ] **Step 3: Local gate + commit + push + CI census check**

```bash
cd ~/bendobundles && cargo check -p admin-api --tests -j 1 \
  && git add crates/admin-api/tests/ \
  && git commit -S -m "test: v1 response translation — admin Set-Cookie lands in multiValueHeaders (#186, spec G2/D7)"
```

---

### Task 8: batched sabotage census — every new test observed RED for the right reason (spec D6)

**Files:**
- Modify (temporarily): the five fixture JSONs + `apigw_v1_note_with_cookie.json`
- Net diff after this task: zero (sabotage commit + revert commit; branch is squash-merged so history stays clean on main)

**Why:** a test whose red state has never been observed is a comment. The box cannot run these tests; CI can — so sabotage is batched into ONE commit and Task 1's `--no-fail-fast` (already on this branch) makes the single red run report the COMPLETE census of failures. One red cycle, one revert, all controls observed.

- [ ] **Step 1: The sabotage commit**

Two of this plan's original recipes were themselves no-ops, caught in plan review (a stage
edit that a presence-flag binary ignores BY DESIGN; a `version` edit that `#[serde(default)]`
swallows). The recipes below are the corrected set. In ONE commit:

| # | File (edit) | Expected red — test, binary, and the SPECIFIC assertion |
|---|---|---|
| 1 | `apigw_v1_fallback.json`: path `/api/definitely/no/such/route` → `/api/l/x` (+ keep `pathParameters.proxy` and `requestContext.path` consistent — the loader predicate must PASS; we sabotage the property, not the parse) | `v1_translation_derives_stageless_path_and_falls_back` (public adapter_test): now MATCHES a route → fake store errors → 500 ≠ 404 status assert, or unknown-link body ≠ fallback body assert. Either message is the test's own. |
| 2 | `apigw_v1_link_unknown.json`: path `/api/l/definitely-not-a-token` → `/api/x/definitely-not-a-token` (+ consistent `pathParameters`/`requestContext.path`) | THREE tests, one edit: `v1_translation_reaches_link_route_with_unknown_token` (public adapter_test — fallback body ≠ unknown-link assert); `without_stage_flag_the_stage_is_prepended_and_routing_breaks` (control binary — `/live/api/x/...` ≠ expected `/live/api/l/...`); `flag_set_to_false_still_strips_the_stage` (false binary — `/api/x/...` ≠ `/api/l/...`). The cross-binary fan-out is DELIBERATE — the census reader must expect all three lines. |
| 3 | `apigw_v1_multi_value_query.json`: `"a": ["1","2"]` → `["2"]` (and single-value map stays `"a":"2"` — consistent) | `v1_translation_preserves_multi_value_query` (public adapter_test): `qs.all("a")` equality assert fires. |
| 4 | `apigw_v1_base64_thanks.json`: `"isBase64Encoded": true` → `false` | `v1_translation_decodes_base64_body_to_reach_json_extractor` (public adapter_test): the decoded-bytes `from_utf8`/equality assert fires FIRST (body is still base64 text) — that exact message is the expected line. |
| 5 | `apigw_v2_guard.json`: add top-level `"httpMethod": "GET"` (Lilith's measured recipe — the v1 arm requires top-level `httpMethod` + a parseable `requestContext`, and v1 is tried FIRST; the v2 fixture's own `requestContext` may satisfy v1's all-optional context, flipping capture to v1) | `v2_guard_still_parses_as_v2`: the variant match panics with "parsed as ... deserializer arm changed". **If it stays green**, the cascade did NOT capture it as v1 — escalate the sabotage to also replacing `requestContext` with the v1 fixture's (then the fixture is v1-shaped and MUST capture); diagnose per the stop-rule, don't guess. |
| 6 | `alb_guard.json`: delete its `requestContext` (ALB's ELB marker lives there; without it no arm matches and `pass_through` is off) | `alb_guard_still_parses_as_alb`: `from_str` errors → the `expect("ALB must remain parseable")` fires. |
| 7 | `apigw_v1_note_with_cookie.json`: delete the `Cookie` entry from **BOTH** `headers` AND `multiValueHeaders` (translation merges the maps — deleting one is a no-op) | `v1_translation_carries_session_cookie_and_csrf_header` (admin adapter_test): the `headers().get("cookie")` expect, or the 200 status assert (observed 401). Record whichever fires — both are the test's own teeth. |
| 8 | `apigw_v1_login.json`: body `{"password":"pw"}` → `{"password":"wrong"}` | `v1_response_translation_puts_set_cookie_in_multi_value_headers` (admin adapter_test): login 401 mints no Set-Cookie → the `multiValueHeaders["set-cookie"]` extraction expect fires — the property assertion itself, input-side form (spec D6). |
| 9 | `apigw_v1_no_stage.json`: set `requestContext.stage` from `null` → `"live"` (Lilith's recipe — exactly one test reads this fixture, so attribution stays clean) | `without_stage_in_event_the_path_is_untouched_even_without_flag` (control binary): flag absent + stage present → path becomes `/live/...` → its equality assert fires. This UN-demotes the `None`-branch property to fully pinned. |
| 10 | `apigw_v1_default_stage.json`: set `requestContext.stage` from `"$default"` → `"live"` (same one-fixture-one-test attribution) | `default_stage_is_never_prepended_even_without_flag` (control binary): the sentinel arm's equality assert fires — `Some("$default")` and `None` are distinct match arms, each pinned by its own red (OMBB M1). |

**Why batching all sabotages into one commit is sound (invariant for future editors):**
attribution is by-construction — each test reads exactly ONE sabotaged file, so every red
maps to its row with no ambiguity; the single fixture shared across binaries (edit #2) fans
out *within one counted row*, not across rows. If you ever share a fixture across two
DIFFERENT property rows, this invariant breaks and the census must be split into two commits.

NOT sabotaged, with reasons stated in the PR table: Task 2's dynamo test (race is
nondeterministic; excused in its own commit message) · the two `degenerate_*` predicate
tests (they ARE permanent red-exercises — their sabotage is their normal operation).

```bash
cd ~/bendobundles \
  && git add crates/*/tests/fixtures/ \
  && git commit -S -m "SABOTAGE (will be reverted): every adapter fixture wrong in its own dimension — observing the RED census (spec D6)" \
  && git push
```

- [ ] **Step 1b: Prove every sabotage LANDED before reading any result (spec D6 item 2)**

```bash
cd ~/bendobundles && git show --stat HEAD && git show HEAD -- crates/ | grep -E '^[+-]' | grep -vE '^[+-]{3}'
```
Expected: exactly the 10 files above, and every changed line is one of the named edits — a
mis-aimed edit that matched nothing leaves a test green for a reason that is NOT vacuity.
Paste this diff into the evidence table alongside the failures.

- [ ] **Step 2: Read the red census from the CI log**

Run: `gh pr checks --watch` (expect the test job RED), then pull the log (run-id command from
Task 4 Step 5) and verify BOTH DIRECTIONS: **every test in the expected-red column above
appears FAILED with its named assertion message** — per-binary (OMBB MINOR-4's restatement;
the table's rows govern): **public adapter_test 6** (rows 1-6) · **control 3** (row 2's
fan-out + rows 9-10) · **false-binary 1** (row 2's fan-out) · **admin adapter_test 2**
(rows 7-8) = **12 lines total** — **and no test outside the column went red** (a stranger's
red means a sabotage landed somewhere it shouldn't — diagnose before proceeding). Record the run URL + the
per-test failure list with messages — it goes in the PR body as the D6 evidence table.
**Stop-rule (softened per plan review — my own first draft had two no-op recipes):** if a
listed test stays GREEN, diagnose **which of test/sabotage is broken** — a green under
sabotage has two causes and only one of them means the test is vacuous. Fix whichever is
actually at fault, document which it was, re-run this step for that family.

- [ ] **Step 3: Revert**

```bash
cd ~/bendobundles && git revert --no-edit HEAD && git push
```

Expected: CI green again; `git diff main...HEAD -- crates/*/tests/fixtures/` shows the fixtures at their Task 4/6/7 content.

- [ ] **Step 4: Record the evidence**

Edit the PR body: add a "Sabotage census (D6)" section — table of test → sabotage → landed-proof diff → observed failure line → link to the red run. This is what review passes and OMBB's gate will check.
**Do NOT use `gh pr edit --body` — it FAILS on this box** (Projects-deprecation 4xx; OMBB M2, measured). Use the GraphQL mutation:
```bash
prid=$(gh api repos/yourcodekitten/bendobundles/pulls/<N> --jq .node_id)
gh api graphql -f query='mutation($id: ID!, $body: String!) { updatePullRequest(input: {pullRequestId: $id, body: $body}) { pullRequest { number } } }' -F id="$prid" -F body="$(cat /tmp/pr-body.md)"
```

---

### Task 9: bookkeeping — PR ready, #168 verdict, #186 correction

**Files:** none (GitHub state only)

- [ ] **Step 1: PR ready for review**

Write the final body to a file first, then apply via GraphQL (NOT `gh pr edit` — fails on
this box, OMBB M2; same mutation as Task 8 Step 4). Body contents, all required: what
shipped per issue (#186 adapter coverage: request-side ×4 + guards ×2 + three control-binary
arms + response-side ×1; #185 one line; #168 hardening ×2), the v1-not-v2 correction with
terraform receipts, the sabotage census table from Task 8 (12 lines, landed-proof included),
per-suite counts read from the green run's log, and the spec/plan paths. `Fixes #185.
Fixes #186.`

```bash
cd ~/bendobundles && gh pr ready <N> \
  && prid=$(gh api repos/yourcodekitten/bendobundles/pulls/<N> --jq .node_id) \
  && gh api graphql -f query='mutation($id: ID!, $body: String!) { updatePullRequest(input: {pullRequestId: $id, body: $body}) { pullRequest { number } } }' -F id="$prid" -F body="$(cat /tmp/claude-1003/pr-body.md)"
```

- [ ] **Step 2: comment on #186 with the correction** (v2 → v1, terraform receipts, spec link) — so the issue record carries it even after autoclose.

- [ ] **Step 3: close #168 (verdict FINAL — family answered Q3, spec rev 2)**

Comment and close. The comment must contain, in this order: (1) the measured inventory (zero
`set_var` in `crates/`; mechanism a = waiter-less delete→create on the shared dynamodb-local;
mechanism b = two uuid-per-call sites ≈19+ tables/run) with the hardening commit SHAs;
(2) this exact claim, verbatim: **"not reproduced; two real defects found by inspection and
hardened; reopen on captured output"**; (3) reopen instructions: capture the failing test
name + full assertion output + run id BEFORE re-running, then reopen with that attached.
The close must never read as a diagnosis — "probable mechanism" appears only wearing the
word *probable*.

---

## Self-review notes (rev 2 — after cold plan review + family review, all integrated)

- Spec coverage: G1→Tasks 4-6, G2→Task 7, G3→Task 1, G4→Tasks 2-3+9, D2→Tasks 4/5/5b (three
  env worlds), D4→Task 4 (v2 + ALB guards), D5→Task 4's `load_v1_fixture` + degenerate red
  (AC4), D6→Task 8 (landed-proof, per-property, bidirectional census, response-side included),
  AC6/7→Global Constraints. Q1/Q2/Q3 are ANSWERED and folded — no task branches on an open
  question.
- Plan-review disposition: B1 (tokio dep) → Task 2 Step 1b + Global Constraints exception;
  B2/B3 (no-op sabotages) → Task 8's corrected table + softened stop-rule; M1 (Invoker
  shape) → verified `gift`/fulfillment types in Task 4; M2 (base64 constant) → corrected with
  the byte-level reason; M3 (lambda_runtime path) → lambda_http re-export named in Task 7;
  M4 (stage-absent arm) → Task 5's no-stage fixture; M5 (multi-value narrowing) → recorded in
  spec Non-goals + Task 4 note; M6 (response-side red) → Task 8 row 8; M7 (Task 3
  placeholders) → real signatures, caller counts, verbatim-name rule. Minors: line refs
  corrected (`test_link:204`, store_test `store_or_skip:30` returns bare `Store`), run-id
  command supplied, pushes added, admin fixture resource shape fixed, isBase64Encoded assert
  added, PyYAML fallback named.
- The one deliberate deviation from strict TDD: behavioral RED cannot precede implementation
  locally (no linker — measured 2026-08-07, ≥1638M to link); Task 8's batched sabotage census
  is the compensating control, and each test's red is still observed (or its exclusion
  recorded) before the PR claims coverage.
- OMBB step-5 gate disposition (rev 3): B1 → `requestContext.path` in the skeleton + a
  per-derivation update rule in Tasks 4/6/7; B2 → `load_v1_fixture` mirrored into the admin
  binary (verbatim copy — a test binary cannot import another crate's test binary, Lilith's
  rider) and both admin loads wrapped, predicate-before-substitution; M1 → the `$default`
  arm gets its own fixture, test, and sabotage row 10 (census 12: public 6 / control 3 /
  false 1 / admin 2); M2 → `gh pr edit --body` replaced with the GraphQL `updatePullRequest`
  mutation in Tasks 8/9 (fails on this box, measured); minors 1-6 and the AC7 clause all
  applied.
