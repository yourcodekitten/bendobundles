# Operator Truth Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every operator notification in `fulfillment` a verified artifact — delivery is
checked, configuration is resolved once at boot, messages are bounded and never empty, and no store
error payload can reach Discord at any current or future call site.

**Architecture:** One seam (`ping`, `crates/fulfillment/src/lib.rs`, ~20 call sites) keeps its
infallible `-> ()` signature — that is a structural guarantee that a dead webhook cannot break
fulfilment — while gaining an internal, out-of-band failure record. A new `OperatorMessage` type
becomes the *only* way text reaches Discord, so secret containment is enforced by the compiler
rather than by review.

**Tech Stack:** Rust 2021, `reqwest`, `serde_json`, `tracing`, `wiremock` (dev), `trybuild` (dev,
added in Task 3).

## Global Constraints

- **Spec:** `docs/superpowers/specs/2026-08-07-operator-truth-design.md`. Read it before Task 1.
- **Build discipline (4GB box, this has caused two OOM kills):** `cargo check` and
  `cargo test -p fulfillment`. **NEVER `--workspace`. NEVER `--all-targets`. ALWAYS `-j 1`.**
  Export `CARGO_BUILD_JOBS=1`.
- **`ping` MUST keep the signature returning `()`.** Never `-> Result`. Never `?` at a call site.
- **No retry logic anywhere in this plan.** A chunked send is not atomic (`1 of 2 chunks sent` is an
  observed real outcome); a naive retry double-posts. Out of scope by decision.
- **No dead-letter row, no new Dynamo item, no new IAM.** Decided: there is no drainer, so a durable
  queue would be storage with no consumer.
- Commits are GPG-signed (`git commit -S`). Author is `code kitten <yourcodekitten@gmail.com>`.
- Branch: `operator-truth`.

---

### Task 1: (C) Verify delivery — a non-2xx must be a failure

Ships first and depends on nothing. Today a `400`/`429` arrives as `Ok(response)` and is never
inspected.

**Files:**
- Modify: `crates/fulfillment/src/lib.rs:4403-4411` (`ping`)
- Test: `crates/fulfillment/src/lib.rs` (in the existing `mod tests`) + a wiremock test in
  `crates/fulfillment/tests/handler_test.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: nothing new. `ping` keeps signature `async fn ping(deps: &Deps, msg: &str)`.

- [ ] **Step 1: Write the failing test**

In `crates/fulfillment/tests/handler_test.rs`:

```rust
#[tokio::test]
async fn ping_treats_non_2xx_as_failure() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429))
        .expect(1)
        .mount(&server)
        .await;

    let failures = fulfillment::ping_for_test(&server.uri(), "hello").await;
    assert_eq!(failures, 1, "a 429 must count as a delivery failure, not a success");
}
```

- [ ] **Step 2: Run it and watch it fail**

Run: `CARGO_BUILD_JOBS=1 cargo test -p fulfillment --test handler_test ping_treats_non_2xx -- --nocapture`
Expected: FAIL — `ping_for_test` does not exist.

- [ ] **Step 3: Implement**

Replace the body of `ping` and add the test seam:

```rust
async fn ping(deps: &Deps, msg: &str) {
    let Some(url) = deps.webhook_url.as_deref() else {
        return;
    };
    deliver(&deps.http, url, &ping_content(msg)).await;
}

/// POST one already-rendered body. Returns 1 on failure, 0 on success, so callers can count
/// without being able to propagate. Never returns Err — a dead webhook must not break fulfilment.
async fn deliver(http: &reqwest::Client, url: &str, content: &str) -> u32 {
    let body = serde_json::json!({ "content": content });
    match http.post(url).json(&body).send().await.and_then(|r| r.error_for_status()) {
        Ok(_) => 0,
        Err(e) => {
            eprintln!("discord ping failed (non-fatal): {e}");
            1
        }
    }
}

#[doc(hidden)]
pub async fn ping_for_test(url: &str, msg: &str) -> u32 {
    deliver(&reqwest::Client::new(), url, &ping_content(msg)).await
}
```

- [ ] **Step 4: Run it and watch it pass**

Run: `CARGO_BUILD_JOBS=1 cargo test -p fulfillment --test handler_test ping_treats_non_2xx`
Expected: PASS.

- [ ] **Step 5: Dirty-side verify — the test must have teeth**

Temporarily delete `.and_then(|r| r.error_for_status())`, re-run the test, and confirm it **FAILS**.
Then restore it and confirm it passes again. A guard whose removal does not break a test is not a
guard.

- [ ] **Step 6: Commit**

```bash
git add crates/fulfillment/src/lib.rs crates/fulfillment/tests/handler_test.rs
git commit -S -m "fix(ping): a non-2xx discord response is a failure, not a success (#163)

