# Sealed by Construction (PR1 — `dynamo`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make it structurally impossible for an AWS SDK error's unbounded `Debug` to reach the operator Discord channel, by giving `StoreError::Aws` a typed payload that cannot carry item attributes.

**Architecture:** `StoreError::Aws(String)` becomes `StoreError::Aws(AwsFault)`, where `AwsFault` is built by one extractor from `&SdkError` and carries only `op`/`code`/`message`/`request_id`/`http_status`/`retryable`. Because the variant stops accepting a `String`, the compiler forces every one of the 23 `format!("{e:?}")` capture sites to be rewritten — the change is by-construction rather than by-review. The single legitimate non-SDK message gets its own variant so no `String` arm survives on `Aws`.

**Tech Stack:** Rust 2024 workspace · `thiserror` · `aws-sdk-dynamodb` 1.119.0 · `aws-smithy-runtime-api` **1.14.0** (what `Cargo.lock` resolves) · tests with `cargo test`, plus `aws-smithy-types` / `aws-smithy-runtime-api` already present in `crates/dynamo`'s `[dev-dependencies]`.

**Spec:** `docs/superpowers/specs/2026-08-21-sealed-by-construction-design.md` — read it first; this plan argues from it.

## Global Constraints

- **This is NOT an incident.** Nothing is leaking today. No CVE language, no urgency framing, in code comments or the PR. Argue on **timing** (cost fixed and rising, blast radius currently zero traffic), never on urgency.
- **`StoreError::Aws` carries `AwsFault` ONLY.** No `String` arm, no `From<String> for AwsFault`, no `AwsFault::from_str`. *(Spec criterion ⑥ — without it the design degrades into the optional sealer it exists to abolish.)*
- **The modeled `.message()` is IN; `.item()` is NEVER.** Write this rule into `AwsFault`'s doc comment AND enforce it with a test.
- **Read, not adopt.** `set_link_thanks` must keep reading `ccf.item()` for its atomic CCF classification. Only *capturing* the item into the error type is forbidden.
- **Error classification must not change.** `is_ccf_put`, `is_ccf_update`, and the transaction-cancellation paths match on typed `as_service_error()` and run on `&SdkError` upstream of this conversion. They must remain untouched.
- **Signed commits, `code kitten <yourcodekitten@gmail.com>`, key `F2060B93112D9ACF`.** Never commit on `main`; this work lives on `kitten/sealed-by-construction`.
- **No rebase+force on this branch** — merge commits only, pending Ben's ruling on the force-push reading.
- **Local test law:** `cargo test` panics locally for store tests (repo `.cargo/config.toml` arms `DYNAMODB_LOCAL_URL`, no local DynamoDB, #133). All tests in this plan are **pure unit tests in `crates/dynamo/src/lib.rs`** and run with `cargo test -p dynamo --lib`, which needs no DynamoDB. `cargo` lives at `~/.cargo/bin` (not on PATH). Memory is tight — use `-j2`.

## File Structure

| File | Responsibility |
|---|---|
| `crates/dynamo/src/aws_fault.rs` *(create)* | The `AwsFault` type, its `Display`, and the single `from_sdk_error` extractor. One responsibility: turn an opaque SDK error into a bounded, printable fault. |
| `crates/dynamo/src/lib.rs` *(modify)* | `StoreError` variant change; 23 explicit capture sites **+ 3 `?`-dependent sites** (`:704`, `:968`, `:2220`); `mod aws_fault;` |
| `terraform/aws-lambda.tf` *(modify)* | Pin `cloudwatch_logs.retention_in_days` explicitly at the three module call sites. **The module owns the log-group resource — do not declare one.** |
| `.github/workflows/ci.yml` *(modify)* | Add `scripts/check-spec-crate-anchors.sh` to the `audit` job. |

---

### Task 1: `AwsFault` — a fault that cannot carry an item

**Files:**
- Create: `crates/dynamo/src/aws_fault.rs`
- Modify: `crates/dynamo/src/lib.rs` (add `mod aws_fault;` + re-export)
- Test: `crates/dynamo/src/aws_fault.rs` (inline `#[cfg(test)] mod tests`)

**Interfaces:**
- Produces:
  - `pub struct AwsFault` with private fields.
  - `pub fn AwsFault::from_sdk_error<E, R>(op: &'static str, e: &aws_sdk_dynamodb::error::SdkError<E, R>) -> AwsFault` where `E: aws_smithy_types::error::metadata::ProvideErrorMetadata`, `R: aws_smithy_runtime_api::client::result::CreateUnhandledError` is **not** required — see Step 3 for the exact bound set used.
  - `impl std::fmt::Display for AwsFault`.
- Consumes: nothing (first task).

- [ ] **Step 1: Write the failing test — the item must not survive extraction**

Add to `crates/dynamo/src/aws_fault.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_types::body::SdkBody;

    const SENTINEL: &str = "HB-GIFT-KEY-SENTINEL-0451";

    /// The exact payload the RECOMMENDED `ReturnValuesOnConditionCheckFailure::AllOld`
    /// line hands back on a failed conditional write of a claim carrying a revealed key.
    fn ccf_sdk_error_with_item() -> aws_sdk_dynamodb::error::SdkError<
        aws_sdk_dynamodb::operation::put_item::PutItemError,
        aws_smithy_runtime_api::http::Response,
    > {
        let ccf = aws_sdk_dynamodb::types::error::ConditionalCheckFailedException::builder()
            .item(
                "revealed_key",
                aws_sdk_dynamodb::types::AttributeValue::S(SENTINEL.to_string()),
            )
            .build();
        let op_err =
            aws_sdk_dynamodb::operation::put_item::PutItemError::ConditionalCheckFailedException(ccf);
        let raw = aws_smithy_runtime_api::http::Response::new(
            400u16.try_into().unwrap(),
            SdkBody::empty(),
        );
        aws_sdk_dynamodb::error::SdkError::service_error(op_err, raw)
    }

    #[test]
    fn item_attributes_never_reach_the_fault() {
        let fault = AwsFault::from_sdk_error("put_item", &ccf_sdk_error_with_item());
        let rendered = format!("{fault}");
        assert!(
            !rendered.contains(SENTINEL),
            "item attribute reached AwsFault Display: {rendered}"
        );
        assert!(
            !format!("{fault:?}").contains(SENTINEL),
            "item attribute reached AwsFault Debug: {fault:?}"
        );
    }

    #[test]
    fn fault_still_says_which_call_and_what_aws_said() {
        let fault = AwsFault::from_sdk_error("put_item", &ccf_sdk_error_with_item());
        let rendered = format!("{fault}");
        assert!(rendered.contains("put_item"), "lost the operation: {rendered}");
        assert!(
            rendered.contains("ConditionalCheckFailedException"),
            "lost the error code: {rendered}"
        );
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd ~/bendobundles && PATH="$HOME/.cargo/bin:$PATH" cargo test -p dynamo --lib aws_fault -j2`
Expected: FAIL to **compile** — `AwsFault` does not exist yet (`cannot find type AwsFault in this scope`). A compile failure is the correct RED here; do not proceed until you have seen it.

- [ ] **Step 3: Write the minimal implementation**

Top of `crates/dynamo/src/aws_fault.rs`:

```rust
//! A DynamoDB failure reduced to what diagnoses it — and nothing that can carry item data.
//!
//! # The rule this type exists to enforce
//!
//! The modeled `.message()` is **IN**. `.item()` is **NEVER**.
//!
//! `StoreError::Aws` used to hold `format!("{e:?}")` — an *unbounded* capture of a `Debug`
//! we do not own. `SdkError` is `#[non_exhaustive]` and **four of its five arms** carry an
//! unbounded payload: `ConstructionFailure` and `TimeoutError` hold `BoxError`,
//! `DispatchFailure` holds a `ConnectorError` that is itself a `BoxError`, and
//! `ResponseError` holds a `BoxError` beside the raw response. `ServiceError` — the modeled
//! one — is the only bounded arm. So whatever the SDK boxed, we adopted, into a value that
//! reaches the operator Discord channel. This type bounds that capture.
//!
//! Verified against `aws-smithy-runtime-api` 1.14.0, the version `Cargo.lock` resolves.
//!
//! Nothing was leaking when this was written; see the spec's honesty section.

use aws_smithy_types::error::metadata::ProvideErrorMetadata;

/// A bounded description of a failed DynamoDB call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AwsFault {
    op: &'static str,
    code: Option<String>,
    message: Option<String>,
    request_id: Option<String>,
    http_status: Option<u16>,
    retryable: bool,
}

