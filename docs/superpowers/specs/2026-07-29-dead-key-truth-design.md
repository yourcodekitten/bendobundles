# dead-key truth — refusal taxonomy, terminal claim state, and the end of silent retry loops

status: DRAFT for family review (OMBB + lilith), 2026-07-29
author: code kitten
prod receipts: cloudwatch `/aws/lambda/brd-prod-ue1-bendobundles-fulfillment`, pending-claims GSI,
2026-07-29 morning (queries preserved in code-kitten/state/checkpoint.md)

## Problem

Humble redeem refusals carry *classes*, and the system flattens them all into one
retry-forever park with no operator signal and no honest friend-facing outcome.
Three claims have been stuck PENDING in a silent daily reconcile loop for ~3 weeks:

| claim | game | link | created | humble's refusal | loop shape |
|---|---|---|---|---|---|
| `87b9a4d8` | doom_eternal (choice) | **FRIEND** | 07-09 | "This key has expired and can no longer be redeemed." | choice reconcile B2: pick spent, key present+unredeemed → re-redeem daily → refused, forever |
| `3f46c058` | mylittleuniverse (choice) | SELF | 07-06 | "Keys are temporarily exhausted for this product" | choice B2 self: re-reveal daily → refused |
| `3da0c011` | soulcalibur6 (monthly) | SELF | 07-06 | "Keys are temporarily exhausted" + intermittent `RedeemAuthRejected 403` | self-claim plan B1: not-redeemed → re-reveal daily → refused |

A fourth refusal class already behaves correctly by accident: diabloiv's "Your
Battle.net grant is still processing. Please try again in a moment." parked, retried,
and completed — transient refusals are exactly what park-for-reconcile was built for.
The design gap is that *terminal* refusals ride the same rail.

### Root gaps (verified on code + prod logs)

1. **`HumbleError::RedeemRefused(String)` is untyped.** `gift_error_decision`
   (fulfillment `lib.rs:191`) maps every refusal → `Decision::Park`. Expired
   (terminal-forever), keys-exhausted (retryable-long), and grant-processing
   (transient) are indistinguishable downstream.
2. **RedeemRefused parks are born silent.** All three Park executors (`handle_gift`
   ~:688, `redeem_claimed_tpk` ~:1134, `reveal_claimed_tpk` ~:1239) ping only for
   `SecureAreaStepUpFailed` and `RedeemAuthRejected`. A refused park produces a
   CloudWatch warn and nothing else — at claim time AND on every reconcile pass.
3. **Retry loops have no age escalation.** `alert_unreconcilable` (+
   `RECONCILE_STUCK_ALERT_AGE = 24h`) covers only *structurally* unreconcilable
   claims (unsplittable game_id / machine_name not in order keys). A claim that
   reconcile CAN act on — and acts on, failingly, every day — never qualifies.
   All three stuck claims are in this blind spot.
