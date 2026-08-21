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
| `crates/dynamo/src/lib.rs` *(modify)* | `StoreError` variant change; the 23 capture sites; `mod aws_fault;` |
| `terraform/aws-cloudwatch-logs.tf` *(create)* | Pin `retention_in_days` on the three lambda log groups, so L1's "CloudWatch is the safe surface" rests on managed config. |

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
//! we do not own. `SdkError` is `#[non_exhaustive]` and three of its five arms carry
//! opaque payloads (`ConstructionFailure`/`TimeoutError` hold `BoxError`, `DispatchFailure`
//! holds `ConnectorError`), so whatever the SDK puts there, we adopted — into a value that
//! reaches the operator Discord channel. This type bounds that capture.
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
            request_id: svc.and_then(|s| s.meta().request_id().map(str::to_string)),
            http_status: None,
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

/// The three arms that carry payloads we do not control must not smuggle their
/// opaque Debug into the fault. This is the arm that needs no future author to go wrong.
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

    let e: aws_sdk_dynamodb::error::SdkError<
        aws_sdk_dynamodb::operation::put_item::PutItemError,
        aws_smithy_runtime_api::http::Response,
    > = aws_sdk_dynamodb::error::SdkError::construction_failure(Nosy);

    let fault = AwsFault::from_sdk_error("put_item", &e);
    assert!(
        !format!("{fault}").contains(OPAQUE),
        "ConstructionFailure payload reached the fault: {fault}"
    );
    assert!(
        !format!("{fault:?}").contains(OPAQUE),
        "ConstructionFailure payload reached the fault Debug: {fault:?}"
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd ~/bendobundles && PATH="$HOME/.cargo/bin:$PATH" cargo test -p dynamo --lib aws_fault -j2`
Expected: `http_status_is_captured` FAILS with "lost the http status". `opaque_arms_do_not_leak_their_payload` should already PASS — Task 1 never reads those arms. **That is a legitimate green, and you must still run it:** it is a regression guard, and its value is that it fails if someone later "improves" the extractor by adding a `_ => format!("{e:?}")` fallback.

- [ ] **Step 3: Populate `http_status`**

In `from_sdk_error`, replace the `http_status: None,` line with:

```rust
            http_status: e.raw_response().map(|r| r.status().as_u16()),
```

and add to the imports at the top of the file:

```rust
use aws_smithy_runtime_api::client::result::SdkError as _;
```

**If `raw_response()` is not in scope on the pinned 1.14.0**, obtain the status from the service-error path instead:

```rust
            http_status: svc.and_then(|s| s.meta().code()).and(None).or_else(|| {
                match e {
                    aws_sdk_dynamodb::error::SdkError::ServiceError(se) => {
                        Some(se.raw().status().as_u16())
                    }
                    aws_sdk_dynamodb::error::SdkError::ResponseError(re) => {
                        Some(re.raw().status().as_u16())
                    }
                    _ => None,
                }
            }),
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

### Task 3: Flip the variant and let the compiler find all 23 sites

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
**This list IS the task.** The 23 known capture sites are at `lib.rs` lines 338 (deleted in Step 4), 613, 658, 737, 769, 794, 888, 1149, 1260, 1319, 1369, 1489, 1534, 1633, 1718, 1836, 1921, 2008, 2163, 2725, 2777, 2994, 3017.

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

### Task 4: Pin CloudWatch log retention

**Files:**
- Create: `terraform/aws-cloudwatch-logs.tf`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: managed `aws_cloudwatch_log_group` resources for the three lambdas.

**Why this is in this PR:** L1's mitigation is *"full Debug may go to CloudWatch, never to Discord."* That argument rests on the log group's retention. Measured 2026-08-21: the three groups are at **30 days** in the account, but `terraform/` sets no retention at all — so the safe state is **correct by accident and managed by nothing.** A recreate lands at Never Expire and nobody is told.

- [ ] **Step 1: Confirm the current live state before changing anything**

Run:
```bash
AWS_PROFILE=kitten-debug aws logs describe-log-groups \
  --log-group-name-prefix /aws/lambda/brd-prod-ue1-bendobundles \
  --query 'logGroups[].[logGroupName,retentionInDays]' --output text
```
Expected: three rows, each `30`. **Record the output in the PR.** If any row differs, stop and report — the spec's premise has moved.

- [ ] **Step 2: Write the terraform**

Create `terraform/aws-cloudwatch-logs.tf`:

```hcl
# Log retention is load-bearing, not housekeeping.
#
# The operator ping deliberately carries only a bounded `AwsFault`; the full SDK Debug is
# allowed to reach CloudWatch instead. That split is only defensible while these groups have
# a retention policy someone decided on. Measured 2026-08-21: all three were already at 30
# days in the account while terraform set nothing — correct by accident. A recreate would
# have landed at Never Expire silently.
#
# 30 matches what production already had, so applying this is a no-op today and a guarantee
# tomorrow.
locals {
  lambda_log_groups = toset(["admin-api", "public-api", "fulfillment"])
}

resource "aws_cloudwatch_log_group" "lambda" {
  for_each          = local.lambda_log_groups
  name              = "/aws/lambda/${var.name_prefix}-${each.value}"
  retention_in_days = 30
}
```

- [ ] **Step 3: Confirm the name prefix variable actually resolves**

Run:
```bash
cd ~/bendobundles/terraform
grep -n 'name_prefix' tf-variables.tf *.tf | head
```
Expected: a `var.name_prefix` (or equivalent) exists and composes to `brd-prod-ue1-bendobundles`. **If the variable has a different name, use the real one** — do not invent one. Cross-check against the live group names from Step 1.

- [ ] **Step 4: Validate**

Run: `cd ~/bendobundles/terraform && terraform fmt -check && terraform validate`
Expected: both clean. **Do not run `terraform plan` or `apply`** — the box's default AWS identity is OMBB's and plan/apply is not this task's job; CI's `terraform` job validates.

⚠️ **These groups already exist in the account.** A future `apply` will need `terraform import` for each, or it will fail with `ResourceAlreadyExistsException`. **Say so in the PR body** — do not leave it for whoever applies.

- [ ] **Step 5: Commit**

```bash
cd ~/bendobundles
git add terraform/aws-cloudwatch-logs.tf
git commit -S -m "infra: manage lambda log retention explicitly (30d, matching production)

L1 lets full SDK Debug reach CloudWatch instead of Discord. That split rests on
retention, which was set to 30d in the account and to nothing in terraform.
Correct by accident is not correct. Requires terraform import; see PR body."
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
- Retention pin → Task 4 ✅
- Three opaque arms → Task 2 `opaque_arms_do_not_leak_their_payload` ✅
- **L2/L3 (`steam-client`, sealed payload type) → NOT IN THIS PLAN.** Deliberate: different crate, and it is OMBB's design and gated on his read. **Filed as a follow-up issue with the agreed design before this PR merges** — an unrecorded deferral is indistinguishable from an oversight.

**2. Placeholder scan.** No TBDs. Every code step carries real code. The two "if the pinned SDK differs" notes (Task 1 Step 3, Task 2 Step 3) give an exact fallback rather than "handle appropriately", and both forbid relaxing the assertion.

**3. Type consistency.** `AwsFault::from_sdk_error(op, &e)` is used with that exact signature in Tasks 1, 2, 3. `StoreError::Internal(&'static str)` is defined in Task 3 Step 3 and used in Step 6. `AwsFault` is re-exported in Task 1 so `lib.rs` can name it unqualified in Task 3.