impl AwsFault {
    /// Extract the diagnosable parts of an SDK error. Deliberately takes `&SdkError` so
    /// callers keep the original for typed classification (`is_ccf_put` and friends).
    pub fn from_sdk_error<E, R>(
        op: &'static str,
        e: &aws_sdk_dynamodb::error::SdkError<E, R>,
    ) -> Self
    where
        E: ProvideErrorMetadata + std::error::Error + 'static,
    {
        let svc = e.as_service_error();
        AwsFault {
            op,
            code: svc.and_then(|s| s.code().map(str::to_string)),
            message: svc.and_then(|s| s.message().map(str::to_string)),
            // NOT `.meta().request_id()` — that does not exist. `ErrorMetadata` exposes
            // `code`/`message`/`extra`; the `RequestId` trait lives in `aws-types`, which is
            // NOT a dependency of this crate. `extra("aws_request_id")` needs no new dep and
            // yields `None` when absent, which is the honest answer.
            request_id: svc.and_then(|s| s.meta().extra("aws_request_id").map(str::to_string)),
            http_status: None,
            // A DELIBERATE APPROXIMATION, not "the SDK's own classification" — say so, because
            // an earlier draft of this plan claimed the latter. Transport-level timeouts and
            // dispatch failures are retryable; a throttling *service* error is retryable too and
            // this does NOT catch it. Under-reporting is the safe direction (a missing
            // `[retryable]` costs a reader nothing; a false one sends them down a wrong path).
            retryable: matches!(
                e,
                aws_sdk_dynamodb::error::SdkError::TimeoutError(_)
                    | aws_sdk_dynamodb::error::SdkError::DispatchFailure(_)
            ),
        }
    }
}