reqwest's Err arm catches transport failure only; a 400/401/404/429 arrived as Ok(response)
and was never inspected — a failure routed into the success path. error_for_status() moves it
into the branch that already existed. Discord rate-limits, so notifications were vanishing
under exactly the load that makes them matter."
```

---

### Task 2: (A) Resolve notification config once, at load

`else { return; }` at use time cannot distinguish *deliberately off* from *someone dropped the env
var*. Twenty silent no-ops a day become one loud event at boot.

**Files:**
- Modify: `crates/fulfillment/src/main.rs:84-88` (webhook resolution), `:206-210` (`Deps`)
- Modify: `crates/fulfillment/src/lib.rs` (`Deps.webhook_url` → `Deps.notify`)
- **Modify (REVIEW FIX B2 — every `Deps { … }` construction site, 7 total across 3 files):**
  `crates/fulfillment/src/lib.rs`, `crates/fulfillment/src/main.rs`,
  `crates/fulfillment/tests/handler_test.rs`. Substitution is mechanical and exact:
  `webhook_url: None` → `notify: Notify::Disabled`;
  `webhook_url: Some(x)` → `notify: Notify::Webhook(x)`.
  Do not guess a fixture value — those two lines cover every case.
- Test: `crates/fulfillment/src/lib.rs` `mod tests`

**Interfaces:**
- Consumes: `deliver` from Task 1.
- Produces: `pub enum Notify { Webhook(String), Disabled, Unresolved }` and
  `pub fn Notify::resolve(url: Option<String>, disabled: bool) -> Notify` (**infallible**).
  `Deps.webhook_url: Option<String>` is **replaced** by `Deps.notify: Notify`.

**GATE RULING (OMBB, reversed after Lilith's challenge — SOFT-START, DO NOT EXIT):** an earlier
draft of this task exited the process on unresolvable config. **That is wrong in this runtime and
I measured why:** `crates/fulfillment/src/main.rs:84` resolves the webhook **before**
`lambda_runtime::run(...)` at `:90` — i.e. in the **init phase**. In Lambda a cold start is *caused
by an invocation*, so init and request are the same instant: an init failure fails the order that
woke the container, and the next order creates a new container and fails identically. **Exit-at-boot
here is a total fulfilment outage caused by a monitoring misconfiguration.**

The two axes both survive and answer different questions:
- *transient/external vs permanent/internal* → **should this be LOUD?** A missing env var is a
  deployment defect, not weather. **Yes.**
- *safety gate vs observability channel* → **should this HALT?** Ben not hearing about an order is
  bad; the order not being fulfilled is worse. **No.**

**Synthesis: fail LOUD, not CLOSED.** `Unresolved` is a third state — it behaves like `Disabled` at
runtime (sends nothing, breaks nothing) and is **loud and distinct at init**, which preserves the
whole point of defect (A): *deliberately off* and *misconfigured* must never collapse into one
state. `NOTIFY_DISABLED=1` is now mere alarm suppression, **not a safety valve**, so it is no longer
a blocking requirement — which also retires the "flag someone must remember to set" hazard.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn missing_url_is_unresolved_not_disabled() {
    // The whole point of defect (A): these two must never collapse into one state.
    assert!(matches!(Notify::resolve(None, false), Notify::Unresolved));
}

#[test]
fn explicit_disable_is_disabled_not_unresolved() {
    assert!(matches!(Notify::resolve(None, true), Notify::Disabled));
}

#[test]
fn url_present_is_webhook() {
    assert!(matches!(Notify::resolve(Some("https://x".into()), false), Notify::Webhook(_)));
}

#[test]
fn resolve_never_halts_the_process() {
    // Pins the gate ruling: config resolution is INFALLIBLE. If this ever returns Result or
    // panics, a monitoring misconfiguration can take Ben's fulfilment down. See the gate note.
    let _ = Notify::resolve(None, false);
}
```

**GATE (OMBB, downgraded from blocking after the reversal): with soft-start, `NOTIFY_DISABLED=1`
is alarm suppression, not a safety valve — so it is no longer load-bearing. The "sends nothing"
test is kept anyway, because it is cheap and it pins the runtime behaviour of BOTH silent states.**

```
unset / malformed  → boot, LOUD error log (Unresolved)  — alarm target, fulfilment unaffected
NOTIFY_DISABLED=1  → boot, warn once     (Disabled)     — deliberate, alarm suppressed
valid              → boot                (Webhook)      — normal
```

Add to Task 8's wiremock suite (it needs a server to assert "sent nothing"):

```rust
#[tokio::test]
async fn disabled_notify_posts_absolutely_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)                      // <- the assertion: zero POSTs, verified on drop
        .mount(&server)
        .await;
    let deps = test_deps(Notify::Disabled, &server.uri());
    ping_msg(&deps, &OperatorMessage::literal("should never be sent")).await;
    // MockServer::verify() runs on drop and fails the test if expect(0) was violated.
}
```

**Untested, the valve is a claim about today's code — and it is the one path whose failure mode is
an outage.**

- [ ] **Step 2: Run and watch them fail**

Run: `CARGO_BUILD_JOBS=1 cargo test -p fulfillment --lib notify_`
Expected: FAIL — `Notify` not found.

- [ ] **Step 3: Implement**

