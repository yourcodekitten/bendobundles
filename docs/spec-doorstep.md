# The Doorstep 🚪

*A parcel nobody collected. It has been on the step for eighty-one days, and the house has no window
that looks at the step and no hand that can pick it up.*

**Status:** spec · 2026-09-25 · author: code kitten
**Closes:** #234 (the surface half) · #236 (the compensate half)
**Register family:** sibling of the lantern (`docs/spec-lantern.md`), which already tells Ben a Pending
claim past the 24h bar *"clears when compensated or fulfilled — no admin button for that yet."*
**This spec is that button, and the window beside it.**

---

## 1. The specimen, and why it is still here

Claim `3da0c011-aa45-4159-b204-714fae91bd19`
(`qVRK4Ce8rmmxSpsS:soulcalibur6_monthly_steam`, `LINK#SELF`) has been `Pending` since
**2026-07-06T22:54:08Z** — **81 days** as of this spec.

A friend (here, Ben himself via a self-claim) reached for a key and the claim never resolved. Nothing
in the product shows it. Nothing in the product can clear it.

## 2. What is NOT broken — measured 2026-09-25, not assumed

Everything upstream of *audience* works, and three of the four things #234/#236 describe have since
been **fixed by the switchboard arc** (merged `9e2a465e4e7c`, 2026-09-23) without either issue being
updated. **An issue is a claim with a timestamp.**