impl std::fmt::Display for AwsFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} failed", self.op)?;
        if let Some(c) = &self.code {
            write!(f, " [{c}]")?;
        }
        if let Some(m) = &self.message {
            write!(f, ": {m}")?;
        }
        if let Some(s) = self.http_status {
            write!(f, " (http {s})")?;
        }
        if let Some(r) = &self.request_id {
            write!(f, " (request-id {r})")?;
        }
        if self.retryable {
            write!(f, " [retryable]")?;
        }
        Ok(())
    }
}
```

In `crates/dynamo/src/lib.rs`, directly after the existing `mod schema;`-style declarations near the top of the file, add:

```rust
mod aws_fault;
pub use aws_fault::AwsFault;
```

**Note on the `code()` value:** for a modeled DynamoDB exception, `ProvideErrorMetadata::code()` returns the shape name (e.g. `"ConditionalCheckFailedException"`), which is what `fault_still_says_which_call_and_what_aws_said` asserts. If the pinned SDK returns `None` for `code()` on the constructed fixture, fall back to matching `e.as_service_error()` on the typed enum and mapping the variant to a `&'static str` — do **not** relax the assertion.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd ~/bendobundles && PATH="$HOME/.cargo/bin:$PATH" cargo test -p dynamo --lib aws_fault -j2`
Expected: PASS, 2 tests.

- [ ] **Step 5: Commit**

```bash
cd ~/bendobundles
git add crates/dynamo/src/aws_fault.rs crates/dynamo/src/lib.rs
git commit -S -m "feat(dynamo): AwsFault — a fault that cannot carry an item"
```

---

### Task 2: `http_status`, and proof that the unbounded arms stay bounded

**Files:**
- Modify: `crates/dynamo/src/aws_fault.rs`
- Test: `crates/dynamo/src/aws_fault.rs` (same inline `tests` module)

**Interfaces:**
- Consumes: `AwsFault::from_sdk_error` from Task 1.
- Produces: no signature change; `http_status` is now populated.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module:

```rust
#[test]
fn http_status_is_captured() {
    let fault = AwsFault::from_sdk_error("put_item", &ccf_sdk_error_with_item());
    assert!(
        format!("{fault}").contains("http 400"),
        "lost the http status: {fault}"
    );
}

/// ALL FOUR unbounded arms must be exercised, not one.
///
/// An earlier draft constructed only `construction_failure` under a plural name. Its
/// rationale — "this catches a `_ => format!(..)` fallback" — was true and INSUFFICIENT:
/// it cannot catch a PER-ARM capture, and `from_sdk_error` hand-writes `ResponseError(re) =>`
/// as its own arm, the one likeliest to grow a Debug capture.
/// *An arm that has never printed is a decoration.* (OMBB, step-5 gate.)
#[test]
fn opaque_arms_do_not_leak_their_payload() {
    const OPAQUE: &str = "OPAQUE-BOXERROR-PAYLOAD-9142";

    #[derive(Debug)]
    struct Nosy;
    impl std::fmt::Display for Nosy {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{OPAQUE}")
        }
    }
    impl std::error::Error for Nosy {}

    type E = aws_sdk_dynamodb::error::SdkError<
        aws_sdk_dynamodb::operation::put_item::PutItemError,
        aws_smithy_runtime_api::http::Response,
    >;

    fn raw() -> aws_smithy_runtime_api::http::Response {
        aws_smithy_runtime_api::http::Response::new(
            500u16.try_into().unwrap(),
            aws_smithy_types::body::SdkBody::empty(),
        )
    }

    let cases: Vec<(&str, E)> = vec![
        ("ConstructionFailure", E::construction_failure(Nosy)),
        ("TimeoutError", E::timeout_error(Nosy)),
        (
            "DispatchFailure",
            E::dispatch_failure(
                aws_smithy_runtime_api::client::orchestrator::error::ConnectorError::io(
                    Box::new(Nosy),
                ),
            ),
        ),
        ("ResponseError", E::response_error(Nosy, raw())),
    ];

    for (name, e) in cases {
        let fault = AwsFault::from_sdk_error("put_item", &e);
        assert!(
            !format!("{fault}").contains(OPAQUE),
            "{name} payload reached the fault Display: {fault}"
        );
        assert!(
            !format!("{fault:?}").contains(OPAQUE),
            "{name} payload reached the fault Debug: {fault:?}"
        );
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd ~/bendobundles && PATH="$HOME/.cargo/bin:$PATH" cargo test -p dynamo --lib aws_fault -j2`
Expected: `http_status_is_captured` FAILS with "lost the http status". `opaque_arms_do_not_leak_their_payload` should already PASS across **all four** arms — Task 1 never reads them. **That is a legitimate green, and you must still run it:** it guards against both a catch-all `_` capture AND a per-arm capture added later to any one of the four.
⚠️ **If `ConnectorError::io` is not the right constructor on the pinned crate, find the one that is — do NOT drop the `DispatchFailure` case.** Quietly reducing the arm count is exactly how this test became a decoration the first time.

