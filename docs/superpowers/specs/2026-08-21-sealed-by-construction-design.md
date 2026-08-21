# sealed by construction — the compiler is the only reviewer that never forgets

**date:** 2026-08-21 · **author:** code kitten · **status:** DRAFT, pre-family-review

## ⚠️ HONESTY FIRST: this is a LATENT trap, not a live incident

**Say the weak half before the strong half**, because the last spec in this folder died of an
unexamined premise and its retraction is three files away.

**#151 describes a leak — a revealed Humble gift key reaching the operator Discord channel through
an interpolated SDK error Debug. Measured 2026-08-21, that leak is NOT REACHABLE TODAY**, for two
independent reasons:

1. **The raw HTTP payload an `SdkError` carries is the RESPONSE, never the REQUEST.** In
   `aws-smithy-runtime-api` **1.14.0** — the version `Cargo.lock` resolves — `ServiceError<E,R>` and
   `ResponseError<R>` each hold a field `raw: R` = `HttpResponse`. **The item *we sent*, the one
   carrying `revealed_key`, is in no variant of `SdkError` at all.**
   ⚠️ **`ResponseError` is named here for its `raw` field ONLY. It is NOT a "safe" arm** — it also
   holds a `source: BoxError`, and it belongs in the unbounded list below. *The first draft used it
   as proof of the safe side; see the correction.*
   🔴 **CORRECTED (OMBB, 2026-08-21): I ENUMERATED 2 OF 5 VARIANTS AND WROTE THE CONCLUSION AS IF I
   HAD DONE ALL FIVE.** `SdkError` is `#[non_exhaustive]` with **five**, and **FOUR** carry an unbounded
   payload: `ConstructionFailure{source: BoxError}` · `TimeoutError{source: BoxError}` ·
   `DispatchFailure{source: ConnectorError}` · **`ResponseError{source: BoxError, raw: R}`** — the
   variant this spec originally cited *for the safe side*, which carries a `BoxError` next to the
   response I was pointing at. **`ServiceError{source: E, raw: R}` is the ONLY bounded arm — and it
   is the one the entire AllOld analysis is about.** *Three of us spent a morning on the single
   closed variant.* (2→3 Lilith, 3→4 OMBB. **I had `ResponseError`'s struct on screen in my own
   first read and reasoned only about `raw: R`.**)
   *`ConstructionFailure` is literally the variant for "the request failed while being built", and
   `DispatchFailure` — the one OMBB and I both left off — is the most-travelled of the three, since
   it fires on every connection failure.* (Third arm: Lilith.)
   ⇒ **This is a NARROWED IMPOSSIBILITY, not an established one. Neither OMBB nor I has shown a leak
   through those three — the point is that nothing rules it out, and `#[non_exhaustive]` means AWS
   can add a sixth whenever it likes and `format!("{e:?}")` will adopt it silently.**
   🔑 **And this is the STRONGER answer to Q4 than the one I had:** the AllOld argument needs a
   future author to do something; **this argument needs nobody.** The capture is unbounded *today*.
2. **The only way a DynamoDB error response carries an item back is
   `ReturnValuesOnConditionCheckFailure::AllOld`, and the sole site that requests it is
   `set_link_thanks`** (`crates/dynamo/src/lib.rs:849`), whose item is `LINK#/META` —
   `thank_note`/`thanked_at`, **never a key**.

⇒ **Nothing is leaking. This spec must not be sold as an incident, and its PR must not imply one.**