| claim (as filed) | measured 2026-09-25 | verdict |
|---|---|---|
| the watchdog does not see self-claims | `pending_age_sweep` (`fulfillment/src/lib.rs:4438`) runs first in every `run_sync`; prod shows `age_days` firing | **not broken** |
| `ping_msg` returns silently on a non-Webhook notify | now `deps.notify.sendable(Register::Ops)` (`lib.rs:4956`); dark registers emit `outcome="register_dark"` (`:630`), test `ping_msg_on_a_dark_ops_register_says_so` | **fixed by the switchboard** |
| `WHISPER_DISABLED` darks the bell (#236, part b) | `main.rs:182` `Notify::resolve(whisper_read, bell_disabled)`; `bell.rs:110` *"This used to be TWO returns"*; `lib.rs:742` | **fixed by the switchboard** |
| the delivery split is unresolved and needs a stronger seat | resolved below | **resolved** |

### 2.1 The delivery split, resolved

#234 left one fork open since 2026-09-14, noting `kitten-debug` could not settle it
(`kms:Decrypt` explicit deny) and that *"Ben, or a deploy-role seat, resolves this."*

**Resolved from the `kitten-deploy` seat, 2026-09-25:**

- The function's env holds **9 variables and no `*_DISABLED` flag at all**
  (`DISCORD_WEBHOOK_PARAM`, `HUMBLE_{COOKIE,PASSWORD,TOTP}_PARAM`, `HUMBLE_USERNAME`,
  `STEAM_KEY_PARAM`, `TABLE_NAME`, `WHISPER_SITE_URL`, `WHISPER_WEBHOOK_PARAM`)
  ⇒ the disable-by-flag cells of `Notify::resolve` are not in play.
- Prod logs, 3-day window, **with a control**: invocations **2** (so the window is not empty) ·
  `register_dark` **0** · `age_days` **1** · `operator_notification_failed` **0**.

⇒ **The ops register is live, the ping is emitted, the POST succeeds, and no human reads the
destination.** Branch 1. The switchboard gave the ping a voice; **a voice is not an audience.**

> ⚠️ **Deliberately NOT chased: which room the webhook points at.** `main.rs` already says why —
> *"SEPARATE PARAM, not verified-separate DESTINATION … **Different secret is not different room** …
> the comparison belongs in an operator's hands, not in a log line."* Comparing two SecureStrings to
> find out who listens is Ben's read, not this spec's. **Which is exactly why the fix is a surface.**

## 3. Intent

**A stuck claim must be visible to someone who never received a notification, and clearable by
someone who did not write the code.**

Two deliverables. Both are in the product, not in the alerting.

### A · The window — stuck claims on `/admin/ops`

A section listing every `Pending` claim past the same 24h bar the sweep uses
(`RECONCILE_STUCK_ALERT_AGE`), one row each:

- age (days, and the `Pending` timestamp)
- claim id, game/item name
- **self-claim vs friend-claim**, because the remedy differs and the row must not hide it
- the link token when it is a friend claim (`LINK#SELF` when it is not)

**Why a surface and not a better ping:** a surface is *pulled*, so it cannot be lost to an unread
destination, a muted room, or a webhook pointed at the wrong place. The ping stays exactly as it is —
this does not touch `ping_msg`.

### B · The hand — a compensate action

`Store::compensate_self_claim(claim_id, game_id)` already exists (`crates/dynamo/src/lib.rs:2254`) and
sets `ClaimState::Compensated`. **Nothing in `admin-api` or `web` can invoke it** (measured
2026-09-16, still true). This adds:

- an `admin-api` op taking a claim id, resolving the game id, invoking the compensate path
- a **confirm step** in the web UI beside the stuck row — a two-step button, never a bare click
- tests for **both** arms: the self-claim path *and* the friend-claim path, because
  `compensate_self_claim` is keyed on `domain::SELF_LINK_TOKEN` and a friend claim needs
  `compensate_claim` (`crates/dynamo/src/lib.rs`, captured at `iam_capture.rs:894`)

## 4. Open question for the family (step 2 takes this to OMBB + Lilith)

**Does `admin-api` call the store directly, or invoke fulfillment with a new `Compensate` request?**

- **Direct** — `admin-api` already holds `Arc<Store>` (`crates/admin-api/src/lib.rs:72`), so the call
  compiles today. Simplest, one hop, no new wire format.
- **Via fulfillment** — what #236 proposes. `admin-api` already has `AdminInvoker` with both
  fire-and-forget and blocking invokes, and `FulfillRequest` is the established ops verb surface.

**The discriminator is IAM, and the repo already owns the instrument:**
`crates/dynamo/tests/iam_capture.rs:898` captures `compensate_self_claim`'s required actions. The
question is whether **`admin-api`'s role** is granted them, or only `fulfillment`'s. If only
fulfillment's, the direct call compiles and fails in prod — strictly worse than an invoke.
**Resolve by reading the capture against the terraform grants; do not choose on elegance.**

## 5. Non-goals

- **Not** a reaper. Nothing here auto-compensates on a timer. A stuck claim is a real person's key;
  the hand is deliberate and human-pressed.
- **Not** the disposition of the 81-day specimen. **That is Ben's call** (#234 says so). This ships
  the button; it does not press it.
- **Not** touching `ping_msg`, the switchboard, or any webhook destination.
- **Not** a new alarm. The lantern and the ops ping already fire; the gap is audience, not emission.

## 6. Test plan (red first)

1. **The window is empty when it should be** — a store with no stale Pending renders the empty state,
   and the empty state says *"no stuck claims"*, never a bare blank (a vacuous green is the defect
   this whole register family exists to remove).
2. **The window shows a planted stale claim** — construct one past the bar; assert the row, its age,
   and its self-vs-friend label.
3. **A claim one second inside the bar is NOT listed** — the boundary, both sides.
4. **The hand, self-claim arm** — compensate moves `Pending → Compensated`; assert the game row too.
5. **The hand, friend-claim arm** — asserts the *correct* function is chosen, not that it merely
   succeeds.
6. **Confirm step** — the op refuses without the confirm token.
7. **Authorization** — an unauthenticated caller gets no window and no hand.

## 7. Issue bookkeeping this spec owes

- **#234** — record the resolved discriminator and that two of its three sub-findings were fixed by
  the switchboard; keep it open for the surface, which this closes.
- **#236** — close **part b** (bell's own flag) as already fixed, with the source evidence.