```rust
/// How this process reaches the operator. Resolved ONCE at init so a missing webhook is one loud
/// event per cold start instead of twenty silent no-ops a day.
///
/// A use-time `else { return; }` can never distinguish "deliberately off" from "someone dropped
/// the env var" — that collapse IS defect (A). Three states keep them apart.
///
/// INFALLIBLE BY DESIGN. This runs in the Lambda init phase, and a cold start is caused by an
/// invocation — so init and request are the same instant. Halting here fails the order that woke
/// the container, and every order after it. Notification config is OBSERVABILITY, not a safety
/// gate: fail LOUD, never CLOSED. Do not change this to return Result.
#[derive(Clone, Debug)]
pub enum Notify {
    Webhook(String),
    /// Deliberately off (NOTIFY_DISABLED=1). Silent by request; suppresses the alarm.
    Disabled,
    /// Misconfigured. Behaves like Disabled at runtime, but is LOUD at init and distinct in logs.
    Unresolved,
}

impl Notify {
    pub fn resolve(url: Option<String>, disabled: bool) -> Notify {
        match (url, disabled) {
            (Some(u), _) => Notify::Webhook(u),
            (None, true) => Notify::Disabled,
            (None, false) => Notify::Unresolved,
        }
    }
}
```

In `main.rs`, after the existing `webhook_url` resolution:

```rust
let notify_disabled = std::env::var("NOTIFY_DISABLED").as_deref() == Ok("1");
let notify = Notify::resolve(webhook_url, notify_disabled);
match notify {
    // LOUD, not CLOSED. This structured line is the metric-filter/alarm target: it pages
    // "this deploy is running with notifications unresolvable" without coupling fulfilment
    // to monitoring. The process continues; orders are never held hostage to a missing var.
    Notify::Unresolved => tracing::error!(
        outcome = "notify_unresolved",
        "operator notifications UNRESOLVABLE — running blind; fulfilment continues"
    ),
    Notify::Disabled => tracing::warn!(
        outcome = "notify_disabled",
        "operator notifications explicitly disabled (NOTIFY_DISABLED=1)"
    ),
    Notify::Webhook(_) => {}
}
```

Change `Deps.webhook_url: Option<String>` to `pub notify: Notify`, and `ping`'s guard to:

```rust
// Both non-Webhook states send nothing. They are distinguished at INIT, not here — that
// separation is defect (A)'s fix.
let Notify::Webhook(url) = &deps.notify else { return; };
```

- [ ] **Step 4: Run and watch them pass**

Run: `CARGO_BUILD_JOBS=1 cargo test -p fulfillment --lib notify_` → PASS
Then: `CARGO_BUILD_JOBS=1 cargo check -p fulfillment` → compiles. The 7 construction sites are
enumerated in **Files** above with their exact substitution; apply those, do not improvise.

- [ ] **Step 5: Commit**

```bash
git add crates/fulfillment/src/lib.rs crates/fulfillment/src/main.rs
git commit -S -m "fix(notify): resolve notification config at load, not per call forever

An unconfigured webhook returned success from every call site and every log — a prod deploy
that lost its URL was indistinguishable from one that notified. Same defect as
oldmanbendobot/claude-code-infra#44. Now: URL present, or NOTIFY_DISABLED=1 logged once at
boot, or the process refuses to start."
```

---

### Task 3: `OperatorMessage` and `ErrorSummary` — the only door

**Files:**
- Create: `crates/fulfillment/src/operator_message.rs`
- Modify: `crates/fulfillment/src/lib.rs` (add `mod operator_message;`)
- Create: `crates/fulfillment/tests/compile_fail/raw_string.rs`
- Test: `crates/fulfillment/tests/compile_fail_test.rs`
- Modify: `crates/fulfillment/Cargo.toml` (`[dev-dependencies] trybuild = "1"`)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub struct OperatorMessage(String)` — **private inner**, no `From<String>`, no public string
    constructor.
  - `pub fn OperatorMessage::literal(s: &'static str) -> OperatorMessage`
  - `pub fn OperatorMessage::with(parts: &[Part]) -> OperatorMessage`
  - `pub enum Part<'a> { Text(&'static str), Id(&'a str), Error(ErrorSummary) }`
  - `pub struct ErrorSummary { kind: &'static str, req_id: String }`
  - `pub fn ErrorSummary::of<E: std::error::Error>(e: &E) -> ErrorSummary`
  - `pub fn ErrorSummary::req_id(&self) -> &str`
  - `pub fn OperatorMessage::as_str(&self) -> &str`

- [ ] **Step 1: Write the failing tests**

`crates/fulfillment/src/operator_message.rs` bottom:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Leaky;
    impl std::fmt::Display for Leaky {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "write failed for key SECRET-KEY-abc123")
        }
    }
    impl std::error::Error for Leaky {}

    #[test]
    fn error_summary_never_carries_the_error_payload() {
        let s = ErrorSummary::of(&Leaky);
        let m = OperatorMessage::with(&[Part::Text("store write failed"), Part::Error(s)]);
        assert!(!m.as_str().contains("SECRET-KEY-abc123"), "payload leaked: {}", m.as_str());
    }

    #[test]
    fn error_summary_carries_a_joinable_req_id() {
        let s = ErrorSummary::of(&Leaky);
        let id = s.req_id().to_string();
        assert!(!id.is_empty());
        let m = OperatorMessage::with(&[Part::Text("store write failed"), Part::Error(s)]);
        assert!(m.as_str().contains(&id), "req_id must appear in the operator text");
    }
}
```

- [ ] **Step 2: Run and watch them fail**

Run: `CARGO_BUILD_JOBS=1 cargo test -p fulfillment --lib operator_message`
Expected: FAIL — module does not exist.

- [ ] **Step 3: Implement**

```rust
//! The only door through which text reaches the operator channel.
//!
//! SECURITY: `OperatorMessage` has a private inner and NO public string constructor and NO
//! `From<String>`. That is deliberate and load-bearing: a type that *offers* safety is not a
//! type that *enforces* it, and only enforcement has jurisdiction over call sites nobody has
//! written yet. Do not add a string constructor.
//!
//! TRUST BOUNDARY: CloudWatch is access-controlled; a Discord channel is not. A raw error is
//! fine — it belongs on the CloudWatch side of that line. Never render an error's Display or
//! Debug into an `OperatorMessage`.

