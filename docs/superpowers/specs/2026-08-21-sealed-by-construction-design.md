# sealed by construction — the compiler is the only reviewer that never forgets

**date:** 2026-08-21 · **author:** code kitten · **status:** DRAFT, pre-family-review

## ⚠️ HONESTY FIRST: this is a LATENT trap, not a live incident

**Say the weak half before the strong half**, because the last spec in this folder died of an
unexamined premise and its retraction is three files away.

**#151 describes a leak — a revealed Humble gift key reaching the operator Discord channel through
an interpolated SDK error Debug. Measured 2026-08-21, that leak is NOT REACHABLE TODAY**, for two
independent reasons:

1. **`SdkError` carries the RESPONSE, never the REQUEST.** `ServiceError<E,R>`/`ResponseError<R>`
   hold `raw: R` = `HttpResponse` (`aws-smithy-runtime-api-1.15.0/src/client/result.rs:241,267`).
   The item *we sent* — the one carrying `revealed_key` — is not in any variant.
2. **The only way a DynamoDB error response carries an item back is
   `ReturnValuesOnConditionCheckFailure::AllOld`, and the sole site that requests it is
   `set_link_thanks`** (`crates/dynamo/src/lib.rs:849`), whose item is `LINK#/META` —
   `thank_note`/`thanked_at`, **never a key**.

⇒ **Nothing is leaking. This spec must not be sold as an incident, and its PR must not imply one.**

## 🔴 so why build it: the trap is one line away at ~29 sites, and we DOCUMENT that line as correct

```
conditional writes in crates/dynamo/src/lib.rs : 31
...of which request AllOld today               :  2
claim write that sets revealed_key             : crates/dynamo/src/lib.rs:1355
```

`set_link_thanks`'s own docstring **recommends** the AllOld pattern, correctly and at length:

> *"A CCF is classified ATOMICALLY from the failed write's own
> `ReturnValuesOnConditionCheckFailure::AllOld` item — the exact item the condition was evaluated
> against. No follow-up read: a plain re-read is eventually consistent…"*

**That reasoning is right, and it generalises to the claim writes.** The next author who wants an
atomic CCF classification on a claim write will find a docstring telling them how — and the moment
they add that line to the write at `:1355`, the revealed key is inside `ccf.item`, inside the modeled
error `E`, inside `format!("{e:?}")`, inside `StoreError::Aws(String)`, and inside the Discord ping.

⇒ ***The hazard is not that someone will do the wrong thing. It is that someone will do the
RECOMMENDED thing.*** That is the version worth engineering against.

## the unifying thesis — three open issues are one defect

| issue | what it says | the shared shape |
|---|---|---|
| **#151** | operator ping can interpolate a store-error Debug | the boundary accepts anything |
| **#187** | `fn net()` is a sealer nothing forces — 13 unforced verb sites | the sealer is optional |
| **#188** | make `net()` TAKE the correlator (OMBB's idea) | "stripping the URL is free" is re-asserted by hand at every site, forever |

**All three are the same sentence: *the property holds only while every author remembers.*** #187
says it outright. That is the single lesson this body has re-learned more than any other, and here it
is sitting in our own security-relevant plumbing, filed and unfixed.

**Census re-measured today, unchanged from the issue's 2026-08-10 pin** (so the issues have not
rotted): `.send()` 10 · `.bytes()` 1 · `.text()` 1 · `.json::<` 1 = **13**.

## design

Three layers, ordered by my own rung ledger — *an artifact that REFUSES ≫ a default that makes the
safe path EASY ≫ a note you must remember to read.* **L1 is the whole security argument; L2/L3 are
hardening.**

### L1 — the SDK Debug never enters a string we own  *(closes #151's class, not just its instance)*

Today:

```rust
impl<E: Debug, R: Debug> From<SdkError<E, R>> for StoreError {
    fn from(e: SdkError<E, R>) -> Self { StoreError::Aws(format!("{e:?}")) }   // ← everything
}
```

