# Self-claim (admin key reveal) — design

**Status:** approved by Ben 2026-07-06; revised same day after spec review (B1/M1–M4 + minors
addressed; hidden-eligible + record-on-AlreadyRedeemed confirmed by Ben)
**Author:** kitten
**Depends on:** the shipped gift/claim machinery — durable-first claim writes, park/reconcile,
the phase-3 Choice orchestration (`choose_content` → redeem, intent snapshot, PRs #24–#41), and the
humble redeemkey endpoint documented in `2026-07-05-humble-choice-design.md` §2.

## 1. Goal

Ben can claim a game **for himself** from the admin catalog — no gift link, no gift-URL dance. The
system reveals the **actual key string** (Steam key, GOG code, …) in the admin UI, with a one-click
"redeem on Steam" button for steam keys (`https://store.steampowered.com/account/registerkey?key=…`).

Scope (Ben, 2026-07-06): **everything unclaimed is eligible** — including non-giftable keys
(self-claim is *more* permissive than gifting), **including hidden games** (hidden means "don't show
friends"; the admin is not a friend — confirmed by Ben 2026-07-06), and `requires_choice` games
(which spend a real monthly pick, behind a loud confirm). All key types get the reveal treatment;
only `key_type == "steam"` gets the easy redeem button.

Non-goals: bulk self-claim (one game per click), any auto-claiming without an explicit admin action.

## 2. The humble layer

The choice design doc §2 captured the self-claim variants from live HARs **in the Choice flow**:

- Reveal a key: `POST /humbler/redeemkey` with `gift` **omitted** → `{success:true, key:<keystring>}`
  (captured with `keytype=<machine>_choice_steam`).
- Choice self-claim: `POST /humbler/choosecontent` with `is_gift` **omitted** (spends the pick),
  then the redeemkey call above.

**Assumption, stated honestly:** the `{success, key}` response shape for *plain bundle* keys — and
especially for *non-giftable* keys (non-giftable for a reason humble knows and we don't) — is
extrapolated from the Choice capture, not proven. The repo holds no HAR of either. Mitigation: the
first live receipts (§6.5) include a plain bundle key AND a non-giftable key, before the feature is
trusted unattended.

New humble-client method, mirroring its sibling's exact signature discipline (domain-named params,
**no heal flag** — heal is the fulfillment-layer ladder's job, as with `redeem_as_gift`):

```rust
pub async fn reveal_key(&self, gamekey: &str, machine_name: &str, keyindex: u32)
    -> Result<RevealedKey, HumbleError>   // RevealedKey { key: String, key_type: String }
```

Same `csrf_write` builder, CF-bypass, and secure-area step-up handling as `redeem_as_gift`; parses
`{success, key}` instead of `{success, giftkey}`. Exhaustive error enum arms, no `_` catch-all,
wiremock-tested. Its `Unauthorized` outcome satisfies the heal-ladder membership rule the same way
the gift redeem does (the login interstitial answers before the redeem handler touches the key), so
fulfillment may ride it on the heal-then-retry-once ladder.

## 3. Storage — the three writes, inventoried

Self-claims are Claim records under a reserved partition: `pk=LINK#SELF`, `sk=CLAIM#{id}`, with
**no LINK META item**. The token `SELF` is reserved; `/api/l/SELF` already 404s (link lookup finds
no META — keep a test pinning this). The claims *read* paths work unchanged, but **all three
LINK-META-coupled store writes need explicit treatment** — the META item is load-bearing in two:

1. **Intake — new `claim_game_self` (two-item transaction).** GAME `available→pending` + stamp
   `claim_id`, CLAIM put. GAME condition is `#st = :available` **alone** — NOT the gift path's
   `#st = :available AND attribute_exists(gsi1pk)`. `gsi1pk` exists only when
   available ∧ giftable ∧ ¬hidden (the sparse listable marker; its `attribute_exists` gate is the
   friend-claim hide-race TOCTOU guard). Reusing it would reject exactly the non-giftable and hidden
   games §1 includes. No LINK counter item (no budget to enforce). The status condition alone still
   makes gift-vs-self and self-vs-self races single-winner.
2. **Fulfill — new `fulfill_self_claim`,** structurally `fulfill_claim` with `revealed_key` in place
   of `gift_url`: write the key to the CLAIM item **durable-first**, then flip the GAME to
   `ben_redeemed` asserting `claim_id` ownership. (The existing `fulfill_claim` is *not* reused
   as-is: different field, different terminal status.)
3. **Compensate — new `compensate_self_claim` (two-item transaction).** CLAIM → compensated
   (conditioned on the pending marker), GAME re-list (conditioned `#st = :pending`) — and **no LINK
   decrement**. The gift `compensate_claim`'s third item decrements LINK META guarded
   `claims_used >= 1`; against the absent `LINK#SELF` META that condition fails and cancels the
   whole transaction — every self-claim compensation would wedge permanently. Hence the variant.

Domain: new Claim field `revealed_key: Option<String>` (skip-serializing None — old records
deserialize unchanged). Game terminal status: the existing `BenRedeemed` variant.

## 4. Fulfillment orchestration

New `FulfillRequest::SelfClaim { claim_id, game_id, gamekey, machine_name, keyindex,
requires_choice }` and a key-bearing success variant `FulfillResponse::RevealedKey { key, key_type }`
(today's enum has no such variant; `gift_decision` is typed over `Result<GiftUrl, _>`). New pure
decision fn `reveal_decision(&Result<RevealedKey, HumbleError>) -> Decision` (or `gift_decision`
genericized over the success type — implementer's pick), reusing the existing `Decision` enum:

- **Ok → Record**: `fulfill_self_claim` (durable-first key write, flip `ben_redeemed`).
- **AlreadyRedeemed → Recover-then-Record** (Ben's call, 2026-07-06 — NOT the gift path's
  Compensate): for a self-claim, "already redeemed" means the key was already revealed to Ben's
  account and its value sits in the order's `all_tpks[].redeemed_key_val`. Compensating would
  re-list a game whose key is burned-to-Ben and record nothing. Instead: re-read the order, find the
  tpk by `machine_name`, record `redeemed_key_val` via `fulfill_self_claim`. **Fallback:** if the
  re-read shows redeemed but carries no `redeemed_key_val` (the choice doc §7 notes gift-redeems may
  not set it — e.g. the key was actually gifted away out-of-band), **Park + ping** for a human; never
  guess, never compensate blind.
- **Unauthorized → ParkCookieDead**, **everything ambiguous → Park** — verbatim gift semantics.

**Choice path** (`requires_choice=true`): the phase-3 two-write orchestration with a reveal terminal —
intent snapshot (`choice_pre_tpks`) durable before choose, `choose_content(gamekey, [machine],
is_gift=false)`, ambiguous choose outcomes Park never re-choose, then `reveal_key` on the post-choose
tpk (`find_new_tpk` mapping as shipped).

**Reconcile — discriminator and per-branch terminals.** Today every reconcile terminal is
gift-shaped. A parked self-claim lands in the same `PENDINGCLAIM` GSI, so reconcile discriminates on
`claim.link_token == "SELF"` and routes to reveal-side terminals:

| Branch (existing semantics) | Gift terminal today | SELF terminal |
|---|---|---|
| bundle: order shows tpk unredeemed | `redeem_as_gift` → record gift_url | `reveal_key` → `fulfill_self_claim` |
| bundle: tpk already redeemed | ping human (gift URL unrecoverable) | record `redeemed_key_val` if present; else ping (fallback above) |
| bundle: provably never redeemed | `compensate_claim` | `compensate_self_claim` |
| choice A: no intent snapshot | compensate (no pick spent) | `compensate_self_claim` |
| choice B1: snapshot, no new tpk | compensate (no pick spent) | `compensate_self_claim` |
| choice B2: new tpk, unredeemed | redeem from reconcile (never choose) | `reveal_key` from reconcile (never choose), heal disallowed same as B2 today |
| choice B3: new tpk, already redeemed | ping human | record `redeemed_key_val` if present; else ping |
| choice B4: ambiguous new tpks | ping human | ping human (unchanged) |

Invariants inherited verbatim: pick spent exactly once; key burned exactly once; a revealed key's
value is never lost (durable-first); **reconcile never chooses** (the merge-gate test that mounts no
choosecontent route extends to cover the SELF paths).

## 5. API + admin UI

- `POST /admin/api/games/:id/self-claim` (session-guarded) — synchronous like the public claim:
  200 `{revealed_key, key_type}` | 202 `{status:"processing"}` | 409/410 `{error}`. Requires the
  **admin-api → fulfillment RequestResponse invoker**, which admin-api does not have today (only
  public-api does) — new wiring + IAM.
- `GET /admin/api/claims/self` — list self-claims (state, game, revealed_key). Implementation note:
  this calls `claims_for_link("SELF")` **without** the link-existence pre-check that the existing
  `GET /admin/api/links/:token/claims` handler performs (`LINK#SELF` has no META; the existing
  handler would 404 — do not reuse it).
- **202 path is poll-based, deliberately** (Ben-confirmed): a parked self-claim's key appears in the
  self-claims list when reconcile completes; no push notification. The existing discord ping fires on
  the human-recovery branches.
- **Catalog UI:** "claim for me" on every `available` game (hidden included). Confirm uses the
  codebase's **two-step arm/confirm button idiom** (Links.tsx pattern — not window.confirm, not a
  dialog); when `requires_choice`, the armed state carries the loud "this spends 1 monthly pick"
  warning — which requires **exposing `requires_choice` on `CatalogGameView`** (it isn't today).
  On success: key in a selectable copy box + "redeem on Steam" button for steam keys; other key
  types get key + copy. On 202: "processing — key will appear under self-claims."
- **Secrets posture — a deliberate, scoped invariant change:** today's rule "fulfillment secrets
  never reach the admin surface" (asserted in admin-api docs and web tests) is hereby **scoped to
  gift claims**: `gift_url` stays hidden from the admin surface exactly as now; `revealed_key` is
  admin-visible by design (it's Ben's own key). Update the invariant comment and the asserting
  tests to say so explicitly. `revealed_key` must **never appear in fulfillment logs or pings**
  (same discipline as key values in reconcile pings today), and stays out of every friend-facing
  response.

## 6. Rollout

1. `reveal_key` in humble-client (wiremock: success, already-redeemed, unauthorized interstitial,
   step-up).
2. Domain field + the three store writes from §3 (moto).
3. Fulfillment SelfClaim orchestration: `reveal_decision`, recover-then-record, choice path,
   reconcile discriminator + SELF terminals (moto; merge-gate style test asserting reconcile never
   calls choosecontent on SELF paths).
4. Admin API (invoker wiring + both endpoints) + catalog UI.
5. Live receipts, in order: one plain bundle key, one **non-giftable** key (the §2 assumption
   check), one choice game. Each verified: key recorded on the claim, game `ben_redeemed`, key
   redeems on its store.

## 7. Verification

Wiremock fixtures for `reveal_key`. Moto tests: gift-vs-self race and self-vs-self race on the same
game (one winner each); non-giftable + hidden games pass intake (the §3.1 condition); durable-first
ordering (crash between key-write and game-flip leaves the key recorded); SELF compensate succeeds
with no LINK item (pinning B1's fix); recover-then-record on AlreadyRedeemed incl. the no-key-val
park fallback; choice-path park/reconcile per the §4 table (spent-pick-unrevealed reconciles by
revealing, never re-choosing); `/l/SELF` stays 404; `revealed_key` absent from all logs/pings
(grep-style assertion in the log-capture tests). Live receipts per §6.5 before trusting unattended.
