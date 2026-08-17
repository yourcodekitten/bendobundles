# the surfacing engine — reasons that fire once

**date:** 2026-08-17 · **author:** code kitten
**status: 🔴 RETRACTED — NOT BUILT.** family-reviewed 2026-08-17; OMBB step-5 sign-off received on
the decision *not* to build, with two conditions, both applied. **the criteria and the measurement
are the deliverable.**

## 🔴 VERDICT FIRST: the premise did not survive contact with production

**status: NOT BUILT, deliberately.** this document is kept because the *criteria* are durable and the
*measurement* is the deliverable. the reasons died. read this section before anything below it.

**the original lead argument of this spec was:** *"this app is sitting on kindness ben has never
seen"* — that `Link.thanked_at` is written and reaches him through nothing.

***it is false, and it was false in two independent ways.***

**① there is no kindness sitting there.** measured against `brd-prod-ue1-bendobundles-table`:

```
games 1114 · links 18 · claims 24 · total items 2025
thank-you notes ever left: 0
```

not *"0 unread"* — **0 ever.** `set_link_thanks` writes top-level `thank_note`/`thanked_at`; a
full-table scan finds **19 distinct top-level attributes and none matching /thank/i**, scan
positive-controlled.
⚠️ *and the near-miss on the way: the first query looked for `thanked_at` inside the serialized
`body` blob, where it is not. a filter on the wrong population returns `0` — **indistinguishable from
the truth it eventually found.***

**② and there IS a reader.** the grep that produced *"read by nothing"* was scoped to
`operator|notify|message|discord` — **push readers.** the feature is wired end to end:

```
friend affordance : web/src/friend/ThanksCard.tsx, rendered at LinkPage.tsx:511
route             : POST /api/l/{token}/thanks -> handle_post_thanks
store write       : set_link_thanks (top-level thank_note + thanked_at)
reader            : web/src/admin/Links.tsx:587 — "The friend's thank-you — read-only,
                    ben receives their words."
```

⇒ **nothing is broken, unwired, or undiscoverable. 24 people claimed a game and none chose to
thank.** *the sample cannot distinguish "poor affordance" from "people don't always thank," and
there is no query that can.*
⚖️ **so it is recorded as a FACT, not a problem: *it works; nobody chose to.*** **not** broken, **not**
undiscoverable, **not** an engagement failure — *we have now been wrong twice by deciding what a
number means before ben has.*
⚠️ **the population i examined excluded the answer** — which is the defect §"the problem this belongs
to" names, committed in the same document that names it.

## 🔴 AND THE MEASUREMENT THAT KILLED THE ENGINE: FLOW ≈ 0

*stock justifies a backfill; **flow** justifies an engine* (OMBB).

```
links   2026-07 -> 17   ·   2026-08 -> 1
claims  2026-07 -> 24   ·   2026-08 -> 0        (measured 2026-08-17)
reachability, all three reasons, against production:
  unread thanks : 0 rows            UNREACHABLE
  stale invites : 14 links with unused capacity, oldest 2026-07-03
  surplus keys  : 14 owned+giftable+unclaimed  (control: 297 owned_by_ben, 681 giftable
                  ⇒ the filters discriminate; 14 is a real intersection)
18 links across 1114 games ⇒ ~1.6% of the collection has ever been shared.
```

⇒ **every claim in the app's life happened in its launch month.** the engine would fire a backfill
and then be **correct and silent forever** — and *criterion ① would score that as success.*
🔴 ***the null state this document is proudest of would have concealed that the engine has nothing
to do.***

**and structurally, independent of the numbers (Lilith):** *two of the three reasons have flow
downstream of **ben's own engagement**, in an app that exists because he disengages.* **it goes
quiet exactly when the problem it was built for is at its worst** — ***a metric that cannot express
its own bad state.*** the only reason whose flow is exogenous is unread thanks, **and that is the one
with no data.**

## how both retractions got past review, recorded as a pattern

