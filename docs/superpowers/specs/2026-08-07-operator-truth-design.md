# Operator truth: a notification you cannot verify is a rumour

**Date:** 2026-08-07
**Author:** code kitten
**Design review:** OldManBendoBot + Lilith, family channel, 2026-08-07T11:04–11:07-04:00
**Closes:** #163, #161, #151 — plus one defect not previously filed (A below)
**Out of scope, deliberately:** #162 (parked-claim nag cadence) — reconcile cadence state, a
different subsystem. Bundling it would sprawl the plan.

## The problem, measured

Every operator notification funnels through one function, `crates/fulfillment/src/lib.rs:4403`,
with ~20 call sites. Eleven lines, four independent ways to tell Ben nothing while looking like it
told him something:

```rust
async fn ping(deps: &Deps, msg: &str) {
    let Some(url) = deps.webhook_url.as_deref() else {   // (A) unconfigured → silent success
        return;
    };
    let body = serde_json::json!({ "content": ping_content(msg) });  // (B) no length bound
    if let Err(e) = deps.http.post(url).json(&body).send().await {   // (C) transport errors only
        eprintln!("discord ping failed (non-fatal): {e}");           // (D) prose, into a stream
    }                                                                //     nobody tails
}
```

**(A) An unconfigured webhook returns success.** A prod deploy that lost its webhook URL is
indistinguishable — from every caller and every log — from one that notified correctly. Same defect
as `oldmanbendobot/claude-code-infra#44`, which I reviewed on 2026-08-06 and did not ask whether I
had. Not previously filed here.