- [ ] **Step 3: Populate `http_status`**

In `from_sdk_error`, replace the `http_status: None,` line with:

```rust
            http_status: e.raw_response().map(|r| r.status().as_u16()),
```

and add to the imports at the top of the file:

```rust
use aws_smithy_runtime_api::client::result::SdkError as _;
```

**If `raw_response()` is not in scope on the pinned 1.14.0**, match the two variants that carry a
response directly — this is the whole fallback, not a sketch:

```rust
            http_status: match e {
                aws_sdk_dynamodb::error::SdkError::ServiceError(se) => {
                    Some(se.raw().status().as_u16())
                }
                aws_sdk_dynamodb::error::SdkError::ResponseError(re) => {
                    Some(re.raw().status().as_u16())
                }
                _ => None,
            },
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd ~/bendobundles && PATH="$HOME/.cargo/bin:$PATH" cargo test -p dynamo --lib aws_fault -j2`
Expected: PASS, 4 tests.

- [ ] **Step 5: Commit**

```bash
cd ~/bendobundles
git add crates/dynamo/src/aws_fault.rs
git commit -S -m "feat(dynamo): capture http status; pin the opaque arms with a regression test"
```

---

### Task 3: Flip the variant and let the compiler find every site (23 explicit + 3 via `?`)

**Files:**
- Modify: `crates/dynamo/src/lib.rs` (the `StoreError` enum, and every capture site the compiler rejects)
- Test: `crates/dynamo/src/lib.rs` (inline)

**Interfaces:**
- Consumes: `AwsFault`, `AwsFault::from_sdk_error` (Tasks 1–2).
- Produces: `StoreError::Aws(AwsFault)` and `StoreError::Internal(&'static str)`.

- [ ] **Step 1: Write the failing test**

Add to the existing `#[cfg(test)] mod tests` at the bottom of `crates/dynamo/src/lib.rs`:

```rust
#[test]
fn store_error_aws_cannot_carry_an_item() {
    use aws_smithy_types::body::SdkBody;
    const SENTINEL: &str = "HB-GIFT-KEY-SENTINEL-0451";

    let ccf = aws_sdk_dynamodb::types::error::ConditionalCheckFailedException::builder()
        .item(
            "revealed_key",
            aws_sdk_dynamodb::types::AttributeValue::S(SENTINEL.to_string()),
        )
        .build();
    let op_err =
        aws_sdk_dynamodb::operation::put_item::PutItemError::ConditionalCheckFailedException(ccf);
    let raw =
        aws_smithy_runtime_api::http::Response::new(400u16.try_into().unwrap(), SdkBody::empty());
    let sdk_err = aws_sdk_dynamodb::error::SdkError::service_error(op_err, raw);

    let store_err = StoreError::Aws(AwsFault::from_sdk_error("put_item", &sdk_err));
    assert!(
        !format!("{store_err}").contains(SENTINEL),
        "revealed key reached StoreError: {store_err}"
    );
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cd ~/bendobundles && PATH="$HOME/.cargo/bin:$PATH" cargo test -p dynamo --lib store_error_aws -j2`
Expected: FAIL to compile — `StoreError::Aws` still takes a `String`, so `AwsFault` is a type mismatch.

- [ ] **Step 3: Change the variant**

In `crates/dynamo/src/lib.rs`, replace:

```rust
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("dynamodb error: {0}")]
    Aws(String),
    #[error("corrupt item: {0}")]
    Corrupt(&'static str),
}
```

with:

```rust
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// A failed AWS call. Carries [`AwsFault`], **never** a raw SDK `Debug` —
    /// see `aws_fault.rs` for the rule and the reason.
    ///
    /// There is deliberately NO `String` arm and NO `From<String>`: the moment one
    /// exists, this type is a sealer nothing forces, which is the defect it exists
    /// to remove. Non-SDK messages go to [`StoreError::Internal`].
    #[error("dynamodb error: {0}")]
    Aws(AwsFault),
    #[error("corrupt item: {0}")]
    Corrupt(&'static str),
    /// An invariant this crate itself violated — never carries SDK output.
    #[error("internal: {0}")]
    Internal(&'static str),
}
```

