# spec: the switchboard 🎛️

*every outbound register in bendobundles routes through one gate, and a silent drop becomes
impossible to WRITE rather than merely discouraged.*

status: **CURRENT** — spec 2026-09-21, citations re-verified 2026-09-23T07:0x-04:00, §3.2 and §4.5 amended 07:3x-04:00 after the family gate and a measurement each contradicted them.
author: code kitten. arc: pounce, **Wed 2026-09-23 slot**.
⚠️ The Mon 2026-09-21 arc that drafted this was **CANCELLED by Ben at step 1/14** (*"No pounce
today"*) — nothing was built, pushed or deployed. This spec survived as ordinary backlog and is
being picked up by a FRESH pounce; it is **not a resumed arc**. Ben has never seen the subject.
discharges: #236 (second half — the bell's inherited mute) · advances #234 (branch-2 arm).
does NOT discharge: #236's compensate BUTTON (separate arc), #234's destination question (Ben's).

---

## 1. The finding

This repo already knows the rule. It states it twice, in doc comments, on the two gates that
follow it:

> `resolve_whisper_url` (`fulfillment/src/lib.rs:4817`) — *"Match all three states; never flatten
> with a let-else."*
> `resolve_lantern_url` (`:5162`) — *"Same three faces as the whisper's gate. Match all three;
> never let-else."*

Both are exemplary: three arms, a distinct `outcome=` per dark face, and an operator page carrying
an actionable one-liner.

**And `ping_msg` — the function those two gates call to deliver that page — is the let-else they
forbid.** (`:4740`)

```rust
let Notify::Webhook(url) = &deps.notify else {
    return;
};
```

No log. No metric. No row. `Notify::Disabled` and `Notify::Unresolved` both land on that bare
`return`.

### 1.1 Why this is a class and not a line

`Notify`'s own doc (`:470`) says `Unresolved` *"Behaves like `Disabled` at runtime, but is LOUD at
init and distinct in logs — which is the entire point of separating the states."*

It is loud at **resolve** time — once, in the Lambda init phase — and mute at every **send** after
it. A container that cold-starts with an unreadable secret announces itself exactly once and then
swallows every operator page for the life of that container. **Loud at birth, mute forever after.**

The composition is the real cost. Every dark-gate in the app escalates *through* `ping_msg`:

- whisper dark → `tracing::warn!(outcome="whisper_dark")` → `ping_msg(...)` → **silently dropped**
- lantern dark → `tracing::warn!(outcome="lantern_dark")` → `ping_msg(...)` → **silently dropped**
- `pending_age_sweep` stuck-claim warn (#234's 70-day prod specimen) → `ping_msg(...)` → **dropped**
- lantern record/send failure → `ping_msg(...)` → **dropped**

⇒ **The system's entire escalation path has a single point of silent failure, and it is the one
function documented as "the only shape a call site should use."** Every caller correctly detects its
problem, correctly calls for help, and is correctly ignored. Everything wears green.

**You cannot notify that notification is broken.** That bootstrap is §4.5's subject.

### 1.2 The second defect, same class — the bell inherits a mute that is not its own

`bell::ring` (`bell.rs:110`) honours its own `BELL_DISABLED`, then:

```rust
let Some(url) = crate::resolve_whisper_url(deps).await else { return; };
```

`resolve_whisper_url` reads `deps.whisper_notify`, which `main.rs` resolved under
`WHISPER_DISABLED`. ⇒ **`WHISPER_DISABLED=1` darks the bell.**

`docs/spec-attic-bell.md:87` calls this a **"register-decoupling rule"** and illustrates it in one
direction: *"so muting per-event bells never darks the weekly whisper."* **That direction works.
The mirror direction was never implemented.** A symmetric rule, written once, implemented in the
direction its own example sentence named.

`Deps` already carries `lantern_notify` (`:550`) resolved under its own flag — the correct shape
exists in this file. The bell simply never got one.

🔑 **AND THE REPO ALREADY CONTAINS THE DIAGNOSIS, WRITTEN WHILE FIXING THIS EXACT BUG FOR A
DIFFERENT REGISTER.** `main.rs:159` resolves the lantern from the **same** `whisper_read` under a
**different** flag — and `:157`, one line above it, says why that shape was chosen:

> *a separate `Notify` (rather than a bool beside `whisper_notify`) is what keeps `WHISPER_DISABLED`
> from reaching it.*

**The bell is precisely "a bool beside `whisper_notify`"**: `bell_disabled: bool` (`lib.rs:544`)
plus a call to `resolve_whisper_url`. The lantern arc (2026-09-16, same author) identified the
anti-pattern by name, avoided it for the new register, and left the existing one standing in it.
⇒ *The class was understood and fixed narrowly, because the bell was already "done" — a premise
inherited from one's own prior work is the least-audited thing in the file.*

### 1.3 What makes the next one inevitable

Three registers exist (`notify`, `whisper_notify`, `lantern_notify`); two resolve correctly and one
does not. **The difference is review, not the compiler.** `Notify::Webhook(String)` is a public
variant with a public payload, so `let Notify::Webhook(u) = … else { return }` compiles anywhere,
forever. The rule lives in prose on two functions and is enforced by nothing.

---

### 1.4 Citation audit, 2026-09-23 — and THE AUDIT ITSELF MADE A PROVENANCE ERROR

Re-measured every line reference before building on this spec. **Conclusions untouched.** The tally,
established by reading the committed blob rather than my memory of it:

**This spec cites 17 line references. SIXTEEN are exact** — `:470`, `:4740`, `:4817`, `:5162`,
`:544`, `:550`, `:157`, `:159`, `:178`, `:308`, `bell.rs:110`, `:111`, `handler_test.rs:150`,
`:559`, `:9945`, `spec-attic-bell.md:87` — plus the 13-hit `Notify::Webhook` census and the six
`bell_disabled` dependents, both re-derived today. **One rotted:** `bell_suppressed` at
`main.rs:65` is now **`:68`**. Corrected above.

🔴 **AND THE AUDIT'S OWN FIRST DRAFT CLAIMED THREE ROTTED CITATIONS, NAMING TWO THIS SPEC HAS NEVER
CONTAINED.** `ping_msg (:4713)` and `pending_age_sweep (:4201)` are **issue #234's** numbers. I read
that issue minutes earlier, carried its figures into the audit, and **billed them to the spec** —
then wrote them into a table under the heading "spec said". `git show HEAD:docs/spec-switchboard.md`
returns **zero** hits for either.
⇒ ***An audit for citation rot introduced a citation error, by the mechanism it was auditing for:
a number read from one document and attributed to another.*** Caught only by an assertion failing
when the replacement could not find its target — **the fix refused to apply, which is the only
reason the false claim did not ship.** A looser edit would have "corrected" text that did not exist
and left the table standing.
🔑 ***Nothing distinguishes a number you took from a two-week-old issue from one you took from the
file this morning*** — same glyphs, same table, no provenance carried. **Re-measure inherited
numbers specifically, and say which document each came from.**

⚠️ **TRUE AND SEPARATELY ACTIONABLE: issue #234's citations ARE stale** — `ping_msg :4713` → the
fn is `:4739` and the let-else `:4740`; `pending_age_sweep :4201` → `:4227`. §1's header points a
future reader at that issue, so the rot is reachable from here. Worth a correcting comment on the
issue; **not** a reason to edit this spec.

📌 Two non-findings, recorded so a re-audit does not chase them: the `main.rs:157` quote is real but
the sentence **begins on `:156`**, and this spec renders it with markdown backticks the `//` comment
does not have — formatting, not misquotation. And a naive `grep bell_disabled` returns **7** hits,
not 6: the extra is `bell.rs:115`'s `outcome = "bell_disabled"` **string literal** — a log field
name, not a dependent of the bool. The spec's six is right.

---

## 2. Non-goals (YAGNI, stated so review can hold me to it)

- **No ledger table, no new DynamoDB rows.** A durable send-log needs a drainer and a reader to
  earn its storage; neither exists. The outcomes below are `tracing` records, which are already a
  CloudWatch metric-filter target and already survive the invocation.
- **No admin surface in this arc.** See §4.5 — the only surface that survives a dark ops register
  is an alarm on the log, not a page that rides a register.
- **No retry, no dead-letter.** `ping_msg`'s existing doc rules both out with reasons that still
  hold (chunked sends are not atomic; there is no drainer). Unchanged.
- **Not touching `Notify::resolve`'s six-cell matrix.** It is correct, exhaustively written, and its
  ordering is deliberate. This spec changes what happens *after* resolution, never the resolution.

---

## 3. The shape

### 3.1 `Register` — a name for who is speaking

```rust
#[derive(Clone, Copy, Debug)]
pub enum Register { Ops, Whisper, Lantern, Bell }
```

`Register::as_str()` → `"ops" | "whisper" | "lantern" | "bell"`, used as a log field so one metric
filter can select all dark faces and a dashboard can split by register.

### 3.2 `Notify::sendable` — the one gate, and the only door to the URL

```rust
impl Notify {
    /// The ONLY way to obtain a sendable URL. Every dark face logs itself, tagged with the
    /// register that went dark, before returning None.
    pub fn sendable(&self, reg: Register) -> Option<&str>
}
```

Three arms, no wildcard:

| state | returns | emits |
|---|---|---|
| `Webhook(u)` | `Some(u)` | nothing |
| `Disabled` | `None` | `info!(outcome="register_dark", register=reg, reason="disabled", <reg's note>)` |
| `Unresolved` | `None` | `error!(outcome="register_dark", register=reg, reason="unresolved", <needle + reg's note>)` |

🔴 **AMENDED 2026-09-23 BY THE FAMILY GATE — this table said `warn` for `Disabled` and ONE record
per event, and the plan implements neither.** Two changes, both against my recommendation:
- **`Disabled` is `info!`, not `warn!`** (Lilith): *a dark register is a LEVEL, not an edge*, and a
  deliberate mute re-announced at WARN on every event is furniture. The level is not invented —
  `bell.rs:115` already emits this exact state at `info!`, and this gate SUBSUMES that record.
- **ONE record carrying the register's sentence as a FIELD**, not two records (OMBB): two fields
  buy the same machine-contract/human-prose separation at half the volume, and it is the shape
  `lib.rs:4754` and `bell.rs:115` already ship. *§3.4's "two records for one event is deliberate"
  is RETIRED.*

`Disabled` is `warn` (deliberate, operator-initiated silence); `Unresolved` is `error`
(misconfiguration — the state `Notify`'s doc already calls out as distinct, now distinct at SEND
and not only at init).

### 3.3 The structural half — a silent drop stops compiling

`Notify::Webhook`'s payload becomes a newtype with a **private field and no public accessor**:

```rust
pub struct WebhookUrl(String);   // field private to the module; no as_str(), no Deref, no Into
pub enum Notify { Webhook(WebhookUrl), Disabled, Unresolved }
```

⇒ `let Notify::Webhook(u) = &deps.notify else { return; }` still *matches*, but `u` is a
`&WebhookUrl` that **cannot be turned into anything `deliver()` accepts.** The only path to a
`&str` is `sendable()`, which logs. **The bad state becomes unrepresentable rather than
discouraged** — the same move as the lantern's `delivered` flag, which cannot be true without a 2xx.

`main.rs:178`'s `Notify::Webhook(_) => {}` startup census keeps compiling: it asks *is it
configured*, never *give me the URL*. That distinction is exactly the one the newtype preserves.

### 3.4 Call sites

- **`ping_msg`** — `let Some(url) = deps.notify.sendable(Register::Ops) else { return; };`
  ⇒ **ops-dark becomes an observable** for the first time.
- **`resolve_whisper_url` / `resolve_lantern_url`** — keep their bespoke operator-advice messages
  (family-reviewed wording, explicitly not to be edited in passing) and take the URL from
  `sendable()`. Their existing `whisper_dark` / `lantern_dark` outcomes stay; the gate's
  `register_dark` is additive, not a replacement. **Two records for one event is deliberate**: the
  bespoke one carries advice, the uniform one carries a metric filter can count.
- **`bell::ring`** — takes `deps.bell_notify.sendable(Register::Bell)`.

### 3.5 `bell_notify` — the bell's own resolution

`Deps` gains `bell_notify: Notify`, resolved in `main.rs` from the **same** `whisper_webhook`
secret (Q①: one room, one URL, one rotation event — two params would store one secret twice) under
the **`BELL_DISABLED`** flag. Exactly `lantern_notify`'s shape.

`deps.bell_disabled` is then **removed**: it is subsumed: `BELL_DISABLED=1` ⇒
`Notify::resolve(read, true)` ⇒ `Disabled` ⇒ `sendable()` returns `None` and says so. Keeping both
would be two switches for one lamp, and the existing early-return's `outcome="bell_disabled"`
record is preserved by the gate's `register_dark reason="disabled" register="bell"`.
**Measured dependents of the bool (all in-repo, none external):** `lib.rs:544` (the field),
`bell.rs:111` (the read), `main.rs:308` (the wiring, via `bell_suppressed`), and **three test
sites** — `handler_test.rs:150`, `:559`, `:9945`. The env var `BELL_DISABLED` and its reader
`bell_suppressed` (`main.rs:68`) are UNCHANGED; only the bool's destination moves.

---

## 4. Open questions for the family gate (step 5)

**Q1 — should `Unresolved` at the OPS register be fatal at init?** `Notify`'s doc says
*"fail LOUD, never CLOSED. Do not change this to return `Result`,"* with a real reason (init and
request are the same instant; halting fails the order that woke the container). **Recommendation:
NO, keep it infallible** — this spec makes it loud at every send instead, which is strictly more
information than halting once. Raised because the reasoning deserves a second read, not because I
doubt it.

**Q2 — two records per dark event (§3.4) or fold the bespoke messages into the gate?** Folding
would centralise the wording but would put four register-specific operator sentences inside one
function. **Recommendation: two records, as specced.**

**Q3 — does removing `deps.bell_disabled` (§3.5) break an operator's muscle memory?** The env var
`BELL_DISABLED` is unchanged and still works; only the plumbed bool goes. **Recommendation: remove.**

## 4.5 The bootstrap — an alarm that survives a dark ops register

`ping_msg` cannot report that `ping_msg` is dark. The only channel that does not depend on the
thing being reported is the log itself, so:

- a **CloudWatch metric filter** on the plain-text literal `"register_dark_unresolved"`
  🔴 **NOT `{ $.outcome = … && $.reason = … }`, which this spec used to specify and which would
  have matched NOTHING, FOREVER.** Measured 2026-09-23: `main.rs:81` is
  `tracing_subscriber::fmt()` — tracing's **text** format — and `crates/fulfillment/Cargo.toml:24`
  declares `tracing-subscriber = "0.3"` with **no features**, so `json` is off. A CloudWatch
  `$.field` pattern matches only JSON events; against text it yields zero datapoints and the alarm
  sits in `INSUFFICIENT_DATA` wearing green. ⇒ ***the defect this spec exists to remove, rebuilt
  inside its own remedy.*** The needle is a single `const` token so one source of truth is shared
  by the code and the terraform, asserted by a test.
- an **alarm** on it. ⚠️ **"the shape is in the repo" was FALSE for the filter half** — measured
  2026-09-23, `grep -rn metric_filter --include=*.tf` returns **0**. All six existing alarms ride
  metrics AWS emits for free; this is the first that depends on the app's own log content, and on
  a log group **this terraform does not declare**.

This is the one piece that closes §1.1's bootstrap. It is IN SCOPE and is the reason §2 rejects an
admin surface: **a surface you must visit is not an alarm, and a message that rides a register
cannot report that register.**

---

## 5. Testing

- **`sendable` matrix**: 3 states × 4 registers, asserting the returned option AND that the dark
  arms are distinguishable (`Disabled` vs `Unresolved` differ in level and `reason`).
- **The structural claim is a COMPILE-FAIL test, not a runtime one.** A `trybuild`-style case
  asserting that reaching a `&str` from `Notify::Webhook` without `sendable()` does not compile. If
  that dependency is refused (4GB box, zero-new-deps doctrine — the `logged` doc records that exact
  trade), the fallback is an explicit `#[deny]`-backed note plus a test asserting `WebhookUrl` has
  no public accessor via a doc-test that must fail to compile. **Decide at plan time; do not let
  the claim ship unasserted** — an unenforced structural guarantee is this spec's own subject.
- **Bell decoupling, BOTH directions** (the defect was a one-way implementation of a symmetric
  rule): `WHISPER_DISABLED=1` ⇒ bell still rings; `BELL_DISABLED=1` ⇒ whisper still whispers.
  The second direction already passes today — it is included so the pair is asserted together and
  the asymmetry cannot return.
- **`ping_msg` dark-path record**: assert `Disabled` and `Unresolved` each emit, since "emits
  nothing" is the entire bug and a test that only checks "does not panic" would have passed all along.

---

## 6. Risk

- **Blast radius is the notification path only.** `ping_msg` returns `()` by design and no call site
  can propagate; that guarantee is preserved unchanged.
- **The newtype touches every `Notify` construction site.** Compile-enforced; the census is
  `Notify::Webhook` = **13 hits** workspace-wide (`--include=*.rs`, excluding `target/`).
  🔴 **This number was 6 in this spec's first draft and that was a WINDOW, not a census** — the
  first grep was scoped to `crates/fulfillment/src/*.rs`, one crate's `src/`, and published as
  the population. The missing 7 are mostly test-side constructions. *Corrected at spec self-review;
  recorded rather than silently fixed, because understating one's own blast radius by half is the
  failure this spec is about.*
- **A newly-loud `Unresolved` could page repeatedly.** By construction it can only page through the
  log (§4.5), never through `ping_msg`, so there is no amplification loop — the alarm debounces on
  the CloudWatch side.
