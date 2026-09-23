# The Switchboard 🎛️ — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every outbound register in `bendobundles` routes through one gate that logs every dark face, and reaching a webhook URL without going through that gate stops compiling.

**Architecture:** Add `Register` (who is speaking) and `Notify::sendable(Register) -> Option<&str>` (the only door to a URL, logging each dark face). Make `Notify::Webhook`'s payload a newtype with a private field and no accessor, so the existing `let Notify::Webhook(u) = … else { return }` shape still *matches* but yields a value nothing can send — the bad state becomes unrepresentable rather than discouraged. Then route `ping_msg`, both existing dark-gates, and the bell through it, and give the bell its own `Notify` so `WHISPER_DISABLED` stops darking it.

**Tech Stack:** Rust (workspace, `crates/fulfillment`), `tracing`, Terraform + CloudWatch. **Zero new dependencies.**

**Spec:** `docs/spec-switchboard.md` (status CURRENT, citations re-verified 2026-09-23)

## Where these tests can actually RUN — measured 2026-09-23, before a line was written

🔴 **`dynamodb-local` CANNOT RUN ON THIS BOX.** No `docker`, no `podman`, no `java` (`which` finds
none; `docker info` → rc 127). `store_or_skip` (`handler_test.rs:81`) therefore prints `SKIP` and
returns `None` for every store-backed test here — **a green that asserted nothing.** With
`DYNAMODB_LOCAL_URL` *set*, it instead **panics**: *"refusing to skip (this would forge a green
run)"*. CI is the honest environment: `.github/workflows/ci.yml:12` runs
`amazon/dynamodb-local:2.5.2` with `DYNAMODB_LOCAL_URL: http://localhost:8000`.

⇒ **Each new test below is labelled LOCAL or CI-ONLY. Do not discover this per-task at 2am.**

| test | needs a live store? | why | where it runs |
|---|---|---|---|
| T1 `sendable` matrix | no | calls `Notify::sendable` directly, no `Deps` | **LOCAL** |
| T1 doctests | no | lib target | **LOCAL** |
| T2 `ping_msg…dark_ops` | **no** | measured: `ping_msg`'s dark path returns at the gate, **before** `msg.chunks` or any `deps.store` use | **LOCAL** — build a `Store` against an unreachable endpoint; it is never dereferenced |
| T3 `a_dark_whisper…` | **no** | measured: `grep 'deps.store'` over `resolve_whisper_url`'s range returns **zero** | **LOCAL** — same unreachable-endpoint `Store` |
| T4 `the_bool_and_the_gate…` | no | calls `Notify::resolve` directly | **LOCAL** |
| T4 `the_bell_and_the_whisper…` | **YES** | `bell::ring` hits `deps.store.get_link` (`bell.rs:131`, `:142`, `:162`) on the path that must RING | **CI-ONLY** |
| T5 coupling test | no | `sendable` + a file read | **LOCAL** |

**Building that `Store` without a live endpoint** — `dynamo::Store::new(client, table)` is `pub`
(`crates/dynamo/src/lib.rs:661`) and `aws_sdk_dynamodb::Client::new(config)` **does not connect at
construction**. So:

```rust
// A Store that is VALID but never reachable. Sound only for tests whose path provably never
// touches it — the table above says which. If a test you write starts touching the store, it
// belongs in the CI-ONLY row, not behind a longer timeout.
fn unreachable_store() -> Store {
    let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .endpoint_url("http://127.0.0.1:1")
        .region("us-east-1")
        .test_credentials()
        .load();
    let config = futures::executor::block_on(config); // or make the helper `async` and await it
    Store::new(aws_sdk_dynamodb::Client::new(&config), "sw-unused".to_string())
}
```

⚠️ **The CI-ONLY test's red/green cycle is a PUSH cycle, not a local one.** Write the failing test,
push, watch the CI job go **red for the stated reason** (read the log, do not infer it from the
red), then implement, push, watch it go green. **Do not mark that step done off a local run** — a
local run of it is a skip or a panic, never a pass.

## Global Constraints

- **Zero new dependencies.** 4GB box; the structural assertion uses rustdoc's built-in `compile_fail`, never `trybuild`.
- **`ping_msg` returns `()` and no call site may propagate.** This is the structural guarantee that a dead webhook cannot break fulfilment. **Never introduce a panic, `unwrap`, or `unreachable!()` on the notification path.**
- **No wildcards over `Notify`.** Every match on it writes all three arms out. `Notify::resolve`'s six-cell matrix is NOT touched by this plan.
- **Do not reword the whisper/lantern operator PING payloads.** They are family-reviewed (`lib.rs:4817`, `:5162` say so) and they interpolate `whisper_param_name` — they are webhook messages, not log lines, and they stay byte-identical. ⚠️ Their short **log** sentences are a different thing and DO move, into `Register::note`'s table (Task 1), verbatim.
- **No new DynamoDB rows, no retry, no dead-letter, no admin surface.** Spec §2.
- New tests that capture `tracing` output **must live in `crates/fulfillment/tests/handler_test.rs`** — `capture_logs()` (`:4404`) is defined there and must not be copied into a second file.
- Commit messages: signed (`-S`), authored as `code kitten <yourcodekitten@gmail.com>`.

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/fulfillment/src/lib.rs` | `Register`, `WebhookUrl`, `Notify::sendable`, `Deps`, `ping_msg`, the two dark-gates | Modify |
| `crates/fulfillment/src/bell.rs` | `bell::ring` takes its own register | Modify `:110-123` |
| `crates/fulfillment/src/main.rs` | resolve `bell_notify` at init; drop the plumbed bool | Modify `:154-160`, `:308` |
| `crates/fulfillment/tests/handler_test.rs` | the `sendable` matrix, dark-record assertions, both decoupling directions, the tf/string-coupling assertion | Modify (`:150`, `:559`, `:9945` + new tests) |
| `terraform/aws-cloudwatch-alarms.tf` | the metric filter + alarm that survives a dark ops register | Modify |

---

### Task 1: The gate — `Register`, `WebhookUrl`, `Notify::sendable`, and the structural assertion

**Files:**
- Modify: `crates/fulfillment/src/lib.rs` (the `Notify` enum — `#[derive]` at `:465`, `pub enum Notify` at `:466`, body to `:473`)
- Modify: every `Notify::Webhook(...)` **construction** site — compile-enforced; census is **13** hits workspace-wide (`grep -rn 'Notify::Webhook' --include=*.rs . | grep -v /target/`), most of them test-side
- Test: `crates/fulfillment/tests/handler_test.rs`

**Interfaces:**
- Consumes: nothing (first task)
- Produces:
  - `pub enum Register { Ops, Whisper, Lantern, Bell }` with `pub fn as_str(&self) -> &'static str` and `pub fn note(&self, DarkFace) -> &'static str` (8 cells, no wildcard — **public so the integration test asserts against the table instead of a hand-typed copy of it**)
  - `pub enum DarkFace { Disabled, Unresolved }`
  - `pub const REGISTER_UNRESOLVED_NEEDLE: &str`
  - `pub struct WebhookUrl(String)` — field private to the crate root module, **no** `as_str`, **no** `Deref`, **no** `Into<String>`; constructed via `pub fn WebhookUrl::new(String) -> WebhookUrl`
  - `pub fn Notify::sendable(&self, reg: Register) -> Option<&str>`

- [ ] **Step 1: Write the failing test — the `sendable` matrix**

In `crates/fulfillment/tests/handler_test.rs`:

