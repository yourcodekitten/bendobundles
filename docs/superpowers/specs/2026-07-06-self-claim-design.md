# Self-claim (admin key reveal) — design

**Status:** approved by Ben 2026-07-06 (brainstormed via Discord, all sections approved)
**Author:** kitten
**Depends on:** the shipped gift/claim machinery — `fulfill_claim` durable-first write, park/reconcile,
the phase-3 Choice orchestration (`choose_content` → redeem, intent snapshot, PRs #24–#41), and the
humble redeemkey endpoint already documented in `2026-07-05-humble-choice-design.md` §2.

## 1. Goal

Ben can claim a game **for himself** from the admin catalog — no gift link, no gift-URL dance. The
system reveals the **actual key string** (Steam key, GOG code, …) in the admin UI, with a one-click
"redeem on Steam" button for steam keys (`https://store.steampowered.com/account/registerkey?key=…`).

Scope (Ben, 2026-07-06): **everything unclaimed is eligible** — including non-giftable keys (self-claim
is *more* permissive than gifting: a key Humble won't gift can still be revealed for its owner) and
`requires_choice` games (which spend a real monthly pick, behind a loud confirm). All key types get the
reveal treatment; only `key_type == "steam"` gets the easy redeem button.

Non-goals: bulk self-claim (one game per click), any auto-claiming without an explicit admin action.

## 2. The humble layer — already proven

The choice design doc §2 captured both writes from live HARs, including the **self-claim variants**:

- Reveal an already-minted key: `POST /humbler/redeemkey` with `gift` **omitted** →
  `{success:true, key:<keystring>}`. This is `redeem_as_gift`'s sibling with the gift flag off and a
  different success payload.
- Choice self-claim: `POST /humbler/choosecontent` with `is_gift` **omitted** (spends the pick), then
  the redeemkey call above.

So the genuinely new humble-client surface is **one method**:

```rust
pub async fn reveal_key(&self, keytype: &str, key: &str, keyindex: u32, allow_heal: bool)
    -> Result<RevealedKey, HumbleError>
```

Same `csrf_write` builder, CF-bypass, secure-area step-up and self-heal as `redeem_as_gift`; parses
`{success, key}` instead of `{success, giftkey}`. Exhaustive error enum arms, no `_` catch-all,
wiremock-tested against recorded fixtures.

## 3. Domain / storage

- **Claims machinery reused wholesale.** Self-claims are Claim records under a reserved partition:
  `pk=LINK#SELF`, `sk=CLAIM#{id}` — no schema migration, the existing claim read/write paths work
  unchanged. `LINK#SELF` has no Link META item; code treats the token `SELF` as reserved (public-api
  already 404s unknown tokens, so `/l/SELF` stays a 404 — verify with a test).
- **New Claim field** `revealed_key: Option<String>` — written **durable-first** exactly like
  `gift_url` is today (key lands in the CLAIM item before the GAME flips), so a crash after reveal
  never loses a key. Re-viewable later from the claim record — not show-once.
- **Game terminal status:** the existing `BenRedeemed` variant. Intake flips
  `available→pending` with the same conditional-transaction discipline (no link counter — the
  transaction is GAME + CLAIM only); success lands `ben_redeemed`.

## 4. Fulfillment orchestration

New `FulfillRequest::SelfClaim { claim_id, game_id, gamekey, machine_name, keyindex, requires_choice }`.

Two paths, one decision tree:

- **Bundle game** (`requires_choice=false`): `reveal_key(...)` → Record (write `revealed_key`, flip
  game to `ben_redeemed`) | AlreadyRedeemed → Compensate | Unauthorized → ParkCookieDead | else → Park.
  Mirrors `gift_decision` exactly; pure decision function, no `_` arm.
- **Choice game** (`requires_choice=true`): the phase-3 two-write orchestration with a **reveal
  terminal instead of a gift terminal** — intent snapshot (`choice_pre_tpks`) before choose,
  `choose_content(gamekey, [machine], is_gift=false)`, ambiguous choose outcomes **Park never
  re-choose**, then `reveal_key` on the post-choose tpk (`find_new_tpk` mapping as shipped).
  Reconcile handles a parked self-claim the same way it handles a parked gift: redeem/reveal the
  now-present key, never re-choose. No new money-safety logic — a new terminal on the proven machine.

Safety invariants inherited verbatim: pick spent exactly once; key burned exactly once; a burned key's
value is never lost (durable-first `revealed_key`); reconcile never chooses.

## 5. API + admin UI

- `POST /admin/api/games/:id/self-claim` (session-guarded) — synchronous like the public claim:
  200 `{revealed_key, key_type}` | 202 `{status:"processing"}` (parked; key appears on the claim record
  when reconcile completes) | 409/410 `{error}` (race/already-claimed).
- `GET /admin/api/claims/self` — list self-claims (state, game, revealed_key) so keys stay findable.
- **Catalog UI:** a "claim for me" action on every `available` game. Confirm dialog; when
  `requires_choice`, a loud extra warning ("this spends 1 monthly pick"). On success: key in a
  selectable copy box + **"redeem on Steam"** button for steam keys; gog/origin/drm-free get key +
  copy only. On 202: "processing — key will appear under self-claims." Failure surfaced plainly.
- `AdminClaimView` gains `revealed_key` **only** on the self-claims surface (`LINK#SELF`); gift-link
  claim views keep hiding fulfillment secrets exactly as today.

## 6. Rollout

1. `reveal_key` in humble-client (wiremock).
2. Domain field + `LINK#SELF` intake/fulfill/compensate paths (moto).
3. Fulfillment SelfClaim orchestration incl. the choice path + reconcile (moto, merge-gate style test
   asserting reconcile never calls choosecontent).
4. Admin API + UI.
5. Live receipt: one Ben-authorized self-claim of a bundle game, then one choice game.

## 7. Verification

Wiremock fixtures for `reveal_key` (success, already-redeemed, unauthorized/step-up). Moto tests:
intake race (two self-claims, one wins), durable-first ordering (crash between claim-write and
game-flip leaves the key recorded), choice-path park/reconcile (spent-pick-unrevealed reconciles by
revealing, never re-choosing), `/l/SELF` is a 404. Live receipts per §6.5 before trusting unattended.
