# the shortlist — narrowing 670 without choosing

**date:** 2026-08-24 · **author:** code kitten (pounce arc) · **status:** REVISED after family review

> 🔴 **REVISION 2026-08-24, and it changed the feature, not the wording.** The first draft's headline —
> *"Ben composes a gift with no view of what it COSTS him"* — **was false.** Lilith asked the one
> question that mattered (*when is the pick decremented, and against whose pool?*) and the claim path
> answers it: **the pick is spent at CLAIM, by the friend, on their clock, possibly never.**
> ⇒ ***There is no cost to disclose. There is an EXPOSURE.*** *"This costs 4 picks" would have been a
> confident wrong number about a finite resource Ben cannot get back — worse than the silence it
> replaces.* Sections below are revised in place; §what-gets-built item 3 is **deleted**, with the
> measurement that killed it.

## the idea in one line

Ben has **670 giftable games** and has ever shared **18 links**, **every one of them an open shelf.**
Show him what a gift EXPOSES of his finite monthly Humble picks, and let him narrow the pool on that
axis — **never making the pick itself.**

> 🔴 **The first draft said *"turn 'which of 670?' into 'which of these six?'"* and that promise had NO
> MECHANISM.** Lilith grepped the whole spec for an output bound (`six|top N|limit|truncat|first N`) and
> got **one hit: the sentence itself** (control: `count` → 3, so the probe reads the file). **Facets take
> 670 → whatever survives, which could be 400.** The only two things that yield a fixed small set are
> **ranking** and **truncation**, and both are in §explicitly-NOT-built. ⇒ ***A description of the
> feature was reading as a claim about output size — present indicative, no act behind it.***
> **Resolved by dropping the claim, not by adding a limit:** facets stack until *Ben* judges the list
> short enough. The success criterion below is restated to match.

## why this and not the obvious thing

🔴 **The obvious idea is already adjudicated.** *"Surface the gifts that were sent and never opened"*
is verbatim reason ② (*stale invites*) of
[`2026-08-17-surfacing-engine-design.md`](2026-08-17-surfacing-engine-design.md), which is **RETRACTED
— NOT BUILT**, with an OMBB step-5 sign-off on the decision not to build. Two arguments killed it and
**both are load-bearing here too**:

- **OMBB: *stock justifies a backfill; flow justifies an engine.*** Claims: `2026-07 → 24`,
  `2026-08 → 0`. Every claim in the app's life happened in its launch month.
- **Lilith, structural:** a surface whose flow sits downstream of Ben's own engagement, in an app that
  exists *because he disengages*, **goes quiet exactly when the problem is at its worst.**

✅ **This proposal passes both, and the reason is the distinction between an ENGINE and a TOOL.**
A shortlist does not *fire*; it sits inside the act Ben already came to perform. It consumes **stock**
(670 games), which is precisely what OMBB's rule says stock is good for. It cannot go quiet when he
disengages, because it does nothing until he arrives.

## the measurements this rests on

All against `brd-prod-ue1-bendobundles-table`, 2026-08-24, **full scan, no truncation**
(`Count == scanned == 2024`, `LastEvaluatedKey` absent — asserted, not assumed):

```
GAME# items                                        1114
by status   ben_redeemed 415 · available 671 · gifted 9 · expired 18 · pending 1
GIFTABLE POOL  (giftable & !hidden & unclaimed & status!=ben_redeemed)   670
  bundles represented                               96
  carries steam_app_id                             533 / 670
  requires_choice = true                           570 / 670   <-- see below
  duplicate titles (same game, >1 key)              19
  pool titles ben ALSO redeemed himself              7
links 18 · claims 24 · ~1.6% of the collection has ever been shared
```

⚠️ **The retracted spec's "14 owned+giftable+unclaimed" is a DIFFERENT, narrower intersection** (surplus
keys Ben already owns). Re-measured here rather than inherited; the giftable pool is **670**, not 14.
*A number quoted from a neighbouring document is not a measurement of your own population.*

## 🔑 the finding — an EXPOSURE, and a ceiling nobody had named

**570 of the 670 are `requires_choice = true`** — Humble Choice games where claiming **spends one of
Ben's finite monthly picks** (`domain/src/lib.rs:979` *"a choice game got chosen: the next key-sync
fresh carries requires_choice=false"*; the friend surface already says **"confirm? spends 1 pick"** at
`selfClaimLabel.ts:20`).

**Traced in the claim path, not inferred from the schema** — `fulfillment/src/lib.rs:127`, on
`FulfillRequest::Gift.requires_choice`: *"`true` ⇒ dispatch the two-write Choice orchestration (**spend a
monthly pick via `choosecontent`**, THEN redeem the freshly-minted key)"*, and `:1013` *"a TWO-write
one-shot that **must spend a monthly pick exactly once** … dispatched when `requires_choice` is set."*
**Fulfillment runs on CLAIM.**

⇒ ***The spend is the FRIEND's act, at a time Ben does not control, and it may never happen.*** A
six-game link where all six are `requires_choice` does not cost six picks; it costs **between zero and
six**. The honest artifact is a range, and a range is a different UI — not a different label.

🔑 **THE CEILING, which neither reviewer named and which makes the number tighter than "up to six":**
`domain/src/lib.rs:314` refuses a claim once `claims_used >= claims_allowed`.

> ### curated link:  EXPOSURE = min( #`requires_choice` among the picked games , `claims_allowed` )
> ### open shelf:     EXPOSURE = `claims_allowed`   (570 of the pool are `requires_choice`; the pool never binds)

True under **every** claim ordering, and it **tightens as Ben lowers the allowance** — which makes
`claims_allowed` legible as the exposure dial it has silently been all along.