> 🔴 **CITATION HYGIENE — I cited a crate version my build does not use.** The first draft anchored
> to a **newer minor of `aws-smithy-runtime-api` than the lockfile resolves** (the retired version
> string is deliberately DESCRIBED, not quoted — `scripts/check-spec-crate-anchors.sh` greps this
> file, and a quote of a bad anchor is indistinguishable from the bad anchor). **`Cargo.lock`
> resolves `1.14.0`.** That newer tree was on disk as residue from *my own abandoned
> `cargo update --precise` experiment earlier the same morning* —
> I reverted it out of `Cargo.lock` and not out of `~/.cargo/registry`, then read it back as if it
> were the build. **`ls ~/.cargo/registry` is not a measurement of what you compile; the lockfile
> is.** Re-verified against 1.14.0: shapes identical, so the conclusion held and only the anchor was
> wrong. *(Lilith's catch.)*
> 🔪 **AND THE CITATION WAS SELF-DISPROVING.** I cited `result.rs:241,267` as proof of the *safe*
> side. **`:241` IS `ResponseError` — and `:243`, two lines below my own citation, is its
> `source: BoxError`.** *I cited the struct and stopped one field short of its disproof.* (Lilith.)
> ⇒ **Not a research failure — a place the eye stops.** Same shape three times this morning:
> **I enumerated 2 arms, Lilith corrected to 3, OMBB corrected to 4 — every correction stopping at
> the first number sufficient to make the point.** ***An enumeration that stops when it is SUFFICIENT
> is not an enumeration.***
> ⚖️ **Scoreboard for this review, worth keeping: `AllOld: 2` (counted a docstring) · smithy
> **wrong crate version** (see above; not quoted, for the same reason) · `2 unbounded arms`
> (short by one). EVERY CONCLUSION SURVIVED; NOT ONE
> CITATION DID.** Three of us reading the source is the only reason we know that.

## 🔴 so why build it: the trap is one line away at ~29 sites, and we DOCUMENT that line as correct

```
conditional writes            : 28   ← grep -c '\.condition_expression('        crates/dynamo/src/lib.rs
Query key conditions (READS)  :  3   ← grep -c '\.key_condition_expression('
                          partition : 28 + 3 = 31 naive 'condition_expression' matches
AllOld call site              :  1   ← the sole `.return_values_on_condition_check_failure(` call
                                        (in `set_link_thanks`)
claim write setting a key     :      ← the `claim.revealed_key = Some(` assignment
raw `format!("{e:?}")` captures:  23  ← AS MEASURED ON THE PRE-FIX BRANCH. On main after this
                                        change the count is 0 — that is the point of the change.
```

> 🔴 **EVERY LINE NUMBER THIS TABLE ONCE CARRIED WAS STALE WITHIN HOURS — MOVED BY THE VERY PR THIS
> SPEC DESCRIBES.** `:849 → :888`, `:1355 → :1408`, and the `23` became `0`. **The counts and the
> function names survived; only the line numbers rotted.** (OMBB, who then ran the same check on his
> own `CLAUDE.md` and found a citation with a line and *no path* — *"worse than one with neither: it
> looks precise and sends you to the wrong file."* Lilith found all four of her checkpoint's
> citations dead, `+39` to `+94`, same PR.)
> ⇒ ***CITE BY NAME, BY A COMMAND THAT FINDS IT, OR BY A SHA — never by a line number in a file that
> moves.*** `CLAUDE.md` has carried that rule for days; all three of us wrote integers anyway.
> 📌 And say which measurements are **historical**: the `23` above is true of the pre-fix branch and
> false of `main`. *A count in a document is a claim with a timestamp.*

> 🔴 **CORRECTED after Lilith's review (2026-08-21).** This table first said **2** AllOld sites. **It
> is 1.** My grep was `ReturnValuesOnConditionCheckFailure::AllOld`, which matched `:850` (the real
> call) **and `:815` — a DOCSTRING.** *I counted my own documentation as a call site — and it is the
> very docstring this spec then quotes as recommending the pattern. I cited it and tallied it.*
> ⚠️ **Lilith reached the right number by a different route** and read the `2` as me conflating
> `:2466`'s `return_values(ReturnValue::AllOld)` — a genuinely different API parameter (it returns
> the item on **success**, in `out.attributes`, and never touches `SdkError`). **That is a real
> distinction and worth knowing, but it was not my error.** *Right conclusion, wrong mechanism — so
> the lesson is "a grep over a source file counts prose as code", not "two nouns, one string match".*
> ⇒ **A census by `grep` cannot distinguish a call from a comment about a call.** Same family as the
> guard that tripped on the prose quoting the value it banned, six hours earlier, in this same repo.

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

### 🔬 DEMONSTRATED, not argued — the trap fires

Built the recommended line's payload in a throwaway unit test (`SdkError::service_error` +
`ConditionalCheckFailedException::builder().item(...)`, both public) and converted it through the
real `From` impl. **Output, verbatim:**

```
dynamodb error: ServiceError(ServiceError { source: ConditionalCheckFailedException(
  ConditionalCheckFailedException { message: None,
    item: Some({"revealed_key": S("HB-GIFT-KEY-SENTINEL-0451")}), ... }), ... })
```

**The key is in `StoreError`, and `ping_content` redacts nothing.** The test was reverted, not
committed — it becomes criterion ①'s test during execution. *This also proves criterion ① is
buildable as a pure unit test with no local DynamoDB, which was the open feasibility question.*

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
adopt, forever, into a value that flows to Discord. **There are 23 such captures** (verified line by
line). Replace the blanket capture with a deliberate extraction:

```rust
/// A DynamoDB failure, reduced to what diagnoses it and nothing that can carry item data.
///
/// RULE, and it is the whole point of the type: the modeled `.message()` is IN;
/// `.item()` is NEVER. Enforced by a test, not by this sentence.
pub struct AwsFault {
    op: &'static str,            // which call
    code: Option<String>,        // what AWS said
    message: Option<String>,     // modeled message — NOT the item
    request_id: Option<String>,  // support handle
    http_status: Option<u16>,    // 4xx vs 5xx, instantly
    retryable: bool,             // the SDK's own classification
}
```

🔑 **`http_status` + `retryable` are Lilith's additions and they are the two that shorten a 3am
night** — my original `op`/`code`/`request_id` was not enough to debug a real failure, which was
exactly the question I flagged as the one I least trusted myself on. *I was the one who wanted the
tidy invariant, and I undercounted its cost; that is the predictable direction of my own bias.*
📌 **My prose and my struct disagreed** in the first draft — the prose promised "and the modeled
message", the struct had no such field. Fixed above.

**This is the fix that makes the AllOld question stop mattering.** Whether a future write requests
AllOld, whether the SDK changes its Debug, whether the ping redacts — none of it can leak, because
the item never enters our error type.

🔴 **THE WAY THIS DESIGN FAILS, AND IT IS THE FAILURE IT IS SUPPOSED TO PREVENT.** One of the 23
sites (`:3017`, feeding `:3021`) is a legitimate **non-SDK** string —
`"describe_table returned no table"`. That creates real pressure to keep a `String`-carrying arm
beside `Aws(AwsFault)`. ***The moment `Aws(String)` survives, L1 IS #187: a sealer nothing forces.***
⇒ **The non-SDK case gets its OWN variant, and there is NO `From<String>` for `Aws`** — written in
as criterion ⑥ so nobody adds the bridge at 3am to make a build green. *(Lilith's catch. Without it
I would have shipped the trap inside its own cure — the "prettier promise" I asked her to check for,
and she found it.)*

⚠️ **The cost is real and must be stated, not glossed:** we lose the SDK's full Debug at the error
site. *Mitigation:* the full Debug still goes to **CloudWatch**, never into the value that reaches
**Discord**.
🔑 **And the principled line is NOT private-vs-public, which is where I first drew it.** It is that
**the keys already live in DynamoDB in that same AWS account, so CloudWatch introduces no new
principal — it is inside a boundary the data is already in. Discord is a third party with different
retention and different access control.** That reframing is Lilith's and it survives my own Q1
objection, which the private-vs-public framing did not.
⚠️ **It holds only if that log group's retention is actually set and nothing ships it onward.**
🔬 **MEASURED, and the repo and the world DISAGREE — this is the good kind of finding:**
OMBB read the **repo** (`terraform/*.tf` has no `retention_in_days`, no `aws_cloudwatch_log_group`)
and correctly concluded *"default is Never Expire."* I read the **account**:

```
/aws/lambda/brd-prod-ue1-bendobundles-admin-api      30
/aws/lambda/brd-prod-ue1-bendobundles-fulfillment    30
/aws/lambda/brd-prod-ue1-bendobundles-public-api     30
```

🔴 **AND THIS WHOLE PARAGRAPH WAS WRONG — CORRECTED 2026-08-21 BY READING THE MODULE.**
The claim was *"managed by nothing, correct by accident."* **It is managed.**
`terraform/.terraform/modules/lambda_admin_api/main.tf:75` — the `bendoerr-terraform-modules/lambda/aws`
module **declares `aws_cloudwatch_log_group "this"` itself**, with
`retention_in_days = local.cloudwatch_retention_in_days`, whose variable defaults to **30**
(`variables.tf:209`, `optional(number, 30)`). ⇒ **the live 30 is the module's managed default, not
console drift.**
⚖️ **How BOTH of us missed it, and it is the same miss:** OMBB grepped `terraform/*.tf` and found no
`retention_in_days`; I read the *account*. **Neither of us read the MODULE** — the thing that actually
owns the resource. *A repo-vs-world comparison has a third surface when the resource is module-owned,
and we each checked one of the two we could see.*
⇒ **The "correct by accident" finding is RETRACTED. Terraform Task 4 shrinks accordingly** — from
"declare four log groups" (which would have **collided with the module's own resource**) to, at most,
**pinning `cloudwatch_logs.retention_in_days` explicitly at the module call sites** so a module
version bump cannot move it silently. **That is a much weaker argument and is labelled as one.**
🔑 **And the comparison that makes the CloudWatch line hold for a MEASURED reason rather than an
assumed one (Lilith, the other half of OMBB's measurement):** `claim_item` (`schema.rs`) has **no
`ttl` attribute** — the claim serialises whole into `body`, `revealed_key` inside it — while `ttl`
**is** set on OIDC state, its sibling, and the STEAMOWN cache. ⇒ **this codebase deliberately TTLs
three item classes and deliberately does not TTL the one holding the keys.** So a never-expiring log
group and a never-expiring claim record are **the same retention posture, same principal, same
lifetime** — the log adds no new exposure.
🔄 **REVERSED, in the argument's favour, once the live `30` landed next to it (Lilith, retracting her
own reason):** CloudWatch is **30 days**; the claim holding `revealed_key` is **forever**. ⇒ ***the
log is the SHORTER-lived surface, not the equal one.*** The split is *more* defensible than argued —
but the stated reason had been wrong, and **a right answer resting on a wrong reason is the thing
this whole review kept catching.**
⚠️ **Limit, recorded because it bounds the evidence:** Lilith **cannot verify the `30` from her
seat** (her deploy role is `omyac`-scoped, no read on this account's log groups). She is taking it on
my measurement — *which is itself an argument for pinning it in code, where any reviewer can see it.* *Neither of them had this alone: OMBB had the log side, I
had neither, Lilith had the store side, and the inference needs both.*
⚠️ **But a default is not a decision, and the coupling is coincidence rather than design.** If
fulfil-and-purge on claims is ever implemented, a Never-Expire log group **silently outlives the
record it mirrors.**
⇒ **IN SCOPE only in its reduced form** (explicit pin at the module call). L1's "CloudWatch is the
safe surface" argument **already rests on managed configuration** — it just rests on a module
*default* rather than an explicit value.

### L2 — `net(e, Correlator)` *(#188, OMBB's design)*

Make the sealing constructor demand what the stripped URL was carrying, so *"`without_url()` is
free"* becomes a claim the compiler checks rather than one a docstring asserts.

### L3 — seal the PAYLOAD TYPE, not the verb *(#187 — and #187's own framing was wrong)*

🔴 **REDESIGNED after OMBB, and my own comment is the evidence against me.** The first draft said
"route the 13 raw verb sites through a wrapper." **The verb census is the wrong unit:**

- **`SteamError::Network(String)` (`:147`) is a PUBLIC `String` variant.** Anyone can build
  `SteamError::Network(whatever)` without ever touching `net()`. Sealing the verbs does nothing to it.
- **`.build()` mints a `reqwest::Error` that no verb census reaches** — and the comment saying so is
  **mine**, at `crates/steam-client/src/lib.rs:418`: *"`.build()` is not a request verb — so no verb
  census reaches it and the compiler has nothing to say."*

⇒ ***I filed a census (#187), documented the hole in it myself, and then wrote a spec proposing to
close the census.*** The seam is the **type**, not the function:

```rust
Network(SealedNetworkError)   // sole constructor takes (reqwest::Error, Correlator)
```

which subsumes #187 **and** #188: a site cannot mint the payload without the sealer, and cannot call
the sealer without naming what the stripped URL was carrying. **The verb census becomes a
belt-and-braces check, not the mechanism.**

🔑 **THE UNIFICATION THE FIRST DRAFT MISSED — the fix is one shape in both crates.** `Aws(String)`
in L1 and `Network(String)` in L2 are **the same bug**: *a publicly-constructible `String` payload
makes the sealer optional no matter what any census says.* **Seal the payload type; the function is
downstream of it.** ⚖️ *OMBB and Lilith reached this from **different crates** — two independent
specimens. They share a rule-set, so their agreement is not itself independent evidence; the two
specimens are. Recorded that way on purpose.*

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
   retry; breaking them is far worse than the leak this spec prevents.**
   ✅ **VERIFIED SAFER THAN I FEARED:** `is_ccf_put` (`:345`) and `is_ccf_update` (`:358`) are
   `matches!(e.as_service_error(), Some(..ConditionalCheckFailedException(_)))` — **typed matching,
   no Debug-string parsing** — and they take `&SdkError`, i.e. they run **upstream of the conversion
   L1 changes**. They never see `StoreError::Aws` at all. *My scariest criterion is nearly free.*
   ⚠️ **One real constraint falls out of it:** `set_link_thanks` at `:857` does `ccf.item.clone()`
   for its atomic classification. **L1 must let that site READ `ccf.item()` while forbidding the item
   from being CAPTURED into the error type. Read, not adopt — that distinction IS the
   implementation.** (Lilith's, verified in source.)
5. **`ClaimTxError`/`SetThanksOutcome` semantics unchanged**, proven by the existing suite.
6. 🔴 **`StoreError::Aws` carries `AwsFault` ONLY — no `String` arm, no `From<String>` bridge.**
   Non-SDK messages get a separate variant. *Without this criterion the whole design degrades into
   the optional-sealer it exists to abolish.*

## non-goals

- **Not** an incident response; see the honesty section. No CVE language, no urgency framing.
- **Not** a rewrite of `operator_message.rs`'s chunking/parts — that machinery is sound.
- **Not** touching the `AllOld` pattern itself. **It is correct.** We are removing the consequence
  of adopting it, not discouraging it.
- **Not** humble-client re-sealing (#173 did that; its `Debug`-redaction test is the precedent this
  generalises).

## family review — answered 2026-08-21 (Lilith), OMBB still open on L2

**Verified her claims against the source before adopting them; two needed adjusting.**

1. ✅ **Q1 (diagnostic split) — ANSWERED, and my framing was wrong.** Not private-vs-public;
   **same-AWS-account vs third-party**. See L1. *Conditional on measuring the log group's retention.*
2. ✅ **Q2 (`AwsFault` shape) — ANSWERED: mine was too thin.** `http_status` + `retryable` added.
   *This was the question I flagged as least trustworthy in my own hands, and it was indeed the one
   I got wrong — in the predicted direction, toward the tidy invariant.*
3. ✅ **Q3 (scope) — ANSWERED, and it dissolves rather than resolves.** L1 is **23 call sites**; it
   *is* the churn PR, so "keep mechanical churn separate" cannot argue against bundling L2/L3 with
   it. **Split on the CRATE boundary instead:**
   **PR1 = L1 in `dynamo`** (type + 23 sites + RED-first test + the non-SDK variant) ·
   **PR2 = L2+L3 in `steam-client`.** Different crates, different reviewers, and **L2 is OMBB's
   design to defend.**
4. 🔴 **Q4 (is a zero-incident trap worth a pounce?) — MY COUNTER-ARGUMENT WAS VACUOUS, NOT FAIR.**
   I offered *"zero incidents while the app had zero claims."* **Zero claims means the path was never
   exercised: the clean record has a denominator of ZERO.** That is absence of sampling, not evidence
   of safety — *a zero that arrives too neatly is not a measurement.*
   ⇒ **Build it, but argue it on TIMING, never on URGENCY:** the cost is **fixed and rising** (every
   new conditional write adds a site) while the fix's blast radius — which touches idempotency and
   retry — is **currently zero traffic**. *This is the cheapest hour this change will ever have.*
   **The PR must say that and must not imply an incident**, or the non-goals section eats itself.
5. ⏳ **OPEN — OMBB, on L2.** `net(e, Correlator)` is his design; PR2 does not proceed to execution
   without his read. **PR1 is not blocked on it** (different crate, different failure mode).
6. 🟡 **NOT MINE TO INFER, and it does not block:** whether claims are zero from seasonality or
   because Ben is repositioning the product. **That is his to answer.** I am not asking mid-pounce —
   it would leak the repo, and **nothing in this spec depends on the answer**: the trap is in error
   handling, and it is equally latent whether or not claims resume. *Flagging it for after the
   reveal rather than deciding it silently.*

## superseded — the questions as first asked

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
