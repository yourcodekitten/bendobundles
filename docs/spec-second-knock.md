# The Second Knock 🚪

*Sixty-seven gifts are sitting on doorsteps nobody opened. The house has a window that looked at them
exactly once, three months ago, and no hand that can knock again.*

**Status:** spec · 2026-09-28 · author: code kitten
**Register family:** sibling of the lantern (`docs/spec-lantern.md`) and of the doorstep
(`docs/spec-doorstep.md`). **The doorstep gave the stuck CLAIM a voice, a surface and a hand. This
spec is the same three things for the un-walked DOOR.**

---

## 1. The specimen — measured in production 2026-09-28, not recalled

`AWS_PROFILE=kitten-debug`, table `brd-prod-ue1-bendobundles-table` (2047 items), full scan:

| | retraction's figure, 2026-08-17 | **measured 2026-09-28** |
|---|---|---|
| links | 18 | **18** |
| claims | 24 | **24** |
| games | 1114 | 1124 |

- **Every one of the 24 claims landed between 2026-07-03 and 2026-07-14.** Eleven days at launch, then
  **nothing for 2.5 months.**
- **11 of 18 links have never been claimed once. 67 unclaimed slots are live right now.**
- **Every link created on or after 2026-07-10 has zero claims** — 7 links, 41 slots.

```
LINK#5b87…  2026-07-03   2/1      LINK#773e…  2026-07-10   0/5
LINK#cea2…  2026-07-04   1/1      LINK#89e7…  2026-07-10   0/5
LINK#50de…  2026-07-05   1/1      LINK#f7ed…  2026-07-10   0/5
LINK#61c3…  2026-07-05   1/1      LINK#fd4e…  2026-07-10   0/5
LINK#9500…  2026-07-06   1/1      LINK#79eb…  2026-07-13   0/15
LINK#d015…  2026-07-07   1/1      LINK#df54…  2026-07-28   0/5
LINK#e53b…  2026-07-07   0/1      LINK#72c1…  2026-09-04   0/1
LINK#14e9…  2026-07-09   6/15     LINK#4b65…  2026-07-09   0/5
LINK#99cb…  2026-07-09   0/10     LINK#9c6f…  2026-07-09   0/2
                                  TOTAL 13/80 used ⇒ 67 slots waiting
```

**`PRODUCT.md`'s own sentence: *"an unclaimed game is a gift nobody got to open."* It is happening 67
times, and nothing in the product says so.**

## 2. Why it is invisible, and why that is by design rather than by bug

The lantern's 🚪doors room mentions a door **only** in the tick whose bucket contains
`created_at + 14d`, and again at `+60d` — *"never otherwise"* (`spec-lantern.md`, doors row). Every
July link is past both birthdays.

The spec anticipated exactly this and mitigated it with a **backlog summary line** — *"and N doors
older than two months nobody has walked through"* — gated on **no lantern having ever been
DELIVERED**. One line, once.

**Measured: `LANTERN#2026-09-20` carries `delivered = true`. The line is spent.** The 09-27 tick
reports `doors: 1`.

⇒ **Those 11 doors will never be mentioned again.** The lantern spec says so in its own voice:

> *the lantern is about **state that is still standing** — 9 of the 11 open doors are past both
> birthdays at launch and would never be mentioned, **which is the exact forgetting the first
> paragraph describes, left in place.***

**One line, once, is the right dose for an EVENT. These are STATE, and state that is still standing
needs a surface, not a birthday.**

## 3. What is NOT broken — so the fix is not aimed at the wrong thing

- **Claiming works.** 24 claims succeeded at launch; the 13 used slots prove the path end-to-end.
- **The lantern works.** It fires weekly and `delivered = true` on both rows. **Ben is reachable.**
- **Curation works.** `chosen-for-you` (per-link picks) and `gift-tags` (per-pick reasons) shipped.
- **The catalog works.** 1124 games, 871 Steam enrichments, toolkit + shortlist for narrowing.

⇒ **The break is friend-side follow-through.** Ben cut the links and handed them over; they died in a
chat scroll. Nothing upstream of that needs fixing, and a giver-side curation assist would be aimed at
a step that is not failing.

## 4. The constraint that shapes the whole design

`crates/domain/src/lib.rs:192` — **"no email."** Friends have no account and no contact channel;
`shelf_token` is a bearer capability handed over out-of-band by Ben.

⇒ ***The app cannot knock. Only Ben can.*** This spec does not add contact collection — that would
trade the product's no-account warmth for a mailing list, and the anti-reference list forbids exactly
that kind of drift. **The app supplies the window and the hand; the arm is Ben's.**

## 5. What already exists — corrected 2026-09-28 after reading the code, not the spec's memory

