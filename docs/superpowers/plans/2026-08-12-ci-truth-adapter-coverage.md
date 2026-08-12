# CI-Truth: Adapter Coverage + Complete Failure Census — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Put the `lambda_http` event-translation layer of both API crates under test with production-shaped (REST v1) fixtures, make CI report a complete failure census, and harden the two probable flake mechanisms behind #168.

**Architecture:** New dedicated test binaries (`tests/adapter_test.rs`) per API crate feed checked-in API-Gateway **v1** proxy-event JSON through `lambda_http::request::from_str` (request translation) and `lambda_http::Adapter::from(router)` (response translation), then into the same `pub fn router(...)` constructors the existing 109 tests use. The stage env flag `AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH` is set once per binary via `std::sync::Once` + edition-2024 `unsafe { set_var }`; an env-free sibling binary proves the flag is load-bearing. CI is the test oracle (this box cannot link test binaries — banked measurement, `cargo test` LINK ≥1638M); local gate is `cargo check`/`clippy` at `-j 1`.

**Tech Stack:** Rust edition 2024 · axum 0.8.9 · lambda_http 1.3.0 (default features: `apigw_rest` first in the deserializer, `pass_through` OFF) · tower 0.5.3 `ServiceExt::oneshot` · dynamodb-local (CI service container, `store_or_skip` idiom) · GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-08-12-ci-truth-adapter-coverage.md` (this plan implements it; the spec's D-numbers are referenced below).

## Global Constraints

- Commits are GPG-signed (`git commit -S`), author `code kitten <yourcodekitten@gmail.com>`. Never force-push. Space out pushes (throttle).
- **The box cannot link test binaries.** Local verification = `cargo check -p <crate> --tests -j 1` and `cargo clippy -p <crate> --all-targets -j 1 -- -D warnings`. Behavior verification = CI on the draft PR (CI runs on `pull_request` only). "Run test, expect FAIL/PASS" steps below therefore name **which oracle** (check vs CI).
- `unsafe { std::env::set_var }` appears ONLY inside the `Once`-guarded `init()` of the two adapter binaries — never in `api_test.rs`, never mid-test (edition-2024 racy; spec D2).
- New store-backed tests use table prefixes **`t-pubadp-`** / **`t-admadp-`** — NOT `t-adm-`/`t-pub-` (adapter binaries run in parallel with `api_test.rs` binaries against the same dynamodb-local; a shared prefix + same test name would collide).
- Fixture JSON lives at `crates/<crate>/tests/fixtures/*.json`, loaded with `include_str!` (repo idiom).
- Existing `api_test.rs` files: do not modify except where Task 3 names exact lines. The 109 existing tests' isolation profile must not change (spec AC6).
- Branch: `kitten/ci-truth-adapter` on `yourcodekitten/bendobundles`. PR will carry `Fixes #185`, `Fixes #186`; #168 is closed manually with a verdict comment (Task 9).

## File Structure

- Modify `.github/workflows/ci.yml:36` — one line (Task 1).
- Modify `crates/dynamo/src/lib.rs:2895-2948` — `create_table_for_tests` waiter (Task 2).
- Modify `crates/admin-api/tests/api_test.rs:1299,:1598` — uuid-table leak (Task 3).
- Create `crates/public-api/tests/adapter_test.rs` + `crates/public-api/tests/fixtures/{apigw_v1_fallback.json, apigw_v1_link_unknown.json, apigw_v1_multi_value_query.json, apigw_v1_base64_thanks.json, apigw_v2_guard.json}` (Task 4).
- Create `crates/public-api/tests/adapter_stage_control_test.rs` (Task 5).
- Create `crates/admin-api/tests/adapter_test.rs` + `crates/admin-api/tests/fixtures/{apigw_v1_note_with_cookie.json, apigw_v1_login.json}` (Tasks 6-7).
- No `Cargo.toml` changes: `lambda_http` is a normal dep of both API crates; `tower`, `serde_json`, `aws-config` already present where needed. **Exception:** admin-api's dev-deps must gain nothing — verify at Task 6 Step 2 (`cargo check` says so).

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

- [ ] **Step 3: Commit**

```bash
cd ~/bendobundles && python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml')); print('YAML OK')" \
  && git add .github/workflows/ci.yml \
  && git commit -S -m "ci: --no-fail-fast so a two-crate breakage reports both crates (#185)"
```

---

### Task 2: `create_table_for_tests` waits for ACTIVE and drains DELETING (#168 hardening, mechanism a)

**Files:**
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

(If `store_or_skip` in store_test.rs has a different name/shape, adapt the guard but keep the three-creates + immediate-GSI-query body. Read `crates/dynamo/tests/store_test.rs:40-70` first.)

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

Notes for the implementer: `attr`/`key`/`gsi` closures already exist above this block — keep them. The exact service-error predicate name may differ by SDK version (`is_resource_in_use_exception` on the create-table error type); `cargo check` will name it — fix to what the SDK exposes, do NOT string-match on `format!("{e:?}")`. Also update the doc comment above the fn (`:2890-2894`) — it currently documents the discard-and-return behavior; describe the drain+waiter instead.

- [ ] **Step 4: Local gate**

Run: `cd ~/bendobundles && cargo check -p dynamo --tests -j 1 && cargo clippy -p dynamo --all-targets -j 1 -- -D warnings`
Expected: clean. (CI proves behavior on the draft PR after Task 4 opens it.)

- [ ] **Step 5: Commit**

```bash
cd ~/bendobundles && cargo check -p dynamo --tests -j 1 \
  && git add crates/dynamo/src/lib.rs crates/dynamo/tests/store_test.rs \
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

- [ ] **Step 1: Read the two call sites**

Run: `cd ~/bendobundles && grep -n 'uid\[\.\.' crates/admin-api/tests/api_test.rs`
Read ±20 lines around each hit. Determine why uuid was used: if the same *stable* name would collide (two tests sharing one helper), the fix is a caller-supplied stable name per test; if it's just habit, the fix is direct.

- [ ] **Step 2: Replace with stable per-test names**

At `:1272-1310` (`test_app_with_call_invoker`) change the helper to take the test name and pass it through, e.g.:

```rust
// BEFORE (approx):
//   let uid = uuid::Uuid::new_v4().simple().to_string();
//   let store = store_or_skip(&format!("sc{}", &uid[..10])).await?;
// AFTER:
async fn test_app_with_call_invoker(test: &str, ...) -> ... {
    let store = store_or_skip(&format!("sc-{test}")).await?;
    ...
}
```

and update each caller to pass its own test fn name (they are unique in the file — the same guarantee `store_or_skip("t-adm-{test}")` already relies on). Do the same for the `steam{}` site at `:1598`. Table names must stay ≤255 chars and `[a-zA-Z0-9_.-]` — test names in this file satisfy both.

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
    "identity": { "sourceIp": "203.0.113.10", "userAgent": "adapter-test" },
    "resourcePath": "/api/{proxy+}",
    "httpMethod": "GET",
    "apiId": "wt6mne2s9k"
  },
  "body": null,
  "isBase64Encoded": false
}
```

`apigw_v1_link_unknown.json` — same skeleton, with:
- `"path": "/api/l/definitely-not-a-token"`, `"pathParameters": { "proxy": "l/definitely-not-a-token" }` (this mirrors the manual prod probe from #186's interim mitigation).

`apigw_v1_multi_value_query.json` — same skeleton, with:
- `"path": "/api/l/definitely-not-a-token"`, matching `pathParameters`,
- `"queryStringParameters": { "a": "2", "b": "3" }`,
- `"multiValueQueryStringParameters": { "a": ["1", "2"], "b": ["3"] }`.

`apigw_v1_base64_thanks.json` — same skeleton, with:
- `"path": "/api/l/definitely-not-a-token/thanks"`, `"pathParameters": { "proxy": "l/definitely-not-a-token/thanks" }`,
- `"httpMethod": "POST"` (both top-level and in `requestContext`),
- headers gain `"Content-Type": "application/json"` (and mirror it into `multiValueHeaders`),
- `"body": "eyJub3RlIjoidGhhbmsgeW91IGJlbiDinaEifQ=="` (base64 of `{"note":"thank you ben ❤"}` — non-ASCII on purpose: it proves byte-level decode, not just ASCII luck),
- `"isBase64Encoded": true`.

First verify the base64 is right: `python3 -c "import base64; print(base64.b64encode('{\"note\":\"thank you ben ❤\"}'.encode()).decode())"` and paste the output — do not trust the value above without running that.

`apigw_v2_guard.json` — copy `~/.cargo/registry/src/*/lambda_http-1.3.0/tests/data/apigw_v2_proxy_request_minimal.json` verbatim, then set `"rawPath": "/api/l/x"`, `"routeKey": "$default"`. (v2 is identified by `version: "2.0"` + `rawPath`; it must keep parsing as v2 — spec D4.)

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

struct NoInvoker;
#[async_trait::async_trait]
impl public_api::Invoker for NoInvoker {
    async fn fire(&self, _req: domain::FulfillRequest) -> Result<(), String> {
        Err("adapter tests never invoke".into())
    }
    async fn call(&self, _req: domain::FulfillRequest) -> Result<domain::FulfillResponse, String> {
        Err("adapter tests never invoke".into())
    }
}
// NOTE for implementer: check the real `Invoker` trait shape in
// crates/public-api/src/lib.rs (name, methods, types) and mirror it exactly —
// or reuse an existing test mock if one is importable. api_test.rs::MockInvoker
// (public api_test.rs:105-140) is the reference; copy its impl, don't invent one.

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
    let req = lambda_http::request::from_str(include_str!("fixtures/apigw_v1_fallback.json"))
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
    let req = lambda_http::request::from_str(include_str!("fixtures/apigw_v1_link_unknown.json"))
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
    let req = lambda_http::request::from_str(include_str!("fixtures/apigw_v1_multi_value_query.json"))
        .expect("v1 fixture must translate");
    let qs = req.query_string_parameters();
    assert_eq!(qs.all("a").expect("a present"), vec!["1", "2"]);
    assert_eq!(qs.first("b"), Some("3"));
    let query = req.uri().query().expect("query must be reconstructed");
    assert!(query.contains("a=1") && query.contains("a=2") && query.contains("b=3"),
        "reconstructed URI must carry every value: got {query}");
}

/// isBase64Encoded body is DECODED by translation (to bytes, non-ASCII intact), and
/// axum's Json extractor parses it — proven by reaching the handler's own link lookup
/// (unknown-link 404) instead of a 4xx extractor refusal. Validation precedes the
/// link read in handle_post_thanks, so a non-empty note is required to get there.
#[tokio::test]
async fn v1_translation_decodes_base64_body_to_reach_json_extractor() {
    init_stage_env();
    let Some(store) = store_or_skip("b64-thanks").await else { return };
    let req = lambda_http::request::from_str(include_str!("fixtures/apigw_v1_base64_thanks.json"))
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
```

