# Humble Choice support — design

**Status:** proposed (discovery complete, flow proven from live HAR captures 2026-07-05)
**Author:** kitten
**Depends on:** the shipped humble session/redeem stack — CF-bypass (wreq), secure-area step-up (#15),
self-login (#18), the `redeem_as_gift` + park/reconcile machinery.

## 1. Goal

Make **Humble Choice** games giftable through bendobundles the same way bundle keys already are: a friend
opens a claim link, fulfillment turns one of Ben's available Choice games into a gift key, and records the
gift URL durably. Secondarily, surface Ben's **unclaimed** Choice games so there's inventory to gift
(discovery found many unspent picks across 2019–2021 months).

Scope (Ben, 2026-07-05): **both gifting and self-claim** (self-claim avoids the email-lookup + clicks that
gifting-to-self costs). Non-goals (this pass): bulk-claiming the older 2019–2021 stale-pick backlog (build
the mechanism, circle back), and anything that auto-spends picks without an explicit trigger (a friend's
claim or an explicit self-claim).

## 2. What discovery proved (the flow)

Humble Choice ("humble_monthly" internally) is a subscription. Each month offers a set of games; a
subscriber **spends a pick** to claim a game, which then yields a Steam key. Key facts, all captured live:

**A Choice month IS an order.** The month carries a `gamekey`; `GET /api/v1/order/{gamekey}` (which our
client already calls) returns it with claimed keys in `tpkd_dict.all_tpks` — **byte-for-byte the same tpk
shape as a bundle key** (`machine_name`, `key_type`, `steam_app_id`, `keyindex`, `redeemed_key_val`, …).
Proven: Ben's claimed "Life is Strange: Double Exposure" sits in the `june_2026_choice` order exactly like
any bundle key.

**Listing a month's offered games:** `GET /membership/<month-url>` embeds a
`<script id="webpack-monthly-product-data">` JSON blob → `contentChoiceOptions.contentChoiceData.initial`:
`content_choices{machine_name:{title,…}}` (the offered games), `total_choices` (budget),
`contentChoicesMade`, plus siblings `gamekey`, `usesChoices`, `canRedeemGames`, `isActiveContent` — and a
`csrfToken`. One page load = offered list + state + csrf. (The paginated
`/api/v1/subscriptions/humble_monthly/subscription_products_with_gamekeys/` returns the same `game_data`
per month but is clumsier.)

**Claiming a game = TWO writes** (captured from a real self-claim + two real gifts):

1. **CHOOSE — the one genuinely new call:** `POST /humbler/choosecontent`
   `gamekey=<month>` · `parent_identifier=initial` · `chosen_identifiers[]=<machine_name>` (array) ·
   `is_gift=true` (gift; omit to self-claim) → `{success:true, force_refresh:true}`.
   **This SPENDS a monthly pick.**
2. **REDEEM — the endpoint we already implement:** `POST /humbler/redeemkey`
   `keytype=<machine_name>_choice_steam` · `key=<month gamekey>` · `keyindex=0` · `gift=true` (gift; omit
   to self-claim) → self-claim `{success:true, key:<steamkey>}`, gift `{success:true, giftkey:<token>}`.
   The gift response is the **same `{success, giftkey}` shape `redeem_as_gift` already turns into**
   `https://www.humblebundle.com/gift?key=<token>`.

(The `/api/v1/analytics/content-choice/*` POSTs around these are pure telemetry — ignored.)

State model: `content_choices` (offered) + `choices_remaining`/`total_choices` (budget) on the month;
`all_tpks` (claimed, has key) on the order. `usesChoices=false` months (Ben's newer tier) claim ALL games
("Get My Games"); `usesChoices=true` months (older) are pick-N-of-M ("Make My Choices"). `choosecontent`
works for both — the tier only changes how many picks exist.

## 3. The key architectural consequence

The "parallel redemption path" we feared **barely exists.** A Choice gift is:

```
choose_content(gamekey, [machine_name], is_gift=true)   # NEW — spends a pick
        │
        ▼
redeem_as_gift(keytype=<machine_name>_choice_steam,      # EXISTING — burns the key, returns GiftUrl
               key=gamekey, keyindex, gift=true)
```

Everything below `redeem_as_gift` is untouched: CF-bypass, secure-area step-up, the `{success,giftkey}` →
`GiftUrl` parse, and the park/reconcile discipline. The genuinely-new surface is **one humble-client
method + choice-aware discovery + the two-write orchestration in fulfillment.**

## 4. Components

### 4.1 humble-client: `choose_content`
New method:
```rust
pub async fn choose_content(&self, gamekey: &str, chosen: &[&str], is_gift: bool)
    -> Result<(), HumbleError>
```
POST `/humbler/choosecontent` via the existing `csrf_write` builder (double-submit csrf, same-origin
headers — the csrf comes from the membership blob or a preflight, same as redeem). Parse `{success}`;
non-success → a typed error. It **never touches a key** (redeem does), so choose is safe to retry on a
pre-effect failure, but see §5 — a *successful* choose has spent a pick.

Also add a read helper to list a month's offered/claimed state (parse the membership blob, or the
subscription endpoint) — feeds discovery.

### 4.2 Discovery / sync
Extend the sync walk: enumerate Choice months (subscription endpoint or membership blobs), and for each,
surface **claimable** games (`content_choices` minus already-claimed, gated by `choices_remaining`) as
giftable inventory, alongside bundle games. Claimed Choice keys already flow in through the existing
`order()` walk (they're in `all_tpks`) — so *redeeming* an already-claimed Choice game needs zero new sync.
The new part is representing the **not-yet-claimed** ones as "giftable, requires a pick."

### 4.3 domain / storage
A Choice game needs a state a bundle game doesn't have: **claimable-but-unclaimed** (no key yet; gifting it
will spend a pick) vs **claimed** (key present; gift like any bundle key). Proposal: extend the `Game`
model with a `source`/`claim` descriptor — enough for fulfillment to know "to gift this I must
choosecontent first." Exact shape is an open question (§7).

### 4.4 fulfillment orchestration (the two-write gift)
When a friend claims a link for a not-yet-claimed Choice game:
```
1. choose_content(gamekey, [machine], is_gift=true)   # spend the pick
2. redeem_as_gift(<machine>_choice_steam, gamekey, keyindex, gift=true)   # → GiftUrl
3. record the gift URL durably (existing path)
```
If the game is ALREADY claimed (key in all_tpks), skip step 1 — it's just an existing-style redeem.

## 5. Safety model (the heart of the design)

Today's invariant: **a key burns exactly once, and a burned key's gift URL is never lost.** Choice adds a
second one-shot resource: **a monthly pick is spent exactly once.** The two-write flow creates a new
crash window between them:

| Crash point | State | Recovery |
|---|---|---|
| before `choosecontent` | nothing spent | retry cleanly |
| `choosecontent` failed | pick NOT spent (success=false ⇒ no effect) | retry cleanly |
| after `choosecontent` succeeds, before `redeemkey` | **pick spent, key not yet gifted** | the spent choice leaves the key sitting in the order's `all_tpks` → a normal `redeem_as_gift` on the next pass completes the gift. **Park + reconcile, don't re-choose.** |
| `redeemkey` failed after choose | pick spent, key un-gifted | same as above — reconcile redeems the now-present key |
| both succeeded | gift URL returned | record; done |

Design rules that fall out:
- **choosecontent is idempotent-guarded:** never call it for a game already claimed (check `all_tpks` /
  `contentChoicesMade` first) — that prevents double-spending a pick.
- **The redeem half keeps its existing burns-once discipline** (an `Unauthorized` redeem proves the key
  was untouched, etc.).
- **Reconcile learns Choice:** a parked Choice claim whose pick was spent must reconcile by *redeeming the
  now-claimed key*, never by re-choosing. This is the one real new reconcile branch.
- Like #15 (step-up) and #18 (self-login), the between-writes recovery gets **proven on a live receipt**
  before it's trusted unattended — a deliberate Ben-authorized first Choice gift.

## 6. Rollout (phased, each independently shippable — the pattern that worked all week)

1. **`choose_content` + read helper** in humble-client, unit-tested (wiremock), no fulfillment wiring yet.
2. **Discovery**: surface claimable Choice games as giftable inventory (sync + domain + storage state).
3. **Two-write gift orchestration** in fulfillment + the reconcile branch, moto-tested end to end.
4. **Live receipt**: one Ben-authorized real Choice gift, proving the between-writes recovery.
5. UI/admin surfacing as needed.

## 7. Decisions (resolved by Ben 2026-07-05)

1. **Scope: BOTH gift AND self-claim.** Self-claim is worth it — it avoids the email-lookup + several
   button clicks that gifting-to-self needs. The older stale-pick backlog (retroactive claims in 2019–2021
   months) is **deferred** — build the mechanism, circle back to bulk-claiming those later.
2. **Auto-discover ALL months** ("Auto-Discover those mother fuckers"). Sync enumerates every Choice month,
   not just recent/active. (Pagination handled in discovery.)
3. **`Game` model extension: approved** as proposed (§4.3) — the claimable-vs-claimed descriptor.
4. **Pick budget policy: confirm-then-hide-but-list.** Ben is verifying that `choices_remaining=0` truly
   means no claims left (checking April 2023 / August 2023, both `cr=0`). Once confirmed: a `cr=0` month's
   games are **not offered for gifting**, but **still listed in the admin view with a status**.
5. **First-receipt games (Ben-authorized, real):** he wants the resulting gift URLs / keys returned after
   testing —
   - **January 2026** → Nice Day for Fishing
   - **June 2026** → Construction Simulator
   - **October 2025** → Hotel Renovator

## 8. Verification

Mirror the proven approach: wiremock unit tests for `choose_content`; moto-backed handler tests for the
two-write orchestration incl. the crash-between-writes reconcile branch (assert a spent-pick-but-unredeemed
claim reconciles by redeeming, never re-choosing); then the single Ben-authorized live gift as the end-to-
end receipt. No pick is ever spent in automated tests (humble is mocked).