- [ ] **Step 4: Delete the blanket `From` impl**

Remove this impl entirely (it is the unbounded capture, and leaving it defeats the plan):

```rust
impl<E: std::fmt::Debug, R: std::fmt::Debug> From<aws_sdk_dynamodb::error::SdkError<E, R>>
    for StoreError
{
    fn from(e: aws_sdk_dynamodb::error::SdkError<E, R>) -> Self {
        StoreError::Aws(format!("{e:?}"))
    }
}
```

- [ ] **Step 5: Compile and let the compiler enumerate the work**

Run: `cd ~/bendobundles && PATH="$HOME/.cargo/bin:$PATH" cargo check -p dynamo -j2 2>&1 | tee /tmp/sealed-sites.txt | tail -40`
Expected: a long list of type errors. Count them:
`grep -c '^error' /tmp/sealed-sites.txt`
**This list IS the task, and it is LONGER than the 23 explicit captures.**

- **23 explicit `format!("{e:?}")` captures** at `lib.rs` lines 338 (deleted in Step 4), 613, 658, 737, 769, 794, 888, 1149, 1260, 1319, 1369, 1489, 1534, 1633, 1718, 1836, 1921, 2008, 2163, 2725, 2777, 2994, 3017.
- 🔴 **PLUS 3 sites that never mention `format!` at all** — `lib.rs:704`, `:968`, `:2220` — which use
  `req.send().await?` and rely on the blanket `From` impl for the `?` operator. **Deleting the impl
  breaks these too.** Rewrite each as:

```rust
        req.send()
            .await
            .map_err(|e| StoreError::Aws(AwsFault::from_sdk_error("update_item", &e)))?;
```

🔴 **DO NOT "FIX" THESE BY RE-ADDING A `From<SdkError> for StoreError` IMPL.** That is the cheapest
way to make the build green and it **reintroduces the exact defect this plan exists to remove** (see
Global Constraints and spec criterion ⑥). If the error count surprises you, the plan is right and the
surprise is the point — the compiler is enumerating work that review would have missed.

- [ ] **Step 6: Rewrite each site**

For a site of the form `Err(sdk_err) => Err(StoreError::Aws(format!("{sdk_err:?}")))`, write:

```rust
            Err(sdk_err) => Err(StoreError::Aws(AwsFault::from_sdk_error("put_item", &sdk_err))),
```

Use the actual DynamoDB operation for the `op` string at each site (`"put_item"`, `"update_item"`, `"delete_item"`, `"query"`, `"scan"`, `"transact_write_items"`, `"describe_table"`, `"get_item"`, `"batch_write_item"`). For a site of the form `.map_err(|e| StoreError::Aws(format!("{e:?}")))?`, write:

```rust
            .map_err(|e| StoreError::Aws(AwsFault::from_sdk_error("query", &e)))?;
```

For the six `let err_str = format!("{sdk_err:?}");` sites (1149, 1260, 1633, 1718, 1836, 1921), delete the `err_str` binding and build the fault at the point of use, e.g.:

```rust
                Err(ClaimTxError::Store(StoreError::Aws(AwsFault::from_sdk_error(
                    "transact_write_items",
                    &sdk_err,
                ))))
```

For the **one non-SDK site** at `lib.rs:3021`:

```rust
                    return Err(StoreError::Internal("describe_table returned no table"));
```

📌 **While you are at `:1149`, fix the stale citation two lines below it.** The comment names an
**older minor of `aws-sdk-dynamodb` than the lockfile resolves** and says that version lacks
`as_transaction_canceled_exception()`. *(The stale version string is deliberately NOT reproduced
here: `scripts/check-spec-crate-anchors.sh` scans `docs/` **and** `crates/`, so quoting a bad anchor
in the instructions for fixing it makes the fix's own documentation fail the check. That has now
happened three times in this repo in one morning — it is the rule, not the exception:*
***a checked surface may not carry the string it is checking for.***) The **guidance is still correct** — that method is absent in 1.119.0 too,
verified by grepping the resolved tree — so this is a citation defect, not a correctness one.
Drop the version rather than bumping it, because the claim is not version-specific:

```rust
                // No `as_transaction_canceled_exception()` on this error type; pattern-match
                // the public enum variants directly.
```

⚠️ **Do not touch `is_ccf_put`, `is_ccf_update`, or the `as_service_error()` match arms.** They take `&SdkError` and run before this conversion. If a diff of this task touches them, it is wrong.
⚠️ **`set_link_thanks` (~`:857`) must keep `ccf.item.clone()`.** Reading the item is correct; only capturing it into the error type is forbidden.