**Lilith reviewed 271 lines, produced six findings, and ratified the premise** — opening with *"your
opening finding is that `thanked_at` is written and reaches him through nothing"* and calling the
measurements *"real and controlled."* ⇒ ***she audited the mechanism and took the premise on
report.*** **OMBB asked the one question that mattered — *is the founding claim checkable?* — without
having read the spec at all.**
🔴 **and the scope was PRINTED in the evidence she quoted.** the block at the old §data table read
`filtered to operator|notify|message|discord` — **a push population, structurally unable to see
`admin/Links.tsx`.** *the filter was in the code block copied into the review; the conclusion was
read and the filter skipped.*
⇒ **reviewer-specific shape, and she asked for it recorded as a pattern rather than an incident:**
***a reviewer who checks the mechanism and trusts the lead has inspected the half that was already
argued.*** this is her second of the morning — the other is the C retraction below — **both inside
paragraphs where she was agreeing.** *nobody audits the paragraph that agrees with them.*
⇒ **and the author's half:** ***a spec's LEAD argument should be the most-measured thing in it, not
the least.*** the lead is what survives summarisation — mine propagated into a plan, nine tasks, a
commit message and four channel posts **before anyone counted the rows.**

## what was NOT concluded, because it was not measured

⚠️ *"the quiet is an engagement problem"* is **exactly as unmeasured as the lead this document
retracts.** ben shared 18 links, 24 were claimed, and he stopped. **that may simply be what
happened — a tool with a burst of use and then quiet is not broken; it may be finished.**
⇒ **whether low engagement is a defect or the natural shape of the tool is a question about what ben
WANTS, and there is no query for it.** *the criteria are ours, the threshold is his — and one turn
further out, **the problem statement is his too.***

## the problem this belongs to

the README's last line states the app's whole purpose:

> *built for ben, who forgets his bundles exist 95% of the time. this fixes that.*

**it is a pull system.** it fixes forgetting only when he remembers to visit — so the one thing it
was built to solve is the one thing its architecture structurally cannot reach.

⇒ ***a mechanism whose reach excludes its own stated purpose.*** this is not a feature request. it
is the same defect class as a readiness probe that enumerates `.ports` while claiming to report on
*channels*: **the population it examines excludes the subject it is named for.** push is the
correctness fix.

## what the data actually supports (measured, not assumed)

**two of the four "why now" hooks in the original pitch were fiction**, and specifying killed them —
naming the field forces contact with the artifact that pitching never does.

| hook | verdict | evidence |
|---|---|---|
| purchase anniversary | 🔴 **fiction** | `Game` has **no temporal field at all** — no purchase date, no ingest date, no first-seen |
| "the bundle turned N" | 🔴 **fiction** | `Game.bundle` is an unparsed name string |
| unspent Choice pick | 🟡 **field, no data** | `requires_choice` exists; its own doc says *nothing writes `true` yet* |
| unread thanks | 🟢 **real** | `Link.thanked_at: Option<OffsetDateTime>` |
| stale invite | 🟢 **real** | `Link.created_at: OffsetDateTime` |
| surplus key | 🟢 predicate real · ⚠️ **transition NOT MEASURABLE** | `Game.owned_by_ben: bool` + `giftable` + unclaimed — **14 in prod. but a bare `bool` has no timestamp, so its FLOW cannot be measured at all: case 4 of criterion ⑥, instrument before judging** |

**and the upstream is bare too** — humble supplies no dates, verified against captured payloads
rather than against the wire model alone:

```
crates/humble-client/src/model.rs        : zero temporal fields
tests/fixtures/order_detail.json         : no date-ish key
tests/fixtures/user_order.json           : no date-ish key
POSITIVE CONTROL: the same grep found release_date / start_date / end_date in the STEAM
                  fixtures ⇒ the instrument works; the humble payloads are genuinely bare
```

⇒ **the collection has no history; only the app does.** every reason must be built from
`Link`/`Claim` timestamps and record transitions, never from purchase time. adding purchase
temporality is a **deliberate schema change with ben's name on it**, not a hook we get for free.

## the five criteria a surfacing must meet

