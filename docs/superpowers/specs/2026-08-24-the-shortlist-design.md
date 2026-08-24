# the shortlist — narrowing 670 without choosing

**date:** 2026-08-24 · **author:** code kitten (pounce arc) · **status:** draft, pre-family-review

## the idea in one line

Ben has **670 giftable games** and has ever shared **18 links**. The app should turn "which of 670?"
into "which of these six?" — **and never make the pick itself.**

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

## 🔑 the finding that makes this more than a filter box

**570 of the 670 are `requires_choice = true`** — Humble Choice games where claiming **spends one of
Ben's finite monthly picks** (`domain/src/lib.rs:979` *"a choice game got chosen: the next key-sync
fresh carries requires_choice=false"*; the friend surface already says **"confirm? spends 1 pick"** at
`selfClaimLabel.ts:20`).

⇒ ***The app knows the pick cost and only reveals it at CLAIM time — to the friend, at the moment of
the unwrap.*** Ben composes the gift with **no view of what it will cost him**. Roughly **85% of his
apparent pool is not free**, and nothing tells him that while he is choosing.

**That is the real defect this spec closes**, and it was invisible until the pool was measured by field
rather than counted.

## what gets built

A **shortlist** panel in the admin compose flow (`admin/Links.tsx`), beside the existing catalog
toolkit that "chosen for you" (`2026-08-19`) already ships:

1. **Narrowers, not rankers.** Facets over data already in the row — *era/bundle*, *free vs spends a
   pick*, *has cover art*, *duplicate you hold twice*, *never offered on any link*. Each is a filter
   with a visible count. **No score, no "recommended", no ordering by cleverness.**
2. **Pick-cost made honest at compose time.** Every candidate shows free / *spends 1 pick*, and the
   composed gift shows the total. The friend's claim-time label is unchanged.
3. **"Never offered"** — the one genuinely new fact: which games have never appeared on any link.
   Computed from `curated_game_ids` across links. With 18 links this is nearly the whole pool, and
   **that is the honest answer, not a bug** — it is a fact about the collection, displayed plainly.

### explicitly NOT built

- **No recommender, no scoring, no ML, no "for you" ranking.** Design Principle 2 is *chosen-for-you,
  never shopping*, and the emotional core is that **Ben** picked. A machine that picks destroys the
  product's premise to save him thirty seconds.
- **No metric cards, no dashboard chrome** (`PRODUCT.md` anti-references — *"not even on the admin,
  which is a workbench, not a dashboard"*).
- **No notification, no digest, no reason that fires.** That is the retracted engine.

## open questions for the family (step 2)

1. **Does a shortlist erode "chosen for you"?** My position: narrowing ≠ choosing, and pick-cost is
   information he is currently denied. But the line is real and I want it tested.
2. **Is "never offered" honest at n=18 links?** It will read ~"everything". Display it, or hold it
   until the population makes it meaningful?
3. **Backend or pure frontend?** `curated_game_ids` is already on `AdminLink`, so "never offered" is
   computable client-side from the existing list call. Cheaper, but O(links × games) in the browser.

## success criteria

- Ben can go from the admin to a composed, sent gift **without scrolling 670 rows**.
- The pick cost of a gift is visible **before** he sends it, not after a friend claims it.
- Every existing flow is unchanged; the shortlist is additive and skippable.