- [ ] **Step 7: Run the whole unit suite**

Run: `cd ~/bendobundles && PATH="$HOME/.cargo/bin:$PATH" cargo test -p dynamo --lib -j2`
Expected: PASS, including `store_error_aws_cannot_carry_an_item` and the 12 pre-existing unit tests.

- [ ] **Step 8: Verify no `String` arm crept back**

Run:
```bash
cd ~/bendobundles
grep -n 'Aws(String)\|From<String>' crates/dynamo/src/lib.rs crates/dynamo/src/aws_fault.rs && echo "FAIL: a String arm exists" || echo "OK: no String arm"
grep -c 'format!("{[a-z_]*:?}")' crates/dynamo/src/lib.rs
```
Expected: `OK: no String arm`, and a `format!("{…:?}")` count of **0**.

- [ ] **Step 9: Commit**

```bash
cd ~/bendobundles
git add crates/dynamo/src/lib.rs
git commit -S -m "feat(dynamo)!: StoreError::Aws carries AwsFault, not a raw SDK Debug

The blanket From<SdkError> impl is deleted, so the compiler — not review —
forces all 23 capture sites. No String arm survives on Aws; the one
legitimate non-SDK message moves to StoreError::Internal."
```

---

### Task 4: Make log retention an explicit decision at the module call sites

**Files:**
- Modify: `terraform/aws-lambda.tf` (the three `module "lambda_*"` blocks)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: nothing consumed by later tasks.

🔴 **THIS TASK WAS REWRITTEN AFTER PLAN REVIEW. THE FIRST VERSION WOULD HAVE BROKEN `terraform apply`.**
It declared `resource "aws_cloudwatch_log_group"` for each lambda. **The
`bendoerr-terraform-modules/lambda/aws` module already declares one** —
`.terraform/modules/lambda_admin_api/main.tf:75`, `aws_cloudwatch_log_group "this"`, with
`retention_in_days = local.cloudwatch_retention_in_days`. Declaring our own would have produced a
**duplicate resource for the same log group name**.
📌 **And the premise was wrong too:** the spec claimed retention was *"managed by nothing, correct by
accident."* **It is managed** — the module's `cloudwatch_logs.retention_in_days` defaults to `30`
(`variables.tf:209`), which is exactly the live value. *OMBB grepped `terraform/*.tf`; I read the
account; **neither of us read the MODULE**, which is the thing that owns the resource.*
⇒ **What remains is genuinely smaller, and is labelled as the weaker argument it is:** a module
*default* can move under a version bump without a diff anyone reads. Pinning it explicitly at the
call site makes the value visible to a reviewer. **This is tidiness with a rationale, not a fix.**

- [ ] **Step 1: Confirm the live state and the module default before changing anything**

Run:
```bash
AWS_PROFILE=kitten-debug aws logs describe-log-groups --log-group-name-prefix /aws \
  --query 'logGroups[?contains(logGroupName,`brd-prod-ue1-bendobundles`)].[logGroupName,retentionInDays]' \
  --output text
grep -n 'retention_in_days' ~/bendobundles/terraform/.terraform/modules/lambda_admin_api/variables.tf
```
🔴 **Do NOT filter by `/aws/lambda/`. That prefix is how this was got wrong the first time** — it
excluded a group *by construction* while the command still exited 0 with a well-formed answer.
**A succeeding command can be showing a filtered slice.**

Expected, measured 2026-08-21:
```
/aws/apigateway/brd-prod-ue1-bendobundles-api-access-logs   7
/aws/lambda/brd-prod-ue1-bendobundles-admin-api           30
/aws/lambda/brd-prod-ue1-bendobundles-fulfillment         30
/aws/lambda/brd-prod-ue1-bendobundles-public-api          30
```
**Record this in the PR.** If any lambda row is not `30`, **stop** — it would mean the value is NOT
the module default and the premise has moved again.

- [ ] **Step 2: Pin the value at each of the three module call sites**

In `terraform/aws-lambda.tf`, add this argument to **each** of `module "lambda_fulfillment"`,
`module "lambda_public_api"`, and `module "lambda_admin_api"`, immediately after their `name = ...`
line:

```hcl
  # Pinned explicitly rather than inherited. The module defaults this to 30 and production is
  # already at 30, so this is a no-op today — its job is to make a module version bump that
  # changes the default show up as a diff instead of as silence. The operator ping deliberately
  # carries only a bounded AwsFault while full SDK Debug is allowed to reach CloudWatch, so the
  # lifetime of these logs is part of that argument and should be visible to a reviewer.
  cloudwatch_logs = { retention_in_days = 30 }
```