Lilith's ①-⑤ and ⑥, 2026-08-17. ⑥ was added *after* the measurement killed this design.
⚠️ **honesty about enforcement: ①②④⑤ each have a mechanism below. ③ does not — it is a design
principle here, not an enforced arm, and saying so beats a five-item claim where one item isn't**
(Lilith's review: *count-vs-list, in the sentence asserting enforcement*).

1. **it must be able to say nothing.** a digest of *N games* has no null verdict **by construction**
   — it manufactures content on a quiet day, and that is the day it becomes furniture. **a
   surfacing with no silent state is an alarm that cannot be switched off.**
2. **the "why now" must be falsifiable.** a reason that could not have been false is not a reason.
   every line must be a claim that could be gone and refuted — the standing law here is already
   ours: ***a notification you cannot verify is a rumour*** (#171).
3. **the sender bears the cost.** *"here are five games"* costs ben the work of evaluating five
   games. **interrupt only when you have already done the work he would otherwise have to do.**
4. **tunable per-reason, never all-or-nothing.** an unsubscribe is a cliff — he will kill the whole
   channel to stop the one reason that annoys him, and the four that worked die with it.
5. 🔑 **the trigger is the event, not the calendar.** ***if the schedule is the trigger, the content
   is filler by default.*** a daily digest is a schedule hunting for content. EventBridge still
   drives the tick; it **evaluates predicates** instead of filling a quota.

6. 🔑 **A NULL STATE NEEDS A FIRE-RATE FLOOR — AND THE FLOOR HAS THREE VERDICTS AND TWO APPLICATION
   POINTS.** (Lilith, after the measurement below killed this design; refined three times by OMBB and
   restated by her. The evolution is recorded at the end because *how* it got here is the useful part.)

   **The defect it repairs:** ***a null-capable engine cannot distinguish "correctly quiet" from
   "structurally empty."*** An engine that fires zero times in a year makes criterion ① report
   **perfect health, every day.** *"Nothing fired in N days" is not silence — it is a dead engine, and
   it must say so in a different voice.* This is the **assertion-count floor aimed at a product**: a
   suite with no floor cannot tell *34 passed* from *34 passed with a stage silently skipped.*

   **The rule.** A fire-rate floor is only meaningful over a nonzero flow. **Derive N from measured
   flow — never pick it.** If the derivation has no answer, **ask why the zero**, and answer with one
   of *three* verdicts:

   | why the flow is zero | verdict | action |
   |---|---|---|
   | the generating behaviour **EXISTS** and its rate is ~0 | 🔴 **FIRE** | **don't build.** *(this app: claims and links exist, and are near-zero)* |
   | the source **does not exist yet** (greenfield) | ⚠️ **NOT MEASURED** | **silent, never permissive.** justify separately, and **schedule the measurement as a JOB** |
   | the source **EXISTS but is UNINSTRUMENTED** — the behaviour happens, nothing records *when* | ⚠️ **NOT MEASURED** | **instrument the flow first.** do not proceed on a FIRE or a CLEAR you cannot see |
   | flow is nonzero and measured | ✅ **CLEAR** | derive N and set the floor |

   ⚠️ **the fourth row is not the greenfield row folded over — the ACTION differs.** greenfield says
   *justify separately and schedule the measurement.* **case 4's fix is available today: add the
   timestamp.** *waiting for post-ship data when you could instrument it this afternoon is the wrong
   remedy.* (Lilith, catching that her own table had **three verdicts over four cases** — *at least
   the direction of the error changed.*)
   🔴 **AND THIS DOCUMENT CONTAINS A LIVE INSTANCE OF CASE 4 AND WAS SILENT ABOUT IT:** **reason 3,
   surplus keys.** 14 in stock; **flow unmeasurable, because `owned_by_ben` is a bare `bool` with no
   timestamp** — which is precisely *why* the clearing marker had to double as its transition
   detector. **that was said in the review channel and never written here.** ⇒ *the spec carried a
   specimen of the case its own table could not classify.* **reason 3's honest verdict is NOT
   MEASURED / instrument first — not the 🟢 the data table gave it.**

   ⚠️ **A KILL DATE IS NOT A MECHANISM.** *"Justify another way, with a kill date"* is a date someone
   writes down, and **a date passes silently.** ⇒ **it must be a job, not a calendar note:** *ships at
   D; at D+90 the flow is MEASURED and ⑥ is evaluated for real.* otherwise NOT MEASURED becomes
   permanent and the feature lives on a promissory note nobody redeems — ***a deferral the tool
   doesn't print is one you forgot***, and this one has to print itself.

   ⭐ **TWO APPLICATION POINTS, chosen by which verdict ⑥ returns:**
   - **brownfield — ⑥ asks *should this be built?*** measurable now. **it kills this app's engine at
     spec stage**, which is where this document should have died.
   - **greenfield — ⑥ asks *did this earn its existence?*** measurable only after shipping, so ⑥
     becomes a **RETIREMENT criterion.** *this is the more valuable half long-term, because nothing
     we build currently has a mechanism for asking whether it should still exist.*

   📌 **How it got here, because the evolution is the lesson.** ⑥ was written as the fix for ①'s blind
   spot **and carried ①'s mirror**: ① cannot see a dead engine; a mis-set ⑥ cannot see a **healthy
   quiet** one, and on this app *any* N would be red on day one and red forever — ***a fix that
   introduces the inverse of the defect it repairs.*** Reframed as a build/don't-build gate, it then
   **forbade all greenfield work**, since a not-yet-shipped feature has flow `0` by construction —
   *the always-red check one layer up: a gate that says no to everything new, and gets switched off.*
   🔴 **Both versions had TWO verdicts, and both collapsed *"I cannot judge this"* into one of the
   two** — the exact defect the three of us spent a night on, shipped twice inside one criterion, the
   second time in the replacement for the first. **The third verdict is what makes it work.**

## the mechanic: FIRES-ONCE

the cut is **not** state-vs-event. it is whether the thing has a **first time** — and *a standing
fact plus a threshold IS a transition*:

- ❌ *"you own 40 surplus keys"* — state, true every day. **furniture.**
- ✅ *"you just crossed 40"* — transition. **fires once.**
- ✅ *"this key has been surplus for a year"* — a state **with an age**, which has exactly one
  moment when the age crosses the line.

**we already own the pattern.** `host-agent-watch` writes its marker above threshold and **`rm -f`s
it below**. ⇒ ***a reason needs a marker so it cannot fire twice, and the marker must clear so it
can fire again when genuinely true again.*** that is the whole engine:

```
state + threshold + a clearing marker = a surfaceable event
```

🔑 **and the marker is not merely anti-furniture plumbing. for a value with no observable history —
a bare `bool` — the marker IS the transition detector.** its absence is the only evidence that a
currently-true predicate is *newly* true. that is a stronger argument for the pattern than
"stop it firing twice."

## 🔴 the first-run hazard: an absent marker is a PER-REASON policy

(OMBB, 2026-08-17, on the spec above.) **on the first deployment no key has a marker**, so
*marker-absent-means-new* makes **every** qualifying record read as new. ⇒ the first run would page
ben with **forty items at once** — *not news; furniture delivered all at once, on day one.*

**so "what does an absent marker mean?" must be decided per reason, and getting it backwards is
expensive in both directions:**

| reason | first-run policy | why |
|---|---|---|
| 💰 **surplus key** | **SEED SILENTLY** | record what exists, announce nothing. only *later* arrivals are news. **he already knows he owns them.** |
| 💔 **unread thanks** | **ANNOUNCE THE BACKLOG** | he is **owed** these — silence is the bug being fixed. **seeding them silently would ship the fix and have it behave exactly like the break.** |
| ⏳ **stale invite** | **seed silently, then threshold** | the age is real but the *crossing* under our watch is the event. |

⇒ ***same mechanism, opposite defaults.*** every reason declares its first-run policy **with its
reason**, in code, not in a comment.

**and the policy is chosen by a ONE-QUESTION TEST, so whoever adds reason #4 gets the answer without
re-deriving it** (Lilith): ***would he say "why didn't you tell me sooner?"***
- **yes ⇒ DEBT ⇒ announce the backlog.** he is owed it; **a remedy whose output is
  indistinguishable from the defect has not been deployed.**
- **no ⇒ INVENTORY ⇒ seed silently.** a standing fact he already knows; announcing it is
  **furniture wearing a migration's clothes.**

⚠️ **third case — a debt that is also LARGE.** fourteen unread thanks going back to march are *owed*
**and** unshippable as fourteen messages. ⇒ **announce the debt's EXISTENCE and SIZE, not its
contents**: *"you have 14 unread thank-you notes, oldest from march."* **the backlog is a single
event; its items are not.**

## 🔴 event decides IF; calendar decides WHEN

(OMBB, same review.) criterion ⑤ says the event triggers, not the calendar — **and taken alone that
hands the moment to the event.** events fire at 03:00, or five of them inside ten minutes. **ben's
attention has hours; a predicate does not.**

⇒ **the calendar returns, not as the trigger but as a GOVERNOR ON DELIVERY:** the engine evaluates
whenever the tick runs and records what fired; **delivery coalesces the fired set into one message
at a civil hour.** this preserves ① exactly — a window with nothing in it sends nothing — and kills
the 3am page. *not a contradiction of ⑤; its completion.*

**phase-1 consequence:** the dry-run ledger must record **when each reason fired**, so the
coalescing window is designed against real timing data rather than guessed at in phase 2.

🔴 **AND THE HAZARD INSIDE THE GOVERNOR — where criterion ① is most likely to die.** a coalescing
delivery job **must be able to produce no message at all.** if it wakes at its civil hour and sends
*"nothing today"*, we have **built the null state and then thrown it away in the plumbing.**
⇒ ***a delivery scheduler is exactly where a null verdict dies, because a job that runs feels like
it owes a report.*** the governor's **silent path is an asserted arm like any other** — and it is
the one nobody will think to test, *because testing it means running a job and watching it do
nothing.*

## phase 1 scope — the reason engine, run DRY   🔴 *(NOT BUILT — see VERDICT FIRST)*

> ⚠️ **everything from here to §"the threshold is ben's" is the design as it stood BEFORE the
> measurement.** it is kept because the mechanics are reusable; **it is not a plan of record and
> nothing in it was implemented.** *a scope section reads as intent, and intent reads as approval.*

🔑 **the delivery door is NOT in phase 1, deliberately.** the sealed `DigestMessage` type mirroring
`OperatorMessage` is the satisfying part and it would feel like progress; building it first is the
mistake. **build the reason engine with no delivery at all, run it dry, and read the log.**

⇒ ***a dry run is the positive control for the digest itself.*** if the ledger is boring, that is
learned **before** a channel exists — instead of discovering it by paging ben. if the null days are
frequent and the fire days are genuinely interesting, the product has been **measured** rather than
hoped for.

**in scope:**

- a **reason** vocabulary: each reason names its subject, its predicate, and **the evidence that
  makes it falsifiable** (criterion ②) — a reader must be able to go re-derive it from the store.
- **transition detection with clearing markers**, per the mechanic above. a reason that is true
  today and was true yesterday **does not fire**.
- reasons implemented, in this order (Lilith's ordering, adopted):
  1. **unread thanks** — a real transition, a real column, a human on the other end who has been
     quietly ignored.
  2. **stale invite crosses a threshold** — `Link.created_at`, unclaimed, age crossing the line.
  3. **surplus key appears** — an unclaimed giftable key **observed for the first time** with
     `owned_by_ben == true`. *the 41st arriving is news; owning 40 is furniture.*
     🔴 **NOT "the flag flipping" — that is undetectable and the first draft of this spec said it.**
     `owned_by_ben` is a bare `bool`: **a boolean has no transition without a prior value**, so
     nothing in current state distinguishes *flipped today* from *flipped last March*. that is the
     same defect as the temporal hooks above, **one layer subtler because the field genuinely
     exists.** (Lilith, 2026-08-17, catching it in this document.)
     ⇒ **the fix is free and it upgrades the pattern:** the clearing marker does double duty —
     **write a marker the first time the predicate is observed for a key; *marker absent IS the
     "this is new" signal*.** no schema change, and it is `host-agent-watch`'s shape exactly. the
     alternative, an `owned_by_ben_at` timestamp, is a schema change (so **ben's**) and only ever
     works forward.
- an **explicit null verdict**: on a tick where nothing transitions, the engine records
  `NOTHING TO SAY` — a first-class outcome, not an empty list.
- a **first-run policy per reason** (seed-silently vs announce-backlog), declared in the reason's
  own definition so a new reason cannot be added without choosing one.
- **fire timestamps in the ledger**, so phase 2's coalescing window is designed against measured
  timing rather than a guess.
- a **dry-run ledger**: every tick writes what it *would* have said, with the evidence, and whether
  it fired or was silent.
- driven by the **existing** EventBridge rule + fulfillment lambda; no new schedule.

**explicitly NOT in scope:** any delivery to any human, the `DigestMessage` type, discord/email/web
surfaces, cadence, and thresholds ben has not chosen.

## the threshold is ben's, not ours

⚖️ **the criteria are ours; the threshold is his.** *"what is worth interrupting me for"* is a
question about **his** attention, and he is the one whose attention it is — the same way the box's
`terraform apply` is his button. **do not ship a cadence chosen for him.** phase 1 produces the
ledger; **he sets the bar from real data**, and per-reason tuning (criterion ④) is designed in from
the start rather than retrofitted.

## approaches considered

- **B · the matchmaker** (friends declare wants; system matches unclaimed keys) — **parked, and not
  on value grounds.** it spends **social capital belonging to a person who never granted it**, on
  behalf of someone who cannot see it being spent. worse, **the feedback path runs backwards**: if
  the matcher misfires, the *friend* sees it and has no channel; *ben* has the channel and cannot
  see it. **a system whose misfires are invisible to its operator will misfire indefinitely**, and
  *"nobody complained"* is not a measurement. also: **a want is a claim with a timestamp and nothing
  re-confirms it** — a wants-store is made of the wrong material unless it decays.
  ⇒ **A is B's positive control.** ben is the only population where the loop closes: he receives it,
  operates it, and can say it is wrong. **A earns the right to build B.**
- **C · the retrospective** (15 years as a collection story: eras, taste drift) — **parked for lack
  of substrate.** it was the natural reason-generator for A, and the measurement above shows the
  temporal data does not exist in our model *or* in any captured humble payload. revisit only
  alongside a deliberate schema change.
- 🔴 **C was recommended, then refuted by a measurement that had already been published.** Lilith
  proposed C as A's reason-generator **in the same message that quoted my finding that `Game` has no
  temporal field at all** — *"the record was right there and the reflex won,"* her own sentence from
  four hours earlier, earned again in one message. she asked for it named here rather than tidied
  away. ⚠️ **note the shape it wore: the error rode inside a paragraph praising me for killing two
  hooks against the schema, while proposing a third that dies to the same check** — *a compliment is
  a fine place to hide a defect, because nobody audits the paragraph that agrees with them.*

## testing

**phase 1 (what this plan builds):**

- **null state is a test, not a hope:** a tick with no transitions must produce `NOTHING TO SAY`,
  asserted — and a sibling arm must produce a fire from the same fixture, or the null arm proves
  nothing.
- **fires-once:** the same true-state across two consecutive ticks fires **once**; the marker
  clears; a genuine re-occurrence fires again. all three arms.
- **falsifiability:** every emitted reason carries evidence sufficient to re-derive it from the
  store; a test re-derives one and fails if it cannot.
- **the placement-pin rule applies** (house convention): each test asserts the property at the
  layer that owns it.

**carried forward to phase 2, recorded here so it is not re-derived** — ⚠️ *phase 1 builds no
delivery, so it cannot test a governor; asserting a test for code that does not exist would be its
own defect:*
- **the governor's silent path must be an asserted arm:** a delivery window whose fired-set is empty
  produces **no message**, with a sibling arm that produces a message from the same harness — or the
  silent arm proves nothing.

## family review (2026-08-17, shared channel)

- **Lilith** — the five criteria; the event-not-calendar inversion (⑤) that restructured this from a
  digest into a predicate engine; the **build-the-engine-before-the-door** ordering and *"a dry run
  is the positive control for the digest itself"*; the **first-time / clearing-marker** refinement
  that rescued surplus keys and stale links from the furniture bin; the B verdict; *"the criteria
  are ours, the threshold is Ben's."*
- **OMBB** — the **first-run hazard** (an absent marker is a per-reason policy; surplus seeds
  silently, thanks announce the backlog — *"seeding them silently would ship the fix and have it
  behave exactly like the break"*), and **event decides IF, calendar decides WHEN** (the calendar
  returns as a governor on delivery, coalescing into one message at a civil hour — ⑤'s completion,
  not its contradiction). Formal sign-off pending (pounce step 5).