**(B) No length bound.** Discord rejects `content` over 2000 characters (#163).

**Evidence, corrected — and the correction changes what to build.** I first wrote that this was
"measured twice on this box." **Both citations were wrong, and OMBB refuted them from his own
disk:**

- `ERROR: reply failed: undefined is not an object (evaluating 'text.length')` is a **JavaScript
  null-deref**, not a length-limit error. The word "length" in it is a substring, not a semantic.
  His verification grep (`too long|2000|length|400`) matched `length` *inside* `text.length` and
  reported 7; the real count of Discord length failures on that disk, ever, is **0**.
- My family-channel message describing this defect was **not truncated — it was chunked and fully
  delivered.** The section I thought was lost arrived intact in the second chunk. "Split across
  messages" and "truncated" are different events and only one loses data.

**So: the oversize guard is correct on principle and has ZERO observed instances.** Recorded plainly
rather than dressed up, because a spec that inflates its evidence teaches the next reader to trust
inflated evidence.

**The end that DOES have evidence is the opposite one: empty content.** Two observed failures on
that disk, both at the under-length end (`Cannot send an empty message`, and the null-deref). An
empty `content` is a Discord 400.

**This seam is immune to that today, by construction:** `ping_content` prepends
`🐱 bendobundles: `, so the body is never empty regardless of `msg`. **But chunking would introduce
the hole** — the observed `reply failed after 1 of 2 chunk(s) sent: Cannot send an empty message`
*is* an empty trailing chunk. **The fix proposed here creates the failure mode the evidence actually
supports**, which is exactly why it needs a guard and a test rather than confidence.

**The one true finding from the "truncation" that wasn't.** The chunk boundary fell **mid-`**`
markdown token** — chunk one ended `…a deliberate and correct*`, chunk two began `* choice:…`.
Joined, it reconstructs perfectly; zero characters lost. But it *rendered* as a mangled mid-word
cut, and three readers — including its author — spent real time treating it as data loss. **A
boundary that merely looks like data loss costs the same investigation as actual data loss.** That
is the live argument for this design's rule that a bound must announce itself: split on line
boundaries, never mid-token, and label the parts.

**And the receipt was in my hand the whole time.** The send returned **`sent 2 parts`** — a *chunk*
report, not a truncation report — and I read it, kept it, and later published the opposite. Last
night's lesson was *a 2xx proves delivery, never fidelity*; this is its inversion: **I had a receipt
that reported exactly what happened and misread what it was reporting.** The instrument was honest
and the reader supplied the failure.

**(C) A non-2xx response is `Ok(_)`.** `reqwest`'s `Err` arm catches *transport* failure only. A
`400` (too long), `401`/`404` (rotated or deleted webhook), or `429` (rate limited) arrives as
`Ok(response)` and is never inspected — a failure routed into the success path, the same shape as
`channels_ready` reporting a dead gateway as healthy. **Discord rate-limits, so notifications
disappear precisely under the load that makes you need them.**

**(D) The only failure report is `eprintln!`** — the sole occurrence in 4,827 lines.

Separately: `shelf_truth_audit`'s per-row write failure is `Err(e) => tracing::warn!` with no ping
and no counter, asymmetric with the order walk's `orders_failed` (#161). And ~6 sites do
`format!("... failed: {e} ...")`, interpolating a store error into an operator message; any variant
whose `Display` carries a revealed key publishes it (#151).

**Unifying statement:** *this app's only window into itself is an instrument that reports a narrower
thing than its reader believes.*

## What this is not

Not new notification content, not a second channel, not retries, not a dashboard, not a dead-letter
queue. **The delivery path learns to tell the truth about itself. Nothing else changes.**

## Design

### `ping` stays infallible. That is the point, not a compromise.

The existing doc comment — *"never fails the caller; a dead webhook must not break fulfillment"* —
is correct and load-bearing, and the `-> ()` signature makes it a **structural guarantee**. Making
`ping` return `Result` would trade that guarantee for a convention twenty call sites must
independently honour, and call site twenty-one, written in six months, writes `ping(...).await?` and
takes down fulfilment.

**The doc comment licenses not *propagating* a failure. It never licensed not *recording* one.**
Those were collapsed into a single decision; this design separates them. **Make not-reporting
impossible, rather than not-failing impossible.**

### (A) Resolve configuration once, loudly, at load — not per call forever

`else { return; }` at use time can never distinguish *deliberately off* from *someone dropped the
env var*. Move the decision to config construction:

- `NOTIFY_DISABLED=1` → notifications explicitly off, **logged once at startup**.
- Webhook URL present → normal operation.
- Neither → **construction fails.**

Twenty silent no-ops a day become one loud event at boot, at the point where someone can still act
on it.

### (B) Bounded by construction, and never silently shortened

`OperatorMessage::chunks()` yields chunks each ≤2000 **including** the `🐱 bendobundles: ` prefix
and the label, splitting on line boundaries where possible and hard-splitting where not. Multi-chunk
messages carry `(1/3)`, `(2/3)`, `(3/3)`.

**A silently-shortened operator message is a lie that reads as complete.** Chunking preserves the
content; the label makes multiplicity visible. If any path ever truncates instead, it must append an
explicit marker.

**Three guards the evidence demands, all of which this fix would otherwise introduce:**

1. **No empty chunk, ever.** An empty `content` is a Discord 400, and it is the *only* end of the
   length axis with observed failures on this box. The seam is immune today via the prefix;
   chunking would break that immunity. `chunks()` never emits an empty or prefix-only chunk.
2. **Never split mid-token.** Split on line boundaries; fall back to whitespace; hard-split only as
   a last resort, and never inside a markdown delimiter pair. A boundary that looks like damage
   costs what damage costs.
3. **A chunked send is NOT atomic.** `1 of 2 chunks sent` is a real observed outcome: partial
   success exists. **This design therefore adds no retry** — a naive retry re-posts chunk one and
   duplicates operator messages. Failure of any chunk is recorded per-chunk (`chunk` field in the
   structured log) and the run continues. **Retry semantics are deliberately out of scope; anyone
   adding them must handle partial success first.**

### (C) Verify delivery

`.error_for_status()` between `.send().await` and the existing `if let Err` arm. A 400/429 becomes
`Err` and flows into the branch **that already exists**. One line, no new state, no dependency on
any other decision here — **land it first and independently.**

### (D) Report out-of-band, terminate at a counter

**No dead-letter row.** The discriminating question is *do we need to REPLAY the notification, or
only to KNOW it failed?* **Nobody ever re-sends a failed operator ping** — the content is
time-sensitive and the next sync's summary supersedes it. A durable queue with no drainer is
storage with no consumer: `claude-code-infra#52`'s orphaned-state shape in better clothes. *A
dead-letter queue nobody drains is just a slower log.* **Recorded here so nobody rebuilds it.**

Instead: `eprintln!` becomes a **structured** `tracing::error!` with fields
(`outcome`, `status`, `chunk`, `req_id`), and a CloudWatch **metric filter + alarm** on it. No new
state, no new IAM, no delay — **and it fires even if the next sync never runs**, which is the hole
an in-band mechanism cannot close: the thing that is broken may *be* the invoker.

**The recursion terminates at a counter, never at another network call.** The reporting path can
itself fail; we increment and log, we never try to narrate the failure of our narration. A per-run
`notifications_failed` count rides the summary when the summary can be sent — free, and explicitly
**not** relied upon, because a failing channel cannot report its own failure.

### (3) Secrets: structural containment, not a filter

`OperatorMessage` is constructible only from string literals and an explicit `ErrorSummary`.
`ErrorSummary::of(&e)` renders a **variant name plus a fixed operator-facing description**, never
the error's `Display`/`Debug` payload. The ~6 `{e}` sites become `ErrorSummary::of(&e)`.

**The constructor must be the ONLY door — this is what makes it structural rather than
conventional.** `OperatorMessage` has a private inner, **no `From<String>`, no public string
constructor**, and `ping` no longer accepts `&str`. Otherwise a safe path and an unguarded path sit
side by side, indistinguishable to review, and call site seven writes `format!("{e}")` while the
compiler says nothing. **A type that *offers* safety is not a type that *enforces* it, and only the
second has jurisdiction over call sites nobody has written yet.**

**Variant names become published content.** The allowlisted output is the variant name, so a future
variant named after its payload leaks by naming. One line in the enum's doc comment says so —
costs nothing today, unfindable in six months.

**Name the actual decision: which side of a trust boundary the error lands on.** CloudWatch is
access-controlled; a Discord channel is not. **The raw error is fine — it belongs on the other side
of that line.** Stated explicitly so nobody later "helpfully" restores the detail to the operator
message.

**Why this is an allowlist when my standing rule says "denylist guards, never allowlists."** The
rule governs **enumeration** — when *searching*, an allowlist silently returns a subset and the
unknown-unknown becomes invisible. This is **construction** — when *permitting*, an allowlist makes
the unknown-unknown unrepresentable. Same structure, opposite consequence, because the failure
direction reverses. **The test: does the artifact SEARCH, or does it PERMIT?** A grep, a sweep, a
health check → denylist. A type, a constructor, a schema → allowlist. And it is outranked by a rule
already held: *make the bad state unrepresentable, not merely detected.* **A filter protects the
code you audited; a type protects the code you haven't** — a denylist has no jurisdiction over
future call sites.

**Debuggability is preserved by a joinable correlation id.** One `req_id` generated per failure,
emitted on **both** sides: the operator sees `store write failed — req abc123`; the structured log
line carries the full error **and the same id as a structured field**. An id that appears on only
one side, or is buried in prose on the log side, is worse than the leak — at 3am it *looks* like you
can chase it. **A test asserts both sides carry the same value**, because nothing announces drift.

**The conversion boundary is now the entire security surface.** Whatever renders `OperatorMessage`
is where a lazy `{e:?}` reopens everything, and it will not look dangerous. A comment saying exactly
that lives at that impl.

### (5) The audit counter (#161)

`shelf_truth_audit` gains `rows_failed`, incremented on the per-row `Err` arm and appended to the
sync summary when `> 0`, mirroring `orders_failed` and `audit-pulled`.

## Data flow

```
config load ─┬─ webhook set ──────────────> normal
             ├─ NOTIFY_DISABLED=1 ────────> off, logged ONCE at boot
             └─ neither ──────────────────> construction FAILS

ping(msg) ─> OperatorMessage ─> chunks(≤2000, labelled) ─> deliver()
                                                             ├─ 2xx ─> done
                                                             └─ else ─> tracing::error!{outcome,
                                                                        status, chunk, req_id}
                                                                        └─> metric filter ─> alarm
                                                                        └─> notifications_failed++

shelf_truth_audit ─> rows_failed++ ─> summary line when >0
```

## Error handling

Every new path fails toward visibility. The structured log emit is infallible (it is a log). The
counter is in-memory. **Nothing in the reporting path performs I/O that could itself need
reporting.**

## Testing

Test-first, each red before the code, per this repo's standing practice.

1. **Config: missing webhook and no disable flag → construction fails.**
2. **Config: `NOTIFY_DISABLED=1` → constructs, logs once, sends nothing.**
3. **Non-2xx is a failure** — wiremock 400 and 429 → error branch taken, `notifications_failed`
   incremented. *(This is the dirty-side test for (C): remove `.error_for_status()` and it must
   fail.)*
4. **Transport error is a failure** — connection refused → error branch.
5. **Chunking is bounded** — a 5,000-char message yields chunks each ≤2000 *including prefix and
   label*, and concatenating their bodies reproduces the input exactly.
6. **Chunk labelling** — a 3-chunk message carries `(1/3)`, `(2/3)`, `(3/3)`.
7. **Secret containment (planted violation)** — an error variant whose `Display` contains a
   key-shaped string is rendered through `ErrorSummary::of`; the output **must not** contain it.
   Must fail if containment is removed.
8. **Correlation id is joinable** — the id in the operator message equals the id in the structured
   log record. Must fail if either side drops or renames it.
9. **`rows_failed` surfaces** — an audit row write failure appears in the summary; zero adds no line.
10. **`ping` is still infallible** — a totally dead webhook leaves `handle`'s outcome unchanged.
    *(Pins the existing guarantee against this work.)*
11. **No empty chunk** — inputs of `""`, whitespace-only, and a length that lands a chunk boundary
    exactly at the end all yield zero empty/prefix-only chunks. *(The evidenced failure end.)*
12. **No mid-token split** — a message containing `**bold**` at the boundary offset splits on the
    line boundary instead, and no emitted chunk ends inside a markdown delimiter pair.
13. **The only-door property** — a compile-fail test (`trybuild` or a doc-test marked
    `compile_fail`) asserting `OperatorMessage::from("raw".to_string())` and
    `ping(&deps, format!("{e}"))` **do not compile.** *(Without this, "structural" is a claim about
    today's code rather than a property of the type.)*

## Build discipline (this box)

4GB. `cargo check` and per-crate `cargo test -p fulfillment`. **Never `--workspace`, never
`--all-targets`, always `-j 1`.** Two OOM deaths on 2026-08-03 came from violating exactly this.

## Success criteria

- A misconfigured webhook fails at boot instead of being invisible forever.
- A Discord 400/429 is visible, and alarmable, without the next sync running.
- No operator message can exceed 2000 characters or be silently shortened.
- No store error payload can reach Discord, at any current or future call site.
- Every operator error is joinable to its full detail in CloudWatch by `req_id`.
- A chronically failing audit row is visible from Discord alone.
- `handle`'s behaviour under a dead webhook is unchanged.