Implementer notes: (1) exact trait paths (`public_api::Invoker`, `domain::FulfillRequest/Response`) must be read from `crates/public-api/src/lib.rs` and mirrored from `crates/public-api/tests/api_test.rs:105-140`'s `MockInvoker` — copy that mock's shape verbatim; (2) `RequestContext` variant names come from `lambda_http::request` — `cargo check` will name them; (3) if `async_trait` isn't in public-api's dev-deps, reuse whatever api_test.rs uses (it defines mocks, so the pattern exists — do what it does).

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

Run: `cd ~/bendobundles && gh pr checks --watch` then fetch the test job log and confirm the census names the binary:
`gh run view <run-id> --log | grep -E 'Running.*adapter_test|test result'`
Expected: a `Running tests/adapter_test.rs` line for public-api, all its tests listed pass, 0 fail, and the pre-existing suites all present (per-suite counts read from the log, per #185's lesson — never infer from the green check alone).

---

### Task 5: the stage flag is load-bearing — env-free control binary (spec D3 third bullet)

**Files:**
- Create: `crates/public-api/tests/adapter_stage_control_test.rs`

**Interfaces:**
- Consumes: `apigw_v1_fallback.json` + `apigw_v1_link_unknown.json` from Task 4 (same files — that's the point: same input, different env, different verdict).

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
```

(No router/oneshot needed: the path assertion alone proves the divergence, and a storeless test keeps the control binary trivial. The precondition assert makes the control self-checking — if ambient env ever leaks in, it reports "NOT MEASURED" loudly instead of passing vacuously.)

- [ ] **Step 2: Local gate + commit + push (throttled: batch with next task's push if < 10 min apart)**

```bash
cd ~/bendobundles && cargo check -p public-api --tests -j 1 \
  && git add crates/public-api/tests/adapter_stage_control_test.rs \
  && git commit -S -m "test: env-free control — without the stage flag, translation prepends /live and routing breaks

Same fixture as the flag-set binary, opposite verdict: the pair proves the
env var is load-bearing rather than decorative (spec D3)."
```

- [ ] **Step 3: CI check as in Task 4 Step 5** — both new binaries must appear in the census.

---

### Task 6: admin-api adapter test — cookie-carried session through translation (spec D3 second bullet)

**Files:**
- Create: `crates/admin-api/tests/adapter_test.rs`
- Create: `crates/admin-api/tests/fixtures/apigw_v1_note_with_cookie.json`

**Interfaces:**
- Consumes: `admin_api::router(store, invoker, admin_hash: String, steam: Option<Arc<SteamClient>>) -> Router` (`crates/admin-api/src/lib.rs:78`); helpers mirrored from `crates/admin-api/tests/api_test.rs`: `store_or_skip` (`:28-56`, reprefix `t-admadp-`), `test_admin_hash` (`:78-88`), `admin_login` (`:138-178`), `MockAdminInvoker` (`:92-125`), `test_link` (near `:341-418` — read it), plus `body_json` (`:129`).
- Produces: `apigw_v1_login.json` skeleton reused by Task 7.

**Flow:** mint a real session via direct-router `admin_login` (that half is already covered by api_test.rs — not the subject), then send the note POST **through translation**: fixture carries `Cookie: session=__SESSION__` and `X-Admin-Request: 1` headers; the test substitutes the real session into the JSON string before `from_str`. Assert the note landed (read back through the store or a direct-router GET) — proving the Cookie header and CSRF header both survive translation and satisfy the middleware (`crates/admin-api/src/lib.rs:145-194`).

- [ ] **Step 1: Fixture**

`apigw_v1_note_with_cookie.json` — Task 4's v1 skeleton with:
- `"resource": "/admin/api/{proxy+}"`, `"resourcePath": "/admin/api/{proxy+}"`,
- `"path": "/admin/api/links/fixture-tok/note"`, `"pathParameters": { "proxy": "links/fixture-tok/note" }`,
- `"httpMethod": "POST"` (both places),
- headers (mirrored into `multiValueHeaders`): `"Content-Type": "application/json"`, `"Cookie": "session=__SESSION__"`, `"X-Admin-Request": "1"` — read the exact CSRF header name/value the middleware requires from `crates/admin-api/src/lib.rs:160` and `:184` first and use THAT, not this guess,
- `"body": "{\"gift_note\":\"from the adapter side\"}"`, `"isBase64Encoded": false`.

- [ ] **Step 2: Test**

```rust
//! Adapter-boundary tests for admin-api. Same design as public-api's
//! adapter_test.rs (see its header comment for the Once/env rationale — spec D2).

// HELPERS — copy, do not invent (the repo has no tests/common; duplication is the idiom):
//   init_stage_env()        → verbatim from crates/public-api/tests/adapter_test.rs (Task 4)
//   store_or_skip()         → crates/admin-api/tests/api_test.rs:28-56, table prefix
//                             changed to format!("t-admadp-{test}") — ONLY that change
//   test_admin_hash()       → api_test.rs:78-88 verbatim
//   MockAdminInvoker        → api_test.rs:92-125 verbatim
//   admin_login()           → api_test.rs:138-178 verbatim
//   body_json()             → api_test.rs:129-134 verbatim
//   test_link()             → find with `grep -n 'fn test_link' api_test.rs`, copy verbatim

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

    let fixture = include_str!("fixtures/apigw_v1_note_with_cookie.json")
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

Implementer notes: `test_link`'s exact shape and `Link.gift_note`'s field name must be read from api_test.rs / `crates/domain/src/lib.rs` — if the field differs (e.g. accessor method), assert through whatever api_test.rs's own note tests assert through (find them: `grep -n 'note' crates/admin-api/tests/api_test.rs`). The 200-vs-401 distinction is the test's teeth: sabotage in Task 8 strips the Cookie from the fixture and must produce 401, observed red.

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

`apigw_v1_login.json` — the v1 skeleton with `"path": "/admin/api/login"`, `"pathParameters": { "proxy": "login" }`, `"httpMethod": "POST"`, `Content-Type: application/json` header, `"body": "{\"password\":\"pw\"}"`, `"isBase64Encoded": false`.

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

    let payload: lambda_http::request::LambdaRequest =
        serde_json::from_str(include_str!("fixtures/apigw_v1_login.json"))
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
}
```

Implementer notes: `LambdaEvent`/`Context` paths and whether `Adapter` implements `tower::Service` directly (needing `.oneshot` from ServiceExt) are cargo-check-verified; if `Context::default()` is not available, build the minimal Context the constructor requires (`lambda_runtime` types.rs:66,90-96 documents `LambdaEvent::new`). If `serde_json::to_value(&resp)` fails because `LambdaResponse` isn't `Serialize` in this direction, serialize via the type the runtime actually returns to AWS — find `into_response`/serialization in lambda_http `response.rs` and assert on that exact struct. Header-name case in the serialized map is whatever `http::HeaderMap` yields (lowercase) — the test uses `"set-cookie"` lowercase for that reason; verify against the actual output and pin what's real.

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

In one commit, make each family wrong in the way its test claims to detect:
- `apigw_v1_fallback.json` + `apigw_v1_link_unknown.json`: change `"stage": "live"` → `"stage": "sabotage"` AND path `/api/l/definitely-not-a-token` → `/api/x/definitely-not-a-token` (path tests must fail on path, not luck).
- `apigw_v1_multi_value_query.json`: drop `"a": ["1", "2"]` to `["2"]`.
- `apigw_v1_base64_thanks.json`: set `"isBase64Encoded": false` (body stays base64 text → Json extractor must refuse → the unknown-link assertion fails).
- `apigw_v2_guard.json`: change `"version": "2.0"` to `"version": "1.0"` (guard must detect arm change).
- `apigw_v1_note_with_cookie.json`: delete the `Cookie` header entirely (expect 401, so the 200 assertion fails).

```bash
cd ~/bendobundles \
  && git add crates/*/tests/fixtures/ \
  && git commit -S -m "SABOTAGE (will be reverted): every adapter fixture wrong in its own dimension — observing the RED census (spec D6)" \
  && git push
```

- [ ] **Step 2: Read the red census from the CI log**

Run: `gh pr checks --watch` (expect the test job RED), then pull the log and verify EVERY sabotaged test appears as FAILED, each with an assertion message matching its dimension (path, multi-value, base64, discriminator, cookie). Record the run URL + the per-test failure list — it goes in the PR body as the D6 evidence table.
Expected: ≥6 named failures across 3 adapter binaries in ONE run — the `--no-fail-fast` census working as designed. If any sabotaged test still PASSES, that test is vacuous: STOP, fix the test (not the sabotage), re-run this task for that test.

- [ ] **Step 3: Revert**

```bash
cd ~/bendobundles && git revert --no-edit HEAD && git push
```

Expected: CI green again; `git diff main...HEAD -- crates/*/tests/fixtures/` shows the fixtures at their Task 4/6/7 content.

- [ ] **Step 4: Record the evidence**

Edit the PR body: add a "Sabotage census (D6)" section — table of test → sabotage → observed failure line → link to the red run. This is what review passes and OMBB's gate will check.

---

### Task 9: bookkeeping — PR ready, #168 verdict, #186 correction

**Files:** none (GitHub state only)

- [ ] **Step 1: PR ready for review**

```bash
cd ~/bendobundles && gh pr ready \
  && gh pr edit --body "$(cat <<'EOF'
[Write the final body: what shipped per issue (#186 adapter coverage: request-side x4 + control binary + response-side x1 + v2 guard; #185 one line; #168 hardening x2), the v1-not-v2 correction with terraform receipts, the sabotage census table from Task 8, per-suite counts read from the green run's log, and the spec/plan paths. Fixes #185. Fixes #186.]
EOF
)"
```

- [ ] **Step 2: comment on #186 with the correction** (v2 → v1, terraform receipts, spec link) — so the issue record carries it even after autoclose.

- [ ] **Step 3: #168 per the family verdict (spec Q3)**

If family agreed close-on-hardening: comment with the measured inventory (no set_var; the two mechanisms; commits), state plainly "probable-mechanism hardening, NOT a reproduced diagnosis", give reopen-on-captured-output instructions, close. If family said keep open: post the same comment, leave open, add to the PR body that #168 remains open pending reproduction.

---

## Self-review notes (run before handing to implementation-plan-review)

- Spec coverage: G1→Tasks 4-6, G2→Task 7, G3→Task 1, G4→Tasks 2-3+9, D4→Task 4 (v2 guard), D6→Task 8, AC6→Global Constraints (separate binaries, distinct prefixes). Q1/Q2/Q3 answers from family review get folded in before execution; the plan encodes the spec's stated leans.
- Types: `router` signatures copied from source (`public-api lib.rs:162`, `admin-api lib.rs:78`); mocks explicitly mirrored from api_test.rs rather than invented; every "the SDK will name it" spot is marked as a cargo-check-resolved step, never silent.
- The one deliberate deviation from strict TDD: behavioral RED cannot precede implementation locally (no linker); Task 8's batched sabotage census is the compensating control, and each test's red is still observed before the PR claims coverage.