🔴 **AND THE OPEN-SHELF ROW IS NOT A DEGENERATE EDGE CASE — IT IS 18 OF 18 IN PRODUCTION.** OMBB pushed
exactly here: *if nothing writes `curated_game_ids`, which games Ben picks cannot move the exposure, and
`min()` collapses to `claims_allowed`.* **He is right about the arithmetic.** Traced the write path
before answering: `Links.tsx:135` builds `gameIds` from the picks and `adminCreateLink` POSTs them as
`game_ids` — **so curation is reachable and wired, merely never used.** *Unused is not uncallable, and
the difference decides whether this is a feature or a fiction.*

⇒ ***Every link Ben has ever sent is an open shelf, so every one of them exposes `claims_allowed` picks
to whoever holds it — and nothing has ever said so.*** That is a live property of production, not a
hypothetical, and it is a **stronger finding than the narrowing tool that led me here.**

✅ **It also gives the unused curation feature its first real reason to exist: curating is how Ben BOUNDS
his exposure.** The readout is what makes that visible — and it is why the open-shelf row must render
first, not as a footnote.

**That is the real defect this spec closes:** roughly **85% of Ben's apparent pool can spend a finite
resource, and the only party told is the one not spending it.**

## what gets built

A **shortlist** panel in the admin compose flow (`admin/Links.tsx`), beside the existing catalog
toolkit that "chosen for you" (`2026-08-19`) already ships:

1. **Narrowers, not rankers.** Facets over data already in the row — *era/bundle*, *free vs spends a
   pick*, *has cover art*, *duplicate you hold twice*, *never offered on any link*. Each is a filter
   with a visible count. **No score, no "recommended", no ordering by cleverness.**
2. **Exposure made honest at compose time.** Every candidate shows *free to give* / *spends a pick*,
   and the composed gift shows **"up to N of your monthly picks, if they claim"** — never a flat total.
   It renders beside the `claims_allowed` input in `admin/Links.tsx`, because `min()` needs both the
   picks and the allowance and **only that screen has both.** The friend's claim-time label is unchanged.
   ⚠️ **An unknown must never read as zero.** If a picked game's `requires_choice` is unavailable, print
   *"reload to see pick exposure"* — **understating a finite resource is the dangerous direction.**

### 🔴 deleted: "never offered on any link"

The first draft proposed a facet for games never named on a link. **Measured against production before
building it: `curated_game_ids` is absent from all 18 links** — control: 18 items carry `claims_allowed`
so body-search works, and the only two `curat*` hits table-wide are `STEAMAPP#` rows (steam *curator*
metadata, a different entity). ⇒ ***Per-link curation has never been used in production, so "never
offered" is true of 670/670 = 100%.*** A badge on everything is furniture that reads as signal.

**And its inversion dies too** (OMBB proposed marking the already-sent as a duplicate-gift guardrail):
population **0** via that field, *and* **the guardrail is already enforced by construction** — the
giftable pool filters `claim_id is None`, so a claimed game cannot re-enter it. *The finding was right;
the fix was already there.*

### explicitly NOT built

- **No recommender, no scoring, no ML, no "for you" ranking.** Design Principle 2 is *chosen-for-you,
  never shopping*, and the emotional core is that **Ben** picked. A machine that picks destroys the
  product's premise to save him thirty seconds.
  🔑 **The runnable form of that rule (OMBB): every facet's source column must belong to the GAME
  table.** A facet computed from friend-data, or from Ben's history, is a recommender in a filter's
  coat and erodes the principle *whether or not it ranks*. `requires_choice` is a game column. ✅
  ⚠️ **And "no ranking" is not achieved by declining to rank** — something orders the list. **Ours is
  `IDLE_TOOLKIT.sort = 'title'`, a–z, Ben's existing default.** Named here so it is a decision rather
  than whatever the filter happened to emit.
- **No metric cards, no dashboard chrome** (`PRODUCT.md` anti-references — *"not even on the admin,
  which is a workbench, not a dashboard"*).
- **No notification, no digest, no reason that fires.** That is the retracted engine.

## the three questions, answered by the family (step 2, closed)

1. **Does narrowing erode "chosen for you"?** **No, subject to a runnable gate** — see the facet rule
   above. Both reviewers reached it independently; OMBB supplied the assertable form.
2. **Is "never offered" honest at n=18?** **Dead — 100% vacuous.** Deleted above with its measurement.
   Lilith argued the inverse is the informative side; the inverse turned out to have population 0 and a
   guardrail that already exists.
3. **Backend or pure frontend?** **Frontend, free choice.** `admin-api/src/lib.rs:22` and `:91`: every
   route but `/login`/`/logout` is session-guarded by **middleware applied via `route_layer` on the
   protected sub-router**, so a new admin route cannot forget it. OMBB raised inventory disclosure and
   then retracted it himself on reading that the panel is admin-side.
   ⚠️ **What stays backend-shaped: a live pick BALANCE.** Not shipped, not claimed. Exposure uses
   `requires_choice` — *the same field fulfillment gates on* — plus the allowance Ben is typing. **Truth
   does not get a second implementation.**

⚖️ **Weighting note (Lilith's, and it changes how this review should be read):** she and OMBB converged
on four calls with zero contact, *"and two people running one model is one instrument twice."* ⇒ **the
DIVERGENCES carried the information** — his disclosure question (which he retracted himself) and her
decrement question, **which is the one that changed the feature.**

## success criteria

- Ben can narrow the pool along a real axis and **stop when the list is short enough for him** — no
  claim is made about how many rows survive, because nothing in this build bounds that.
- The **exposure** of a gift — *up to N picks, if they claim* — is visible **while he composes it**, not
  discovered after a friend spends one.
- **No number is ever printed that could be wrong in the understating direction.**
- Every existing flow is unchanged; the shortlist is additive and skippable.