4. **No terminal-failure ClaimState.** `ClaimState = {Pending, Fulfilled,
   Compensated}` (domain `lib.rs:29-35`). The friend sees the amber "processing"
   chip forever (`ClaimsHistory.tsx` STATE_CHIP; claim POST 202 → "processing…
   check this page later"; no polling, no failed UI). Admin link-audit shows the
   same three states; a dead claim is indistinguishable from a live one.
5. **Shelf truth exists for bundle keys but not choice keys.** Sync maps the
   wire's per-tpk `is_expired` → `KeyEntry.expired` → `GameStatus::Expired`
   (unlistable). But a choice game's key doesn't exist until choose spends the
   pick — expiry is only discoverable at redeem time, after the pick is gone.
   That is the doom trap, and it will recur on any vintage choice month.

## What already exists and gets REUSED (not rebuilt)

- `GameStatus::Expired` + the `as_wire`-tracks-serde discipline
  (domain `lib.rs:5-27`, pinned by `wire_strings_track_serde_for_all_mirrored_enums`).
- The compensate transaction shape: claim → terminal + game flip + guarded
  `ADD claims_used -1` (dynamo `lib.rs:1241-1302`); the SELF variant that skips the
  decrement (`:1375` — no link-meta item in the SELF partition).
- The typed-refusal routing precedent: `redeem_once` already routes
  "already redeemed" message text → `HumbleError::AlreadyRedeemed`
  (humble-client `lib.rs:1191-1198`).
- The unparsed refusal `error` CODE field: live capture shows
  `{"error":"keys_depleted_email","error_msg":"Keys are temporarily exhausted…"}`
  (client_test.rs:1370) — machine-readable, currently dropped.
- `alert_unreconcilable`'s age-gated ping pattern (`:3256-3287`).
- `FulfillResponse::AlreadyRedeemed` → HTTP 410 "pick another" mapping
  (public-api `:692-697`) — the model for a friend-honest terminal response.

## Design

### 1. Typed dead-key detection (humble-client)

Detection ladder, most-reliable first:

1. **Structural:** a tpk with `expired == true` is dead — no redeem call needed.
   Reconcile B2 / `claimed_tpk_terminal` checks `tpk.expired` BEFORE attempting
   the redeem. (Bundle sync already honors this flag at listing time; the redeem
   paths currently ignore it.)
2. **Coded:** parse the refusal body's `error` field (today dropped). A known
   terminal code → typed variant. Known codes registry starts with what we have
   receipts for; unknown codes fall through.
3. **String:** exact-match "This key has expired and can no longer be redeemed."
   → `HumbleError::KeyExpired`. Mirrors the existing already-redeemed routing.

Everything unrecognized stays `RedeemRefused(String)` — byte-for-byte today's
behavior. Conservative by construction: an unknown refusal parks and retries,
exactly as now.

`KeysExhausted` stays UNTYPED (plain `RedeemRefused`): retry is the correct
behavior (humble restocks); its problem is silence, fixed by §3. (Family input
welcome — typing it buys only nicer ping copy today.)

The crates' own exhaustive-match discipline (no `_` arms on `HumbleError`) forces
conscious classification of the new variant at every decision site before
anything compiles.

### 2. `Decision::DeadKey` → terminal claim state

- New `ClaimState::Failed`, wire `"failed"` (generic name on purpose — future
  terminal failures reuse it; the *reason* lives in the ping + logs, not the enum).
- New dynamo transaction `fail_claim_dead_key` (sibling of `compensate_claim`):
  - claim → `Failed` (consumes the `gsi2pk` pending marker — leaves the GSI), and
    **persists the failure reason on the claim item** (the matched code or string that
    fired) — pings scroll away and CloudWatch has retention; the claim record is the
    durable truth a future admin surface reads (lilith's rider, 2026-07-29),
  - game → `GameStatus::Expired` (NOT re-listed; reuse, no new status; unlistable
    by the existing `is_listable` rule, counted by the existing Ops bucket),
  - `ADD claims_used -1` guarded (the friend's slot returns — same guard rails as
    compensate); SELF variant skips the decrement, mirroring `compensate_self_claim`.
  - Same Pending-lock guards as compensate on both items; idempotency arms match.
- Flavors: **choice** — the pick is already spent and the key is dead; nothing is
  recoverable; the ping says the pick is stranded, plainly. **bundle** — no pick
  involved; the game simply retires.
- **One ping at the transition**: claim id, game, humble's exact words, what
  happened to the slot. (Distinct from the escalation pings of §3 — this is a
  terminal event, not a nag.)
- public-api: at claim time a DeadKey outcome → HTTP 410 with its own message
  ("that key can't be redeemed anymore — pick another"); reuses the AlreadyRedeemed
  response pattern. A `failed` claim in the history endpoint just serializes its
  state like the rest.
- web (friend): `STATE_CHIP` gains `failed` — soft, brand voice, no error-red
  panic: the game bowed out, your pick came back. ClaimDialog handles the 410-class
  response with the same warmth ("whoops" heading pattern already exists).
- web (admin): link-audit badge for `failed`; Ops `game_counts` already buckets
  `expired` so the retired game is visible with zero new admin surface.

### 3. Pending-age escalation — a SET-driven sweep (revised per lilith's review)

New invariant, enforced on the SET, not on pass outcomes:
**every claim in the pending-claims GSI older than `RECONCILE_STUCK_ALERT_AGE`
(24h) pings, every sync, full stop.**

- The sweep runs over `list_pending_claims()` output BEFORE the reconcile pass
  (so a dead-session early-return, a future claim shape reconcile can't touch,
  or reconcile itself breaking can never starve the invariant — the original
  per-pass-touched-claim framing was alert_unreconcilable's classification
  blind spot reborn one layer up, as coverage).
- Ping names game, claim id, age in days. When the same run's reconcile pass
  produces an outcome class for that claim, logs carry it; when reconcile never
  reaches the claim at all, that absence is itself the loudest line.
- Cadence: once per claim per sync = daily. A once-ever marker recreates the
  exact failure this spec kills (alert fires once, gets missed, silence
  forever). Dedup arrives the day volume actually hurts, not before.
- This covers the exhausted pair, intermittent 403s, reconcile regressions, and
  every FUTURE unknown refusal class — the escape hatch for everything §1
  declines to type.
- `alert_unreconcilable` remains for structural specifics (its "fix by hand"
  guidance is more precise); a structurally-stuck claim thus pings twice per
  sync — accepted at this scale, noted for a later dedup fold.

### 4. The three stuck claims are the acceptance test

Post-deploy, the next sync should — with NO hand-surgery:

- `87b9a4d8` (doom): reconcile B2 sees `tpk.expired` (or the redeem returns the
  expired string) → `Decision::DeadKey` → claim `failed`, game `expired`, friend
  slot returned, ONE ping with the receipts. The daily
  `reconcile(choice): terminal did not complete` log pair stops.
- `3f46c058` + `3da0c011` (exhausted): remain pending BY DESIGN, but now fire
  daily escalation pings (age ≫ 24h) → ben decides: wait for restock or
  hand-compensate. (An admin "compensate this claim" button is deliberately NOT
  in scope — if ben wants the exhausted pair freed, that's dynamo surgery today
  and a follow-up issue.)
- pending-claims GSI count: 3 → 2.

## Non-goals

- Lost-months discovery hardening (#53) + the 26-page walk truncation — separate
  arc, deliberately sequenced AFTER this one (discovering more vintage months
  without dead-key truth would mint more doom-shaped traps).
- #35 (resume-redeem snapshot marker) — own design, unchanged by this work.
- Admin compensate/free-slot button — follow-up issue if wanted.
- Probing shelf keys by redeeming them (spends keys), or any humble-side
  inventory magic: we cannot un-expire a key.

## Verification plan (deploy)

- Full deploy (all three lambdas rebuild; web only if the friend/admin UI changes
  ship together — they do) per terraform/README "Deploying as kitten".
- Same-day sync via admin `POST /admin/api/sync` (sync-now; kitten-deploy has no
  lambda:Invoke and needs none).
- Watch: the doom claim transitions end-to-end live; escalation pings fire for
  the exhausted pair; GSI count drops; friend link page for the doom claim shows
  the soft terminal state; CloudWatch loop-pair gone.

## Family review outcomes (2026-07-29, shared channel)

lilith (msgs 1531984906660479039/1531984907386224810): all five positions
endorsed; two riders folded above — failure-reason persisted on the claim item
(§2) and escalation as a GSI age sweep, set-driven (§3, rewritten). OMBB review
in flight; plan gate his as always.

## Open questions (family review)

1. **Refusal `error`-code registry vs string-only:** the exhausted fixture proves
   the code field exists (`keys_depleted_email`); we have NO captured code for the
   expired case (only the string). Parse-and-log the code always (observability),
   classify on it when known, string-fallback otherwise — agreed?
2. **`ClaimState::Failed` naming** — generic `failed` vs specific `key_dead`?
   (I hold: generic state + specific reason in ping/log.)
3. **Slot auto-return on DeadKey** — instant (compensate-like guards, friend-kind)
   vs ben-approves-first? (I hold: instant; the guard rails are identical to
   compensate and the friend did nothing wrong.)
4. **Escalation cadence** — per-pass/daily (matches existing, zero new state) vs
   once-ever (needs a durable marker on the claim)? (I hold: daily.)
5. **B3-Gift already-redeemed human-recovery ping** — today it re-pings every
   pass; fold into the same escalation grammar or leave untouched? (I hold:
   leave untouched this arc; it's a different failure with a real recovery path.)