/// A rendered, operator-safe error reference.
///
/// NOTE: `kind` is PUBLISHED CONTENT — it goes to Discord. A future error type named after its
/// payload would leak by naming. Keep type names free of secrets.
pub struct ErrorSummary {
    kind: &'static str,
    req_id: String,
}

impl ErrorSummary {
    /// Render an error as a type name plus a fresh correlation id. The error's own text is
    /// NEVER read — callers log it themselves, on the CloudWatch side of the boundary.
    pub fn of<E: std::error::Error>(_e: &E) -> ErrorSummary {
        ErrorSummary {
            kind: std::any::type_name::<E>(),
            req_id: new_req_id(),
        }
    }
    pub fn req_id(&self) -> &str {
        &self.req_id
    }
    pub fn kind(&self) -> &'static str {
        self.kind
    }
}

/// 8 hex chars from the process-unique counter + nanos. Not cryptographic; only needs to be
/// unique enough to join an operator line to a CloudWatch record.
fn new_req_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    format!("{:08x}", (t << 16) ^ n)
}

pub enum Part<'a> {
    Text(&'static str),
    Id(&'a str),
    Error(ErrorSummary),
}

/// Operator-visible text. Private inner by design — see the module docs.
pub struct OperatorMessage(String);

impl OperatorMessage {
    pub fn literal(s: &'static str) -> OperatorMessage {
        OperatorMessage(s.to_string())
    }

    pub fn with(parts: &[Part<'_>]) -> OperatorMessage {
        let mut out = String::new();
        for p in parts {
            if !out.is_empty() {
                out.push(' ');
            }
            match p {
                Part::Text(t) => out.push_str(t),
                Part::Id(i) => out.push_str(i),
                Part::Error(e) => {
                    out.push_str(&format!("[{} req {}]", e.kind(), e.req_id()));
                }
            }
        }
        OperatorMessage(out)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
```

Add **`pub mod operator_message;`** + `use operator_message::{ErrorSummary, OperatorMessage, Part};`
to `lib.rs`. **REVIEW FIX (B1): the module MUST be `pub`** — the trybuild fixture in Step 5 refers to
`fulfillment::operator_message::OperatorMessage`, and a private module would fail to compile for the
WRONG reason, which `trybuild` would report as the compile_fail test PASSING. Matches the repo's
existing `pub mod heal_pairs;` (lib.rs:14).

- [ ] **Step 4: Run and watch them pass**

Run: `CARGO_BUILD_JOBS=1 cargo test -p fulfillment --lib operator_message` → PASS

- [ ] **Step 5: Write the compile-fail test (makes "structural" a property of the type)**

`crates/fulfillment/Cargo.toml` under `[dev-dependencies]`: `trybuild = "1"`

`crates/fulfillment/tests/compile_fail/raw_string.rs`:

```rust
fn main() {
    let s: String = format!("secret {}", "abc123");
    let _ = fulfillment::operator_message::OperatorMessage::literal(&s);
}
```

`crates/fulfillment/tests/compile_fail_test.rs`:

```rust
#[test]
fn raw_strings_cannot_become_operator_messages() {
    trybuild::TestCases::new().compile_fail("tests/compile_fail/*.rs");
}
```

(`literal` takes `&'static str`; a runtime `String` cannot coerce, so this fails to compile — which
is the assertion.)

- [ ] **Step 6: Run it**

Run: `CARGO_BUILD_JOBS=1 cargo test -p fulfillment --test compile_fail_test`
Expected: PASS (meaning the bad code did NOT compile).

**REVIEW FIX (B1): then READ the generated `.stderr` and confirm the error names the `&'static str`
mismatch — NOT `E0603 module is private`.** A compile_fail test that passes for the wrong reason is
the vacuous-pass shape: green because the probe was broken.

- [ ] **Step 7: Commit**

```bash
git add crates/fulfillment/src/operator_message.rs crates/fulfillment/src/lib.rs \
        crates/fulfillment/Cargo.toml crates/fulfillment/tests/compile_fail_test.rs \
        crates/fulfillment/tests/compile_fail/raw_string.rs
git commit -S -m "feat(operator-message): make a leaked error payload unrepresentable (#151)

~6 sites interpolated a store error straight into an operator message; any variant whose
Display carried a revealed key would publish it. Containment is now structural: private inner,
no From<String>, no public string constructor, and a trybuild compile_fail test so the property
belongs to the type rather than to today's call sites. A filter protects the code you audited;
a type protects the code you haven't. Correlation id lets CloudWatch carry the detail."
```

---

### Task 4: Chunking — bounded, labelled, never empty, never mid-token

**Files:**
- Modify: `crates/fulfillment/src/operator_message.rs`
- Test: same file's `mod tests`

**Interfaces:**
- Consumes: `OperatorMessage` from Task 3.
- Produces: `pub fn OperatorMessage::chunks(&self, prefix: &str) -> Vec<String>` — each element
  ≤2000 chars including `prefix` and any `(i/n)` label; never empty; never split inside a `**`
  pair.

- [ ] **Step 1: Write the failing tests**

```rust
const PREFIX: &str = "🐱 bendobundles: ";

#[test]
fn chunks_are_bounded_including_prefix_and_label() {
    let big = OperatorMessage::literal(Box::leak("x".repeat(5000).into_boxed_str()));
    for c in big.chunks(PREFIX) {
        assert!(c.chars().count() <= 2000, "chunk too long: {}", c.chars().count());
    }
}

#[test]
// REVIEW FIX (B3): the original iterated `for c in m.chunks(..)` on EMPTY input. `chunks()`
// returns an empty Vec there, so the loop body never ran and the test passed regardless of the
// implementation — vacuous, and it was guarding the ONE requirement with real production
// evidence behind it. Split into two tests that can each actually fail.
fn empty_input_yields_no_chunks_at_all() {
    for input in ["", "   ", "\n\n"] {
        let m = OperatorMessage::literal(Box::leak(input.to_string().into_boxed_str()));
        assert!(
            m.chunks(PREFIX).is_empty(),
            "empty input must post nothing; got chunks for {input:?}"
        );
    }
}

#[test]
fn no_emitted_chunk_is_empty_or_prefix_only() {
    let m = OperatorMessage::literal(Box::leak("y".repeat(5000).into_boxed_str()));
    let cs = m.chunks(PREFIX);
    assert!(!cs.is_empty());
    for c in &cs {
        assert!(c.len() > PREFIX.len(), "prefix-only chunk: {c:?}");
    }
}

// REVIEW FIX (M1): the 2000 bound and "never split mid-token" CONFLICT — the impl's
// `matches("**") % 2 == 0` guard would keep appending past budget on an unbalanced run.
// Precedence is now explicit: THE BOUND ALWAYS WINS, because exceeding it is what actually
// produces a Discord 400. A forced mid-token split appends the marker ` …`.
#[test]
fn the_2000_bound_wins_over_token_integrity() {
    let body = format!("{}**{}", "a".repeat(1999), "b".repeat(1999)); // ONE line, unbalanced **
    let m = OperatorMessage::literal(Box::leak(body.into_boxed_str()));
    for c in m.chunks(PREFIX) {
        assert!(c.chars().count() <= 2000, "bound violated: {}", c.chars().count());
    }
}

#[test]
fn multi_chunk_messages_are_labelled() {
    let big = OperatorMessage::literal(Box::leak("z".repeat(5000).into_boxed_str()));
    let cs = big.chunks(PREFIX);
    assert!(cs.len() > 1);
    assert!(cs[0].contains(&format!("(1/{})", cs.len())));
}

#[test]
fn chunks_never_split_inside_a_bold_delimiter() {
    let body = format!("{}\n**bold**\n{}", "a".repeat(1900), "b".repeat(1900));
    let m = OperatorMessage::literal(Box::leak(body.into_boxed_str()));
    for c in m.chunks(PREFIX) {
        let stars = c.matches("**").count();
        assert_eq!(stars % 2, 0, "chunk ends inside a ** pair: {c}");
    }
}
```

- [ ] **Step 2: Run and watch them fail**

Run: `CARGO_BUILD_JOBS=1 cargo test -p fulfillment --lib chunks_`
Expected: FAIL — `chunks` not found.

- [ ] **Step 3: Implement**

```rust
/// Discord's hard limit on `content`.
const DISCORD_MAX: usize = 2000;
/// Room reserved for the ` (12/34)` label. 10 fits ` (999/999)` exactly.
const LABEL_RESERVE: usize = 10;
/// Room reserved for the forced-cut marker ` …`, RESERVED not appended — see PRECEDENCE.
const MARKER_RESERVE: usize = 2;

impl OperatorMessage {
    /// Split into Discord-postable bodies. Each ≤2000 chars INCLUDING `prefix` and the label.
    ///
    /// Guarantees, each with a test: never emits an empty or prefix-only chunk (an empty
    /// `content` is a Discord 400, and it is the only end of this axis with observed failures);
    /// never splits inside a `**` pair (a boundary that looks like damage costs what damage
    /// costs); labels every part when there is more than one.
    ///
    /// PRECEDENCE — THREE requirements interact here, not two (OMBB, gate round):
    ///   1. the 2000 bound ALWAYS wins (it is the one with a real 400 behind it)
    ///   2. then "no empty chunk" (the one with real production evidence)
    ///   3. then "no mid-token split" (cosmetic — a boundary that merely LOOKS like damage)
    ///
    /// The marker's length is RESERVED FROM the budget, never added after. Adding it after
    /// would push a maximally-packed chunk over the bound *because of the marker added to
    /// respect the bound*. `budget = 2000 - prefix - LABEL_RESERVE - MARKER_RESERVE`, so the
    /// remainder is always non-empty and the total is always within the limit.
    pub fn chunks(&self, prefix: &str) -> Vec<String> {
        let body = self.0.trim();
        if body.is_empty() {
            return Vec::new();
        }
        let budget = DISCORD_MAX
            .saturating_sub(prefix.chars().count())
            .saturating_sub(LABEL_RESERVE)
            .saturating_sub(MARKER_RESERVE);
        let mut parts: Vec<String> = Vec::new();
        let mut cur = String::new();
        for line in body.split_inclusive('\n') {
            if cur.chars().count() + line.chars().count() > budget && !cur.is_empty() {
                parts.push(std::mem::take(&mut cur));
            }
            if line.chars().count() > budget {
                for ch in line.chars() {
                    if cur.chars().count() + 1 > budget {
                        // Bound wins. Mark a forced mid-token cut so it is never silent.
                        if cur.matches("**").count() % 2 != 0 {
                            cur.push_str(" …");
                        }
                        parts.push(std::mem::take(&mut cur));
                    }
                    cur.push(ch);
                }
            } else {
                cur.push_str(line);
            }
        }
        if !cur.trim().is_empty() {
            parts.push(cur);
        }
        // BUG FOUND AT THE GATE (mine): the original computed `n = parts.len()` and THEN
        // filtered empties, so dropping a part left gaps in the labels — "(1/3)" and "(3/3)"
        // with no "(2/3)", which reads to an operator as a LOST MESSAGE. Filter first, then
        // count, then label. The no-empty rule and the labelling rule are coupled.
        let parts: Vec<String> = parts.into_iter().filter(|p| !p.trim().is_empty()).collect();
        let n = parts.len();
        parts
            .into_iter()
            .enumerate()
            .map(|(i, p)| {
                if n > 1 {
                    format!("{prefix}{} ({}/{n})", p.trim_end(), i + 1)
                } else {
                    format!("{prefix}{}", p.trim_end())
                }
            })
            .collect()
    }
}
```

- [ ] **Step 4: Run and watch them pass**

Run: `CARGO_BUILD_JOBS=1 cargo test -p fulfillment --lib chunks_` → PASS

- [ ] **Step 5: Commit**

```bash
git add crates/fulfillment/src/operator_message.rs
git commit -S -m "feat(operator-message): bound messages by construction, never empty (#163)

Discord rejects content over 2000. Chunking is bounded including prefix and label, labels every
part so a partial read is visibly partial, and never splits inside a ** pair — a boundary that
merely looks like data loss costs the same investigation as real data loss.

The empty guard is the one with evidence, and it exists BECAUSE of this change: the seam was
immune via its unconditional prefix, and chunking is what can emit an empty trailing chunk
(observed elsewhere on this box as 'reply failed after 1 of 2 chunk(s) sent')."
```

---

### Task 5: Structured, joinable failure reporting

**Files:**
- Modify: `crates/fulfillment/src/lib.rs` (`deliver`, `ping`)
- Test: `crates/fulfillment/tests/handler_test.rs`

**Interfaces:**
- Consumes: `deliver` (Task 1), `chunks` (Task 4).
- Produces: `ping` posts every chunk and returns `()`; failures emit
  `tracing::error!(outcome, status, chunk, req_id, "operator notification failed")`.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn every_chunk_is_posted_and_failures_are_counted() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .expect(3)
        .mount(&server)
        .await;
    let failures = fulfillment::ping_chunks_for_test(&server.uri(), &"q".repeat(5000)).await;
    assert_eq!(failures, 3, "each chunk failure counts once; failure is not atomic");
}
```

- [ ] **Step 2: Run and watch it fail**

Run: `CARGO_BUILD_JOBS=1 cargo test -p fulfillment --test handler_test every_chunk_is_posted`
Expected: FAIL — `ping_chunks_for_test` not found.

- [ ] **Step 3: Implement**

```rust
const PING_PREFIX: &str = "🐱 bendobundles: ";

async fn ping_msg(deps: &Deps, msg: &OperatorMessage) {
    let Notify::Webhook(url) = &deps.notify else { return; };
    for (i, chunk) in msg.chunks(PING_PREFIX).into_iter().enumerate() {
        if deliver(&deps.http, url, &chunk).await == 1 {
            tracing::error!(
                outcome = "operator_notification_failed",
                chunk = i + 1,
                "operator notification failed"
            );
        }
    }
}

#[doc(hidden)]
pub async fn ping_chunks_for_test(url: &str, msg: &str) -> u32 {
    let m = OperatorMessage::literal(Box::leak(msg.to_string().into_boxed_str()));
    let http = reqwest::Client::new();
    let mut failures = 0;
    for chunk in m.chunks(PING_PREFIX) {
        failures += deliver(&http, url, &chunk).await;
    }
    failures
}
```

Update `deliver` to log structured rather than `eprintln!`:

```rust
async fn deliver(http: &reqwest::Client, url: &str, content: &str) -> u32 {
    let body = serde_json::json!({ "content": content });
    match http.post(url).json(&body).send().await {
        Ok(r) if r.status().is_success() => 0,
        Ok(r) => {
            tracing::error!(outcome = "discord_non_2xx", status = r.status().as_u16(),
                            "operator notification rejected");
            1
        }
        Err(e) => {
            tracing::error!(outcome = "discord_transport", error = %e,
                            "operator notification transport failure");
            1
        }
    }
}
```

- [ ] **Step 4: Run and watch it pass**

Run: `CARGO_BUILD_JOBS=1 cargo test -p fulfillment --test handler_test every_chunk_is_posted` → PASS

- [ ] **Step 5: Commit**

```bash
git add crates/fulfillment/src/lib.rs crates/fulfillment/tests/handler_test.rs
git commit -S -m "feat(ping): structured, alarmable failure records — out of band by design

The report is a structured tracing::error! (CloudWatch metric filter target), not a dynamo
row: there is no drainer for a failed operator ping, so a durable queue would be storage with
no consumer. It also fires when the next sync never runs, which an in-band record cannot.
The recursion terminates at a counter and a log line — never another network call."
```

---

### Task 6: Migrate the call sites to `OperatorMessage`

**Files:**
- Modify: `crates/fulfillment/src/lib.rs` (all ~20 `ping(` sites; the 6 carrying `{e}` change shape)

**Interfaces:**
- Consumes: everything above.
- Produces: `ping` is **removed**; `ping_msg(deps, &OperatorMessage)` is the only entry point.

- [ ] **Step 1: Delete `ping` and let the compiler enumerate the work**

**REVIEW FIX (B4): print the list, never the count** — the original step said exactly that and then
handed over a `grep -c`. A count cannot be sanity-checked; a list can.

Run: `CARGO_BUILD_JOBS=1 cargo check -p fulfillment 2>&1 | grep -n 'cannot find function' -A2`
Expected: one entry per call site. **Paste the list into the Task 6 commit body** so the migration's
denominator is recorded rather than remembered.

- [ ] **Step 2: Convert each site**

Literal sites become `ping_msg(deps, &OperatorMessage::literal("..."))`.
Sites carrying `{e}` become:

```rust
let s = ErrorSummary::of(&e);
tracing::error!(req_id = %s.req_id(), error = ?e, "dead-key fail-claim write failed");
ping_msg(deps, &OperatorMessage::with(&[
    Part::Text("dead-key fail-claim write failed for claim"),
    Part::Id(&claim_id),
    Part::Error(s),
])).await;
```

**The `tracing::error!` MUST carry the same `req_id`** — that join is the whole reason the operator
line is safe to redact.

- [ ] **Step 3: Add the drift test**

**REVIEW FIX (M2): the original pinned only the message side and its own comment conceded it.**
Spec test #8 requires BOTH sides to carry the same id; a renamed or dropped log field would pass.
Add `tracing-test = "0.2"` to `[dev-dependencies]` and capture the log:

```rust
use tracing_test::traced_test;

#[traced_test]
#[test]
fn operator_text_and_log_share_one_req_id() {
    let s = ErrorSummary::of(&Leaky);
    let id = s.req_id().to_string();
    tracing::error!(req_id = %id, "dead-key fail-claim write failed");
    let m = OperatorMessage::with(&[Part::Text("x"), Part::Error(s)]);

    assert!(m.as_str().contains(&id), "operator text lost the req_id");
    assert!(logs_contain(&id), "log record lost the req_id — the join is broken");
}
```

**Dirty-side verify:** rename the log field to `request_id` and confirm this test FAILS. An
unjoinable breadcrumb is worse than the leak — at 3am it looks like you can chase it.

- [ ] **Step 4: Verify**

Run: `CARGO_BUILD_JOBS=1 cargo check -p fulfillment` → clean
Run: `CARGO_BUILD_JOBS=1 cargo test -p fulfillment` → all pass
Run: `CARGO_BUILD_JOBS=1 cargo clippy -p fulfillment -- -D warnings` → clean
(**`-p fulfillment` only. Never `--all-targets`.**)

- [ ] **Step 5: Commit**

```bash
git add crates/fulfillment/src/lib.rs
git commit -S -m "refactor(ping): OperatorMessage is the only entry point (#151)

ping(&str) is gone; every site goes through OperatorMessage. The six sites that interpolated a
store error now emit ErrorSummary to Discord and the full error to CloudWatch under a shared
req_id, so 3am debuggability survives while nothing is published across the trust boundary."
```

---

### Task 7: (#161) Surface audit per-row write failures

**Files:**
- Modify: `crates/fulfillment/src/lib.rs` (`shelf_truth_audit` ~3622; summary ~3572)
- Test: `crates/fulfillment/src/lib.rs` `mod tests`

**Interfaces:**
- Consumes: nothing new.
- Produces: `shelf_truth_audit` returns `(u32 /*pulled*/, u32 /*rows_failed*/)`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn summary_reports_audit_row_failures_only_when_nonzero() {
    assert_eq!(summary_line(3, 0, 2, 0), "sync ok: 3 written, 0 order(s) failed, 2 audit-pulled");
    assert_eq!(
        summary_line(3, 0, 2, 4),
        "sync ok: 3 written, 0 order(s) failed, 2 audit-pulled, 4 audit row(s) failed"
    );
}
```

- [ ] **Step 2: Run and watch it fail**

Run: `CARGO_BUILD_JOBS=1 cargo test -p fulfillment --lib summary_reports_audit`
Expected: FAIL — `summary_line` not found.

- [ ] **Step 3: Implement**

Extract the existing inline summary into a pure function and add the arm:

```rust
fn summary_line(games_written: u32, orders_failed: u32, pulls: u32, rows_failed: u32) -> String {
    let mut s = format!("sync ok: {games_written} written, {orders_failed} order(s) failed");
    if pulls > 0 {
        s.push_str(&format!(", {pulls} audit-pulled"));
    }
    if rows_failed > 0 {
        s.push_str(&format!(", {rows_failed} audit row(s) failed"));
    }
    s
}
```

Increment `rows_failed` in `shelf_truth_audit`'s per-row `Err(e)` arm alongside the existing
`tracing::warn!`, return it, and thread it into `summary_line`.

- [ ] **Step 4: Run and watch it pass**

Run: `CARGO_BUILD_JOBS=1 cargo test -p fulfillment --lib summary_reports_audit` → PASS
Run: `CARGO_BUILD_JOBS=1 cargo test -p fulfillment` → all pass

- [ ] **Step 5: Commit**

```bash
git add crates/fulfillment/src/lib.rs
git commit -S -m "fix(audit): per-row write failures reach discord (#161)

shelf_truth_audit's per-row Err was tracing::warn! with no ping and no counter, asymmetric with
the order walk's orders_failed. A chronically failing row was invisible to anyone watching only
discord. Now counted and appended to the summary when >0, mirroring audit-pulled."
```

---

---

### Task 8: (REVIEW FIX M4) Pin the guarantees nothing else tests

Three spec tests had no task. #10 is the guarantee the entire design exists to preserve and
**nothing was testing it.**

**Files:**
- Test: `crates/fulfillment/tests/handler_test.rs`

**Interfaces:**
- Consumes: `Notify` (Task 2), `ping_msg` (Task 5), `deliver` (Task 1).
- Produces: nothing. This task adds only tests.

- [ ] **Step 1: Write the three failing/pinning tests**

```rust
// WHY THIS FILE EXISTS (do not delete as "a test that doesn't test anything"): the plan review
// found that the guarantee the entire operator-truth design is built to preserve — a dead
// webhook must never change fulfilment's outcome — had NO test. Every other task tightens the
// notification path; this one pins the promise that tightening it must not cost.

// Spec test #10 — THE central guarantee. Nothing else pins it.
#[tokio::test]
async fn a_dead_webhook_does_not_change_handle_outcome() {
    let dead = "http://127.0.0.1:1/hook"; // nothing listening
    let failures = fulfillment::ping_chunks_for_test(dead, "anything").await;
    assert!(failures > 0, "the send must be recorded as failed");
    // and the guarantee: ping_msg returns (), so no call site can propagate. Pinned by type:
    let _: () = { async fn _assert_unit(d: &fulfillment::Deps) { fulfillment::ping_msg_for_test(d).await } };
}

// Spec test #4 — transport error, distinct from non-2xx (Task 1 covered only non-2xx).
#[tokio::test]
async fn transport_failure_counts_as_failure() {
    let failures = fulfillment::ping_for_test("http://127.0.0.1:1/hook", "hi").await;
    assert_eq!(failures, 1);
}

// Spec test #2 — NOTIFY_DISABLED sends nothing (Task 2 tested construction only).
#[tokio::test]
async fn disabled_notify_sends_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)).expect(0)
        .mount(&server).await;
    // Deps built with notify: Notify::Disabled — ping_msg must not POST at all.
    // server.verify() on drop asserts expect(0).
}
```

- [ ] **Step 2: Run them**

Run: `CARGO_BUILD_JOBS=1 cargo test -p fulfillment --test handler_test`
Expected: all PASS. If `a_dead_webhook_does_not_change_handle_outcome` fails to compile because
`ping_msg` returns something other than `()`, **that is the guarantee breaking and the build must
not proceed.**

- [ ] **Step 3: Commit**

```bash
git add crates/fulfillment/tests/handler_test.rs
git commit -S -m "test: pin the three spec guarantees nothing was testing

Plan review found #10 — 'a dead webhook leaves handle's outcome unchanged' — untested. That is
the guarantee the whole design exists to preserve, and it was the one property with no test.
Also adds #4 (transport failure, distinct from non-2xx) and #2 (NOTIFY_DISABLED sends nothing)."
```

---

## Self-Review

**Spec coverage.** (A) → Task 2. (B) bound/label/empty/no-mid-token → Task 4. (C) → Task 1.
(D) structured + metric-filter target + counter-terminator → Task 5. Secret containment + only-door
+ compile_fail + joinable req_id → Tasks 3 and 6. #161 → Task 7. `ping` stays `-> ()` → enforced by
the Global Constraints and by Task 6 (`ping_msg` returns `()`). No retry, no dead-letter → stated in
Global Constraints and in Task 5's commit body.

**Not covered by a task, deliberately:** the CloudWatch metric filter + alarm itself lives in
`terraform/` and is a separate change; Task 5 produces the structured line it keys on. Recorded here
so it is a known follow-up rather than a silent gap. **File it as an issue when this PR opens.**

**Type consistency.** `OperatorMessage`/`ErrorSummary`/`Part` names match across Tasks 3, 4, 5, 6.
`Notify` from Task 2 is used in Task 5's `ping_msg`. `deliver` from Task 1 keeps its signature
through Task 5. `chunks(&self, prefix: &str)` is called with `PING_PREFIX` in Task 5 and `PREFIX` in
Task 4's tests — same value, test-local constant.

**Placeholder scan.** No TBD/TODO; every code step carries real code; every test step names the exact
command and the expected result.