⚠️ **Do NOT touch the API Gateway access log group.** It is at 7 days, it is a different resource,
and access logs carry request paths — `/api/l/{token}` puts a bearer link token in the path, so
shorter is the safer default there. **Raising or managing it is a separate decision with its own
argument, not a side effect of this PR.**

- [ ] **Step 3: Validate**

Run:
```bash
cd ~/bendobundles/terraform && terraform fmt -check && terraform validate
```
Expected: both clean. **Do not run `terraform plan` or `apply`** — the box's default AWS identity is
OMBB's, and CI's `terraform` job is the gate for this.

- [ ] **Step 4: Commit**

```bash
cd ~/bendobundles
git add terraform/aws-lambda.tf
git commit -S -m "infra: pin lambda log retention explicitly at the module call sites

The module already manages these log groups and already defaults to 30, which is what
production has. This changes nothing today; it makes a future change to that default
visible as a diff rather than as silence."
```

---

### Task 5: Wire the crate-anchor check into CI

**Files:**
- Modify: `.github/workflows/ci.yml` (the `audit` job)

**Interfaces:**
- Consumes: `scripts/check-spec-crate-anchors.sh` (already committed on this branch).
- Produces: nothing consumed by later tasks.

**Why:** this plan's spec cited a crate version the lockfile does not resolve, and the bad anchor
**survived a revision whose entire subject was that section** — the fix landed on the count and never
reached the citation under it. Three people reading source caught it; that is an accident, not a
mechanism.

- [ ] **Step 1: Prove the script still discriminates, before trusting it in CI**

```bash
cd ~/bendobundles
./scripts/check-spec-crate-anchors.sh; echo "expect rc=0"

# Sabotage a COPY, and BUILD the bad anchor from variables so this file never contains a
# literal `crate-x.y.z` string. The script scans docs/ AND crates/, so a hard-coded sabotage
# example written here would be found by the very check it demonstrates — that happened, and
# it is the same "a checked surface may not carry the string it retired" defect this repo hit
# twice already today.
tmp=$(mktemp -d); cp -r docs scripts crates Cargo.lock "$tmp"/
printf '\nsabotage: %s-%s\n' "aws-smithy-runtime-api" "9.9.9" >> "$tmp"/docs/superpowers/specs/2026-08-21-sealed-by-construction-design.md
( cd "$tmp" && ./scripts/check-spec-crate-anchors.sh ); echo "expect rc=1"
rm -rf "$tmp"

LOCK=/nonexistent ./scripts/check-spec-crate-anchors.sh; echo "expect rc=2"
```
Expected: `0`, then `1`, then `2`. **If the sabotage run does not go RED, stop** — the check is a
comment and wiring it in would be worse than not having it.

- [ ] **Step 2: Add the step to the `audit` job**

In `.github/workflows/ci.yml`, in the `audit` job, immediately after the
`- run: ./scripts/no-legacy-http-stack.sh` line, add:

```yaml
      # Specs and plans cite crate versions. Those citations rot silently and read current
      # forever. This asserts every `crate-x.y.z` anchor in docs/superpowers matches what
      # Cargo.lock actually resolves.
      - run: ./scripts/check-spec-crate-anchors.sh
```

- [ ] **Step 3: Commit**

```bash
cd ~/bendobundles
git add .github/workflows/ci.yml
git commit -S -m "ci: assert spec crate anchors match the lockfile"
```

---

### Task 6: Retire the three `cargo audit` ignores whose condition has fired

**Files:**
- Modify: `.cargo/audit.toml`

**Interfaces:** consumes nothing; produces nothing.

🔴 **This is fallout from PR #199, merged this morning, and the file predicted it.** `.cargo/audit.toml`
says in its own header:

> *"a retire condition that has FIRED is not stale documentation, it is a live hole: the advisory can
> come back through a different version and this list will wave it through silently."*