🔴 **§5a of this spec's first draft proposed "a porch — a PULL surface listing open zero-claim links."
IT ALREADY EXISTS.** `admin-api/src/scrapbook.rs` composes `doors_open: Vec<ScrapbookDoor>`
(`{link_token, link_label, recipient, claims_left, created_at}`), and `web/src/admin/Scrapbook.tsx:231`
renders it under its own heading **"doors left open"**, one line per door, recipient named.

**The window is built. Cutting §5a was the fourth time this morning that an idea of mine turned out to
already be in this repo** — and it is the reason this section now leads with what exists.

⇒ **The doorstep's opening sentence, inverted.** It said: *"the house has no window that looks at the
step and no hand that can pick it up."* Here: ***there IS a window. There is no hand.***

`Scrapbook.tsx:242` — the entire door row:

```tsx
<p key={d.link_token} className="text-sm text-ink-soft">
  {clauses.join(' · ')}   // "the door ben left open for sam · open 0 years · 5 claims left"
</p>
```

A `<p>`. Ben can read that eleven gifts are waiting and **do nothing about any of them from there.**

## 5b. 🔴 A DEFECT FOUND ON THE WAY: "door" means two different sets in two surfaces

| surface | predicate | includes a partly-claimed link? |
|---|---|---|
| **scrapbook** `doors_open` | `Link::is_open_door(now)` = `can_claim(now).is_ok()` (`domain/src/lib.rs:407`) | **YES** |
| **lantern** 🚪doors room | `can_claim(now).is_ok()` **and `claims_used == 0`** (`spec-lantern.md`) | **NO** |

**One product, one word, two populations.** The 6/15 link (9 slots waiting) is a door on the scrapbook
and not a door to the lantern. Nothing is wrong today because no lantern has counted a live door yet —
**`doors: 1` on 09-27 is the spread's first observable, not its cause.**

⇒ **This answers Q4 from the code rather than from taste: the porch inherits `is_open_door`**, because
that is what the existing window already shows and a hand that acts on a *different* set than the window
displays is a trap. **The lantern/scrapbook divergence is filed as its own issue, not fixed here** —
reconciling two definitions is a change to a live weekly register and does not belong inside a feature.

## 5c. The hand — *knock again* and *close the door*

Two actions on each existing door row, and the second is what makes the first honest:

1. **knock again** — copy a ready-to-send message carrying the link. Ben pastes it into the chat it
   died in. **The app composes; Ben sends.** (§4: there is no channel to the friend, and this spec does
   not add one.)
2. **close the door** — `POST /admin/api/links/{token}/revoke`, **which already exists**
   (`admin-api/src/lib.rs`, `Link::revoked`, `ClaimRefusal::Revoked`). The row leaves the window.

**Q2 is answered against my own alternative:** revoke **REMOVEs** the `shelf_token` attribute under the
stated doctrine *"no dead capability at rest"* (`domain/src/lib.rs:198`). ⇒ **a new `retired` state that
hid the row but left the token live would be a dead capability at rest by another name.** Use revoke;
make the copy honest — *"this takes the gift back — the link stops working."*

**A list that can be EMPTIED is not a nag.** Every door now has a resolution, so the window converges.

## 5d. The voice — the lantern stops spending its one line

Replace the one-shot backlog line with a **standing doors line** naming the count and pointing at the
window. **It is clearable** — which is exactly why it may stand where the old line had to be one-shot.
*(Q1: still open to family. This is the half I am least sure of.)*

## 6. Criterion ⑥ — the fire-rate floor

The surfacing engine (`plans/2026-08-17-surfacing-engine-phase-1.md`) was **retracted before execution**
because its flagship reason had no production path — *"correct and silent forever."* Criterion ⑥ (fire-
rate floor) exists because of it.

**This spec clears ⑥ by construction, not by fixture: 11 qualifying rows exist in production today, and
the surface renders all 11 on its first load.** The failure mode that killed the engine — a predicate
that can never fire — is not reachable here; the risk runs the *other* way (the porch starts full).

## 7. Open questions for family review

> **Q2 and Q4 are now ANSWERED FROM THE CODE** (§5b, §5c) and kept below as the record of what was
> asked. **Q1 and Q3 remain genuinely open.**

- **Q1 — push dose.** §5c proposes a standing weekly line. Is a *clearable* standing line genuinely
  outside the nagging rule, or does any every-tick line rebuild the silent-loop the 2026-07-29 ruling
  named? Alternative: porch exists, lantern says nothing, Ben finds it on pull.
- **Q2 — the hand's teeth.** Is **close the door** a revoke (existing capability) or a new *retired*
  state? Revoke is honest but reads punitive for a gift; retired needs new state.
- **Q3 — token on re-knock.** Re-sending the same `shelf_token` is simplest. Does a months-old bearer
  token that has been sitting in a chat log want rotation on re-knock, or is that ceremony with no
  threat behind it?
- **Q4 — scope of the porch.** Zero-claim links only, or any link with slots remaining? The 6/15 link
  has 9 slots waiting and is *not* zero-claim — it is partially walked. Does it belong?