`format!("{e:?}")` is an **unbounded** capture: whatever the SDK ever decides to put in Debug, we
adopt, forever, into a value that flows to Discord. **Replace the blanket capture with a deliberate
extraction** — operation, error code, request id, and the modeled message — none of which can carry
item attributes:

```rust
StoreError::Aws(AwsFault { op: &'static str, code: Option<String>, request_id: Option<String> })
```

**This is the fix that makes the AllOld question stop mattering.** Whether a future write requests
AllOld, whether the SDK changes its Debug, whether the ping redacts — none of it can leak, because
the item never enters our error type.

⚠️ **The cost is real and must be stated, not glossed:** we lose the SDK's full Debug at the error
site. That is a genuine diagnostic loss. *Mitigation:* `code` + `request_id` are what actually
resolve an AWS issue, and the full Debug can still be emitted to **CloudWatch** (a private,
access-controlled surface) while never entering the value that reaches **Discord**. **The split
between "diagnostic surface" and "human channel" is the design, and it is the part reviewers should
attack hardest.**

### L2 — `net(e, Correlator)` *(#188, OMBB's design)*

Make the sealing constructor demand what the stripped URL was carrying, so *"`without_url()` is
free"* becomes a claim the compiler checks rather than one a docstring asserts.

### L3 — make the unsealed verb unreachable *(#187)*

Route steam-client's 13 raw verb sites through a wrapper that exposes only the sealed conversion, so
site 14 cannot mint a raw `reqwest::Error` **and** cannot forget the correlator. **Census 13 → 0,
enforced by a committed script in CI, not by this document.**

## success criteria — falsifiable, and RED before green

The retracted surfacing spec was killed by criterion ⑥ (a fire-rate floor) precisely because a
correct-and-silent mechanism scored as success. Same discipline here:

1. **A test that FAILS on today's code.** Construct an `SdkError` whose response body carries a
   sentinel "key", assert the resulting `StoreError` string does not contain it. **It must be
   demonstrated RED against `main` before the fix lands** — a redaction test that never failed is a
   comment.
2. **Census 13 → 0**, by committed script, exercised RED against `main`'s tree.
3. **A new call site that forgets the correlator does not compile.** Demonstrated, not asserted.
4. **No behaviour change on the happy path** — the store's error *classification* (`is_ccf_put`,
   `is_ccf_update`, `TxConflict`) must be untouched. **These predicates decide idempotency and
   retry; breaking them is far worse than the leak this spec prevents.** ⇐ *the real risk of this
   change, named up front.*
5. **`ClaimTxError`/`SetThanksOutcome` semantics unchanged**, proven by the existing suite.

## non-goals

- **Not** an incident response; see the honesty section. No CVE language, no urgency framing.
- **Not** a rewrite of `operator_message.rs`'s chunking/parts — that machinery is sound.
- **Not** touching the `AllOld` pattern itself. **It is correct.** We are removing the consequence
  of adopting it, not discouraging it.
- **Not** humble-client re-sealing (#173 did that; its `Debug`-redaction test is the precedent this
  generalises).

## open questions for OMBB + Lilith  *(step 2)*

1. **L1's diagnostic split.** Is "full Debug to CloudWatch, classified fault to Discord" the right
   line — or should the SDK Debug be dropped entirely, on the grounds that a private-but-persistent
   log is still a place a key can come to rest? *I lean to the split; I am not confident.*
2. **Scope.** L1 alone is the security argument. Is L1+L2+L3 one PR, or does L3's mechanical churn
   deserve its own diff (the #187 argument for why it was split out originally)?
3. **`AwsFault` shape.** Is `op`/`code`/`request_id` enough to debug a real production DynamoDB
   failure at 3am, or does dropping the Debug cost more than the trap is worth? **This is the
   question I most want a second opinion on, because I am the one who wants the tidy invariant.**
4. **Is the trap real enough to spend a pounce on?** Nothing is leaking. The honest counter-argument
   is *"you are hardening a path with zero incidents while the app has had zero claims in a month."*
   **I want that argued, not assumed away.**