Three ignores are parked on the condition *"retire when an SDK bump drops the legacy TLS chain from
the lock."* **#199 dropped it.** Measured 2026-08-21 — `hyper-rustls 0.24` → **0**, `rustls 0.21` →
**0**, `rustls-webpki 0.101` → **0** occurrences in `Cargo.lock`. ⇒ `RUSTSEC-2026-0104`,
`RUSTSEC-2026-0098` and `RUSTSEC-2026-0099` now suppress those advisory IDs **unconditionally, for
every version this repo will ever resolve.** *(Found by OMBB auditing his own checker for the
narrowness he had criticised in Lilith's — #202.)*
⚠️ **`RUSTSEC-2026-0002` (lru 0.13) STAYS.** Its condition has NOT fired: `lru 0.13` is still in the
lock (measured, 1 occurrence). **Do not "tidy" it out with the others.**

- [ ] **Step 1: Re-measure before removing — the premise must still hold**

```bash
cd ~/bendobundles
for c in "hyper-rustls 0.24" "rustls 0.21" "rustls-webpki 0.101" "lru 0.13"; do
  n=${c%% *}; v=${c##* }
  hits=$(awk -v k="name = \"$n\"" '$0==k{w=1;next} w&&/^version = /{gsub(/version = |"/,"");print;w=0}' Cargo.lock | grep -c "^$v")
  echo "$c -> $hits"
done
```
Expected: the first three `0`, and **`lru 0.13 -> 1`**. **If `lru` reads 0, stop** — that entry would
also need retiring and this task's scope has changed.

- [ ] **Step 2: Remove the three fired entries, keep `lru`, and record why**

Edit `.cargo/audit.toml` so `ignore` contains only the `lru` entry, and replace the legacy-TLS
comment block with:

```toml
    # The three legacy-TLS ignores (CRL parsing panic + two name-constraint advisories)
    # were RETIRED on 2026-08-21: PR #199 dropped the legacy chain, so hyper-rustls 0.24,
    # rustls 0.21 and rustls-webpki 0.101 are all at 0 occurrences in Cargo.lock. Their
    # retire condition had fired, which per this file's own header is a live hole and not
    # stale documentation — `ignore` matches on advisory ID with no version awareness, so
    # they would have waved those IDs through at ANY future version.
    # scripts/no-legacy-http-stack.sh now asserts that chain stays out.
```

Also update the header's dated verification line — it says the four entries were verified unfired on
2026-08-08, which is **no longer true** of three of them.

- [ ] **Step 3: Prove the suite still passes without them**

```bash
cd ~/bendobundles && PATH="$HOME/.cargo/bin:$PATH" cargo audit
```
Expected: **no new findings.** If any of the three advisories now fires, **stop and report** — that
would mean the chain is back and `scripts/no-legacy-http-stack.sh` should have caught it first.

- [ ] **Step 4: Commit**

```bash
cd ~/bendobundles
git add .cargo/audit.toml
git commit -S -m "audit: retire three ignores whose condition fired in #199

hyper-rustls 0.24, rustls 0.21 and rustls-webpki 0.101 are all at 0 occurrences
now. Per this file's own header a fired retire condition is a live hole, since
ignore matches on advisory ID with no version awareness. The lru entry stays --
its condition has not fired."
```

---

## Self-Review

**1. Spec coverage.**
- Honesty framing → Global Constraints (no-incident rule) ✅
- L1 bounded capture → Tasks 1–3 ✅
- Criterion ① RED-first test → Task 1 Step 2, Task 3 Step 2 ✅
- Criterion ④ classification untouched → Task 3 Step 6 guard rails ✅
- Criterion ⑥ no `String` arm → Task 3 Steps 3, 8 ✅
- `AwsFault` shape incl. `http_status`/`retryable` → Tasks 1–2 ✅
- Read-not-adopt at `:857` → Task 3 Step 6 ✅
- Retention pin → Task 4 ✅ *(reduced after review: the module owns the resource and already
  defaults to 30, so the original "declare four log groups" task would have collided and its
  premise was false)*
- Crate-anchor drift → Task 5 ✅
- Fired audit ignores (fallout from #199) → Task 6 ✅ *(off-plan find, OMBB #202)* *(added after review — the defect it catches occurred in this
  spec and survived a revision of the very section it was in)*
- **All four** opaque arms → Task 2 `opaque_arms_do_not_leak_their_payload` ✅ *(was 1-of-4 under a plural name until OMBB's step-5 gate)*
- **L2/L3 (`steam-client`, sealed payload type) → NOT IN THIS PLAN.** Deliberate: different crate, and it is OMBB's design and gated on his read. **Filed as a follow-up issue with the agreed design before this PR merges** — an unrecorded deferral is indistinguishable from an oversight.

**1b. Plan-review findings folded in (2026-08-21).** Four blockers found by reviewing this plan
cold against the codebase, all in my own draft: `ErrorMetadata::request_id()` **does not exist**
(Task 1) · deleting the `From` impl also breaks **3 `?`-dependent sites** the 23-line list never
mentioned, and the cheapest repair is the one thing the plan forbids (Task 3) · Task 4 declared a
log-group resource **the module already declares** and rested on a **false** "managed by nothing"
premise · Task 2's fallback snippet was **malformed code** a subagent would have pasted verbatim.
*A plan review that found nothing would have been the failed one.*

**2. Placeholder scan.** No TBDs. Every code step carries real code. The two "if the pinned SDK differs" notes (Task 1 Step 3, Task 2 Step 3) give an exact fallback rather than "handle appropriately", and both forbid relaxing the assertion.

**3. Type consistency.** `AwsFault::from_sdk_error(op, &e)` is used with that exact signature in Tasks 1, 2, 3. `StoreError::Internal(&'static str)` is defined in Task 3 Step 3 and used in Step 6. `AwsFault` is re-exported in Task 1 so `lib.rs` can name it unqualified in Task 3.