```rust
#[test]
fn sendable_returns_url_only_for_webhook_and_logs_every_dark_face() {
    let (log_buf, _capture) = capture_logs();

    let hook = fulfillment::Notify::Webhook(fulfillment::WebhookUrl::new(
        "https://discord.example/hook".to_string(),
    ));
    assert_eq!(
        hook.sendable(fulfillment::Register::Ops),
        Some("https://discord.example/hook")
    );

    assert_eq!(fulfillment::Notify::Disabled.sendable(fulfillment::Register::Whisper), None);
    assert_eq!(fulfillment::Notify::Unresolved.sendable(fulfillment::Register::Lantern), None);

    let logs = String::from_utf8(log_buf.lock().unwrap().clone()).unwrap();

    // The dark faces must be DISTINGUISHABLE, not merely both absent-of-url.
    assert!(logs.contains(r#"outcome="register_dark""#), "no register_dark record: {logs}");
    assert!(logs.contains(r#"register="whisper""#), "disabled face not tagged with its register: {logs}");
    assert!(logs.contains(r#"reason="disabled""#), "disabled face lost its reason: {logs}");
    assert!(logs.contains(r#"register="lantern""#), "unresolved face not tagged with its register: {logs}");
    assert!(logs.contains(r#"reason="unresolved""#), "unresolved face lost its reason: {logs}");

    // Levels differ: Disabled is operator-initiated silence (INFO — the level bell.rs already
    // used for exactly this state); Unresolved is misconfiguration (ERROR).
    //
    // 🔴 ASSERTED PER LINE, NOT OVER THE WHOLE BUFFER. Both faces emit into one buffer, so
    // `logs.contains("INFO") && logs.contains("ERROR")` passes even if the two levels are
    // SWAPPED — and which face gets which level is exactly what the family gate spent the
    // morning settling. An assertion that cannot detect the inversion of the decision it
    // encodes is not asserting the decision.
    let line_for = |reason: &str| -> String {
        logs.lines()
            .find(|l| l.contains(&format!(r#"reason="{reason}""#)))
            .unwrap_or_else(|| panic!("no record for reason={reason}: {logs}"))
            .to_string()
    };
    assert!(line_for("disabled").contains("INFO"), "disabled face is not INFO: {}", line_for("disabled"));
    assert!(line_for("unresolved").contains("ERROR"), "unresolved face is not ERROR: {}", line_for("unresolved"));

    // The machine contract: the alarm's needle must actually appear in the emitted text.
    assert!(
        logs.contains(fulfillment::REGISTER_UNRESOLVED_NEEDLE),
        "the unresolved face does not carry the alarm's needle: {logs}"
    );

    // A sendable Webhook emits NOTHING — the gate must not spam the healthy path.
    assert!(!logs.contains(r#"register="ops""#), "the healthy path emitted a record: {logs}");
}
```

- [ ] **Step 2: Run the test and watch it fail for the RIGHT reason**

Run: `cargo test -p fulfillment --test handler_test sendable_returns_url_only -- --nocapture`

Expected: **compile error** — `Register`, `WebhookUrl`, and `sendable` do not exist. *If it fails any other way, stop and read the error: a test that fails for the wrong reason proves nothing about the code you are about to write.*

- [ ] **Step 3: Add `Register` and `WebhookUrl`**

In `crates/fulfillment/src/lib.rs`, directly above the `Notify` enum:

```rust
/// WHO is speaking. A log field, so one metric filter selects every dark face and a dashboard
/// can split by register. Four registers exist and each resolves its own `Notify`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Register {
    Ops,
    Whisper,
    Lantern,
    Bell,
}

/// Which way a register is dark. Exists so the note table below is keyed by a TYPE and not by a
/// string — a stringly-keyed lookup silently returns the wrong sentence when a name drifts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DarkFace {
    Disabled,
    Unresolved,
}

impl Register {
    /// The human sentence for one (register × face). **All eight cells written out, no wildcard**
    /// — the same discipline `Notify::resolve`'s six-cell matrix already uses, and for the same
    /// reason: adding a register or a face must fail to compile here rather than silently reuse a
    /// neighbour's wording.
    ///
    /// 🔑 **Prose lives HERE and the machine key lives in `sendable`.** OMBB's review, 2026-09-23,
    /// answering a question Lilith and I had both answered worse: we proposed TWO RECORDS to give
    /// the machine contract and the human prose different lifetimes. **Two FIELDS buys the same
    /// separation at half the volume**, on a per-send path, with one emission site — and it is the
    /// shape this file **already ships twice**: `:4754` (`outcome="operator_notification_failed"`
    /// + `chunk`/`of` + prose) for a failure, and `bell.rs:115`
    /// (`outcome="bell_disabled"` + prose, *"Loud, so a muted bell never reads as broken"*) for
    /// chosen silence — *at `info!`, which is also where this gate's `Disabled` level comes from.*
    /// **This is not a new shape; it is the one the file uses, generalised to all four registers.**
    /// The filter reads `outcome`/`reason`; nothing machine-readable ever reads this text, so
    /// improving the wording can never kill the alarm.
    pub fn note(&self, face: DarkFace) -> &'static str {
        match (self, face) {
            (Register::Ops, DarkFace::Disabled) => "operator notifications are off by request",
            (Register::Ops, DarkFace::Unresolved) => "operator notifications are configured but UNREADABLE — running blind",
            (Register::Whisper, DarkFace::Disabled) => "whisper webhook unconfigured — no-op, zero writes",
            (Register::Whisper, DarkFace::Unresolved) => "whisper webhook configured but UNREADABLE — no-op, zero writes",
            (Register::Lantern, DarkFace::Disabled) => "lantern register unconfigured or LANTERN_DISABLED — no-op, zero writes",
            (Register::Lantern, DarkFace::Unresolved) => "lantern register configured but UNREADABLE — no-op, zero writes",
            (Register::Bell, DarkFace::Disabled) => "bell: BELL_DISABLED set — not ringing, by choice",
            (Register::Bell, DarkFace::Unresolved) => "bell register configured but UNREADABLE — not ringing",
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Register::Ops => "ops",
            Register::Whisper => "whisper",
            Register::Lantern => "lantern",
            Register::Bell => "bell",
        }
    }
}

/// A webhook URL that **cannot be sent to without passing the gate**.
///
/// The field is private and there is deliberately NO `as_str`, NO `Deref`, NO `Into<String>`.
/// `let Notify::Webhook(u) = &deps.notify else { return; }` still *matches* — it simply yields a
/// `&WebhookUrl` that nothing in `deliver()`'s signature accepts. The only path to a `&str` is
/// [`Notify::sendable`], which logs every dark face on the way past.
///
/// ⚠️ **Adding an accessor here re-opens the bug this type exists to close.** The silent drop
/// became impossible to WRITE; an accessor makes it merely discouraged again.
///
/// # Why a TYPE and not a rule — measured, 2026-09-23
///
/// Commit `525dc10` (#171, 2026-08-07) added, in ONE diff, both the prohibition at `:458` —
/// *"A use-time `else { return; }` can never distinguish deliberately off from someone dropped
/// the env var — that collapse IS the defect"* — and, at `:4740`, that exact collapse. **The rule
/// and its violation shipped together.** (OMBB's find, from the tree.)
///
/// ⭐ **And Lilith's reading of his own receipt is the argument this type rests on. Two distances,
/// both from that commit:**
/// - prohibition `:458` → violation `:4740` = **4,282 lines**. Too far to be holding in mind.
/// - the reviewer's own gate comment (`GATE MAJOR 2 (OMBB, #171)`) → the defect = **4 lines**.
///   Attention demonstrably on that screen, actively gating, raising a major about a neighbouring
///   field.
///
/// - and a **third**, added by that same reviewer an hour later: `bell.rs:111` — the line that
///   refuted his own claim about this change — was **in output he had printed 40 seconds earlier**,
///   read past because he was looking at `:115` for a different reason.
///
/// ⇒ ***There is no distance at which prose works.*** Four thousand lines away it is unread; four
/// lines away, under an active reviewer, it is read past; four lines away **in your own terminal,
/// printed by you, for another purpose**, it is read past again. The mechanism is his:
/// ***attention is aimed by the question you are asking, not by proximity.*** "Read it more
/// carefully" is a remedy whose experiment has been run three times and lost three times.
/// **That is why the guarantee is a private field and not a sentence.**
///
/// 🔑 The general form, from the same morning and three other artifacts that each stated their own
/// rule in their own header and were violated anyway: ***a rule written inside the artifact it
/// governs is not enforcement — it is decoration with good intentions.*** (Lilith's wording.)
#[derive(Clone, Debug)]
pub struct WebhookUrl(String);

/// The ONE literal the CloudWatch metric filter matches, and the only machine contract in this
/// module. **Fixed forever; never reword it.** The prose around it is for humans and SHOULD churn
/// — that is exactly why the two are separated (Lilith, 2026-09-23: *the uniform record is a
/// machine contract with a stable key; the bespoke one is prose that should churn. Fold them and
/// the day someone improves the wording, the alarm dies silently.*)
///
/// It is a single token rather than a structured field because **these logs are not JSON**
/// (`main.rs:81` is `tracing_subscriber::fmt()` in text mode and the crate's `json` feature is
/// off), and a `{ $.outcome = … }` filter pattern matches **nothing** against text — silently.
/// See Task 5.
pub const REGISTER_UNRESOLVED_NEEDLE: &str = "register_dark_unresolved";

impl WebhookUrl {
    pub fn new(url: String) -> Self {
        WebhookUrl(url)
    }
}
```

- [ ] **Step 4: Change the payload type and add the gate**

Change the enum variant:

```rust
pub enum Notify {
    Webhook(WebhookUrl),
```

And add, in `impl Notify`:

```rust
    /// The ONLY way to obtain a sendable URL. Every dark face logs itself, tagged with the
    /// register that went dark, before returning `None`.
    ///
    /// `Disabled` is `warn` — deliberate, operator-initiated silence. `Unresolved` is `error` —
    /// misconfiguration. That distinction is the entire point of separating the states, and until
    /// this gate existed it was only observable at INIT: a container that cold-started with an
    /// unreadable secret announced itself exactly once and then swallowed every page for its
    /// whole life. **Loud at birth, mute forever after.** Now it is loud at every send.
    ///
    /// Three arms, no wildcard: adding a `Notify` variant must fail to compile here.
    pub fn sendable(&self, reg: Register) -> Option<&str> {
        match self {
            Notify::Webhook(WebhookUrl(u)) => Some(u.as_str()),
            Notify::Disabled => {
                // INFO, not WARN. Lilith's review, 2026-09-23: *a dark register is a LEVEL, not an
                // edge* — a deliberate mute re-announced at WARN on every event is furniture that
                // teaches the eye to skip the line. The level is not invented: `bell.rs:115` today
                // emits this exact state at `info!` with the reasoning "loud, so a muted bell never
                // reads as broken", and this gate SUBSUMES that record — promoting it to WARN would
                // silently change the character of an existing production signal. Unresolved stays
                // `error!`: that one is a fault, and a fault is an edge.
                tracing::info!(
                    outcome = "register_dark",
                    register = reg.as_str(),
                    reason = "disabled",
                    "{}",
                    reg.note(DarkFace::Disabled)
                );
                None
            }
            Notify::Unresolved => {
                // The needle is INTERPOLATED from the const, not spelled again — one source of
                // truth for the string the CloudWatch filter matches (Task 5 explains why it must
                // be a single literal token and not a `$.field` pattern). The register's own
                // sentence rides the same event as a second field: one record, two lifetimes.
                tracing::error!(
                    outcome = "register_dark",
                    register = reg.as_str(),
                    reason = "unresolved",
                    "{}: {}",
                    REGISTER_UNRESOLVED_NEEDLE,
                    reg.note(DarkFace::Unresolved)
                );
                None
            }
        }
    }
```

- [ ] **Step 5: Fix every construction site the compiler names**

Run: `cargo build --workspace 2>&1 | grep -E '^error' | head -40`

Every `Notify::Webhook(some_string)` becomes `Notify::Webhook(WebhookUrl::new(some_string))`. **Work the compiler's list; do not grep for them by hand** — the compiler's census is exhaustive and a grep is a guess about spelling.

`main.rs:178`'s startup census arm `Notify::Webhook(_) => {}` keeps compiling unchanged: it asks *is it configured*, never *give me the URL*. That distinction is exactly what the newtype preserves — leave it alone.

- [ ] **Step 6: Run the test and watch it pass**

Run: `cargo test -p fulfillment --test handler_test sendable_returns_url_only`
Expected: PASS

- [ ] **Step 7: Assert the structural claim — a PASSING doctest first, then the compile-fail**

⚠️ **The permit arm goes first and this is not stylistic.** A `compile_fail` doctest passes when the snippet fails to compile *for any reason at all* — a typo, a missing import, a renamed type. On its own it is the purest form of a vacuous green. The passing twin proves the snippet's scaffolding is sound, so the failing twin's failure can only be the private field. The error code is pinned for the same reason.

Add to `WebhookUrl`'s doc comment in `crates/fulfillment/src/lib.rs`:

```rust
/// # The gate is the only door — asserted, not merely documented
///
/// Going through `sendable()` compiles:
///
/// ```
/// use fulfillment::{Notify, Register, WebhookUrl};
/// let n = Notify::Webhook(WebhookUrl::new("https://example.invalid/h".to_string()));
/// let url: Option<&str> = n.sendable(Register::Ops);
/// assert_eq!(url, Some("https://example.invalid/h"));
/// ```
///
/// Reaching around it does **not** — the field is private (E0616), so the silent-drop shape
/// cannot be written even though the pattern still matches:
///
/// ```compile_fail,E0616
/// use fulfillment::{Notify, WebhookUrl};
/// let n = Notify::Webhook(WebhookUrl::new("https://example.invalid/h".to_string()));
/// let Notify::Webhook(u) = &n else { unreachable!() };
/// let _leaked: &str = &u.0;   // E0616: field `0` of struct `WebhookUrl` is private
/// ```
```

- [ ] **Step 8: Run the doctests and verify BOTH arms**

Run: `cargo test -p fulfillment --doc 2>&1 | tail -20`
Expected: both doctests reported, `test result: ok`. **Read the count** — if only one ran, the other was not collected and its guarantee is not asserted.

- [ ] **Step 9: Prove the compile-fail arm can FAIL — temporarily make the field public**

Run:
```bash
sed -i 's/^pub struct WebhookUrl(String);/pub struct WebhookUrl(pub String);/' crates/fulfillment/src/lib.rs
cargo test -p fulfillment --doc 2>&1 | tail -20
```
Expected: the `compile_fail` doctest now **FAILS** (the snippet compiled when it must not). *This is the control: an assertion that has never been seen to fail is unexercised, not sound.*

Then revert and confirm green:
```bash
sed -i 's/^pub struct WebhookUrl(pub String);/pub struct WebhookUrl(String);/' crates/fulfillment/src/lib.rs
cargo test -p fulfillment --doc 2>&1 | tail -5
```

- [ ] **Step 10: Commit**

```bash
git add crates/fulfillment/src/lib.rs crates/fulfillment/tests/handler_test.rs
git commit -S -m "🎛️ the gate: Register, WebhookUrl, and sendable() — a silent drop stops compiling"
```

---

### Task 2: `ping_msg` through the gate — ops-dark becomes an observable

**Files:**
- Modify: `crates/fulfillment/src/lib.rs:4739-4742` (`ping_msg`'s let-else)
- Test: `crates/fulfillment/tests/handler_test.rs`

**Interfaces:**
- Consumes: `Notify::sendable(Register) -> Option<&str>`, `Register::Ops` (Task 1)
- Produces: no new API. Behavioural: `ping_msg` on a dark ops register now emits `outcome="register_dark" register="ops"`.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn ping_msg_on_a_dark_ops_register_says_so() {
    // "emits nothing" IS the bug. A test asserting only "does not panic" would have passed
    // every one of the 78 days the prod alarm was being swallowed.
    let Some(store) = store_or_skip("sw-ping-dark").await else {
        return;
    };
    let (log_buf, _capture) = capture_logs();

    // `deps` is the fixture at handler_test.rs:135 — `fn deps(store: Store, humble_uri: &str,
    // webhook_url: Option<String>) -> Deps`. `Store` is NOT Clone (main.rs reconstructs it
    // per-invoke), so build ONE Deps and re-point `notify` between iterations.
    let mut d = deps(store, "http://humble.invalid", None);

    for (notify, want_reason) in [
        (fulfillment::Notify::Disabled, "disabled"),
        (fulfillment::Notify::Unresolved, "unresolved"),
    ] {
        d.notify = notify;
        fulfillment::ping_msg(&d, &fulfillment::OperatorMessage::literal("knock knock")).await;

        let logs = String::from_utf8(log_buf.lock().unwrap().clone()).unwrap();
        assert!(
            logs.contains(r#"register="ops""#) && logs.contains(&format!(r#"reason="{want_reason}""#)),
            "a dark ops register returned in silence ({want_reason}): {logs}"
        );
    }
}
```

**Decided, not left to the implementer:** `ping_msg` becomes **`pub`** (it is `pub(crate)` today).
No new test-only feature flag for one function, and no `#[cfg(test)]` accessor — the integration
test is a separate crate and needs the real symbol. Say so in the commit message.

⚠️ **`OperatorMessage::plain` DOES NOT EXIST — this plan said so in an earlier draft and it was
wrong.** The real constructors are `fmt` (59 call sites), `literal` (21) and `with` (3), in
`crates/fulfillment/src/operator_message.rs:139`. Use `literal` for a fixed string.

- [ ] **Step 2: Run it and watch it fail**

Run: `DYNAMODB_LOCAL_URL=http://localhost:8000 cargo test -p fulfillment --test handler_test ping_msg_on_a_dark_ops_register`
Expected: FAIL — the assertion fires with empty or record-less logs, because the bare `return` emits nothing.

- [ ] **Step 3: Route it through the gate**

Replace `lib.rs:4740-4742`:

```rust
    let Notify::Webhook(url) = &deps.notify else {
        return;
    };
```

with:

```rust
    // The let-else this function used to open with is the exact shape its own callers'
    // doc comments forbid ("Match all three states; never flatten with a let-else").
    // It had no log, no metric and no row, so every dark-gate in the app escalated INTO
    // silence — you cannot notify that notification is broken.
    let Some(url) = deps.notify.sendable(Register::Ops) else {
        return;
    };
```

- [ ] **Step 4: Run it and watch it pass**

Run: `DYNAMODB_LOCAL_URL=http://localhost:8000 cargo test -p fulfillment --test handler_test ping_msg_on_a_dark_ops_register`
Expected: PASS

- [ ] **Step 5: Run the whole suite — this function is on every escalation path**

Run: `cargo test -p fulfillment 2>&1 | tail -15`
Expected: no new failures. Borrow note: `sendable` borrows `deps.notify` for the life of `url`; `deliver(&deps.http, url, &chunk)` borrows a different field, so this compiles. If the borrow checker objects, take `let url = url.to_owned();` — do **not** widen `sendable`'s signature to dodge it.

- [ ] **Step 6: Commit**

```bash
git add crates/fulfillment/src/lib.rs crates/fulfillment/tests/handler_test.rs
git commit -S -m "🎛️ ping_msg through the gate — the escalation path stops escalating into silence"
```

---

### Task 3: The whisper and lantern gates take their URL from `sendable` — ONE record, and the doc that taught the old shape

**Files:**
- Modify: `crates/fulfillment/src/lib.rs:4817` (`resolve_whisper_url`), `:5162` (`resolve_lantern_url`)
- Test: `crates/fulfillment/tests/handler_test.rs`

**Interfaces:**
- Consumes: `Notify::sendable`, `Register::{Whisper, Lantern}` (Task 1)
- Produces: no signature change. Both still return `Option<String>`. Behavioural: a dark whisper/lantern emits **one** record — `outcome="register_dark"` (the machine contract) carrying this register's sentence as a message field (the churnable prose). The bespoke `whisper_dark` / `lantern_dark` / `*_unresolved` outcomes are **retired**; measured 2026-09-23, each had exactly one hit in the tree — its own emitter — and **zero machine consumers**. The actionable operator PING is unchanged: it is a webhook payload interpolating `whisper_param_name`, not a log line, and it still fires from these gates.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn a_dark_whisper_emits_one_record_carrying_both_lifetimes() {
    let (log_buf, _capture) = capture_logs();

    let Some(store) = store_or_skip("sw-whisper-dark").await else {
        return;
    };
    // The fixture at handler_test.rs:135; whisper_notify defaults to Disabled there already,
    // set explicitly so the test states the state it is testing.
    let mut d = deps(store, "http://humble.invalid", None);
    d.whisper_notify = fulfillment::Notify::Disabled;
    assert_eq!(fulfillment::resolve_whisper_url(&d).await, None);

    let logs = String::from_utf8(log_buf.lock().unwrap().clone()).unwrap();

    // The machine contract: stable keys, what the filter reads. Must never churn.
    assert!(logs.contains(r#"outcome="register_dark""#), "lost the countable record: {logs}");
    assert!(logs.contains(r#"register="whisper""#), "the record does not say WHICH register: {logs}");
    assert!(logs.contains(r#"reason="disabled""#), "the record does not say WHICH face: {logs}");

    // The human half rides the SAME event as a message field, so improving the wording can
    // never kill the alarm. Asserted against the note table, not a hand-typed copy of it.
    assert!(
        logs.contains(fulfillment::Register::Whisper.note(fulfillment::DarkFace::Disabled)),
        "the record lost this register's own sentence: {logs}"
    );

    // EXACTLY ONE record for one event — the volume property the one-record shape was chosen for.
    assert_eq!(
        logs.matches(r#"outcome="register_dark""#).count(),
        1,
        "one dark event emitted more than one record: {logs}"
    );
}
```

**`Register::note` is `pub`** (Task 1) precisely so this assertion can read the table rather than a
copy of it. **Do not hand-type the sentence into the test** — a test that types the string it is
checking asserts only that you typed it twice.

- [ ] **Step 2: Run it and watch it fail**

Run: `DYNAMODB_LOCAL_URL=http://localhost:8000 cargo test -p fulfillment --test handler_test a_dark_whisper_emits_one_record`
Expected: FAIL — today only the bespoke `whisper_dark` record exists and no `register_dark` is emitted here.

- [ ] **Step 3: Rewrite both gates in one shape**

`resolve_whisper_url` becomes (and `resolve_lantern_url` takes the identical shape with `Register::Lantern`, `deps.lantern_notify`, and its own two messages **left byte-for-byte as they are**):

```rust
pub(crate) async fn resolve_whisper_url(deps: &Deps) -> Option<String> {
    // sendable() FIRST: the healthy path returns here, and each dark face emits THE record on
    // the way past — machine keys plus this register's own sentence, one event, two lifetimes.
    // The match below adds only the actionable PING, which is a webhook payload and not a log.
    if let Some(u) = deps.whisper_notify.sendable(Register::Whisper) {
        return Some(u.to_owned());
    }
    match &deps.whisper_notify {
        // NOT `unreachable!()`. sendable() returns Some for Webhook, so this arm is dead today —
        // but this is the notification path, whose whole contract is that it returns () and
        // cannot break fulfilment. A panic here would trade a silent drop for a louder outage.
        // Written out rather than wildcarded so a new Notify variant still fails to compile,
        // and returning None fails CLOSED if sendable's behaviour ever changes underneath it.
        Notify::Webhook(_) => None,
        Notify::Disabled => {
            // No `tracing::warn!(outcome = "whisper_dark")` here any more: sendable() emitted the
            // one record above, carrying this register's sentence from the note table. What
            // remains is the actionable PING — a webhook payload that interpolates the param
            // name, not a log line, and the half a metric filter was never going to read.
            ping_msg(deps, &OperatorMessage::fmt(
                "whisper is DARK — the attic has a voice and no throat. Light it: aws ssm put-parameter --name {} --type SecureString --overwrite --value <discord webhook url>",
                &[Part::Id(&deps.whisper_param_name)],
            ))
            .await;
            None
        }
        Notify::Unresolved => {
            ping_msg(deps, &OperatorMessage::fmt(
                "whisper webhook {} is configured but UNREADABLE — check ssm:GetParameter and the KMS grant. Do NOT overwrite the value; the stored secret may be fine and the fault is the read path.",
                &[Part::Id(&deps.whisper_param_name)],
            ))
            .await;
            None
        }
    }
}
```

- [ ] **Step 4: Fix the two doc comments that now contradict the code**

🔑 **Lilith's catch, 2026-09-23, and it is the one that would have outlived the fix.** Both gates'
doc comments say *"Match all three states; never flatten with a let-else"* (`lib.rs:4816`,
`:5161`). After this change the **canonical shape IS a let-else** — over `sendable()`. Leave those
sentences standing and a future reader finds the repo's two exemplary gates forbidding, by name,
the thing the repo's own gate now does. *That doc is why the old shape spread; it must change in
the same commit as the code, or it re-teaches the defect.*

Replace the rule clause in both (keep every other word):

- `:4816` — *"Match all three states; never flatten with a let-else."*
  → *"The URL comes from `Notify::sendable` and nowhere else: a let-else over `sendable()` is now
  the correct shape, because the gate logs the dark face before returning `None`. What is still
  forbidden is a let-else over the **variant** — `let Notify::Webhook(u) = … else` — which is why
  `WebhookUrl`'s field is private. Match all three arms below for the bespoke advice; no wildcard."*
- `:5161` — *"Match all three; never let-else."* → the same replacement, in that gate's voice.

- [ ] **Step 5: Run it and watch it pass**

Run: `DYNAMODB_LOCAL_URL=http://localhost:8000 cargo test -p fulfillment --test handler_test a_dark_whisper_emits_one_record`
Expected: PASS

- [ ] **Step 6: Run the whole suite**

Run: `cargo test -p fulfillment 2>&1 | tail -15`
Expected: no new failures. ⚠️ Any existing test asserting `whisper_dark` / `lantern_dark` / `whisper_unresolved` / `lantern_unresolved` must be UPDATED to the new record, not deleted — those outcomes are retired deliberately (zero machine consumers, measured) and the assertion should move to `outcome="register_dark"` + `register="..."`. **A deleted assertion and a migrated one look identical in a diff; migrate.**

- [ ] **Step 7: Commit**

```bash
git add crates/fulfillment/src/lib.rs crates/fulfillment/tests/handler_test.rs
git commit -S -m "🎛️ whisper + lantern take their url from the gate — and the doc that taught the old shape"
```

---

### Task 4: `bell_notify` — the bell gets its own register, and the plumbed bool goes

**Files:**
- Modify: `crates/fulfillment/src/lib.rs:544` (remove `bell_disabled: bool`), add `bell_notify: Notify`
- Modify: `crates/fulfillment/src/bell.rs:110-123` (`ring`'s off-switch and URL)
- Modify: `crates/fulfillment/src/main.rs:154-160` (resolve at init), `:308` (drop from `Deps`)
- Test: `crates/fulfillment/tests/handler_test.rs:150`, `:559`, `:9945`

**Interfaces:**
- Consumes: `Notify::sendable`, `Register::Bell` (Task 1)
- Produces: `Deps.bell_notify: Notify` replaces `Deps.bell_disabled: bool`. The env var `BELL_DISABLED` and its reader `bell_suppressed` (`main.rs:68`) are **unchanged** — only the bool's destination moves.

- [ ] **Step 1: Write the failing test — BOTH directions of the decoupling rule**

```rust
#[tokio::test]
async fn the_bell_and_the_whisper_cannot_dark_each_other() {
    // The defect was a ONE-WAY implementation of a symmetric rule: spec-attic-bell.md:87
    // states register-decoupling and illustrates only the direction that was built.
    // Both directions are asserted together so the asymmetry cannot come back.

    let Some(store) = store_or_skip("sw-bell-decouple").await else {
        return;
    };
    let discord = wiremock::MockServer::start().await;
    // Accept any POST and count them — WHO posted is the assertion, not what.
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .mount(&discord)
        .await;

    let mut d = deps(store, "http://humble.invalid", None);
    let hook = || fulfillment::Notify::Webhook(fulfillment::WebhookUrl::new(discord.uri()));

    // ① WHISPER_DISABLED must NOT dark the bell. This is the half that is broken today, and it
    //    is asserted through the WIRING (bell::ring), not by reading a field back off the
    //    fixture — a field-level assertion would only prove the test set the field.
    d.whisper_notify = fulfillment::Notify::Disabled;
    d.bell_notify = hook();
    let before = discord.received_requests().await.unwrap().len();
    fulfillment::bell::ring(&d, &<the BellEvent the neighbouring bell tests build>).await;
    assert_eq!(
        discord.received_requests().await.unwrap().len(),
        before + 1,
        "WHISPER_DISABLED reached the bell — the mute is still coupled"
    );

    // ② BELL_DISABLED must NOT dark the whisper. Already true; asserted so it stays true and the
    //    symmetric rule can never again be implemented in one direction only.
    d.whisper_notify = hook();
    d.bell_notify = fulfillment::Notify::Disabled;
    assert!(
        fulfillment::resolve_whisper_url(&d).await.is_some(),
        "BELL_DISABLED reached the whisper"
    );
}
```

⚠️ **`<the BellEvent …>` is the ONE thing this task does not spell out, deliberately and
narrowly: read `handler_test.rs:9945` and the bell tests around it and reuse the `BellEvent` they
already construct** — `bell::ring`'s `Unwrap` arm does a `store.get_link` lookup and returns early
on an unknown token, so an invented event would make this test pass by ringing nothing. **If you
cannot find one, extend the existing test at `:9945` instead of writing a new one; it already has
the wiremock + register scaffolding.** Do not invent a `BellEvent`.

- [ ] **Step 2: Run it and watch direction ① fail**

Run: `DYNAMODB_LOCAL_URL=http://localhost:8000 cargo test -p fulfillment --test handler_test the_bell_and_the_whisper_cannot_dark`
Expected: compile error (`bell_notify` does not exist). **That is the right failure** — the field is the fix.

- [ ] **Step 3: Move the bool's destination**

In `lib.rs`, replace the `bell_disabled: bool` field (`:544`) with:

```rust
    /// The BELL register: the whisper's CREDENTIAL (same `SecretRead`, one rotation event)
    /// resolved with the bell's OWN flag, `BELL_DISABLED` — so `WHISPER_DISABLED` cannot dark
    /// the bell and vice versa. This used to be a `bool` beside `whisper_notify` plus a call to
    /// `resolve_whisper_url`, which is the exact anti-pattern `lantern_notify`'s own doc names:
    /// routing through the whisper's gate re-couples the mutes. The lantern arc identified it,
    /// avoided it for the new register, and left this one standing in it.
    pub bell_notify: Notify,
```

In `bell.rs`, replace `:111-123` (the `if deps.bell_disabled` block **and** the `resolve_whisper_url` let-else) with:

```rust
    let Some(url) = deps.bell_notify.sendable(Register::Bell) else {
        // BELL_DISABLED ⇒ resolve() ⇒ Disabled ⇒ sendable() logged
        // `register_dark register="bell" reason="disabled"`. One switch, one lamp: the old
        // `outcome="bell_disabled"` record is preserved by that uniform record.
        return;
    };
```

In `main.rs`, after the lantern resolution (`:159`), add:

```rust
    // The BELL register: same credential read, its OWN flag — the third register resolved this
    // way and the last one that was not.
    let bell_disabled = bell_suppressed(|k| std::env::var(k).ok());
    let bell_notify = Notify::resolve(whisper_read, bell_disabled);
```

and change `:159` to clone rather than move, since the bell now takes the last use:

```rust
    let lantern_notify = Notify::resolve(whisper_read.clone(), lantern_disabled);
```

In the `Deps` construction (`main.rs:308`), replace
`bell_disabled: bell_suppressed(|k| std::env::var(k).ok()),` with `bell_notify,`.

**Note:** this moves the `BELL_DISABLED` read from per-invocation to init. That matches how `whisper_notify` and `lantern_notify` already work, and a Lambda container's environment does not change within its life.

- [ ] **Step 4: Update the three test sites the compiler names**

Run: `cargo build --workspace 2>&1 | grep -E '^error' | head -20`

`handler_test.rs:150` and `:559` set `bell_disabled: false` → `bell_notify: Notify::Webhook(WebhookUrl::new("https://discord.example/hook".to_string()))`.
`handler_test.rs:9945` sets `d.bell_disabled = true;` → `d.bell_notify = Notify::Disabled;`.

- [ ] **Step 5: Run the test and the suite**

Run: `DYNAMODB_LOCAL_URL=http://localhost:8000 cargo test -p fulfillment --test handler_test the_bell_and_the_whisper_cannot_dark`
Expected: PASS

Run: `cargo test -p fulfillment 2>&1 | tail -15`
Expected: no new failures.

- [ ] **Step 6: Construct the disagreement — the test that licenses the DELETION**

🔑 **Lilith's method, 2026-09-23, and it is stronger than the argument it replaces.** My reason for
deleting the bool was *"operator muscle memory is unaffected"* — which is true and is not a warrant.
Hers: **construct a state where the bool and the gate disagree, in both directions. Unconstructible
⇒ pure subsumption, delete it. Constructible ⇒ you have a finding, not a refactor.**

```rust
#[test]
fn the_bool_and_the_gate_cannot_disagree() {
    // Direction ①: flag set ⇒ can the gate still be sendable? `Notify::resolve` checks the flag
    // FIRST and it beats a resolved secret (its doc: "ORDER MATTERS AND IS DELIBERATE"), so a
    // set flag can only ever yield Disabled. Asserted over BOTH secret outcomes, not argued.
    for read in [
        fulfillment::SecretRead::Value("https://discord.example/hook".to_string()),
        fulfillment::SecretRead::DeliberatelyOff,
    ] {
        assert!(
            matches!(fulfillment::Notify::resolve(read, true), fulfillment::Notify::Disabled),
            "flag set but the gate is not dark — the bool is NOT subsumed and deleting it is a behaviour change"
        );
    }

    // Direction ②: flag clear but the gate dark (unreadable secret). The old code returned early
    // on the bool and then ALSO returned on the dark resolve — same outcome, two switches. The
    // gate alone must still refuse.
    assert_eq!(
        fulfillment::Notify::resolve(fulfillment::SecretRead::ReadFailed, false)
            .sendable(fulfillment::Register::Bell),
        None,
        "flag clear and secret unreadable must still be unsendable"
    );
}
```

🔑 **WHY CONSTRUCTION AND NOT JUST READING `resolve()` — Lilith vs OMBB, 2026-09-23, settled by a
grep.** He argued the question is decidable by reading `Notify::resolve`, whose six cells are
already enumerated. She held that reading `resolve()` settles what the **gate** does, while the
disagreement question is about the **bool's own dependents** — a different set — and that if any
one of them reads the bool directly, the gate's matrix cannot speak for it. **She is right, and it
is a grep:** of the bool's six dependents, `bell.rs:111` (`if deps.bell_disabled`) reads the bool
**directly, never through `resolve()`**. So the read does not close it and the construction below
earns its place.

**Implementer note:** `SecretRead`'s real variant names are in `lib.rs` — **read them and use the
real ones**; the names above are the shape, not a promise. If a variant this test names does not
exist, that is the test telling you the matrix moved, not a licence to weaken the assertion.

📌 **Measured 2026-09-23 while answering this question, so the executor need not re-derive it:**
`outcome="bell_disabled"` has **zero machine consumers** — `grep -rn bell_disabled --include=*.rs
--include=*.tf` finds the emitter (`bell.rs:115`), the field, the three test sites and old plan
docs, and **nothing that asserts or matches the record**. `handler_test.rs:9945` uses the bool as
*setup* and asserts `discord.received_requests().len() == 1` — behaviour, not the log line. The
record can go.

- [ ] **Step 7: Verify the bool is gone everywhere**

Run: `grep -rn 'bell_disabled' --include=*.rs . | grep -v /target/`
Expected: **only** `main.rs`'s local `let bell_disabled` and `bell_suppressed`'s own definition/tests. No `deps.bell_disabled`, no `Deps { bell_disabled`. The `outcome = "bell_disabled"` string literal in `bell.rs` goes with the block it lived in.

- [ ] **Step 8: Commit**

```bash
git add crates/fulfillment/src/lib.rs crates/fulfillment/src/bell.rs crates/fulfillment/src/main.rs crates/fulfillment/tests/handler_test.rs
git commit -S -m "🎛️ the bell gets its own register — WHISPER_DISABLED stops darking it"
```

---

### Task 5: The bootstrap — an alarm that survives a dark ops register

**Files:**
- Modify: `terraform/aws-cloudwatch-alarms.tf`
- Test: `crates/fulfillment/tests/handler_test.rs`

**Interfaces:**
- Consumes: the `outcome="register_dark"` / `reason="unresolved"` record from Task 1
- Produces: `aws_cloudwatch_log_metric_filter.register_dark` + `aws_cloudwatch_metric_alarm.register_unresolved`

🔴 **READ THIS BEFORE WRITING TERRAFORM.** The spec's §4.5 says this follows "the existing lantern alarm pattern — the shape is in the repo." **Measured 2026-09-23: it is not.** `grep -rn metric_filter --include=*.tf .` returns **0**. All six existing alarms are `aws_cloudwatch_metric_alarm` over metrics **AWS emits for free** (Lambda `Errors`, `AWS/Scheduler` on a schedule group). This is the repo's **first** alarm that depends on the application's own log *content*, which means:

1. It is a **new resource type** — confirm the deploy role can `logs:PutMetricFilter` before promising the alarm exists.
2. **It is coupled to a string.** Rename the needle in Rust and the filter silently stops counting — an alarm that quietly stops alarming, which is *this spec's own subject shipped inside its own remedy*. Step 3 asserts the coupling so a rename breaks a test instead of the alarm.

🔴 **AND THE OBVIOUS PATTERN WOULD HAVE MATCHED NOTHING, FOREVER, SILENTLY.** This plan's first
draft used a JSON filter pattern — `{ $.outcome = "register_dark" && $.reason = "unresolved" }`.
**Measured 2026-09-23:** `main.rs:81` is

```rust
tracing_subscriber::fmt().with_ansi(false).without_time().init();
```

— tracing's **human text** format, and `crates/fulfillment/Cargo.toml:24` declares
`tracing-subscriber = "0.3"` with **no features**, so the `json` feature is off. **A CloudWatch
`{ $.field = … }` pattern only matches JSON log events.** Against text it matches zero events, the
metric never receives a datapoint, and `treat_missing_data = "notBreaching"` holds the alarm at
`INSUFFICIENT_DATA`/`OK` **forever** — an alarm that has never been able to fire, wearing green.
⇒ ***The defect this whole spec exists to remove, rebuilt inside its own remedy.*** The pattern is
therefore a **plain-text match on one literal token**, which is also why
`REGISTER_UNRESOLVED_NEEDLE` is a `const` (Task 1) rather than a field name.

📌 **The alternative was considered and REJECTED for this arc, not overlooked:** switching the
subscriber to `.json()` would make every future filter trivial — and it rewrites the format of
every log line in a production Lambda, inside a PR about notification gating. That is a
cross-cutting change owed its own review. **Open it as a follow-up issue** (`log format: emit JSON
so CloudWatch filters can address fields`) and link it from the terraform comment.

- [ ] **Step 1: Write the metric filter and alarm**

Append to `terraform/aws-cloudwatch-alarms.tf`:

```hcl
# ── the switchboard: a register that cannot announce its own darkness ─────────────────────────
# `ping_msg` cannot report that `ping_msg` is dark. The only channel that does not depend on the
# thing being reported is the log itself, so this alarm rides a metric filter over the log and
# NOT the ops webhook. A surface you must visit is not an alarm, and a message that rides a
# register cannot report that register.
#
# ⚠️ FIRST log-content-derived alarm in this repo — every other one here rides a metric AWS emits
# on its own. The filter pattern below is coupled to the exact strings the Rust gate emits; that
# coupling is asserted by `the_alarm_and_the_code_agree_on_the_string` in handler_test.rs so a
# rename breaks a test rather than the alarm.
resource "aws_cloudwatch_log_metric_filter" "register_dark" {
  name = "${module.label.id}-register-dark"
  # 🔴 THIS TERRAFORM MANAGES NO LOG GROUP — `grep -n aws_cloudwatch_log_group terraform/*.tf`
  # returns ZERO. An earlier draft referenced `aws_cloudwatch_log_group.fulfillment.name` and would
  # have failed at plan time on an undeclared resource. The group is created implicitly by Lambda,
  # so the name is DERIVED the same way `fulfillment_silent` addresses the function (`:50`).
  # ⚠️ Confirm against the lambda module's outputs before applying; if it exposes a log-group name
  # or ARN output, USE THAT rather than reconstructing the convention by hand.
  log_group_name = "/aws/lambda/${module.lambda_fulfillment.lambda_function_name}"

  # PLAIN-TEXT literal, NOT a `{ $.field = ... }` JSON pattern: these logs are tracing's text
  # format (main.rs:81; the crate's `json` feature is off), and a JSON pattern matches zero text
  # events — silently, forever. The quoted form below is a substring match, which text logs do
  # support. The token itself is `fulfillment::REGISTER_UNRESOLVED_NEEDLE`, asserted equal to this
  # literal by `the_alarm_and_the_code_agree_on_the_string`.
  #
  # Only `unresolved` carries this token — `disabled` is operator-initiated silence and must never
  # page, which is enforced by the needle being absent from that arm rather than by a filter clause.
  pattern = "\"register_dark_unresolved\""

  metric_transformation {
    name      = "RegisterUnresolved"  # keep in sync with the alarm's metric_name below
    namespace = "bendobundles/switchboard"
    value     = "1"
    unit      = "Count"
    # No default_value: absent data must read as "no signal", not as a stream of zeros that
    # would keep the alarm permanently OK even if the log group stopped receiving entirely.
  }
}

resource "aws_cloudwatch_metric_alarm" "register_unresolved" {
  alarm_name        = "${module.label.id}-register-unresolved"
  alarm_description = "A notification register is configured but UNREADABLE. This alarm rides the log, not the ops webhook, because a dark ops register cannot report itself."

  namespace           = "bendobundles/switchboard"
  metric_name         = "RegisterUnresolved"
  statistic           = "Sum"
  # CloudWatch enforces TWO limits invisible to `terraform validate`: period <= 86400 AND
  # period * evaluation_periods <= 86400, both rejected only at APPLY time. Documented on
  # `fulfillment_silent` in this same file after a gate review found them the hard way; 300x1 is
  # comfortably inside both. Read that comment before widening this window.
  period              = 300
  evaluation_periods  = 1
  threshold           = 1
  comparison_operator = "GreaterThanOrEqualToThreshold"
  treat_missing_data  = "notBreaching"

  # NOT a splat: `aws_sns_topic.ops_alarms` has no `count` in this file, and every existing alarm
  # here writes it exactly this way (`:41`, `:63`, `:92`, `:115`, `:140`, `:160`).
  alarm_actions = [aws_sns_topic.ops_alarms.arn]
  tags          = module.label.tags
}
```

**Implementer note — VERIFIED 2026-09-23, not assumed:** `aws_sns_topic.ops_alarms` exists at
`:17` with **no `count`**, and `aws_sns_topic_subscription.ops_alarms_email` (`:23`) points at
`var.ops_alarm_email` with the comment *"ben confirms the subscription once by mail"*. ⇒ **this
alarm's destination is a mail subscription Ben has already confirmed**, not the shared agent ops
webhook — so it does **not** inherit the readership problem measured on that webhook this morning
(seven seats, one channel, at least two of seven cannot read it). `module.label` is used by the
neighbouring alarms; **read the file and match whichever label module they use** — `label_lantern`
and `label_alarms` both appear, and picking the wrong one only changes the resource's name.

- [ ] **Step 2: Validate the terraform**

Run: `cd terraform && terraform fmt -check && terraform validate`
Expected: clean. (`terraform init` first if the working dir is fresh.)

- [ ] **Step 2b: Decide the absence question — and record the decision, not just the choice**

🔑 **Lilith's review, 2026-09-23:** *"Filter over the log is right, and it dies the same way yours
did. Alarm on ABSENCE: emit the uniform record on a heartbeat, page when it stops. Then a dead
filter and a dark log both surface."*

She is right about the hole. The coupling test (Step 3) catches a **rename**; it cannot catch a
deleted filter, a broken log pipeline, or a namespace typo — each of which leaves a permanently
quiet metric that reads as healthy.

**Decision for this arc: DO NOT build the heartbeat. Do write down what stays open.** Reasons,
measured rather than asserted:
- `aws_cloudwatch_metric_alarm.fulfillment_silent` (same file, `:45`) **already pages when the
  lambda has not been invoked in 24h**, which covers the "nothing is running" half of a dark log.
- What it does *not* cover is "the lambda runs, the filter is broken." Closing that needs a
  positive heartbeat record plus a second alarm — new emission on a hot path, for an infra failure
  mode, inside a PR about notification gating. **Same scope argument as the JSON switch above, and
  it should be the same follow-up issue's sibling.**
- ⚠️ **Her second half is the cheaper and more urgent one and IS actionable now:** *"check the log
  path shares no config loading with notify, or one fault takes both."* **Verify during execution:**
  the subscriber is initialised at `main.rs:81`, **before** any SSM read — so a secret fault cannot
  dark the log. Assert that ordering holds when you touch `main.rs` in Task 4; if a future change
  moves subscriber init below the secret reads, the alarm and the thing it watches share a fault.

⇒ **Open the follow-up issue as part of this task** so the residual is a tracked item and not a
paragraph in a plan nobody re-reads: *"switchboard residuals: JSON log format + a heartbeat so a
dead metric filter surfaces."*

- [ ] **Step 3: Write the failing coupling test**

```rust
#[test]
fn the_alarm_and_the_code_agree_on_the_string() {
    // The alarm is a STRING MATCH against log content. Nothing in Rust's type system knows the
    // filter exists, so a rename of `register_dark` would leave a green build, a green test
    // suite, and an alarm that has quietly stopped counting — the exact defect the switchboard
    // was built to remove, reintroduced by its own remedy. This test is the coupling.
    let tf = std::fs::read_to_string(
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../terraform/aws-cloudwatch-alarms.tf"),
    )
    .expect("cannot read the alarms terraform — if this file moved, fix the path, do not delete the test");

    // Positive control: prove we are reading the right file before trusting any absence below.
    assert!(
        tf.contains("aws_cloudwatch_log_metric_filter"),
        "read a terraform file with no metric filter in it — the path is wrong and every assertion below would be vacuous"
    );

    // Emit a real dark record and take the needles from the OUTPUT, not from memory.
    let (log_buf, _capture) = capture_logs();
    assert_eq!(fulfillment::Notify::Unresolved.sendable(fulfillment::Register::Ops), None);
    let logs = String::from_utf8(log_buf.lock().unwrap().clone()).unwrap();

    // ONE needle, taken from the const — not two hand-typed words. The const is the single
    // source of truth; this test asserts the terraform literal and the emitted text still agree
    // with it, in that order.
    let needle = fulfillment::REGISTER_UNRESOLVED_NEEDLE;
    assert!(
        logs.contains(needle),
        "the gate no longer emits its own needle {needle:?}: {logs}"
    );
    assert!(
        tf.contains(needle),
        "the gate emits {needle:?} but the metric filter pattern does not contain it — the alarm has stopped counting"
    );

    // The needle must NOT ride the `disabled` arm: a deliberate mute must never page.
    let (dis_buf, _dis) = capture_logs();
    assert_eq!(fulfillment::Notify::Disabled.sendable(fulfillment::Register::Ops), None);
    let dis = String::from_utf8(dis_buf.lock().unwrap().clone()).unwrap();
    assert!(
        !dis.contains(needle),
        "the DISABLED face carries the alarm's needle — an operator asking for silence would page: {dis}"
    );

    // The pattern is a plain-text match, so the token must survive as a CONTIGUOUS substring of
    // the rendered line. A structured-field rendering that split or escaped it would pass the
    // `contains` above on the fields and still never match in CloudWatch.
    assert!(
        logs.lines().any(|l| l.contains(needle)),
        "the needle does not appear contiguously on any single rendered line: {logs}"
    );
}
```

- [ ] **Step 4: Run it and watch it pass, then prove it can FAIL**

Run: `DYNAMODB_LOCAL_URL=http://localhost:8000 cargo test -p fulfillment --test handler_test the_alarm_and_the_code_agree`
Expected: PASS

Now the control — break the coupling on purpose and confirm the test notices.

🔴 **SABOTAGE THE CONST, NOT THE `outcome` FIELD — an earlier draft of this step got this wrong and
the control could not have fired.** It renamed `outcome = "register_dark"`, while the test asserts
on `REGISTER_UNRESOLVED_NEEDLE`, a **separate** const. Every assertion stayed true, so the step
would have printed PASS where it says *Expected: FAIL* — and the executor's only options are to
stall or to "fix" the test until it fails. ***A control aimed at a string its test does not read is
the defect this whole task exists to prevent, rebuilt inside the proof that the task works.***

```bash
sed -i 's/"register_dark_unresolved"/"register_dark_GLOOMY"/' crates/fulfillment/src/lib.rs
DYNAMODB_LOCAL_URL=http://localhost:8000 cargo test -p fulfillment --test handler_test the_alarm_and_the_code_agree 2>&1 | tail -5
```
Expected: **FAIL**, and read WHICH assertion fired — it must be the `tf.contains(needle)` one
("the metric filter pattern does not contain it"), because the code moved and the terraform did
not. A failure on any other line means the control is testing something else. Then revert:
```bash
sed -i 's/"register_dark_GLOOMY"/"register_dark_unresolved"/' crates/fulfillment/src/lib.rs
DYNAMODB_LOCAL_URL=http://localhost:8000 cargo test -p fulfillment --test handler_test the_alarm_and_the_code_agree 2>&1 | tail -5
```
Expected: PASS. *A guard that has never been seen to say no is unexercised.*

- [ ] **Step 5: Full suite + clippy**

Run: `cargo test --workspace --no-fail-fast 2>&1 | tail -20 && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -10`
Expected: green both.

- [ ] **Step 6: Commit**

```bash
git add terraform/aws-cloudwatch-alarms.tf crates/fulfillment/tests/handler_test.rs
git commit -S -m "🎛️ the bootstrap: an alarm that rides the log, and a test that keeps it coupled"
```

---

## Deploy (after the PR merges — step 11 of the pounce arc)

Per `terraform/README.md` "Deploying as kitten" → full deploy, and `docs/runbook-158-deploy.md`:

```bash
cd terraform
AWS_PROFILE=kitten-deploy terraform plan -out tf.tfplan    # show this output to Ben BEFORE apply
AWS_PROFILE=kitten-deploy terraform apply tf.tfplan
```

⚠️ `production.tfvars` carries **6** keys and **not** `admin_password_hash` — a plan stops on *"No value for required variable"* if you assume otherwise. Never re-hash the plaintext for a routine apply.

🔴 **BEFORE APPLY — THE ALARM'S DESTINATION MAY BE DEAD AND EVERY TEST WOULD STILL PASS.**
Lilith, 2026-09-23: **an unconfirmed SNS email subscription sits in `PendingConfirmation`
indefinitely, and every `Publish` to the topic still returns a MessageId.** Success at the API,
nothing in the mailbox. *Terraform declares the subscription; it cannot declare the click.*

```bash
aws sns list-subscriptions-by-topic --topic-arn <ops_alarms arn>
```

**If the only subscription reads `PendingConfirmation`, this alarm is dead before Task 1 and so are
the six that already ride that topic.** ⚠️ **NOT MEASURED from either of my seats** —
`SNS:ListTopics` is denied to both `kitten-debug` and `kitten-deploy` (`AuthorizationError`,
rc=254, measured 2026-09-23). The prior is "confirmed" because six alarms already point there; **a
prior is not a measurement**, and *"six other alarms also point at a mailbox nobody clicked"* is
exactly the shape of this morning's 204. **Carry it as an explicit unknown and settle it at step 12
or ask Ben; do not let it become an assumption inside the plan.**

**Deploy verification (step 12) — the deploy is not done when apply returns:**
1. `aws logs describe-metric-filters --log-group-name "/aws/lambda/<fn>"` shows `*-register-dark`.
2. The alarm exists and is in `OK` or `INSUFFICIENT_DATA` — **not** `ALARM`. If it is in `ALARM` on arrival, a register really is unresolved in prod and that is a finding, not a deploy failure.
3. Force one real dark record if a safe path exists, and confirm the metric moves. **If it cannot be forced safely, say so plainly rather than reporting the alarm as verified** — an alarm that has never counted anything is a configuration, not an instrument.
4. 🔑 **ASSERT RECEIPT, NOT PUBLISH SUCCESS.** OMBB's corpse, measured this morning: eight operator
   pages returned a real HTTP **204** into a room he is not allowlisted to read — *8 sent, 0
   received.* ***A MessageId is a 204.*** The acceptance test for this alarm is **a human
   confirming the mail arrived**, not an API returning 200. Anything less verifies the request and
   says nothing about the artifact.

---

## Self-Review

**1. Spec coverage.** §3.1 `Register` → Task 1. §3.2 `sendable` → Task 1. §3.3 the newtype → Task 1. §3.4 call sites → Tasks 2 (`ping_msg`), 3 (whisper/lantern), 4 (bell). §3.5 `bell_notify` + bool removal → Task 4. §4.5 the bootstrap → Task 5. §5 testing: `sendable` matrix → T1S1; compile-fail → T1S7-9 (mechanism decided: rustdoc `compile_fail,E0616` with a passing twin, zero new deps); bell decoupling both directions → T4S1; `ping_msg` dark record → T2S1. §2 non-goals: no ledger, no admin surface, no retry, no dead-letter, `Notify::resolve` untouched — nothing in any task violates these.

**Gap found and closed during review:** §4.5's claim that the metric-filter shape already exists in the repo is **false** (0 hits). Task 5 now leads with that correction, flags the new resource type, and adds the string-coupling test the spec never asked for — because a string-matched alarm with nothing asserting the string is this spec's own defect wearing the remedy's clothes.

**The family gate ANSWERED during step 3, and three of the spec's four recommendations lost.** Amendments folded in, each attributed where the reasoning came from:

| question | spec recommended | shipped | whose reasoning |
|---|---|---|---|
| Q1 `Unresolved` fatal at init? | no, loud at every send | **no, and `Disabled` drops to `info!`** | Lilith (a dark register is a LEVEL, not an edge). Her volume point conceded; her reading of `:455` as forbidding per-send records **rejected** — that line rejects *silent* no-ops and is about where RESOLUTION happens. The `info!` level is not invented: `bell.rs:115` already used it for this exact state. |
| Q2 two records or fold? | two records | **ONE record, two FIELDS** | OMBB, and it subsumes Lilith's reason — she wanted two lifetimes, he showed fields give that at half the volume on a per-send path. She withdrew her own Q2 as contradicting her Q1. |
| Q3 remove the bool? | yes (muscle memory unaffected) | **yes, on a CONSTRUCTED warrant** | Lilith. My reason was a hand-wave; OMBB's "just read `resolve()`" reads the gate's cells, not the bool's dependents — and `bell.rs:111` reads the bool directly. Settled by grep, not argument. |
| bootstrap | metric filter + alarm | **filter + alarm, absence deferred with the residual named, and fire-it-once at deploy** | both, independently. OMBB's corpse: 8 ops pages delivered `204` into a room nobody read. *A success response confirms the request, never the artifact.* |

**Found by my own measurement during the same pass, and it was a blocker:** the JSON filter pattern would have matched nothing forever (Task 5). **Found by Lilith, receipted by OMBB:** the two exemplary gates' doc comments forbid the shape the fix adopts, and must change in the same commit (Task 3, Step 4).

**2. Placeholder scan.** No TBDs. Three places name a judgement the implementer must make against the real file rather than a placeholder: `ping_msg`'s visibility (T2S1), the terraform resource names (T5S1), and the borrow fallback (T2S5). Each states the decision rule and the forbidden shortcut.

**3. Type consistency.** `Register`/`WebhookUrl`/`sendable` are spelled identically in Tasks 1-5. `WebhookUrl::new(String)` is the only constructor and is used that way at every site. `sendable` returns `Option<&str>` everywhere; the two `resolve_*_url` functions keep `Option<String>` and bridge with `.to_owned()`. `Deps.bell_notify` (Task 4) replaces `Deps.bell_disabled` and is referenced by that name in Task 4's test and nowhere earlier.
