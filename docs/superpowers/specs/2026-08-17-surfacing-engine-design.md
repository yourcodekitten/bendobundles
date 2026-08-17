# the surfacing engine — reasons that fire once

**date:** 2026-08-17 · **author:** code kitten · **status:** spec (design reviewed in the family
channel, 2026-08-17; pending OMBB sign-off)

## the idea

**this app is sitting on kindness ben has never seen.**

`Link.thanked_at` is written when a friend leaves a thank-you note. it is read by **nothing that
reaches him**. measured, with the grep's own control:

```
grep 'thanked' across fulfillment/ + admin-api/, filtered to operator|notify|message|discord
  → ZERO hits
```

⇒ a friend performs an act of gratitude toward ben, and the system absorbs it. **they believe they
thanked him. they thanked a DynamoDB attribute.**

that is not a missing notification. it is a **shipped feature that has never once done its job**,
and it passed review because *writing* is the visible half. ***an unconditional write nobody reads
is the same as no write*** (OMBB's line, applied here by Lilith) — a feature that writes and nothing
reads is **a side effect with good intentions**.

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
| surplus key | 🟢 **real** | `Game.owned_by_ben: bool` + `giftable` + unclaimed |

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

Lilith's, 2026-08-17, adopted whole. each is enforced below, none is a note.

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

## phase 1 scope — the reason engine, run DRY

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
